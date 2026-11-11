use crate::{
    game,
    lua::{Context, Result},
};
use luabuddy_core::{ModId, Profile, read_profiles, selection};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

type Claims = BTreeMap<PathBuf, Result<Vec<Profile>>>;
static CLAIMS: OnceLock<Mutex<Claims>> = OnceLock::new();

fn directory(context: &mut Context<'_, '_>) -> Result<PathBuf> {
    let path = PathBuf::from(
        game::property(context, "luabuddy.config")?.ok_or("Missing LuaBuddy profile directory")?,
    );
    path.canonicalize()
        .map_err(|e| format!("Profile directory {}: {e}", path.display()))
}

pub fn initialize(context: &mut Context<'_, '_>) -> Result<()> {
    let directory = directory(context)?;
    let mut claims = CLAIMS
        .get_or_init(|| Mutex::new(BTreeMap::new()))
        .lock()
        .map_err(|_| "One-launch selection lock is poisoned")?;
    let once = claims
        .entry(directory.clone())
        .or_insert_with(|| selection::claim(&directory).map_err(|error| error.to_string()));
    once.as_ref().map(|_| ()).map_err(Clone::clone)
}

fn claimed(context: &mut Context<'_, '_>) -> Result<Vec<Profile>> {
    initialize(context)?;
    let directory = directory(context)?;
    CLAIMS
        .get_or_init(|| Mutex::new(BTreeMap::new()))
        .lock()
        .map_err(|_| "One-launch selection lock is poisoned")?
        .get(&directory)
        .cloned()
        .ok_or("One-launch selection snapshot is missing".to_owned())?
}

pub fn configured(context: &mut Context<'_, '_>) -> Result<Vec<Profile>> {
    let once = claimed(context)?;
    let persistent = read_profiles(&directory(context)?).map_err(|e| e.to_string())?;
    selection::merge(persistent, &once).map_err(|e| e.to_string())
}

pub fn temporary(context: &mut Context<'_, '_>, id: &ModId) -> Result<bool> {
    Ok(claimed(context)?.iter().any(|profile| profile.id == *id))
}

pub fn queued(context: &mut Context<'_, '_>) -> Result<Vec<Profile>> {
    selection::pending(&directory(context)?).map_err(|error| error.to_string())
}
