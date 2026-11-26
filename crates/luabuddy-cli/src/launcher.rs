use crate::*;
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

pub(crate) fn supported_launcher_files(game: &Path) -> Result<(PathBuf, PathBuf)> {
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

pub(crate) fn launcher_paths(game: &Path) -> Result<(PathBuf, PathBuf)> {
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

pub(crate) fn launcher_config(data: &[u8], path: &Path) -> Result<Value> {
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

pub(crate) fn vm_args(config: &Value) -> Vec<String> {
    config["vmArgs"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect()
}

pub(crate) fn property(argument: &str, name: &str) -> bool {
    argument
        .strip_prefix("-D")
        .and_then(|rest| rest.strip_prefix(name))
        .is_some_and(|value| value.is_empty() || value.starts_with('='))
}

pub(crate) fn owned_agent(game: &Path, argument: &str) -> bool {
    argument
        .strip_prefix("-agentpath:")
        .is_some_and(|path| Path::new(path).starts_with(game.join(INSTALL_DIR).join("native")))
}

pub(crate) fn owned_argument(game: &Path, argument: &str) -> bool {
    owned_agent(game, argument)
        || [
            props::CONFIG,
            props::TEMPLATES,
            props::DEBUG,
            props::EARLY_UI,
        ]
        .iter()
        .any(|name| property(argument, name))
}

/// Splits launcher settings into everything else and LuaBuddy's own arguments.
pub(crate) fn split_integration(game: &Path, mut config: Value) -> (Value, Vec<String>) {
    let (owned, kept): (Vec<_>, Vec<_>) = vm_args(&config)
        .into_iter()
        .partition(|argument| owned_argument(game, argument));
    config["vmArgs"] = json!(kept);
    (config, owned)
}

/// The base launcher settings the override was built from, when known. A
/// digest from an older record identifies them among the candidates at hand.
pub(crate) fn recorded_base(
    record: &InstallRecord,
    clean: &Value,
    base: Option<&Value>,
) -> Result<Option<Value>> {
    if let Some(data) = &record.base_bytes {
        return serde_json::from_slice(data)
            .map(Some)
            .map_err(|e| invalid(format!("Invalid recorded base launcher: {e}")));
    }
    for candidate in base.into_iter().chain([clean]) {
        if record.windows_base_digest.as_ref() == Some(&sha256(&json_bytes(candidate)?)) {
            return Ok(Some(candidate.clone()));
        }
    }
    Ok(None)
}

/// The settings to build on, without LuaBuddy's arguments, and whether later
/// changes to Steam's base can be told apart from user edits. An override that
/// LuaBuddy created follows Steam's base; one that predates it is the user's.
pub(crate) fn launcher_source(
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
    match (recorded_base(record, &clean, base)?, base) {
        (Some(was), Some(now)) => Ok((rebase(clean, &was, now), true)),
        (_, now) => {
            let unedited = now == Some(&clean);
            Ok((clean, unedited))
        }
    }
}

/// Carries the user's edits, the difference between `mine` and the base it was
/// built from, over to Steam's current base.
pub(crate) fn rebase(mut mine: Value, was: &Value, now: &Value) -> Value {
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
pub(crate) fn recorded_if_broken(
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
