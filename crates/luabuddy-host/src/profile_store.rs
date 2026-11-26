//! Reads and changes the player's mod selections on disk.

use crate::{
    game,
    lua::{Context, Result},
};
use luabuddy_core::{
    ModId, Profile,
    profile::{profile_filename, read_each_profile, with_enabled},
};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

pub fn templates(context: &mut Context<'_, '_>) -> Result<Vec<Profile>> {
    let path = game::property(context, "luabuddy.templates")?.map(PathBuf::from);
    match path {
        Some(path) if path.is_dir() => {
            let (templates, errors) =
                read_each_profile(&path).map_err(|error| error.to_string())?;
            for error in errors {
                crate::log::warn("profile.template", error.to_string()).write();
            }
            Ok(templates)
        }
        _ => Ok(Vec::new()),
    }
}

/// The settings a mod would be loaded with if it were turned on now.
pub fn effective(id: &ModId, profiles: &[Profile], templates: &[Profile]) -> Result<Profile> {
    if let Some(profile) = profiles
        .iter()
        .chain(templates)
        .find(|profile| &profile.id == id)
    {
        return Ok(profile.clone());
    }
    Profile::parse(
        Path::new("LuaBuddy/unconfigured.properties"),
        &format!("id={}\n", escape(id.as_str())),
    )
    .map_err(|error| error.to_string())
}

pub struct Change {
    pub id: ModId,
    pub file: PathBuf,
    pub created: bool,
}

/// Turns the mods on or off together: either every file is saved or none is.
pub fn select(context: &mut Context<'_, '_>, ids: &[ModId], enabled: bool) -> Result<Vec<Change>> {
    let existing = crate::selections::configured(context)?;
    // Only mods whose selection would change are written, or refused.
    let ids = ids
        .iter()
        .filter(|id| {
            existing
                .iter()
                .find(|profile| &profile.id == *id)
                .is_none_or(|profile| profile.enabled != enabled)
        })
        .collect::<Vec<_>>();
    let queued = crate::selections::queued(context)?;
    for id in ids.iter().copied() {
        if crate::selections::temporary(context, id)? {
            return Err(format!(
                "{id} was selected for this game only, so it turns itself off when you quit. Nothing was changed."
            ));
        }
        if queued.iter().any(|profile| &profile.id == id) {
            return Err(format!(
                "{id} is queued for one future game launch. Cancel that with: luabuddy deselect --profiles-dir DIR --mod {id} --once. Nothing was changed."
            ));
        }
    }
    let directory = crate::selections::directory(context)?;
    let templates = templates(context)?;
    let mut changes = Vec::new();
    let mut writes: Vec<Edit> = Vec::new();
    for id in ids.iter().copied() {
        let profile = existing.iter().find(|profile| &profile.id == id);
        let path = match profile {
            Some(profile) => profile.file.clone(),
            None => directory.join(profile_filename(id.as_str())),
        };
        refuse_collision(&directory, &path)?;
        // Two new files in one batch can collide on a case-insensitive disk.
        if let Some((other, ..)) = writes.iter().find(|(other, ..)| {
            other
                .to_string_lossy()
                .eq_ignore_ascii_case(&path.to_string_lossy())
        }) {
            return Err(format!(
                "Profile filename {} collides with {}; no selection changed",
                path.display(),
                other.display()
            ));
        }
        let original = match fs::read(&path) {
            Ok(value) => Some(value),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(format!("Read {}: {error}", path.display())),
        };
        if profile.is_none() && original.is_some() {
            return Err(format!(
                "Profile filename collision at {}; no selection changed",
                path.display()
            ));
        }
        let content = if let Some(bytes) = &original {
            String::from_utf8(bytes.clone())
                .map_err(|_| format!("{} is not UTF-8 text", path.display()))?
        } else if let Some(template) = templates.iter().find(|profile| &profile.id == id) {
            fs::read_to_string(&template.file)
                .map_err(|error| format!("Read {}: {error}", template.file.display()))?
        } else {
            format!("id={}\n", escape(id.as_str()))
        };
        let before = Profile::parse(&path, &content).map_err(|error| error.to_string())?;
        let edited = with_enabled(&content, enabled);
        let candidate = Profile::parse(&path, &edited).map_err(|error| error.to_string())?;
        let mut expected = before;
        expected.enabled = enabled;
        if candidate.id != *id || candidate != expected {
            return Err(format!(
                "Profile settings changed unexpectedly while editing {}",
                path.display()
            ));
        }
        changes.push(Change {
            id: id.clone(),
            file: path.clone(),
            created: original.is_none(),
        });
        writes.push((path, original, edited.into_bytes()));
    }
    write(writes)?;
    let (saved, _) = read_each_profile(&directory).map_err(|error| error.to_string())?;
    for change in &changes {
        if saved
            .iter()
            .find(|profile| profile.id == change.id)
            .is_none_or(|profile| profile.enabled != enabled)
        {
            return Err(format!(
                "The selection for {} changed before it could be verified. Review {}.",
                change.id,
                change.file.display()
            ));
        }
    }
    Ok(changes)
}

