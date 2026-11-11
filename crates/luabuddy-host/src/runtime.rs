use crate::{
    callables, game, hooks,
    lua::{Context, Result, Value},
};
use jni::{JNIEnv, objects::JObject};
use luabuddy_core::diagnostics::{ModDiagnostic, ModStatus};
use luabuddy_core::lifecycle::{
    Candidate, CleanupDecision, CleanupOutcome, Operation, RequestContext, Session as Policy,
};
use luabuddy_core::source::{ModuleKey, ModuleRegistry, Owner, Require};
use luabuddy_core::{ModId, ReplayEvent, SkipReason, read_profiles, resolve};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

struct Session {
    id: u64,
    environment: Value,
    state: Value,
    base_require: Value,
    policy: Policy,
    modules: ModuleRegistry<Value>,
    chunks: BTreeMap<(Owner, ModuleKey), Value>,
    owner_thread: std::thread::ThreadId,
    statuses: BTreeMap<ModId, String>,
    details: BTreeMap<ModId, ModDiagnostic>,
    startup_attempted: bool,
    start_depth: usize,
    debug: bool,
    cleanup: BTreeMap<Owner, CooperativeCleanup>,
    favorites_requested: Option<Owner>,
    favorites: Option<(Owner, Value, Vec<String>)>,
}

#[derive(Clone)]
struct CooperativeCleanup {
    function: Value,
    context: Value,
}

static SESSION: OnceLock<Mutex<Option<Session>>> = OnceLock::new();
static NEXT: AtomicU64 = AtomicU64::new(1);

fn with<T>(id: u64, action: impl FnOnce(&mut Session) -> Result<T>) -> Result<T> {
    let mut guard = SESSION
        .get_or_init(|| Mutex::new(None))
        .lock()
        .map_err(|_| "LuaBuddy session lock is poisoned")?;
    let session = guard
        .as_mut()
        .filter(|session| session.id == id)
        .ok_or("This LuaBuddy session is no longer active")?;
    action(session)
}

fn id() -> Option<u64> {
    SESSION
        .get_or_init(|| Mutex::new(None))
        .lock()
        .ok()?
        .as_ref()
        .map(|s| s.id)
}

pub fn event(env: &mut JNIEnv<'_>, name: &str, enter: bool) -> Result<()> {
    if !matches!(
        name,
        "OnMainMenuEnter" | "OnGameStart" | "OnCreateUI" | "OnTick"
    ) {
        return Ok(());
    }
    if name == "OnTick"
        && (enter || id().and_then(|id| with(id, |s| Ok(s.favorites.is_some())).ok()) != Some(true))
    {
        return Ok(());
    }
    let mut context = Context::current(env)?;
    let mut initialized_start = false;
    if context.jni(|env| {
        env.get_static_field("zombie/network/GameServer", "server", "Z")?
            .z()
    })? {
        return Ok(());
    }
    if enter
        && (name == "OnMainMenuEnter"
            || name == "OnGameStart"
            || name == "OnCreateUI"
                && game::property(&mut context, "luabuddy.earlyUI")?.as_deref() == Some("true"))
    {
        let load = name != "OnMainMenuEnter" && game::client(&mut context)?;
        let existing = id().and_then(|id| with(id, |s| Ok(s.environment.clone())).ok());
        let same = existing
            .as_ref()
            .map(|value| context.same(value, &context.environment.clone()))
            .transpose()?
            .unwrap_or(false);
        if !same {
            start(&mut context, load, name == "OnGameStart")?;
            initialized_start = name == "OnGameStart";
        } else if load {
            let session_id = id().ok_or("Missing session")?;
            let attempted = with(session_id, |s| {
                let previous = s.startup_attempted;
                s.startup_attempted = true;
                Ok(previous)
            })?;
            if !attempted {
                if name == "OnGameStart" {
                    with(session_id, |s| {
                        s.start_depth = s
                            .start_depth
                            .checked_add(1)
                            .ok_or("Startup dispatch nesting exhausted")?;
                        s.policy.set_ready(false).map_err(|e| e.to_string())
                    })?;
                    initialized_start = true;
                }
                if let Err(error) = apply(&mut context, session_id, Operation::Startup) {
                    game::log(&error);
                }
            }
        }
    }
    if let Some(id) = id() {
        let environment = with(id, |s| Ok(s.environment.clone()))?;
        if !context.same(&environment, &context.environment.clone())? {
            return Ok(());
        }
        if name == "OnGameStart" {
            let ready = with(id, |s| {
                if s.owner_thread != std::thread::current().id() {
                    return Err("Startup dispatch arrived on a foreign Lua thread".into());
                }
                if !enter {
                    s.start_depth = s
                        .start_depth
                        .checked_sub(1)
                        .ok_or("Startup dispatch completed without a matching entry")?;
                } else if !initialized_start {
                    s.start_depth = s
                        .start_depth
                        .checked_add(1)
                        .ok_or("Startup dispatch nesting exhausted")?;
                }
                let ready = s.start_depth == 0;
                s.policy.set_ready(ready).map_err(|e| e.to_string())?;
                Ok(ready)
            })?;
            let state = with(id, |s| Ok(s.state.clone()))?;
            let value = context.boolean(ready)?;
            context.set(&state, "ready", &value)?;
            if !enter && ready {
                snapshot_favorites(&mut context, id)?;
            }
        }
        if name == "OnTick" && !enter {
            restore_favorites(&mut context, id)?;
        }
    }
    Ok(())
}

