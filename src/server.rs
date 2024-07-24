use std::sync::{Arc, Mutex};
use std::thread;

use dbus::blocking::{Connection, LocalConnection};
use dbus::Message;
use dbus_crossroads::{Context, Crossroads};

#[allow(unused_imports)]
use log::{debug, error, info, warn};

use crate::lrc::LyricsTiming;

pub struct Server {
    active_lyrics_lines: Arc<Mutex<Option<Vec<String>>>>,
    current_timing: Arc<Mutex<Option<LyricsTiming>>>,
}

impl Server {
    pub fn new() -> Self {
        Self {
            active_lyrics_lines: Arc::new(Mutex::new(None)),
            current_timing: Arc::new(Mutex::new(None)),
        }
    }

    fn run_dbus_server(
        active_lyrics_lines: Arc<Mutex<Option<Vec<String>>>>,
        current_timing: Arc<Mutex<Option<LyricsTiming>>>,
    ) -> Result<(), dbus::Error> {
        let c = Connection::new_session()?;
        c.request_name("com.github.nikola_kocic.lrcshow_rs", false, true, false)?;
        let mut cr = Crossroads::new();

        let iface_token = cr.register("com.github.nikola_kocic.lrcshow_rs.Lyrics", |b| {
            b.method(
                "GetCurrentLyrics",
                (),
                ("reply",),
                move |_: &mut Context, _: &mut (), ()| -> Result<(Vec<String>,), dbus::MethodErr> {
                    let v = active_lyrics_lines.lock().unwrap();
                    debug!("GetCurrentLyrics called");
                    let reply = v.clone().unwrap_or_default();
                    Ok((reply,))
                },
            );
            b.method(
                "GetCurrentLyricsPosition",
                (),
                ("reply",),
                move |_: &mut Context,
                      _: &mut (),
                      ()|
                      -> Result<((i32, i32, i32),), dbus::MethodErr> {
                    let v = current_timing.lock().unwrap();
                    debug!("GetCurrentLyricsPosition called");
                    let reply = v
                        .as_ref()
                        .map(|x| (x.line_index, x.line_char_from_index, x.line_char_to_index))
                        .unwrap_or((-1, -1, -1));
                    Ok((reply,))
                },
            );
        });
        cr.insert(
            "/com/github/nikola_kocic/lrcshow_rs/Lyrics",
            &[iface_token],
            (),
        );
        cr.serve(&c)?;
        unreachable!()
    }

    pub fn on_active_lyrics_segment_changed(
        &self,
        timing: Option<&LyricsTiming>,
        c: &LocalConnection,
    ) {
        let mut s = Message::new_signal(
            "/com/github/nikola_kocic/lrcshow_rs/Daemon",
            "com.github.nikola_kocic.lrcshow_rs.Daemon",
            "ActiveLyricsSegmentChanged",
        )
        .unwrap();
        if let Some(timing) = &timing {
            s = s.append1((
                timing.line_index,
                timing.line_char_from_index,
                timing.line_char_to_index,
            ));
        } else {
            s = s.append1((-1, -1, -1));
        }

        let value_changed = {
            let mut prev_value = self.current_timing.lock().unwrap();
            if prev_value.as_ref() != timing {
                info!("ActiveLyricsSegmentChanged {:?}", timing);
                *prev_value = timing.cloned();
                true
            } else {
                false
            }
        };

        if value_changed {
            use dbus::channel::Sender;
            c.send(s).unwrap();
        }
    }

    pub fn on_lyrics_changed(&self, lines: Option<Vec<String>>, c: &LocalConnection) {
        {
            *self.active_lyrics_lines.lock().unwrap() = lines;
        }
        let s = Message::new_signal(
            "/com/github/nikola_kocic/lrcshow_rs/Daemon",
            "com.github.nikola_kocic.lrcshow_rs.Daemon",
            "ActiveLyricsChanged",
        )
        .unwrap();
        use dbus::channel::Sender;
        c.send(s).unwrap();

        info!("ActiveLyricsChanged");
    }

    pub fn run_async(&self) {
        let active_lyrics_lines_clone = self.active_lyrics_lines.clone();
        let current_timing_clone = self.current_timing.clone();
        thread::spawn(move || {
            Self::run_dbus_server(active_lyrics_lines_clone, current_timing_clone).unwrap();
        });
    }
}
