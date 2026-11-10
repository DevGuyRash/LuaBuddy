use crate::lua::{Context, Result, Value};
use jni::{
    JNIEnv,
    objects::{JObject, JString, JValue},
};
use luabuddy_core::{ModId, Profile, profile::LivePolicy};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd)]
pub enum StartupComponent {
    Definitions,
    Maps,
    Animations,
    TexturePacks,
    TileDefinitions,
    ServerLua,
}

impl StartupComponent {
    fn description(self) -> &'static str {
        match self {
            Self::Definitions => "native definitions",
            Self::Maps => "maps",
            Self::Animations => "animation assets",
            Self::TexturePacks => "texture packs",
            Self::TileDefinitions => "tile definitions",
            Self::ServerLua => "server Lua",
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum Eligibility {
    Live { limitations: Vec<StartupComponent> },
    RestartRequired { reason: String },
}

impl Eligibility {
    pub fn explanation(&self) -> String {
        match self {
            Self::Live { limitations } if limitations.is_empty() => "Client/shared Lua can be requested through the late loader. Native compatibility and server policy still apply.".into(),
            Self::Live { limitations } => format!("This profile explicitly restores client/shared Lua only. Its {} are not activated by LuaBuddy. Server code remains controlled by the server.", descriptions(limitations)),
            Self::RestartRequired { reason } => reason.clone(),
        }
    }
}

fn descriptions(components: &[StartupComponent]) -> String {
    components
        .iter()
        .map(|component| component.description())
        .collect::<Vec<_>>()
        .join(", ")
}

pub fn assess(profile: &Profile, roots: &[PathBuf], templates: &[Profile]) -> Result<Eligibility> {
    let policy = profile
        .live
        .or_else(|| {
            templates
                .iter()
                .find(|template| template.id == profile.id)
                .and_then(|template| template.live)
        })
        .unwrap_or(LivePolicy::Auto);
    let mut components = BTreeSet::new();
    for root in roots {
        for media in resource_paths(root, "media")? {
            if contains_tile_definitions(&media)? {
                components.insert(StartupComponent::TileDefinitions);
                break;
            }
        }
        for (relative, component) in [
            ("media/scripts", StartupComponent::Definitions),
            ("media/maps", StartupComponent::Maps),
            ("media/animations", StartupComponent::Animations),
            ("media/animsets", StartupComponent::Animations),
            ("media/anims_X", StartupComponent::Animations),
            ("media/texturepacks", StartupComponent::TexturePacks),
            ("media/lua/server", StartupComponent::ServerLua),
        ] {
            for candidate in resource_paths(root, relative)? {
                if contains_file(&candidate, &mut BTreeSet::new())? {
                    components.insert(component);
                    break;
                }
            }
        }
    }
    let components: Vec<_> = components.into_iter().collect();
    if policy == LivePolicy::Startup {
        return Ok(Eligibility::RestartRequired { reason: "This profile explicitly requires native startup loading. Select Startup enable and restart through the game's normal loading path.".into() });
    }
    if policy == LivePolicy::Auto && !components.is_empty() {
        return Ok(Eligibility::RestartRequired {
            reason: format!(
                "Restart required. This mod supplies {}. Enable it for native startup rather than treating client/shared Lua restoration as complete activation. Multiplayer server code still requires server approval.",
                descriptions(&components)
            ),
        });
    }
    Ok(Eligibility::Live {
        limitations: components,
    })
}

fn contains_tile_definitions(root: &Path) -> Result<bool> {
    let mut pending = vec![root.to_owned()];
    let mut visited = BTreeSet::new();
    while let Some(path) = pending.pop() {
        if !path.exists() {
            continue;
        }
        if path.is_file() {
            if path
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("tiles"))
            {
                return Ok(true);
            }
            continue;
        }
        let canonical = path
            .canonicalize()
            .map_err(|error| format!("Inspect tile definitions {}: {error}", path.display()))?;
        if !visited.insert(canonical) {
            continue;
        }
        for entry in fs::read_dir(&path)
            .map_err(|error| format!("Inspect tile definitions {}: {error}", path.display()))?
        {
            pending.push(
                entry
                    .map_err(|error| {
                        format!("Inspect tile definitions {}: {error}", path.display())
                    })?
                    .path(),
            );
        }
    }
    Ok(false)
}

fn resource_paths(root: &Path, relative: &str) -> Result<Vec<PathBuf>> {
    let mut candidates = vec![root.to_owned()];
    for segment in relative.split('/') {
        let mut matching = Vec::new();
        for directory in candidates {
            match fs::metadata(&directory) {
                Ok(_) => (),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => {
                    return Err(format!(
                        "Inspect startup resource directory {}: {error}",
                        directory.display()
                    ));
                }
            }
            for entry in fs::read_dir(&directory).map_err(|error| {
                format!(
                    "Inspect startup resource directory {}: {error}",
                    directory.display()
                )
            })? {
                let entry = entry.map_err(|error| {
                    format!(
                        "Inspect startup resource directory {}: {error}",
                        directory.display()
                    )
                })?;
                if entry
                    .file_name()
                    .to_string_lossy()
                    .eq_ignore_ascii_case(segment)
                {
                    matching.push(entry.path());
                }
            }
        }
        candidates = matching;
    }
    Ok(candidates)
}

fn contains_file(path: &Path, visited: &mut BTreeSet<PathBuf>) -> Result<bool> {
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(format!(
                "Inspect startup assets {}: {error}",
                path.display()
            ));
        }
    };
    if metadata.is_file() {
        return Ok(true);
    }
    let identity = path
        .canonicalize()
        .map_err(|error| format!("Inspect startup assets {}: {error}", path.display()))?;
    if !visited.insert(identity) {
        return Ok(false);
    }
    for entry in fs::read_dir(path)
        .map_err(|error| format!("Inspect startup assets {}: {error}", path.display()))?
    {
        let entry =
            entry.map_err(|error| format!("Inspect startup assets {}: {error}", path.display()))?;
        if contains_file(&entry.path(), visited)? {
            return Ok(true);
        }
    }
    Ok(false)
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct StartupSelection {
    pub selected: BTreeSet<ModId>,
    pub changed: Vec<ModId>,
    pub enabled: bool,
}