fn start(context: &mut Context<'_, '_>, load: bool, start_entered: bool) -> Result<()> {
    if !context.global("LuaBuddy")?.is_nil() {
        return Err(
            "The LuaBuddy namespace is already owned by another runtime in this environment".into(),
        );
    }
    if let Err(error) = crate::selections::initialize(context) {
        game::log(&format!("One-launch selections unavailable: {error}"));
    }
    let session_id = NEXT.fetch_add(1, Ordering::Relaxed);
    let base_require = context.global("require")?;
    let state = context.table()?;
    let statuses = context.table()?;
    context.set(&state, "mods", &statuses)?;
    let ready = context.boolean(false)?;
    context.set(&state, "ready", &ready)?;
    let debug = game::property(context, "luabuddy.debug")?.as_deref() == Some("true");
    let debug_value = context.boolean(debug)?;
    context.set(&state, "debug", &debug_value)?;
    let active = game::active(context)?;
    let session = Session {
        id: session_id,
        environment: context.environment.clone(),
        state: state.clone(),
        base_require,
        policy: Policy::new(session_id, active),
        modules: ModuleRegistry::default(),
        chunks: BTreeMap::new(),
        owner_thread: std::thread::current().id(),
        statuses: BTreeMap::new(),
        details: BTreeMap::new(),
        startup_attempted: load,
        start_depth: usize::from(start_entered),
        debug,
        cleanup: BTreeMap::new(),
        favorites_requested: None,
        favorites: None,
    };
    *SESSION
        .get_or_init(|| Mutex::new(None))
        .lock()
        .map_err(|_| "LuaBuddy session lock is poisoned")? = Some(session);
    let require = Value(Some(callables::create(
        context.env,
        "LuaBuddy",
        session_id,
        session_id,
        require_callback,
    )?));
    let refresh = Value(Some(callables::create(
        context.env,
        "LuaBuddy",
        session_id,
        session_id,
        refresh_callback,
    )?));
    let reload = Value(Some(callables::create(
        context.env,
        "LuaBuddy",
        session_id,
        session_id,
        reload_callback,
    )?));
    let retry_patches = Value(Some(callables::create(
        context.env,
        "LuaBuddy",
        session_id,
        session_id,
        retry_patches_callback,
    )?));
    context.set(&state, "refresh", &refresh)?;
    context.set(&state, "reload", &reload)?;
    context.set(&state, "retryPatches", &retry_patches)?;
    let environment = context.environment.clone();
    context.set(&environment, "LuaBuddy", &state)?;
    context.set(&environment, "require", &require)?;
    if let Err(error) = crate::extensions::initialize(context) {
        game::log(&format!("Approved extension initialization: {error}"));
    }
    if let Err(error) = crate::extensions::publish(context, &state) {
        game::log(&format!("Patch diagnostics unavailable: {error}"));
    }
    if let Err(error) = crate::ui::install(context, session_id) {
        game::log(&format!("Runtime controls unavailable: {error}"));
    }
    if load {
        let result = apply(context, session_id, Operation::Startup);
        if let Err(error) = result {
            game::log(&format!("startup failed: {error}"));
        }
    }
    Ok(())
}

enum RequireStep {
    Delegate(Value),
    Cached(Value),
    Execute(Owner, ModuleKey, Value),
}

fn require_value(context: &mut Context<'_, '_>, session_id: u64, name: &str) -> Result<Value> {
    let step = with(session_id, |session| {
        if session.owner_thread != std::thread::current().id() {
            return Err("Run LuaBuddy imports on the owning game thread".into());
        }
        match session.modules.require(name).map_err(|e| e.to_string())? {
            Require::Delegate => Ok(RequireStep::Delegate(session.base_require.clone())),
            Require::Cached(value) => Ok(RequireStep::Cached(value.cloned().unwrap_or_default())),
            Require::Execute(key) => {
                let owner = session
                    .modules
                    .get(&key)
                    .ok_or("Missing source owner")?
                    .owner
                    .clone();
                let chunk = session
                    .chunks
                    .get(&(owner.clone(), key.clone()))
                    .cloned()
                    .ok_or_else(|| format!("Compiled source is unavailable: {}", key.as_str()))?;
                session.modules.begin(&key).map_err(|e| e.to_string())?;
                Ok(RequireStep::Execute(owner, key, chunk))
            }
        }
    })?;
    execute_step(context, session_id, name, step)
}

fn execute_owned(
    context: &mut Context<'_, '_>,
    session_id: u64,
    owner: &Owner,
    name: &str,
) -> Result<Value> {
    let key = ModuleKey::new(name).map_err(|e| e.to_string())?;
    let step = with(session_id, |session| {
        match session
            .modules
            .require_owned(owner, &key)
            .map_err(|e| e.to_string())?
        {
            Require::Cached(value) => Ok(RequireStep::Cached(value.cloned().unwrap_or_default())),
            Require::Execute(key) => {
                let chunk = session
                    .chunks
                    .get(&(owner.clone(), key.clone()))
                    .cloned()
                    .ok_or("Missing compiled owned source")?;
                session
                    .modules
                    .begin_owned(owner, &key)
                    .map_err(|e| e.to_string())?;
                Ok(RequireStep::Execute(owner.clone(), key, chunk))
            }
            Require::Delegate => Err("Unknown owned module cannot delegate".into()),
        }
    })?;
    execute_step(context, session_id, name, step)
}

fn execute_step(
    context: &mut Context<'_, '_>,
    session_id: u64,
    name: &str,
    step: RequireStep,
) -> Result<Value> {
    match step {
        RequireStep::Cached(value) => Ok(value),
        RequireStep::Delegate(function) => {
            let name = context.string(name)?;
            hooks::first(context.call(&function, &[name])?)
        }
        RequireStep::Execute(owner, key, chunk) => {
            let result = context.call(&chunk, &[]).map(|mut values| {
                if values.is_empty() {
                    Value::default()
                } else {
                    values.remove(0)
                }
            });
            with(session_id, |session| {
                session.chunks.remove(&(owner.clone(), key.clone()));
                session
                    .modules
                    .finish_owned(
                        &owner,
                        &key,
                        result
                            .clone()
                            .map(|value| if value.is_nil() { None } else { Some(value) }),
                    )
                    .map_err(|e| e.to_string())
            })?;
            result
        }
    }
}

