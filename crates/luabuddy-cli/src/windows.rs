//! Native x64 installer helpers. No Steam client, account or game process is opened.
use super::{Result, invalid};
use std::ffi::OsString;
use std::fs;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};
use windows_sys::Win32::Foundation::ERROR_SUCCESS;
use windows_sys::Win32::Globalization::{
    CP_ACP, CP_UTF8, GetACP, WC_NO_BEST_FIT_CHARS, WideCharToMultiByte,
};
use windows_sys::Win32::Storage::FileSystem::{FILE_ATTRIBUTE_REPARSE_POINT, GetShortPathNameW};
use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_SZ, RegGetValueW};

pub(super) fn reparse(metadata: &fs::Metadata) -> bool {
    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

// fs::canonicalize returns a verbatim path. Java properties and external
// launchers should receive an ordinary absolute path, not a Win32 device name.
pub(super) fn launcher_path(canonical: PathBuf) -> Option<PathBuf> {
    let name = canonical.to_str()?;
    let name = if let Some(unc) = name.strip_prefix(r"\\?\UNC\") {
        PathBuf::from(format!(r"\\{unc}"))
    } else {
        PathBuf::from(name.strip_prefix(r"\\?\")?)
    };
    // Do not reinterpret another namespace, or silently change a path that
    // depends on verbatim-only components (e.g. terminal periods/spaces).
    if !name.is_absolute() || fs::canonicalize(&name).ok()? != canonical {
        return None;
    }
    // Discovery still finds a real game when its JVM can't spell this path.
    Some(jvm_spelling(&name).unwrap_or(name))
}

fn native_encodes(path: &Path) -> bool {
    // JDK 25's Windows launcher and HotSpot DLL loader use ANSI paths.
    // File verification by Rust (wide Win32 calls) alone cannot establish this.
    if path.to_str().is_some_and(str::is_ascii) {
        return true;
    }
    // SAFETY: reads the current process ANSI code page, without changing it.
    if unsafe { GetACP() } == CP_UTF8 {
        return path.to_str().is_some();
    }
    let wide: Vec<_> = path.as_os_str().encode_wide().collect();
    if wide.len() > 32_767 {
        return false;
    }
    let mut encoded = vec![0u8; wide.len() * 4];
    let mut replaced = 0;
    // SAFETY: exact input/output lengths, no default-character override.
    let count = unsafe {
        WideCharToMultiByte(
            CP_ACP,
            WC_NO_BEST_FIT_CHARS,
            wide.as_ptr(),
            wide.len() as i32,
            encoded.as_mut_ptr(),
            encoded.len() as i32,
            std::ptr::null(),
            &mut replaced,
        )
    };
    count > 0 && replaced == 0
}

pub(super) fn jvm_spelling(path: &Path) -> Result<PathBuf> {
    if native_encodes(path) {
        return Ok(path.to_owned());
    }
    let wide: Vec<_> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut short = vec![0u16; 32_768];
    // SAFETY: absolute existing path and bounded UTF-16 output, both terminated.
    let count = unsafe { GetShortPathNameW(wide.as_ptr(), short.as_mut_ptr(), short.len() as u32) }
        as usize;
    if count > 0 && count < short.len() {
        let alias = PathBuf::from(OsString::from_wide(&short[..count]));
        if alias.is_absolute()
            && native_encodes(&alias)
            && let Ok(target) = fs::canonicalize(path)
            && fs::canonicalize(&alias).is_ok_and(|resolved| resolved == target)
        {
            return Ok(alias);
        }
    }
    Err(invalid(format!(
        "The Windows JVM cannot represent this path in the active code page, and no equivalent short path is available: {}. Choose a Steam library or profile folder with a representable path (ASCII is portable). No launcher override was written.",
        path.display()
    )))
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
