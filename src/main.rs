mod lrc;
mod player;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use dbus::{BusType, Connection};
use structopt::StructOpt;

use crate::player::{Event, PlaybackStatus, Progress};

use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use std::sync::mpsc::channel;

/// Show lyrics
#[derive(StructOpt, Debug)]
#[structopt(name = "lrcshow-rs")]
struct Opt {
    /// Lyrics file to use
    #[structopt(short = "l", long, parse(from_os_str))]
    lyrics: PathBuf,

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
        let mut current = iter.next();
        let mut next = current;

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

    fn on_new_progress(&mut self, progress: &Progress) -> Option<&'a lrc::TimedText> {
        if let Some(timed_text) = self.next {
            let v = progress.position() + (Instant::now() - progress.instant());
            if v >= timed_text.position {
                self.current = Some(timed_text);
                self.next = self.iter.next();
                return self.current;
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
                        eprintln!("Reloading lyrics");
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

fn run(player: &str, lrc_filepath: &PathBuf) -> Option<()> {
    // eprintln!("lrc = {:?}", lrc);
    let c = Connection::get_private(BusType::Session).unwrap();

    let player_owner_name = player::subscribe(&c, &player)?;

    let mut progress = player::query_progress(&c, &player_owner_name);
    // eprintln!("progress = {:?}", progress);

    let mut lrc = LrcManager::new(lrc_filepath.clone());
    let mut lrc_state = lrc.new_timed_text_state(&progress);

    for i in c.iter(16) {
        let events = player::create_events(&i, &player_owner_name);
        for event in &events {
            eprintln!("{:?}", event);
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
                }
                Event::PlayerShutDown => {
                    return Some(());
                }
            }

            // eprintln!("progress = {:?}", progress);
        }

        if let Some(new_lrc) = lrc.maybe_recreate() {
            lrc = new_lrc;
            lrc_state = lrc.new_timed_text_state(&progress);
        }

        if !events.is_empty() {
            lrc_state = lrc.new_timed_text_state(&progress);
            if let Some(timed_text) = lrc_state.current {
                println!("{}", timed_text.text);
            }
        } else if progress.playback_status() == PlaybackStatus::Playing {
            if let Some(timed_text) = lrc_state.on_new_progress(&progress) {
                println!("{}", timed_text.text);
            }
        }
    }
    Some(())
}

fn main() {
    let opt = Opt::from_args();
    let lyrics_filepath = opt.lyrics;
    if !lyrics_filepath.is_file() {
        eprintln!("Lyrics path must be a file");
        return;
    }
    run(&opt.player, &lyrics_filepath);
}