fn validate_environment(context: &mut Context<'_, '_>, session_id: u64) -> Result<()> {
    let environment = with(session_id, |s| Ok(s.environment.clone()))?;
    if !context.same(&environment, &context.environment.clone())? {
        return Err("This LuaBuddy session is no longer active".into());
    }
    Ok(())
}

pub(crate) fn validate_session(context: &mut Context<'_, '_>, session_id: u64) -> Result<()> {
    let state = with(session_id, |s| {
        if s.owner_thread != std::thread::current().id() {
            return Err("Run LuaBuddy operations on the owning game thread".into());
        }
        Ok(s.state.clone())
    })?;
    validate_environment(context, session_id)?;
    let current = context.global("LuaBuddy")?;
    if !context.same(&state, &current)? {
        return Err("This LuaBuddy session is no longer active".into());
    }
    Ok(())
}

fn require_callback<'vm>(
    env: &mut JNIEnv<'vm>,
    frame: &JObject<'vm>,
    count: i32,
    session_id: u64,
) -> Result<i32> {
    let mut context = Context::current(env)?;
    validate_environment(&mut context, session_id)?;
    let args = context.frame_arguments(frame, count)?;
    if args.len() != 1 {
        return Err("require expects one module name".into());
    }
    let name = context.text(&args[0])?;
    let value = require_value(&mut context, session_id, &name)?;
    context.frame_return(frame, &[value])
}

fn refresh_callback<'vm>(
    env: &mut JNIEnv<'vm>,
    frame: &JObject<'vm>,
    count: i32,
    session_id: u64,
) -> Result<i32> {
    operation_callback(env, frame, count, session_id, false)
}
fn reload_callback<'vm>(
    env: &mut JNIEnv<'vm>,
    frame: &JObject<'vm>,
    count: i32,
    session_id: u64,
) -> Result<i32> {
    operation_callback(env, frame, count, session_id, true)
}

fn retry_patches_callback<'vm>(
    env: &mut JNIEnv<'vm>,
    frame: &JObject<'vm>,
    count: i32,
    session_id: u64,
) -> Result<i32> {
    let mut context = Context::current(env)?;
    validate_session(&mut context, session_id)?;
    let result = (|| {
        if count != 0 {
            return Err("retryPatches expects no arguments".to_owned());
        }
        with(session_id, |s| {
            if s.owner_thread != std::thread::current().id() {
                return Err("Apply patches on the owning game thread".into());
            }
            if s.policy.is_busy() {
                return Err(
                    "Wait for the current LuaBuddy operation before applying approved patches"
                        .into(),
                );
            }
            Ok(())
        })?;
        if crate::ui::pending_options(&mut context)? {
            return Err(
                "Apply or discard pending Options changes before applying approved patches".into(),
            );
        }
        crate::extensions::retry(&mut context)?;
        let state = with(session_id, |s| Ok(s.state.clone()))?;
        crate::extensions::publish(&mut context, &state)?;
        serde_json::to_string_pretty(&crate::extensions::diagnostics())
            .map_err(|error| error.to_string())
    })();
    let success = context.boolean(result.is_ok())?;
    let message = context.string(&result.unwrap_or_else(|error| error))?;
    context.frame_return(frame, &[success, message])
}

fn operation_callback<'vm>(
    env: &mut JNIEnv<'vm>,
    frame: &JObject<'vm>,
    count: i32,
    session_id: u64,
    reload: bool,
) -> Result<i32> {
    let mut context = Context::current(env)?;
    let result = (|| {
        validate_environment(&mut context, session_id)?;
        let args = context.frame_arguments(frame, count)?;
        let operation = if reload {
            if args.len() != 1 {
                return Err("Provide one configured mod ID to reload".into());
            }
            Operation::Reload(ModId::new(context.text(&args[0])?).map_err(|e| e.to_string())?)
        } else {
            Operation::Refresh
        };
        apply(&mut context, session_id, operation)
    })();
    let ok = context.boolean(result.is_ok())?;
    let message = context.string(&result.unwrap_or_else(|e| e))?;
    context.frame_return(frame, &[ok, message])
}

pub fn apply(
    context: &mut Context<'_, '_>,
    session_id: u64,
    operation: Operation,
) -> Result<String> {
    validate_session(context, session_id)?;
    let pending_options = crate::ui::pending_options(context)?;
    let multiplayer_client = game::client(context)?;
    if !multiplayer_client {
        return Err("Live client/shared loading becomes available after joining a multiplayer session. Use the game's native Mods screen for normal startup loading in other contexts.".into());
    }
    let token = with(session_id, |s| {
        s.policy
            .begin(
                RequestContext {
                    environment: session_id,
                    multiplayer_client,
                    pending_options,
                },
                operation.clone(),
            )
            .map_err(|e| e.to_string())
    })?;
    let directory = PathBuf::from(game::property(context, "luabuddy.config")?.unwrap_or_default());
    let previous = with(session_id, |s| Ok(s.details.clone()))?;
    let retained = if let Operation::Reload(target) = &operation {
        with(session_id, |s| {
            Ok(s.policy
                .running()
                .get(target)
                .map(|running| (running.owner.clone(), previous.get(target).cloned())))
        })?
    } else {
        None
    };
    let mut report = match crate::report::OperationReport::begin(
        context, session_id, &operation, directory, previous,
    ) {
        Ok(report) => Some(report),
        Err(error) => {
            game::log(&format!("Diagnostic report preparation failed: {error}"));
            None
        }
    };
    let result = apply_inner(context, session_id, &token, &operation);
    if result.is_err()
        && let Some((owner, Some(detail))) = retained
    {
        let restored = with(session_id, |s| {
            if s.policy.running().get(&owner.id).is_some_and(|running| {
                running.owner == owner
                    && running.state == luabuddy_core::lifecycle::GenerationState::Loaded
            }) {
                s.details.insert(owner.id.clone(), detail);
                s.statuses.insert(owner.id.clone(), "loaded".to_owned());
                return Ok(Some((s.state.clone(), owner.id)));
            }
            Ok(None)
        })?;
        if let Some((state, target)) = restored {
            let mods = context.get(&state, "mods")?;
            let loaded = context.string("loaded")?;
            context.set(&mods, target.as_str(), &loaded)?;
        }
    }
    if let Some(mut report) = report.take() {
        for (id, detail) in with(session_id, |s| Ok(s.details.clone()))? {
            report.record(id, detail);
        }
        if let Err(error) = report.finish(context, result.as_ref().err().map(String::as_str)) {
            game::log(&format!("Diagnostic report write failed: {error}"));
        }
    }
    let state = with(session_id, |s| Ok(s.state.clone()))?;
    if let Err(error) = crate::extensions::publish(context, &state) {
        game::log(&format!("Patch diagnostics update failed: {error}"));
    }
    let finish = with(session_id, |s| {
        s.policy.end(&token).map_err(|e| e.to_string())
    });
    result.and_then(|value| {
        finish?;
        Ok(value)
    })
}

