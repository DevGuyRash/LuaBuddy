//! Reads the surrounding game state so a journal entry says what was true at
//! the time. Every reading is optional: a missing one never fails an action.

use crate::{
    game,
    lua::{Context, Result},
};
use jni::{
    JNIEnv,
    objects::{JObject, JObjectArray, JString},
};
use serde_json::{Map, Value as Json, json};
use std::{collections::BTreeMap, time::Duration};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Place {
    MainMenu,
    SinglePlayer,
    Server,
}

impl Place {
    pub fn name(self) -> &'static str {
        match self {
            Self::MainMenu => "main-menu",
            Self::SinglePlayer => "single-player",
            Self::Server => "server",
        }
    }
}

pub fn place(context: &mut Context<'_, '_>) -> Result<Place> {
    if game::client(context)? {
        return Ok(Place::Server);
    }
    let ingame = context.call_global("isIngameState", &[])?;
    let ingame = match ingame.first() {
        Some(value) => context.truth(value)?,
        None => false,
    };
    Ok(if ingame {
        Place::SinglePlayer
    } else {
        Place::MainMenu
    })
}

/// Where the player is and what they and their open containers are doing.
pub fn world(context: &mut Context<'_, '_>) -> Json {
    let mut data = Map::new();
    if let Ok(place) = place(context) {
        data.insert("place".into(), json!(place.name()));
    }
    if let Ok(speed) = context.call_global("getGameSpeed", &[])
        && let Some(speed) = speed.first()
        && let Ok(speed) = context.numeric(speed)
    {
        data.insert("gameSpeed".into(), json!(speed));
    }
    let players = context
        .jni(|env| env.with_local_frame(256, |env| Ok(players(env))))
        .unwrap_or_default();
    let mut described = Vec::new();
    for (index, mut player) in players {
        if let Some(action) = current_action(context, index) {
            player.insert("action".into(), action);
        }
        let containers = ["getPlayerInventory", "getPlayerLoot"]
            .into_iter()
            .filter_map(|getter| open_container(context, getter, index))
            .collect::<Vec<_>>();
        if !containers.is_empty() {
            player.insert("openContainers".into(), Json::Array(containers));
        }
        described.push(Json::Object(player));
    }
    if !described.is_empty() {
        data.insert("players".into(), Json::Array(described));
    }
    Json::Object(data)
}

fn players(env: &mut JNIEnv<'_>) -> Vec<(i32, Map<String, Json>)> {
    let mut result = Vec::new();
    let Some(array) = clean(env, |env| {
        env.get_static_field(
            "zombie/characters/IsoPlayer",
            "players",
            "[Lzombie/characters/IsoPlayer;",
        )?
        .l()
    })
    .filter(|array| !array.is_null())
    .map(JObjectArray::from) else {
        return result;
    };
    let count = env.get_array_length(&array).unwrap_or(0);
    for index in 0..count {
        let Some(player) = clean(env, |env| env.get_object_array_element(&array, index))
            .filter(|player| !player.is_null())
        else {
            continue;
        };
        result.push((index, player_state(env, &player)));
    }
    result
}

