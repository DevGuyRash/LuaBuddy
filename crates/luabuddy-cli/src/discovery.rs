use crate::*;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

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

pub(crate) fn vdf_strings(text: &str) -> Vec<String> {
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

pub(crate) fn steam_roots() -> BTreeSet<PathBuf> {
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

pub(crate) fn libraries_from(roots: BTreeSet<PathBuf>) -> (Vec<PathBuf>, Vec<String>) {
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
