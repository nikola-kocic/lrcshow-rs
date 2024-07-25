use std::sync::{Arc, Mutex};
use std::thread;

use dbus::blocking::SyncConnection;
use dbus::Message;
use dbus_crossroads::{Context, Crossroads};

#[allow(unused_imports)]
use log::{debug, error, info, warn};

use crate::lrc::LyricsTiming;

#[derive(Clone)]
struct ServerData {
    active_lyrics_lines: Arc<Mutex<Option<Vec<String>>>>,
    current_timing: Arc<Mutex<Option<LyricsTiming>>>,
}

#[derive(Clone)]
pub struct Server {
    connection: Arc<SyncConnection>,
    data: ServerData,
}

impl ServerData {
    fn get_current_lyrics(&self) -> Vec<String> {
        let v = self.active_lyrics_lines.lock().unwrap();
        debug!("GetCurrentLyrics called");
        v.clone().unwrap_or_default()
    }

    fn get_current_lyrics_position(&self) -> (i32, i32, i32) {
        let v = self.current_timing.lock().unwrap();
        debug!("GetCurrentLyricsPosition called");
        let reply = v.as_ref().map_or((-1, -1, -1), |x| {
            (x.line_index, x.line_char_from_index, x.line_char_to_index)
        });
        reply
    }
}

impl Server {
    pub fn on_active_lyrics_segment_changed(&self, timing: Option<&LyricsTiming>) {
        {
            let mut prev_value = self.data.current_timing.lock().unwrap();
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

        dbus::channel::Sender::send(self.connection.as_ref(), s).unwrap();
    }

    pub fn on_lyrics_changed(&self, lines: Option<Vec<String>>) {
        {
            *self.data.active_lyrics_lines.lock().unwrap() = lines;
        }
        let s = Message::new_signal(
            "/com/github/nikola_kocic/lrcshow_rs/Daemon",
            "com.github.nikola_kocic.lrcshow_rs.Daemon",
            "ActiveLyricsChanged",
        )
        .unwrap();

        info!("ActiveLyricsChanged");

        dbus::channel::Sender::send(self.connection.as_ref(), s).unwrap();
    }
}

fn run_dbus_server(s: Server) -> Result<Server, dbus::Error> {
    s.connection
        .request_name("com.github.nikola_kocic.lrcshow_rs", false, true, false)?;
    let cr = Arc::new(Mutex::new(Crossroads::new()));
    {
        let mut cr_lock = cr.lock().unwrap();

        let iface_token = cr_lock.register("com.github.nikola_kocic.lrcshow_rs.Lyrics", |b| {
            b.method(
                "GetCurrentLyrics",
                (),
                ("reply",),
                move |_: &mut Context, server: &mut ServerData, ()| {
                    Ok((server.get_current_lyrics(),))
                },
            );
            b.method(
                "GetCurrentLyricsPosition",
                (),
                ("reply",),
                move |_: &mut Context, server: &mut ServerData, ()| {
                    Ok((server.get_current_lyrics_position(),))
                },
            );
        });
        cr_lock.insert(
            "/com/github/nikola_kocic/lrcshow_rs/Lyrics",
            &[iface_token],
            s.data,
        );
    }

    use dbus::channel::MatchingReceiver;
    s.connection.start_receive(
        dbus::message::MatchRule::new_method_call(),
        Box::new(move |msg, conn| {
            let mut cr_lock = cr.lock().unwrap();
            cr_lock.handle_message(msg, conn).unwrap();
            true
        }),
    );

    // Serve clients forever.
    loop {
        s.connection
            .process(std::time::Duration::from_millis(1000))?;
    }
}

pub fn run_async() -> (Server, std::thread::JoinHandle<()>) {
    let server = Server {
        connection: Arc::new(SyncConnection::new_session().unwrap()),
        data: ServerData {
            active_lyrics_lines: Arc::new(Mutex::new(None)),
            current_timing: Arc::new(Mutex::new(None)),
        },
    };
    let ret = server.clone();
    let join_handle = thread::spawn(move || {
        run_dbus_server(server).unwrap();
    });
    (ret, join_handle)
}
