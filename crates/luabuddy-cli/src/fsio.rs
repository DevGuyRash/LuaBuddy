use crate::*;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt;
use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::AtomicU64;

pub(crate) static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

#[derive(Debug)]
pub struct Error(String);

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl std::error::Error for Error {}

impl From<luabuddy_core::CoreError> for Error {
    fn from(value: luabuddy_core::CoreError) -> Self {
        Self(value.to_string())
    }
}

pub(crate) type Result<T> = std::result::Result<T, Error>;

pub(crate) fn invalid(message: impl Into<String>) -> Error {
    Error(message.into())
}

pub(crate) fn io_error(action: &str, path: &Path, error: std::io::Error) -> Error {
    invalid(format!("{action} {}: {error}", path.display()))
}

pub(crate) fn link_like(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    if windows::reparse(metadata) {
        return true;
    }
    metadata.file_type().is_symlink()
}

pub(crate) fn reject_link_ancestors(path: &Path) -> Result<()> {
    for ancestor in path.ancestors() {
        if fs::symlink_metadata(ancestor).is_ok_and(|metadata| link_like(&metadata)) {
            return Err(invalid(format!(
                "Refusing a symlink or reparse point in an install destination: {}",
                ancestor.display()
            )));
        }
    }
    Ok(())
}

pub(crate) fn bytes(path: &Path, limit: u64) -> Result<Vec<u8>> {
    let metadata = fs::symlink_metadata(path).map_err(|e| io_error("Inspect", path, e))?;
    if !metadata.is_file() || link_like(&metadata) || metadata.len() > limit {
        return Err(invalid(format!(
            "Expected a regular file of at most {limit} bytes: {}",
            path.display()
        )));
    }
    let mut value = Vec::new();
    File::open(path)
        .map_err(|e| io_error("Open", path, e))?
        .take(limit + 1)
        .read_to_end(&mut value)
        .map_err(|e| io_error("Read", path, e))?;
    if value.len() as u64 > limit {
        return Err(invalid(format!(
            "File grew beyond its size limit: {}",
            path.display()
        )));
    }
    Ok(value)
}

pub(crate) fn optional_bytes(path: &Path, limit: u64) -> Result<Option<Vec<u8>>> {
    match fs::symlink_metadata(path) {
        Ok(_) => bytes(path, limit).map(Some),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(io_error("Inspect", path, e)),
    }
}

pub fn sha256(value: &[u8]) -> String {
    format!("{:x}", Sha256::digest(value))
}

pub(crate) fn json_bytes(value: &impl Serialize) -> Result<Vec<u8>> {
    let mut data =
        serde_json::to_vec_pretty(value).map_err(|e| invalid(format!("Encode JSON: {e}")))?;
    data.push(b'\n');
    Ok(data)
}

pub(crate) fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T> {
    serde_json::from_slice(&bytes(path, METADATA_LIMIT)?)
        .map_err(|e| invalid(format!("Read JSON {}: {e}", path.display())))
}

pub(crate) fn portable_path(value: &str) -> Result<PathBuf> {
    if value.is_empty() || value.contains(['\\', '\0', '\r', '\n', ':']) {
        return Err(invalid(format!("Invalid package path {value:?}")));
    }
    let path = Path::new(value);
    if !path
        .components()
        .all(|part| matches!(part, Component::Normal(_)))
    {
        return Err(invalid(format!(
            "Package path must be relative without traversal: {value:?}"
        )));
    }
    Ok(path.to_owned())
}

