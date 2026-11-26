use luabuddy_core::{
    ModId, Profile, diagnostics as report_contract, lifecycle::Operation, property as props,
    read_profiles, selection::case_variant,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::env;
use std::path::{Path, PathBuf};

mod discovery;
pub mod extensions;
mod fsio;
mod launcher;
mod package;
mod report;
mod transaction;
#[cfg(windows)]
mod windows;

pub use discovery::{Discovery, JvmIdentity, discover, find_game, game_directory, probe_jvm};
pub use fsio::{Error, sha256};
pub use package::{Package, PackageFile, PackageManifest, make_package, verify_package};
pub use report::{diagnostics, doctor};
pub use transaction::{InstallRecord, InstalledGeneration};

use fsio::*;
use launcher::*;
use transaction::*;

const SCHEMA: u32 = 1;
const METADATA_LIMIT: u64 = 4 * 1024 * 1024;
const RUNTIME_LIMIT: u64 = 256 * 1024 * 1024;
const JOURNAL_LIMIT: u64 = 512 * 1024 * 1024;
const INSTALL_DIR: &str = ".luabuddy";
const RECORD_FILE: &str = "native-install.json";
const LOCK_FILE: &str = "install.lock";
const JOURNAL_FILE: &str = "native-transaction.json";

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
    let queued = if directory.exists() {
        luabuddy_core::selection::pending(directory)?
    } else {
        Vec::new()
    };
    let mut selected = BTreeMap::new();
    let mut files = Vec::new();
    let mut outcomes = Vec::new();
    for id in ids {
        let id = ModId::new(id)?;
        let name = profile_filename(id.as_str());
        if let Some(other) = selected.insert(name.to_ascii_lowercase(), id.clone()) {
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
        if queued.iter().any(|profile| profile.id == id) {
            return Err(invalid(format!(
                "{id} is queued for one future game launch. Cancel that first: luabuddy deselect --profiles-dir {} --mod {} --once",
                shell_word(&directory.to_string_lossy()),
                shell_word(id.as_str())
            )));
        }
        if directory.exists() {
            luabuddy_core::selection::filename_available(directory, &target)?;
        }
        let template = templates.iter().find(|profile| profile.id == id);
        let template_text = template
            .map(|template| luabuddy_core::profile::read_text(&template.file))
            .transpose()?;
        let (text, _) =
            luabuddy_core::selection::new_profile(&target, &id, None, template_text.as_deref())?;
        let content = text.into_bytes();
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
        .find_map(|arg| arg.strip_prefix(&props::argument(props::CONFIG, "")))
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
    let config_argument = props::argument(
        props::CONFIG,
        profiles
            .to_str()
            .ok_or_else(|| invalid("Profile path must be UTF-8"))?,
    );
    let templates = generation_root.join("profiles");
    let templates_argument = props::argument(
        props::TEMPLATES,
        templates
            .to_str()
            .ok_or_else(|| invalid("Template path must be UTF-8"))?,
    );
    let mut args = vm_args(&config);
    for (name, choice) in [
        (props::DEBUG, options.debug),
        (props::EARLY_UI, options.early_ui),
    ] {
        match choice {
            Some(true) => args.push(props::argument(name, "true")),
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
    let record_file = record_path(game);
    changes.push(Change {
        before: optional_bytes(&record_file, METADATA_LIMIT)?,
        path: record_file,
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
    // A package without templates still has a template directory to read.
    plain_directory(&templates)?;
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
                || last(props::CONFIG) != Some(&generation.config_argument)
                || last(props::TEMPLATES) != Some(&generation.templates_argument)
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
        .find(|arg| property(arg, props::CONFIG))
        .ok_or_else(|| invalid("Previous generation has no profile directory argument"))?;
    let profiles = profile_argument
        .strip_prefix(&props::argument(props::CONFIG, ""))
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
    let record_file = record_path(game);
    let changes = vec![
        Change {
            before: optional_bytes(&record_file, METADATA_LIMIT)?,
            path: record_file,
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
                        && recorded_base(&record, &source, None)?.as_ref() == Some(&source) =>
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
    let record_file = record_path(game);
    changes.push(Change {
        before: optional_bytes(&record_file, METADATA_LIMIT)?,
        path: record_file,
        after: None,
        executable: false,
    });
    let changed = transact(game, &record.profiles, &[], changes, retired)?;
    Ok(
        json!({"operation":"uninstall", "changed":changed, "profilesPreserved":record.profiles, "modifiedFilesPreserved":retained, "invalidLauncherPreserved":broken_launcher, "invalidJournalPreserved":journal, "notes":Vec::from_iter(notes), "restartRequired":true}),
    )
}
