use luabuddy_core::{ModId, Profile, read_profiles};
pub mod extensions;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

const SCHEMA: u32 = 1;
const METADATA_LIMIT: u64 = 4 * 1024 * 1024;
const RUNTIME_LIMIT: u64 = 256 * 1024 * 1024;
const JOURNAL_LIMIT: u64 = 512 * 1024 * 1024;
const INSTALL_DIR: &str = ".luabuddy";
const RECORD_FILE: &str = "native-install.json";
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
fn bytes(path: &Path, limit: u64) -> Result<Vec<u8>> {
    let metadata = fs::symlink_metadata(path).map_err(|e| io_error("Inspect", path, e))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > limit {
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
fn absolute(path: &Path) -> Result<PathBuf> {
    if path.is_absolute() {
        Ok(path.to_owned())
    } else {
        env::current_dir()
            .map(|cwd| cwd.join(path))
            .map_err(|e| io_error("Read working directory", path, e))
    }
}
fn plain_directory(path: &Path) -> Result<()> {
    for ancestor in path.ancestors() {
        if let Ok(metadata) = fs::symlink_metadata(ancestor)
            && metadata.file_type().is_symlink()
        {
            return Err(invalid(format!(
                "Refusing symlink in an install destination: {}",
                ancestor.display()
            )));
        }
    }
    if path.exists() {
        let metadata = fs::symlink_metadata(path).map_err(|e| io_error("Inspect", path, e))?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
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

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct HookMigration {
    sha256: String,
    source: String,
    hook: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MigrationManifest {
    #[serde(rename = "schemaVersion")]
    schema: u32,
    hooks: Vec<HookMigration>,
}

fn migration_manifest(directory: &Path) -> Result<Option<MigrationManifest>> {
    let path = directory.join("profiles/migration.json");
    let Some(data) = optional_bytes(&path, METADATA_LIMIT)? else {
        return Ok(None);
    };
    let manifest: MigrationManifest = serde_json::from_slice(&data)
        .map_err(|e| invalid(format!("Invalid hook migration manifest: {e}")))?;
    if manifest.schema != SCHEMA {
        return Err(invalid("Unsupported hook migration schema"));
    }
    let mut entries = BTreeSet::new();
    for entry in &manifest.hooks {
        if portable_path(&entry.source)?.components().count() != 1
            || entry.sha256.len() != 64
            || !entry
                .sha256
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        {
            return Err(invalid("Invalid hook migration source or SHA256"));
        }
        let checked = Profile::parse(
            "migration.properties",
            &format!("id=Migration\nhook={}\n", entry.hook),
        )?;
        if checked.hook.as_deref() != Some(&entry.hook) {
            return Err(invalid(
                "Invalid stable hook identifier in migration metadata",
            ));
        }
        if !entries.insert((&entry.source, &entry.sha256)) {
            return Err(invalid("Duplicate hook migration identity"));
        }
    }
    Ok(Some(manifest))
}

fn walk_files(root: &Path, directory: &Path, result: &mut BTreeSet<String>) -> Result<()> {
    for entry in fs::read_dir(directory).map_err(|e| io_error("Read directory", directory, e))? {
        let entry = entry.map_err(|e| io_error("Read directory entry", directory, e))?;
        let path = entry.path();
        let kind = entry
            .file_type()
            .map_err(|e| io_error("Inspect", &path, e))?;
        if kind.is_symlink() {
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
    for profile in read_profiles_if_exists(&directory.join("profiles"))? {
        if let Some(error) = profile.hook_migration_error() {
            return Err(error.into());
        }
    }
    migration_manifest(&directory)?;
    Ok(Package {
        directory,
        manifest,
        digest,
    })
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
    for profile in &profiles {
        if let Some(error) = profile.hook_migration_error() {
            return Err(error.into());
        }
    }
    let output = absolute(output)?;
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
        if let Some(data) = optional_bytes(&templates.join("migration.json"), METADATA_LIMIT)? {
            add_package_file(&stage, &mut files, "profiles/migration.json", &data, false)?;
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
    [path.to_owned(), path.join("projectzomboid")]
        .into_iter()
        .find(|p| {
            p.join("projectzomboid.jar").is_file() || p.join("java/projectzomboid.jar").is_file()
        })
        .and_then(|p| fs::canonicalize(p).ok())
}

fn vdf_strings(text: &str) -> Vec<String> {
    let mut result = Vec::new();
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '"' {
            continue;
        }
        let mut value = String::new();
        while let Some(c) = chars.next() {
            match c {
                '"' => break,
                '\\' => {
                    if let Some(c) = chars.next() {
                        value.push(c);
                    }
                }
                _ => value.push(c),
            }
        }
        result.push(value);
    }
    result
}

pub fn steam_libraries() -> Result<Vec<PathBuf>> {
    let home = env::var_os("HOME")
        .or_else(|| env::var_os("USERPROFILE"))
        .map(PathBuf::from);
    let mut roots = BTreeSet::new();
    if let Some(home) = home {
        roots.extend([
            home.join(".local/share/Steam"),
            home.join(".steam/steam"),
            home.join(".var/app/com.valvesoftware.Steam/.local/share/Steam"),
            home.join("Library/Application Support/Steam"),
        ]);
    }
    for key in ["ProgramFiles(x86)", "ProgramFiles", "LOCALAPPDATA"] {
        if let Some(path) = env::var_os(key) {
            roots.insert(PathBuf::from(path).join("Steam"));
        }
    }
    let mut libraries = roots.clone();
    for root in roots {
        if let Some(data) =
            optional_bytes(&root.join("steamapps/libraryfolders.vdf"), METADATA_LIMIT)?
        {
            let text = String::from_utf8(data)
                .map_err(|_| invalid("Steam libraryfolders.vdf is not UTF-8"))?;
            let tokens = vdf_strings(&text);
            for pair in tokens.windows(2) {
                if pair[0] == "path" {
                    libraries.insert(PathBuf::from(&pair[1]));
                }
            }
        }
    }
    Ok(libraries.into_iter().collect())
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
    let found: BTreeSet<_> = steam_libraries()?
        .into_iter()
        .filter_map(|p| game_directory(&p.join("steamapps/common/ProjectZomboid")))
        .collect();
    if found.len() != 1 {
        return Err(invalid(format!(
            "Specify --game-dir or PZ_GAME_DIR; Steam discovery found {} installations",
            found.len()
        )));
    }
    found
        .into_iter()
        .next()
        .ok_or_else(|| invalid("No game installation found"))
}

pub fn probe_jvm(game: &Path) -> Result<JvmIdentity> {
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
    let output = Command::new(&executable)
        .args(["-XshowSettings:properties", "-version"])
        .env_remove("JAVA_TOOL_OPTIONS")
        .env_remove("JDK_JAVA_OPTIONS")
        .env_remove("_JAVA_OPTIONS")
        .output()
        .map_err(|e| io_error("Run bundled JVM architecture probe", &executable, e))?;
    if !output.status.success() {
        return Err(invalid(format!(
            "Bundled JVM probe failed with {}",
            output.status
        )));
    }
    let text = String::from_utf8(output.stderr)
        .map_err(|_| invalid("Bundled JVM identity output is not UTF-8"))?;
    if text.len() as u64 > METADATA_LIMIT {
        return Err(invalid("Bundled JVM identity output is too large"));
    }
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

fn launcher_paths(game: &Path) -> Result<(PathBuf, PathBuf)> {
    if !cfg!(target_os = "linux") {
        return Err(invalid(
            "Native launcher integration has only been proven on Linux; this platform is unverified",
        ));
    }
    let base = game.join("ProjectZomboid64.json");
    if !base.is_file() {
        return Err(invalid(format!(
            "Native launcher configuration missing: {}",
            base.display()
        )));
    }
    Ok((base, game.join("ProjectZomboid64.site.json")))
}

fn launcher_config(data: &[u8]) -> Result<Value> {
    let value: Value =
        serde_json::from_slice(data).map_err(|e| invalid(format!("Invalid launcher JSON: {e}")))?;
    let args = value
        .get("vmArgs")
        .and_then(Value::as_array)
        .ok_or_else(|| invalid("Launcher vmArgs must be an array of strings"))?;
    if !value.is_object() || args.iter().any(|arg| !arg.is_string()) {
        return Err(invalid("Launcher vmArgs must be an array of strings"));
    }
    Ok(value)
}

fn owned_legacy_agent(argument: &str) -> bool {
    let Some((agent, options)) = argument
        .strip_prefix("-javaagent:")
        .and_then(|s| s.split_once('='))
    else {
        return false;
    };
    if !agent
        .replace('\\', "/")
        .rsplit('/')
        .next()
        .is_some_and(|n| n.eq_ignore_ascii_case("ZombieBuddy.jar"))
    {
        return false;
    }
    let Some(patches) = options.strip_prefix("patches_jar=") else {
        return false;
    };
    if patches.contains([',', ';']) {
        return false;
    }
    let normalized = patches.replace('\\', "/");
    normalized == "LuaBuddy.jar:luabuddy"
        || normalized.ends_with("/LuaBuddy.jar:luabuddy")
        || normalized == "PZClientMods.jar:localmods"
        || normalized.ends_with("/PZClientMods.jar:localmods")
}

fn refuse_shared_legacy(argument: &str) -> Result<()> {
    if argument.starts_with("-javaagent:")
        && argument.contains("LuaBuddy.jar:luabuddy")
        && !owned_legacy_agent(argument)
    {
        return Err(invalid(
            "LuaBuddy shares a ZombieBuddy agent argument with other patches. Remove only its patches_jar entry manually before migrating; no files changed.",
        ));
    }
    Ok(())
}

fn owned_property(argument: &str) -> bool {
    [
        "luabuddy.config",
        "luabuddy.templates",
        "luabuddy.debug",
        "luabuddy.earlyUI",
        "localmods.config",
    ]
    .iter()
    .any(|name| argument == format!("-D{name}") || argument.starts_with(&format!("-D{name}=")))
}

fn without_integration(data: &[u8], record: &InstallRecord) -> Result<Value> {
    let mut config = launcher_config(data)?;
    let generations: Vec<_> = std::iter::once(&record.current)
        .chain(record.previous.iter())
        .collect();
    let args = config["vmArgs"]
        .as_array()
        .ok_or_else(|| invalid("Invalid launcher arguments"))?
        .iter()
        .filter(|value| {
            value.as_str().is_none_or(|arg| {
                !owned_property(arg)
                    && !owned_legacy_agent(arg)
                    && !generations
                        .iter()
                        .any(|generation| arg == generation.agent_argument)
            })
        })
        .cloned()
        .collect::<Vec<_>>();
    config["vmArgs"] = json!(args);
    Ok(config)
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

fn write_atomic(path: &Path, data: Option<&[u8]>, executable: bool) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| invalid("Install file has no parent directory"))?;
    plain_directory(parent)?;
    if let Some(data) = data {
        let stage = stage_path(path)?;
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
        if path.exists() {
            fs::remove_file(path).map_err(|e| io_error("Remove owned file", path, e))?;
        }
        sync_directory(parent)
    }
}

struct InstallLock {
    path: PathBuf,
    identity: String,
}

impl InstallLock {
    fn acquire(game: &Path) -> Result<Self> {
        let root = game.join(INSTALL_DIR);
        plain_directory(&root)?;
        let path = root.join("native-install.lock");
        let identity = process_identity(std::process::id())?;
        if let Some(data) = optional_bytes(&path, 4096)? {
            let prior = String::from_utf8(data).map_err(|_| {
                invalid("Invalid native installer lock; preserve it for inspection")
            })?;
            let pid: u32 = prior
                .split_once(':')
                .and_then(|(pid, _)| pid.parse().ok())
                .ok_or_else(|| invalid("Invalid native installer lock identity"))?;
            match process_identity(pid) {
                Ok(current) if current == prior => {
                    return Err(invalid(format!(
                        "Another native installer is running with process ID {pid}"
                    )));
                }
                Ok(_) => fs::remove_file(&path)
                    .map_err(|e| io_error("Remove stale native installer lock", &path, e))?,
                Err(_) if !Path::new(&format!("/proc/{pid}")).exists() => fs::remove_file(&path)
                    .map_err(|e| io_error("Remove stale native installer lock", &path, e))?,
                Err(error) => return Err(error),
            }
        }
        write_new(&path, identity.as_bytes(), false)?;
        Ok(Self { path, identity })
    }
    fn release(self) -> Result<()> {
        if optional_bytes(&self.path, 4096)?.as_deref() != Some(self.identity.as_bytes()) {
            return Err(invalid("Native installer lock ownership changed"));
        }
        fs::remove_file(&self.path)
            .map_err(|e| io_error("Release native installer lock", &self.path, e))
    }
}

impl Drop for InstallLock {
    fn drop(&mut self) {
        if fs::read(&self.path).ok().as_deref() == Some(self.identity.as_bytes()) {
            let _ = fs::remove_file(&self.path);
        }
    }
}

fn process_identity(pid: u32) -> Result<String> {
    #[cfg(target_os = "linux")]
    {
        let path = PathBuf::from(format!("/proc/{pid}/stat"));
        let text = fs::read_to_string(&path)
            .map_err(|e| io_error("Read installer process identity", &path, e))?;
        let (_, fields) = text
            .rsplit_once(')')
            .ok_or_else(|| invalid("Malformed process identity"))?;
        let start = fields
            .split_whitespace()
            .nth(19)
            .ok_or_else(|| invalid("Process identity lacks start time"))?;
        Ok(format!("{pid}:{start}"))
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = pid;
        Err(invalid(
            "Native installation is unverified on this platform",
        ))
    }
}

fn validate_transaction(game: &Path, journal: &Journal) -> Result<()> {
    if journal.schema != SCHEMA || journal.game != game || !journal.profiles.is_absolute() {
        return Err(invalid(
            "Native transaction journal identity does not match this game",
        ));
    }
    let (_, site) = launcher_paths(game)?;
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
    Ok(())
}

fn recover(game: &Path) -> Result<bool> {
    let journal_path = game.join(INSTALL_DIR).join(JOURNAL_FILE);
    let stage = stage_path(&journal_path)?;
    if !journal_path.exists() && stage.exists() {
        let candidate: std::result::Result<Journal, Error> =
            bytes(&stage, JOURNAL_LIMIT).and_then(|data| {
                serde_json::from_slice(&data)
                    .map_err(|e| invalid(format!("Read transaction journal: {e}")))
            });
        match candidate {
            Ok(journal) => {
                validate_transaction(game, &journal)?;
                fs::rename(&stage, &journal_path)
                    .map_err(|e| io_error("Recover journal preparation", &journal_path, e))?;
            }
            Err(_) => {
                fs::remove_file(&stage)
                    .map_err(|e| io_error("Remove incomplete journal preparation", &stage, e))?;
                return Ok(true);
            }
        }
    }
    if !journal_path.exists() {
        return Ok(false);
    }
    let journal: Journal = serde_json::from_slice(&bytes(&journal_path, JOURNAL_LIMIT)?)
        .map_err(|e| invalid(format!("Read transaction journal: {e}")))?;
    validate_transaction(game, &journal)?;
    for change in journal.changes.iter().rev() {
        let temporary = stage_path(&change.path)?;
        if optional_bytes(&temporary, RUNTIME_LIMIT)?.is_some() {
            fs::remove_file(&temporary)
                .map_err(|e| io_error("Remove interrupted stage", &temporary, e))?;
        }
        let current = optional_bytes(&change.path, RUNTIME_LIMIT)?;
        if current == change.before {
            continue;
        }
        if current != change.after {
            return Err(invalid(format!(
                "Interrupted install encountered user-modified file {}; preserve it and its transaction journal",
                change.path.display()
            )));
        }
        write_atomic(&change.path, change.before.as_deref(), change.executable)?;
    }
    fs::remove_file(&journal_path)
        .map_err(|e| io_error("Complete transaction recovery", &journal_path, e))?;
    sync_directory(&game.join(INSTALL_DIR))?;
    Ok(true)
}

fn transact(game: &Path, profiles: &Path, changes: Vec<Change>) -> Result<bool> {
    let changes: Vec<_> = changes
        .into_iter()
        .filter(|change| change.before != change.after)
        .collect();
    if changes.is_empty() {
        return Ok(false);
    }
    let journal = Journal {
        schema: SCHEMA,
        game: game.to_owned(),
        profiles: profiles.to_owned(),
        changes,
    };
    validate_transaction(game, &journal)?;
    let journal_path = game.join(INSTALL_DIR).join(JOURNAL_FILE);
    let encoded = json_bytes(&journal)?;
    if encoded.len() as u64 > JOURNAL_LIMIT {
        return Err(invalid(
            "Install transaction is too large for recoverable metadata",
        ));
    }
    write_atomic(&journal_path, Some(&encoded), false)?;
    for change in &journal.changes {
        if optional_bytes(&change.path, RUNTIME_LIMIT)? != change.before {
            let error = invalid(format!(
                "Install file changed during preparation: {}",
                change.path.display()
            ));
            return match recover(game) {
                Ok(_) => Err(error),
                Err(recovery) => Err(invalid(format!(
                    "{error}; recovery also failed: {recovery}"
                ))),
            };
        }
        if let Err(error) = write_atomic(&change.path, change.after.as_deref(), change.executable) {
            return match recover(game) {
                Ok(_) => Err(error),
                Err(recovery) => Err(invalid(format!(
                    "{error}; recovery also failed: {recovery}"
                ))),
            };
        }
    }
    fs::remove_file(&journal_path)
        .map_err(|e| io_error("Commit native install transaction", &journal_path, e))?;
    sync_directory(&game.join(INSTALL_DIR))?;
    Ok(true)
}

fn read_record(game: &Path) -> Result<Option<InstallRecord>> {
    let path = game.join(INSTALL_DIR).join(RECORD_FILE);
    let Some(data) = optional_bytes(&path, METADATA_LIMIT)? else {
        return Ok(None);
    };
    let record: InstallRecord = serde_json::from_slice(&data)
        .map_err(|e| invalid(format!("Invalid native installation record: {e}")))?;
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
            for ancestor in path.ancestors() {
                if fs::symlink_metadata(ancestor)
                    .is_ok_and(|metadata| metadata.file_type().is_symlink())
                {
                    return Err(invalid(format!(
                        "Native generation contains a symlink path: {}",
                        ancestor.display()
                    )));
                }
            }
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

fn selected_profile_files(
    ids: &[String],
    directory: &Path,
    package: &Package,
) -> Result<Vec<(PathBuf, Vec<u8>)>> {
    let existing = read_profiles_if_exists(directory)?;
    let mut file_names = BTreeMap::new();
    if directory.exists() {
        for entry in
            fs::read_dir(directory).map_err(|e| io_error("Read profile directory", directory, e))?
        {
            let entry =
                entry.map_err(|e| io_error("Read profile directory entry", directory, e))?;
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| invalid("Profile filenames must be UTF-8"))?;
            file_names
                .entry(name.to_lowercase())
                .or_insert_with(Vec::new)
                .push((name, entry.path()));
        }
    }
    let templates = read_profiles_if_exists(&package.directory.join("profiles"))?;
    let templates: BTreeMap<_, _> = templates.into_iter().map(|p| (p.id.clone(), p)).collect();
    let mut selected = BTreeMap::new();
    let mut result = Vec::new();
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
        let target = directory.join(&name);
        if let Some(matches) = file_names.get(&name.to_lowercase()) {
            if matches.len() != 1 || matches[0].0 != name {
                return Err(invalid(format!("Profile filename collision for {name}")));
            }
            if !existing
                .iter()
                .any(|profile| profile.file == target && profile.id == id)
            {
                return Err(invalid(format!(
                    "Existing profile {name} belongs to another mod"
                )));
            }
        }
        if existing
            .iter()
            .any(|profile| profile.id == id && profile.file != target)
        {
            return Err(invalid(format!(
                "Mod ID {id} already exists under another profile filename"
            )));
        }
        if target.exists() {
            continue;
        }
        let content = if let Some(template) = templates.get(&id) {
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
        let profile = Profile::parse(
            &target,
            std::str::from_utf8(&content).map_err(|_| invalid("Profile is not UTF-8"))?,
        )?;
        if let Some(error) = profile.hook_migration_error() {
            return Err(error.into());
        }
        result.push((target, content));
    }
    Ok(result)
}

fn migrate_profiles(game: &Path, directory: &Path, package: &Package) -> Result<Vec<Change>> {
    let manifest = migration_manifest(&package.directory)?;
    let mut changes = Vec::new();
    for profile in read_profiles_if_exists(directory)? {
        let Some(hook_file) = &profile.legacy_plugin else {
            continue;
        };
        let hook = bytes(hook_file, METADATA_LIMIT).map_err(|error| {
            invalid(format!(
                "{}; {}",
                profile
                    .hook_migration_error()
                    .map(|error| error.to_string())
                    .unwrap_or_default(),
                error
            ))
        })?;
        let digest = sha256(&hook);
        let name = hook_file.file_name().and_then(|name| name.to_str());
        let matched = manifest.as_ref().and_then(|manifest| {
            manifest
                .hooks
                .iter()
                .find(|entry| Some(entry.source.as_str()) == name && entry.sha256 == digest)
        });
        let Some(matched) = matched else {
            return Err(invalid(format!(
                "{} The supplied package has no exact source-name and SHA256 migration for this hook; custom or modified hooks require a Rust port.",
                profile
                    .hook_migration_error()
                    .map(|error| error.to_string())
                    .unwrap_or_default()
            )));
        };
        let original = bytes(&profile.file, METADATA_LIMIT)?;
        let backup = game
            .join(INSTALL_DIR)
            .join("recovery/profiles")
            .join(format!("{}.properties", sha256(&original)));
        let before_backup = optional_bytes(&backup, METADATA_LIMIT)?;
        if before_backup.as_ref().is_some_and(|data| data != &original) {
            return Err(invalid(
                "Profile migration recovery filename has conflicting contents",
            ));
        }
        changes.push(Change {
            path: backup,
            before: before_backup,
            after: Some(original.clone()),
            executable: false,
        });
        let mut migrated = original.clone();
        migrated.extend_from_slice(format!("\n\nplugin=\nhook={}\n", matched.hook).as_bytes());
        let checked = Profile::parse(
            &profile.file,
            std::str::from_utf8(&migrated)
                .map_err(|_| invalid("Profile migration input is not UTF-8"))?,
        )?;
        if checked.legacy_plugin.is_some() || checked.hook.as_deref() != Some(&matched.hook) {
            return Err(invalid(
                "Profile migration did not produce its declared Rust hook",
            ));
        }
        changes.push(Change {
            path: profile.file,
            before: Some(original),
            after: Some(migrated),
            executable: false,
        });
    }
    Ok(changes)
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
    let lock = InstallLock::acquire(game)?;
    let recovered = recover(game)?;
    let prior = read_record(game)?;
    let original_site = optional_bytes(&site, METADATA_LIMIT)?;
    let selected_launcher = original_site
        .clone()
        .unwrap_or(bytes(&base, METADATA_LIMIT)?);
    let mut config = launcher_config(&selected_launcher)?;
    let old_args: Vec<String> = config["vmArgs"]
        .as_array()
        .ok_or_else(|| invalid("Invalid launcher arguments"))?
        .iter()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect();
    for arg in &old_args {
        refuse_shared_legacy(arg)?;
    }
    let old_profile = old_args
        .iter()
        .rev()
        .find_map(|arg| arg.strip_prefix("-Dluabuddy.config="))
        .map(PathBuf::from);
    let profiles = absolute(
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
    let migration_changes = migrate_profiles(game, &profiles, &package)?;
    let profile_files = selected_profile_files(&options.mods, &profiles, &package)?;
    plain_directory(&profiles)?;
    let runtime_content = bytes(
        &package.directory.join(&package.manifest.runtime),
        RUNTIME_LIMIT,
    )?;
    let runtime_digest = sha256(&runtime_content);
    let generation_root = game.join(INSTALL_DIR).join("native").join(&package.digest);
    let runtime = generation_root.join(&package.manifest.runtime);
    let runtime_before = optional_bytes(&runtime, RUNTIME_LIMIT)?;
    if runtime_before
        .as_ref()
        .is_some_and(|content| sha256(content) != runtime_digest)
        && !options.repair
    {
        return Err(invalid(format!(
            "Installed runtime was modified: {}; use repair with the original verified package to preserve a recovery copy",
            runtime.display()
        )));
    }
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
    let mut args = Vec::new();
    for arg in old_args {
        let prior_agent = prior.as_ref().is_some_and(|r| {
            arg == r.current.agent_argument
                || r.previous.as_ref().is_some_and(|p| arg == p.agent_argument)
        });
        if prior_agent
            || owned_legacy_agent(&arg)
            || arg.starts_with("-Dluabuddy.config=")
            || arg.starts_with("-Dluabuddy.templates=")
            || arg.starts_with("-Dlocalmods.config=")
        {
            continue;
        }
        if options.debug.is_some()
            && (arg == "-Dluabuddy.debug" || arg.starts_with("-Dluabuddy.debug="))
        {
            continue;
        }
        if options.early_ui.is_some()
            && (arg == "-Dluabuddy.earlyUI" || arg.starts_with("-Dluabuddy.earlyUI="))
        {
            continue;
        }
        args.push(arg);
    }
    if options.debug == Some(true) {
        args.push("-Dluabuddy.debug=true".into());
    }
    if options.early_ui == Some(true) {
        args.push("-Dluabuddy.earlyUI=true".into());
    }
    args.push(config_argument.clone());
    args.push(templates_argument.clone());
    args.push(agent_argument.clone());
    config["vmArgs"] = json!(args);
    let site_bytes = json_bytes(&config)?;
    let mut generation_files = BTreeMap::from([(runtime.clone(), runtime_digest.clone())]);
    let mut template_changes = Vec::new();
    for (name, file) in &package.manifest.files {
        if name.starts_with("profiles/")
            || name == "LICENSE"
            || name == "THIRD_PARTY.md"
            || name.starts_with("licenses/")
        {
            let path = generation_root.join(portable_path(name)?);
            let content = bytes(&package.directory.join(name), METADATA_LIMIT)?;
            let before = optional_bytes(&path, METADATA_LIMIT)?;
            if let Some(changed) = before.as_ref().filter(|data| sha256(data) != file.sha256) {
                if !options.repair {
                    return Err(invalid(format!(
                        "Bundled template was modified: {}; use repair with the original package to preserve a recovery copy",
                        path.display()
                    )));
                }
                let retained = game
                    .join(INSTALL_DIR)
                    .join("recovery/templates")
                    .join(sha256(changed));
                let retained_before = optional_bytes(&retained, METADATA_LIMIT)?;
                if retained_before.as_ref().is_some_and(|data| data != changed) {
                    return Err(invalid(
                        "Template recovery filename has conflicting contents",
                    ));
                }
                template_changes.push(Change {
                    path: retained,
                    before: retained_before,
                    after: Some(changed.clone()),
                    executable: false,
                });
            }
            generation_files.insert(path.clone(), file.sha256.clone());
            template_changes.push(Change {
                path,
                before,
                after: Some(content),
                executable: false,
            });
        }
    }
    plain_directory(&templates)?;
    let generation = InstalledGeneration {
        package_digest: package.digest,
        package_version: package.manifest.version,
        runtime: runtime.clone(),
        runtime_sha256: runtime_digest,
        agent_argument,
        config_argument,
        templates_argument,
        files: generation_files,
        site_bytes: site_bytes.clone(),
    };
    let same = prior.as_ref().is_some_and(|record| {
        record.current.package_digest == generation.package_digest
            && record.current.site_bytes == generation.site_bytes
    });
    let record = InstallRecord {
        schema: SCHEMA,
        game: game.to_owned(),
        profiles: profiles.clone(),
        original_site: prior
            .as_ref()
            .map(|r| r.original_site.clone())
            .unwrap_or(original_site.clone()),
        current: generation,
        previous: if same {
            prior.as_ref().and_then(|r| r.previous.clone())
        } else {
            prior.as_ref().map(|r| r.current.clone())
        },
        jvm,
    };
    let mut changes = Vec::new();
    let migrated_profiles: Vec<_> = migration_changes
        .iter()
        .filter(|change| change.path.parent() == Some(profiles.as_path()))
        .map(|change| change.path.clone())
        .collect();
    changes.extend(migration_changes);
    if options.repair
        && let Some(content) = runtime_before
            .as_ref()
            .filter(|content| sha256(content) != record.current.runtime_sha256)
    {
        let retained = game
            .join(INSTALL_DIR)
            .join("recovery")
            .join(format!("runtime-{}", sha256(content)));
        let retained_before = optional_bytes(&retained, RUNTIME_LIMIT)?;
        if retained_before.as_ref().is_some_and(|data| data != content) {
            return Err(invalid(
                "Runtime recovery filename has conflicting contents",
            ));
        }
        changes.push(Change {
            before: retained_before,
            path: retained,
            after: Some(content.clone()),
            executable: true,
        });
    }
    changes.push(Change {
        path: runtime,
        before: runtime_before,
        after: Some(runtime_content),
        executable: true,
    });
    changes.extend(template_changes);
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
        before: original_site,
        after: Some(site_bytes),
        executable: false,
    });
    let mut retained_modified = Vec::new();
    if let Some(retired) = prior
        .as_ref()
        .and_then(|r| r.previous.as_ref())
        .filter(|g| {
            g.runtime != record.current.runtime
                && record
                    .previous
                    .as_ref()
                    .is_none_or(|previous| previous.runtime != g.runtime)
        })
    {
        for (path, digest) in &retired.files {
            if let Some(content) = optional_bytes(path, RUNTIME_LIMIT)? {
                if sha256(&content) != *digest {
                    retained_modified.push(path.clone());
                    continue;
                }
                changes.insert(
                    0,
                    Change {
                        path: path.clone(),
                        before: Some(content),
                        after: None,
                        executable: path == &retired.runtime,
                    },
                );
            }
        }
    }
    let changed = transact(game, &profiles, changes)?;
    lock.release()?;
    Ok(
        json!({"operation": if options.repair { "repair" } else { "install" }, "changed":changed, "recoveredInterruptedOperation":recovered, "profiles":profiles, "migratedProfiles":migrated_profiles, "modifiedRetiredFilesPreserved":retained_modified, "runtime":record.current.runtime, "runtimeVerified":false, "nextStep":"Fully restart Project Zomboid through its normal launcher. Package integrity is verified; in-game operation requires its own runtime evidence."}),
    )
}

pub fn verify_install(game: &Path) -> Result<Value> {
    let (_, site) = launcher_paths(game)?;
    let record =
        read_record(game)?.ok_or_else(|| invalid("LuaBuddy native runtime is not installed"))?;
    let mut problems = Vec::new();
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
    if optional_bytes(&site, METADATA_LIMIT)?.as_ref() != Some(&record.current.site_bytes) {
        problems.push("Launcher override differs from the last installed generation; user edits are preserved".into());
    }
    if !record.profiles.is_dir() {
        problems.push(format!(
            "Selected profile directory is missing: {}",
            record.profiles.display()
        ));
    }
    for profile in read_profiles_if_exists(&record.profiles)? {
        if let Some(error) = profile.hook_migration_error() {
            problems.push(error.to_string());
        }
    }
    if game.join(INSTALL_DIR).join(JOURNAL_FILE).exists() {
        problems.push("An interrupted operation awaits recover, install, or uninstall".into());
    }
    let ok = problems.is_empty();
    Ok(
        json!({"ok":ok, "game":game, "profiles":record.profiles, "packageVersion":record.current.package_version, "packageDigest":record.current.package_digest, "runtime":record.current.runtime, "runtimeVerified":false, "problems":problems, "jvm":record.jvm}),
    )
}

pub fn recover_install(game: &Path) -> Result<Value> {
    launcher_paths(game)?;
    let lock = InstallLock::acquire(game)?;
    let recovered = recover(game)?;
    lock.release()?;
    Ok(json!({"operation":"recover", "recovered":recovered}))
}

pub fn rollback(game: &Path) -> Result<Value> {
    let (_, site) = launcher_paths(game)?;
    let lock = InstallLock::acquire(game)?;
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
    if current_site.as_ref() != Some(&record.current.site_bytes) {
        return Err(invalid(
            "Launcher override has user changes; rollback refused without overwriting them",
        ));
    }
    let mut config = without_integration(&record.current.site_bytes, &record)?;
    let prior_config = launcher_config(&previous.site_bytes)?;
    let owned_args = prior_config["vmArgs"]
        .as_array()
        .ok_or_else(|| invalid("Invalid previous launcher arguments"))?
        .iter()
        .filter(|value| {
            value
                .as_str()
                .is_some_and(|arg| owned_property(arg) || arg == previous.agent_argument)
        })
        .cloned()
        .collect::<Vec<_>>();
    config["vmArgs"]
        .as_array_mut()
        .ok_or_else(|| invalid("Invalid launcher arguments"))?
        .extend(owned_args);
    let profile_argument = config["vmArgs"]
        .as_array()
        .ok_or_else(|| invalid("Invalid launcher arguments"))?
        .iter()
        .rev()
        .filter_map(Value::as_str)
        .find(|arg| *arg == "-Dluabuddy.config" || arg.starts_with("-Dluabuddy.config="))
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
    let changed = transact(game, &record.profiles, changes)?;
    lock.release()?;
    Ok(
        json!({"operation":"rollback", "changed":changed, "packageVersion":record.current.package_version, "profilesPreserved":record.profiles, "restartRequired":true}),
    )
}

pub fn uninstall(game: &Path) -> Result<Value> {
    let (_, site) = launcher_paths(game)?;
    let lock = InstallLock::acquire(game)?;
    let recovered = recover(game)?;
    let Some(record) = read_record(game)? else {
        lock.release()?;
        return Ok(
            json!({"operation":"uninstall", "changed":false, "recoveredInterruptedOperation":recovered}),
        );
    };
    let current_site = optional_bytes(&site, METADATA_LIMIT)?;
    let restored = if let Some(content) = &current_site {
        let clean = without_integration(content, &record)?;
        let (base, _) = launcher_paths(game)?;
        let original = record
            .original_site
            .clone()
            .unwrap_or(bytes(&base, METADATA_LIMIT)?);
        let original_config = launcher_config(&original)?;
        let original_clean = without_integration(&original, &record)?;
        if clean == original_clean && original_config == original_clean {
            record.original_site.clone()
        } else {
            Some(json_bytes(&clean)?)
        }
    } else {
        None
    };
    let mut changes = vec![Change {
        path: site,
        before: current_site,
        after: restored,
        executable: false,
    }];
    let mut retained = Vec::new();
    let mut seen = BTreeSet::new();
    for generation in std::iter::once(&record.current).chain(record.previous.iter()) {
        for (path, digest) in &generation.files {
            if !seen.insert(path) {
                continue;
            }
            if let Some(content) = optional_bytes(path, RUNTIME_LIMIT)? {
                if sha256(&content) == *digest {
                    changes.push(Change {
                        path: path.clone(),
                        before: Some(content),
                        after: None,
                        executable: path == &generation.runtime,
                    });
                } else {
                    retained.push(path.clone());
                }
            }
        }
    }
    let record_path = game.join(INSTALL_DIR).join(RECORD_FILE);
    changes.push(Change {
        before: optional_bytes(&record_path, METADATA_LIMIT)?,
        path: record_path,
        after: None,
        executable: false,
    });
    let changed = transact(game, &record.profiles, changes)?;
    lock.release()?;
    Ok(
        json!({"operation":"uninstall", "changed":changed, "profilesPreserved":record.profiles, "modifiedFilesPreserved":retained, "sharedZombieBuddyPreserved":true, "restartRequired":true}),
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
