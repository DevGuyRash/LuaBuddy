use luabuddy_core::{ModId, Profile, read_profiles, selection::case_variant};
pub mod extensions;
#[cfg(windows)]
mod windows;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fmt;
use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

const SCHEMA: u32 = 1;
const METADATA_LIMIT: u64 = 4 * 1024 * 1024;
const RUNTIME_LIMIT: u64 = 256 * 1024 * 1024;
const JOURNAL_LIMIT: u64 = 512 * 1024 * 1024;
const INSTALL_DIR: &str = ".luabuddy";
const RECORD_FILE: &str = "native-install.json";
const LOCK_FILE: &str = "install.lock";
const JOURNAL_FILE: &str = "native-transaction.json";
static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

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
type Result<T> = std::result::Result<T, Error>;

fn invalid(message: impl Into<String>) -> Error {
    Error(message.into())
}
fn io_error(action: &str, path: &Path, error: std::io::Error) -> Error {
    invalid(format!("{action} {}: {error}", path.display()))
}
fn link_like(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    if windows::reparse(metadata) {
        return true;
    }
    metadata.file_type().is_symlink()
}

fn reject_link_ancestors(path: &Path) -> Result<()> {
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

fn bytes(path: &Path, limit: u64) -> Result<Vec<u8>> {
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
fn optional_bytes(path: &Path, limit: u64) -> Result<Option<Vec<u8>>> {
    match fs::symlink_metadata(path) {
        Ok(_) => bytes(path, limit).map(Some),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(io_error("Inspect", path, e)),
    }
}
pub fn sha256(value: &[u8]) -> String {
    format!("{:x}", Sha256::digest(value))
}
fn json_bytes(value: &impl Serialize) -> Result<Vec<u8>> {
    let mut data =
        serde_json::to_vec_pretty(value).map_err(|e| invalid(format!("Encode JSON: {e}")))?;
    data.push(b'\n');
    Ok(data)
}
fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T> {
    serde_json::from_slice(&bytes(path, METADATA_LIMIT)?)
        .map_err(|e| invalid(format!("Read JSON {}: {e}", path.display())))
}
fn portable_path(value: &str) -> Result<PathBuf> {
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
fn resolve(path: &Path) -> Result<PathBuf> {
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
fn plain_directory(path: &Path) -> Result<()> {
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

fn walk_files(root: &Path, directory: &Path, result: &mut BTreeSet<String>) -> Result<()> {
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
    let runtime_bytes = bytes(&directory.join(&manifest.runtime), RUNTIME_LIMIT)?;
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
        let cli = bytes(&directory.join("luabuddy.exe"), RUNTIME_LIMIT)?;
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
fn verify_pe_role(data: &[u8], dll: bool) -> Result<()> {
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

fn binary_identity(data: &[u8]) -> Result<(&'static str, &'static str)> {
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

fn add_package_file(
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
        verify_package(&stage)?;
        fs::rename(&stage, &output)
            .map_err(|e| io_error("Publish package directory", &output, e))?;
        verify_package(&output)
    })();
    if result.is_err() && stage.exists() {
        fs::remove_dir_all(&stage)
            .map_err(|e| io_error("Remove failed package stage", &stage, e))?;
    }
    result
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JvmIdentity {
    pub executable: PathBuf,
    pub architecture: String,
    pub version_output: String,
}

pub fn game_directory(path: &Path) -> Option<PathBuf> {
    let game = [path.to_owned(), path.join("projectzomboid")]
        .into_iter()
        .find(|p| {
            p.join("projectzomboid.jar").is_file() || p.join("java/projectzomboid.jar").is_file()
        })
        .and_then(|p| fs::canonicalize(p).ok());
    #[cfg(windows)]
    {
        game.and_then(windows::launcher_path)
    }
    #[cfg(not(windows))]
    {
        game
    }
}

fn vdf_strings(text: &str) -> Vec<String> {
    let mut result = Vec::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '"' {
            continue;
        }
        let mut value = String::new();
        while let Some(c) = chars.next() {
            match c {
                '"' => break,
                '\\' if matches!(chars.peek(), Some('\\' | '"')) => {
                    value.push(chars.next().expect("matched VDF escape"));
                }
                _ => value.push(c),
            }
        }
        result.push(value);
    }
    result
}

/// Where Steam discovery looked and every game installation it found there.
pub struct Discovery {
    pub libraries: Vec<PathBuf>,
    pub candidates: Vec<PathBuf>,
    pub warnings: Vec<String>,
}

pub fn discover() -> Discovery {
    let (libraries, warnings) = libraries_from(steam_roots());
    let candidates: BTreeSet<_> = libraries
        .iter()
        .filter_map(|p| game_directory(&p.join("steamapps/common/ProjectZomboid")))
        .collect();
    Discovery {
        libraries,
        candidates: candidates.into_iter().collect(),
        warnings,
    }
}

fn steam_roots() -> BTreeSet<PathBuf> {
    let mut roots = BTreeSet::new();
    #[cfg(not(windows))]
    if let Some(home) = env::var_os("HOME").map(PathBuf::from) {
        roots.extend([
            home.join(".local/share/Steam"),
            home.join(".steam/steam"),
            home.join(".var/app/com.valvesoftware.Steam/.local/share/Steam"),
            home.join("Library/Application Support/Steam"),
        ]);
    }
    #[cfg(windows)]
    {
        roots.extend(windows::steam_root());
        for key in ["ProgramFiles(x86)", "ProgramFiles", "LOCALAPPDATA"] {
            if let Some(path) = env::var_os(key) {
                roots.insert(PathBuf::from(path).join("Steam"));
            }
        }
    }
    roots
}

fn libraries_from(roots: BTreeSet<PathBuf>) -> (Vec<PathBuf>, Vec<String>) {
    let mut libraries = roots.clone();
    let mut warnings = Vec::new();
    for root in roots {
        let path = root.join("steamapps/libraryfolders.vdf");
        let text = match fs::canonicalize(&path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => Err(io_error("Read", &path, e)),
            Ok(real) => bytes(&real, METADATA_LIMIT).and_then(|data| {
                String::from_utf8(data)
                    .map_err(|_| invalid(format!("Not UTF-8: {}", path.display())))
            }),
        };
        match text {
            Ok(text) => libraries.extend(
                vdf_strings(&text)
                    .windows(2)
                    .filter(|pair| pair[0] == "path")
                    .map(|pair| PathBuf::from(&pair[1])),
            ),
            Err(error) => warnings.push(format!("Skipped a Steam library list. {error}")),
        }
    }
    (libraries.into_iter().collect(), warnings)
}

pub fn find_game(explicit: Option<&Path>) -> Result<PathBuf> {
    let configured = explicit
        .map(PathBuf::from)
        .or_else(|| env::var_os("PZ_GAME_DIR").map(PathBuf::from));
    if let Some(configured) = configured {
        return game_directory(&configured).ok_or_else(|| {
            invalid(format!(
                "No Project Zomboid game JAR under {}",
                configured.display()
            ))
        });
    }
    let found = discover();
    if let [game] = found.candidates.as_slice() {
        return Ok(game.clone());
    }
    let list = |label: &str, paths: &[PathBuf]| -> String {
        paths
            .iter()
            .map(|path| format!("\n{label} {}", path.display()))
            .collect()
    };
    Err(invalid(format!(
        "Specify --game-dir or PZ_GAME_DIR; Steam discovery found {} installations{}{}{}",
        found.candidates.len(),
        list("Found", &found.candidates),
        list("Searched", &found.libraries),
        found
            .warnings
            .iter()
            .map(|warning| format!("\n{warning}"))
            .collect::<String>()
    )))
}

pub fn probe_jvm(game: &Path) -> Result<JvmIdentity> {
    #[cfg(windows)]
    let game = windows::jvm_spelling(game)?;
    let executable = game.join(if cfg!(windows) {
        "jre64/bin/java.exe"
    } else {
        "jre64/bin/java"
    });
    if !executable.is_file() {
        return Err(invalid(format!(
            "Bundled JVM not found: {}",
            executable.display()
        )));
    }
    // Resolve the explicitly selected executable, not a JDK from PATH. For
    // linked JREs this also lets java.exe locate its actual sibling runtime.
    #[cfg(windows)]
    let probe = fs::canonicalize(&executable)
        .ok()
        .and_then(windows::launcher_path)
        .ok_or_else(|| invalid("Cannot resolve the selected bundled java.exe"))?;
    #[cfg(not(windows))]
    let probe = &executable;
    let output = Command::new(probe)
        .args(["-XshowSettings:properties", "-version"])
        .env_remove("JAVA_TOOL_OPTIONS")
        .env_remove("JDK_JAVA_OPTIONS")
        .env_remove("_JAVA_OPTIONS")
        .output()
        .map_err(|e| io_error("Run bundled JVM architecture probe", &executable, e))?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr);
        let detail: String = detail
            .lines()
            .next()
            .unwrap_or("")
            .chars()
            .take(240)
            .collect();
        return Err(invalid(format!(
            "Bundled JVM probe failed with {}: {}",
            output.status, detail
        )));
    }
    if output.stderr.len() as u64 > METADATA_LIMIT {
        return Err(invalid("Bundled JVM identity output is too large"));
    }
    // Windows launchers can render unrelated settings (e.g. user.home)
    // in a legacy code page. The identity property and accepted values are ASCII.
    let text = String::from_utf8_lossy(&output.stderr);
    let architecture = text
        .lines()
        .find_map(|line| line.trim().strip_prefix("os.arch = "))
        .ok_or_else(|| invalid("Bundled JVM did not report os.arch"))?;
    let architecture = match architecture.trim() {
        "amd64" | "x86_64" => "x86_64",
        "aarch64" | "arm64" => "aarch64",
        other => {
            return Err(invalid(format!(
                "Unsupported actual JVM process architecture {other}"
            )));
        }
    };
    let version_output = text
        .lines()
        .filter(|line| {
            let line = line.trim();
            [
                "java.version = ",
                "java.runtime.version = ",
                "java.vm.name = ",
                "os.arch = ",
                "os.name = ",
                "openjdk version ",
                "java version ",
                "OpenJDK Runtime Environment",
                "OpenJDK 64-Bit",
            ]
            .iter()
            .any(|prefix| line.starts_with(prefix))
        })
        .collect::<Vec<_>>()
        .join("\n");
    Ok(JvmIdentity {
        executable,
        architecture: architecture.into(),
        version_output,
    })
}

fn supported_launcher_files(game: &Path) -> Result<(PathBuf, PathBuf)> {
    if !cfg!(any(
        target_os = "linux",
        all(windows, target_arch = "x86_64", target_env = "msvc")
    )) {
        return Err(invalid(
            "Native launcher integration is available on Linux and experimental x64 MSVC Windows only",
        ));
    }
    Ok((
        game.join("ProjectZomboid64.json"),
        game.join("ProjectZomboid64.site.json"),
    ))
}

fn launcher_paths(game: &Path) -> Result<(PathBuf, PathBuf)> {
    let (base, site) = supported_launcher_files(game)?;
    #[cfg(windows)]
    if !game.join("ProjectZomboid64.exe").is_file() {
        return Err(invalid(
            "The normal Windows ProjectZomboid64.exe launcher is missing",
        ));
    }
    if !base.is_file() {
        return Err(invalid(format!(
            "Native launcher configuration missing: {}",
            base.display()
        )));
    }
    Ok((base, site))
}

fn launcher_config(data: &[u8], path: &Path) -> Result<Value> {
    let value: Value = serde_json::from_slice(data)
        .map_err(|e| invalid(format!("Invalid launcher JSON in {}: {e}", path.display())))?;
    let valid = value.is_object()
        && value
            .get("vmArgs")
            .and_then(Value::as_array)
            .is_some_and(|args| args.iter().all(Value::is_string));
    if !valid {
        return Err(invalid(format!(
            "Launcher vmArgs must be an array of strings in {}",
            path.display()
        )));
    }
    Ok(value)
}

fn vm_args(config: &Value) -> Vec<String> {
    config["vmArgs"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect()
}

fn property(argument: &str, name: &str) -> bool {
    argument
        .strip_prefix("-D")
        .and_then(|rest| rest.strip_prefix(name))
        .is_some_and(|value| value.is_empty() || value.starts_with('='))
}

fn owned_agent(game: &Path, argument: &str) -> bool {
    argument
        .strip_prefix("-agentpath:")
        .is_some_and(|path| Path::new(path).starts_with(game.join(INSTALL_DIR).join("native")))
}

fn owned_argument(game: &Path, argument: &str) -> bool {
    owned_agent(game, argument)
        || [
            "luabuddy.config",
            "luabuddy.templates",
            "luabuddy.debug",
            "luabuddy.earlyUI",
        ]
        .iter()
        .any(|name| property(argument, name))
}

/// Splits launcher settings into everything else and LuaBuddy's own arguments.
fn split_integration(game: &Path, mut config: Value) -> (Value, Vec<String>) {
    let (owned, kept): (Vec<_>, Vec<_>) = vm_args(&config)
        .into_iter()
        .partition(|argument| owned_argument(game, argument));
    config["vmArgs"] = json!(kept);
    (config, owned)
}

/// The base launcher settings an unedited override was built from, when known.
fn recorded_base(record: &InstallRecord, clean: &Value) -> Result<Option<Value>> {
    if let Some(data) = &record.base_bytes {
        return serde_json::from_slice(data)
            .map(Some)
            .map_err(|e| invalid(format!("Invalid recorded base launcher: {e}")));
    }
    let matches = match &record.windows_base_digest {
        Some(digest) => *digest == sha256(&json_bytes(clean)?),
        None => false,
    };
    Ok(matches.then(|| clean.clone()))
}

/// The settings to build on, without LuaBuddy's arguments, and whether later
/// changes to Steam's base can be told apart from user edits. An override that
/// LuaBuddy created follows Steam's base; one that predates it is the user's.
fn launcher_source(
    game: &Path,
    site: &Path,
    data: &[u8],
    record: &InstallRecord,
    base: Option<&Value>,
) -> Result<(Value, bool)> {
    let config = launcher_config(data, site).map_err(|error| {
        invalid(format!(
            "{error}. Run repair to keep a copy and restore the recorded settings."
        ))
    })?;
    let (clean, _) = split_integration(game, config);
    if record.original_site.is_some() {
        return Ok((clean, true));
    }
    match (recorded_base(record, &clean)?, base) {
        (Some(was), Some(now)) => Ok((rebase(clean, &was, now), true)),
        (_, now) => {
            let unedited = now == Some(&clean);
            Ok((clean, unedited))
        }
    }
}

/// Carries the user's edits, the difference between `mine` and the base it was
/// built from, over to Steam's current base.
fn rebase(mut mine: Value, was: &Value, now: &Value) -> Value {
    let keys: BTreeSet<_> = [was, now]
        .into_iter()
        .filter_map(Value::as_object)
        .flat_map(|fields| fields.keys().cloned())
        .collect();
    for key in keys {
        let (edit, old, new) = (mine.get(&key), was.get(&key), now.get(&key));
        let merged = if edit == old {
            new.cloned()
        } else if old != new
            && let (Some(Value::Array(edit)), Some(Value::Array(old)), Some(Value::Array(new))) =
                (edit, old, new)
        {
            let kept = new
                .iter()
                .filter(|item| edit.contains(item) || !old.contains(item));
            let added = edit
                .iter()
                .filter(|item| !old.contains(item) && !new.contains(item));
            Some(kept.chain(added).cloned().collect())
        } else {
            edit.cloned()
        };
        match merged {
            Some(value) => mine[key.as_str()] = value,
            None => {
                if let Some(fields) = mine.as_object_mut() {
                    fields.remove(&key);
                }
            }
        }
    }
    mine
}

/// The override to work from. Repair and uninstall set unreadable contents
/// aside and use the recorded ones.
fn recorded_if_broken(
    game: &Path,
    site: &Path,
    data: Option<Vec<u8>>,
    record: &InstallRecord,
) -> Result<(Option<Vec<u8>>, Option<PathBuf>)> {
    match data {
        Some(broken) if launcher_config(&broken, site).is_err() => {
            let name = format!("launcher-{}.json", sha256(&broken));
            let copy = preserve(game, &name, &broken)?;
            Ok((Some(record.current.site_bytes.clone()), Some(copy)))
        }
        data => Ok((data, None)),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstalledGeneration {
    pub package_digest: String,
    pub package_version: String,
    pub runtime: PathBuf,
    pub runtime_sha256: String,
    pub agent_argument: String,
    pub config_argument: String,
    pub templates_argument: String,
    pub files: BTreeMap<PathBuf, String>,
    pub site_bytes: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstallRecord {
    pub schema: u32,
    pub game: PathBuf,
    pub profiles: PathBuf,
    pub original_site: Option<Vec<u8>>,
    // The site file replaces Steam's base file entirely. The base it was built
    // from tells a Steam update apart from a user's edit. Records written before
    // it was kept have only a Windows digest, or nothing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_bytes: Option<Vec<u8>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub windows_base_digest: Option<String>,
    pub current: InstalledGeneration,
    pub previous: Option<InstalledGeneration>,
    pub jvm: JvmIdentity,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Change {
    path: PathBuf,
    before: Option<Vec<u8>>,
    after: Option<Vec<u8>>,
    executable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Journal {
    schema: u32,
    game: PathBuf,
    profiles: PathBuf,
    changes: Vec<Change>,
    /// Generation files to remove, by SHA256, once `changes` are undone: those an
    /// interrupted operation had written, or those a completed one superseded.
    #[serde(default)]
    drop: BTreeMap<PathBuf, String>,
}

/// A content-addressed file that nothing uses until the launcher selects its
/// generation, so recovery needs only its hash.
struct GenerationFile {
    path: PathBuf,
    sha256: String,
    content: Vec<u8>,
    executable: bool,
}

fn sync_directory(path: &Path) -> Result<()> {
    #[cfg(unix)]
    File::open(path)
        .and_then(|file| file.sync_all())
        .map_err(|e| io_error("Sync directory", path, e))?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

fn write_new(path: &Path, data: &[u8], executable: bool) -> Result<()> {
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

fn stage_path(path: &Path) -> Result<PathBuf> {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| invalid("Install filenames must be UTF-8"))?;
    Ok(path.with_file_name(format!(".{name}.luabuddy-stage")))
}

fn remove_if_present(path: &Path) -> Result<bool> {
    match fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(io_error("Remove owned file", path, e)),
    }
}

fn write_atomic(path: &Path, data: Option<&[u8]>, executable: bool) -> Result<()> {
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
struct InstallLock(#[expect(dead_code, reason = "held until drop")] File);

impl InstallLock {
    fn acquire(game: &Path) -> Result<Self> {
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

fn validate_transaction(game: &Path, journal: &Journal) -> Result<()> {
    if journal.schema != SCHEMA || journal.game != game || !journal.profiles.is_absolute() {
        return Err(invalid(
            "Native transaction journal identity does not match this game",
        ));
    }
    let (_, site) = supported_launcher_files(game)?;
    let root = game.join(INSTALL_DIR);
    let mut seen = BTreeSet::new();
    for change in &journal.changes {
        if !seen.insert(&change.path)
            || change
                .path
                .components()
                .any(|c| matches!(c, Component::ParentDir))
        {
            return Err(invalid(
                "Native transaction contains duplicate or traversing paths",
            ));
        }
        reject_link_ancestors(&change.path)?;
        if change.path != site
            && !change.path.starts_with(&root)
            && change.path.parent() != Some(journal.profiles.as_path())
        {
            return Err(invalid(format!(
                "Native transaction contains an unowned path: {}",
                change.path.display()
            )));
        }
        if change.path.starts_with(&root) && change.path == root.join(JOURNAL_FILE) {
            return Err(invalid("Native transaction cannot overwrite itself"));
        }
    }
    for path in journal.drop.keys() {
        if !path.starts_with(root.join("native"))
            || path.components().any(|c| matches!(c, Component::ParentDir))
        {
            return Err(invalid(format!(
                "Native transaction contains an unowned path: {}",
                path.display()
            )));
        }
        reject_link_ancestors(path)?;
    }
    Ok(())
}

fn journal_path(game: &Path) -> PathBuf {
    game.join(INSTALL_DIR).join(JOURNAL_FILE)
}

fn read_journal(game: &Path) -> Result<Option<Journal>> {
    let path = journal_path(game);
    let Some(data) = optional_bytes(&path, JOURNAL_LIMIT)? else {
        return Ok(None);
    };
    serde_json::from_slice::<Journal>(&data)
        .map_err(|e| invalid(e.to_string()))
        .and_then(|journal| validate_transaction(game, &journal).map(|()| Some(journal)))
        .map_err(|e| {
            invalid(format!(
                "The interrupted operation recorded in {} cannot be undone: {e}. Uninstall sets that file aside.",
                path.display()
            ))
        })
}

/// What an interrupted operation left to do, if one is recorded.
fn interrupted(game: &Path) -> Option<String> {
    match read_journal(game) {
        Ok(None) => None,
        Ok(Some(_)) => Some(format!(
            "An interrupted operation is recorded in {}; run recover to undo it",
            journal_path(game).display()
        )),
        Err(error) => Some(error.to_string()),
    }
}

fn write_journal(game: &Path, journal: &Journal) -> Result<()> {
    validate_transaction(game, journal)?;
    let encoded = serde_json::to_vec(journal).map_err(|e| invalid(format!("Encode JSON: {e}")))?;
    if encoded.len() as u64 > JOURNAL_LIMIT {
        return Err(invalid(
            "Install transaction is too large for recoverable metadata",
        ));
    }
    write_atomic(&journal_path(game), Some(&encoded), false)
}

fn set_journal_aside(game: &Path) -> Result<PathBuf> {
    let recovery = game.join(INSTALL_DIR).join("recovery");
    plain_directory(&recovery)?;
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let target = recovery.join(format!("transaction-{unique}.json"));
    let path = journal_path(game);
    fs::rename(&path, &target).map_err(|e| io_error("Set aside", &path, e))?;
    Ok(target)
}

fn recover(game: &Path) -> Result<bool> {
    let path = journal_path(game);
    // A staged journal never took effect, so nothing it lists was applied.
    let staged = remove_if_present(&stage_path(&path)?)?;
    let Some(journal) = read_journal(game)? else {
        return Ok(staged);
    };
    for change in journal.changes.iter().rev() {
        remove_if_present(&stage_path(&change.path)?)?;
        let current = optional_bytes(&change.path, RUNTIME_LIMIT)?;
        if current == change.before {
            continue;
        }
        if current != change.after {
            return Err(invalid(format!(
                "Interrupted install encountered user-modified file {}; preserve it and its transaction journal {}",
                change.path.display(),
                path.display()
            )));
        }
        write_atomic(&change.path, change.before.as_deref(), change.executable)?;
    }
    drop_generation_files(game, &journal.drop)?;
    remove_if_present(&path)?;
    sync_directory(&game.join(INSTALL_DIR))?;
    Ok(true)
}

/// Removes unmodified generation files and the directories they leave empty.
fn drop_generation_files(game: &Path, files: &BTreeMap<PathBuf, String>) -> Result<()> {
    let native = game.join(INSTALL_DIR).join("native");
    let mut roots = BTreeSet::new();
    for (path, digest) in files {
        remove_if_present(&stage_path(path)?)?;
        if optional_bytes(path, RUNTIME_LIMIT)?.is_some_and(|data| sha256(&data) == *digest) {
            remove_if_present(path)?;
        }
        roots.extend(
            path.ancestors()
                .find(|ancestor| ancestor.parent() == Some(&native)),
        );
    }
    for root in roots {
        prune_empty(root);
    }
    let _ = fs::remove_dir(&native);
    Ok(())
}

fn prune_empty(directory: &Path) {
    for entry in fs::read_dir(directory).into_iter().flatten().flatten() {
        if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            prune_empty(&entry.path());
        }
    }
    let _ = fs::remove_dir(directory);
}

/// Splits recorded files into the unmodified ones to retire and the ones a user changed.
fn retire(files: &BTreeMap<PathBuf, String>) -> Result<(BTreeMap<PathBuf, String>, Vec<PathBuf>)> {
    let mut retired = BTreeMap::new();
    let mut modified = Vec::new();
    for (path, digest) in files {
        match optional_bytes(path, RUNTIME_LIMIT)? {
            Some(content) if sha256(&content) == *digest => {
                retired.insert(path.clone(), digest.clone());
            }
            Some(_) => modified.push(path.clone()),
            None => (),
        }
    }
    Ok((retired, modified))
}

/// Keeps a copy of content that is about to be replaced.
fn preserve(game: &Path, name: &str, content: &[u8]) -> Result<PathBuf> {
    let path = game.join(INSTALL_DIR).join("recovery").join(name);
    match optional_bytes(&path, RUNTIME_LIMIT)? {
        None => write_atomic(&path, Some(content), false)?,
        Some(existing) if existing == content => (),
        Some(_) => {
            return Err(invalid(format!(
                "Recovery filename has conflicting contents: {}",
                path.display()
            )));
        }
    }
    Ok(path)
}

fn transact(
    game: &Path,
    profiles: &Path,
    files: &[GenerationFile],
    changes: Vec<Change>,
    retired: BTreeMap<PathBuf, String>,
) -> Result<bool> {
    let changes: Vec<_> = changes
        .into_iter()
        .filter(|change| change.before != change.after)
        .collect();
    if files.is_empty() && changes.is_empty() && retired.is_empty() {
        return Ok(false);
    }
    let journal = Journal {
        schema: SCHEMA,
        game: game.to_owned(),
        profiles: profiles.to_owned(),
        changes,
        drop: files
            .iter()
            .filter(|file| !file.path.exists())
            .map(|file| (file.path.clone(), file.sha256.clone()))
            .collect(),
    };
    write_journal(game, &journal)?;
    let applied = (|| {
        for file in files {
            write_atomic(&file.path, Some(&file.content), file.executable)?;
        }
        for change in &journal.changes {
            if optional_bytes(&change.path, RUNTIME_LIMIT)? != change.before {
                return Err(invalid(format!(
                    "Install file changed during preparation: {}",
                    change.path.display()
                )));
            }
            write_atomic(&change.path, change.after.as_deref(), change.executable)?;
        }
        // Replacing the journal commits: from here recovery only removes what
        // the operation superseded.
        write_journal(
            game,
            &Journal {
                changes: Vec::new(),
                drop: retired,
                ..journal.clone()
            },
        )
    })();
    if let Err(error) = applied {
        return match recover(game) {
            Ok(_) => Err(error),
            Err(recovery) => Err(invalid(format!(
                "{error}; recovery also failed: {recovery}"
            ))),
        };
    }
    recover(game).map_err(|error| {
        invalid(format!(
            "The change took effect, but superseded files remain: {error}. Close the game, then run recover."
        ))
    })?;
    Ok(true)
}

fn read_record(game: &Path) -> Result<Option<InstallRecord>> {
    let path = game.join(INSTALL_DIR).join(RECORD_FILE);
    let Some(data) = optional_bytes(&path, METADATA_LIMIT)? else {
        return Ok(None);
    };
    let record: InstallRecord = serde_json::from_slice(&data).map_err(|e| {
        invalid(format!(
            "Invalid native installation record {}: {e}",
            path.display()
        ))
    })?;
    if record.schema != SCHEMA || record.game != game || !record.profiles.is_absolute() {
        return Err(invalid(
            "Native installation record does not match this game",
        ));
    }
    for generation in std::iter::once(&record.current).chain(record.previous.iter()) {
        if !generation
            .runtime
            .starts_with(game.join(INSTALL_DIR).join("native"))
            || generation
                .runtime
                .components()
                .any(|part| matches!(part, Component::ParentDir))
        {
            return Err(invalid(
                "Native installation record contains an unowned runtime path",
            ));
        }
        let root = generation
            .runtime
            .parent()
            .ok_or_else(|| invalid("Native runtime has no generation directory"))?;
        if !generation.files.contains_key(&generation.runtime) {
            return Err(invalid("Native generation does not own its runtime file"));
        }
        for path in generation.files.keys() {
            if !path.starts_with(root)
                || path.components().any(|c| matches!(c, Component::ParentDir))
            {
                return Err(invalid("Native generation contains an unowned file path"));
            }
            reject_link_ancestors(path)?;
        }
    }
    Ok(Some(record))
}

fn read_profiles_if_exists(directory: &Path) -> Result<Vec<Profile>> {
    if directory.exists() {
        Ok(read_profiles(directory)?)
    } else {
        Ok(Vec::new())
    }
}

/// A word the user can paste back into their shell.
fn shell_word(value: &str) -> String {
    if !value.is_empty()
        && value
            .chars()
            .all(|c| c.is_alphanumeric() || "_./:\\-".contains(c) && (c != '\\' || cfg!(windows)))
    {
        value.to_owned()
    } else if cfg!(windows) {
        format!("\"{value}\"")
    } else {
        format!("'{}'", value.replace('\'', "'\\''"))
    }
}

struct SelectedProfiles {
    files: Vec<(PathBuf, Vec<u8>)>,
    outcomes: Vec<Value>,
}

fn selected_profile_files(
    ids: &[String],
    directory: &Path,
    package: &Package,
) -> Result<SelectedProfiles> {
    let existing = read_profiles_if_exists(directory)?;
    let templates = read_profiles_if_exists(&package.directory.join("profiles"))?;
    let mut selected = BTreeMap::new();
    let mut files = Vec::new();
    let mut outcomes = Vec::new();
    for id in ids {
        if id.chars().any(char::is_control) {
            return Err(invalid("Mod IDs must not contain control characters"));
        }
        let id = ModId::new(id)?;
        let name = profile_filename(id.as_str());
        if let Some(other) = selected.insert(name.to_lowercase(), id.clone()) {
            if other != id {
                return Err(invalid(format!(
                    "Mod IDs {other} and {id} collide at {name}"
                )));
            }
            continue;
        }
        if let Some(variant) = case_variant(&existing, &id) {
            return Err(invalid(format!(
                "Mod IDs are case-sensitive; {id} is not selected, but {} exists in {}",
                variant.id,
                variant.file.display()
            )));
        }
        let target = directory.join(&name);
        if let Some(profile) = existing.iter().find(|profile| profile.id == id) {
            if profile.file != target {
                return Err(invalid(format!(
                    "Mod ID {id} already exists under another profile filename: {}",
                    profile.file.display()
                )));
            }
            outcomes.push(if profile.enabled {
                json!({"id":id, "outcome":"alreadyEnabled", "profile":target})
            } else {
                json!({"id":id, "outcome":"existsDisabled", "profile":target, "enableCommand":format!(
                    "luabuddy select --profiles-dir {} --mod {}",
                    shell_word(&directory.to_string_lossy()),
                    shell_word(id.as_str())
                )})
            });
            continue;
        }
        if directory.exists() {
            for entry in fs::read_dir(directory)
                .map_err(|e| io_error("Read profile directory", directory, e))?
            {
                let entry =
                    entry.map_err(|e| io_error("Read profile directory entry", directory, e))?;
                if entry.file_name().to_string_lossy().to_lowercase() == name.to_lowercase() {
                    return Err(invalid(format!(
                        "Profile filename for {id} is taken by another mod: {}",
                        entry.path().display()
                    )));
                }
            }
        }
        let template = templates.iter().find(|profile| profile.id == id);
        let content = if let Some(template) = template {
            bytes(&template.file, METADATA_LIMIT)?
        } else {
            format!(
                "id={}\n",
                id.as_str()
                    .replace('\\', "\\\\")
                    .replace(' ', "\\ ")
                    .replace('=', "\\=")
                    .replace(':', "\\:")
            )
            .into_bytes()
        };
        Profile::parse(
            &target,
            std::str::from_utf8(&content).map_err(|_| invalid("Profile is not UTF-8"))?,
        )?;
        outcomes.push(
            json!({"id":id, "outcome":"created", "profile":target, "fromTemplate":template.is_some(), "templateWithDifferentCase":case_variant(&templates, &id).map(|profile| &profile.id)}),
        );
        files.push((target, content));
    }
    Ok(SelectedProfiles { files, outcomes })
}

pub use luabuddy_core::profile::profile_filename;

#[derive(Debug, Clone)]
pub struct InstallOptions {
    pub package: PathBuf,
    pub profiles: Option<PathBuf>,
    pub mods: Vec<String>,
    pub debug: Option<bool>,
    pub early_ui: Option<bool>,
    pub repair: bool,
}

pub fn install(game: &Path, options: &InstallOptions) -> Result<Value> {
    let package = verify_package(&options.package)?;
    if package.manifest.os != env::consts::OS {
        return Err(invalid("Package OS does not match the running installer"));
    }
    let (base, site) = launcher_paths(game)?;
    let jvm = probe_jvm(game)?;
    if jvm.architecture != package.manifest.architecture {
        return Err(invalid(
            "Package architecture does not match the actual bundled JVM process",
        ));
    }
    let _lock = InstallLock::acquire(game)?;
    let recovered = recover(game)?;
    let prior = read_record(game)?;
    if options.repair
        && let Some(record) = &prior
        && record.current.package_digest != package.digest
    {
        return Err(invalid(format!(
            "Repair needs the installed package {}, but the supplied package is {}. Use update to change packages.",
            record.current.package_digest, package.digest
        )));
    }
    let current_site = optional_bytes(&site, METADATA_LIMIT)?;
    let base_bytes = bytes(&base, METADATA_LIMIT)?;
    let base_config = launcher_config(&base_bytes, &base)?;
    let (launcher, broken_launcher) = match &prior {
        Some(record) if options.repair => {
            recorded_if_broken(game, &site, current_site.clone(), record)?
        }
        _ => (current_site.clone(), None),
    };
    let (mut config, follows_base) = match (&launcher, &prior) {
        (None, _) => (split_integration(game, base_config).0, true),
        (Some(data), Some(record)) => {
            launcher_source(game, &site, data, record, Some(&base_config))?
        }
        (Some(data), None) => (
            split_integration(game, launcher_config(data, &site)?).0,
            true,
        ),
    };
    let old_args = match launcher
        .as_ref()
        .or(prior.as_ref().map(|r| &r.current.site_bytes))
    {
        Some(data) => split_integration(game, launcher_config(data, &site)?).1,
        None => Vec::new(),
    };
    let old_profile = old_args
        .iter()
        .rev()
        .find_map(|arg| arg.strip_prefix("-Dluabuddy.config="))
        .map(PathBuf::from);
    let profiles = resolve(
        &options
            .profiles
            .clone()
            .or_else(|| prior.as_ref().map(|r| r.profiles.clone()))
            .or(old_profile)
            .unwrap_or_else(|| game.join(INSTALL_DIR).join("profiles")),
    )?;
    if profiles.to_string_lossy().contains(['\0', '\r', '\n']) {
        return Err(invalid(
            "Profile directory contains a forbidden control character",
        ));
    }
    let SelectedProfiles {
        files: profile_files,
        outcomes,
    } = selected_profile_files(&options.mods, &profiles, &package)?;
    plain_directory(&profiles)?;
    #[cfg(windows)]
    let (profiles, profile_files) = {
        let spelled = windows::jvm_spelling(&profiles)?;
        let selected = profile_files
            .into_iter()
            .map(|(path, data)| {
                let name = path
                    .file_name()
                    .ok_or_else(|| invalid("Invalid selected profile filename"))?;
                Ok((spelled.join(name), data))
            })
            .collect::<Result<Vec<_>>>()?;
        (spelled, selected)
    };
    let generation_root = game.join(INSTALL_DIR).join("native").join(&package.digest);
    let runtime = generation_root.join(&package.manifest.runtime);
    let runtime_string = runtime
        .to_str()
        .ok_or_else(|| invalid("Native runtime path must be UTF-8"))?;
    if runtime_string.contains(['=', '\0', '\r', '\n']) {
        return Err(invalid(
            "Native runtime path contains an unsupported agentpath separator",
        ));
    }
    let agent_argument = format!("-agentpath:{runtime_string}");
    let config_argument = format!(
        "-Dluabuddy.config={}",
        profiles
            .to_str()
            .ok_or_else(|| invalid("Profile path must be UTF-8"))?
    );
    let templates = generation_root.join("profiles");
    let templates_argument = format!(
        "-Dluabuddy.templates={}",
        templates
            .to_str()
            .ok_or_else(|| invalid("Template path must be UTF-8"))?
    );
    let mut args = vm_args(&config);
    for (name, choice) in [
        ("luabuddy.debug", options.debug),
        ("luabuddy.earlyUI", options.early_ui),
    ] {
        match choice {
            Some(true) => args.push(format!("-D{name}=true")),
            Some(false) => (),
            None => args.extend(old_args.iter().filter(|arg| property(arg, name)).cloned()),
        }
    }
    args.push(config_argument.clone());
    args.push(templates_argument.clone());
    args.push(agent_argument.clone());
    config["vmArgs"] = json!(args);
    let site_bytes = json_bytes(&config)?;
    let same = prior
        .as_ref()
        .is_some_and(|record| record.current.package_digest == package.digest);
    let mut generation_files = BTreeMap::new();
    let mut missing = Vec::new();
    let mut modified = Vec::new();
    for (name, file) in &package.manifest.files {
        let native = *name == package.manifest.runtime;
        if !native
            && !name.starts_with("profiles/")
            && name != "LICENSE"
            && name != "THIRD_PARTY.md"
            && !name.starts_with("licenses/")
        {
            continue;
        }
        let path = generation_root.join(portable_path(name)?);
        generation_files.insert(path.clone(), file.sha256.clone());
        if let Some(before) = optional_bytes(&path, RUNTIME_LIMIT)? {
            let digest = sha256(&before);
            if digest == file.sha256 {
                continue;
            }
            if same && !options.repair {
                return Err(invalid(format!(
                    "Installed generation file was modified: {}; use repair with the installed package to preserve a recovery copy",
                    path.display()
                )));
            }
            modified.push((
                if native {
                    format!("runtime-{digest}")
                } else {
                    format!("templates/{digest}")
                },
                before,
            ));
        }
        let content = bytes(&package.directory.join(name), RUNTIME_LIMIT)?;
        if sha256(&content) != file.sha256 {
            return Err(invalid(format!(
                "Package changed during installation: {name}"
            )));
        }
        missing.push(GenerationFile {
            path,
            sha256: file.sha256.clone(),
            content,
            executable: file.executable,
        });
    }
    plain_directory(&templates)?;
    let generation = InstalledGeneration {
        package_digest: package.digest,
        package_version: package.manifest.version,
        runtime: runtime.clone(),
        runtime_sha256: package.manifest.files[&package.manifest.runtime]
            .sha256
            .clone(),
        agent_argument,
        config_argument,
        templates_argument,
        files: generation_files,
        site_bytes: site_bytes.clone(),
    };
    let record = InstallRecord {
        schema: SCHEMA,
        game: game.to_owned(),
        profiles: profiles.clone(),
        original_site: prior
            .as_ref()
            .map(|r| r.original_site.clone())
            .unwrap_or(current_site.clone()),
        base_bytes: follows_base.then_some(base_bytes),
        windows_base_digest: None,
        current: generation,
        previous: if same {
            prior.as_ref().and_then(|r| r.previous.clone())
        } else {
            prior.as_ref().map(|r| r.current.clone())
        },
        jvm,
    };
    let mut changes = Vec::new();
    for (path, content) in profile_files {
        changes.push(Change {
            path,
            before: None,
            after: Some(content),
            executable: false,
        });
    }
    let record_path = game.join(INSTALL_DIR).join(RECORD_FILE);
    changes.push(Change {
        before: optional_bytes(&record_path, METADATA_LIMIT)?,
        path: record_path,
        after: Some(json_bytes(&record)?),
        executable: false,
    });
    changes.push(Change {
        path: site,
        before: current_site,
        after: Some(site_bytes),
        executable: false,
    });
    let (retired, retained_modified) = prior
        .as_ref()
        .and_then(|r| r.previous.as_ref())
        .filter(|g| {
            g.runtime != record.current.runtime
                && record
                    .previous
                    .as_ref()
                    .is_none_or(|previous| previous.runtime != g.runtime)
        })
        .map(|g| retire(&g.files))
        .transpose()?
        .unwrap_or_default();
    for (name, content) in modified {
        preserve(game, &name, &content)?;
    }
    let changed = transact(game, &profiles, &missing, changes, retired)?;
    Ok(
        json!({"operation": if options.repair { "repair" } else { "install" }, "changed":changed, "recoveredInterruptedOperation":recovered, "profiles":profiles, "mods":outcomes, "modifiedRetiredFilesPreserved":retained_modified, "invalidLauncherPreserved":broken_launcher, "runtime":record.current.runtime, "runtimeVerified":false, "nextStep":"Fully restart Project Zomboid through its normal launcher. Package integrity is verified; in-game operation requires its own runtime evidence."}),
    )
}

/// The directories an installation uses.
pub struct InstalledDirectories {
    pub profiles: PathBuf,
    pub templates: PathBuf,
}

pub fn installed_directories(game: &Path) -> Result<InstalledDirectories> {
    let record = read_record(game)?.ok_or_else(|| {
        invalid(format!(
            "LuaBuddy native runtime is not installed in {}",
            game.display()
        ))
    })?;
    Ok(InstalledDirectories {
        templates: record.current.runtime.with_file_name("profiles"),
        profiles: record.profiles,
    })
}

#[derive(Debug, Clone)]
pub struct InstallationStatus {
    pub package_version: String,
    pub package_digest: String,
    pub previous_version: Option<String>,
    pub problems: Vec<String>,
}

const BASE_CHANGED: &str = "Steam's base launcher JSON changed since the launcher override was built from it. Close the game, then run update or repair to rebuild the override.";

/// What is wrong with an installation, and what a user changed around it.
fn inspect(game: &Path, record: &InstallRecord) -> Result<(Vec<String>, Vec<String>)> {
    let (base, site) = launcher_paths(game)?;
    let mut problems = Vec::new();
    let mut notes = Vec::new();
    for (path, digest) in &record.current.files {
        match optional_bytes(path, RUNTIME_LIMIT)? {
            Some(data) if sha256(&data) == *digest => (),
            Some(_) => problems.push(format!(
                "Installed generation file integrity failed: {}",
                path.display()
            )),
            None => problems.push(format!(
                "Installed generation file is missing: {}",
                path.display()
            )),
        }
    }
    let base_bytes = bytes(&base, METADATA_LIMIT)?;
    let base_config = serde_json::from_slice::<Value>(&base_bytes).ok();
    let base_changed = match (
        &record.base_bytes,
        &record.windows_base_digest,
        &base_config,
    ) {
        (Some(saved), _, _) => serde_json::from_slice(saved).ok() != base_config,
        (None, Some(saved), Some(now)) => *saved != sha256(&json_bytes(now)?),
        (None, Some(_), None) => true,
        (None, None, _) => false,
    };
    if base_changed && record.original_site.is_none() {
        problems.push(BASE_CHANGED.into());
    } else if base_changed {
        notes.push(format!(
            "Steam's base launcher JSON changed since the last install. {} is your own override, so compare it with {}.",
            site.display(),
            base.display()
        ));
    }
    match optional_bytes(&site, METADATA_LIMIT)?.map(|data| launcher_config(&data, &site)) {
        None => problems.push(format!(
            "Launcher override is missing; run repair: {}",
            site.display()
        )),
        Some(Err(error)) => problems.push(format!(
            "{error}. Run repair to keep a copy and restore the recorded settings."
        )),
        Some(Ok(config)) => {
            let generation = &record.current;
            let args = vm_args(&config);
            let last = |name: &str| args.iter().rev().find(|arg| property(arg, name));
            if !args
                .iter()
                .filter(|arg| owned_agent(game, arg))
                .eq([&generation.agent_argument])
                || last("luabuddy.config") != Some(&generation.config_argument)
                || last("luabuddy.templates") != Some(&generation.templates_argument)
            {
                problems.push(format!(
                    "The launcher override does not select the installed generation; run repair: {}",
                    site.display()
                ));
            }
            if launcher_config(&generation.site_bytes, &site)? != config {
                notes.push(format!(
                    "The launcher override was edited since the last install. Unrelated edits are kept: {}",
                    site.display()
                ));
            }
            if record.base_bytes.is_none()
                && record.windows_base_digest.is_none()
                && record.original_site.is_none()
                && base_config.as_ref() != Some(&split_integration(game, config).0)
            {
                notes.push(format!(
                    "The launcher override differs from Steam's base launcher, and this older installation did not record which settings are yours. If you did not edit it, close the game, remove {} and run repair.",
                    site.display()
                ));
            }
        }
    }
    if !record.profiles.is_dir() {
        problems.push(format!(
            "Selected profile directory is missing: {}",
            record.profiles.display()
        ));
    }
    read_profiles_if_exists(&record.profiles)?;
    problems.extend(interrupted(game));
    Ok((problems, notes))
}

pub fn installation_status(game: &Path) -> Result<Option<InstallationStatus>> {
    launcher_paths(game)?;
    let Some(record) = read_record(game)? else {
        return interrupted(game).map_or(Ok(None), |problem| Err(invalid(problem)));
    };
    let problems = match inspect(game, &record) {
        Ok((problems, _)) => problems,
        Err(error) => vec![format!(
            "Installation checks could not be completed: {error}"
        )],
    };
    Ok(Some(InstallationStatus {
        package_version: record.current.package_version,
        package_digest: record.current.package_digest,
        previous_version: record.previous.map(|generation| generation.package_version),
        problems,
    }))
}

pub fn verify_install(game: &Path) -> Result<Value> {
    launcher_paths(game)?;
    let record = match (read_record(game), interrupted(game)) {
        (Ok(Some(record)), _) => record,
        (Ok(None), Some(problem)) => {
            return Ok(json!({"ok":false, "game":game, "profiles":null, "problems":[problem]}));
        }
        (Err(error), Some(problem)) => return Err(invalid(format!("{problem}. {error}"))),
        (Ok(None), None) => return Err(invalid("LuaBuddy native runtime is not installed")),
        (Err(error), None) => return Err(error),
    };
    let (problems, notes) = inspect(game, &record)?;
    let ok = problems.is_empty();
    Ok(
        json!({"ok":ok, "game":game, "profiles":record.profiles, "packageVersion":record.current.package_version, "packageDigest":record.current.package_digest, "runtime":record.current.runtime, "runtimeVerified":false, "problems":problems, "notes":notes, "jvm":record.jvm}),
    )
}

pub fn recover_install(game: &Path) -> Result<Value> {
    launcher_paths(game)?;
    let _lock = InstallLock::acquire(game)?;
    let recovered = recover(game)?;
    Ok(json!({"operation":"recover", "recovered":recovered}))
}

pub fn rollback(game: &Path) -> Result<Value> {
    let (base, site) = launcher_paths(game)?;
    let _lock = InstallLock::acquire(game)?;
    recover(game)?;
    let mut record =
        read_record(game)?.ok_or_else(|| invalid("LuaBuddy native runtime is not installed"))?;
    let mut previous = record
        .previous
        .take()
        .ok_or_else(|| invalid("No previous native runtime generation is available"))?;
    for (path, digest) in &previous.files {
        if sha256(&bytes(path, RUNTIME_LIMIT)?) != *digest {
            return Err(invalid(format!(
                "Previous generation integrity failed for {}; rollback refused",
                path.display()
            )));
        }
    }
    let current_site = optional_bytes(&site, METADATA_LIMIT)?;
    let base_bytes = bytes(&base, METADATA_LIMIT)?;
    let base_config = launcher_config(&base_bytes, &base)?;
    let (mut config, follows_base) = match &current_site {
        Some(data) => launcher_source(game, &site, data, &record, Some(&base_config))?,
        None => (split_integration(game, base_config).0, true),
    };
    record.base_bytes = follows_base.then_some(base_bytes);
    record.windows_base_digest = None;
    let mut args = vm_args(&config);
    args.extend(split_integration(game, launcher_config(&previous.site_bytes, &site)?).1);
    config["vmArgs"] = json!(args);
    let profile_argument = args
        .iter()
        .rev()
        .find(|arg| property(arg, "luabuddy.config"))
        .ok_or_else(|| invalid("Previous generation has no profile directory argument"))?;
    let profiles = profile_argument
        .strip_prefix("-Dluabuddy.config=")
        .ok_or_else(|| invalid("Previous generation has an invalid profile directory argument"))?;
    if !Path::new(profiles).is_absolute() || profiles.contains(['\0', '\r', '\n']) {
        return Err(invalid(
            "Previous generation has an invalid profile directory",
        ));
    }
    record.profiles = PathBuf::from(profiles);
    previous.config_argument = profile_argument.to_owned();
    previous.site_bytes = json_bytes(&config)?;
    record.previous = Some(std::mem::replace(&mut record.current, previous));
    let record_path = game.join(INSTALL_DIR).join(RECORD_FILE);
    let changes = vec![
        Change {
            before: optional_bytes(&record_path, METADATA_LIMIT)?,
            path: record_path,
            after: Some(json_bytes(&record)?),
            executable: false,
        },
        Change {
            path: site,
            before: current_site,
            after: Some(record.current.site_bytes.clone()),
            executable: false,
        },
    ];
    let changed = transact(game, &record.profiles, &[], changes, BTreeMap::new())?;
    Ok(
        json!({"operation":"rollback", "changed":changed, "packageVersion":record.current.package_version, "profilesPreserved":record.profiles, "restartRequired":true}),
    )
}

pub fn uninstall(game: &Path) -> Result<Value> {
    let (base, site) = supported_launcher_files(game)?;
    let _lock = InstallLock::acquire(game)?;
    // Removal needs only the installation record, so an unusable journal must
    // not prevent it.
    let journal = match read_journal(game) {
        Ok(_) => None,
        Err(_) => Some(set_journal_aside(game)?),
    };
    let recovered = recover(game)?;
    let Some(record) = read_record(game)? else {
        return Ok(
            json!({"operation":"uninstall", "changed":false, "recoveredInterruptedOperation":recovered, "invalidJournalPreserved":journal}),
        );
    };
    let current_site = optional_bytes(&site, METADATA_LIMIT)?;
    // The override must be removable whatever Steam's base file became.
    let base_config = bytes(&base, METADATA_LIMIT)
        .and_then(|data| launcher_config(&data, &base))
        .ok();
    let (launcher, broken_launcher) =
        recorded_if_broken(game, &site, current_site.clone(), &record)?;
    let restored = match launcher {
        None => None,
        Some(data) => {
            let (source, _) = launcher_source(game, &site, &data, &record, base_config.as_ref())?;
            match &record.original_site {
                Some(original) if source == launcher_config(original, &site)? => {
                    Some(original.clone())
                }
                _ if base_config.as_ref() == Some(&source)
                    || base_config.is_none()
                        && recorded_base(&record, &source)?.as_ref() == Some(&source) =>
                {
                    None
                }
                _ => Some(json_bytes(&source)?),
            }
        }
    };
    let notes = restored.as_ref().map(|_| {
        format!(
            "{} stays because it holds your own settings. It replaces Steam's base launcher entirely, so compare them if the game does not start.",
            site.display()
        )
    });
    let mut changes = vec![Change {
        path: site,
        before: current_site,
        after: restored,
        executable: false,
    }];
    let owned = std::iter::once(&record.current)
        .chain(record.previous.iter())
        .flat_map(|generation| generation.files.clone())
        .collect();
    let (retired, retained) = retire(&owned)?;
    let record_path = game.join(INSTALL_DIR).join(RECORD_FILE);
    changes.push(Change {
        before: optional_bytes(&record_path, METADATA_LIMIT)?,
        path: record_path,
        after: None,
        executable: false,
    });
    let changed = transact(game, &record.profiles, &[], changes, retired)?;
    Ok(
        json!({"operation":"uninstall", "changed":changed, "profilesPreserved":record.profiles, "modifiedFilesPreserved":retained, "invalidLauncherPreserved":broken_launcher, "invalidJournalPreserved":journal, "notes":Vec::from_iter(notes), "restartRequired":true}),
    )
}

fn valid_utc_timestamp(value: &str) -> bool {
    let body = value
        .strip_suffix('Z')
        .or_else(|| value.strip_suffix("+00:00"));
    let Some(body) = body else {
        return false;
    };
    let Some(base) = body.as_bytes().get(..19) else {
        return false;
    };
    for (index, byte) in base.iter().enumerate() {
        let expected = match index {
            4 | 7 => Some(b'-'),
            10 => Some(b'T'),
            13 | 16 => Some(b':'),
            _ => None,
        };
        if expected.map_or_else(|| !byte.is_ascii_digit(), |expected| *byte != expected) {
            return false;
        }
    }
    let fraction = &body[19..];
    if !fraction.is_empty()
        && (!fraction.starts_with('.')
            || !(2..=10).contains(&fraction.len())
            || !fraction[1..].bytes().all(|b| b.is_ascii_digit()))
    {
        return false;
    }
    let number = |start: usize, end: usize| body[start..end].parse::<u32>().ok();
    let (Some(year), Some(month), Some(day), Some(hour), Some(minute), Some(second)) = (
        number(0, 4),
        number(5, 7),
        number(8, 10),
        number(11, 13),
        number(14, 16),
        number(17, 19),
    ) else {
        return false;
    };
    if year == 0 || !(1..=12).contains(&month) || hour > 23 || minute > 59 || second > 59 {
        return false;
    }
    let leap = year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400));
    let maximum_day = match month {
        2 if leap => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    };
    (1..=maximum_day).contains(&day)
}

fn valid_session(value: &str) -> bool {
    let Some(uuid) = value.as_bytes().get(..36) else {
        return false;
    };
    for (index, byte) in uuid.iter().enumerate() {
        if [8, 13, 18, 23].contains(&index) {
            if *byte != b'-' {
                return false;
            }
        } else if !byte.is_ascii_hexdigit() {
            return false;
        }
    }
    value.len() == 36
        || value.get(36..).is_some_and(|suffix| {
            suffix.strip_prefix('-').is_some_and(|count| {
                !count.is_empty() && count.bytes().all(|byte| byte.is_ascii_digit())
            })
        })
}

pub fn diagnostics(cache: &Path) -> Result<Value> {
    let path = cache.join("LuaBuddy/diagnostics.json");
    let mut value: Value = read_json(&path)?;
    if !value.is_object() {
        return Err(invalid("LuaBuddy diagnostic report must be an object"));
    }
    let report: luabuddy_core::diagnostics::DiagnosticReport =
        serde_json::from_value(value.clone()).map_err(|e| {
            invalid(format!(
                "Invalid LuaBuddy diagnostic report {}: {e}",
                path.display()
            ))
        })?;
    if report.schema_version != SCHEMA
        || report.kind != "loader-initialization"
        || !valid_session(&report.session)
        || !valid_utc_timestamp(&report.started_at)
        || report
            .completed_at
            .as_ref()
            .is_some_and(|timestamp| !valid_utc_timestamp(timestamp))
        || !matches!(report.operation.as_str(), "startup" | "refresh" | "reload")
    {
        return Err(invalid(
            "Unsupported or incomplete LuaBuddy diagnostic report identity",
        ));
    }
    if report
        .duration_ms
        .is_some_and(|duration| !duration.is_finite() || duration < 0.0)
    {
        return Err(invalid(
            "Diagnostic duration must be finite and nonnegative",
        ));
    }
    for key in ["loaded", "serverLoaded", "failed", "disabled", "pending"] {
        if !report.summary.contains_key(key) {
            return Err(invalid(format!("Diagnostic summary is missing {key}")));
        }
    }
    use luabuddy_core::diagnostics::ModStatus;
    let failed = report.error.is_some()
        || report.summary.get("failed").is_some_and(|count| *count > 0)
        || report
            .mods
            .values()
            .any(|detail| matches!(detail.status, ModStatus::Failed | ModStatus::Quarantined));
    let incomplete = report.completed_at.is_none()
        || report.duration_ms.is_none()
        || report
            .summary
            .get("pending")
            .is_some_and(|count| *count > 0)
        || report
            .mods
            .values()
            .any(|detail| detail.status == ModStatus::Prepared);
    value["ok"] = json!(!failed && !incomplete);
    value["reportStatus"] = json!(if failed {
        "failed"
    } else if incomplete {
        "incomplete"
    } else {
        "completed"
    });
    Ok(value)
}

pub fn doctor(game: &Path, cache: &Path) -> Result<Value> {
    let mut value = verify_install(game)?;
    let path = cache.join("LuaBuddy/diagnostics.json");
    if !path.exists() {
        value["lastInitialization"] = Value::Null;
        value["warnings"] = json!([
            "No runtime diagnostic report is available. Installed files do not establish successful in-game initialization."
        ]);
        return Ok(value);
    }
    match diagnostics(cache) {
        Ok(report) => {
            if report["ok"] == false {
                value["ok"] = json!(false);
            }
            if report["configDirectory"] != value["profiles"] {
                value["warnings"] = json!([
                    "The last runtime report used a different profile directory from the installed launcher generation."
                ]);
            }
            value["lastInitialization"] = report;
        }
        Err(error) => {
            value["ok"] = json!(false);
            value["lastInitializationError"] = json!(error.to_string());
        }
    }
    Ok(value)
}

#[cfg(test)]
mod steam_discovery_tests {
    use super::*;

    #[test]
    fn vdf_keeps_windows_drive_paths_spaces_and_escaped_or_literal_separators() {
        assert_eq!(
            vdf_strings(
                r#""path" "D:\\Steam Libraries\\雪" "path" "E:\Other Games" "path" "\\\\host\\share""#
            ),
            [
                "path",
                r"D:\Steam Libraries\雪",
                "path",
                r"E:\Other Games",
                "path",
                r"\\host\share"
            ]
        );
    }

    #[test]
    fn a_secondary_library_is_discovered_without_consulting_a_real_steam_account() {
        let root = env::temp_dir().join(format!("luabuddy-steam-test-{}", std::process::id()));
        let primary = root.join("primary");
        let secondary = root.join("second library 雪");
        fs::create_dir(&root).unwrap();
        fs::create_dir_all(primary.join("steamapps")).unwrap();
        let game = secondary
            .join("steamapps")
            .join("common")
            .join("ProjectZomboid");
        fs::create_dir_all(game.join("java")).unwrap();
        fs::write(game.join("java").join("projectzomboid.jar"), []).unwrap();
        let escaped = secondary.to_str().unwrap().replace('\\', "\\\\");
        fs::write(
            primary.join("steamapps").join("libraryfolders.vdf"),
            format!(r#""libraryfolders" {{ "1" {{ "path" "{escaped}" }} }}"#),
        )
        .unwrap();
        let (found, warnings) = libraries_from(BTreeSet::from([primary]));
        assert!(warnings.is_empty());
        assert!(found.contains(&secondary));
        assert!(game_directory(&game).is_some());
        #[cfg(windows)]
        assert!(
            !game_directory(&game)
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with(r"\\?\")
        );
        fs::remove_dir_all(root).unwrap();
    }
}
