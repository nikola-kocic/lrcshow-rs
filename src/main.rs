mod lrc;
mod player;

use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use dbus::blocking::Connection;
use dbus::tree::Factory;
use dbus::Message;
use log::{debug, error, info, warn};
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use structopt::StructOpt;

use crate::player::{get_connection_proxy, Event, PlaybackStatus, Progress};

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

struct LyricsTiming {
    pub time: Duration,
    pub line_index: i32,           // index of line
    pub line_char_from_index: i32, // from this character in line
    pub line_char_to_index: i32,   // to this character in line
}

struct Lyrics {
    lines: Vec<String>,
    timings: Vec<LyricsTiming>,
}

impl Lyrics {
    fn new(lrc_file: lrc::LrcFile) -> Self {
        let mut lines = Vec::new();
        let mut timings = Vec::new();
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
    fn new(lrc: &'a Lyrics, progress: &Progress) -> LrcTimedTextState<'a> {
        let mut iter = lrc.timings.iter();
        let mut current = None;
        let mut next = iter.next();

        let v = progress.position() + (Instant::now() - progress.instant());

        while let Some(timing) = next {
            if timing.time > v {
                break;
            }
            current = Some(timing);
            next = iter.next();
        }
        LrcTimedTextState {
            current,
            next,
            iter,
        }
    }

    fn on_new_progress(&mut self, progress: &Progress) -> Option<(Duration, &'a LyricsTiming)> {
        if let Some(timed_text) = self.next {
            let current_duration = progress.position() + (Instant::now() - progress.instant());
            if current_duration >= (timed_text.time - (REFRESH_EVERY / 2)) {
                self.current = Some(timed_text);
                self.next = self.iter.next();
                return Some((current_duration, timed_text));
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
    fn new(lrc_filepath: PathBuf) -> Option<Self> {
        let (tx, rx) = channel();
        let watcher = create_watcher(tx, lrc_filepath.parent()?)
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

    fn new_timed_text_state<'a>(&'a self, progress: &Progress) -> LrcTimedTextState<'a> {
        LrcTimedTextState::new(&self.lyrics, progress)
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

fn get_lrc_filepath(progress: &Progress) -> Option<PathBuf> {
    if let Some(metadata) = progress.metadata() {
        let mut lrc_filepath = metadata.file_path().clone();
        lrc_filepath.set_extension("lrc");
        if lrc_filepath.is_file() {
            info!("Loading lyrics from {}", lrc_filepath.display());
            return Some(lrc_filepath);
        } else {
            warn!("Lyrics not found for {}", metadata.file_path().display());
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

fn read_events(c: &mut Connection, receiver: &Receiver<Event>) -> Option<Vec<Event>> {
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

fn run_dbus_server(
    active_lyrics_lines: Arc<Mutex<Option<Vec<String>>>>,
) -> Result<(), dbus::Error> {
    let mut c = Connection::new_session().unwrap();
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
                        .outarg::<(&str,), _>("reply"),
                    ),
            ),
    );
    tree.start_receive(&c);
    loop {
        c.process(Duration::from_millis(1000))?;
    }
}

fn run(player: &str, lrc_filepath: Option<PathBuf>) -> Option<()> {
    let mut c = Connection::new_session().unwrap();

    let active_lyrics_lines = Arc::new(Mutex::new(None));
    let active_lyrics_lines_clone = active_lyrics_lines.clone();
    thread::spawn(move || {
        run_dbus_server(active_lyrics_lines_clone).unwrap();
    });

    let on_active_lyrics_segment_changed = |timing: &LyricsTiming, c: &Connection| {
        let mut s = Message::new_signal(
            "/com/github/nikola_kocic/lrcshow_rs/Daemon",
            "com.github.nikola_kocic.lrcshow_rs.Daemon",
            "ActiveLyricsSegmentChanged",
        )
        .unwrap();
        s = s.append3(
            timing.line_index,
            timing.line_char_from_index,
            timing.line_char_to_index,
        );
        use dbus::channel::Sender;
        c.send(s).unwrap();

        info!(
            "ActiveLyricsSegmentChanged {}: {} - {}",
            timing.line_index, timing.line_char_from_index, timing.line_char_to_index
        );
    };

    let on_lyrics_changed = |c: &Connection| {
        let s = Message::new_signal(
            "/com/github/nikola_kocic/lrcshow_rs/Daemon",
            "com.github.nikola_kocic.lrcshow_rs.Daemon",
            "ActiveLyricsChanged",
        )
        .unwrap();
        use dbus::channel::Sender;
        c.send(s).unwrap();

        info!("ActiveLyricsChanged");
    };

    let (sender, receiver) = channel::<Event>();
    player::subscribe_to_player_start_stop(&c, &player, &sender).unwrap();
    let mut player_owner_name = player::subscribe(&c, &player, &sender).unwrap();

    let mut lrc = None;
    let mut lrc_state = None;
    let mut progress: Option<Progress> = None;
    let mut init = player_owner_name.is_some();

    loop {
        if init {
            init = false;
            player_owner_name = player::subscribe(&c, &player, &sender).unwrap();
            progress = Some(player::query_progress(&get_connection_proxy(
                &c,
                &player_owner_name.clone().unwrap(),
            ))
            .unwrap());  // TODO: This is often crashing on player restart
            debug!("progress = {:?}", progress);

            if let Some(progress) = progress.as_ref() {
                if let Some(filepath) =
                    get_lrc_filepath(&progress).or_else(|| lrc_filepath.clone())
                {
                    lrc = LrcManager::new(filepath);
                    lrc_state = lrc.as_ref().map(|l| l.new_timed_text_state(&progress));
                }
                {
                    let mut active_lyrics_lines = active_lyrics_lines.lock().unwrap();
                    *active_lyrics_lines = lrc.as_ref().map(|l| l.lyrics.lines.clone());
                    on_lyrics_changed(&c);
                }
            }
        }

        let events = read_events(&mut c, &receiver)?;
        let received_events = !events.is_empty();
        for event in events {
            debug!("{:?}", event);
            match event {
                Event::Seeked { position } => {
                    progress = progress
                        .map(|p| Progress::new(p.playback_status(), position, p.take_metadata()));
                }
                Event::PlaybackStatusChange(PlaybackStatus::Playing) => {
                    // position was already queryied on pause and seek
                    progress = progress.map(|p| {
                        Progress::new(PlaybackStatus::Playing, p.position(), p.take_metadata())
                    });
                }
                Event::PlaybackStatusChange(PlaybackStatus::Stopped) => {
                    progress = Some(Progress::new(
                        PlaybackStatus::Stopped,
                        Duration::from_millis(0),
                        None,
                    ));
                }
                Event::PlaybackStatusChange(PlaybackStatus::Paused) => {
                    progress = progress.as_ref().map(|p| {
                        Progress::new(
                            PlaybackStatus::Paused,
                            player::query_player_position(&get_connection_proxy(
                                &c,
                                &player_owner_name.clone().unwrap(),
                            ))
                            .unwrap(),
                            p.metadata().clone(),
                        )
                    });
                }
                Event::MetadataChange(metadata) => {
                    progress = progress
                        .map(|p| Progress::new(p.playback_status(), p.position(), metadata));
                    match progress
                        .as_ref()
                        .and_then(|p| get_lrc_filepath(&p))
                        .or_else(|| lrc_filepath.clone())
                    {
                        Some(filepath) => {
                            lrc = LrcManager::new(filepath);
                            lrc_state = lrc.as_ref().and_then(|l| {
                                progress.as_ref().map(|p| l.new_timed_text_state(&p))
                            });
                        }
                        None => {
                            lrc = None;
                            lrc_state = None;
                        }
                    }
                    {
                        let mut active_lyrics_lines = active_lyrics_lines.lock().unwrap();
                        *active_lyrics_lines = lrc.as_ref().map(|l| l.lyrics.lines.clone());
                        on_lyrics_changed(&c);

                        on_active_lyrics_segment_changed(
                            &LyricsTiming {
                                time: Duration::from_secs(0),
                                line_index: 0,
                                line_char_from_index: 0,
                                line_char_to_index: 0,
                            },
                            &c,
                        );
                    }
                }
                Event::PlayerShutDown => {
                    // return Some(());
                    progress = None;
                    player_owner_name = None;
                }
                Event::PlayerStarted => {
                    init = true;
                }
            }

            debug!("progress = {:?}", progress);
        }

        if let Some(new_lrc) = lrc.as_ref().and_then(|l| l.maybe_recreate()) {
            {
                let mut active_lyrics_lines = active_lyrics_lines.lock().unwrap();
                *active_lyrics_lines = Some(new_lrc.lyrics.lines.clone());
                on_lyrics_changed(&c);
            }
            lrc = Some(new_lrc);
            lrc_state = lrc
                .as_ref()
                .and_then(|l| progress.as_ref().map(|p| l.new_timed_text_state(&p)));
        }

        // Print new lyrics line, if needed
        if received_events {
            lrc_state = lrc
                .as_ref()
                .and_then(|l| progress.as_ref().map(|p| l.new_timed_text_state(&p)));
            if let Some(timed_text) = lrc_state.as_ref().and_then(|l| l.current) {
                on_active_lyrics_segment_changed(timed_text, &c);
            }
        } else if progress.as_ref().map(|p| p.playback_status()) == Some(PlaybackStatus::Playing) {
            if let Some((duration, timed_text)) = lrc_state
                .as_mut()
                .and_then(|l| progress.as_ref().and_then(|p| l.on_new_progress(&p)))
            {
                on_active_lyrics_segment_changed(timed_text, &c);
                debug!(
                    "Matched lyrics line at time {}, player time {}",
                    format_duration(&timed_text.time),
                    format_duration(&duration)
                );
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
