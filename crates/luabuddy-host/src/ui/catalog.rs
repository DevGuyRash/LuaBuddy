//! Reads installed mods through the game's own metadata parser, a few at a
//! time, so opening the window never waits for every mod on disk.

use crate::{
    activation::{self, Eligibility},
    game,
    lua::{Context, Result, Value},
    profile_store,
    steam::ItemId,
};
use jni::objects::{JObjectArray, JValue};
use luabuddy_core::{ModId, Profile};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    path::PathBuf,
    rc::Rc,
    time::{Duration, Instant},
};

pub struct Mod {
    pub id: ModId,
    pub title: String,
    pub description: String,
    pub author: Option<String>,
    pub version: Option<String>,
    pub minimum: Option<String>,
    pub maximum: Option<String>,
    pub requires: Vec<ModId>,
    pub workshop: Option<u64>,
    pub directory: String,
    pub roots: Vec<PathBuf>,
    pub compatible: bool,
    pub native_startup: bool,
    pub poster: Option<String>,
}

/// A folder that should hold a mod but that the game could not read as one.
#[derive(Clone)]
pub struct Broken {
    pub directory: String,
    pub reason: String,
}

#[derive(Default)]
pub struct Catalog {
    pub mods: BTreeMap<ModId, Rc<Mod>>,
    pub broken: Vec<Broken>,
    queue: VecDeque<(String, bool)>,
    total: usize,
    eligibility: BTreeMap<ModId, Eligibility>,
}

impl Catalog {
    /// Lists the folders to read. `fresh` also drops the game's cached discovery.
    pub fn begin(context: &mut Context<'_, '_>, fresh: bool) -> Result<Self> {
        if fresh {
            game::reset_discovery(context)?;
        }
        let filesystem = game::instance(context)?;
        let folders = context.jni(|env| {
            let list = env.new_object("java/util/ArrayList", "()V", &[])?;
            env.call_method(
                filesystem.object(),
                "getAllModFolders",
                "(Ljava/util/List;)V",
                &[JValue::Object(&list)],
            )?;
            env.new_global_ref(list)
                .map(|reference| Value(Some(reference)))
        })?;
        let mut queue = VecDeque::new();
        let mut seen = BTreeSet::new();
        for folder in game::java_list(context, &folders)? {
            let folder = context.text(&folder)?;
            if seen.insert(folder.clone()) {
                queue.push_back((folder, true));
            }
        }
        for folder in workshop_folders(context)? {
            let flat = PathBuf::from(&folder).join("mod.info").is_file();
            if flat && seen.insert(folder.clone()) {
                queue.push_back((folder, false));
            }
        }
        Ok(Self {
            total: queue.len(),
            queue,
            ..Self::default()
        })
    }

    pub fn progress(&self) -> Option<(usize, usize)> {
        (!self.queue.is_empty()).then(|| (self.total - self.queue.len(), self.total))
    }

    /// Reads folders until the budget is spent. Returns whether anything changed.
    pub fn step(&mut self, context: &mut Context<'_, '_>, budget: Duration) -> bool {
        let started = Instant::now();
        let mut changed = false;
        while let Some((directory, native)) = self.queue.pop_front() {
            changed = true;
            // Each folder's references are released before the next one is read.
            let read = context
                .jni(|env| env.push_local_frame(64))
                .and_then(|()| {
                    let found = read(context, &directory, native);
                    context
                        .jni(|env| {
                            // SAFETY: The frame was pushed just above and nothing read
                            // in it escapes as a local reference; values are global.
                            unsafe { env.pop_local_frame(&jni::objects::JObject::null()) }.map(drop)
                        })
                        .map(|()| found)
                })
                .and_then(|found| found);
            match read {
                Ok(Some(found)) => {
                    self.mods
                        .entry(found.id.clone())
                        .or_insert_with(|| Rc::new(found));
                }
                Ok(None) => (),
                Err(reason) => {
                    crate::log::warn("catalog.unreadable", reason.clone())
                        .with(serde_json::json!({ "folder": directory }))
                        .write();
                    self.broken.push(Broken { directory, reason });
                }
            }
            if started.elapsed() >= budget {
                break;
            }
        }
        changed
    }

    pub fn finish(&mut self, context: &mut Context<'_, '_>) {
        while !self.queue.is_empty() {
            self.step(context, Duration::from_secs(1));
        }
    }