pub fn selected_next_startup(context: &mut Context<'_, '_>) -> Result<BTreeSet<ModId>> {
    let selection = default_selection(context)?;
    lists(context, &selection)?
        .0
        .into_iter()
        .map(ModId::new)
        .collect::<std::result::Result<_, _>>()
        .map_err(|error| error.to_string())
}

pub fn select_next_startup(
    context: &mut Context<'_, '_>,
    ids: &[ModId],
    enabled: bool,
) -> Result<StartupSelection> {
    if context.jni(|env| {
        env.get_static_field("zombie/network/GameServer", "server", "Z")?
            .z()
    })? {
        return Err(
            "Native player startup preferences are unavailable in a dedicated server".into(),
        );
    }
    if ids.is_empty() {
        return Err("Select an installed native mod identity".into());
    }
    let default = default_selection(context)
        .map_err(|error| format!("Resolve native default selection: {error}"))?;
    let original = lists(context, &default)
        .map_err(|error| format!("Read native default selection: {error}"))?;
    let native_active = native_active_ids(context)
        .map_err(|error| format!("Read active native mod IDs: {error}"))?;
    let current_game = named_selection(context, "currentGame")
        .map_err(|error| format!("Resolve native current-game selection: {error}"))?;
    let current_game_before = lists(context, &current_game)
        .map_err(|error| format!("Read native current-game selection: {error}"))?;
    let candidate = copied_selection(context, &default)?;
    let mut metadata = BTreeMap::new();
    let mut updates = Vec::new();
    if enabled {
        let mut visiting = BTreeSet::new();
        let mut visited = BTreeSet::new();
        for id in ids {
            dependency_order(
                context,
                id,
                &mut metadata,
                &mut visiting,
                &mut visited,
                &mut updates,
            )?;
        }
    } else {
        let removing: BTreeSet<_> = ids.iter().cloned().collect();
        for id in &original.0 {
            let id = ModId::new(id.clone()).map_err(|error| error.to_string())?;
            if removing.contains(&id) {
                continue;
            }
            if let Some(info) = native_info(context, &id)? {
                let required = required_ids(context, &info)?;
                if let Some(required) = required
                    .iter()
                    .find(|required| removing.contains(*required))
                {
                    return Err(format!(
                        "Disable native dependent {id} before disabling {required} for next startup"
                    ));
                }
            }
        }
        updates.extend(ids.iter().cloned());
    }
    for id in &updates {
        context.jni(|env| {
            let id = env.new_string(id.as_str())?;
            env.call_method(
                candidate.object(),
                "setModActive",
                "(Ljava/lang/String;Z)V",
                &[JValue::Object(&id), JValue::Bool(enabled.into())],
            )?;
            Ok(())
        })?;
    }
    let expected = lists(context, &candidate)?;
    let changed = updates
        .into_iter()
        .filter(|id| original.0.contains(&id.to_string()) != enabled)
        .collect();
    let destination = native_destination(context)?;
    reject_symlinks(&destination)?;
    let parent = destination
        .parent()
        .ok_or("Native selection has no parent directory")?;
    fs::create_dir_all(parent).map_err(|error| {
        format!(
            "Create native mod preferences {}: {error}",
            parent.display()
        )
    })?;
    let before = read_existing(&destination)?;
    let original_permissions = if before.is_some() {
        Some(
            fs::metadata(&destination)
                .map_err(|error| format!("Read native preference permissions: {error}"))?
                .permissions(),
        )
    } else {
        None
    };
    if before.is_some() && expected == original {
        let persisted = empty_selection(context)?;
        if read_native(context, &destination, &persisted)?
            && lists(context, &persisted)? == expected
        {
            let selected = expected
                .0
                .into_iter()
                .map(ModId::new)
                .collect::<std::result::Result<_, _>>()
                .map_err(|error| error.to_string())?;
            return Ok(StartupSelection {
                selected,
                changed,
                enabled,
            });
        }
    }
    let stage = Stage::new(parent)?;
    if !write_native(context, &stage.0, &candidate)? {
        return Err("The game's native ActiveMods writer refused the startup selection".into());
    }
    OpenOptions::new()
        .write(true)
        .open(&stage.0)
        .and_then(|file| file.sync_all())
        .map_err(|error| format!("Flush staged native preferences: {error}"))?;
    let verified = empty_selection(context)?;
    if !read_native(context, &stage.0, &verified)? || lists(context, &verified)? != expected {
        return Err(
            "Native startup preference readback does not match the requested mods and map order"
                .into(),
        );
    }
    if read_existing(&destination)? != before {
        return Err(
            "Native startup preferences changed during the operation; no selection was overwritten"
                .into(),
        );
    }
    if let Some(permissions) = &original_permissions {
        if fs::metadata(&destination)
            .map_err(|error| format!("Recheck native preference permissions: {error}"))?
            .permissions()
            != *permissions
        {
            return Err("Native startup preference permissions changed during the operation; no selection was overwritten".into());
        }
        fs::set_permissions(&stage.0, permissions.clone())
            .map_err(|error| format!("Preserve native preference permissions: {error}"))?;
    }
    let published = read_existing(&stage.0)?.ok_or("Staged native preference file disappeared")?;
    let published_permissions = fs::metadata(&stage.0)
        .map_err(|error| format!("Read staged native preference permissions: {error}"))?
        .permissions();
    fs::rename(&stage.0, &destination).map_err(|error| {
        format!(
            "Publish native startup preferences {}: {error}",
            destination.display()
        )
    })?;
    let publish = (|| {
        copy_selection(context, &default, &candidate)?;
        if lists(context, &default)? != expected {
            return Err("The game's default selection did not retain the requested state".into());
        }
        if native_active_ids(context)? != native_active
            || lists(context, &current_game)? != current_game_before
        {
            return Err(
                "Native current-game or active server selection changed unexpectedly".into(),
            );
        }
        Ok(())
    })();
    if let Err(error) = publish {
        let owns_publication = read_existing(&destination)
            .is_ok_and(|current| current.as_deref() == Some(published.as_slice()))
            && fs::metadata(&destination)
                .is_ok_and(|metadata| metadata.permissions() == published_permissions);
        let rollback = if owns_publication {
            restore_file(
                &destination,
                before.as_deref(),
                original_permissions.as_ref(),
            )
        } else {
            Err("Native startup preferences changed after publication; independent edits were retained".to_owned())
        };
        let memory = restore_lists(context, &default, &original);
        return match (rollback, memory) {
            (Ok(()), Ok(())) => Err(error),
            (file, memory) => Err(format!(
                "{error}; startup preference restoration uncertain: file={file:?}, memory={memory:?}"
            )),
        };
    }
    let selected = expected
        .0
        .into_iter()
        .map(ModId::new)
        .collect::<std::result::Result<_, _>>()
        .map_err(|error| error.to_string())?;
    Ok(StartupSelection {
        selected,
        changed,
        enabled,
    })
}

