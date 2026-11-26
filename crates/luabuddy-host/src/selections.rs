use crate::{
    game,
    lua::{Context, Result},
};
use luabuddy_core::{ModId, Profile, profile::read_each_profile, selection};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

/// A selection file that can't be used, and why.
pub type Unreadable = (PathBuf, String);

type Claims = BTreeMap<PathBuf, Vec<Profile>>;
static CLAIMS: OnceLock<Mutex<Claims>> = OnceLock::new();

/// Where selections are kept, with links in the player's own path resolved.
pub fn directory(context: &mut Context<'_, '_>) -> Result<PathBuf> {
    let path = PathBuf::from(
        game::property(context, "luabuddy.config")?.ok_or("Missing LuaBuddy profile directory")?,
    );
    path.canonicalize()
        .map_err(|e| format!("Profile directory {}: {e}", path.display()))
}

/// Takes the queued one-launch selections once per process. A queue that can't
/// be taken costs only those selections, never the persistent ones.
pub fn initialize(context: &mut Context<'_, '_>) -> Result<()> {
    let directory = directory(context)?;
    let mut claims = CLAIMS
        .get_or_init(|| Mutex::new(BTreeMap::new()))
        .lock()
        .map_err(|_| "One-launch selection lock is poisoned")?;
    if claims.contains_key(&directory) {
        return Ok(());
    }
    let (claimed, result) = match selection::claim(&directory) {
        Ok(profiles) => (profiles, Ok(())),
        Err(error) => (Vec::new(), Err(error.to_string())),
    };
    claims.insert(directory, claimed);
    result
}

fn claimed(context: &mut Context<'_, '_>) -> Result<Vec<Profile>> {
    let directory = directory(context)?;
    Ok(CLAIMS
        .get_or_init(|| Mutex::new(BTreeMap::new()))
        .lock()
        .map_err(|_| "One-launch selection lock is poisoned")?
        .get(&directory)
        .cloned()
        .unwrap_or_default())
}

/// Every readable selection, and the files that could not be read.
pub fn readable(context: &mut Context<'_, '_>) -> Result<(Vec<Profile>, Vec<Unreadable>)> {
    let once = claimed(context)?;
    let (persistent, errors) =
        read_each_profile(&directory(context)?).map_err(|e| e.to_string())?;
    let errors = errors
        .into_iter()
        .map(|error| (error.path.clone().unwrap_or_default(), error.message))
        .collect();
    let profiles = selection::merge(persistent, &once).map_err(|e| e.to_string())?;
    Ok((profiles, errors))
}

pub fn configured(context: &mut Context<'_, '_>) -> Result<Vec<Profile>> {
    readable(context).map(|(profiles, _)| profiles)
}

/// The mods selected for this game process only.
pub fn temporary_ids(context: &mut Context<'_, '_>) -> Result<std::collections::BTreeSet<ModId>> {
    Ok(claimed(context)?
        .into_iter()
        .map(|profile| profile.id)
        .collect())
}

pub fn temporary(context: &mut Context<'_, '_>, id: &ModId) -> Result<bool> {
    Ok(claimed(context)?.iter().any(|profile| profile.id == *id))
}

pub fn queued(context: &mut Context<'_, '_>) -> Result<Vec<Profile>> {
    selection::pending(&directory(context)?).map_err(|error| error.to_string())
}
