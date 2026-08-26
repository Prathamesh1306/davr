use chrono::Utc;
use davr_types::{Confidence, DavrError, Result};
use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver};
use std::time::{Duration, Instant};
use tracing::{debug, warn};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FsEventKind {
    Created,
    Modified,
    Deleted,
    Renamed { old_path: Option<String> },
}

#[derive(Debug, Clone)]
pub struct FsEvent {
    pub path: String,
    pub kind: FsEventKind,
    pub confidence: Confidence,
    pub content_hash_after: Option<String>,
    pub detected_at: i64,
}

pub struct FilesystemMonitor {
    project_root: PathBuf,
    _watcher: RecommendedWatcher,
    rx: Receiver<notify::Result<Event>>,
    pending_events: HashMap<PathBuf, (FsEventKind, Instant)>,
    debounce_duration: Duration,
}

impl FilesystemMonitor {
    pub fn start(project_root: impl AsRef<Path>) -> Result<Self> {
        let project_root = project_root
            .as_ref()
            .canonicalize()
            .unwrap_or_else(|_| project_root.as_ref().to_path_buf());
        let (tx, rx) = channel();

        let mut watcher = RecommendedWatcher::new(
            move |res| {
                let _ = tx.send(res);
            },
            Config::default(),
        )
        .map_err(|e| DavrError::General(format!("Failed to init file watcher: {}", e)))?;

        watcher
            .watch(&project_root, RecursiveMode::Recursive)
            .map_err(|e| DavrError::General(format!("Failed to watch project root: {}", e)))?;

        debug!(root = %project_root.display(), "Filesystem monitor started");

        Ok(Self {
            project_root,
            _watcher: watcher,
            rx,
            pending_events: HashMap::new(),
            debounce_duration: Duration::from_millis(300),
        })
    }

    /// Polls and drains debounced filesystem events
    pub fn poll_events(&mut self) -> Vec<FsEvent> {
        // 1. Drain raw events from channel into pending debounce map
        while let Ok(res) = self.rx.try_recv() {
            match res {
                Ok(event) => {
                    for raw_path in event.paths {
                        if is_ignored_path(&self.project_root, &raw_path) {
                            continue;
                        }

                        let kind = match event.kind {
                            EventKind::Create(_) => FsEventKind::Created,
                            EventKind::Modify(_) => FsEventKind::Modified,
                            EventKind::Remove(_) => FsEventKind::Deleted,
                            EventKind::Any => FsEventKind::Modified,
                            _ => FsEventKind::Modified,
                        };

                        self.pending_events.insert(raw_path, (kind, Instant::now()));
                    }
                }
                Err(e) => {
                    warn!(err = %e, "Filesystem watch event error");
                }
            }
        }

        // 2. Settle debounced events past debounce_duration
        let now = Instant::now();
        let mut settled = Vec::new();
        let mut to_remove = Vec::new();

        for (path, (kind, timestamp)) in &self.pending_events {
            if now.duration_since(*timestamp) >= self.debounce_duration {
                to_remove.push(path.clone());

                let canonical_path = path.canonicalize().unwrap_or_else(|_| path.clone());
                let rel_path = canonical_path
                    .strip_prefix(&self.project_root)
                    .or_else(|_| path.strip_prefix(&self.project_root))
                    .unwrap_or(path)
                    .to_string_lossy()
                    .to_string();

                if rel_path.is_empty() {
                    continue;
                }

                let (actual_kind, hash) = if path.exists() && path.is_file() {
                    let hash = compute_file_hash(path);
                    (kind.clone(), hash)
                } else if !path.exists() {
                    (FsEventKind::Deleted, None)
                } else {
                    (kind.clone(), None)
                };

                settled.push(FsEvent {
                    path: rel_path,
                    kind: actual_kind,
                    confidence: Confidence::High,
                    content_hash_after: hash,
                    detected_at: Utc::now().timestamp_millis(),
                });
            }
        }

        for path in to_remove {
            self.pending_events.remove(&path);
        }

        settled
    }

    /// Drains all remaining events regardless of debounce timer (used on session finish)
    pub fn drain_all_events(&mut self) -> Vec<FsEvent> {
        // Drain any remaining raw events
        while let Ok(res) = self.rx.try_recv() {
            if let Ok(event) = res {
                for raw_path in event.paths {
                    if !is_ignored_path(&self.project_root, &raw_path) {
                        let kind = match event.kind {
                            EventKind::Create(_) => FsEventKind::Created,
                            EventKind::Remove(_) => FsEventKind::Deleted,
                            _ => FsEventKind::Modified,
                        };
                        self.pending_events.insert(raw_path, (kind, Instant::now()));
                    }
                }
            }
        }

        let mut settled = Vec::new();
        for (path, (kind, _)) in self.pending_events.drain() {
            let canonical_path = path.canonicalize().unwrap_or_else(|_| path.clone());
            let rel_path = canonical_path
                .strip_prefix(&self.project_root)
                .or_else(|_| path.strip_prefix(&self.project_root))
                .unwrap_or(&path)
                .to_string_lossy()
                .to_string();

            if rel_path.is_empty() {
                continue;
            }

            let (actual_kind, hash) = if path.exists() && path.is_file() {
                let hash = compute_file_hash(&path);
                (kind, hash)
            } else if !path.exists() {
                (FsEventKind::Deleted, None)
            } else {
                (kind, None)
            };

            settled.push(FsEvent {
                path: rel_path,
                kind: actual_kind,
                confidence: Confidence::High,
                content_hash_after: hash,
                detected_at: Utc::now().timestamp_millis(),
            });
        }

        settled
    }
}

fn is_ignored_path(project_root: &Path, path: &Path) -> bool {
    if path == project_root {
        return true;
    }
    let rel = match path.strip_prefix(project_root) {
        Ok(r) => r,
        Err(_) => path,
    };
    let path_str = rel.to_string_lossy();

    path_str.is_empty()
        || path_str == "."
        || path_str.starts_with(".git")
        || path_str.starts_with(".davr")
        || path_str.contains("node_modules")
        || path_str.contains("/target/")
        || path_str.starts_with("target")
        || path_str.contains("/.venv/")
        || path_str.starts_with(".venv")
        || path_str.contains("/__pycache__/")
        || path_str.ends_with(".tmp")
}

fn compute_file_hash(path: &Path) -> Option<String> {
    fs::read(path)
        .ok()
        .map(|bytes| blake3::hash(&bytes).to_hex().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread::sleep;
    use tempfile::TempDir;

    #[test]
    fn test_filesystem_monitor_debounce_and_events() {
        let temp = TempDir::new().unwrap();
        let mut monitor = FilesystemMonitor::start(temp.path()).unwrap();

        // Give FSEvents watcher a moment to register on macOS
        sleep(Duration::from_millis(100));

        let test_file = temp.path().join("monitored.txt");
        fs::write(&test_file, b"first write").unwrap();
        fs::write(&test_file, b"second write").unwrap();

        // Poll with timeout
        let start = Instant::now();
        let mut captured = Vec::new();
        while start.elapsed() < Duration::from_secs(3) {
            sleep(Duration::from_millis(100));
            let events = monitor.poll_events();
            if !events.is_empty() {
                captured.extend(events);
                break;
            }
        }

        assert!(!captured.is_empty(), "Should capture settled file event");
        let ev = &captured[0];
        assert_eq!(ev.path, "monitored.txt");
        assert!(ev.content_hash_after.is_some());
    }
}
