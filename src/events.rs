use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::lrc::Lyrics;
use crate::player::BusName;

#[derive(Debug, PartialEq, Eq, Copy, Clone)]
pub enum PlaybackStatus {
    Playing,
    Paused,
    Stopped,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Metadata {
    pub file_path: PathBuf,
}

#[derive(Debug, PartialEq)]
pub struct PositionSnapshot {
    /// Position at the time of construction
    pub position: Duration,

    /// When this object was constructed, in order to calculate how old it is.
    pub instant: Instant,
}

#[derive(Debug, PartialEq)]
pub struct PlayerState {
    pub playback_status: PlaybackStatus,

    pub position_snapshot: PositionSnapshot,

    /// If player is stopped, metadata will be None
    pub metadata: Option<Metadata>,
}

impl PlayerState {
    pub fn current_position(&self) -> Duration {
        if self.playback_status == PlaybackStatus::Playing {
            self.position_snapshot.position + self.position_snapshot.instant.elapsed()
        } else {
            self.position_snapshot.position
        }
    }
}

#[derive(Debug, PartialEq)]
pub enum PlayerEvent {
    PlayerStarted { player_owner_name: BusName },
    PlayerShutDown,
    PlaybackStatusChange(PlaybackStatus),
    Seeked { position: Duration },
    MetadataChange(Option<Metadata>),
    Unknown { key: String, value: String },
}

#[derive(Debug)]
pub enum LyricsEvent {
    LyricsChanged {
        lyrics: Option<Lyrics>,
        #[allow(dead_code)] // TODO
        file_path: Option<PathBuf>,
    },
}

#[derive(Debug)]
pub enum Event {
    PlayerEvent(PlayerEvent),
    LyricsEvent(LyricsEvent),
}

#[derive(Debug)]
pub struct TimedEventBase<T> {
    pub instant: Instant,
    pub event: T,
}

pub type TimedEvent = TimedEventBase<Event>;
