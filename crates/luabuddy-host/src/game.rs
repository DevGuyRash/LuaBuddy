use crate::lua::{Context, Result, Value};
use jni::objects::JValue;
use luabuddy_core::{ModId, NativeMetadata, Profile};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

pub fn client(context: &mut Context<'_, '_>) -> Result<bool> {
    context.jni(|env| {
        Ok(env
            .get_static_field("zombie/network/GameClient", "client", "Z")?
            .z()?
            && !env
                .get_static_field("zombie/network/GameServer", "server", "Z")?
                .z()?)
    })
}

pub fn property(context: &mut Context<'_, '_>, key: &str) -> Result<Option<String>> {
    let value = context.jni(|env| {
        let key = env.new_string(key)?;
        let value = env
            .call_static_method(
                "java/lang/System",
                "getProperty",
                "(Ljava/lang/String;)Ljava/lang/String;",
                &[JValue::Object(&key)],
            )?
            .l()?;
        if value.is_null() {
            Ok(Value::default())
        } else {
            env.new_global_ref(value).map(|v| Value(Some(v)))
        }
    })?;
    if value.is_nil() {
        Ok(None)
    } else {
        context.text(&value).map(Some)
    }
}

pub fn instance(context: &mut Context<'_, '_>) -> Result<Value> {
    context.jni(|env| {
        let value = env
            .get_static_field(
                "zombie/ZomboidFileSystem",
                "instance",
                "Lzombie/ZomboidFileSystem;",
            )?
            .l()?;
        env.new_global_ref(value).map(|v| Value(Some(v)))
    })
}

pub fn java_list(context: &mut Context<'_, '_>, list: &Value) -> Result<Vec<Value>> {
    if list.is_nil() {
        return Ok(Vec::new());
    }
    context.jni(|env| {
        let size = env.call_method(list.object(), "size", "()I", &[])?.i()?;
        let mut values = Vec::new();
        for i in 0..size {
            let value = env
                .call_method(
                    list.object(),
                    "get",
                    "(I)Ljava/lang/Object;",
                    &[JValue::Int(i)],
                )?
                .l()?;
            values.push(if value.is_null() {
                Value::default()
            } else {
                Value(Some(env.new_global_ref(value)?))
            });
        }
        Ok(values)
    })
}

pub fn active(context: &mut Context<'_, '_>) -> Result<BTreeSet<ModId>> {
    let filesystem = instance(context)?;
    let list = context.jni(|env| {
        let list = env
            .call_method(filesystem.object(), "getModIDs", "()Ljava/util/List;", &[])?
            .l()?;
        env.new_global_ref(list).map(|v| Value(Some(v)))
    })?;
    let mut ids = BTreeSet::new();
    for value in java_list(context, &list)? {
        ids.insert(ModId::new(context.text(&value)?).map_err(|e| e.to_string())?);
    }
    Ok(ids)
}

pub fn reset_discovery(context: &mut Context<'_, '_>) -> Result<()> {
    let filesystem = instance(context)?;
    context.jni(|env| {
        env.call_method(filesystem.object(), "resetModFolders", "()V", &[])?;
        env.call_static_method("zombie/gameStates/ChooseGameInfo", "Reset", "()V", &[])?;
        Ok(())
    })
}

fn object_method(
    context: &mut Context<'_, '_>,
    object: &Value,
    method: &str,
    signature: &str,
) -> Result<Value> {
    context.jni(|env| {
        let value = env
            .call_method(object.object(), method, signature, &[])?
            .l()?;
        if value.is_null() {
            Ok(Value::default())
        } else {
            env.new_global_ref(value).map(|v| Value(Some(v)))
        }
    })
}

fn strings(context: &mut Context<'_, '_>, object: &Value, method: &str) -> Result<Vec<ModId>> {
    let list = object_method(context, object, method, "()Ljava/util/ArrayList;")?;
    let mut ids = Vec::new();
    for value in java_list(context, &list)? {
        ids.push(ModId::new(context.text(&value)?).map_err(|e| e.to_string())?);
    }
    Ok(ids)
}

