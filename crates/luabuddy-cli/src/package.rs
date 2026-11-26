use crate::*;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageFile {
    pub sha256: String,
    pub size: u64,
    pub executable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageManifest {
    pub schema: u32,
    pub product: String,
    pub version: String,
    pub os: String,
    pub architecture: String,
    pub runtime: String,
    pub native_runtime_verified: bool,
    pub files: BTreeMap<String, PackageFile>,
}

#[derive(Debug, Clone)]
pub struct Package {
    pub directory: PathBuf,
    pub manifest: PackageManifest,
    pub digest: String,
}

pub(crate) fn walk_files(
    root: &Path,
    directory: &Path,
    result: &mut BTreeSet<String>,
) -> Result<()> {
    for entry in fs::read_dir(directory).map_err(|e| io_error("Read directory", directory, e))? {
        let entry = entry.map_err(|e| io_error("Read directory entry", directory, e))?;
        let path = entry.path();
        let kind = fs::symlink_metadata(&path).map_err(|e| io_error("Inspect", &path, e))?;
        if link_like(&kind) {
            return Err(invalid(format!(
                "Package contains a symlink: {}",
                path.display()
            )));
        }
        if kind.is_dir() {
            walk_files(root, &path, result)?;
        } else if kind.is_file() {
            let relative = path
                .strip_prefix(root)
                .map_err(|_| invalid("Package traversal escaped its root"))?;
            let name = relative
                .to_str()
                .ok_or_else(|| invalid("Package path is not UTF-8"))?
                .replace('\\', "/");
            portable_path(&name)?;
            result.insert(name);
        } else {
            return Err(invalid(format!(
                "Package contains a special file: {}",
                path.display()
            )));
        }
    }
    Ok(())
}

pub fn verify_package(directory: &Path) -> Result<Package> {
    let directory =
        fs::canonicalize(directory).map_err(|e| io_error("Resolve package", directory, e))?;
    let manifest_path = directory.join("manifest.json");
    let data = bytes(&manifest_path, METADATA_LIMIT)?;
    let manifest: PackageManifest = serde_json::from_slice(&data)
        .map_err(|e| invalid(format!("Invalid package manifest: {e}")))?;
    if manifest.schema != SCHEMA || manifest.product != "LuaBuddy" || manifest.version.is_empty() {
        return Err(invalid("Unsupported LuaBuddy package schema or identity"));
    }
    if manifest.native_runtime_verified {
        return Err(invalid(
            "This installer does not accept an unauthenticated claim of native runtime verification",
        ));
    }
    let runtime = portable_path(&manifest.runtime)?;
    if runtime.components().count() != 1 || !manifest.files.contains_key(&manifest.runtime) {
        return Err(invalid(
            "Package runtime must be a listed file at the package root",
        ));
    }
    let mut actual = BTreeSet::new();
    walk_files(&directory, &directory, &mut actual)?;
    // The verified bytes of the two binaries are inspected below without another read.
    let mut runtime_bytes = None;
    let mut windows_cli = None;
    let mut expected = BTreeSet::from(["manifest.json".to_owned(), "manifest.sha256".to_owned()]);
    for (name, expected_file) in &manifest.files {
        let relative = portable_path(name)?;
        if name == "manifest.json"
            || name == "manifest.sha256"
            || expected_file.size > RUNTIME_LIMIT
        {
            return Err(invalid(format!("Invalid package file entry {name:?}")));
        }
        if relative.starts_with("profiles")
            && (relative.components().count() != 2
                || !relative
                    .extension()
                    .is_some_and(|extension| extension == "properties"))
        {
            return Err(invalid(format!(
                "Unsupported profile template entry {name:?}; expected a direct .properties file",
            )));
        }
        let file = bytes(&directory.join(relative), RUNTIME_LIMIT)?;
        if file.len() as u64 != expected_file.size || sha256(&file) != expected_file.sha256 {
            return Err(invalid(format!("Package integrity failed for {name}")));
        }
        if *name == manifest.runtime {
            runtime_bytes = Some(file);
        } else if name == "luabuddy.exe" {
            windows_cli = Some(file);
        }
        expected.insert(name.clone());
    }
    if actual != expected {
        return Err(invalid("Package has missing or unlisted files"));
    }
    let checksum = String::from_utf8(bytes(&directory.join("manifest.sha256"), 256)?)
        .map_err(|_| invalid("Manifest checksum is not UTF-8"))?;
    let digest = sha256(&data);
    if checksum != format!("{digest}  manifest.json\n") {
        return Err(invalid("Manifest checksum does not match"));
    }
    let runtime_bytes = runtime_bytes
        .ok_or_else(|| invalid("Package runtime must be a listed file at the package root"))?;
    let (os, architecture) = binary_identity(&runtime_bytes)?;
    if os != manifest.os || architecture != manifest.architecture {
        return Err(invalid(
            "Runtime binary architecture does not match its manifest",
        ));
    }
    if os == "windows" {
        if manifest.runtime != "luabuddy_host.dll" || !manifest.files.contains_key("luabuddy.exe") {
            return Err(invalid(
                "Windows package must own luabuddy.exe and luabuddy_host.dll",
            ));
        }
        let cli = windows_cli.ok_or_else(|| invalid("Windows package must own luabuddy.exe"))?;
        if binary_identity(&cli)? != (os, architecture) {
            return Err(invalid(
                "Windows package CLI and runtime architectures disagree",
            ));
        }
        verify_pe_role(&cli, false)?;
        verify_pe_role(&runtime_bytes, true)?;
    }
    read_profiles_if_exists(&directory.join("profiles"))?;
    Ok(Package {
        directory,
        manifest,
        digest,
    })
}

// COFF Characteristics and PE32+ magic distinguish a native x64 DLL from
// a renamed executable or a truncated PE-signature-only file.
pub(crate) fn verify_pe_role(data: &[u8], dll: bool) -> Result<()> {
    let valid = (|| {
        let offset = u32::from_le_bytes(data.get(60..64)?.try_into().ok()?) as usize;
        let nt = data.get(offset..)?;
        let header = nt.get(..24)?;
        let opt_size = u16::from_le_bytes(header[20..22].try_into().ok()?) as usize;
        let flags = u16::from_le_bytes(header[22..24].try_into().ok()?);
        let optional = nt.get(24..)?.get(..opt_size)?;
        Some(
            &header[..4] == b"PE\0\0"
                && optional.get(..2)? == [0x0b, 0x02]
                && flags & 0x0002 != 0
                && (flags & 0x2000 != 0) == dll,
        )
    })();
    if valid == Some(true) {
        Ok(())
    } else {
        Err(invalid(if dll {
            "Windows runtime must be a PE32+ DLL"
        } else {
            "Windows CLI must be a PE32+ executable"
        }))
    }
}

pub(crate) fn binary_identity(data: &[u8]) -> Result<(&'static str, &'static str)> {
    if data.len() >= 20 && &data[..4] == b"\x7fELF" && data[4] == 2 && data[5] == 1 {
        let architecture = match u16::from_le_bytes([data[18], data[19]]) {
            62 => "x86_64",
            183 => "aarch64",
            _ => return Err(invalid("Unsupported ELF runtime architecture")),
        };
        return Ok(("linux", architecture));
    }
    if data.len() >= 64 && &data[..2] == b"MZ" {
        let offset = u32::from_le_bytes(
            data[60..64]
                .try_into()
                .map_err(|_| invalid("Invalid PE header"))?,
        ) as usize;
        if let Some(header) = data.get(offset..offset.saturating_add(6))
            && &header[..4] == b"PE\0\0"
        {
            let architecture = match u16::from_le_bytes([header[4], header[5]]) {
                0x8664 => "x86_64",
                0xaa64 => "aarch64",
                _ => return Err(invalid("Unsupported PE runtime architecture")),
            };
            return Ok(("windows", architecture));
        }
    }
    if data.len() >= 8 && &data[..4] == b"\xcf\xfa\xed\xfe" {
        let architecture = match u32::from_le_bytes(
            data[4..8]
                .try_into()
                .map_err(|_| invalid("Invalid Mach-O header"))?,
        ) {
            0x01000007 => "x86_64",
            0x0100000c => "aarch64",
            _ => return Err(invalid("Unsupported Mach-O runtime architecture")),
        };
        return Ok(("macos", architecture));
    }
    Err(invalid("Runtime is not a supported 64-bit native library"))
}

pub(crate) fn add_package_file(
    root: &Path,
    files: &mut BTreeMap<String, PackageFile>,
    name: &str,
    content: &[u8],
    executable: bool,
) -> Result<()> {
    let path = root.join(portable_path(name)?);
    if let Some(parent) = path.parent() {
        plain_directory(parent)?;
    }
    write_new(&path, content, executable)?;
    files.insert(
        name.to_owned(),
        PackageFile {
            sha256: sha256(content),
            size: content.len() as u64,
            executable,
        },
    );
    Ok(())
}

pub fn make_package(runtime: &Path, templates: &Path, output: &Path) -> Result<Package> {
    if output.exists() {
        return Err(invalid(format!(
            "Package destination already exists: {}",
            output.display()
        )));
    }
    let runtime_content = bytes(runtime, RUNTIME_LIMIT)?;
    let (os, architecture) = binary_identity(&runtime_content)?;
    if os == "windows" {
        verify_pe_role(&runtime_content, true)?;
    }
    let runtime_name = match os {
        "linux" => "libluabuddy_host.so",
        "windows" => "luabuddy_host.dll",
        "macos" => "libluabuddy_host.dylib",
        _ => return Err(invalid("Unsupported package OS")),
    };
    let cli = env::current_exe()
        .map_err(|e| io_error("Locate LuaBuddy executable", Path::new("luabuddy"), e))?;
    let cli_content = bytes(&cli, RUNTIME_LIMIT)?;
    let (cli_os, cli_architecture) = binary_identity(&cli_content)?;
    if (cli_os, cli_architecture) != (os, architecture) {
        return Err(invalid(
            "Package CLI and runtime must target the same OS and architecture",
        ));
    }
    let profiles = read_profiles(templates)?;
    let output = resolve(output)?;
    let parent = output
        .parent()
        .ok_or_else(|| invalid("Package output needs a parent directory"))?;
    plain_directory(parent)?;
    let stage = parent.join(format!(
        ".luabuddy-package-{}-{}",
        std::process::id(),
        NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir(&stage).map_err(|e| io_error("Create package staging directory", &stage, e))?;
    let result = (|| {
        let mut files = BTreeMap::new();
        add_package_file(&stage, &mut files, runtime_name, &runtime_content, true)?;
        add_package_file(
            &stage,
            &mut files,
            if os == "windows" {
                "luabuddy.exe"
            } else {
                "luabuddy"
            },
            &cli_content,
            true,
        )?;
        add_package_file(
            &stage,
            &mut files,
            "LICENSE",
            include_bytes!("../../../LICENSE"),
            false,
        )?;
        add_package_file(
            &stage,
            &mut files,
            "THIRD_PARTY.md",
            include_bytes!("../../../THIRD_PARTY.md"),
            false,
        )?;
        add_package_file(
            &stage,
            &mut files,
            "licenses/Rust-standard-library.txt",
            include_bytes!("../../../licenses/Rust-standard-library.txt"),
            false,
        )?;
        for profile in profiles {
            let name = profile
                .file
                .file_name()
                .and_then(|n| n.to_str())
                .ok_or_else(|| invalid("Template filename must be UTF-8"))?;
            add_package_file(
                &stage,
                &mut files,
                &format!("profiles/{name}"),
                &bytes(&profile.file, METADATA_LIMIT)?,
                false,
            )?;
        }
        if os == "windows" {
            add_package_file(
                &stage,
                &mut files,
                "WINDOWS-SETUP.md",
                include_bytes!("../../../docs/windows-cli.md"),
                false,
            )?;
        }
        let manifest = PackageManifest {
            schema: SCHEMA,
            product: "LuaBuddy".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            os: os.into(),
            architecture: architecture.into(),
            runtime: runtime_name.into(),
            native_runtime_verified: false,
            files,
        };
        let encoded = json_bytes(&manifest)?;
        write_new(&stage.join("manifest.json"), &encoded, false)?;
        write_new(
            &stage.join("manifest.sha256"),
            format!("{}  manifest.json\n", sha256(&encoded)).as_bytes(),
            false,
        )?;
        let mut package = verify_package(&stage)?;
        // A rename within one directory keeps every byte, so the stage's
        // verification stands for the published package.
        fs::rename(&stage, &output)
            .map_err(|e| io_error("Publish package directory", &output, e))?;
        package.directory =
            fs::canonicalize(&output).map_err(|e| io_error("Resolve package", &output, e))?;
        Ok(package)
    })();
    if result.is_err() && stage.exists() {
        fs::remove_dir_all(&stage)
            .map_err(|e| io_error("Remove failed package stage", &stage, e))?;
    }
    result
}
