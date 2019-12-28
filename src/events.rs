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
pub struct PositionSnapshot {
    /// Position at the time of construction
    pub position: Duration,

    /// When this object was constructed, in order to calculate how old it is.
    pub instant: Instant,
}

#[derive(Debug)]
pub struct PlayerState {
    pub playback_status: PlaybackStatus,

    pub position_snapshot: PositionSnapshot,

    /// If player is stopped, metadata will be None
    pub metadata: Option<Metadata>,
}

impl PlayerState {
    pub fn current_position(&self) -> Duration {
        if self.playback_status == PlaybackStatus::Playing {
            self.position_snapshot.position + (Instant::now() - self.position_snapshot.instant)
        } else {
            self.position_snapshot.position
        }
    }
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
