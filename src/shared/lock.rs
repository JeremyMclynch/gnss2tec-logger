use anyhow::{Context, Result};
use fs2::FileExt;
use std::fs::{self, File, OpenOptions};
use std::path::Path;

// Process-level lock guard backed by an OS file lock.
// This prevents duplicate logger/converter instances from stepping on each other.
pub struct LockGuard {
    file: File,
}

impl LockGuard {
    // Acquire an exclusive lock on the given file path.
    pub fn acquire(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent).with_context(|| {
                    format!("creating lock directory failed: {}", parent.display())
                })?;
            }
        }

        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(path)
            .with_context(|| format!("opening lock file failed: {}", path.display()))?;

        file.try_lock_exclusive()
            .with_context(|| format!("another instance is already running: {}", path.display()))?;

        Ok(Self { file })
    }
}

impl Drop for LockGuard {
    // Release the lock automatically when the guard goes out of scope.
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}
