use crate::{
    game,
    lua::{Context, Result, Value},
    patcher::JvmPatcher,
};
use luabuddy_patcher::{
    PatchState,
    extension::{ApprovalRecordV1, EnableConsent, ExtensionManifest, LoadedExtension, verify},
};
use serde_json::{Value as Json, json};
use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};

struct Entry {
    id: String,
    sha256: String,
    manifest_sha256: String,
    enabled: bool,
    provenance: String,
    patches: Vec<String>,
    error: Option<String>,
    pending: Option<Pending>,
}

struct Pending {
    artifact: PathBuf,
    manifest: ExtensionManifest,
    consent: EnableConsent,
}

#[derive(Default)]
struct Manager {
    root: Option<PathBuf>,
    store_error: Option<String>,
    engine: JvmPatcher,
    entries: BTreeMap<String, Entry>,
    initialized: bool,
    busy: bool,
}

static MANAGER: OnceLock<Mutex<Manager>> = OnceLock::new();

fn manager() -> &'static Mutex<Manager> {
    MANAGER.get_or_init(|| Mutex::new(Manager::default()))
}

fn root(context: &mut Context<'_, '_>) -> Result<PathBuf> {
    if let Some(root) = game::property(context, "luabuddy.extensions")? {
        return Ok(PathBuf::from(root));
    }
    Ok(PathBuf::from(
        game::property(context, "luabuddy.config")?.ok_or("Missing LuaBuddy profile directory")?,
    )
    .join("extensions"))
}

fn plain(path: &Path) -> Result<()> {
    for ancestor in path.ancestors() {
        if fs::symlink_metadata(ancestor).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
            return Err(format!(
                "Refusing a symlink in the approved native extension path {}",
                ancestor.display()
            ));
        }
    }
    Ok(())
}

fn bytes(path: &Path) -> Result<Vec<u8>> {
    plain(path)?;
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("Inspect {}: {error}", path.display()))?;
    if !metadata.is_file() || metadata.len() > 1024 * 1024 {
        return Err(format!(
            "Expected bounded regular extension metadata {}",
            path.display()
        ));
    }
    let mut bytes = Vec::new();
    File::open(path)
        .and_then(|file| file.take(1024 * 1024 + 1).read_to_end(&mut bytes))
        .map_err(|error| format!("Read {}: {error}", path.display()))?;
    if bytes.len() > 1024 * 1024 {
        return Err("Extension metadata grew beyond its size limit".into());
    }
    Ok(bytes)
}

fn artifact_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "artifact.dll"
    } else if cfg!(target_os = "macos") {
        "artifact.dylib"
    } else {
        "artifact.so"
    }
}

fn load_entry(context: &mut Context<'_, '_>, directory: &Path, engine: &mut JvmPatcher) -> Entry {
    let fallback = directory
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let mut entry = Entry {
        id: fallback,
        sha256: String::new(),
        manifest_sha256: String::new(),
        enabled: false,
        provenance: String::new(),
        patches: Vec::new(),
        error: None,
        pending: None,
    };
    let result = (|| -> Result<()> {
        let record: ApprovalRecordV1 =
            serde_json::from_slice(&bytes(&directory.join("approval.json"))?)
                .map_err(|error| error.to_string())?;
        record.validate().map_err(|error| error.to_string())?;
        entry.manifest_sha256 = record.manifest_sha256.clone();
        let consent = record.consent;
        if consent.extension_id != entry.id
            || consent.extension_id.is_empty()
            || consent.extension_id.len() >= 64
            || consent
                .extension_id
                .bytes()
                .any(|b| !b.is_ascii_alphanumeric() && !b"._-".contains(&b))
        {
            return Err("Approval identity differs from its owned directory".into());
        }
        entry.sha256 = consent.artifact_sha256.clone();
        entry.enabled = consent.enable;
        entry.provenance = consent.provenance.clone();
        if !consent.install {
            return Err("Native extension has no explicit installation approval".into());
        }
        if !consent.enable {
            return Ok(());
        }
        if consent.artifact_sha256.len() != 64
            || !consent
                .artifact_sha256
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        {
            return Err("Native extension approval has an invalid SHA256 generation".into());
        }
        let generation = directory.join(&record.manifest_sha256);
        plain(&generation)?;
        let manifest_bytes = bytes(&generation.join("manifest.json"))?;
        use sha2::{Digest, Sha256};
        if format!("{:x}", Sha256::digest(&manifest_bytes)) != record.manifest_sha256 {
            return Err("Manifest bytes differ from explicit native approval".into());
        }
        let manifest: ExtensionManifest =
            serde_json::from_slice(&manifest_bytes).map_err(|error| error.to_string())?;
        entry.patches = manifest
            .patches
            .iter()
            .map(|patch| patch.id.clone())
            .collect();
        let artifact = generation.join(artifact_name());
        plain(&artifact)?;
        let metadata = fs::metadata(&artifact).map_err(|error| error.to_string())?;
        if !metadata.is_file() || metadata.len() > 256 * 1024 * 1024 {
            return Err("Native artifact exceeds the regular 256 MiB limit".into());
        }
        verify(&artifact, manifest.clone(), &consent).map_err(|error| error.to_string())?;
        entry.pending = Some(Pending {
            artifact,
            manifest,
            consent,
        });
        install_pending(context, &mut entry, engine)
    })();
    if let Err(error) = result {
        entry.error = Some(error);
    }
    entry
}

