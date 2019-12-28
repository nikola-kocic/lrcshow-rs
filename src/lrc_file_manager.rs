use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Sender};
use std::time::Duration;

use notify::{RecommendedWatcher, RecursiveMode, Watcher};

#[allow(unused_imports)]
use log::{debug, error, info, warn};

use crate::events::Metadata;
use crate::lrc::{parse_lrc_file, Lyrics};

pub struct LrcManager {
    pub lyrics: Lyrics,
    rx: std::sync::mpsc::Receiver<notify::DebouncedEvent>,
    _watcher: RecommendedWatcher,
    lrc_filepath: PathBuf,
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

impl LrcManager {
    pub fn new(lrc_filepath: PathBuf) -> Option<Self> {
        let (tx, rx) = channel();
        let watcher = create_watcher(tx, lrc_filepath.parent()?)
            .map_err(|e| error!("Creating watched failed: {}", e))
            .ok()?;
        let lrc_file = parse_lrc_file(&lrc_filepath)
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

    pub fn should_recreate(&self) -> bool {
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

    pub fn maybe_recreate(&self) -> Option<LrcManager> {
        if self.should_recreate() {
            info!("Reloading lyrics");
            LrcManager::new(self.lrc_filepath.clone())
        } else {
            None
        }
    }
}

pub fn get_lrc_filepath(metadata: &Option<Metadata>) -> Option<PathBuf> {
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