fn player_state(env: &mut JNIEnv<'_>, player: &JObject<'_>) -> Map<String, Json> {
    let mut data = Map::new();
    let mut at = Vec::new();
    for axis in ["getX", "getY", "getZ"] {
        if let Some(value) = clean(env, |env| env.call_method(player, axis, "()F", &[])?.f()) {
            at.push(json!(value.floor()));
        }
    }
    if at.len() == 3 {
        data.insert("at".into(), Json::Array(at));
    }
    for (name, method) in [
        ("dead", "isDead"),
        ("asleep", "isAsleep"),
        ("sitting", "isSitOnGround"),
    ] {
        if clean(env, |env| env.call_method(player, method, "()Z", &[])?.z()) == Some(true) {
            data.insert(name.into(), json!(true));
        }
    }
    if let Some(health) = clean(env, |env| {
        env.call_method(player, "getHealth", "()F", &[])?.f()
    }) {
        data.insert("health".into(), json!((health * 100.0).round() / 100.0));
    }
    if let Some(vehicle) = object(env, player, "getVehicle", "()Lzombie/vehicles/BaseVehicle;")
        && let Some(name) = text(env, &vehicle, "getScriptName")
    {
        data.insert("vehicle".into(), json!(name));
    }
    for (name, method) in [
        ("primaryHand", "getPrimaryHandItem"),
        ("secondaryHand", "getSecondaryHandItem"),
    ] {
        if let Some(item) = object(env, player, method, "()Lzombie/inventory/InventoryItem;")
            && let Some(kind) = text(env, &item, "getFullType")
        {
            data.insert(name.into(), json!(kind));
        }
    }
    if let Some(inventory) = object(
        env,
        player,
        "getInventory",
        "()Lzombie/inventory/ItemContainer;",
    ) {
        data.insert("inventory".into(), Json::Object(container(env, &inventory)));
    }
    data
}

fn container(env: &mut JNIEnv<'_>, value: &JObject<'_>) -> Map<String, Json> {
    let mut data = Map::new();
    if let Some(kind) = text(env, value, "getType") {
        data.insert("type".into(), json!(kind));
    }
    if let Some(items) = object(env, value, "getItems", "()Ljava/util/ArrayList;")
        && let Some(count) = clean(env, |env| env.call_method(&items, "size", "()I", &[])?.i())
    {
        data.insert("items".into(), json!(count));
    }
    for (name, method) in [
        ("weight", "getContentsWeight"),
        ("capacity", "getCapacityWeight"),
    ] {
        if let Some(weight) = clean(env, |env| env.call_method(value, method, "()F", &[])?.f()) {
            data.insert(name.into(), json!((weight * 100.0).round() / 100.0));
        }
    }
    if let Some(parent) = object(env, value, "getParent", "()Lzombie/iso/IsoObject;") {
        if let Some(name) = text(env, &parent, "getObjectName") {
            data.insert("object".into(), json!(name));
        }
        if let Some(square) = object(env, &parent, "getSquare", "()Lzombie/iso/IsoGridSquare;") {
            let at = ["getX", "getY", "getZ"]
                .into_iter()
                .filter_map(|axis| {
                    clean(env, |env| env.call_method(&square, axis, "()I", &[])?.i())
                })
                .collect::<Vec<_>>();
            data.insert("objectAt".into(), json!(at));
        }
    }
    data
}

fn current_action(context: &mut Context<'_, '_>, index: i32) -> Option<Json> {
    let class = context.global("ISTimedActionQueue").ok()?;
    if class.is_nil() {
        return None;
    }
    let player_number = context.number(f64::from(index)).ok()?;
    let player = context
        .call_global("getSpecificPlayer", &[player_number])
        .ok()?
        .into_iter()
        .next()?;
    let getter = context.index(&class, "getTimedActionQueue").ok()?;
    let queue = context.call(&getter, &[player]).ok()?.into_iter().next()?;
    let queue = context.get(&queue, "queue").ok()?;
    if queue.is_nil() {
        return None;
    }
    let entries = context.entries(&queue).ok()?;
    let first = entries.first()?.1.clone();
    let kind = context.index(&first, "Type").ok()?;
    Some(json!({ "current": context.text(&kind).ok()?, "queued": entries.len() }))
}

