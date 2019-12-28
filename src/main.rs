mod events;
mod lrc;
mod player;
mod server;

use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::{Duration, Instant};

use dbus::blocking::Connection;
use log::{debug, error, info, warn};
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use structopt::StructOpt;

use crate::events::{
    Event, LyricsTiming, Metadata, PlaybackStatus, PlayerEvent, PlayerState, PositionSnapshot,
    TimedEvent,
};
use crate::player::get_connection_proxy;

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

struct Lyrics {
    lines: Vec<String>,
    timings: Vec<LyricsTiming>,
}

impl Lyrics {
    fn new(lrc_file: lrc::LrcFile) -> Self {
        let mut lines = Vec::new();
        let mut timings = Vec::new();

        if !lrc_file.timed_texts_lines.is_empty() {
            timings.push(LyricsTiming {
                time: Duration::from_secs(0),
                line_index: 0,
                line_char_from_index: 0,
                line_char_to_index: 0,
            });
        }

        for (line_index, timed_text_line) in (0i32..).zip(lrc_file.timed_texts_lines) {
            lines.push(timed_text_line.text);
            for timing in timed_text_line.timings {
                timings.push(LyricsTiming {
                    time: timing.time,
                    line_index,
                    line_char_from_index: timing.line_char_from_index,
                    line_char_to_index: timing.line_char_to_index,
                })
            }
        }
        Lyrics { lines, timings }
    }
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

struct LrcManager {
    lyrics: Lyrics,
    rx: std::sync::mpsc::Receiver<notify::DebouncedEvent>,
    _watcher: RecommendedWatcher,
    lrc_filepath: PathBuf,
}

impl LrcManager {
    fn create_watcher(
        tx: Sender<notify::DebouncedEvent>,
        folder_path: &Path,
    ) -> Result<RecommendedWatcher, String> {
        RecommendedWatcher::new(tx, Duration::from_millis(100))
            .and_then(|mut watcher| {
                watcher.watch(folder_path, RecursiveMode::Recursive)?;
                Ok(watcher)
            })
            .map_err(|e| e.to_string())
    }

    fn new(lrc_filepath: PathBuf) -> Option<Self> {
        let (tx, rx) = channel();
        let watcher = Self::create_watcher(tx, lrc_filepath.parent()?)
            .map_err(|e| error!("Creating watched failed: {}", e))
            .ok()?;
        let lrc_file = lrc::parse_lrc_file(&lrc_filepath)
            .map_err(|e| error!("Parsing lrc file failed: {}", e))
            .ok()?;
        debug!("lrc_file = {:?}", lrc_file);
        let lyrics = Lyrics::new(lrc_file);
        Some(LrcManager {
            lyrics,
            rx,
            _watcher: watcher,
            lrc_filepath,
        })
    }

    fn new_timed_text_state(&self, current_position: Duration) -> LrcTimedTextState {
        LrcTimedTextState::new(&self.lyrics, current_position)
    }

    fn should_recreate(&self) -> bool {
        self.rx
            .try_recv()
            .ok()
            .map(|event| match event {
                notify::DebouncedEvent::Create(path) | notify::DebouncedEvent::Write(path) => {
                    path == self.lrc_filepath
                }
                _ => false,
            })
            .unwrap_or(false)
    }

    fn maybe_recreate(&self) -> Option<LrcManager> {
        if self.should_recreate() {
            info!("Reloading lyrics");
            LrcManager::new(self.lrc_filepath.clone())
        } else {
            None
        }
    }
}

fn get_lrc_filepath(metadata: &Option<Metadata>) -> Option<PathBuf> {
    if let Some(metadata) = metadata {
        let mut lrc_filepath = metadata.file_path.clone();
        lrc_filepath.set_extension("lrc");
        if lrc_filepath.is_file() {
            info!("Loading lyrics from {}", lrc_filepath.display());
            return Some(lrc_filepath);
        } else {
            warn!("Lyrics not found for {}", metadata.file_path.display());
        }
    }
    None
}

fn format_duration(duration: &Duration) -> String {
    let total_seconds = duration.as_secs_f64();
    let minutes = ((total_seconds / 60.0).floor() as i32) % 60;
    let seconds = total_seconds - f64::from(minutes * 60);
    format!("{:02}:{:05.2}", minutes, seconds)
}

fn read_events(c: &mut Connection, receiver: &Receiver<TimedEvent>) -> Option<Vec<TimedEvent>> {
    let mut v = Vec::new();
    c.process(REFRESH_EVERY).unwrap();
    loop {
        match receiver.try_recv() {
            Err(std::sync::mpsc::TryRecvError::Empty) => break,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => return None,
            Ok(event) => v.push(event),
        }
    }
    Some(v)
}

fn run(player: &str, lrc_filepath: Option<PathBuf>) -> Option<()> {
    let mut c = Connection::new_session().unwrap();
    let server = server::Server::new();
    server.run_async();

    let (sender, receiver) = channel::<TimedEvent>();
    player::subscribe_to_player_start_stop(&c, &player, &sender).unwrap();
    let mut player_owner_name = player::subscribe(&c, &player, &sender).unwrap();

    let mut lrc = None;
    let mut lrc_state: Option<LrcTimedTextState> = None;
    let mut player_state: Option<PlayerState> = None;
    let mut init = player_owner_name.is_some();

    loop {
        if init {
            init = false;
            player_owner_name = player::subscribe(&c, &player, &sender).unwrap();
            player_state = Some(
                player::query_player_state(&get_connection_proxy(
                    &c,
                    &player_owner_name.clone().unwrap(),
                ))
                .unwrap(),
            ); // TODO: This is often crashing on player restart
            debug!("player_state = {:?}", player_state);

            if let Some(player_state) = player_state.as_ref() {
                if let Some(filepath) =
                    get_lrc_filepath(&player_state.metadata).or_else(|| lrc_filepath.clone())
                {
                    lrc = LrcManager::new(filepath);
                    lrc_state = lrc
                        .as_ref()
                        .map(|l| l.new_timed_text_state(player_state.current_position()));
                }
                server.on_lyrics_changed(lrc.as_ref().map(|l| l.lyrics.lines.clone()), &c);
                let timed_text = lrc_state.as_ref().and_then(|l| l.current.cloned());
                server.on_active_lyrics_segment_changed(timed_text, &c);
            }
        }

        let timed_events = read_events(&mut c, &receiver)?;
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
                                player_state
                                    .as_ref()
                                    .map(|p| l.new_timed_text_state(p.current_position()))
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
                Event::PlayerEvent(PlayerEvent::PlayerStarted) => {
                    init = true;
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
                    .map(|p| l.new_timed_text_state(p.current_position()))
            });
        }

        // Print new lyrics line, if needed
        if received_events {
            lrc_state = lrc.as_ref().and_then(|l| {
                player_state
                    .as_ref()
                    .map(|p| l.new_timed_text_state(p.current_position()))
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
