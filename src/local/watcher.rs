use crate::error::Result;
use inotify::{EventMask, Inotify, WatchDescriptor, WatchMask};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchEventKind {
    Create,
    Modify,
    Delete,
    MovedFrom,
    MovedTo,
    /// Queue overflow - requires full rescan
    Overflow,
}

#[derive(Debug, Clone)]
pub struct WatchEvent {
    pub path: PathBuf,
    pub kind: WatchEventKind,
    pub is_dir: bool,
    pub cookie: Option<u32>,
}

pub struct Watcher {
    inotify: Inotify,
    root: PathBuf,
    tx: Sender<WatchEvent>,
    watches: HashMap<WatchDescriptor, PathBuf>,
}

impl Watcher {
    /// Create a new watcher for the given root directory.
    ///
    /// # Errors
    /// Returns an error if inotify initialization fails.
    pub fn new(root: &Path, tx: Sender<WatchEvent>) -> Result<Self> {
        let inotify = Inotify::init()?;
        let mut watcher = Self {
            inotify,
            root: root.to_path_buf(),
            tx,
            watches: HashMap::new(),
        };

        watcher.add_watch_recursive(root)?;
        tracing::info!(root = %root.display(), "👁️ Starting filesystem watcher");
        Ok(watcher)
    }

    fn add_watch_recursive(&mut self, path: &Path) -> Result<()> {
        // Use symlink_metadata to avoid following symlinks
        let md = std::fs::symlink_metadata(path)?;
        let ft = md.file_type();
        if ft.is_symlink() || !ft.is_dir() {
            return Ok(());
        }

        let mask = WatchMask::CREATE
            | WatchMask::DELETE
            | WatchMask::MODIFY
            | WatchMask::MOVED_FROM
            | WatchMask::MOVED_TO
            | WatchMask::CLOSE_WRITE
            | WatchMask::DELETE_SELF
            | WatchMask::MOVE_SELF;

        let wd = self.inotify.watches().add(path, mask)?;
        self.watches.insert(wd, path.to_path_buf());
        tracing::trace!(path = %path.display(), "👁️ Added watch");

        for entry in std::fs::read_dir(path)? {
            let entry = entry?;
            // Skip symlinks when recursing
            let Ok(entry_md) = std::fs::symlink_metadata(entry.path()) else {
                continue;
            };
            if entry_md.file_type().is_symlink() {
                continue;
            }
            if entry_md.file_type().is_dir() {
                self.add_watch_recursive(&entry.path())?;
            }
        }

        Ok(())
    }

    /// Run the watcher loop, sending events to the channel.
    ///
    /// # Errors
    /// Returns an error if reading inotify events fails.
    pub fn run(&mut self) -> Result<()> {
        let mut buffer = [0u8; 4096];

        loop {
            let events = self.inotify.read_events_blocking(&mut buffer)?;

            for event in events {
                let base_path = self
                    .watches
                    .get(&event.wd)
                    .cloned()
                    .unwrap_or_else(|| self.root.clone());

                let path = if let Some(name) = event.name {
                    base_path.join(name)
                } else {
                    base_path
                };

                let is_dir = event.mask.contains(EventMask::ISDIR);

                // Handle queue overflow - requires full rescan
                if event.mask.contains(EventMask::Q_OVERFLOW) {
                    let overflow_event = WatchEvent {
                        path: self.root.clone(),
                        kind: WatchEventKind::Overflow,
                        is_dir: true,
                        cookie: None,
                    };
                    tracing::warn!("👁️ Inotify queue overflow, full rescan needed");
                    if self.tx.send(overflow_event).is_err() {
                        return Ok(());
                    }
                    continue;
                }

                // Handle watch invalidation (directory deleted/moved)
                if event.mask.contains(EventMask::IGNORED) {
                    self.watches.remove(&event.wd);
                    tracing::debug!("👁️ Watch invalidated");
                    continue;
                }
                if event.mask.contains(EventMask::DELETE_SELF)
                    || event.mask.contains(EventMask::MOVE_SELF)
                {
                    self.watches.remove(&event.wd);
                }

                let kind = if event.mask.contains(EventMask::CREATE) {
                    // Add watch for new directories
                    if is_dir {
                        let _ = self.add_watch_recursive(&path);
                    }
                    WatchEventKind::Create
                } else if event.mask.contains(EventMask::MODIFY)
                    || event.mask.contains(EventMask::CLOSE_WRITE)
                {
                    WatchEventKind::Modify
                } else if event.mask.contains(EventMask::DELETE)
                    || event.mask.contains(EventMask::DELETE_SELF)
                {
                    WatchEventKind::Delete
                } else if event.mask.contains(EventMask::MOVED_FROM)
                    || event.mask.contains(EventMask::MOVE_SELF)
                {
                    WatchEventKind::MovedFrom
                } else if event.mask.contains(EventMask::MOVED_TO) {
                    if is_dir {
                        let _ = self.add_watch_recursive(&path);
                    }
                    WatchEventKind::MovedTo
                } else {
                    continue;
                };

                let cookie = if event.cookie == 0 {
                    None
                } else {
                    Some(event.cookie)
                };

                let watch_event = WatchEvent {
                    path,
                    kind,
                    is_dir,
                    cookie,
                };

                tracing::debug!(path = %watch_event.path.display(), kind = ?watch_event.kind, is_dir = watch_event.is_dir, "👁️ Filesystem event");
                if self.tx.send(watch_event).is_err() {
                    tracing::debug!("👁️ Watcher channel closed, stopping");
                    return Ok(());
                }
            }
        }
    }
}