fn status(context: &mut Context<'_, '_>, session_id: u64, id: &ModId, value: &str) -> Result<()> {
    let state = with(session_id, |session| {
        session.statuses.insert(id.clone(), value.to_owned());
        if let Some(detail) = session.details.get_mut(id) {
            detail.status = match value {
                "loaded" => ModStatus::Loaded,
                "disabled" => ModStatus::Disabled,
                "server-loaded" => ModStatus::ServerLoaded,
                _ if value.starts_with("restart-required:") => ModStatus::RestartRequired,
                _ => ModStatus::Failed,
            };
            if value == "loaded" {
                detail.stage = "complete".into();
            }
            if matches!(value, "loaded" | "disabled" | "server-loaded") {
                detail.error = None;
                detail.traceback = None;
                detail.java_traceback = None;
                if value != "loaded" {
                    detail.stage = value.to_owned();
                }
            }
            if let Some(error) = value.strip_prefix("failed: ") {
                detail.error = Some(error.to_owned());
                if let Some((_, trace)) = error.split_once("\nLua traceback:\n") {
                    detail.traceback = Some(
                        trace
                            .split("\nJava traceback:\n")
                            .next()
                            .unwrap_or(trace)
                            .to_owned(),
                    );
                }
                if let Some((_, trace)) = error.split_once("\nJava traceback:\n") {
                    detail.java_traceback = Some(trace.to_owned());
                }
            }
            if let Some(reason) = value.strip_prefix("restart-required: ") {
                detail.error = Some(reason.to_owned());
            }
        }
        Ok(session.state.clone())
    })?;
    let statuses = context.get(&state, "mods")?;
    let text = context.string(value)?;
    context.set(&statuses, id.as_str(), &text)
}

fn stage(session_id: u64, id: &ModId, label: &str, source: Option<&std::path::Path>) -> Result<()> {
    let debug = with(session_id, |s| {
        if let Some(detail) = s.details.get_mut(id) {
            detail.stage = label.to_owned();
            detail.source = source.map(std::path::Path::to_owned);
        }
        Ok(s.debug)
    })?;
    if debug {
        game::log(&format!(
            "{id} stage={label}{}",
            source
                .map(|path| format!(" source={}", path.display()))
                .unwrap_or_default()
        ));
    }
    Ok(())
}