fn refuse_collision(directory: &Path, path: &Path) -> Result<()> {
    let filename = path
        .file_name()
        .ok_or("Profile destination has no filename")?
        .to_string_lossy();
    for entry in
        fs::read_dir(directory).map_err(|error| format!("Inspect profile directory: {error}"))?
    {
        let entry = entry.map_err(|error| format!("Inspect profile filename: {error}"))?;
        if entry.path() != path
            && entry
                .file_name()
                .to_string_lossy()
                .eq_ignore_ascii_case(&filename)
        {
            return Err(format!(
                "Profile filename {} collides with {}; no selection changed",
                path.display(),
                entry.path().display()
            ));
        }
    }
    if path.parent() != Some(directory)
        || fs::symlink_metadata(path)
            .is_ok_and(|metadata| !metadata.is_file() || metadata.file_type().is_symlink())
    {
        return Err(format!(
            "Refusing unsafe profile destination {}",
            path.display()
        ));
    }
    Ok(())
}

fn escape(value: &str) -> String {
    let mut escaped = String::new();
    for character in value.chars() {
        if matches!(character, '\\' | ' ' | ':' | '=' | '#' | '!') {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped
}

type Edit = (PathBuf, Option<Vec<u8>>, Vec<u8>);

fn write(changes: Vec<Edit>) -> Result<()> {
    let mut staged = Vec::new();
    let result = (|| {
        for (index, (path, original, content)) in changes.iter().enumerate() {
            let temporary =
                path.with_extension(format!("luabuddy-{}-{index}.tmp", std::process::id()));
            let mut file = OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&temporary)
                .map_err(|error| format!("Stage {}: {error}", path.display()))?;
            staged.push((path.clone(), temporary.clone(), original.clone()));
            if original.is_some() {
                let permissions = fs::metadata(path)
                    .map_err(|error| format!("Inspect {}: {error}", path.display()))?
                    .permissions();
                file.set_permissions(permissions)
                    .map_err(|error| format!("Preserve permissions {}: {error}", path.display()))?;
            }
            file.write_all(content)
                .and_then(|()| file.sync_all())
                .map_err(|error| format!("Stage {}: {error}", path.display()))?;
        }
        for (index, (path, temporary, _)) in staged.iter().enumerate() {
            let observed = match fs::read(path) {
                Ok(value) => Ok(Some(value)),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(error) => Err(format!("Check {}: {error}", path.display())),
            };
            let result = observed.and_then(|observed| {
                if observed == changes[index].1 {
                    fs::rename(temporary, path).map_err(|error| error.to_string())
                } else {
                    Err("it changed while the selection was being saved".into())
                }
            });
            if let Err(error) = result {
                return Err(format!(
                    "Save {}: {error}. {}",
                    path.display(),
                    roll_back(&changes, &staged[..index])
                ));
            }
        }
        Ok(())
    })();
    for (_, temporary, _) in staged {
        let _ = fs::remove_file(temporary);
    }
    result
}

fn roll_back(changes: &[Edit], saved: &[(PathBuf, PathBuf, Option<Vec<u8>>)]) -> String {
    let mut failures = Vec::new();
    for (index, (path, _, original)) in saved.iter().enumerate().rev() {
        if fs::read(path).ok().as_ref() != Some(&changes[index].2) {
            failures.push(format!(
                "{} was modified after saving and was kept",
                path.display()
            ));
            continue;
        }
        let restored = match original {
            Some(bytes) => fs::write(path, bytes),
            None => fs::remove_file(path),
        };
        if let Err(error) = restored {
            failures.push(format!("{}: {error}", path.display()));
        }
    }
    if failures.is_empty() {
        "Nothing was changed.".into()
    } else {
        format!("Not undone: {}.", failures.join("; "))
    }
}
