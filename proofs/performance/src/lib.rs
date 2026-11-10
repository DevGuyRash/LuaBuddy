#![deny(unsafe_op_in_unsafe_fn)]

mod binding;
mod growth;
#[allow(dead_code)]
#[path = "../../../crates/luabuddy-host/src/jvmti.rs"]
mod jvmti;
mod native_control;

use jni::{
    JNIEnv,
    objects::{GlobalRef, JObject, JString, JValue},
    sys::{JNIEnv as RawJni, JavaVM, jclass, jobject},
};
use jvmti::{CallbackEnv, Jvmti};
use serde::{Deserialize, Serialize};
use std::{
    ffi::{CStr, c_char, c_void},
    fs,
    path::PathBuf,
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum Mode {
    Menu,
    World,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Configuration {
    schema: u32,
    output: PathBuf,
    enabled: bool,
    mode: Mode,
    warmup_seconds: u64,
    measure_seconds: u64,
    progress: PathBuf,
    account: Option<String>,
    hud: PathBuf,
    #[serde(default)]
    growth: bool,
    #[serde(default)]
    cache: Option<PathBuf>,
    #[serde(default)]
    native_control: bool,
}

#[derive(Serialize)]
struct Ready {
    milestone: &'static str,
    runtime_present: bool,
    runtime_ready: Option<bool>,
    selected_profiles: usize,
    player: Option<[f64; 3]>,
    role: Option<String>,
    screen: [i32; 2],
    game_version: String,
    player_name: Option<String>,
    active_native_mods: usize,
}

enum Readiness {
    Pending(&'static str),
    Usable(Ready),
}

#[derive(Serialize)]
struct Report {
    schema: u32,
    pid: u32,
    enabled: bool,
    mode: Mode,
    startup_ms: f64,
    observer_start_unix_ms: f64,
    ready_unix_ms: f64,
    observer_elapsed_ms: f64,
    warmup_seconds: u64,
    measure_seconds: u64,
    frame_count: usize,
    presentation_count: u64,
    frame_ms: Vec<f64>,
    target: &'static str,
    timing_source: String,
    first_frame_index: usize,
    last_frame_index: usize,
    measure_start_unix_ms: f64,
    measure_end_unix_ms: f64,
    ready: Ready,
    end_ready: Option<Ready>,
    error: Option<String>,
}

struct State {
    started: Instant,
    started_wall: SystemTime,
    ready_at: Option<Instant>,
    ready_wall: Option<SystemTime>,
    ready: Option<Ready>,
    end_ready: Option<Ready>,
    presentations: u64,
    frame_ms: Vec<f64>,
    first_frame: Option<usize>,
    measure_at: Option<Instant>,
    measure_wall: Option<SystemTime>,
    last_frame: usize,
    last_progress: String,
}

static CONFIG: OnceLock<Configuration> = OnceLock::new();
static STATE: OnceLock<Mutex<State>> = OnceLock::new();
static FINISHED: AtomicBool = AtomicBool::new(false);
struct PreparedClass(GlobalRef);

impl PreparedClass {
    fn as_obj(&self) -> &JObject<'static> {
        self.0.as_obj()
    }
}

static LUA_MANAGER: OnceLock<PreparedClass> = OnceLock::new();
static GAME_WINDOW: OnceLock<PreparedClass> = OnceLock::new();
static PLAYER: OnceLock<PreparedClass> = OnceLock::new();
static GAME_LOADING: OnceLock<PreparedClass> = OnceLock::new();

/// # Safety
/// The JVM supplies the live invocation interface and a NUL-terminated UTF-8 configuration path.
// SAFETY: The JVM resolves this library's Agent_OnLoad symbol with its native agent calling convention.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn Agent_OnLoad(
    vm: *mut JavaVM,
    options: *mut c_char,
    _reserved: *mut c_void,
) -> i32 {
    let outcome = std::panic::catch_unwind(|| -> Result<(), String> {
        if vm.is_null() || options.is_null() {
            return Err("Performance observer requires an explicit configuration file".into());
        }
        // SAFETY: Agent_OnLoad receives a NUL-terminated options string from the JVM.
        let path = unsafe { CStr::from_ptr(options) }
            .to_str()
            .map_err(|e| e.to_string())?;
        let config: Configuration =
            serde_json::from_slice(&fs::read(path).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?;
        if config.schema != 1
            || !(1..=120).contains(&config.warmup_seconds)
            || !(1..=300).contains(&config.measure_seconds)
            || !config.output.is_absolute()
            || !config.progress.is_absolute()
            || !config.hud.is_absolute()
            || config.native_control && (!config.growth || config.enabled)
            || config.growth
                && ((!config.enabled && !config.native_control)
                    || !matches!(config.mode, Mode::World)
                    || config.cache.as_ref().is_none_or(|p| !p.is_absolute()))
        {
            return Err("Invalid performance configuration bounds".into());
        }
        CONFIG
            .set(config)
            .map_err(|_| "Performance observer already initialized")?;
        let now = Instant::now();
        STATE
            .set(Mutex::new(State {
                started: now,
                started_wall: SystemTime::now(),
                ready_at: None,
                ready_wall: None,
                ready: None,
                end_ready: None,
                presentations: 0,
                frame_ms: Vec::new(),
                first_frame: None,
                measure_at: None,
                measure_wall: None,
                last_frame: 0,
                last_progress: String::new(),
            }))
            .map_err(|_| "Performance state already initialized")?;
        // SAFETY: vm belongs to this agent invocation and remains live for every callback.
        let jvmti = unsafe { Jvmti::from_vm(vm) }?;
        binding::install(&jvmti)
    });
    match outcome {
        Ok(Ok(())) => 0,
        Ok(Err(error)) => {
            eprintln!("[LuaBuddy performance] FAIL {error}");
            -1
        }
        Err(_) => {
            eprintln!("[LuaBuddy performance] FAIL panic at agent entry");
            -1
        }
    }
}

/// # Safety
/// The VM provides the current callback environments and a live prepared class reference.
unsafe extern "system" fn class_prepare(
    raw: CallbackEnv,
    jni: *mut RawJni,
    _thread: jobject,
    class: jclass,
) {
    let result = std::panic::catch_unwind(|| -> Result<(), String> {
        // SAFETY: Both handles are owned by this active callback.
        let jvmti = unsafe { Jvmti::from_raw(raw) };
        // SAFETY: The JNI environment belongs to this same callback thread.
        let env = unsafe { JNIEnv::from_raw(jni) }.map_err(|e| e.to_string())?;
        let name = jvmti.class_signature(class)?;
        let destination = match name.as_str() {
            "Lzombie/Lua/LuaManager;" => Some(&LUA_MANAGER),
            "Lzombie/GameWindow;" => Some(&GAME_WINDOW),
            "Lzombie/characters/IsoPlayer;" => Some(&PLAYER),
            "Lzombie/gameStates/GameLoadingState;" => Some(&GAME_LOADING),
            _ => None,
        };
        if let Some(destination) = destination {
            // SAFETY: ClassPrepare supplies this live local class reference.
            let class = unsafe { JObject::from_raw(class.cast()) };
            let _ = destination.set(PreparedClass(
                env.new_global_ref(class).map_err(|e| e.to_string())?,
            ));
        }
        Ok(())
    });
    if let Err(error) =
        result.unwrap_or_else(|_| Err("Readiness class observation panic contained".into()))
    {
        // SAFETY: Error handling remains within the active callback.
        unsafe { event_error(jni, &error) };
    }
}

/// # Safety
/// `jni` belongs to the current synchronous event callback on this thread.
unsafe fn event_error(jni: *mut RawJni, message: &str) {
    // SAFETY: The caller retains the current JNI callback environment through this call.
    if let Ok(env) = unsafe { JNIEnv::from_raw(jni) }
        && matches!(env.exception_check(), Ok(true))
    {
        let _ = env.exception_describe();
        let _ = env.exception_clear();
    }
    fail(message);
}

/// # Safety
/// Both environments belong to the current live VM-managed readiness agent thread.
unsafe fn observe(raw: CallbackEnv, jni: *mut RawJni) -> Result<(), String> {
    let now = Instant::now();
    let config = CONFIG.get().ok_or("Missing observer configuration")?;
    // SAFETY: RunAgentThread owns both live handles for this call.
    let mut env = unsafe { JNIEnv::from_raw(jni) }.map_err(|e| e.to_string())?;
    // SAFETY: raw belongs to this same JVM and remains live for the agent callback.
    let jvmti = unsafe { Jvmti::from_raw(raw) };
    if FINISHED.load(Ordering::Acquire) {
        return controls(&mut env, &jvmti, true);
    }
    controls(&mut env, &jvmti, false)?;
    if config.growth {
        if config.native_control {
            return native_control::observe(&mut env, &jvmti);
        }
        return growth::observe(&mut env, &jvmti);
    }
    let ready_at = STATE
        .get()
        .ok_or("Missing state")?
        .lock()
        .map_err(|_| "Observer lock poisoned")?
        .ready_at;
    if let Some(ready_at) = ready_at {
        if now.duration_since(ready_at) < Duration::from_secs(config.warmup_seconds) {
            return Ok(());
        }
        let first = STATE
            .get()
            .ok_or("Missing state")?
            .lock()
            .map_err(|_| "Observer lock poisoned")?
            .first_frame;
        if first.is_none() {
            let count = hud_values(&config.hud)?.len();
            let mut state = STATE
                .get()
                .ok_or("Missing state")?
                .lock()
                .map_err(|_| "Observer lock poisoned")?;
            state.first_frame = Some(count);
            state.measure_at = Some(now);
            state.measure_wall = Some(SystemTime::now());
            return Ok(());
        }
        let measure_at = STATE
            .get()
            .ok_or("Missing state")?
            .lock()
            .map_err(|_| "Observer lock poisoned")?
            .measure_at
            .ok_or("Missing measure start")?;
        if now.duration_since(measure_at) < Duration::from_secs(config.measure_seconds) {
            return Ok(());
        }
        let end_ready = match readiness(&mut env, &jvmti, config.mode, config.enabled)? {
            Readiness::Usable(ready) => ready,
            Readiness::Pending(reason) => {
                return Err(format!(
                    "Native workload was not usable at measurement completion: {reason}"
                ));
            }
        };
        if end_ready.screen != [1280, 800] || end_ready.active_native_mods != 0 {
            return Err("Viewport or native mod selection changed".into());
        }
        if matches!(config.mode, Mode::World) {
            let state = STATE
                .get()
                .ok_or("Missing state")?
                .lock()
                .map_err(|_| "Observer lock poisoned")?;
            let initial = state.ready.as_ref().ok_or("Missing initial readiness")?;
            if initial.role.as_deref() != Some("user")
                || end_ready.role.as_deref() != Some("user")
                || initial.player != end_ready.player
            {
                return Err("Ordinary fixture role or fixed player location changed".into());
            }
        }
        let values = hud_values(&config.hud)?;
        let first = first.ok_or("Missing first frame")?;
        if values.len() <= first + 1 {
            return Err("Mesa did not record completed per-frame work".into());
        }
        let mut state = STATE
            .get()
            .ok_or("Missing state")?
            .lock()
            .map_err(|_| "Observer lock poisoned")?;
        state.frame_ms = values[first..].to_vec();
        state.last_frame = values.len();
        state.presentations = values.len() as u64;
        state.end_ready = Some(end_ready);
        drop(state);
        return finish(None);
    }
    if let Readiness::Usable(ready) = readiness(&mut env, &jvmti, config.mode, config.enabled)? {
        let mut state = STATE
            .get()
            .ok_or("Missing state")?
            .lock()
            .map_err(|_| "Observer lock poisoned")?;
        state.ready_at = Some(now);
        state.ready_wall = Some(SystemTime::now());
        state.ready = Some(ready);
        eprintln!("[LuaBuddy performance] usable {:?} milestone", config.mode);
    }
    Ok(())
}

fn hud_values(path: &std::path::Path) -> Result<Vec<f64>, String> {
    let text = fs::read_to_string(path)
        .map_err(|e| format!("Reading Mesa per-frame dump {}: {e}", path.display()))?;
    hud_rows(&text)
}

fn hud_rows(text: &str) -> Result<Vec<f64>, String> {
    let mut values = Vec::new();
    for line in text.split_inclusive('\n') {
        if !line.ends_with('\n') {
            break;
        }
        let value: f64 = line
            .trim()
            .parse()
            .map_err(|_| "Mesa frame row is not numeric")?;
        if !value.is_finite() || value < 0.0 {
            return Err("Invalid Mesa frame duration".into());
        }
        values.push(value);
    }
    Ok(values)
}

fn publish_json(path: &std::path::Path, value: &serde_json::Value) -> Result<(), String> {
    let temporary = path.with_extension("partial");
    fs::write(
        &temporary,
        serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    fs::rename(temporary, path).map_err(|e| e.to_string())
}

fn button(
    env: &mut JNIEnv<'_>,
    value: &JObject<'_>,
    id: &str,
) -> jni::errors::Result<Option<serde_json::Value>> {
    if value.is_null() || !ui_visible(env, value)? {
        return Ok(None);
    }
    let enabled = rawget(env, value, "enable")?;
    if !enabled.is_null() && !env.call_method(enabled, "booleanValue", "()Z", &[])?.z()? {
        return Ok(None);
    }
    let object = rawget(env, value, "javaObject")?;
    let mut number = |name: &str| -> jni::errors::Result<f64> {
        let value = env
            .call_method(&object, name, "()Ljava/lang/Double;", &[])?
            .l()?;
        env.call_method(value, "doubleValue", "()D", &[])?.d()
    };
    let x = number("getAbsoluteX")? + number("getWidth")? / 2.0;
    let y = number("getAbsoluteY")? + number("getHeight")? / 2.0;
    if !x.is_finite() || !y.is_finite() || !(0.0..1280.0).contains(&x) || !(0.0..800.0).contains(&y)
    {
        return Ok(None);
    }
    Ok(Some(
        serde_json::json!({"action":"click","id":id,"x":x,"y":y,"pid":std::process::id()}),
    ))
}

fn controls(env: &mut JNIEnv<'_>, jvmti: &Jvmti, finished: bool) -> Result<(), String> {
    let Some(manager) = LUA_MANAGER.get() else {
        return Ok(());
    };
    if !initialized(jvmti, manager)? {
        return Ok(());
    }
    let configuration = CONFIG.get().ok_or("Missing observer configuration")?;
    let mut inspect = || -> jni::errors::Result<Option<serde_json::Value>> {
        let class: &jni::objects::JClass<'_> = manager.as_obj().into();
        let environment = env
            .get_static_field(class, "env", "Lse/krka/kahlua/vm/KahluaTable;")?
            .l()?;
        if environment.is_null() {
            return Ok(None);
        }
        if finished {
            let connect_class = rawget(env, &environment, "ServerConnectPopup")?;
            let connect_popup = rawget(env, &connect_class, "instance")?;
            let cancel = rawget(env, &connect_popup, "cancelBtn")?;
            if let Some(action) = button(env, &cancel, "ServerConnectPopup.cancelBtn")? {
                return Ok(Some(action));
            }
            let controller_class = rawget(env, &environment, "ConnectToServer")?;
            let controller = rawget(env, &controller_class, "instance")?;
            let back = rawget(env, &controller, "backBtn")?;
            if let Some(action) = button(env, &back, "ConnectToServer.nativeBack")? {
                return Ok(Some(action));
            }
            let main_class = rawget(env, &environment, "MainScreen")?;
            let main = rawget(env, &main_class, "instance")?;
            let multiplayer = rawget(env, &main, "multiplayer")?;
            let back = rawget(env, &multiplayer, "backButton")?;
            if let Some(action) = button(env, &back, "MultiplayerUI.nativeBack")? {
                return Ok(Some(action));
            }
            let options_class = rawget(env, &environment, "MainOptions")?;
            let options_instance = rawget(env, &options_class, "instance")?;
            let options_modal = rawget(env, &options_instance, "modal")?;
            for field in ["ok", "no"] {
                let dismiss = rawget(env, &options_modal, field)?;
                if let Some(action) = button(env, &dismiss, "MainOptions.nativeModalDismiss")? {
                    return Ok(Some(action));
                }
            }
            let options = rawget(env, &main, "mainOptions")?;
            let back = rawget(env, &options, "backButton")?;
            if let Some(action) = button(env, &back, "MainOptions.nativeBack")? {
                return Ok(Some(action));
            }
            let mut modal = rawget(env, &main, "quitToDesktopDialog")?;
            if modal.is_null() {
                modal = rawget(env, &main_class, "quitToDesktopDialog")?;
            }
            let yes = rawget(env, &modal, "yes")?;
            if let Some(action) = button(env, &yes, "MainScreen.quitToDesktopDialog.yes")? {
                return Ok(Some(action));
            }
            let in_game = rawget(env, &main, "inGame")?;
            let in_game =
                !in_game.is_null() && env.call_method(in_game, "booleanValue", "()Z", &[])?.z()?;
            let field = if in_game {
                "quitToDesktopOption"
            } else {
                "exitOption"
            };
            let quit = rawget(env, &main, field)?;
            if let Some(action) = button(env, &quit, "MainScreen.nativeQuitLabel")? {
                return Ok(Some(action));
            }
            if matches!(configuration.mode, Mode::World)
                && !main.is_null()
                && !ui_visible(env, &main)?
            {
                return Ok(Some(
                    serde_json::json!({"action":"key","id":"MainScreen.open-pause-menu","key":"Escape","pid":std::process::id()}),
                ));
            }
            return Ok(None);
        }
        if matches!(configuration.mode, Mode::World) {
            let guide_class = rawget(env, &environment, "SurvivalGuide")?;
            let guide = rawget(env, &guide_class, "instance")?;
            if !guide.is_null() && ui_visible(env, &guide)? {
                let close = rawget(env, &guide, "closeButton")?;
                return button(env, &close, "SurvivalGuide.closeButton");
            }
            let class = rawget(env, &environment, "ServerConnectPopup")?;
            let popup = rawget(env, &class, "instance")?;
            let connect = rawget(env, &popup, "connectBtn")?;
            let entry = rawget(env, &popup, "usernameEntry")?;
            let entry = rawget(env, &entry, "javaObject")?;
            let matching_account = if entry.is_null() {
                false
            } else {
                let text = env
                    .call_method(entry, "getText", "()Ljava/lang/String;", &[])?
                    .l()?;
                let text = env
                    .get_string(&JString::from(text))?
                    .to_string_lossy()
                    .into_owned();
                configuration
                    .account
                    .as_ref()
                    .is_none_or(|expected| text == *expected)
            };
            if matching_account
                && let Some(action) =
                    button(env, &connect, "ServerConnectPopup.instance.connectBtn")?
            {
                return Ok(Some(action));
            }
            if !matching_account {
                let username = rawget(env, &popup, "usernameEntry")?;
                let password = rawget(env, &popup, "passwordEntry")?;
                if let (Some(username), Some(password)) = (
                    button(env, &username, "fixture.username")?,
                    button(env, &password, "fixture.password")?,
                ) {
                    return Ok(Some(
                        serde_json::json!({"action":"fixture-login","id":"ServerConnectPopup.fixture-credentials","username":username,"password":password,"pid":std::process::id()}),
                    ));
                }
            }
            let loading_active = if let Some(window) = GAME_WINDOW.get()
                && initialized(jvmti, window)
                    .map_err(|_| jni::errors::Error::NullPtr("GameWindow class status"))?
            {
                let window: &jni::objects::JClass<'_> = window.as_obj().into();
                let states = env
                    .get_static_field(window, "states", "Lzombie/gameStates/GameStateMachine;")?
                    .l()?;
                if states.is_null() {
                    false
                } else {
                    let current = env
                        .get_field(states, "current", "Lzombie/gameStates/GameState;")?
                        .l()?;
                    if current.is_null() {
                        false
                    } else {
                        let class = env.get_object_class(current)?;
                        jvmti.class_signature(class.as_raw()).map_err(|_| {
                            jni::errors::Error::NullPtr("GameLoadingState signature")
                        })? == "Lzombie/gameStates/GameLoadingState;"
                    }
                }
            } else {
                false
            };
            if loading_active
                && let Some(class) = GAME_LOADING.get()
                && initialized(jvmti, class)
                    .map_err(|_| jni::errors::Error::NullPtr("GameLoadingState class status"))?
            {
                let class: &jni::objects::JClass<'_> = class.as_obj().into();
                if env.get_static_field(class, "showedClickToSkip", "Z")?.z()?
                    && env.get_static_field(class, "done", "Z")?.z()?
                {
                    return Ok(Some(
                        serde_json::json!({"action":"click","id":"GameLoadingState.click-to-start","x":640,"y":750,"pid":std::process::id()}),
                    ));
                }
            }
        }
        Ok(None)
    };
    let mut value = inspect().map_err(|e| format!("Observing native verification control: {e}"))?;
    if configuration.growth
        && let Some(value) = value.as_mut()
    {
        value["sequence"] = serde_json::json!(growth::sequence());
    }
    let value = serde_json::to_string(&value).map_err(|e| e.to_string())?;
    let mut state = STATE
        .get()
        .ok_or("Missing observer state")?
        .lock()
        .map_err(|_| "Observer lock poisoned")?;
    if state.last_progress != value {
        fs::write(&configuration.progress, &value).map_err(|e| e.to_string())?;
        state.last_progress = value;
    }
    Ok(())
}

fn initialized(jvmti: &Jvmti, class: &PreparedClass) -> Result<bool, String> {
    let raw = jvmti.raw();
    let mut status = 0;
    // SAFETY: JVMTI 1.2 slot49 is GetClassStatus. The retained class reference and writable output are valid.
    let call: unsafe extern "system" fn(CallbackEnv, jclass, *mut i32) -> i32 =
        unsafe { std::mem::transmute(*(*raw).add(48)) };
    // SAFETY: The class was observed through ClassPrepare and remains strongly retained by this observer.
    let result = unsafe { call(raw, class.as_obj().as_raw().cast(), &mut status) };
    if result != 0 {
        return Err(format!("GetClassStatus failed ({result})"));
    }
    Ok(status & 4 != 0)
}

fn rawget<'a>(
    env: &mut JNIEnv<'a>,
    table: &JObject<'_>,
    key: &str,
) -> jni::errors::Result<JObject<'a>> {
    if table.is_null() {
        return Ok(JObject::null());
    }
    let key = env.new_string(key)?;
    env.call_method(
        table,
        "rawget",
        "(Ljava/lang/Object;)Ljava/lang/Object;",
        &[JValue::Object(&key)],
    )?
    .l()
}

fn ui_visible(env: &mut JNIEnv<'_>, table: &JObject<'_>) -> jni::errors::Result<bool> {
    let java = rawget(env, table, "javaObject")?;
    if java.is_null() {
        return Ok(false);
    }
    env.call_method(java, "isReallyVisible", "()Z", &[])?.z()
}

fn metadata(env: &mut JNIEnv<'_>) -> jni::errors::Result<([i32; 2], String, usize)> {
    let core = env
        .call_static_method(
            "zombie/core/Core",
            "getInstance",
            "()Lzombie/core/Core;",
            &[],
        )?
        .l()?;
    let screen = [
        env.call_method(&core, "getScreenWidth", "()I", &[])?.i()?,
        env.call_method(&core, "getScreenHeight", "()I", &[])?.i()?,
    ];
    let version = env
        .call_method(core, "getGameAndBuildVersion", "()Ljava/lang/String;", &[])?
        .l()?;
    let version = env
        .get_string(&JString::from(version))?
        .to_string_lossy()
        .into_owned();
    let filesystem = env
        .get_static_field(
            "zombie/ZomboidFileSystem",
            "instance",
            "Lzombie/ZomboidFileSystem;",
        )?
        .l()?;
    let mods = env
        .call_method(filesystem, "getModIDs", "()Ljava/util/List;", &[])?
        .l()?;
    let count = env.call_method(mods, "size", "()I", &[])?.i()? as usize;
    Ok((screen, version, count))
}

fn readiness(
    env: &mut JNIEnv<'_>,
    jvmti: &Jvmti,
    mode: Mode,
    enabled: bool,
) -> Result<Readiness, String> {
    let Some(manager) = LUA_MANAGER.get() else {
        return Ok(Readiness::Pending("LuaManager has not prepared"));
    };
    if !initialized(jvmti, manager)? {
        return Ok(Readiness::Pending("LuaManager has not initialized"));
    }
    let mut attempt = || -> jni::errors::Result<Readiness> {
        let manager_class: &jni::objects::JClass<'_> = manager.as_obj().into();
        let environment = env
            .get_static_field(manager_class, "env", "Lse/krka/kahlua/vm/KahluaTable;")?
            .l()?;
        if environment.is_null() {
            return Ok(Readiness::Pending("native Lua environment is absent"));
        }
        let state = rawget(env, &environment, "LuaBuddy")?;
        let present = !state.is_null();
        let runtime_ready = if present {
            let value = rawget(env, &state, "ready")?;
            if value.is_null() {
                None
            } else {
                Some(env.call_method(value, "booleanValue", "()Z", &[])?.z()?)
            }
        } else {
            None
        };
        let profiles = if present {
            let mods = rawget(env, &state, "mods")?;
            if mods.is_null() {
                0
            } else {
                env.call_method(mods, "size", "()I", &[])?.i()? as usize
            }
        } else {
            0
        };
        let expected_profiles = if CONFIG.get().is_some_and(|c| c.growth && !c.native_control) {
            2
        } else {
            0
        };
        if profiles != expected_profiles || present != enabled {
            return Ok(Readiness::Pending(
                "runtime presence or empty profile selection differs",
            ));
        }
        match mode {
            Mode::Menu => {
                let main_class = rawget(env, &environment, "MainScreen")?;
                let main = rawget(env, &main_class, "instance")?;
                let bottom = rawget(env, &main, "bottomPanel")?;
                let options = rawget(env, &main, "optionsOption")?;
                if main.is_null()
                    || !ui_visible(env, &main)?
                    || !ui_visible(env, &bottom)?
                    || !ui_visible(env, &options)?
                {
                    return Ok(Readiness::Pending(
                        "native main-menu controls are not visible",
                    ));
                }
                let popup = rawget(env, &main, "animPopup")?;
                if !popup.is_null() && ui_visible(env, &popup)? {
                    return Ok(Readiness::Pending("native menu popup remains visible"));
                }
                let (screen, game_version, active_native_mods) = metadata(env)?;
                Ok(Readiness::Usable(Ready {
                    milestone: "visible native main-menu Options control",
                    runtime_present: present,
                    runtime_ready,
                    selected_profiles: profiles,
                    player: None,
                    role: None,
                    screen,
                    game_version,
                    player_name: None,
                    active_native_mods,
                }))
            }
            Mode::World => {
                let guide_class = rawget(env, &environment, "SurvivalGuide")?;
                let guide = rawget(env, &guide_class, "instance")?;
                if !guide.is_null() && ui_visible(env, &guide)? {
                    return Ok(Readiness::Pending("native Survival Guide remains visible"));
                }
                let main_class = rawget(env, &environment, "MainScreen")?;
                let main = rawget(env, &main_class, "instance")?;
                if !main.is_null() && ui_visible(env, &main)? {
                    return Ok(Readiness::Pending("native pause menu is visible"));
                }
                let (Some(window), Some(player_class)) = (GAME_WINDOW.get(), PLAYER.get()) else {
                    return Ok(Readiness::Pending("native world classes have not prepared"));
                };
                if !initialized(jvmti, window)
                    .map_err(|_| jni::errors::Error::NullPtr("GameWindow class status"))?
                    || !initialized(jvmti, player_class)
                        .map_err(|_| jni::errors::Error::NullPtr("IsoPlayer class status"))?
                {
                    return Ok(Readiness::Pending(
                        "native world classes have not initialized",
                    ));
                }
                let window_class: &jni::objects::JClass<'_> = window.as_obj().into();
                let machine = env
                    .get_static_field(
                        window_class,
                        "states",
                        "Lzombie/gameStates/GameStateMachine;",
                    )?
                    .l()?;
                if machine.is_null() {
                    return Ok(Readiness::Pending("native game-state machine is absent"));
                }
                let current = env
                    .get_field(machine, "current", "Lzombie/gameStates/GameState;")?
                    .l()?;
                if current.is_null() {
                    return Ok(Readiness::Pending("native current game state is absent"));
                }
                let class = env.get_object_class(current)?;
                if jvmti
                    .class_signature(class.as_raw())
                    .map_err(|_| jni::errors::Error::NullPtr("GameState signature"))?
                    != "Lzombie/gameStates/IngameState;"
                {
                    return Ok(Readiness::Pending(
                        "native current state is not IngameState",
                    ));
                }
                let player_class: &jni::objects::JClass<'_> = player_class.as_obj().into();
                let player = env
                    .call_static_method(
                        player_class,
                        "getInstance",
                        "()Lzombie/characters/IsoPlayer;",
                        &[],
                    )?
                    .l()?;
                if player.is_null()
                    || env.call_method(&player, "isDead", "()Z", &[])?.z()?
                    || enabled && runtime_ready != Some(true)
                {
                    return Ok(Readiness::Pending(
                        "native player is absent/dead or runtime is not ready",
                    ));
                }
                let position = [
                    env.call_method(&player, "getX", "()F", &[])?.f()? as f64,
                    env.call_method(&player, "getY", "()F", &[])?.f()? as f64,
                    env.call_method(&player, "getZ", "()F", &[])?.f()? as f64,
                ];
                let role = env
                    .call_method(&player, "getAccessLevel", "()Ljava/lang/String;", &[])?
                    .l()?;
                let role = env
                    .get_string(&JString::from(role))?
                    .to_string_lossy()
                    .into_owned();
                let name = env
                    .call_method(&player, "getUsername", "()Ljava/lang/String;", &[])?
                    .l()?;
                let name = env
                    .get_string(&JString::from(name))?
                    .to_string_lossy()
                    .into_owned();
                if CONFIG
                    .get()
                    .and_then(|config| config.account.as_ref())
                    .is_some_and(|expected| expected != &name)
                {
                    return Ok(Readiness::Pending("native player identity differs"));
                }
                let (screen, game_version, active_native_mods) = metadata(env)?;
                Ok(Readiness::Usable(Ready {
                    milestone: "alive native player in IngameState",
                    runtime_present: present,
                    runtime_ready,
                    selected_profiles: profiles,
                    player: Some(position),
                    role: Some(role),
                    screen,
                    game_version,
                    player_name: Some(name),
                    active_native_mods,
                }))
            }
        }
    };
    attempt().map_err(|error| format!("Observing actual usable state: {error}"))
}

fn fail(error: &str) {
    eprintln!("[LuaBuddy performance] FAIL {error}");
    if CONFIG.get().is_some_and(|c| c.growth) {
        if CONFIG.get().is_some_and(|c| c.native_control) {
            native_control::failure(error);
        } else {
            growth::failure(error);
        }
        FINISHED.store(true, Ordering::Release);
        return;
    }
    let _ = finish(Some(error.to_owned()));
}

fn finish(error: Option<String>) -> Result<(), String> {
    if FINISHED.swap(true, Ordering::AcqRel) {
        return Ok(());
    }
    let config = CONFIG.get().ok_or("Missing observer configuration")?;
    let mut state = STATE
        .get()
        .ok_or("Missing observer state")?
        .lock()
        .map_err(|_| "Observer lock poisoned")?;
    let now = Instant::now();
    let Some(ready_at) = state.ready_at else {
        fs::write(&config.output, serde_json::to_vec_pretty(&serde_json::json!({"schema":1,"error":error.unwrap_or_else(|| "No usable milestone".into()),"presentationCount":state.presentations})).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        return Ok(());
    };
    let ready = state
        .ready
        .take()
        .ok_or("Missing usable state observation")?;
    let report = Report {
        schema: 1,
        pid: std::process::id(),
        enabled: config.enabled,
        mode: config.mode,
        startup_ms: ready_at.duration_since(state.started).as_secs_f64() * 1000.0,
        observer_start_unix_ms: state
            .started_wall
            .duration_since(UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_secs_f64()
            * 1000.0,
        ready_unix_ms: state
            .ready_wall
            .ok_or("Missing readiness wall clock")?
            .duration_since(UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_secs_f64()
            * 1000.0,
        observer_elapsed_ms: now.duration_since(state.started).as_secs_f64() * 1000.0,
        warmup_seconds: config.warmup_seconds,
        measure_seconds: config.measure_seconds,
        frame_count: state.frame_ms.len(),
        presentation_count: state.presentations,
        frame_ms: std::mem::take(&mut state.frame_ms),
        target: "Mesa Gallium renderer per-frame presentation interval",
        timing_source:
            "Mesa26.2.6 Gallium frametime per-present query, zero-period line-buffered dump".into(),
        first_frame_index: state.first_frame.unwrap_or(0),
        last_frame_index: state.last_frame,
        measure_start_unix_ms: state
            .measure_wall
            .unwrap_or(state.started_wall)
            .duration_since(UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_secs_f64()
            * 1000.0,
        measure_end_unix_ms: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_secs_f64()
            * 1000.0,
        ready,
        end_ready: state.end_ready.take(),
        error,
    };
    let temporary = config.output.with_extension("partial");
    fs::write(
        &temporary,
        serde_json::to_vec_pretty(&report).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    fs::rename(temporary, &config.output).map_err(|e| e.to_string())?;
    eprintln!(
        "[LuaBuddy performance] completed {} actual presentation intervals",
        report.frame_count
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::hud_rows;

    #[test]
    fn line_buffered_hud_rows_ignore_an_incomplete_final_write() {
        assert_eq!(hud_rows("1.25\n20\n3.5").unwrap(), [1.25, 20.0]);
        assert_eq!(hud_rows("0.004\n0\n").unwrap(), [0.004, 0.0]);
    }

    #[test]
    fn invalid_hud_data_cannot_become_successful_fast_work() {
        for input in ["NaN\n", "inf\n", "-1\n", "not-a-frame\n"] {
            assert!(hud_rows(input).is_err(), "{input}");
        }
    }
}