fn apply_inner(
    context: &mut Context<'_, '_>,
    session_id: u64,
    token: &luabuddy_core::lifecycle::OperationToken,
    operation: &Operation,
) -> Result<String> {
    let mut profiles = crate::selections::configured(context)?;
    with(session_id, |session| {
        for profile in &profiles {
            session
                .details
                .entry(profile.id.clone())
                .or_insert_with(|| ModDiagnostic {
                    status: ModStatus::Prepared,
                    stage: "discovery".into(),
                    profile: profile.file.clone(),
                    source: None,
                    error: None,
                    traceback: None,
                    java_traceback: None,
                    source_count: 0,
                    generation: 0,
                    configuration: luabuddy_core::diagnostics::ProfileDiagnostic {
                        roots: Vec::new(),
                        requires: profile.requires.clone(),
                        before: profile.before.clone(),
                        after: profile.after.clone(),
                        preload: profile.preload.clone(),
                        replay: profile.replay.iter().copied().collect(),
                        hook: profile.hook.clone(),
                        cleanup: profile.cleanup.clone(),
                        live_policy: profile.live,
                        activation_limit: None,
                    },
                });
        }
        Ok(())
    })?;
    let templates = match game::property(context, "luabuddy.templates")? {
        Some(path) => read_profiles(std::path::Path::new(&path))
            .map_err(|error| format!("Reading immutable profile catalog: {error}"))?,
        None => Vec::new(),
    };
    if *operation != Operation::Startup {
        game::reset_discovery(context)?;
    }
    let active = game::active(context)?;
    let existing = with(session_id, |s| Ok(s.statuses.clone()))?;
    profiles.retain(|p| {
        existing
            .get(&p.id)
            .is_none_or(|status| !status.starts_with("failed:"))
    });
    let mut available = active.clone();
    available.extend(
        existing
            .iter()
            .filter(|(_, status)| status.as_str() == "loaded")
            .map(|(id, _)| id.clone()),
    );
    if let Operation::Reload(target) = operation {
        if active.contains(target) || existing.get(target).is_none_or(|status| status != "loaded") {
            return Err(format!(
                "Reload requires a locally loaded profile: {target}"
            ));
        }
        available.remove(target);
    }
    let discovered = game::discover(context, &profiles, &active)?;
    let resolution = resolve(&profiles, &discovered.metadata, &available);
    let mut failures = Vec::new();
    for (id, reason) in &resolution.skipped {
        if existing.get(id).is_none_or(|value| value != "loaded") {
            status(
                context,
                session_id,
                id,
                match reason {
                    SkipReason::Disabled => "disabled",
                    SkipReason::NativeLoaded => "server-loaded",
                },
            )?;
        }
    }
    for (id, error) in &resolution.failures {
        stage(session_id, id, &error.stage, error.path.as_deref())?;
        let error = discovered
            .failures
            .get(id)
            .cloned()
            .unwrap_or_else(|| error.to_string());
        status(context, session_id, id, &format!("failed: {error}"))?;
        game::log(&format!("{id} failed before activation: {error}"));
        failures.push(id.clone());
    }
    let mut candidates = Vec::new();
    for profile in resolution.ordered {
        if let Operation::Reload(target) = operation
            && profile.profile.id != *target
        {
            continue;
        }
        with(session_id, |session| {
            if let Some(detail) = session.details.get_mut(&profile.profile.id) {
                detail.profile = profile.profile.file.clone();
                detail.configuration = luabuddy_core::diagnostics::ProfileDiagnostic {
                    roots: profile.roots.clone(),
                    requires: profile.requires.iter().cloned().collect(),
                    before: profile.before.iter().cloned().collect(),
                    after: profile.after.iter().cloned().collect(),
                    preload: profile.profile.preload.clone(),
                    replay: profile.profile.replay.iter().copied().collect(),
                    hook: profile.profile.hook.clone(),
                    cleanup: profile.profile.cleanup.clone(),
                    live_policy: profile.profile.live,
                    activation_limit: None,
                };
            }
            Ok(())
        })?;
        stage(session_id, &profile.profile.id, "eligibility", None)?;
        let eligibility =
            match crate::activation::assess(&profile.profile, &profile.roots, &templates) {
                Ok(eligibility) => eligibility,
                Err(error) => {
                    if matches!(operation, Operation::Reload(_)) {
                        return Err(error);
                    }
                    status(
                        context,
                        session_id,
                        &profile.profile.id,
                        &format!("failed: {error}"),
                    )?;
                    failures.push(profile.profile.id.clone());
                    continue;
                }
            };
        if let crate::activation::Eligibility::RestartRequired { reason } = &eligibility {
            status(
                context,
                session_id,
                &profile.profile.id,
                &format!("restart-required: {reason}"),
            )?;
            failures.push(profile.profile.id.clone());
            continue;
        }
        if let crate::activation::Eligibility::Live { limitations } = &eligibility
            && !limitations.is_empty()
        {
            let explanation = eligibility.explanation();
            with(session_id, |s| {
                if let Some(detail) = s.details.get_mut(&profile.profile.id) {
                    detail.configuration.activation_limit = Some(explanation.clone());
                }
                Ok(())
            })?;
            game::log(&format!("{}: {explanation}", profile.profile.id));
        }
        let prepare = (|| {
            if let Some(hook) = &profile.profile.hook
                && !hooks::IDS.contains(&hook.as_str())
            {
                return Err(format!("Rust profile hook {hook} is not installed"));
            }
            let mut sources =
                luabuddy_core::source::scan_sources(&profile.profile, &profile.roots, &active)
                    .map_err(|e| e.to_string())?;
            with(session_id, |session| {
                if let Some(detail) = session.details.get_mut(&profile.profile.id) {
                    detail.source_count = sources.len();
                }
                Ok(())
            })?;
            let mut compiled = Vec::new();
            for source in &sources {
                let filename = source.filename.to_string_lossy().replace('\\', "/");
                stage(
                    session_id,
                    &profile.profile.id,
                    "compile",
                    Some(&source.filename),
                )?;
                compiled.push((
                    ModuleKey::new(&source.name).map_err(|e| e.to_string())?,
                    context.compile(&source.text, &filename)?,
                ));
            }
            for source in &mut sources {
                source.text.clear();
                source.text.shrink_to_fit();
            }
            let cleanup_supported = profile.profile.cleanup.is_some();
            Ok(Candidate {
                profile: profile.clone(),
                sources,
                compiled,
                cleanup_supported,
            })
        })();
        match prepare {
            Ok(candidate) => candidates.push(candidate),
            Err(error) => {
                if matches!(operation, Operation::Reload(_)) {
                    return Err(error);
                }
                status(
                    context,
                    session_id,
                    &profile.profile.id,
                    &format!("failed: {error}"),
                )?;
                failures.push(profile.profile.id);
            }
        }
    }
    loop {
        let unavailable: BTreeSet<_> = failures.iter().cloned().collect();
        let rejected: Vec<_> = candidates
            .iter()
            .filter(|candidate| {
                candidate
                    .profile
                    .requires
                    .iter()
                    .any(|id| unavailable.contains(id))
            })
            .map(|candidate| candidate.profile.profile.id.clone())
            .collect();
        if rejected.is_empty() {
            break;
        }
        for id in &rejected {
            status(
                context,
                session_id,
                id,
                "failed: Required profile failed preparation",
            )?;
        }
        failures.extend(rejected.iter().cloned());
        candidates.retain(|candidate| !rejected.contains(&candidate.profile.profile.id));
    }
    let prepared = if matches!(operation, Operation::Reload(_)) {
        with(session_id, |s| {
            s.policy
                .prepare(token, candidates)
                .map_err(|e| e.to_string())
        })?
    } else {
        let mut accepted: Vec<Candidate<Vec<(ModuleKey, Value)>>> = Vec::new();
        for candidate in candidates {
            let id = candidate.profile.profile.id.clone();
            let mut trial = accepted.clone();
            trial.push(candidate.clone());
            let result = with(session_id, |s| {
                s.policy
                    .clone()
                    .prepare(token, trial)
                    .map(|_| ())
                    .map_err(|e| e.to_string())
            });
            match result {
                Ok(()) => accepted.push(candidate),
                Err(error) => {
                    status(context, session_id, &id, &format!("failed: {error}"))?;
                    failures.push(id);
                }
            }
        }
        with(session_id, |s| {
            s.policy.prepare(token, accepted).map_err(|e| e.to_string())
        })?
    };
    let mut permits = Vec::new();
    for prepared in prepared {
        let id = prepared.id().clone();
        let previous = prepared.replaces().cloned();
        let outcome = if let Some(owner) = &previous {
            let cleanup = with(session_id, |s| {
                s.cleanup.get(owner).cloned().ok_or_else(|| {
                    "The running generation has no captured upstream cleanup callable".to_owned()
                })
            })?;
            match context.call(&cleanup.function, &[cleanup.context]) {
                Ok(values) => {
                    let value = values.first().cloned().unwrap_or_default();
                    let accepted = exact_boolean(context, &value)?;
                    match accepted {
                        Some(true) => CleanupOutcome::Accepted,
                        Some(false) => CleanupOutcome::Refused(match values.get(1) {
                            Some(reason) => context.text(reason)?,
                            None => "Finish the current action first".into(),
                        }),
                        None => CleanupOutcome::Uncertain(
                            "Upstream cleanup must return exactly true after completing cleanup"
                                .into(),
                        ),
                    }
                }
                Err(error) => CleanupOutcome::Uncertain(error),
            }
        } else {
            CleanupOutcome::Accepted
        };
        let decision = with(session_id, |s| {
            s.policy
                .after_cleanup(prepared, outcome)
                .map_err(|e| e.to_string())
        })?;
        match decision {
            CleanupDecision::Activate(permit) => {
                if let Some(owner) = previous {
                    let files = with(session_id, |s| {
                        Ok(s.policy
                            .running()
                            .get(&owner.id)
                            .ok_or("Missing running generation")?
                            .sources
                            .iter()
                            .map(|source| source.filename.to_string_lossy().replace('\\', "/"))
                            .collect())
                    })?;
                    if let Err(error) = game::remove_callbacks(context, &files) {
                        with(session_id, |s| {
                            s.modules.fail_owner(&owner, &error);
                            s.policy
                                .finish_activation(permit, Err(error.clone()))
                                .map_err(|e| e.to_string())?;
                            Ok(())
                        })?;
                        status(
                            context,
                            session_id,
                            &id,
                            &format!("failed: Callback cleanup is uncertain: {error}; reconnect"),
                        )?;
                        return Err(error);
                    }
                    with(session_id, |s| {
                        s.modules.remove_owner(&owner);
                        s.chunks.retain(|(previous, _), _| previous != &owner);
                        s.cleanup.remove(&owner);
                        cancel_favorites(s, &owner);
                        Ok(())
                    })?;
                }
                permits.push(permit);
            }
            CleanupDecision::Retained(error) => return Err(format!("{id}: {error}")),
            CleanupDecision::Quarantined(error) => {
                if let Some(owner) = previous {
                    with(session_id, |s| {
                        s.modules.fail_owner(&owner, &error.to_string());
                        cancel_favorites(s, &owner);
                        Ok(())
                    })?;
                }
                status(context, session_id, &id, &format!("failed: {error}"))?;
                return Err(format!("{id}: {error}"));
            }
        }
    }
    let state = with(session_id, |s| Ok(s.state.clone()))?;
    let active_table = context.table()?;
    for id in &available {
        let yes = context.boolean(true)?;
        context.set(&active_table, id.as_str(), &yes)?;
    }
    let mut contexts = BTreeMap::new();
    for permit in &permits {
        let owner = permit.owner().clone();
        let sources = permit.candidate().sources.clone();
        let compiled = permit.candidate().compiled.clone();
        with(session_id, |s| {
            if let Some(detail) = s.details.get_mut(&owner.id) {
                detail.source_count = sources.len();
                detail.generation = owner.generation.0;
            }
            s.modules
                .register(owner.clone(), sources)
                .map_err(|e| e.to_string())?;
            s.chunks.extend(
                compiled
                    .into_iter()
                    .map(|(key, chunk)| ((owner.clone(), key), chunk)),
            );
            Ok(())
        })?;
        let activation = context.table()?;
        let id_value = context.string(owner.id.as_str())?;
        let runtime = context.boolean(*operation != Operation::Startup)?;
        let operation_name = context.string(match operation {
            Operation::Startup => "startup",
            Operation::Refresh => "refresh",
            Operation::Reload(_) => "reload",
        })?;
        for (key, value) in [
            ("id", id_value),
            ("runtime", runtime),
            ("operation", operation_name),
            ("state", state.clone()),
            ("active", active_table.clone()),
        ] {
            context.set(&activation, key, &value)?;
        }
        contexts.insert(owner, activation);
    }
    let mut outcomes: BTreeMap<ModId, Result<()>> = BTreeMap::new();
    for phase in ["shared/", "client/"] {
        for permit in &permits {
            let id = &permit.owner().id;
            if outcomes.get(id).is_some_and(|result| result.is_err()) {
                continue;
            }
            let candidate = permit.candidate();
            let result = (|| {
                for required in &candidate.profile.requires {
                    if failures.contains(required)
                        || outcomes.get(required).is_some_and(|result| result.is_err())
                    {
                        return Err(format!("Required profile {required} failed"));
                    }
                }
                if phase == "shared/" {
                    stage(session_id, id, "preload", None)?;
                    for module in &candidate.profile.profile.preload {
                        require_value(context, session_id, module)?;
                    }
                    stage(session_id, id, "translations", None)?;
                    game::translations(context, &candidate.profile.roots)?;
                    if candidate.profile.profile.mount {
                        stage(session_id, id, "file-mount", None)?;
                        game::mount(
                            context,
                            id,
                            discovered
                                .infos
                                .get(id)
                                .ok_or("Missing native mod metadata")?,
                        )?;
                    }
                }
                for source in &candidate.sources {
                    if source.name.to_lowercase().starts_with(phase) {
                        stage(
                            session_id,
                            id,
                            phase.trim_end_matches('/'),
                            Some(&source.filename),
                        )?;
                        execute_owned(context, session_id, permit.owner(), &source.name)?;
                    }
                }
                if phase == "client/" {
                    if candidate
                        .profile
                        .profile
                        .replay
                        .contains(&ReplayEvent::OnGameBoot)
                    {
                        replay(context, "OnGameBoot", &candidate.sources)?;
                    }
                    if let Some(hook) = &candidate.profile.profile.hook {
                        stage(session_id, id, "profile-after", None)?;
                        hooks::after(
                            context,
                            hook,
                            *operation != Operation::Startup,
                            permit.owner(),
                        )?;
                    }
                    if let Some(path) = &candidate.profile.profile.cleanup {
                        let function = resolve_function(context, path)?;
                        let activation = contexts
                            .get(permit.owner())
                            .cloned()
                            .ok_or("Missing original activation context")?;
                        with(session_id, |s| {
                            s.cleanup.insert(
                                permit.owner().clone(),
                                CooperativeCleanup {
                                    function,
                                    context: activation,
                                },
                            );
                            Ok(())
                        })?;
                    }
                    let yes = context.boolean(true)?;
                    context.set(&active_table, id.as_str(), &yes)?;
                }
                Ok(())
            })();
            if let Err(error) = &result {
                with(session_id, |s| {
                    s.modules.fail_owner(permit.owner(), error);
                    Ok(())
                })?;
                game::log(&format!(
                    "{id} failed stage={phase}: {error}; reconnect before retrying"
                ));
            }
            outcomes.insert(id.clone(), result);
        }
    }
    if *operation != Operation::Startup {
        let requirements: BTreeMap<_, _> = permits
            .iter()
            .map(|permit| {
                (
                    permit.owner().id.clone(),
                    permit.candidate().profile.requires.clone(),
                )
            })
            .collect();
        for (event, replay_event) in [
            ("OnGameStart", ReplayEvent::OnGameStart),
            ("OnLoad", ReplayEvent::OnLoad),
        ] {
            let mut files = BTreeSet::new();
            let mut owners = BTreeMap::new();
            for permit in &permits {
                if outcomes.get(&permit.owner().id).is_some_and(|r| r.is_ok())
                    && permit
                        .candidate()
                        .profile
                        .profile
                        .replay
                        .contains(&replay_event)
                {
                    for source in &permit.candidate().sources {
                        let name = source.filename.to_string_lossy().replace('\\', "/");
                        files.insert(name.clone());
                        owners.insert(name, permit.owner().clone());
                    }
                }
            }
            for callback in game::callbacks(context, event, &files)? {
                let filename = game::callback_filename(context, &callback)?;
                let Some(owner) = owners.get(&filename) else {
                    continue;
                };
                if let Some(required) = failed_requirement(&owner.id, &requirements, &outcomes) {
                    let error = format!("Required profile {required} failed before {event} replay");
                    with(session_id, |s| {
                        s.modules.fail_owner(owner, &error);
                        Ok(())
                    })?;
                    outcomes.insert(owner.id.clone(), Err(error.clone()));
                    status(context, session_id, &owner.id, &format!("failed: {error}"))?;
                    context.set(&active_table, owner.id.as_str(), &Value::default())?;
                    failures.push(owner.id.clone());
                    continue;
                }
                if outcomes
                    .get(&owner.id)
                    .is_some_and(|outcome| outcome.is_err())
                    || !game::callback_registered(context, event, &callback)?
                {
                    continue;
                }
                if let Err(error) = context.call(&callback, &[]) {
                    game::log(&format!("{event} replay failed for {}: {error}", owner.id));
                    with(session_id, |s| {
                        s.modules.fail_owner(owner, &error);
                        Ok(())
                    })?;
                    status(context, session_id, &owner.id, &format!("failed: {error}"))?;
                    context.set(&active_table, owner.id.as_str(), &Value::default())?;
                    outcomes.insert(owner.id.clone(), Err(error));
                    failures.push(owner.id.clone());
                }
            }
        }
    }
    let count = permits.len();
    for permit in permits {
        let owner = permit.owner().clone();
        for required in &permit.candidate().profile.requires {
            if failures.contains(required) {
                outcomes.insert(
                    owner.id.clone(),
                    Err(format!("Required profile {required} failed")),
                );
            }
        }
        let result = outcomes
            .remove(&owner.id)
            .unwrap_or_else(|| Err("No activation outcome".into()));
        let message = match &result {
            Ok(()) => "loaded".to_owned(),
            Err(error) => format!("failed: {error}"),
        };
        if let Err(error) = &result {
            failures.push(owner.id.clone());
            with(session_id, |s| {
                s.modules.fail_owner(&owner, error);
                Ok(())
            })?;
            context.set(&active_table, owner.id.as_str(), &Value::default())?;
        }
        with(session_id, |s| {
            s.policy
                .finish_activation(permit, result)
                .map(|_| ())
                .map_err(|e| e.to_string())
        })?;
        status(context, session_id, &owner.id, &message)?;
    }
    if count > 0 {
        crate::ui::refresh_options(context)?;
    }
    let failed = with(session_id, |session| {
        Ok(session
            .statuses
            .values()
            .filter(|status| status.starts_with("failed:"))
            .count())
    })?;
    let restart = with(session_id, |session| {
        Ok(session
            .statuses
            .values()
            .filter(|status| status.starts_with("restart-required:"))
            .count())
    })?;
    if failures.is_empty() && failed == 0 && restart == 0 {
        Ok(format!(
            "{count} new profiles loaded. Existing profiles were retained."
        ))
    } else {
        Err(format!(
            "{failed} profiles failed; {restart} require native startup. See LuaBuddy diagnostics. Enable startup selections and restart where required; reconnect before retrying failed profiles."
        ))
    }
}