fn dependency_order(
    context: &mut Context<'_, '_>,
    id: &ModId,
    metadata: &mut BTreeMap<ModId, Vec<ModId>>,
    visiting: &mut BTreeSet<ModId>,
    visited: &mut BTreeSet<ModId>,
    result: &mut Vec<ModId>,
) -> Result<()> {
    let mut pending = vec![(id.clone(), false)];
    while let Some((id, leaving)) = pending.pop() {
        if leaving {
            visiting.remove(&id);
            visited.insert(id.clone());
            result.push(id);
            continue;
        }
        if visited.contains(&id) {
            continue;
        }
        if !visiting.insert(id.clone()) {
            return Err(format!("Native startup dependency cycle at {id}"));
        }
        if !metadata.contains_key(&id) {
            let info = native_info(context, &id)
                .map_err(|error| format!("Resolve native startup metadata for {id}: {error}"))?
                .ok_or_else(|| format!("Native startup mod is not installed: {id}"))?;
            let actual = context.jni(|env| {
                env.with_local_frame(8, |env| {
                    let value = env
                        .call_method(info.object(), "getId", "()Ljava/lang/String;", &[])?
                        .l()?;
                    let text = env
                        .get_string(&JString::from(value))?
                        .to_string_lossy()
                        .into_owned();
                    Ok(text)
                })
            })?;
            if actual != id.as_str() {
                return Err(format!("Native startup identity mismatch for {id}"));
            }
            if !context.jni(|env| {
                env.call_method(info.object(), "isAvailableSelf", "()Z", &[])?
                    .z()
            })? {
                return Err(format!(
                    "Native startup mod is incompatible with this game version: {id}"
                ));
            }
            metadata.insert(
                id.clone(),
                required_ids(context, &info).map_err(|error| {
                    format!("Read native startup dependencies for {id}: {error}")
                })?,
            );
        }
        pending.push((id.clone(), true));
        for required in metadata.get(&id).into_iter().flatten().rev() {
            pending.push((required.clone(), false));
        }
    }
    Ok(())
}

