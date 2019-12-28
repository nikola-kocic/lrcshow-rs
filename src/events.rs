use std::path::PathBuf;
use std::time::{Duration, Instant};

#[derive(Debug, PartialEq, Eq, Copy, Clone)]
pub enum PlaybackStatus {
    Playing,
    Paused,
    Stopped,
}

#[derive(Clone, Debug)]
pub struct Metadata {
    pub album: Option<String>,
    pub title: String,
    pub artists: Option<Vec<String>>,
    pub file_path: PathBuf,
    pub length: i64,
}

#[derive(Debug)]
pub enum PlayerEvent {
    PlayerStarted,
    PlayerShutDown,
    PlaybackStatusChange(PlaybackStatus),
    Seeked { position: Duration },
    MetadataChange(Option<Metadata>),
}

#[derive(Debug)]
pub enum Event {
    PlayerEvent(PlayerEvent),
}

#[derive(Debug)]
pub struct TimedEvent {
    pub instant: Instant,
    pub event: Event,
}

#[derive(Clone, Debug)]
pub struct LyricsTiming {
    pub time: Duration,
    pub line_index: i32,           // index of line
    pub line_char_from_index: i32, // from this character in line
    pub line_char_to_index: i32,   // to this character in line
}
