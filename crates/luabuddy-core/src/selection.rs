use crate::{CoreError, ModId, Profile, read_profiles};
use std::fs::{self, OpenOptions, TryLockError};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

const STORE: &str = ".launch-once";
static NEXT: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum Lifetime {
    Persistent,
    Once,
}

fn error(path: &Path, message: impl ToString) -> CoreError {
    CoreError::new("selection", message.to_string()).at(path)
}

/// Canonicalizes a directory after checking the `owned` trailing components
/// this module creates. The player may reach the rest of the path through links.
fn safe_directory(path: &Path, owned: usize, create: bool) -> Result<PathBuf, CoreError> {
    for ancestor in path.ancestors().take(owned) {
        if fs::symlink_metadata(ancestor).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
            return Err(error(
                ancestor,
                "Refusing a symlink in a selection destination",
            ));
        }
    }
    if create {
        fs::create_dir_all(path).map_err(|e| error(path, e))?;
    }
    path.canonicalize().map_err(|e| error(path, e))
}

fn read(path: &Path) -> Result<String, CoreError> {
    if fs::symlink_metadata(path)
        .map_err(|e| error(path, e))?
        .file_type()
        .is_symlink()
    {
        return Err(error(path, "Refusing to follow a symlinked selection file"));
    }
    crate::profile::read_text(path)
}

/// Escapes `value` for a `.properties` key or value, so a profile written
/// from it parses back to exactly `value`.
pub fn escape(value: &str) -> String {
    let mut result = String::new();
    for ch in value.chars() {
        match ch {
            '\n' => result.push_str("\\n"),
            '\r' => result.push_str("\\r"),
            '\t' => result.push_str("\\t"),
            '\u{c}' => result.push_str("\\f"),
            '\\' | ' ' | ':' | '=' | '#' | '!' => {
                result.push('\\');
                result.push(ch);
            }
            _ => result.push(ch),
        }
    }
    result
}

fn unique(directory: &Path, prefix: &str) -> PathBuf {
    directory.join(format!(
        ".{prefix}-{}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ))
}

/// Fails when another entry in `directory` has `destination`'s filename apart
/// from ASCII case, as on a case-insensitive disk. Generated profile filenames
/// are ASCII, so no wider comparison applies.
pub fn filename_available(directory: &Path, destination: &Path) -> Result<(), CoreError> {
    let filename = destination
        .file_name()
        .ok_or_else(|| error(destination, "Missing filename"))?;
    for entry in fs::read_dir(directory).map_err(|e| error(directory, e))? {
        let entry = entry.map_err(|e| error(directory, e))?;
        if entry.path() != destination
            && entry
                .file_name()
                .to_string_lossy()
                .eq_ignore_ascii_case(&filename.to_string_lossy())
        {
            return Err(error(
                destination,
                format!("Profile filename collides with {}", entry.path().display()),
            ));
        }
    }
    Ok(())
}

fn publish(path: &Path, text: &str, original: Option<&str>) -> Result<(), CoreError> {
    let directory = path
        .parent()
        .ok_or_else(|| error(path, "Missing profile directory"))?;
    filename_available(directory, path)?;
    let stage_directory = if directory.file_name().is_some_and(|name| name == "pending")
        && directory
            .parent()
            .and_then(Path::file_name)
            .is_some_and(|name| name == STORE)
    {
        directory
            .parent()
            .ok_or_else(|| error(directory, "Missing one-launch store"))?
    } else {
        directory
    };
    let stage = unique(stage_directory, "selection");
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&stage)
            .map_err(|e| error(&stage, e))?;
        if original.is_some() {
            file.set_permissions(
                fs::symlink_metadata(path)
                    .map_err(|e| error(path, e))?
                    .permissions(),
            )
            .map_err(|e| error(&stage, e))?;
        }
        file.write_all(text.as_bytes())
            .map_err(|e| error(&stage, e))?;
        file.sync_all().map_err(|e| error(&stage, e))?;
        drop(file);
        if let Some(original) = original {
            if read(path)? != original {
                return Err(error(
                    path,
                    "Profile changed while saving; retry after reviewing it",
                ));
            }
            fs::rename(&stage, path).map_err(|e| error(path, e))?;
        } else {
            fs::hard_link(&stage, path).map_err(|e| error(path, e))?;
        }
        Ok(())
    })();
    match fs::remove_file(&stage) {
        Ok(()) => result,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => result,
        Err(e) => result.and(Err(error(&stage, e))),
    }
}