pub struct Discovered {
    pub metadata: BTreeMap<ModId, NativeMetadata>,
    pub infos: BTreeMap<ModId, Value>,
    pub failures: BTreeMap<ModId, String>,
}

pub fn discover(
    context: &mut Context<'_, '_>,
    profiles: &[Profile],
    native: &BTreeSet<ModId>,
) -> Result<Discovered> {
    let mut result = Discovered {
        metadata: BTreeMap::new(),
        infos: BTreeMap::new(),
        failures: BTreeMap::new(),
    };
    for profile in profiles {
        if !profile.enabled && !native.contains(&profile.id) {
            continue;
        }
        let resolved = if profile.enabled {
            discover_one(context, profile)
        } else {
            native_info(context, &profile.id).and_then(|info| metadata(context, &profile.id, info))
        };
        match resolved {
            Ok((metadata, info)) => {
                result.metadata.insert(profile.id.clone(), metadata);
                result.infos.insert(profile.id.clone(), info);
            }
            Err(error) => {
                result.failures.insert(profile.id.clone(), error);
            }
        }
    }
    for id in native
        .iter()
        .filter(|id| !result.metadata.contains_key(*id))
        .cloned()
        .collect::<Vec<_>>()
    {
        match native_info(context, &id).and_then(|info| metadata(context, &id, info)) {
            Ok((metadata, info)) => {
                result.metadata.insert(id.clone(), metadata);
                result.infos.insert(id, info);
            }
            Err(error) => {
                result.failures.entry(id).or_insert(error);
            }
        }
    }
    Ok(result)
}

fn native_info(context: &mut Context<'_, '_>, id: &ModId) -> Result<Value> {
    context.jni(|env| {
        let id = env.new_string(id.as_str())?;
        let value = env
            .call_static_method(
                "zombie/gameStates/ChooseGameInfo",
                "getModDetails",
                "(Ljava/lang/String;)Lzombie/gameStates/ChooseGameInfo$Mod;",
                &[JValue::Object(&id)],
            )?
            .l()?;
        retain(env, &value)
    })
}

fn read_mod_info(context: &mut Context<'_, '_>, root: &std::path::Path) -> Result<Value> {
    context.jni(|env| {
        let path = env.new_string(root.to_string_lossy())?;
        let value = env
            .call_static_method(
                "zombie/gameStates/ChooseGameInfo",
                "readModInfo",
                "(Ljava/lang/String;)Lzombie/gameStates/ChooseGameInfo$Mod;",
                &[JValue::Object(&path)],
            )?
            .l()?;
        retain(env, &value)
    })
}

fn registered_workshop_info(context: &mut Context<'_, '_>, id: &ModId) -> Result<Value> {
    let filesystem = instance(context)?;
    let folders = context.jni(|env| {
        let name = env.new_string(id.as_str())?;
        let known = env
            .call_method(
                filesystem.object(),
                "getModDir",
                "(Ljava/lang/String;)Ljava/lang/String;",
                &[JValue::Object(&name)],
            )?
            .l()?;
        let list = env.new_object("java/util/ArrayList", "()V", &[])?;
        env.call_method(
            filesystem.object(),
            "getAllModFolders",
            "(Ljava/util/List;)V",
            &[JValue::Object(&list)],
        )?;
        if !known.is_null() {
            env.call_method(
                &list,
                "add",
                "(ILjava/lang/Object;)V",
                &[JValue::Int(0), JValue::Object(&known)],
            )?;
        }
        retain(env, &list)
    })?;
    for folder in java_list(context, &folders)? {
        let root = PathBuf::from(context.text(&folder)?);
        let info = read_mod_info(context, &root)?;
        if !info.is_nil() {
            let found = object_method(context, &info, "getId", "()Ljava/lang/String;")?;
            if context.text(&found)? == id.as_str() {
                return Ok(info);
            }
        }
    }
    Ok(Value::default())
}

