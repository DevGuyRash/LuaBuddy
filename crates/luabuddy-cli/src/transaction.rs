use crate::*;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

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
pub(crate) struct Change {
    pub(crate) path: PathBuf,
    pub(crate) before: Option<Vec<u8>>,
    pub(crate) after: Option<Vec<u8>>,
    pub(crate) executable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Journal {
    pub(crate) schema: u32,
    pub(crate) game: PathBuf,
    pub(crate) profiles: PathBuf,
    pub(crate) changes: Vec<Change>,
    /// Generation files to remove, by SHA256, once `changes` are undone: those an
    /// interrupted operation had written, or those a completed one superseded.
    #[serde(default)]
    pub(crate) drop: BTreeMap<PathBuf, String>,
}

/// A content-addressed file that nothing uses until the launcher selects its
/// generation, so recovery needs only its hash.
pub(crate) struct GenerationFile {
    pub(crate) path: PathBuf,
    pub(crate) sha256: String,
    pub(crate) content: Vec<u8>,
    pub(crate) executable: bool,
}

pub(crate) fn validate_transaction(game: &Path, journal: &Journal) -> Result<()> {
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

pub(crate) fn record_path(game: &Path) -> PathBuf {
    game.join(INSTALL_DIR).join(RECORD_FILE)
}

pub(crate) fn journal_path(game: &Path) -> PathBuf {
    game.join(INSTALL_DIR).join(JOURNAL_FILE)
}

pub(crate) fn read_journal(game: &Path) -> Result<Option<Journal>> {
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
pub(crate) fn interrupted(game: &Path) -> Option<String> {
    match read_journal(game) {
        Ok(None) => None,
        Ok(Some(_)) => Some(format!(
            "An interrupted operation is recorded in {}; run recover to undo it",
            journal_path(game).display()
        )),
        Err(error) => Some(error.to_string()),
    }
}

pub(crate) fn write_journal(game: &Path, journal: &Journal) -> Result<()> {
    validate_transaction(game, journal)?;
    let encoded = serde_json::to_vec(journal).map_err(|e| invalid(format!("Encode JSON: {e}")))?;
    if encoded.len() as u64 > JOURNAL_LIMIT {
        return Err(invalid(
            "Install transaction is too large for recoverable metadata",
        ));
    }
    write_atomic(&journal_path(game), Some(&encoded), false)
}

pub(crate) fn set_journal_aside(game: &Path) -> Result<PathBuf> {
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

pub(crate) fn recover(game: &Path) -> Result<bool> {
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
pub(crate) fn drop_generation_files(game: &Path, files: &BTreeMap<PathBuf, String>) -> Result<()> {
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

pub(crate) fn prune_empty(directory: &Path) {
    for entry in fs::read_dir(directory).into_iter().flatten().flatten() {
        if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            prune_empty(&entry.path());
        }
    }
    let _ = fs::remove_dir(directory);
}

/// Splits recorded files into the unmodified ones to retire and the ones a user changed.
pub(crate) fn retire(
    files: &BTreeMap<PathBuf, String>,
) -> Result<(BTreeMap<PathBuf, String>, Vec<PathBuf>)> {
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
pub(crate) fn preserve(game: &Path, name: &str, content: &[u8]) -> Result<PathBuf> {
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

pub(crate) fn transact(
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

pub(crate) fn read_record(game: &Path) -> Result<Option<InstallRecord>> {
    let path = record_path(game);
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
