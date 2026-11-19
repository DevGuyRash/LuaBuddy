//! Native x64 installer helpers. No Steam client, account or game process is opened.
use super::{INSTALL_DIR, Result, io_error, plain_directory};
use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::os::windows::ffi::OsStringExt;
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use windows_sys::Win32::Foundation::ERROR_SUCCESS;
use windows_sys::Win32::Storage::FileSystem::{
    FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_DELETE_ON_CLOSE, FILE_FLAG_OPEN_REPARSE_POINT,
};
use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_SZ, RegGetValueW};

pub(super) fn reparse(metadata: &fs::Metadata) -> bool {
    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

// fs::canonicalize returns a verbatim path. Java properties and external
// launchers should receive an ordinary absolute path, not a Win32 device name.
pub(super) fn launcher_directory(canonical: PathBuf) -> Option<PathBuf> {
    let name = canonical.to_str()?;
    let name = if let Some(unc) = name.strip_prefix(r"\\?\UNC\") {
        PathBuf::from(format!(r"\\{unc}"))
    } else {
        PathBuf::from(name.strip_prefix(r"\\?\")?)
    };
    // Do not reinterpret another namespace, or silently change a path that
    // depends on verbatim-only components (e.g. terminal periods/spaces).
    (name.is_absolute() && fs::canonicalize(&name).ok()? == canonical).then_some(name)
}

pub(super) fn steam_root() -> Option<PathBuf> {
    let key: Vec<_> = r"Software\Valve\Steam"
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let value: Vec<_> = "SteamPath".encode_utf16().chain(Some(0)).collect();
    // Maximum Windows path length including its terminator; the registry
    // is a discovery hint, not a reason to allocate an arbitrary value size.
    let mut buffer = vec![0u16; 32_768];
    let mut size = (buffer.len() * 2) as u32;
    // SAFETY: predefined read-only root, terminated names, and a caller-owned
    // aligned buffer with its exact byte capacity; returns no handle to close.
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            buffer.as_mut_ptr().cast(),
            &mut size,
        )
    };
    if status != ERROR_SUCCESS || size < 4 || !size.is_multiple_of(2) {
        return None;
    }
    let units = buffer.get(..size as usize / 2)?.strip_suffix(&[0])?;
    if units.contains(&0) {
        return None;
    }
    let path = PathBuf::from(OsString::from_wide(units));
    path.is_absolute().then_some(path)
}

// NT owns the exclusive handle, even on process termination. No PID reuse,
// /proc emulation or deleting another live installer's lock after a race.
pub(super) struct InstallLock(File);

impl InstallLock {
    pub(super) fn acquire(game: &Path) -> Result<Self> {
        let root = game.join(INSTALL_DIR);
        plain_directory(&root)?;
        let path = root.join("native-install.lock");
        OpenOptions::new()
            .create_new(true)
            .write(true)
            .share_mode(0)
            .custom_flags(FILE_FLAG_DELETE_ON_CLOSE | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(&path)
            .map(Self)
            .map_err(|error| {
                io_error(
                    "Acquire exclusive installer lock (close other setup sessions)",
                    &path,
                    error,
                )
            })
    }
    pub(super) fn release(self) -> Result<()> {
        drop(self.0);
        Ok(())
    }
}