fn install_pending(
    context: &mut Context<'_, '_>,
    entry: &mut Entry,
    engine: &mut JvmPatcher,
) -> Result<()> {
    let pending = entry
        .pending
        .as_ref()
        .ok_or("No approved native generation is pending")?;
    let capabilities = engine.capabilities();
    for patch in &pending.manifest.patches {
        if !capabilities.hooks.contains(&patch.kind) {
            return Err(format!(
                "Unsupported native patch operation {:?}",
                patch.kind
            ));
        }
        let observed = JvmPatcher::target(
            context.env,
            &patch.target.class,
            &patch.target.method,
            &patch.target.descriptor,
            &patch.target.loader,
        )
        .map_err(|error| error.to_string())?;
        if observed != patch.target {
            return Err(format!(
                "Native target fingerprint differs for {}",
                patch.id
            ));
        }
    }
    let verified = verify(
        &pending.artifact,
        pending.manifest.clone(),
        &pending.consent,
    )
    .map_err(|error| error.to_string())?;
    // SAFETY: Only separately installed and enabled local binaries reach this
    // boundary. The persisted digest, provenance and full privilege approval
    // are the operator's explicit native-code trust contract. Such binaries
    // must implement ABI v1, never unwind across it, and have trusted native
    // dependencies. This executes full-privilege native code, not a sandbox.
    let loaded = unsafe { LoadedExtension::load(verified) };
    entry.pending = None;
    let loaded = loaded.map_err(|error| error.to_string())?;
    engine
        .register_extension(loaded)
        .map_err(|error| error.to_string())?;
    entry.error = None;
    Ok(())
}

fn scan(
    context: &mut Context<'_, '_>,
    root: &Path,
    engine: &mut JvmPatcher,
) -> Result<BTreeMap<String, Entry>> {
    plain(root)?;
    if !root.exists() {
        return Ok(BTreeMap::new());
    }
    let mut directories = Vec::new();
    for directory in fs::read_dir(root)
        .map_err(|error| format!("Read native approvals {}: {error}", root.display()))?
    {
        let directory = directory.map_err(|error| error.to_string())?;
        if directory.file_name() == ".luabuddy" {
            continue;
        }
        if directory
            .file_type()
            .map_err(|error| error.to_string())?
            .is_dir()
        {
            if directories.len() >= 256 {
                return Err("Native extension store exceeds 256 entries".into());
            }
            directories.push(directory.path());
        }
    }
    directories.sort();
    Ok(directories
        .into_iter()
        .map(|path| {
            let entry = load_entry(context, &path, engine);
            (entry.id.clone(), entry)
        })
        .collect())
}