fn pending_directory(directory: &Path) -> PathBuf {
    directory.join(STORE).join("pending")
}

/// The one-launch queue as a claim would see it: the registrations it can use,
/// and the `.properties` entries it leaves in place with the reason. Each entry
/// is in exactly one of the two lists.
pub fn queue(directory: &Path) -> Result<(Vec<Profile>, Vec<CoreError>), CoreError> {
    let (profiles, mut skipped) = read_queue(directory)?;
    // A persistent file that can't be read might shadow any queued entry, so a
    // claim keeps the whole queue until it can be read. Listing says the same,
    // rather than failing as a claim does.
    let (persistent, unreadable) = crate::profile::read_each_profile(directory)?;
    if let Some(blocker) = unreadable.first() {
        let blocker = blocker.path.as_deref().map_or_else(
            || blocker.message.clone(),
            |path| path.display().to_string(),
        );
        skipped.extend(profiles.iter().map(|profile| {
            error(
                &profile.file,
                format!(
                    "One-launch selection {} is held until {blocker} can be read",
                    profile.id
                ),
            )
        }));
        return Ok((Vec::new(), skipped));
    }
    let (usable, conflicts) = split_conflicts(&persistent, profiles);
    skipped.extend(conflicts);
    Ok((usable, skipped))
}

/// Registrations a claim can use. Entries it would leave behind are in [`skipped`].
pub fn pending(directory: &Path) -> Result<Vec<Profile>, CoreError> {
    Ok(queue(directory)?.0)
}

/// Queued `.properties` entries that a claim leaves in place, and why.
pub fn skipped(directory: &Path) -> Result<Vec<CoreError>, CoreError> {
    Ok(queue(directory)?.1)
}

fn read_queue(directory: &Path) -> Result<(Vec<Profile>, Vec<CoreError>), CoreError> {
    let directory = pending_directory(directory);
    match fs::symlink_metadata(&directory) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Default::default()),
        Err(e) => return Err(error(&directory, e)),
        Ok(_) => (),
    }
    let directory = safe_directory(&directory, 2, false)?;
    let mut files = fs::read_dir(&directory)
        .map_err(|e| error(&directory, e))?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| error(&directory, e))?;
    files.retain(|path| path.extension().is_some_and(|ext| ext == "properties"));
    files.sort();
    let mut profiles = Vec::new();
    let mut skipped = Vec::new();
    for path in files {
        match read_registration(&path) {
            Ok(_) if profiles.len() == 256 => {
                skipped.push(error(&path, "One launch accepts at most 256 registrations"))
            }
            Ok(profile) => profiles.push(profile),
            Err(error) => skipped.push(error),
        }
    }
    profiles.sort_by(|a, b| a.id.cmp(&b.id));
    Ok((profiles, skipped))
}

fn read_registration(path: &Path) -> Result<Profile, CoreError> {
    let profile = Profile::parse(path, &read(path)?)?;
    if path.file_name().and_then(|name| name.to_str())
        != Some(&crate::profile::profile_filename(profile.id.as_str()))
        || !profile.enabled
        || profile
            .path
            .as_ref()
            .is_some_and(|path| !path.is_absolute())
    {
        return Err(error(
            path,
            "Invalid one-launch registration; filenames must match IDs, profiles must be enabled, and explicit paths must be absolute",
        ));
    }
    Ok(profile)
}

