use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use dbus::blocking::LocalConnection;
use dbus::Message;
use dbus_tree::Factory;

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
        let c = LocalConnection::new_session().unwrap();
        c.request_name("com.github.nikola_kocic.lrcshow_rs", false, true, false)?;
        let f = Factory::new_fn::<()>();

        let tree = f.tree(()).add(
            f.object_path("/com/github/nikola_kocic/lrcshow_rs/Lyrics", ())
                .introspectable()
                .add(
                    f.interface("com.github.nikola_kocic.lrcshow_rs.Lyrics", ())
                        .add_m(
                            f.method("GetCurrentLyrics", (), move |m| {
                                let v = active_lyrics_lines.lock().unwrap();
                                debug!("GetCurrentLyrics called");
                                Ok(vec![m
                                    .msg
                                    .method_return()
                                    .append1(v.as_ref().map(|x| x.as_slice()).unwrap_or(&[]))])
                            })
                            .outarg::<&str, _>("reply"),
                        )
                        .add_m(
                            f.method("GetCurrentLyricsPosition", (), move |m| {
                                let v = current_timing.lock().unwrap();
                                debug!("GetCurrentLyricsPosition called");
                                Ok(vec![m.msg.method_return().append1(
                                    v.as_ref()
                                        .map(|x| {
                                            (
                                                x.line_index,
                                                x.line_char_from_index,
                                                x.line_char_to_index,
                                            )
                                        })
                                        .unwrap_or((-1, -1, -1)),
                                )])
                            })
                            .outarg::<(i32, i32, i32, i32), _>("reply"),
                        ),
                ),
        );
        tree.start_receive(&c);
        loop {
            c.process(Duration::from_millis(1000))?;
        }
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
