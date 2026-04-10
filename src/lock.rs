use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use fs2::FileExt;

#[derive(Debug)]
pub struct ProcessLock {
    path: PathBuf,
    file: File,
}

impl ProcessLock {
    pub fn acquire(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent).with_context(|| {
                format!("failed to create lock file directory: {}", parent.display())
            })?;
        }

        let mut file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(path)
            .with_context(|| format!("failed to open lock file: {}", path.display()))?;

        if file.try_lock_exclusive().is_err() {
            bail!("lock already held: {}", path.display());
        }

        file.set_len(0)
            .with_context(|| format!("failed to reset lock file: {}", path.display()))?;
        writeln!(file, "pid={}", std::process::id())
            .with_context(|| format!("failed to write lock file: {}", path.display()))?;

        Ok(Self {
            path: path.to_path_buf(),
            file,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for ProcessLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_second_lock_acquisition_on_same_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("runtime.lock");

        let first = ProcessLock::acquire(&path).unwrap();
        let second = ProcessLock::acquire(&path);

        assert!(second.is_err());
        drop(first);
        assert!(ProcessLock::acquire(&path).is_ok());
    }
}