fn discover_one(
    context: &mut Context<'_, '_>,
    profile: &Profile,
) -> Result<(NativeMetadata, Value)> {
    let mut info = if profile.path.is_some() {
        Value::default()
    } else if profile.workshop.is_some() {
        registered_workshop_info(context, &profile.id)?
    } else {
        native_info(context, &profile.id)?
    };
    if info.is_nil() && (profile.path.is_some() || profile.workshop.is_some()) {
        let root = if let Some(path) = &profile.path {
            luabuddy_core::source::absolute_normalized_path(path).map_err(|e| e.to_string())?
        } else {
            let workshop_id = profile.workshop.ok_or("Missing Workshop identity")?;
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
                return Err(format!(
                    "Steam is unavailable for Workshop item {workshop_id}; set path to its root directory."
                ));
            }
            let folder = context.jni(|env| {
                let workshop = env
                    .get_static_field(
                        "zombie/core/znet/SteamWorkshop",
                        "instance",
                        "Lzombie/core/znet/SteamWorkshop;",
                    )?
                    .l()?;
                let value = env
                    .call_method(
                        workshop,
                        "GetItemInstallFolder",
                        "(J)Ljava/lang/String;",
                        &[JValue::Long(workshop_id as i64)],
                    )?
                    .l()?;
                retain(env, &value)
            })?;
            if folder.is_nil() {
                return Err(format!(
                    "Workshop item {workshop_id} is not available; install it through Steam or set path to its root directory."
                ));
            }
            luabuddy_core::source::absolute_normalized_path(&PathBuf::from(context.text(&folder)?))
                .map_err(|e| e.to_string())?
        };
        if profile.workshop.is_some() && root.join("mod.info").is_file() {
            repair_flat_root(context, &root)?;
        }
        info = read_mod_info(context, &root)?;
    }
    let resolved = metadata(context, &profile.id, info)?;
    if profile.workshop.is_some() {
        let filesystem = instance(context)?;
        let dir = object_method(context, &resolved.1, "getDir", "()Ljava/lang/String;")?;
        context.jni(|env| {
            let id = env.new_string(profile.id.as_str())?;
            env.call_method(
                filesystem.object(),
                "setModIdToDir",
                "(Ljava/lang/String;Ljava/lang/String;)V",
                &[JValue::Object(&id), JValue::Object(dir.object())],
            )?;
            Ok(())
        })?;
    }
    Ok(resolved)
}

pub(crate) fn repair_flat_root(
    context: &mut Context<'_, '_>,
    root: &std::path::Path,
) -> Result<()> {
    let filesystem = instance(context)?;
    let flat = context.jni(|env| {
        let root = env.new_string(root.to_string_lossy())?;
        let flat = env
            .call_method(
                filesystem.object(),
                "getModInfoForDir",
                "(Ljava/lang/String;)Lzombie/gameStates/ChooseGameInfo$Mod;",
                &[JValue::Object(&root)],
            )?
            .l()?;
        retain(env, &flat)
    })?;
    for method in ["getCommonDir", "getVersionDir"] {
        let dir = object_method(context, &flat, method, "()Ljava/lang/String;")?;
        if !dir.is_nil()
            && PathBuf::from(context.text(&dir)?)
                .join("mod.info")
                .is_file()
        {
            return Ok(());
        }
    }
    context.jni(|env| {
        let root_string = env.new_string(root.to_string_lossy())?;
        env.set_field(
            flat.object(),
            "commonDir",
            "Ljava/lang/String;",
            JValue::Object(&root_string),
        )?;
        env.set_field(
            flat.object(),
            "versionDir",
            "Ljava/lang/String;",
            JValue::Object(&root_string),
        )?;
        for (field, path) in [
            ("baseFile", root.to_owned()),
            ("mediaFile", root.join("media")),
        ] {
            let path = env.new_string(path.to_string_lossy())?;
            let file = env.new_object(
                "java/io/File",
                "(Ljava/lang/String;)V",
                &[JValue::Object(&path)],
            )?;
            let folder = env
                .get_field(
                    flat.object(),
                    field,
                    "Lzombie/ZomboidFileSystem$PZModFolder;",
                )?
                .l()?;
            env.call_method(
                folder,
                "setWithCatch",
                "(Ljava/io/File;Ljava/io/File;)V",
                &[JValue::Object(&file), JValue::Object(&file)],
            )?;
        }
        Ok(())
    })
}