fn native_info(context: &mut Context<'_, '_>, id: &ModId) -> Result<Option<Value>> {
    context.jni(|env| {
        env.with_local_frame(16, |env| {
            let id = env.new_string(id.as_str())?;
            let value = env
                .call_static_method(
                    "zombie/gameStates/ChooseGameInfo",
                    "getModDetails",
                    "(Ljava/lang/String;)Lzombie/gameStates/ChooseGameInfo$Mod;",
                    &[JValue::Object(&id)],
                )?
                .l()?;
            if value.is_null() {
                Ok(None)
            } else {
                env.new_global_ref(value)
                    .map(|value| Some(Value(Some(value))))
            }
        })
    })
}

fn required_ids(context: &mut Context<'_, '_>, info: &Value) -> Result<Vec<ModId>> {
    let required = context.jni(|env| {
        env.with_local_frame(16, |env| {
            let list = env
                .call_method(info.object(), "getRequire", "()Ljava/util/ArrayList;", &[])?
                .l()?;
            java_strings(env, &list)
        })
    })?;
    required
        .into_iter()
        .map(ModId::new)
        .collect::<std::result::Result<_, _>>()
        .map_err(|error| error.to_string())
}

fn default_selection(context: &mut Context<'_, '_>) -> Result<Value> {
    named_selection(context, "default")
}