    /// What late loading can do with this mod. Walks its files, so ask only for
    /// the mod being looked at.
    pub fn eligibility(
        &mut self,
        id: &ModId,
        profiles: &[Profile],
        templates: &[Profile],
    ) -> Option<Eligibility> {
        if let Some(known) = self.eligibility.get(id) {
            return Some(known.clone());
        }
        let found = self.mods.get(id)?;
        let assessed = profile_store::effective(id, profiles, templates)
            .and_then(|profile| activation::assess(&profile, &found.roots, templates));
        match assessed {
            Ok(eligibility) => {
                self.eligibility.insert(id.clone(), eligibility.clone());
                Some(eligibility)
            }
            Err(error) => {
                crate::log::warn("catalog.eligibility", error)
                    .about(id)
                    .write();
                None
            }
        }
    }

    pub fn title(&self, id: &ModId) -> String {
        self.mods
            .get(id)
            .map_or_else(|| id.to_string(), |found| found.title.clone())
    }

    pub fn contained(&self, workshop: u64) -> Vec<(ModId, String)> {
        self.mods
            .values()
            .filter(|found| found.workshop == Some(workshop))
            .map(|found| (found.id.clone(), found.title.clone()))
            .collect()
    }

    /// The mod and everything it needs that is neither active nor included yet,
    /// requirements first.
    pub fn with_requirements(
        &self,
        id: &ModId,
        profiles: &[Profile],
        active: &BTreeSet<ModId>,
    ) -> Result<Vec<ModId>> {
        let mut order = Vec::new();
        self.visit(id, profiles, active, &mut Vec::new(), &mut order)?;
        Ok(order)
    }

    fn visit(
        &self,
        id: &ModId,
        profiles: &[Profile],
        active: &BTreeSet<ModId>,
        path: &mut Vec<ModId>,
        order: &mut Vec<ModId>,
    ) -> Result<()> {
        if active.contains(id) || order.contains(id) {
            return Ok(());
        }
        if path.contains(id) {
            return Err(format!("Its requirements loop back to {id}."));
        }
        let found = self.mods.get(id).ok_or_else(|| match path.last() {
            Some(parent) => format!("{} needs {id}, which is not installed.", self.title(parent)),
            None => format!("{id} is not installed."),
        })?;
        if !found.compatible {
            return Err(format!(
                "{} is not made for this game version.",
                found.title
            ));
        }
        path.push(id.clone());
        for needed in self.direct_requirements(id, profiles) {
            self.visit(&needed, profiles, active, path, order)?;
        }
        path.pop();
        order.push(id.clone());
        Ok(())
    }

    pub fn direct_requirements(&self, id: &ModId, profiles: &[Profile]) -> Vec<ModId> {
        let mut result: Vec<ModId> = Vec::new();
        let native = self
            .mods
            .get(id)
            .map(|found| found.requires.as_slice())
            .unwrap_or_default();
        let configured = profiles
            .iter()
            .find(|profile| &profile.id == id)
            .map(|profile| profile.requires.as_slice())
            .unwrap_or_default();
        for needed in native.iter().chain(configured) {
            if !result.contains(needed) {
                result.push(needed.clone());
            }
        }
        result
    }

    /// Enabled mods that need `id`, directly or through one another.
    pub fn enabled_dependents(&self, id: &ModId, profiles: &[Profile]) -> Vec<ModId> {
        let mut result: Vec<ModId> = Vec::new();
        let mut pending = vec![id.clone()];
        while let Some(target) = pending.pop() {
            for profile in profiles.iter().filter(|profile| profile.enabled) {
                if profile.id != *id
                    && !result.contains(&profile.id)
                    && self
                        .direct_requirements(&profile.id, profiles)
                        .contains(&target)
                {
                    result.push(profile.id.clone());
                    pending.push(profile.id.clone());
                }
            }
        }
        result.sort();
        result
    }
}

fn workshop_folders(context: &mut Context<'_, '_>) -> Result<Vec<String>> {
    let steam = context.jni(|env| {
        env.call_static_method(
            "zombie/core/znet/SteamUtils",
            "isSteamModeEnabled",
            "()Z",
            &[],
        )?
        .z()
    })?;
    if !steam {
        return Ok(Vec::new());
    }
    context.jni(|env| {
        let workshop = env
            .get_static_field(
                "zombie/core/znet/SteamWorkshop",
                "instance",
                "Lzombie/core/znet/SteamWorkshop;",
            )?
            .l()?;
        let array = env
            .call_method(
                workshop,
                "GetInstalledItemFolders",
                "()[Ljava/lang/String;",
                &[],
            )?
            .l()?;
        if array.is_null() {
            return Ok(Vec::new());
        }
        let array = JObjectArray::from(array);
        let mut folders = Vec::new();
        for index in 0..env.get_array_length(&array)? {
            let folder = env.get_object_array_element(&array, index)?;
            if !folder.is_null() {
                let folder = env.auto_local(jni::objects::JString::from(folder));
                folders.push(env.get_string(&folder)?.to_string_lossy().into_owned());
            }
        }
        Ok(folders)
    })
}