fn metadata(
    context: &mut Context<'_, '_>,
    expected: &ModId,
    info: Value,
) -> Result<(NativeMetadata, Value)> {
    if info.is_nil() {
        return Err(format!(
            "Mod not installed: {expected}. Install it or set path to its mod directory."
        ));
    }
    let id = object_method(context, &info, "getId", "()Ljava/lang/String;")?;
    let id = ModId::new(context.text(&id)?).map_err(|e| e.to_string())?;
    if &id != expected {
        return Err(format!("Expected {expected} but found {id}"));
    }
    let available = context.jni(|env| {
        env.call_method(info.object(), "isAvailableSelf", "()Z", &[])?
            .z()
    })?;
    if !available {
        return Err(format!("Mod is unavailable for this game version: {id}"));
    }
    let mut roots = Vec::new();
    for method in ["getCommonDir", "getVersionDir"] {
        let root = object_method(context, &info, method, "()Ljava/lang/String;")?;
        if root.is_nil() {
            return Err(format!("Native mod metadata has no {method} root for {id}"));
        }
        let root = PathBuf::from(context.text(&root)?);
        if !roots.contains(&root) {
            roots.push(root);
        }
    }
    let requires = strings(context, &info, "getRequire")?;
    let after = strings(context, &info, "getLoadAfter")?;
    let before = strings(context, &info, "getLoadBefore")?;
    Ok((
        NativeMetadata {
            id: Some(id),
            available,
            roots,
            requires,
            after,
            before,
        },
        info,
    ))
}

fn retain(env: &jni::JNIEnv<'_>, object: &jni::objects::JObject<'_>) -> jni::errors::Result<Value> {
    if object.is_null() {
        Ok(Value::default())
    } else {
        env.new_global_ref(object).map(|v| Value(Some(v)))
    }
}

pub fn mount(context: &mut Context<'_, '_>, id: &ModId, info: &Value) -> Result<()> {
    let filesystem = instance(context)?;
    let root = object_method(context, info, "getDir", "()Ljava/lang/String;")?;
    context.jni(|env| {
        let id = env.new_string(id.as_str())?;
        env.call_method(
            filesystem.object(),
            "setModIdToDir",
            "(Ljava/lang/String;Ljava/lang/String;)V",
            &[JValue::Object(&id), JValue::Object(root.object())],
        )?;
        env.call_method(
            filesystem.object(),
            "loadMod",
            "(Ljava/lang/String;)V",
            &[JValue::Object(&id)],
        )?;
        Ok(())
    })
}