/// Splits queued registrations from those a persistent profile shadows.
fn split_conflicts(persistent: &[Profile], queued: Vec<Profile>) -> (Vec<Profile>, Vec<CoreError>) {
    let (conflicts, usable) = queued
        .into_iter()
        .partition::<Vec<_>, _>(|profile| persistent.iter().any(|other| other.id == profile.id));
    (usable, conflicts.iter().map(conflict).collect())
}

fn conflict(profile: &Profile) -> CoreError {
    error(
        &profile.file,
        format!(
            "One-launch selection {} conflicts with a persistent profile; normal selections were preserved",
            profile.id
        ),
    )
}

pub fn merge(mut persistent: Vec<Profile>, once: &[Profile]) -> Result<Vec<Profile>, CoreError> {
    for profile in once {
        if persistent.iter().any(|existing| existing.id == profile.id) {
            return Err(conflict(profile));
        }
        persistent.push(profile.clone());
    }
    Ok(persistent)
}

/// A profile whose ID differs from `id` only by letter case, which is easy to mistype.
pub fn case_variant<'a>(profiles: &'a [Profile], id: &ModId) -> Option<&'a Profile> {
    let lower = id.as_str().to_lowercase();
    profiles
        .iter()
        .find(|profile| profile.id != *id && profile.id.as_str().to_lowercase() == lower)
}

/// Selects a mod, starting a new profile from `template` text when one exists.
pub fn select(
    directory: &Path,
    id: &ModId,
    source: Option<&Path>,
    lifetime: Lifetime,
    template: Option<&str>,
) -> Result<Profile, CoreError> {
    let directory = safe_directory(directory, 0, false)?;
    let persistent = read_profiles(&directory)?;
    let queued = read_queue(&directory)?.0;
    let source = source
        .map(|path| path.canonicalize().map_err(|e| error(path, e)))
        .transpose()?;
    if let Some(path) = &source
        && !path.is_dir()
    {
        return Err(error(path, "A mod path must be a directory"));
    }
    if let Some(variant) = case_variant(&persistent, id) {
        return Err(error(
            &variant.file,
            format!(
                "Mod IDs are case-sensitive; {id} is not selected, but {} exists",
                variant.id
            ),
        ));
    }
    let existing = persistent.iter().find(|profile| &profile.id == id);
    if lifetime == Lifetime::Once && existing.is_some()
        || lifetime == Lifetime::Persistent && queued.iter().any(|profile| &profile.id == id)
    {
        return Err(error(
            &directory,
            format!(
                "Mod {id} already has a selection with another lifetime; no normal profile was changed"
            ),
        ));
    }
    if lifetime == Lifetime::Once {
        let destination = safe_directory(&pending_directory(&directory), 2, true)?;
        let path = destination.join(crate::profile::profile_filename(id.as_str()));
        let (text, profile) = new_profile(&path, id, source.as_deref(), template)?;
        if let Some(existing) = queued.iter().find(|profile| &profile.id == id) {
            if existing == &profile {
                return Ok(profile);
            }
            return Err(error(
                &path,
                "A different one-launch registration already exists; cancel it first",
            ));
        }
        publish(&path, &text, None)?;
        return Ok(profile);
    }
    if let Some(existing) = existing {
        if let Some(source) = source {
            let same = existing
                .path
                .as_ref()
                .and_then(|path| path.canonicalize().ok())
                .as_ref()
                == Some(&source);
            if !same {
                return Err(error(
                    &existing.file,
                    "Explicit path differs from the existing profile; edit that profile deliberately instead",
                ));
            }
        }
        return set_enabled(existing, true);
    }
    let path = directory.join(crate::profile::profile_filename(id.as_str()));
    let (text, profile) = new_profile(&path, id, source.as_deref(), template)?;
    publish(&path, &text, None)?;
    Ok(profile)
}

