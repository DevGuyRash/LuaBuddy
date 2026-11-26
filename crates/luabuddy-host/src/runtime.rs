use crate::{
    callables, game, log,
    lua::{Context, Result, Value},
    observe,
};
use jni::{JNIEnv, objects::JObject};
use luabuddy_core::ModId;
use luabuddy_core::diagnostics::{ModDiagnostic, ModStatus};
use luabuddy_core::lifecycle::{GenerationState, Operation, Session as Policy};
use luabuddy_core::source::{ModuleKey, ModuleRegistry, Owner, Require};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

/// Work an integration asked to run later, once, if its generation still runs.
pub struct Deferred(Box<DeferredAction>);

type DeferredAction = dyn FnOnce(&mut Context<'_, '_>) -> Result<Option<Deferred>> + Send;

impl Deferred {
    /// Returning another `Deferred` schedules it for the end of the next tick.
    pub fn new(
        action: impl FnOnce(&mut Context<'_, '_>) -> Result<Option<Deferred>> + Send + 'static,
    ) -> Self {
        Self(Box::new(action))
    }
}

mod loader;
pub use loader::apply;

struct Session {
    id: u64,
    environment: Value,
    state: Value,
    base_require: Value,
    policy: Policy,
    modules: ModuleRegistry<Value>,
    chunks: BTreeMap<(Owner, ModuleKey), Value>,
    owner_thread: std::thread::ThreadId,
    statuses: BTreeMap<ModId, RunState>,
    details: BTreeMap<ModId, ModDiagnostic>,
    startup_attempted: bool,
    start_depth: usize,
    debug: bool,
    cleanup: BTreeMap<Owner, CooperativeCleanup>,
    after_start: Vec<(Owner, Deferred)>,
    after_tick: Vec<(Owner, Deferred)>,
    /// Selection files already reported as unreadable, with the reason.
    unreadable: BTreeSet<(PathBuf, String)>,
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

/// What became of a selected mod in this environment. The one record of it:
/// the diagnostic report and the `LuaBuddy.mods` table are derived from it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RunState {
    Loaded,
    Disabled,
    ServerLoaded,
    RestartRequired(String),
    Failed(String),
}

impl RunState {
    /// The text other mods read from `LuaBuddy.mods`; it must not change.
    pub fn lua(&self) -> String {
        match self {
            Self::Loaded => "loaded".into(),
            Self::Disabled => "disabled".into(),
            Self::ServerLoaded => "server-loaded".into(),
            Self::RestartRequired(reason) => format!("restart-required: {reason}"),
            Self::Failed(error) => format!("failed: {error}"),
        }
    }

    pub fn is_loaded(&self) -> bool {
        *self == Self::Loaded
    }

    /// A selected mod that could not run here.
    pub fn is_problem(&self) -> bool {
        matches!(self, Self::RestartRequired(_) | Self::Failed(_))
    }

    fn report(&self) -> ModStatus {
        match self {
            Self::Loaded => ModStatus::Loaded,
            Self::Disabled => ModStatus::Disabled,
            Self::ServerLoaded => ModStatus::ServerLoaded,
            Self::RestartRequired(_) => ModStatus::RestartRequired,
            Self::Failed(_) => ModStatus::Failed,
        }
    }

    /// Fills the mod's diagnostic record for this state.
    fn describe(&self, detail: &mut ModDiagnostic) {
        detail.status = self.report();
        match self {
            Self::Loaded | Self::Disabled | Self::ServerLoaded => {
                detail.error = None;
                detail.traceback = None;
                detail.java_traceback = None;
                detail.stage = match self {
                    Self::Loaded => "complete".into(),
                    other => other.lua(),
                };
            }
            Self::RestartRequired(reason) => detail.error = Some(reason.clone()),
            Self::Failed(error) => {
                detail.error = Some(error.clone());
                let java = error.split_once("\nJava traceback:\n");
                if let Some((_, trace)) = error.split_once("\nLua traceback:\n") {
                    let lua = trace.split("\nJava traceback:\n").next().unwrap_or(trace);
                    detail.traceback = Some(lua.to_owned());
                }
                if let Some((_, trace)) = java {
                    detail.java_traceback = Some(trace.to_owned());
                }
            }
        }
    }
}

