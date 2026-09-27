//! Shared file primitives for instruction bundles and MCP definitions.
//! Portable files live in the existing Git library; deployment state stays in SQLite.
use anyhow::{bail, Context, Result};
use std::fs;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

pub const NAMESPACE: &str = ".agents-manager";
pub const MAX_FILE_BYTES: u64 = 1024 * 1024;

pub fn root() -> PathBuf {
    super::central_repo::skills_dir().join(NAMESPACE)
}

/// Check compatibility without creating directories or schema files.
pub fn check_schema() -> Result<()> {
    let path = root();
    reject_symlinks(&path)?;
    if !path.exists() {
        return Ok(());
    }
    let schema = path.join("schema.json");
    reject_symlinks(&schema)?;
    let value: serde_json::Value = serde_json::from_str(&read_limited(&schema)?)?;
    if value.get("version").and_then(|v| v.as_u64()) != Some(1) {
        bail!("This resource library requires another version of Agents Manager");
    }
    Ok(())
}

/// A crash between bundle-directory replacements must not turn into a
/// deletion in the next automatic Git backup. Recovery owns these leftovers.
pub fn check_library_transactions() -> Result<()> {
    let directory = super::central_repo::base_dir().join("resource-transactions");
    if !directory.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(directory)? {
        let name = entry?.file_name();
        let name = name.to_string_lossy();
        if name.starts_with(".instructions-") {
            bail!("An instruction library operation was interrupted. Recover interrupted changes in Instructions before backing up or restoring the library.");
        }
    }
    Ok(())
}

pub fn ensure_root() -> Result<()> {
    let path = root();
    reject_symlinks(&path)?;
    fs::create_dir_all(&path)?;
    let schema = path.join("schema.json");
    reject_symlinks(&schema)?;
    if schema.exists() {
        let value: serde_json::Value = serde_json::from_str(&read_limited(&schema)?)?;
        if value.get("version").and_then(|v| v.as_u64()) != Some(1) {
            bail!("This resource library requires another version of Agents Manager");
        }
    } else {
        atomic_write(&schema, b"{\"version\":1}\n")?;
    }
    Ok(())
}

pub fn validate_id(id: &str) -> Result<()> {
    let parsed = uuid::Uuid::parse_str(id).context("Invalid resource identifier")?;
    if parsed.to_string() != id {
        bail!("Resource identifiers must use canonical UUID format");
    }
    Ok(())
}

pub fn safe_relative(value: &str) -> Result<PathBuf> {
    if value.is_empty()
        || value.contains(['\\', ':', '\0'])
        || value.chars().any(char::is_control)
        || value
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        bail!("Use a relative path without empty components or parent traversal");
    }
    let path = PathBuf::from(value);
    if path
        .components()
        .any(|c| !matches!(c, Component::Normal(_)))
    {
        bail!("An absolute path cannot be used inside a bundle");
    }
    Ok(path)
}

/// Refuse links in existing ancestors as well as in the leaf. Callers editing
/// an existing, permitted symlink must first explicitly resolve its target.
pub fn reject_symlinks(path: &Path) -> Result<()> {
    for ancestor in path.ancestors() {
        match fs::symlink_metadata(ancestor) {
            Ok(meta) if meta.file_type().is_symlink() => {
                // macOS /var and /tmp are system aliases; callers should pass
                // canonical roots so even these do not silently redirect writes.
                bail!(
                    "Resolve the symbolic link before writing: {}",
                    ancestor.display()
                );
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}

pub fn read_limited(path: &Path) -> Result<String> {
    let file = fs::File::open(path)?;
    if !file.metadata()?.is_file() {
        bail!("Expected a regular file");
    }
    let mut bytes = Vec::new();
    file.take(MAX_FILE_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        bail!("File exceeds the 1 MiB editing limit");
    }
    String::from_utf8(bytes).context("File is not UTF-8 text")
}

pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    if bytes.len() as u64 > MAX_FILE_BYTES {
        bail!("File exceeds the 1 MiB editing limit");
    }
    reject_symlinks(path)?;
    let parent = path.parent().context("File has no parent directory")?;
    fs::create_dir_all(parent)?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    if let Ok(metadata) = fs::metadata(path) {
        if !metadata.is_file() {
            bail!("Destination is not a regular file");
        }
        temp.as_file().set_permissions(metadata.permissions())?;
    }
    temp.write_all(bytes)?;
    temp.as_file().sync_all()?;
    reject_symlinks(path)?;
    temp.persist(path).map_err(|e| e.error)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_cross_platform_escape_paths() {
        for path in ["", "../x", "a/../x", "/x", "a\\x", "C:x", "a//b", "./a"] {
            assert!(safe_relative(path).is_err(), "{path}");
        }
        assert_eq!(
            safe_relative("docs/testing.md").unwrap(),
            PathBuf::from("docs/testing.md")
        );
    }

    #[test]
    fn replaces_file_and_rejects_oversized_content() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().canonicalize().unwrap().join("docs/test.md");
        atomic_write(&path, b"first").unwrap();
        atomic_write(&path, b"second").unwrap();
        assert_eq!(read_limited(&path).unwrap(), "second");
        assert!(atomic_write(&path, &vec![b'a'; MAX_FILE_BYTES as usize + 1]).is_err());
        assert_eq!(read_limited(&path).unwrap(), "second");
    }

    #[test]
    fn interrupted_library_swap_blocks_backup_until_recovery_removes_its_artifacts() {
        let _repo = super::super::test_support::test_repo();
        check_library_transactions().unwrap();
        let transaction = super::super::central_repo::base_dir()
            .join("resource-transactions/.instructions-backup-fixture");
        fs::create_dir_all(&transaction).unwrap();
        assert!(check_library_transactions().is_err());
        fs::remove_dir(&transaction).unwrap();
        check_library_transactions().unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn refuses_linked_parent_without_touching_target() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        fs::create_dir(root.join("real")).unwrap();
        std::os::unix::fs::symlink(root.join("real"), root.join("alias")).unwrap();
        assert!(atomic_write(&root.join("alias/file"), b"no").is_err());
        assert!(!root.join("real/file").exists());
    }
}