/// Resolves links in the part of a user's path that already exists. Whatever
/// LuaBuddy then creates below it is still checked for links.
pub(crate) fn resolve(path: &Path) -> Result<PathBuf> {
    let path = std::path::absolute(path).map_err(|e| io_error("Resolve", path, e))?;
    for ancestor in path.ancestors() {
        match fs::canonicalize(ancestor) {
            Ok(real) => {
                #[cfg(windows)]
                let real = windows::launcher_path(real).unwrap_or_else(|| ancestor.to_owned());
                return Ok(match path.strip_prefix(ancestor) {
                    Ok(rest) if !rest.as_os_str().is_empty() => real.join(rest),
                    _ => real,
                });
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(e) => return Err(io_error("Resolve", ancestor, e)),
        }
    }
    Ok(path)
}

pub(crate) fn plain_directory(path: &Path) -> Result<()> {
    reject_link_ancestors(path)?;
    if path.exists() {
        let metadata = fs::symlink_metadata(path).map_err(|e| io_error("Inspect", path, e))?;
        if !metadata.is_dir() || link_like(&metadata) {
            return Err(invalid(format!(
                "Refusing non-directory or symlink at {}",
                path.display()
            )));
        }
    }
    fs::create_dir_all(path).map_err(|e| io_error("Create directory", path, e))
}

pub(crate) fn sync_directory(path: &Path) -> Result<()> {
    #[cfg(unix)]
    File::open(path)
        .and_then(|file| file.sync_all())
        .map_err(|e| io_error("Sync directory", path, e))?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

pub(crate) fn write_new(path: &Path, data: &[u8], executable: bool) -> Result<()> {
    #[cfg(not(unix))]
    let _ = executable;
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
        .map_err(|e| io_error("Create", path, e))?;
    let result = (|| {
        file.write_all(data)
            .map_err(|e| io_error("Write", path, e))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(fs::Permissions::from_mode(if executable {
                0o755
            } else {
                0o600
            }))
            .map_err(|e| io_error("Set permissions", path, e))?;
        }
        file.sync_all().map_err(|e| io_error("Sync", path, e))
    })();
    if result.is_err() {
        fs::remove_file(path).map_err(|e| io_error("Remove incomplete file", path, e))?;
    }
    result
}

pub(crate) fn stage_path(path: &Path) -> Result<PathBuf> {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| invalid("Install filenames must be UTF-8"))?;
    Ok(path.with_file_name(format!(".{name}.luabuddy-stage")))
}

pub(crate) fn remove_if_present(path: &Path) -> Result<bool> {
    match fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(io_error("Remove owned file", path, e)),
    }
}

pub(crate) fn write_atomic(path: &Path, data: Option<&[u8]>, executable: bool) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| invalid("Install file has no parent directory"))?;
    plain_directory(parent)?;
    if let Some(data) = data {
        let stage = stage_path(path)?;
        remove_if_present(&stage)?;
        let permissions = fs::metadata(path)
            .ok()
            .map(|metadata| metadata.permissions());
        write_new(&stage, data, executable)?;
        let result = (|| {
            if let Some(permissions) = permissions {
                fs::set_permissions(&stage, permissions)
                    .map_err(|e| io_error("Preserve permissions", &stage, e))?;
            }
            fs::rename(&stage, path).map_err(|e| io_error("Replace", path, e))?;
            sync_directory(parent)
        })();
        if result.is_err() && stage.exists() {
            fs::remove_file(&stage).map_err(|e| io_error("Remove stage", &stage, e))?;
        }
        result
    } else {
        remove_if_present(path)?;
        sync_directory(parent)
    }
}

// The kernel owns the lock, so an exit releases it and a leftover file is harmless.
pub(crate) struct InstallLock(#[expect(dead_code, reason = "held until drop")] File);

impl InstallLock {
    pub(crate) fn acquire(game: &Path) -> Result<Self> {
        let root = game.join(INSTALL_DIR);
        plain_directory(&root)?;
        let path = root.join(LOCK_FILE);
        if fs::symlink_metadata(&path).is_ok_and(|kind| link_like(&kind) || !kind.is_file()) {
            return Err(invalid(format!(
                "Expected a regular installer lock file: {}",
                path.display()
            )));
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .map_err(|e| io_error("Open installer lock", &path, e))?;
        match file.try_lock() {
            Ok(()) => Ok(Self(file)),
            Err(TryLockError::WouldBlock) => Err(invalid(format!(
                "Another LuaBuddy installer is running and holds {}",
                path.display()
            ))),
            Err(TryLockError::Error(e)) => Err(io_error("Lock", &path, e)),
        }
    }
}