fn open_container(context: &mut Context<'_, '_>, getter: &str, index: i32) -> Option<Json> {
    let number = context.number(f64::from(index)).ok()?;
    let page = context
        .call_global(getter, &[number])
        .ok()?
        .into_iter()
        .next()
        .filter(|page| !page.is_nil())?;
    let visible = context
        .invoke(&page, "getIsVisible", &[])
        .ok()?
        .into_iter()
        .next()?;
    if !context.truth(&visible).ok()? {
        return None;
    }
    let inventory = context
        .get(&page, "inventory")
        .ok()
        .filter(|value| !value.is_nil())?;
    let mut data = context
        .jni(|env| env.with_local_frame(32, |env| Ok(container(env, inventory.object()))))
        .ok()?;
    data.insert(
        "window".into(),
        json!(if getter == "getPlayerLoot" {
            "loot"
        } else {
            "inventory"
        }),
    );
    Some(Json::Object(data))
}

fn object<'local>(
    env: &mut JNIEnv<'local>,
    receiver: &JObject<'_>,
    method: &str,
    signature: &str,
) -> Option<JObject<'local>> {
    clean(env, |env| {
        env.call_method(receiver, method, signature, &[])?.l()
    })
    .filter(|value| !value.is_null())
}

fn text(env: &mut JNIEnv<'_>, receiver: &JObject<'_>, method: &str) -> Option<String> {
    let value = object(env, receiver, method, "()Ljava/lang/String;")?;
    clean(env, |env| {
        Ok(env
            .get_string(&JString::from(value))?
            .to_string_lossy()
            .into_owned())
    })
}

fn clean<'local, T>(
    env: &mut JNIEnv<'local>,
    action: impl FnOnce(&mut JNIEnv<'local>) -> jni::errors::Result<T>,
) -> Option<T> {
    let result = action(env).ok();
    if result.is_none() {
        let _ = env.exception_clear();
    }
    result
}

/// What one mod's code has added to the Lua environment and the event system.
#[derive(Clone, Debug, Default)]
pub struct Footprint {
    globals: usize,
    /// The most recently added global name, which marks where later additions begin.
    newest: Option<String>,
    callbacks: BTreeMap<String, usize>,
}

pub fn footprint(context: &mut Context<'_, '_>) -> Footprint {
    Footprint {
        globals: global_count(context).unwrap_or_default(),
        newest: newest_global(context).unwrap_or_default(),
        callbacks: callbacks(context).unwrap_or_default(),
    }
}

impl Footprint {
    /// Names and counts present now that were absent in `before`. The global
    /// table keeps insertion order, so new names are the ones after the name
    /// that was newest before, wherever deletions have moved it.
    pub fn since(&self, before: &Self, context: &mut Context<'_, '_>) -> Json {
        let globals = if self.newest != before.newest {
            globals_after(context, before.newest.as_deref(), before.globals).unwrap_or_default()
        } else {
            Vec::new()
        };
        let callbacks = self
            .callbacks
            .iter()
            .filter_map(|(event, count)| {
                let added = count.saturating_sub(before.callbacks.get(event).copied().unwrap_or(0));
                (added > 0).then(|| (event.clone(), added))
            })
            .collect::<BTreeMap<_, _>>();
        json!({ "globalsAdded": globals, "callbacksAdded": callbacks })
    }
}

fn global_count(context: &mut Context<'_, '_>) -> Result<usize> {
    let table = context.environment.clone();
    context.jni(|env| {
        let size = env.call_method(table.object(), "size", "()I", &[])?.i()?;
        Ok(size.max(0) as usize)
    })
}

fn newest_global(context: &mut Context<'_, '_>) -> Result<Option<String>> {
    let table = context.environment.clone();
    context.jni(|env| {
        env.with_local_frame(8, |env| -> jni::errors::Result<Option<String>> {
            let map = env
                .get_field(table.object(), "delegate", "Ljava/util/Map;")?
                .l()?;
            // The table's LinkedHashMap is a SequencedMap, whose last entry is O(1).
            let last = env
                .call_method(&map, "lastEntry", "()Ljava/util/Map$Entry;", &[])?
                .l()?;
            if last.is_null() {
                return Ok(None);
            }
            let key = env
                .call_method(&last, "getKey", "()Ljava/lang/Object;", &[])?
                .l()?;
            if !env.is_instance_of(&key, "java/lang/String")? {
                return Ok(None);
            }
            Ok(Some(
                env.get_string(&JString::from(key))?
                    .to_string_lossy()
                    .into_owned(),
            ))
        })
    })
}