fn activate(context: &mut Context<'_, '_>, state: &mut Manager) {
    for entry in state
        .entries
        .values_mut()
        .filter(|entry| entry.enabled && entry.error.is_none())
    {
        if let Err(error) = state.engine.activate(context.env, &entry.patches) {
            entry.error = Some(error.to_string());
        }
    }
    let diagnostics = state.engine.diagnostics();
    for entry in state.entries.values_mut() {
        if !entry.patches.is_empty()
            && entry.patches.iter().all(|id| {
                diagnostics.iter().any(|patch| {
                    &patch.id == id
                        && patch.owner == entry.id
                        && patch.state == PatchState::Installed
                })
            })
        {
            entry.error = None;
        }
    }
}

fn take() -> Result<Manager> {
    let mut shared = manager()
        .lock()
        .map_err(|_| "Native extension manager is poisoned")?;
    if shared.busy {
        return Err("A native extension operation is already running".into());
    }
    let state = std::mem::take(&mut *shared);
    shared.busy = true;
    Ok(state)
}

fn restore(mut state: Manager) -> Result<()> {
    state.busy = false;
    *manager()
        .lock()
        .map_err(|_| "Native extension manager is poisoned")? = state;
    Ok(())
}

pub fn initialize(context: &mut Context<'_, '_>) -> Result<()> {
    let root = root(context)?;
    let mut state = take()?;
    let result = if state.initialized {
        if state.root.as_ref() == Some(&root) {
            match &state.store_error {
                Some(error) => Err(error.clone()),
                None => Ok(()),
            }
        } else {
            Err("Restart the game to change native extension stores".into())
        }
    } else {
        state.root = Some(root.clone());
        state.initialized = true;
        match scan(context, &root, &mut state.engine) {
            Ok(entries) => {
                state.entries = entries;
                activate(context, &mut state);
                Ok(())
            }
            Err(error) => {
                state.store_error = Some(error.clone());
                state.entries.insert(
                    "store".into(),
                    Entry {
                        id: "store".into(),
                        sha256: String::new(),
                        manifest_sha256: String::new(),
                        enabled: false,
                        provenance: String::new(),
                        patches: Vec::new(),
                        error: Some(error.clone()),
                        pending: None,
                    },
                );
                Err(error)
            }
        }
    };
    restore(state)?;
    result
}

pub fn retry(context: &mut Context<'_, '_>) -> Result<()> {
    initialize(context)?;
    let mut state = take()?;
    for entry in state
        .entries
        .values_mut()
        .filter(|entry| entry.enabled && entry.pending.is_some())
    {
        if let Err(error) = install_pending(context, entry, &mut state.engine) {
            entry.error = Some(error);
        }
    }
    let diagnostics = state.engine.diagnostics();
    for entry in state.entries.values_mut().filter(|entry| entry.enabled) {
        if !entry.patches.is_empty()
            && entry.patches.iter().all(|id| {
                diagnostics
                    .iter()
                    .any(|patch| &patch.id == id && patch.owner == entry.id)
            })
        {
            entry.error = None;
        }
    }
    activate(context, &mut state);
    let patches = state.engine.diagnostics();
    let incomplete = state
        .entries
        .values()
        .filter(|entry| {
            entry.enabled
                && (entry.patches.is_empty()
                    || !entry.patches.iter().all(|id| {
                        patches.iter().any(|patch| {
                            &patch.id == id
                                && patch.owner == entry.id
                                && patch.state == PatchState::Installed
                        })
                    }))
        })
        .count();
    restore(state)?;
    if incomplete == 0 {
        Ok(())
    } else {
        Err(format!(
            "{incomplete} approved native extensions remain rejected or unprepared. Inspect patch diagnostics before retrying."
        ))
    }
}

