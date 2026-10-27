use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result};

pub struct Scratch(PathBuf);

impl Scratch {
    pub fn new(name: &str) -> Result<Self> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let parent = PathBuf::from(std::env::var_os("HOME").context("HOME")?)
            .join(".cache/lyracore-scratch/conformance");
        std::fs::create_dir_all(&parent)?;
        loop {
            let path = parent.join(format!(
                "{name}-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            match std::fs::create_dir(&path) {
                Ok(()) => return Ok(Self(path)),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error.into()),
            }
        }
    }
}

impl std::ops::Deref for Scratch {
    type Target = Path;

    fn deref(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
