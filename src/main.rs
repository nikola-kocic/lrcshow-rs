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

fn run(player: &str, lrc_filepath: &Path) {
    let (tx, rx) = channel();
    let mut watcher: RecommendedWatcher = Watcher::new(tx, Duration::from_millis(100)).unwrap();
    watcher
        .watch(lrc_filepath.parent().unwrap(), RecursiveMode::Recursive)
        .unwrap();

    // eprintln!("lrc = {:?}", lrc);
    let c = Connection::get_private(BusType::Session).unwrap();

    let player_owner_name = player::subscribe(&c, &player).unwrap();

    let mut progress = player::query_progress(&c, &player_owner_name);
    // eprintln!("progress = {:?}", progress);

    let mut lrc = lrc::parse_lrc_file(lrc_filepath).unwrap();
    let mut lrc_state = LrcTimedTextState::new(&lrc, &progress);

    for i in c.iter(16) {
        let events = player::create_events(&i, &player_owner_name);
        for event in &events {
            eprintln!("{:?}", event);
            match event {
                Event::Seeked { position } => {
                    progress = Progress::new(progress.playback_status(), *position);
                }
                Event::PlaybackStatusChange(PlaybackStatus::Playing) => {
                    // position was already queryied on pause and seek
                    progress = Progress::new(PlaybackStatus::Playing, progress.position());
                }
                Event::PlaybackStatusChange(playback_status) => {
                    progress = Progress::new(
                        *playback_status,
                        player::query_player_position(&c, &player_owner_name),
                    );
                }
                Event::PlayerShutDown => {
                    return;
                }
            }

            // eprintln!("progress = {:?}", progress);
        }

        if let Ok(x) = rx.try_recv() {
            match x {
                notify::DebouncedEvent::Create(path) | notify::DebouncedEvent::Write(path) => {
                    if path == *lrc_filepath {
                        eprintln!("Reloading lyrics");
                        match lrc::parse_lrc_file(lrc_filepath) {
                            Ok(new_lrc) => {
                                lrc = new_lrc;
                                lrc_state = LrcTimedTextState::new(&lrc, &progress);
                            }
                            Err(e) => {
                                eprintln!("Error parsing new file: {}", e);
                            }
                        }
                    }
                }
                _ => {}
            }
        }

        if !events.is_empty() {
            lrc_state = LrcTimedTextState::new(&lrc, &progress);
            if let Some(timed_text) = lrc_state.current {
                println!("{}", timed_text.text);
            }
        } else if progress.playback_status() == PlaybackStatus::Playing {
            if let Some(timed_text) = lrc_state.on_new_progress(&progress) {
                println!("{}", timed_text.text);
            }
        }
    }
}

fn main() {
    let opt = Opt::from_args();
    let lyrics_filepath = opt.lyrics.as_path();
    if !lyrics_filepath.is_file() {
        eprintln!("Lyrics path must be a file");
        return;
    }
    run(&opt.player, &lyrics_filepath);
}
