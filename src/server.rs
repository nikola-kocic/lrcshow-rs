use std::sync::{Arc, Mutex};
use std::thread;

use dbus::blocking::{Connection, LocalConnection};
use dbus::Message;
use dbus_crossroads::{Context, Crossroads};

#[allow(unused_imports)]
use log::{debug, error, info, warn};

use crate::lrc::LyricsTiming;

#[derive(Clone)]
pub struct Server {
    active_lyrics_lines: Arc<Mutex<Option<Vec<String>>>>,
    current_timing: Arc<Mutex<Option<LyricsTiming>>>,
}

impl Server {
    pub fn get_current_lyrics(&self) -> Vec<String> {
        let v = self.active_lyrics_lines.lock().unwrap();
        debug!("GetCurrentLyrics called");
        v.clone().unwrap_or_default()
    }

    pub fn get_current_lyrics_position(&self) -> (i32, i32, i32) {
        let v = self.current_timing.lock().unwrap();
        debug!("GetCurrentLyricsPosition called");
        let reply = v.as_ref().map_or((-1, -1, -1), |x| {
            (x.line_index, x.line_char_from_index, x.line_char_to_index)
        });
        reply
    }

    pub fn on_active_lyrics_segment_changed(
        &self,
        timing: Option<&LyricsTiming>,
        c: &LocalConnection,
    ) {
        {
            let mut prev_value = self.current_timing.lock().unwrap();
            if prev_value.as_ref() == timing {
                return;
            }
            info!("ActiveLyricsSegmentChanged {:?}", timing);
            *prev_value = timing.cloned();
        }

        let mut s = Message::new_signal(
            "/com/github/nikola_kocic/lrcshow_rs/Daemon",
            "com.github.nikola_kocic.lrcshow_rs.Daemon",
            "ActiveLyricsSegmentChanged",
        )
        .unwrap();

        if let Some(timing) = timing {
            s = s.append1((
                timing.line_index,
                timing.line_char_from_index,
                timing.line_char_to_index,
            ));
        } else {
            s = s.append1((-1, -1, -1));
        }

        dbus::channel::Sender::send(c, s).unwrap();
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

        info!("ActiveLyricsChanged");

        dbus::channel::Sender::send(c, s).unwrap();
    }
}

fn run_dbus_server(s: Server) -> Result<Server, dbus::Error> {
    let c = Connection::new_session()?;
    c.request_name("com.github.nikola_kocic.lrcshow_rs", false, true, false)?;
    let mut cr = Crossroads::new();

    let iface_token = cr.register("com.github.nikola_kocic.lrcshow_rs.Lyrics", |b| {
        b.method(
            "GetCurrentLyrics",
            (),
            ("reply",),
            move |_: &mut Context, server: &mut Server, ()| Ok((server.get_current_lyrics(),)),
        );
        b.method(
            "GetCurrentLyricsPosition",
            (),
            ("reply",),
            move |_: &mut Context, server: &mut Server, ()| {
                Ok((server.get_current_lyrics_position(),))
            },
        );
    });
    cr.insert(
        "/com/github/nikola_kocic/lrcshow_rs/Lyrics",
        &[iface_token],
        s,
    );
    cr.serve(&c)?;
    unreachable!()
}

pub fn run_async() -> (Server, std::thread::JoinHandle<()>) {
    let server = Server {
        active_lyrics_lines: Arc::new(Mutex::new(None)),
        current_timing: Arc::new(Mutex::new(None)),
    };
    let ret = server.clone();
    let join_handle = thread::spawn(move || {
        run_dbus_server(server).unwrap();
    });
    (ret, join_handle)
}
