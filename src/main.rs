mod events;
mod lrc;
mod lrc_file_manager;
mod player;
mod server;

use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver};
use std::time::{Duration, Instant};

use dbus::blocking::Connection;
use structopt::StructOpt;

#[allow(unused_imports)]
use log::{debug, error, info, warn};

use crate::events::{
    Event, PlaybackStatus, PlayerEvent, PlayerState, PositionSnapshot, TimedEvent,
};
use crate::lrc::{Lyrics, LyricsTiming};
use crate::lrc_file_manager::{get_lrc_filepath, LrcManager};
use crate::player::{get_connection_proxy, PlayerNotifications};

static REFRESH_EVERY: Duration = Duration::from_millis(16);

/// Show lyrics
#[derive(StructOpt, Debug)]
#[structopt(name = "lrcshow-rs")]
struct Opt {
    /// Lyrics file to use for all songs.
    /// By default .lrc file next to audio file, with the same filename, will be used, if available.
    #[structopt(short = "l", long, parse(from_os_str))]
    lyrics: Option<PathBuf>,

    /// Player to use
    #[structopt(short = "p", long)]
    player: String,
}

struct LrcTimedTextState<'a> {
    current: Option<&'a LyricsTiming>,
    next: Option<&'a LyricsTiming>,
    iter: std::slice::Iter<'a, LyricsTiming>,
}

impl<'a> LrcTimedTextState<'a> {
    fn new(lrc: &'a Lyrics, current_position: Duration) -> LrcTimedTextState<'a> {
        let mut iter = lrc.timings.iter();
        let mut current = iter.next();
        let mut next = iter.next();

        while let Some(timing) = next {
            if timing.time > current_position {
                break;
            }
            current = Some(timing);
            next = iter.next();
        }
        debug!(
            "LrcTimedTextState::new; current_position = {:?}, current = {:?}",
            current_position, current
        );
        LrcTimedTextState {
            current,
            next,
            iter,
        }
    }

    fn on_new_progress(&mut self, current_position: Duration) -> Option<&'a LyricsTiming> {
        if let Some(timed_text) = self.next {
            if current_position >= (timed_text.time - (REFRESH_EVERY / 2)) {
                self.current = Some(timed_text);
                self.next = self.iter.next();
                debug!(
                    "Matched lyrics line at time {}, player time {}",
                    format_duration(&timed_text.time),
                    format_duration(&current_position)
                );
                return Some(timed_text);
            }
        }
        None
    }
}

fn format_duration(duration: &Duration) -> String {
    let total_seconds = duration.as_secs_f64();
    let minutes = ((total_seconds / 60.0).floor() as i32) % 60;
    let seconds = total_seconds - f64::from(minutes * 60);
    format!("{:02}:{:05.2}", minutes, seconds)
}

fn read_events(receiver: &Receiver<TimedEvent>) -> Option<Vec<TimedEvent>> {
    let mut v = Vec::new();
    loop {
        match receiver.recv_timeout(REFRESH_EVERY) {
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => break,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return None,
            Ok(event) => v.push(event),
        }
    }
    Some(v)
}

