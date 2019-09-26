mod lrc;
mod player;

use std::path::{Path, PathBuf};
use std::sync::mpsc::channel;
use std::time::{Duration, Instant};

use dbus::{BusType, Connection, Message};
use log::{debug, error, info, warn};
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use structopt::StructOpt;

use crate::player::{Event, PlaybackStatus, Progress};

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
    current: Option<&'a lrc::TimedText>,
    next: Option<&'a lrc::TimedText>,
    iter: std::slice::Iter<'a, lrc::TimedText>,
}

impl<'a> LrcTimedTextState<'a> {
    fn new(lrc: &'a lrc::LrcFile, progress: &Progress) -> LrcTimedTextState<'a> {
        let mut iter = lrc.timed_texts.iter();
        let mut current = None;
        let mut next = iter.next();

        let v = progress.position() + (Instant::now() - progress.instant());

        while let Some(timed_text) = next {
            if timed_text.position > v {
                break;
            }
            current = Some(timed_text);
            next = iter.next();
        }
        LrcTimedTextState {
            current,
            next,
            iter,
        }
    }

    fn on_new_progress(&mut self, progress: &Progress) -> Option<(Duration, &'a lrc::TimedText)> {
        if let Some(timed_text) = self.next {
            let current_duration = progress.position() + (Instant::now() - progress.instant());
            if current_duration >= (timed_text.position - (REFRESH_EVERY / 2)) {
                self.current = Some(timed_text);
                self.next = self.iter.next();
                return Some((current_duration, self.current.unwrap()));
            }
        }
        None
    }
}

struct LrcManager {
    lrc_file: lrc::LrcFile,
    rx: std::sync::mpsc::Receiver<notify::DebouncedEvent>,
    _watcher: RecommendedWatcher,
    lrc_filepath: PathBuf,
}

impl LrcManager {
    fn new(lrc_filepath: PathBuf) -> LrcManager {
        let (tx, rx) = channel();
        let watcher = create_watcher(tx, lrc_filepath.parent().unwrap());
        let lrc_file = lrc::parse_lrc_file(&lrc_filepath).unwrap();
        LrcManager {
            lrc_file,
            rx,
            _watcher: watcher,
            lrc_filepath,
        }
    }

    fn new_timed_text_state<'a>(&'a self, progress: &Progress) -> LrcTimedTextState<'a> {
        LrcTimedTextState::new(&self.lrc_file, progress)
    }

    fn maybe_recreate(&self) -> Option<LrcManager> {
        if let Ok(x) = self.rx.try_recv() {
            match x {
                notify::DebouncedEvent::Create(path) | notify::DebouncedEvent::Write(path) => {
                    if path == self.lrc_filepath {
                        info!("Reloading lyrics");
                        return Some(LrcManager::new(self.lrc_filepath.clone()));
                    }
                }
                _ => {}
            }
        }
        None
    }
}

fn create_watcher(
    tx: std::sync::mpsc::Sender<notify::DebouncedEvent>,
    folderpath: &Path,
) -> RecommendedWatcher {
    let mut watcher: RecommendedWatcher = Watcher::new(tx, Duration::from_millis(100)).unwrap();
    watcher.watch(folderpath, RecursiveMode::Recursive).unwrap();
    watcher
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

fn run(player: &str, lrc_filepath: Option<PathBuf>) -> Option<()> {
    let c = Connection::get_private(BusType::Session).unwrap();

    let on_active_lyrics_line_changed = |text: &str| {
        let mut s = Message::new_signal(
            "/com/github/nikola_kocic/lrcshow_rs/Daemon",
            "com.github.nikola_kocic.lrcshow_rs.Daemon",
            "ActiveLyricsLineChanged",
        )
        .unwrap();
        s = s.append1(text);
        c.send(s).unwrap();

        println!("{}", text);
    };

    let player_owner_name = player::subscribe(&c, &player)?;

    let mut progress = player::query_progress(&c, &player_owner_name);
    debug!("progress = {:?}", progress);

    let mut lyrics_filepath = get_lrc_filepath(&progress).or_else(|| lrc_filepath.clone());
    let mut lrc = lyrics_filepath.map(LrcManager::new);
    let mut lrc_state = lrc.as_ref().map(|l| l.new_timed_text_state(&progress));

    for i in c.iter(REFRESH_EVERY.as_millis() as i32) {
        let events = player::create_events(&i, &player_owner_name);
        for event in &events {
            debug!("{:?}", event);
            match event {
                Event::Seeked { position } => {
                    progress =
                        Progress::new(progress.metadata(), progress.playback_status(), *position);
                }
                Event::PlaybackStatusChange(PlaybackStatus::Playing) => {
                    // position was already queryied on pause and seek
                    progress = Progress::new(
                        progress.metadata(),
                        PlaybackStatus::Playing,
                        progress.position(),
                    );
                }
                Event::PlaybackStatusChange(PlaybackStatus::Stopped) => {
                    progress =
                        Progress::new(None, PlaybackStatus::Stopped, Duration::from_millis(0));
                }
                Event::PlaybackStatusChange(PlaybackStatus::Paused) => {
                    progress = Progress::new(
                        progress.metadata(),
                        PlaybackStatus::Paused,
                        player::query_player_position(&c, &player_owner_name),
                    );
                }
                Event::MetadataChange(metadata) => {
                    progress = Progress::new(
                        Some(metadata.clone()),
                        progress.playback_status(),
                        progress.position(),
                    );
                    // TODO: Do this only if file changed
                    on_active_lyrics_line_changed("");
                    lyrics_filepath = get_lrc_filepath(&progress).or_else(|| lrc_filepath.clone());
                    lrc = lyrics_filepath.map(LrcManager::new);
                    lrc_state = lrc.as_ref().map(|l| l.new_timed_text_state(&progress));
                }
                Event::PlayerShutDown => {
                    return Some(());
                }
            }

            debug!("progress = {:?}", progress);
        }

        if let Some(new_lrc) = lrc.as_ref().and_then(|l| l.maybe_recreate()) {
            lrc = Some(new_lrc);
            lrc_state = lrc.as_ref().map(|l| l.new_timed_text_state(&progress));
        }

        // Print new lyrics line, if needed
        if !events.is_empty() {
            lrc_state = lrc.as_ref().map(|l| l.new_timed_text_state(&progress));
            if let Some(timed_text) = lrc_state.as_ref().and_then(|l| l.current) {
                on_active_lyrics_line_changed(&timed_text.text);
            }
        } else if progress.playback_status() == PlaybackStatus::Playing {
            if let Some((duration, timed_text)) = lrc_state
                .as_mut()
                .and_then(|l| l.on_new_progress(&progress))
            {
                on_active_lyrics_line_changed(&timed_text.text);
                debug!(
                    "Matched lyrics line at time {}, player time {}",
                    format_duration(&timed_text.position),
                    format_duration(&duration)
                );
            }
        }
    }
    Some(())
}

fn main() {
    env_logger::from_env(env_logger::Env::default().default_filter_or("info"))
        .write_style(env_logger::WriteStyle::Auto)
        .default_format_module_path(false)
        .default_format_timestamp_nanos(true)
        .init();

    let opt = Opt::from_args();
    let lyrics_filepath = opt.lyrics;
    if Some(false) == lyrics_filepath.as_ref().map(|fp| fp.is_file()) {
        error!("Lyrics path must be a file");
        return;
    }
    run(&opt.player, lyrics_filepath);
}