fn named_selection(context: &mut Context<'_, '_>, name: &str) -> Result<Value> {
    context.jni(|env| {
        let name = env.new_string(name)?;
        let value = env
            .call_static_method(
                "zombie/modding/ActiveMods",
                "getById",
                "(Ljava/lang/String;)Lzombie/modding/ActiveMods;",
                &[JValue::Object(&name)],
            )?
            .l()?;
        env.new_global_ref(value).map(|value| Value(Some(value)))
    })
}

fn empty_selection(context: &mut Context<'_, '_>) -> Result<Value> {
    context.jni(|env| {
        let name = env.new_string("LuaBuddy-private-staging")?;
        let value = env.new_object(
            "zombie/modding/ActiveMods",
            "(Ljava/lang/String;)V",
            &[JValue::Object(&name)],
        )?;
        env.new_global_ref(value).map(|value| Value(Some(value)))
    })
}

fn copied_selection(context: &mut Context<'_, '_>, original: &Value) -> Result<Value> {
    let copy = empty_selection(context)?;
    copy_selection(context, &copy, original)?;
    Ok(copy)
}

fn copy_selection(
    context: &mut Context<'_, '_>,
    destination: &Value,
    source: &Value,
) -> Result<()> {
    context.jni(|env| {
        env.call_method(
            destination.object(),
            "copyFrom",
            "(Lzombie/modding/ActiveMods;)V",
            &[JValue::Object(source.object())],
        )?;
        Ok(())
    })
}

fn lists(context: &mut Context<'_, '_>, selection: &Value) -> Result<(Vec<String>, Vec<String>)> {
    let mut values = |method: &str| {
        context.jni(|env| {
            env.with_local_frame(16, |env| {
                let list = env
                    .call_method(selection.object(), method, "()Ljava/util/ArrayList;", &[])?
                    .l()?;
                java_strings(env, &list)
            })
        })
    };
    Ok((values("getMods")?, values("getMapOrder")?))
}

fn native_active_ids(context: &mut Context<'_, '_>) -> Result<Vec<String>> {
    context.jni(|env| {
        env.with_local_frame(16, |env| {
            let filesystem = env
                .get_static_field(
                    "zombie/ZomboidFileSystem",
                    "instance",
                    "Lzombie/ZomboidFileSystem;",
                )?
                .l()?;
            let list = env
                .call_method(filesystem, "getModIDs", "()Ljava/util/List;", &[])?
                .l()?;
            java_strings(env, &list)
        })
    })
}

fn java_strings(env: &mut JNIEnv<'_>, list: &JObject<'_>) -> jni::errors::Result<Vec<String>> {
    if list.is_null() {
        return Ok(Vec::new());
    }
    let count = env.call_method(list, "size", "()I", &[])?.i()?;
    let mut result = Vec::new();
    for index in 0..count {
        let value = env
            .call_method(list, "get", "(I)Ljava/lang/Object;", &[JValue::Int(index)])?
            .l()?;
        let value = env.auto_local(JString::from(value));
        result.push(env.get_string(&value)?.to_string_lossy().into_owned());
    }
    Ok(result)
}

fn native_destination(context: &mut Context<'_, '_>) -> Result<PathBuf> {
    let folder = context.jni(|env| {
        let value = env
            .call_static_method(
                "zombie/core/Core",
                "getMyDocumentFolder",
                "()Ljava/lang/String;",
                &[],
            )?
            .l()?;
        env.new_global_ref(value).map(|value| Value(Some(value)))
    })?;
    Ok(PathBuf::from(context.text(&folder)?).join("mods/default.txt"))
}