fn failed_requirement(
    id: &ModId,
    requirements: &BTreeMap<ModId, BTreeSet<ModId>>,
    outcomes: &BTreeMap<ModId, Result<()>>,
) -> Option<ModId> {
    let mut pending = requirements
        .get(id)
        .into_iter()
        .flatten()
        .cloned()
        .collect::<Vec<_>>();
    let mut visited = BTreeSet::new();
    while let Some(required) = pending.pop() {
        if !visited.insert(required.clone()) {
            continue;
        }
        if outcomes
            .get(&required)
            .is_some_and(|outcome| outcome.is_err())
        {
            return Some(required);
        }
        pending.extend(requirements.get(&required).into_iter().flatten().cloned());
    }
    None
}

fn replay(
    context: &mut Context<'_, '_>,
    event: &str,
    sources: &[luabuddy_core::source::Source],
) -> Result<()> {
    let files = sources
        .iter()
        .map(|source| source.filename.to_string_lossy().replace('\\', "/"))
        .collect();
    for callback in game::callbacks(context, event, &files)? {
        if game::callback_registered(context, event, &callback)? {
            context.call(&callback, &[])?;
        }
    }
    Ok(())
}

fn exact_boolean(context: &mut Context<'_, '_>, value: &Value) -> Result<Option<bool>> {
    if value.is_nil() {
        return Ok(None);
    }
    context.jni(|env| {
        if env.is_instance_of(value.object(), "java/lang/Boolean")? {
            env.call_method(value.object(), "booleanValue", "()Z", &[])?
                .z()
                .map(Some)
        } else {
            Ok(None)
        }
    })
}