pub fn callbacks(
    context: &mut Context<'_, '_>,
    event: &str,
    filenames: &BTreeSet<String>,
) -> Result<Vec<Value>> {
    context.jni(|env| {
        let name = env.new_string(event)?;
        let event = env
            .call_static_method(
                "zombie/Lua/LuaEventManager",
                "AddEvent",
                "(Ljava/lang/String;)Lzombie/Lua/Event;",
                &[JValue::Object(&name)],
            )?
            .l()?;
        let list = env
            .get_field(event, "callbacks", "Ljava/util/ArrayList;")?
            .l()?;
        let count = env.call_method(&list, "size", "()I", &[])?.i()?;
        let mut result = Vec::new();
        for index in 0..count {
            let callback = env
                .call_method(&list, "get", "(I)Ljava/lang/Object;", &[JValue::Int(index)])?
                .l()?;
            let prototype = env
                .get_field(&callback, "prototype", "Lse/krka/kahlua/vm/Prototype;")?
                .l()?;
            let filename = env
                .get_field(prototype, "filename", "Ljava/lang/String;")?
                .l()?;
            if !filename.is_null() {
                let filename = env
                    .get_string((&filename).into())?
                    .to_string_lossy()
                    .into_owned();
                if filenames.contains(&filename) {
                    result.push(Value(Some(env.new_global_ref(callback)?)));
                }
            }
        }
        Ok(result)
    })
}

pub fn callback_filename(context: &mut Context<'_, '_>, callback: &Value) -> Result<String> {
    let filename = context.jni(|env| {
        let prototype = env
            .get_field(
                callback.object(),
                "prototype",
                "Lse/krka/kahlua/vm/Prototype;",
            )?
            .l()?;
        let filename = env
            .get_field(prototype, "filename", "Ljava/lang/String;")?
            .l()?;
        retain(env, &filename)
    })?;
    context.text(&filename)
}

pub fn callback_registered(
    context: &mut Context<'_, '_>,
    event: &str,
    callback: &Value,
) -> Result<bool> {
    context.jni(|env| {
        let name = env.new_string(event)?;
        let event = env
            .call_static_method(
                "zombie/Lua/LuaEventManager",
                "AddEvent",
                "(Ljava/lang/String;)Lzombie/Lua/Event;",
                &[JValue::Object(&name)],
            )?
            .l()?;
        let list = env
            .get_field(event, "callbacks", "Ljava/util/ArrayList;")?
            .l()?;
        env.call_method(
            list,
            "contains",
            "(Ljava/lang/Object;)Z",
            &[JValue::Object(callback.object())],
        )?
        .z()
    })
}

pub fn remove_callbacks(
    context: &mut Context<'_, '_>,
    filenames: &BTreeSet<String>,
) -> Result<usize> {
    let events = context.jni(|env| {
        let list = env
            .get_static_field(
                "zombie/Lua/LuaEventManager",
                "EventList",
                "Ljava/util/ArrayList;",
            )?
            .l()?;
        retain(env, &list)
    })?;
    let mut removed = 0;
    for event in java_list(context, &events)? {
        let name = context.jni(|env| {
            let name = env
                .get_field(event.object(), "name", "Ljava/lang/String;")?
                .l()?;
            retain(env, &name)
        })?;
        let name = context.text(&name)?;
        let owned = callbacks(context, &name, filenames)?;
        if owned.is_empty() {
            continue;
        }
        let all = context.global("Events")?;
        let event = context.get(&all, &name)?;
        let remove = context.get(&event, "Remove")?;
        for callback in owned {
            if !callback_registered(context, &name, &callback)? {
                continue;
            }
            context.call(&remove, std::slice::from_ref(&callback))?;
            if callback_registered(context, &name, &callback)? {
                return Err(format!(
                    "Native removal did not remove owned callback from {name}; reconnect before retrying"
                ));
            }
            removed += 1;
        }
    }
    Ok(removed)
}

pub fn log(message: &str) {
    eprintln!("[LuaBuddy] {message}");
}