/// Global names added after `marker`. Without a marker, or when it was deleted,
/// the old size is the best remaining estimate of where additions begin.
fn globals_after(
    context: &mut Context<'_, '_>,
    marker: Option<&str>,
    old_size: usize,
) -> Result<Vec<String>> {
    let table = context.environment.clone();
    context.jni(|env| {
        env.with_local_frame(16, |env| -> jni::errors::Result<Vec<String>> {
            let map = env
                .get_field(table.object(), "delegate", "Ljava/util/Map;")?
                .l()?;
            let keys = env
                .call_method(&map, "keySet", "()Ljava/util/Set;", &[])?
                .l()?;
            let keys = JObjectArray::from(
                env.call_method(&keys, "toArray", "()[Ljava/lang/Object;", &[])?
                    .l()?,
            );
            let count = env.get_array_length(&keys)? as usize;
            let name_at = |env: &mut jni::JNIEnv, index: usize| {
                env.with_local_frame(4, |env| -> jni::errors::Result<Option<String>> {
                    let key = env.get_object_array_element(&keys, index as i32)?;
                    if !env.is_instance_of(&key, "java/lang/String")? {
                        return Ok(None);
                    }
                    Ok(Some(
                        env.get_string(&JString::from(key))?
                            .to_string_lossy()
                            .into_owned(),
                    ))
                })
            };
            let mut from = old_size.min(count);
            if let Some(marker) = marker {
                for index in (0..count).rev() {
                    if name_at(env, index)?.as_deref() == Some(marker) {
                        from = index + 1;
                        break;
                    }
                }
            }
            let mut names = Vec::new();
            for index in from..count {
                names.extend(name_at(env, index)?);
            }
            Ok(names)
        })
    })
}

fn callbacks(context: &mut Context<'_, '_>) -> Result<BTreeMap<String, usize>> {
    context.jni(|env| {
        let list = env
            .get_static_field(
                "zombie/Lua/LuaEventManager",
                "EventList",
                "Ljava/util/ArrayList;",
            )?
            .l()?;
        let count = env.call_method(&list, "size", "()I", &[])?.i()?;
        let mut result = BTreeMap::new();
        for index in 0..count {
            env.with_local_frame(8, |env| -> jni::errors::Result<()> {
                let event = env
                    .call_method(&list, "get", "(I)Ljava/lang/Object;", &[index.into()])?
                    .l()?;
                let callbacks = env
                    .get_field(&event, "callbacks", "Ljava/util/ArrayList;")?
                    .l()?;
                let size = env.call_method(&callbacks, "size", "()I", &[])?.i()?;
                if size > 0 {
                    let name = env.get_field(&event, "name", "Ljava/lang/String;")?.l()?;
                    let name = env
                        .get_string(&JString::from(name))?
                        .to_string_lossy()
                        .into_owned();
                    result.insert(name, size as usize);
                }
                Ok(())
            })?;
        }
        Ok(result)
    })
}

/// Processor time used by this thread, to tell slow work from a starved machine.
pub fn thread_cpu(context: &mut Context<'_, '_>) -> Option<Duration> {
    let nanos = context
        .jni(|env| {
            env.with_local_frame(4, |env| -> jni::errors::Result<i64> {
                let threads = env
                    .call_static_method(
                        "java/lang/management/ManagementFactory",
                        "getThreadMXBean",
                        "()Ljava/lang/management/ThreadMXBean;",
                        &[],
                    )?
                    .l()?;
                env.call_method(&threads, "getCurrentThreadCpuTime", "()J", &[])?
                    .j()
            })
        })
        .ok()?;
    u64::try_from(nanos).ok().map(Duration::from_nanos)
}