fn resolve_function(context: &mut Context<'_, '_>, path: &str) -> Result<Value> {
    let mut parts = path.split('.');
    let mut value = context.global(parts.next().ok_or("Empty cleanup path")?)?;
    for part in parts {
        if value.is_nil() {
            return Err(format!("Upstream cleanup function is unavailable: {path}"));
        }
        value = context.get(&value, part)?;
    }
    let callable = context.jni(|env| {
        Ok(!value.is_nil()
            && (env.is_instance_of(value.object(), "se/krka/kahlua/vm/JavaFunction")?
                || env.is_instance_of(value.object(), "se/krka/kahlua/vm/LuaClosure")?))
    })?;
    if !callable {
        return Err(format!("Upstream cleanup path is not callable: {path}"));
    }
    Ok(value)
}

pub fn schedule_favorites(_context: &mut Context<'_, '_>, owner: &Owner) -> Result<()> {
    let session_id = id().ok_or("No active LuaBuddy session")?;
    with(session_id, |session| {
        session.favorites_requested = Some(owner.clone());
        Ok(())
    })
}

fn snapshot_favorites(context: &mut Context<'_, '_>, session_id: u64) -> Result<()> {
    let owner = with(session_id, |session| {
        Ok(session
            .favorites_requested
            .take()
            .filter(|owner| generation_live(session, owner)))
    })?;
    let Some(owner) = owner else {
        return Ok(());
    };
    let player = hooks::first(context.call_global("getPlayer", &[])?)?;
    if player.is_nil() {
        return Ok(());
    }
    let data = context.jni(|env| {
        let value = env
            .call_method(
                player.object(),
                "getModData",
                "()Lse/krka/kahlua/vm/KahluaTable;",
                &[],
            )?
            .l()?;
        env.new_global_ref(value).map(|v| Value(Some(v)))
    })?;
    let recipes = context.jni(|env| {
        let manager = env
            .get_static_field(
                "zombie/scripting/ScriptManager",
                "instance",
                "Lzombie/scripting/ScriptManager;",
            )?
            .l()?;
        let value = env
            .call_method(
                manager,
                "getAllCraftRecipes",
                "()Ljava/util/ArrayList;",
                &[],
            )?
            .l()?;
        env.new_global_ref(value).map(|v| Value(Some(v)))
    })?;
    let mut favorites = Vec::new();
    for recipe in game::java_list(context, &recipes)? {
        let key = context.jni(|env| {
            let key = env
                .call_static_method(
                    "zombie/entity/components/crafting/BaseCraftingLogic",
                    "getFavouriteModDataString",
                    "(Lzombie/scripting/entity/components/crafting/CraftRecipe;)Ljava/lang/String;",
                    &[jni::objects::JValue::Object(recipe.object())],
                )?
                .l()?;
            env.new_global_ref(key).map(|v| Value(Some(v)))
        })?;
        let name = context.text(&key)?;
        let value = context.get(&data, &name)?;
        let exactly_true = context.jni(|env| {
            if value.is_nil() || !env.is_instance_of(value.object(), "java/lang/Boolean")? {
                return Ok(false);
            }
            env.call_method(value.object(), "booleanValue", "()Z", &[])?
                .z()
        })?;
        if exactly_true {
            favorites.push(name);
        }
    }
    with(session_id, |session| {
        if !favorites.is_empty() {
            session.favorites = Some((owner, player, favorites));
        }
        Ok(())
    })
}

