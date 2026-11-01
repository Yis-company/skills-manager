//! Cross-process locks for project-update destinations, outside project folders.
use super::{central_repo, error::AppError, repo_lock::RepoLock};
use fs2::FileExt;
use std::{
    fs::{File, OpenOptions},
    path::{Path, PathBuf},
};

pub struct ProjectUpdateLock {
    _repository: RepoLock,
    _target: File,
}
impl ProjectUpdateLock {
    /// Waits consume no operation execution slot. The coordinator revisits
    /// other destinations before trying a contended target again.
    pub fn try_acquire(target: &Path) -> Result<Option<Self>, AppError> {
        let Some(repository) = RepoLock::try_shared().map_err(AppError::io)? else {
            return Ok(None);
        };
        let mut path = central_repo::base_dir().join("operation-locks");
        // Reversible path encoding, not a hash. Chunking avoids filesystem
        // component limits, including for long nested workspace paths.
        let encoded = hex::encode(target.as_os_str().as_encoded_bytes());
        for chunk in encoded.as_bytes().chunks(100) {
            path.push(std::str::from_utf8(chunk).expect("hex is ASCII"));
        }
        std::fs::create_dir_all(&path)?;
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path.join("lock"))?;
        match FileExt::try_lock_exclusive(&file) {
            Ok(()) => Ok(Some(Self {
                _repository: repository,
                _target: file,
            })),
            Err(error)
                if error.kind() == std::io::ErrorKind::WouldBlock
                    || error.raw_os_error() == Some(33) =>
            {
                Ok(None)
            }
            Err(error) => Err(AppError::io(error)),
        }
    }
    pub fn acquire(target: &Path) -> Result<Self, AppError> {
        let start = std::time::Instant::now();
        loop {
            if let Some(guard) = Self::try_acquire(target)? {
                return Ok(guard);
            }
            if start.elapsed() > super::repo_lock::FOREGROUND_WAIT {
                return Err(AppError::io("Project skill is busy"));
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
    }
}
impl Drop for ProjectUpdateLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self._target);
    }
}
/// Resolve physical folder aliases while keeping replacement of a final
/// symlink attached to the link's location, rather than its library pointee.
pub fn destination(path: &Path) -> Result<PathBuf, AppError> {
    let parent = path
        .parent()
        .ok_or_else(|| AppError::invalid_input("Missing target parent"))?
        .canonicalize()?;
    Ok(parent.join(
        path.file_name()
            .ok_or_else(|| AppError::invalid_input("Missing target name"))?,
    ))
}