pub fn diagnostics() -> Json {
    let Ok(state) = manager().lock() else {
        return json!({"error":"Native extension manager is poisoned"});
    };
    if state.busy {
        return json!({"state":"busy"});
    }
    let patches = state.engine.diagnostics();
    let entries: Vec<_> = state.entries.values().map(|entry| {
        let installed = entry.enabled && !entry.patches.is_empty() && entry.patches.iter().all(|id| patches.iter().any(|patch| &patch.id == id && patch.owner == entry.id && patch.state == PatchState::Installed));
        json!({"id":entry.id,"sha256":entry.sha256,"manifestSha256":entry.manifest_sha256,"provenance":entry.provenance,"enabled":entry.enabled,"state":if installed {"installed"} else if entry.error.is_some() {"rejected"} else if entry.enabled {"pending"} else {"disabled"},"error":entry.error,"patchIds":entry.patches})
    }).collect();
    let layout: Json = serde_json::from_str(include_str!("../layouts.json"))
        .unwrap_or_else(|error| json!({"error":error.to_string()}));
    let targets = |class: &str| {
        layout
            .get("targets")
            .and_then(Json::as_array)
            .map(|targets| {
                targets
                    .iter()
                    .filter(|target| target["class"] == class)
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    };
    let status = |installed| if installed { "installed" } else { "inactive" };
    let runtime_hooks = json!([
        {"id":"luabuddy.callable-bridge","owner":"LuaBuddy","state":status(crate::bridge::ready()),"mechanism":"native declaration and RegisterNatives","disable":"restart required","target":{"class":"zombie/Lua/LuaEventManager","method":"call","descriptor":"(Lse/krka/kahlua/vm/LuaCallFrame;I)I","loader":"system","originalCode":"03ac"}},
        {"id":"luabuddy.lifecycle","owner":"LuaBuddy","state":status(crate::events::installed()),"mechanism":"native bridge around unchanged original dispatcher","disable":"restart required","targets":targets("zombie/Lua/Event")},
        {"id":"luabuddy.workshop-download-observer","owner":"LuaBuddy","state":status(crate::download_events::installed()),"mechanism":"entry breakpoints","targets":targets("zombie/core/znet/SteamWorkshop")}
    ]);
    json!({"schemaVersion":1,"store":state.root,"storeError":state.store_error,"capabilities":state.engine.capabilities(),"runtimeHooks":runtime_hooks,"extensions":entries,"patches":patches,"callbackFailures":state.engine.callback_failures().unwrap_or_else(|error| vec![("diagnostics".into(),error.to_string())]),"settingsApply":"next JVM process"})
}

pub fn publish(context: &mut Context<'_, '_>, state: &Value) -> Result<()> {
    let value = lua_value(context, diagnostics())?;
    context.set(state, "patches", &value)
}

fn lua_value(context: &mut Context<'_, '_>, json: Json) -> Result<Value> {
    context.jni(|env| env.push_local_frame(64))?;
    let result = lua_value_inner(context, json);
    let popped = context.jni(|env| {
        // SAFETY: The matching frame was pushed above. Every returned Value owns
        // a GlobalRef or null, so no callback-local handle escapes this frame.
        unsafe { env.pop_local_frame(&jni::objects::JObject::null()) }.map(|_| ())
    });
    match result {
        Ok(value) => {
            popped?;
            Ok(value)
        }
        Err(error) => Err(error),
    }
}

fn lua_value_inner(context: &mut Context<'_, '_>, json: Json) -> Result<Value> {
    match json {
        Json::Null => Ok(Value::default()),
        Json::Bool(value) => context.boolean(value),
        Json::Number(value) => context.number(
            value
                .as_f64()
                .ok_or("Diagnostic number exceeds native Lua range")?,
        ),
        Json::String(value) => context.string(&value),
        Json::Object(values) => {
            let table = context.table()?;
            for (key, value) in values {
                let value = lua_value(context, value)?;
                context.set(&table, &key, &value)?;
            }
            Ok(table)
        }
        Json::Array(values) => {
            let table = context.table()?;
            for (index, value) in values.into_iter().enumerate() {
                let value = lua_value(context, value)?;
                context.jni(|env| {
                    env.call_method(
                        table.object(),
                        "rawset",
                        "(ILjava/lang/Object;)V",
                        &[
                            jni::objects::JValue::Int(index as i32 + 1),
                            jni::objects::JValue::Object(value.object()),
                        ],
                    )
                })?;
            }
            Ok(table)
        }
    }
}