fn restore_favorites(context: &mut Context<'_, '_>, session_id: u64) -> Result<()> {
    let favorites = with(session_id, |session| Ok(session.favorites.take()))?;
    let Some((owner, player, keys)) = favorites else {
        return Ok(());
    };
    if !with(session_id, |session| Ok(generation_live(session, &owner)))? {
        return Ok(());
    }
    let data = context.jni(|env| {
        let value = env
            .call_method(
                player.object(),
                "getModData",
                "()Lse/krka/kahlua/vm/KahluaTable;",
                &[],
            )?
            .l()?;
        env.new_global_ref(value).map(|v| Value(Some(v)))
    })?;
    let mut changed = false;
    for key in keys {
        let value = context.get(&data, &key)?;
        if value.is_nil() {
            let yes = context.boolean(true)?;
            context.set(&data, &key, &yes)?;
            changed = true;
        }
    }
    if changed {
        context.jni(|env| env.call_method(player.object(), "transmitModData", "()V", &[]))?;
    }
    Ok(())
}

fn generation_live(session: &Session, owner: &Owner) -> bool {
    session
        .policy
        .running()
        .get(&owner.id)
        .is_some_and(|running| {
            running.owner == *owner
                && running.state == luabuddy_core::lifecycle::GenerationState::Loaded
        })
}

fn cancel_favorites(session: &mut Session, owner: &Owner) {
    if session.favorites_requested.as_ref() == Some(owner) {
        session.favorites_requested = None;
    }
    if session
        .favorites
        .as_ref()
        .is_some_and(|(previous, _, _)| previous == owner)
    {
        session.favorites = None;
    }
}