fn read(context: &mut Context<'_, '_>, directory: &str, native: bool) -> Result<Option<Mod>> {
    if !native {
        game::repair_flat_root(context, &PathBuf::from(directory))?;
    }
    let info = context.jni(|env| {
        let path = env.new_string(directory)?;
        let object = env
            .call_static_method(
                "zombie/gameStates/ChooseGameInfo",
                "readModInfo",
                "(Ljava/lang/String;)Lzombie/gameStates/ChooseGameInfo$Mod;",
                &[JValue::Object(&path)],
            )?
            .l()?;
        if object.is_null() {
            Ok(Value::default())
        } else {
            env.new_global_ref(object)
                .map(|reference| Value(Some(reference)))
        }
    })?;
    if info.is_nil() {
        return if native {
            Ok(None)
        } else {
            Err("The game could not read its mod.info.".into())
        };
    }
    let id = ModId::new(string(context, &info, "getId")?).map_err(|error| error.message)?;
    let required = context.jni(|env| {
        let object = env
            .call_method(info.object(), "getRequire", "()Ljava/util/ArrayList;", &[])?
            .l()?;
        if object.is_null() {
            Ok(Value::default())
        } else {
            env.new_global_ref(object)
                .map(|reference| Value(Some(reference)))
        }
    })?;
    let mut requires = Vec::new();
    for value in game::java_list(context, &required)? {
        let needed = context.text(&value)?;
        if !needed.trim().is_empty() {
            requires.push(ModId::new(needed).map_err(|error| error.message)?);
        }
    }
    let mut roots = Vec::new();
    for method in ["getCommonDir", "getVersionDir"] {
        let root = PathBuf::from(string(context, &info, method)?);
        if !root.as_os_str().is_empty() && !roots.contains(&root) {
            roots.push(root);
        }
    }
    let (compatible, poster) = context.jni(|env| {
        let compatible = env
            .call_method(info.object(), "isAvailableSelf", "()Z", &[])?
            .z()?;
        let posters = env
            .call_method(info.object(), "getPosterCount", "()I", &[])?
            .i()?;
        if posters == 0 {
            return Ok((compatible, None));
        }
        let poster = env
            .call_method(
                info.object(),
                "getPoster",
                "(I)Ljava/lang/String;",
                &[JValue::Int(0)],
            )?
            .l()?;
        if poster.is_null() {
            return Ok((compatible, None));
        }
        let poster = env
            .get_string(&poster.into())?
            .to_string_lossy()
            .into_owned();
        Ok((compatible, Some(poster)))
    })?;
    let filled = |text: String| (!text.trim().is_empty()).then(|| text.trim().to_owned());
    let title = filled(string(context, &info, "getName")?).unwrap_or_else(|| id.to_string());
    let workshop = string(context, &info, "getWorkshopID")?
        .parse()
        .ok()
        .and_then(|id| ItemId::new(id).ok())
        .map(ItemId::get)
        .or_else(|| workshop_in_path(directory));
    Ok(Some(Mod {
        title,
        description: string(context, &info, "getDescription")?,
        author: filled(string(context, &info, "getAuthor")?),
        version: filled(string(context, &info, "getModVersion")?),
        minimum: game_version(context, &info, "getVersionMin")?,
        maximum: game_version(context, &info, "getVersionMax")?,
        requires,
        workshop,
        directory: directory.to_owned(),
        roots,
        compatible,
        native_startup: native,
        poster,
        id,
    }))
}

fn workshop_in_path(directory: &str) -> Option<u64> {
    let normalized = directory.replace('\\', "/");
    let (_, tail) = normalized.split_once("/workshop/content/108600/")?;
    let id = tail.split('/').next()?.parse().ok()?;
    ItemId::new(id).ok().map(ItemId::get)
}

fn string(context: &mut Context<'_, '_>, object: &Value, method: &str) -> Result<String> {
    context.jni(|env| {
        let value = env
            .call_method(object.object(), method, "()Ljava/lang/String;", &[])?
            .l()?;
        if value.is_null() {
            return Ok(String::new());
        }
        Ok(env
            .get_string(&value.into())?
            .to_string_lossy()
            .into_owned())
    })
}

fn game_version(
    context: &mut Context<'_, '_>,
    object: &Value,
    method: &str,
) -> Result<Option<String>> {
    let value = context.jni(|env| {
        let value = env
            .call_method(object.object(), method, "()Lzombie/core/GameVersion;", &[])?
            .l()?;
        if value.is_null() {
            Ok(Value::default())
        } else {
            env.new_global_ref(value)
                .map(|reference| Value(Some(reference)))
        }
    })?;
    if value.is_nil() {
        Ok(None)
    } else {
        context.text(&value).map(Some)
    }
}