/// The text and parsed form of a new enabled profile for `id`, from `template`
/// text when one exists. `path` is where it will be saved.
pub fn new_profile(
    path: &Path,
    id: &ModId,
    source: Option<&Path>,
    template: Option<&str>,
) -> Result<(String, Profile), CoreError> {
    // The blank line ends a template whose last line continues.
    let mut text = match template {
        Some(template) => format!("{template}\n\nenabled=true\n"),
        None => format!("id={}\n", escape(id.as_str())),
    };
    if let Some(source) = source {
        let value = source
            .to_str()
            .ok_or_else(|| error(source, "Mod path must be UTF-8"))?;
        text.push_str(&format!("path={}\n", escape(value)));
    }
    let profile = Profile::parse(path, &text)?;
    if profile.id != *id || !profile.enabled {
        return Err(error(
            path,
            "The template would change this selection unexpectedly",
        ));
    }
    Ok((text, profile))
}

fn set_enabled(profile: &Profile, enabled: bool) -> Result<Profile, CoreError> {
    let original = read(&profile.file)?;
    if Profile::parse(&profile.file, &original)? != *profile {
        return Err(error(
            &profile.file,
            "Profile changed before updating its selection",
        ));
    }
    let mut expected = profile.clone();
    expected.enabled = enabled;
    if expected == *profile {
        return Ok(expected);
    }
    let text = crate::profile::with_enabled(&original, enabled);
    if Profile::parse(&profile.file, &text)? != expected {
        return Err(error(
            &profile.file,
            "Profile settings would change unexpectedly; no selection changed",
        ));
    }
    publish(&profile.file, &text, Some(&original))?;
    Ok(expected)
}

/// Turns a persistent selection off, or cancels a queued one, including a
/// queued one a claim would skip.
pub fn deselect(directory: &Path, id: &ModId, lifetime: Lifetime) -> Result<bool, CoreError> {
    let directory = safe_directory(directory, 0, false)?;
    let profiles = match lifetime {
        Lifetime::Persistent => read_profiles(&directory)?,
        Lifetime::Once => read_queue(&directory)?.0,
    };
    let Some(profile) = profiles.iter().find(|profile| &profile.id == id) else {
        return Ok(false);
    };
    match lifetime {
        Lifetime::Persistent if profile.enabled => {
            set_enabled(profile, false)?;
            Ok(true)
        }
        Lifetime::Persistent => Ok(false),
        Lifetime::Once => {
            if Profile::parse(&profile.file, &read(&profile.file)?)? != *profile {
                return Err(error(
                    &profile.file,
                    "Queued selection changed before cancellation",
                ));
            }
            fs::remove_file(&profile.file).map_err(|e| error(&profile.file, e))?;
            Ok(true)
        }
    }
}

pub fn claim(directory: &Path) -> Result<Vec<Profile>, CoreError> {
    let directory = safe_directory(directory, 0, false)?;
    let pending = pending_directory(&directory);
    if !pending.try_exists().map_err(|e| error(&pending, e))? {
        return Ok(Vec::new());
    }
    let store = directory.join(STORE);
    safe_directory(&store, 1, false)?;
    let lock_path = store.join("claim.lock");
    match fs::symlink_metadata(&lock_path) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => (),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
        Err(e) => return Err(error(&lock_path, e)),
        Ok(_) => {
            return Err(error(
                &lock_path,
                "Expected an owned regular claim lock file",
            ));
        }
    }
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .map_err(|e| error(&lock_path, e))?;
    match lock.try_lock() {
        Ok(()) => (),
        Err(TryLockError::WouldBlock) => return Ok(Vec::new()),
        Err(TryLockError::Error(e)) => return Err(error(&lock_path, e)),
    }
    // Everything is validated before anything is consumed. Removing a file is
    // what claims it, so a registration cancelled meanwhile is not claimed.
    // An unreadable persistent profile might shadow a queued one, so the queue
    // is kept for the next launch rather than claimed around it.
    let persistent = read_profiles(&directory)?;
    let (profiles, _) = split_conflicts(&persistent, read_queue(&directory)?.0);
    let claimed = profiles
        .into_iter()
        .filter(|profile| fs::remove_file(&profile.file).is_ok())
        .collect();
    // Only an empty queue goes away; entries left behind keep theirs.
    let _ = fs::remove_dir(&pending);
    Ok(claimed)
}