/// The running state as the interface shows it.
#[derive(Default)]
pub struct Snapshot {
    pub ready: bool,
    pub statuses: BTreeMap<ModId, RunState>,
    pub reloadable: BTreeSet<ModId>,
    /// Code from these ran and failed, so only a new environment can retry them.
    pub spent: BTreeSet<ModId>,
}

pub fn snapshot() -> Snapshot {
    let Some(id) = id() else {
        return Snapshot::default();
    };
    with(id, |s| {
        let running = s.policy.running();
        Ok(Snapshot {
            ready: s.policy.ready() && !s.policy.is_busy(),
            statuses: s.statuses.clone(),
            reloadable: running
                .iter()
                .filter(|(_, running)| {
                    running.cleanup_supported && running.state == GenerationState::Loaded
                })
                .map(|(id, _)| id.clone())
                .collect(),
            spent: running
                .iter()
                .filter(|(_, running)| running.state != GenerationState::Loaded)
                .map(|(id, _)| id.clone())
                .collect(),
        })
    })
    .unwrap_or_default()
}

/// How many selected mods could not be loaded in this environment.
pub fn problems() -> usize {
    snapshot()
        .statuses
        .values()
        .filter(|state| state.is_problem())
        .count()
}

pub fn retry_extensions(context: &mut Context<'_, '_>, session_id: u64) -> Result<()> {
    validate_session(context, session_id)?;
    with(session_id, |s| {
        if s.policy.is_busy() {
            return Err(
                "Wait for the current LuaBuddy operation before applying approved patches".into(),
            );
        }
        Ok(())
    })?;
    if crate::ui::pending_options(context)? {
        return Err(
            "Apply or discard pending Options changes before applying approved patches".into(),
        );
    }
    let result = crate::extensions::retry(context);
    let state = with(session_id, |s| Ok(s.state.clone()))?;
    crate::extensions::publish(context, &state)?;
    result
}

/// Every event the runtime acts on.
pub const EVENTS: [&str; 5] = [
    "OnMainMenuEnter",
    "OnGameBoot",
    "OnGameStart",
    "OnCreateUI",
    "OnTick",
];

/// Whether an event matters here. Asked for every event the game triggers.
pub fn watches(name: &str, enter: bool) -> bool {
    match name {
        "OnMainMenuEnter" | "OnGameStart" | "OnCreateUI" => true,
        "OnGameBoot" => enter,
        "OnTick" => !enter && AFTER_TICK.load(Ordering::Relaxed),
        _ => false,
    }
}

/// Set while deferred work waits for a tick, so other ticks cost one load.
static AFTER_TICK: AtomicBool = AtomicBool::new(false);

pub fn event(env: &mut JNIEnv<'_>, name: &str, enter: bool) -> Result<()> {
    if !watches(name, enter) {
        return Ok(());
    }
    let mut context = Context::current(env)?;
    if name == "OnGameBoot" {
        crate::prefetch::start(&mut context);
        return Ok(());
    }
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
                // The outcome is already recorded; startup has nobody to return it to.
                let _ = apply(&mut context, session_id, Operation::Startup);
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
                log::info("session.ready", "The world has started")
                    .with(observe::world(&mut context))
                    .write();
                run_deferred(&mut context, id, false)?;
            }
        }
        if !enter
            && matches!(name, "OnMainMenuEnter" | "OnGameStart")
            && let Err(error) = crate::ui::attach(&mut context, id)
        {
            log::warn(
                "ui.entry",
                format!("LuaBuddy could not add itself to the menu: {error}"),
            )
            .write();
        }
        if name == "OnTick" && !enter {
            run_deferred(&mut context, id, true)?;
        }
    }
    Ok(())
}