pub fn translations(context: &mut Context<'_, '_>, roots: &[PathBuf]) -> Result<()> {
    context.jni(|env| {
        let mut languages: Vec<jni::objects::GlobalRef> = Vec::new();
        let mut language = env.call_static_method("zombie/core/Translator", "getLanguage", "()Lzombie/core/Language;", &[])?.l()?;
        let manager = env.get_static_field("zombie/core/Languages", "instance", "Lzombie/core/Languages;")?.l()?;
        while !language.is_null() && !same_as_any(env, &languages, &language)? {
            languages.push(env.new_global_ref(&language)?);
            let base = env.call_method(&language, "base", "()Ljava/lang/String;", &[])?.l()?;
            language = env.call_method(&manager, "getByName", "(Ljava/lang/String;)Lzombie/core/Language;", &[JValue::Object(&base)])?.l()?;
        }
        let fallback = env.call_static_method("zombie/core/Translator", "getDefaultLanguage", "()Lzombie/core/Language;", &[])?.l()?;
        if !same_as_any(env, &languages, &fallback)? { languages.push(env.new_global_ref(fallback)?); }
        let maps = env.get_static_field("zombie/core/Translator", "BY_NAME", "Ljava/util/Map;")?.l()?;
        let keys = env.call_method(&maps, "keySet", "()Ljava/util/Set;", &[])?.l()?;
        let keys = env.call_method(keys, "toArray", "()[Ljava/lang/Object;", &[])?.l()?;
        let keys = jni::objects::JObjectArray::from(keys);
        let identity = env.call_static_method("java/util/function/Function", "identity", "()Ljava/util/function/Function;", &[])?.l()?;
        let mut prepared = Vec::new();
        for index in 0..env.get_array_length(&keys)? {
            let section = env.get_object_array_element(&keys, index)?;
            let merged = env.new_object("java/util/LinkedHashMap", "()V", &[])?;
            for language in languages.iter().rev() {
                for root in roots {
                    env.with_local_frame(16, |env| -> jni::errors::Result<()> {
                        let root = env.new_string(root.to_string_lossy())?;
                        env.call_static_method("zombie/core/Translator", "tryFillMapFromFile", "(Ljava/lang/String;Ljava/lang/String;Ljava/util/Map;Lzombie/core/Language;Ljava/util/function/Function;)V", &[JValue::Object(&root), JValue::Object(&section), JValue::Object(&merged), JValue::Object(language.as_obj()), JValue::Object(&identity)])?;
                        Ok(())
                    })?;
                }
            }
            let entries = env.call_method(&merged, "entrySet", "()Ljava/util/Set;", &[])?.l()?;
            let iterator = env.call_method(entries, "iterator", "()Ljava/util/Iterator;", &[])?.l()?;
            while env.call_method(&iterator, "hasNext", "()Z", &[])?.z()? {
                env.with_local_frame(16, |env| -> jni::errors::Result<()> {
                    let entry = env.call_method(&iterator, "next", "()Ljava/lang/Object;", &[])?.l()?;
                    let value = env.call_method(&entry, "getValue", "()Ljava/lang/Object;", &[])?.l()?;
                    let value = env.call_static_method("zombie/core/Translator", "formatFixer", "(Ljava/lang/String;)Ljava/lang/String;", &[JValue::Object(&value)])?.l()?;
                    env.call_method(entry, "setValue", "(Ljava/lang/Object;)Ljava/lang/Object;", &[JValue::Object(&value)])?;
                    Ok(())
                })?;
            }
            prepared.push((env.new_global_ref(section)?, env.new_global_ref(merged)?));
        }
        for (section, merged) in prepared {
            let target = env.call_method(&maps, "get", "(Ljava/lang/Object;)Ljava/lang/Object;", &[JValue::Object(section.as_obj())])?.l()?;
            env.call_method(target, "putAll", "(Ljava/util/Map;)V", &[JValue::Object(merged.as_obj())])?;
        }
        Ok(())
    })
}

fn same_as_any(
    env: &jni::JNIEnv<'_>,
    references: &[jni::objects::GlobalRef],
    object: &jni::objects::JObject<'_>,
) -> jni::errors::Result<bool> {
    for reference in references {
        if env.is_same_object(reference.as_obj(), object)? {
            return Ok(true);
        }
    }
    Ok(false)
}