fn write_native(context: &mut Context<'_, '_>, path: &Path, selection: &Value) -> Result<bool> {
    context.jni(|env| {
        let writer = env.new_object("zombie/modding/ActiveModsFile", "()V", &[])?;
        let path = env.new_string(path.to_string_lossy())?;
        env.call_method(
            writer,
            "write",
            "(Ljava/lang/String;Lzombie/modding/ActiveMods;)Z",
            &[JValue::Object(&path), JValue::Object(selection.object())],
        )?
        .z()
    })
}

fn read_native(context: &mut Context<'_, '_>, path: &Path, selection: &Value) -> Result<bool> {
    context.jni(|env| {
        let reader = env.new_object("zombie/modding/ActiveModsFile", "()V", &[])?;
        let path = env.new_string(path.to_string_lossy())?;
        env.call_method(
            reader,
            "read",
            "(Ljava/lang/String;Lzombie/modding/ActiveMods;)Z",
            &[JValue::Object(&path), JValue::Object(selection.object())],
        )?
        .z()
    })
}

fn restore_lists(
    context: &mut Context<'_, '_>,
    selection: &Value,
    expected: &(Vec<String>, Vec<String>),
) -> Result<()> {
    context.jni(|env| {
        env.call_method(selection.object(), "clear", "()V", &[])?;
        Ok(())
    })?;
    for (method, values) in [("getMods", &expected.0), ("getMapOrder", &expected.1)] {
        context.jni(|env| {
            let list = env
                .call_method(selection.object(), method, "()Ljava/util/ArrayList;", &[])?
                .l()?;
            for value in values {
                let value = env.new_string(value)?;
                env.call_method(
                    &list,
                    "add",
                    "(Ljava/lang/Object;)Z",
                    &[JValue::Object(&value)],
                )?;
            }
            Ok(())
        })?;
    }
    Ok(())
}

fn reject_symlinks(path: &Path) -> Result<()> {
    for ancestor in path.ancestors() {
        if fs::symlink_metadata(ancestor).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
            return Err(format!(
                "Refusing a symlink in native startup preference path {}",
                ancestor.display()
            ));
        }
    }
    Ok(())
}

fn read_existing(path: &Path) -> Result<Option<Vec<u8>>> {
    match fs::metadata(path) {
        Ok(metadata) if metadata.len() > 4 * 1024 * 1024 => {
            Err("Native startup preferences exceed the four MiB safety limit".into())
        }
        Ok(_) => fs::read(path).map(Some).map_err(|error| {
            format!(
                "Read native startup preferences {}: {error}",
                path.display()
            )
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!(
            "Inspect native startup preferences {}: {error}",
            path.display()
        )),
    }
}

struct Stage(PathBuf);
impl Stage {
    fn new(directory: &Path) -> Result<Self> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = directory.join(format!(
            ".luabuddy-native-{}-{}.tmp",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|error| {
                format!(
                    "Stage native startup preferences {}: {error}",
                    path.display()
                )
            })?;
        Ok(Self(path))
    }
}
impl Drop for Stage {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn restore_file(
    destination: &Path,
    before: Option<&[u8]>,
    permissions: Option<&fs::Permissions>,
) -> Result<()> {
    match before {
        Some(bytes) => {
            let stage = Stage::new(
                destination
                    .parent()
                    .ok_or("Native preference path has no parent")?,
            )?;
            let mut file = OpenOptions::new()
                .write(true)
                .truncate(true)
                .open(&stage.0)
                .map_err(|error| error.to_string())?;
            file.write_all(bytes)
                .and_then(|()| file.sync_all())
                .map_err(|error| error.to_string())?;
            if let Some(permissions) = permissions {
                fs::set_permissions(&stage.0, permissions.clone())
                    .map_err(|error| error.to_string())?;
            }
            fs::rename(&stage.0, destination).map_err(|error| error.to_string())
        }
        None => fs::remove_file(destination).map_err(|error| error.to_string()),
    }
}
