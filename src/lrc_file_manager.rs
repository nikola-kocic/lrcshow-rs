use std::path::PathBuf;
use std::sync::mpsc::channel;
use std::thread;
use std::time::Duration;

use notify::{RecommendedWatcher, RecursiveMode, Watcher};

#[allow(unused_imports)]
use log::{debug, error, info, warn};

use crate::events::{Event, LyricsEvent, Metadata, TimedEvent};
use crate::lrc::{parse_lrc_file, Lyrics};

pub enum InputEvents {
    ChangePath(Option<PathBuf>),
    FileChanged(PathBuf),
}

pub struct LrcManager {
    tx: std::sync::mpsc::Sender<InputEvents>,
    rx: std::sync::mpsc::Receiver<InputEvents>,
    lyric_event_tx: std::sync::mpsc::Sender<TimedEvent>,
    watcher: RecommendedWatcher,
    lrc_filepath: Option<PathBuf>,
}

impl LrcManager {
    pub fn change_watched_path(
        file_path: Option<PathBuf>,
        sender: &std::sync::mpsc::Sender<InputEvents>,
    ) {
        debug!("change_watched_path : {:?}", file_path);
        sender.send(InputEvents::ChangePath(file_path)).unwrap();
    }

    pub fn clone_sender(&self) -> std::sync::mpsc::Sender<InputEvents> {
        self.tx.clone()
    }

    pub fn new(lyric_event_tx: std::sync::mpsc::Sender<TimedEvent>) -> Self {
        let (watcher_tx, watcher_rx) = channel();
        let watcher = RecommendedWatcher::new(watcher_tx, Duration::from_millis(100)).unwrap();

        let (tx, rx) = channel();
        {
            let tx_clone = tx.clone();
            thread::spawn(move || loop {
                match watcher_rx.recv() {
                    Ok(event) => {
                        debug!("Watcher event: {:?}", event);
                        match event {
                            notify::DebouncedEvent::Create(path)
                            | notify::DebouncedEvent::Write(path)
                            | notify::DebouncedEvent::NoticeRemove(path)
                            | notify::DebouncedEvent::Rename(_, path) => {
                                tx_clone.send(InputEvents::FileChanged(path)).unwrap();
                            }
                            _ => {}
                        }
                    }
                    Err(_) => {
                        return;
                    }
                }
            });
        }
        Self {
            tx,
            rx,
            lyric_event_tx,
            watcher,
            lrc_filepath: None,
        }
    }

    fn on_file_changed(&self, changed_file_path: Option<PathBuf>) {
        if changed_file_path == self.lrc_filepath {
            let lyrics = {
                if let Some(file_path) = &self.lrc_filepath {
                    if file_path.is_file() {
                        let lrc_file = parse_lrc_file(&file_path)
                            .map_err(|e| error!("Parsing lrc file failed: {}", e))
                            .ok();
                        debug!("lrc_file = {:?}", lrc_file);
                        lrc_file.map(Lyrics::new)
                    } else {
                        None
                    }
                } else {
                    None
                }
            };

            self.lyric_event_tx
                .send(TimedEvent::new(Event::LyricsEvent(
                    LyricsEvent::LyricsChanged {
                        lyrics,
                        file_path: changed_file_path,
                    },
                )))
                .unwrap();
        }
    }

    pub fn run_sync(&mut self) -> Result<(), ()> {
        loop {
            match self.rx.recv().map_err(|_| ())? {
                InputEvents::FileChanged(file_path) => self.on_file_changed(Some(file_path)),
                InputEvents::ChangePath(file_path) => {
                    if let Some(old_file_path) = &self.lrc_filepath {
                        let old_folder_path = old_file_path.parent().unwrap();
                        self.watcher.unwatch(old_folder_path).unwrap();
                        debug!("Stopped watching {:?} for changes", old_folder_path);
                    }
                    self.lrc_filepath = file_path.clone();
                    if let Some(new_file_path) = &self.lrc_filepath {
                        let new_folder_path = new_file_path.parent().unwrap();
                        self.watcher
                            .watch(new_folder_path, RecursiveMode::Recursive)
                            .unwrap();
                        debug!("Watching {:?} for changes", new_folder_path);
                    }
                    self.on_file_changed(file_path);
                }
            }
        }
    }

    pub fn run_async(mut self) {
        thread::spawn(move || {
            self.run_sync().unwrap();
        });
    }
}

pub fn get_lrc_filepath(metadata: &Metadata) -> PathBuf {
    let mut lrc_filepath = metadata.file_path.clone();
    lrc_filepath.set_extension("lrc");
    lrc_filepath
}
