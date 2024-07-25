mod events;
mod formatters;
mod lrc;
mod lrc_file_manager;
mod player;
mod server;

use std::path::PathBuf;
use std::sync::mpsc::channel;
use std::time::{Duration, Instant};

use clap::Parser;
use dbus::blocking::LocalConnection;

#[allow(unused_imports)]
use log::{debug, error, info, trace, warn};

use crate::events::{
    Event, LyricsEvent, PlaybackStatus, PlayerEvent, PlayerState, PositionSnapshot, TimedEvent,
};
use crate::formatters::format_duration;
use crate::lrc::{Lyrics, LyricsTiming};
use crate::lrc_file_manager::{get_lrc_filepath, LrcManager};
use crate::player::{get_connection_proxy, PlayerNotifications, QueryPlayerProperties};

static REFRESH_EVERY: Duration = Duration::from_millis(16);

/// Show lyrics
#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Opt {
    /// Lyrics file to use for all songs.
    /// By default, loads the .lrc file next to audio file, with the matching filename, if available.
    #[arg(short = 'l', long)]
    lyrics: Option<PathBuf>,

    /// Player to use
    #[arg(short = 'p', long)]
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

    fn on_position_advanced(&mut self, current_position: Duration) -> Option<&'a LyricsTiming> {
        if let Some(timed_text) = self.next {
            let subtract = std::cmp::min(REFRESH_EVERY / 2, timed_text.time);
            if current_position >= timed_text.time - subtract {
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

fn run(player: &str, lrc_filepath: &Option<PathBuf>) -> Option<()> {
    let server = server::run_async();

    let (sender, receiver) = channel::<TimedEvent>();

    let player_notifs = PlayerNotifications::new(sender.clone());
    player_notifs.run_async(player);

    let lrc_manager = LrcManager::new(sender);
    let lrc_manager_sender = lrc_manager.clone_sender();
    if lrc_filepath.is_some() {
        LrcManager::change_watched_path(lrc_filepath.clone(), &lrc_manager_sender);
    }
    lrc_manager.run_async();

    let c = LocalConnection::new_session().unwrap();
    let mut player_query: Option<QueryPlayerProperties<'_, LocalConnection>> = None;
    let mut lrc_state: Option<LrcTimedTextState> = None;
    let mut player_state: Option<PlayerState> = None;
    let mut lyrics: Option<Lyrics> = None;

    loop {
        let mut received_events = false;
        match receiver.recv_timeout(REFRESH_EVERY) {
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return None,
            Ok(timed_event) => {
                debug!("{:?}", timed_event);
                received_events = true;
                let instant = timed_event.instant;
                let event = timed_event.event;

                match event {
                    Event::PlayerEvent(PlayerEvent::Seeked { position }) => {
                        if let Some(ref mut ps) = player_state {
                            ps.position_snapshot = PositionSnapshot { position, instant };
                        }
                    }
                    Event::PlayerEvent(PlayerEvent::PlaybackStatusChange(
                        PlaybackStatus::Playing,
                    )) => {
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
                    Event::PlayerEvent(PlayerEvent::PlaybackStatusChange(
                        PlaybackStatus::Stopped,
                    )) => {
                        player_state = Some(PlayerState {
                            playback_status: PlaybackStatus::Stopped,
                            position_snapshot: PositionSnapshot {
                                position: Duration::from_millis(0),
                                instant,
                            },
                            metadata: None,
                        });
                    }
                    Event::PlayerEvent(PlayerEvent::PlaybackStatusChange(
                        PlaybackStatus::Paused,
                    )) => {
                        if let (Some(p), Some(q)) = (&mut player_state, &player_query) {
                            p.playback_status = PlaybackStatus::Paused;
                            p.position_snapshot = PositionSnapshot {
                                position: q.query_player_position().unwrap(),
                                instant: Instant::now(),
                            };
                        }
                    }
                    Event::PlayerEvent(PlayerEvent::MetadataChange(metadata)) => {
                        if lrc_filepath.is_none() {
                            LrcManager::change_watched_path(
                                metadata.as_ref().map(get_lrc_filepath),
                                &lrc_manager_sender,
                            );
                        }
                        if let Some(ref mut p) = player_state {
                            p.metadata = metadata;
                        }
                    }
                    Event::PlayerEvent(PlayerEvent::PlayerShutDown) => {
                        LrcManager::change_watched_path(None, &lrc_manager_sender);
                        player_state = None;
                        player_query = None;
                    }
                    Event::PlayerEvent(PlayerEvent::PlayerStarted {
                        player_owner_name: n,
                    }) => {
                        let q = QueryPlayerProperties {
                            proxy: get_connection_proxy(&c, n),
                        };
                        // TODO: This is often crashing on player restart
                        player_state = Some(q.query_player_state().unwrap());
                        player_query = Some(q);

                        if lrc_filepath.is_none() {
                            LrcManager::change_watched_path(
                                player_state
                                    .as_ref()
                                    .and_then(|p| p.metadata.as_ref().map(get_lrc_filepath)),
                                &lrc_manager_sender,
                            );
                        }
                    }
                    Event::PlayerEvent(PlayerEvent::Unknown {
                        key: unknown_key,
                        value: unknown_value,
                    }) => {
                        warn!("Unknown player event property: {unknown_key} = {unknown_value}");
                    }
                    Event::LyricsEvent(LyricsEvent::LyricsChanged { lyrics: l, .. }) => {
                        lrc_state = None; // will be asigned after event processing
                        lyrics = l;
                        server.on_lyrics_changed(lyrics.as_ref().map(|l| l.lines.clone()), &c);
                    }
                }

                debug!("player_state = {:?}", player_state);
            }
        }

        // Print new lyrics line, if needed
        if received_events {
            lrc_state = lyrics.as_ref().and_then(|l| {
                player_state
                    .as_ref()
                    .map(|p| LrcTimedTextState::new(l, p.current_position()))
            });
            let timed_text = lrc_state.as_ref().and_then(|l| l.current);
            server.on_active_lyrics_segment_changed(timed_text, &c);
        } else if let Some(ref player_state) = player_state {
            if player_state.playback_status == PlaybackStatus::Playing {
                let new_timed_text = lrc_state
                    .as_mut()
                    .and_then(|l| l.on_position_advanced(player_state.current_position()));
                // None also means that current lyrics segment should not change
                if new_timed_text.is_some() {
                    server.on_active_lyrics_segment_changed(new_timed_text, &c);
                }
            }
        }
    }
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .write_style(env_logger::WriteStyle::Auto)
        .format_module_path(false)
        .format_timestamp_nanos()
        .init();

    let opt = Opt::parse();
    let lyrics_filepath = opt.lyrics;
    if Some(false) == lyrics_filepath.as_ref().map(|fp| fp.is_file()) {
        error!("Lyrics path must be a file");
        return;
    }
    run(&opt.player, &lyrics_filepath);
}
