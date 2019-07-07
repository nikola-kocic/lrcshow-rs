mod lrc;
mod player;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use dbus::{BusType, Connection};
use structopt::StructOpt;

use crate::player::{Event, PlaybackStatus, Progress};

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

fn run(player: &str, lrc_filepath: &PathBuf) {
    let lrc = lrc::parse_lrc_file(lrc_filepath);
    // eprintln!("lrc = {:?}", lrc);
    let c = Connection::get_private(BusType::Session).unwrap();

    let player_owner_name = player::subscribe(&c, &player).unwrap();

    let mut progress = player::query_progress(&c, &player_owner_name);
    // eprintln!("progress = {:?}", progress);

    let mut iter_timed_text = lrc.timed_texts.iter();
    let default_timed_text = lrc::TimedText {
        position: Duration::from_micros(0),
        text: String::new(),
    };
    let mut current_timed_text; // = &default_timed_text;
    let mut next_timed_text = iter_timed_text.next();
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

        if !events.is_empty() {
            iter_timed_text = lrc.timed_texts.iter();
            let v = progress.position() + (Instant::now() - progress.instant());

            current_timed_text = iter_timed_text.next().unwrap_or(&default_timed_text);
            next_timed_text = iter_timed_text.next();

            while let Some(timed_text) = next_timed_text {
                if timed_text.position > v {
                    break;
                }
                current_timed_text = timed_text;
                next_timed_text = iter_timed_text.next();
            }

            println!("{}", current_timed_text.text);
        } else if progress.playback_status() == PlaybackStatus::Playing {
            if let Some(timed_text) = next_timed_text {
                let v = progress.position() + (Instant::now() - progress.instant());
                if v >= timed_text.position {
                    current_timed_text = timed_text;
                    next_timed_text = iter_timed_text.next();
                    println!("{}", current_timed_text.text);
                }
                // eprintln!("at {}", v.as_micros());
            }
        }
    }
}

fn main() {
    let opt = Opt::from_args();
    run(&opt.player, &opt.lyrics);
}