pub fn after_startup(owner: &Owner, action: Deferred) -> Result<()> {
    let session_id = id().ok_or("No active LuaBuddy session")?;
    with(session_id, |session| {
        session.after_start.push((owner.clone(), action));
        Ok(())
    })
}

fn run_deferred(context: &mut Context<'_, '_>, session_id: u64, tick: bool) -> Result<()> {
    let due = with(session_id, |session| {
        Ok(std::mem::take(if tick {
            &mut session.after_tick
        } else {
            &mut session.after_start
        }))
    })?;
    if tick {
        AFTER_TICK.store(false, Ordering::Relaxed);
    }
    for (owner, action) in due {
        if !with(session_id, |session| Ok(generation_live(session, &owner)))? {
            continue;
        }
        match (action.0)(context) {
            Ok(Some(next)) => {
                with(session_id, |session| {
                    session.after_tick.push((owner, next));
                    Ok(())
                })?;
                AFTER_TICK.store(true, Ordering::Relaxed);
            }
            Ok(None) => (),
            Err(error) => log::error("mod.deferred", error).about(&owner.id).write(),
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
    let session_id = NEXT.fetch_add(1, Ordering::Relaxed);
    let debug = game::property(context, "luabuddy.debug")?.as_deref() == Some("true");
    if let Err(error) = log::attach(context, session_id, debug) {
        log::warn("journal.unavailable", error).write();
    }
    log::info("session.started", "A new Lua environment started")
        .with(json!({
            "version": env!("CARGO_PKG_VERSION"),
            "loadsNow": load,
            "config": game::property(context, "luabuddy.config")?,
            "templates": game::property(context, "luabuddy.templates")?,
            "earlyUI": game::property(context, "luabuddy.earlyUI")?,
            "world": observe::world(context),
        }))
        .write();
    if let Err(error) = crate::selections::initialize(context) {
        log::warn(
            "selection.once",
            format!("One-launch selections are unavailable: {error}"),
        )
        .write();
    }
    let base_require = context.global("require")?;
    let state = context.table()?;
    let statuses = context.table()?;
    context.set(&state, "mods", &statuses)?;
    let ready = context.boolean(false)?;
    context.set(&state, "ready", &ready)?;
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
        after_start: Vec::new(),
        after_tick: Vec::new(),
        unreadable: BTreeSet::new(),
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
        log::warn(
            "extensions.start",
            format!("Approved extensions did not start: {error}"),
        )
        .write();
    }
    if let Err(error) = crate::extensions::publish(context, &state) {
        log::warn(
            "extensions.publish",
            format!("Patch diagnostics are unavailable: {error}"),
        )
        .write();
    }
    if let Err(error) = crate::ui::install(context, session_id) {
        log::warn(
            "ui.install",
            format!("The LuaBuddy window is unavailable: {error}"),
        )
        .write();
    }
    if load {
        // The outcome is recorded; startup has nobody to return it to.
        let _ = apply(context, session_id, Operation::Startup);
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
            Ok(crate::lua::first(context.call(&function, &[name])?))
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
        retry_extensions(&mut context, session_id)?;
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

/// Records what became of a mod, and publishes it to `LuaBuddy.mods`.
fn status(
    context: &mut Context<'_, '_>,
    session_id: u64,
    id: &ModId,
    value: RunState,
) -> Result<()> {
    let text = value.lua();
    let state = with(session_id, |session| {
        if let Some(detail) = session.details.get_mut(id) {
            value.describe(detail);
        }
        session.statuses.insert(id.clone(), value);
        Ok(session.state.clone())
    })?;
    let statuses = context.get(&state, "mods")?;
    let text = context.string(&text)?;
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
        log::debug("mod.stage", label)
            .about(id)
            .with(json!({ "source": source }))
            .write();
    }
    Ok(())
}

fn generation_live(session: &Session, owner: &Owner) -> bool {
    session
        .policy
        .running()
        .get(&owner.id)
        .is_some_and(|running| running.owner == *owner && running.state == GenerationState::Loaded)
}