fn run(player: &str, lrc_filepath: Option<PathBuf>) -> Option<()> {
    let server = server::Server::new();
    server.run_async();

    let (sender, receiver) = channel::<TimedEvent>();

    let player_notifs = PlayerNotifications::new(sender);
    player_notifs.run_async(&player);
    let c = Connection::new_session().unwrap();

    let mut player_owner_name: Option<String> = None;
    let mut lrc = None;
    let mut lrc_state: Option<LrcTimedTextState> = None;
    let mut player_state: Option<PlayerState> = None;

    loop {
        let timed_events = read_events(&receiver)?;
        let received_events = !timed_events.is_empty();
        for timed_event in timed_events {
            debug!("{:?}", timed_event);
            let instant = timed_event.instant;
            let event = timed_event.event;

            match event {
                Event::PlayerEvent(PlayerEvent::Seeked { position }) => {
                    if let Some(ref mut ps) = player_state {
                        ps.position_snapshot = PositionSnapshot { position, instant };
                    }
                }
                Event::PlayerEvent(PlayerEvent::PlaybackStatusChange(PlaybackStatus::Playing)) => {
                    // position was already queried on pause and seek
                    player_state = player_state.map(|p| PlayerState {
                        playback_status: PlaybackStatus::Playing,
                        position_snapshot: PositionSnapshot {
                            position: p.position_snapshot.position,
                            instant,
                        },
                        metadata: p.metadata,
                    });
                }
                Event::PlayerEvent(PlayerEvent::PlaybackStatusChange(PlaybackStatus::Stopped)) => {
                    player_state = Some(PlayerState {
                        playback_status: PlaybackStatus::Stopped,
                        position_snapshot: PositionSnapshot {
                            position: Duration::from_millis(0),
                            instant,
                        },
                        metadata: None,
                    });
                }
                Event::PlayerEvent(PlayerEvent::PlaybackStatusChange(PlaybackStatus::Paused)) => {
                    if let Some(p) = player_state {
                        player_state = Some(PlayerState {
                            playback_status: PlaybackStatus::Paused,
                            position_snapshot: PositionSnapshot {
                                position: player::query_player_position(&get_connection_proxy(
                                    &c,
                                    &player_owner_name.clone().unwrap(),
                                ))
                                .unwrap(),
                                instant: Instant::now(),
                            },
                            metadata: p.metadata,
                        });
                    }
                }
                Event::PlayerEvent(PlayerEvent::MetadataChange(metadata)) => {
                    if let Some(ref mut p) = player_state {
                        p.metadata = metadata;
                    }
                    let mut timing = None;
                    match player_state
                        .as_ref()
                        .and_then(|p| get_lrc_filepath(&p.metadata))
                        .or_else(|| lrc_filepath.clone())
                    {
                        Some(filepath) => {
                            lrc = LrcManager::new(filepath);
                            lrc_state = lrc.as_ref().and_then(|l| {
                                player_state.as_ref().map(|p| {
                                    LrcTimedTextState::new(&l.lyrics, p.current_position())
                                })
                            });
                            timing = Some(LyricsTiming {
                                time: Duration::from_secs(0),
                                line_index: 0,
                                line_char_from_index: 0,
                                line_char_to_index: 0,
                            });
                        }
                        None => {
                            lrc = None;
                            lrc_state = None;
                        }
                    }
                    server.on_lyrics_changed(lrc.as_ref().map(|l| l.lyrics.lines.clone()), &c);
                    server.on_active_lyrics_segment_changed(timing, &c);
                }
                Event::PlayerEvent(PlayerEvent::PlayerShutDown) => {
                    // return Some(());
                    player_state = None;
                    player_owner_name = None;
                }
                Event::PlayerEvent(PlayerEvent::PlayerStarted {
                    player_owner_name: n,
                }) => {
                    player_owner_name = Some(n);

                    player_state = Some(
                        player::query_player_state(&get_connection_proxy(
                            &c,
                            &player_owner_name.clone().unwrap(),
                        ))
                        .unwrap(),
                    ); // TODO: This is often crashing on player restart
                    debug!("player_state = {:?}", player_state);

                    if let Some(player_state) = player_state.as_ref() {
                        if let Some(filepath) = get_lrc_filepath(&player_state.metadata)
                            .or_else(|| lrc_filepath.clone())
                        {
                            lrc = LrcManager::new(filepath);
                            lrc_state = lrc.as_ref().map(|l| {
                                LrcTimedTextState::new(&l.lyrics, player_state.current_position())
                            });
                        }
                        server.on_lyrics_changed(lrc.as_ref().map(|l| l.lyrics.lines.clone()), &c);
                        let timed_text = lrc_state.as_ref().and_then(|l| l.current.cloned());
                        server.on_active_lyrics_segment_changed(timed_text, &c);
                    }
                }
            }

            debug!("player_state = {:?}", player_state);
        }

        if let Some(new_lrc) = lrc.as_ref().and_then(|l| l.maybe_recreate()) {
            server.on_lyrics_changed(Some(new_lrc.lyrics.lines.clone()), &c);
            lrc = Some(new_lrc);
            lrc_state = lrc.as_ref().and_then(|l| {
                player_state
                    .as_ref()
                    .map(|p| LrcTimedTextState::new(&l.lyrics, p.current_position()))
            });
        }

        // Print new lyrics line, if needed
        if received_events {
            lrc_state = lrc.as_ref().and_then(|l| {
                player_state
                    .as_ref()
                    .map(|p| LrcTimedTextState::new(&l.lyrics, p.current_position()))
            });
            let timed_text = lrc_state.as_ref().and_then(|l| l.current);
            server.on_active_lyrics_segment_changed(timed_text.cloned(), &c);
        } else if player_state.as_ref().map(|p| p.playback_status) == Some(PlaybackStatus::Playing)
        {
            if let Some(new_timed_text) = lrc_state.as_mut().and_then(|l| {
                player_state
                    .as_ref()
                    .and_then(|p| l.on_new_progress(p.current_position()))
            }) {
                server.on_active_lyrics_segment_changed(Some(new_timed_text.clone()), &c);
            }
        }
    }
}

fn main() {
    env_logger::from_env(env_logger::Env::default().default_filter_or("info"))
        .write_style(env_logger::WriteStyle::Auto)
        .format_module_path(false)
        .format_timestamp_nanos()
        .init();

    let opt = Opt::from_args();
    let lyrics_filepath = opt.lyrics;
    if Some(false) == lyrics_filepath.as_ref().map(|fp| fp.is_file()) {
        error!("Lyrics path must be a file");
        return;
    }
    run(&opt.player, lyrics_filepath);
}
