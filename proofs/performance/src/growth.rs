use crate::{
    CONFIG, FINISHED, GAME_WINDOW, LUA_MANAGER, Mode, Readiness, STATE, button, initialized,
    rawget, readiness, ui_visible,
};
use jni::{
    JNIEnv,
    objects::{JObject, JValue, WeakRef},
};
use serde_json::{Value, json};
use std::{
    fs,
    sync::{Mutex, OnceLock, atomic::Ordering},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const SETTLE: Duration = Duration::from_secs(5);
const SAMPLE: Duration = Duration::from_secs(2);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Refresh,
    Reload,
    Reconnect,
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Self::Refresh => "refresh",
            Self::Reload => "reload",
            Self::Reconnect => "reconnect",
        }
    }
    fn count(self) -> usize {
        if self == Self::Reconnect { 6 } else { 11 }
    }
}

enum Phase {
    AwaitWorld,
    Warmup {
        at: Instant,
        before: Value,
    },
    Operate {
        kind: Kind,
        cycle: usize,
        before: Value,
        started: Instant,
    },
    Gc {
        kind: Kind,
        cycle: usize,
        report: Value,
    },
    Settle {
        kind: Kind,
        cycle: usize,
        at: Instant,
        report: Value,
    },
    Sample {
        kind: Kind,
        cycle: usize,
        at: Instant,
        report: Value,
        rss: Vec<u64>,
        last: Instant,
    },
    Done,
}

struct Objects {
    owned: WeakRef,
    module: WeakRef,
    namespace: WeakRef,
    foreign: WeakRef,
}

#[derive(Clone, Copy, Debug, serde::Serialize)]
struct TickCounts {
    owned: f64,
    foreign: f64,
}

struct Growth {
    started: Instant,
    phase: Phase,
    current: Option<Objects>,
    retired: Vec<WeakRef>,
    records: Vec<Value>,
    position: Option<[f64; 3]>,
    settle_ticks: Option<TickCounts>,
    last_report: Option<Value>,
}

static GROWTH: OnceLock<Mutex<Growth>> = OnceLock::new();

pub(crate) fn sequence() -> usize {
    if CONFIG.get().is_some_and(|c| c.native_control) {
        return crate::native_control::sequence();
    }
    GROWTH
        .get()
        .and_then(|s| s.lock().ok())
        .map_or(1, |s| s.records.len() + 1)
}

pub(crate) fn environment<'a>(env: &mut JNIEnv<'a>) -> jni::errors::Result<JObject<'a>> {
    let manager = LUA_MANAGER
        .get()
        .ok_or(jni::errors::Error::NullPtr("LuaManager"))?;
    let class: &jni::objects::JClass<'_> = manager.as_obj().into();
    env.get_static_field(class, "env", "Lse/krka/kahlua/vm/KahluaTable;")?
        .l()
}

fn text(env: &mut JNIEnv<'_>, value: &JObject<'_>) -> jni::errors::Result<String> {
    if value.is_null() {
        return Ok(String::new());
    }
    env.get_string(value.into())
        .map(|s| s.to_string_lossy().into_owned())
}

fn number(env: &mut JNIEnv<'_>, value: &JObject<'_>) -> jni::errors::Result<f64> {
    if value.is_null() {
        return Ok(0.0);
    }
    env.call_method(value, "doubleValue", "()D", &[])?.d()
}

fn index<'a>(
    env: &mut JNIEnv<'a>,
    table: &JObject<'_>,
    index: usize,
) -> jni::errors::Result<JObject<'a>> {
    env.call_method(
        table,
        "rawget",
        "(I)Ljava/lang/Object;",
        &[JValue::Int(index as i32)],
    )?
    .l()
}

fn report() -> Result<Value, String> {
    let config = CONFIG.get().ok_or("Missing growth configuration")?;
    let cache = config
        .cache
        .as_ref()
        .ok_or("Growth requires an owned cache")?;
    let report: Value = serde_json::from_slice(
        &fs::read(cache.join("LuaBuddy/diagnostics.json")).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    if report["completedAt"].is_null()
        || !report["error"].is_null()
        || report["mods"].as_object().is_none_or(|m| m.len() != 2)
    {
        return Err("Growth requires exactly two successfully loaded fixture profiles".into());
    }
    for id in ["LuaBuddyGrowth", "LuaBuddyGrowthForeign"] {
        if report["mods"][id]["status"] != "loaded" {
            return Err(format!("Growth fixture is not loaded: {id}"));
        }
    }
    Ok(report)
}

fn validate_transition(before: &Value, after: &Value, kind: Kind) -> Result<(), String> {
    let session = |value: &Value| -> Result<(String, u64), String> {
        let value = value["session"]
            .as_str()
            .ok_or("Missing native session identity")?;
        let (process, id) = value
            .rsplit_once('-')
            .ok_or("Malformed native session identity")?;
        Ok((
            process.into(),
            id.parse::<u64>().map_err(|e| e.to_string())?,
        ))
    };
    let old_session = session(before)?;
    let new_session = session(after)?;
    if old_session.0 != new_session.0
        || if kind == Kind::Reconnect {
            new_session.1 <= old_session.1
        } else {
            new_session.1 != old_session.1
        }
    {
        return Err(format!(
            "Native session contract failed: {} -> {}",
            before["session"], after["session"]
        ));
    }
    let generation = |value: &Value, id: &str| {
        value["mods"][id]["generation"]
            .as_u64()
            .ok_or_else(|| format!("Missing {id} generation"))
    };
    let a = generation(before, "LuaBuddyGrowth")?;
    let b = generation(after, "LuaBuddyGrowth")?;
    let foreign_a = generation(before, "LuaBuddyGrowthForeign")?;
    let foreign_b = generation(after, "LuaBuddyGrowthForeign")?;
    let valid = match kind {
        Kind::Refresh => a == b && foreign_a == foreign_b && after["target"].is_null(),
        Kind::Reload => b > a && foreign_a == foreign_b && after["target"] == "LuaBuddyGrowth",
        Kind::Reconnect => b > 0 && foreign_b > 0 && after["target"].is_null(),
    };
    if !valid {
        return Err(format!(
            "Native {} generation contract failed: target {a} -> {b}, foreign {foreign_a} -> {foreign_b}",
            kind.name()
        ));
    }
    Ok(())
}

fn ticks(env: &mut JNIEnv<'_>) -> Result<TickCounts, String> {
    let root = environment(env).map_err(|e| e.to_string())?;
    let mut counter = |id: &str| -> Result<f64, String> {
        let module = rawget(env, &root, id).map_err(|e| e.to_string())?;
        let value = rawget(env, &module, "ticks").map_err(|e| e.to_string())?;
        if value.is_null() {
            return Err(format!("Missing {id} native tick counter"));
        }
        let value = number(env, &value).map_err(|e| e.to_string())?;
        if !value.is_finite() || value < 0.0 {
            return Err("Invalid native fixture tick counter".into());
        }
        Ok(value)
    };
    Ok(TickCounts {
        owned: counter("LuaBuddyGrowth")?,
        foreign: counter("LuaBuddyGrowthForeign")?,
    })
}

fn validate_work(settle: TickCounts, after: TickCounts) -> Result<(), String> {
    if after.owned <= settle.owned || after.foreign <= settle.foreign {
        return Err("Both native fixture callbacks must advance after the completed operation and world resume".into());
    }
    Ok(())
}

pub(crate) fn rss() -> Result<u64, String> {
    fs::read_to_string("/proc/self/status")
        .map_err(|e| e.to_string())?
        .lines()
        .find_map(|line| line.strip_prefix("VmRSS:"))
        .and_then(|s| s.split_whitespace().next())
        .ok_or("Missing native VmRSS")?
        .parse::<u64>()
        .map(|v| v * 1024)
        .map_err(|e| e.to_string())
}

pub(crate) fn heap(env: &mut JNIEnv<'_>) -> jni::errors::Result<Value> {
    let bean = env
        .call_static_method(
            "java/lang/management/ManagementFactory",
            "getMemoryMXBean",
            "()Ljava/lang/management/MemoryMXBean;",
            &[],
        )?
        .l()?;
    let usage = env
        .call_method(
            bean,
            "getHeapMemoryUsage",
            "()Ljava/lang/management/MemoryUsage;",
            &[],
        )?
        .l()?;
    Ok(
        json!({"usedBytes":env.call_method(&usage,"getUsed","()J",&[])?.j()?,"committedBytes":env.call_method(usage,"getCommitted","()J",&[])?.j()?}),
    )
}

fn callbacks<'a>(
    env: &mut JNIEnv<'a>,
) -> jni::errors::Result<(Vec<JObject<'a>>, Vec<JObject<'a>>, usize)> {
    let events = env
        .get_static_field(
            "zombie/Lua/LuaEventManager",
            "EventMap",
            "Ljava/util/HashMap;",
        )?
        .l()?;
    let name = env.new_string("OnTick")?;
    let event = env
        .call_method(
            events,
            "get",
            "(Ljava/lang/Object;)Ljava/lang/Object;",
            &[JValue::Object(&name)],
        )?
        .l()?;
    if event.is_null() {
        return Err(jni::errors::Error::NullPtr("native OnTick event"));
    }
    let list = env
        .get_field(event, "callbacks", "Ljava/util/ArrayList;")?
        .l()?;
    let count = env.call_method(&list, "size", "()I", &[])?.i()?;
    let mut owned = Vec::new();
    let mut foreign = Vec::new();
    for i in 0..count {
        let callback = env
            .call_method(&list, "get", "(I)Ljava/lang/Object;", &[JValue::Int(i)])?
            .l()?;
        let prototype = env
            .get_field(&callback, "prototype", "Lse/krka/kahlua/vm/Prototype;")?
            .l()?;
        let filename = env
            .get_field(prototype, "filename", "Ljava/lang/String;")?
            .l()?;
        let filename = text(env, &filename)?;
        if filename.ends_with("/LuaBuddyGrowth/42/media/lua/shared/LuaBuddyGrowth.lua") {
            owned.push(callback);
        } else if filename
            .ends_with("/LuaBuddyGrowthForeign/42/media/lua/shared/LuaBuddyGrowthForeign.lua")
        {
            foreign.push(callback);
        }
    }
    Ok((owned, foreign, count as usize))
}

fn objects(env: &mut JNIEnv<'_>) -> jni::errors::Result<Objects> {
    let (owned, foreign, _) = callbacks(env)?;
    if owned.len() != 1 || foreign.len() != 1 {
        return Err(jni::errors::Error::NullPtr(
            "Exactly one owned and one foreign native callback required",
        ));
    }
    let root = environment(env)?;
    let namespace = rawget(env, &root, "LuaBuddy")?;
    let module = rawget(env, &root, "LuaBuddyGrowth")?;
    Ok(Objects {
        owned: env
            .new_weak_ref(&owned[0])?
            .ok_or(jni::errors::Error::NullPtr("owned callback"))?,
        module: env
            .new_weak_ref(module)?
            .ok_or(jni::errors::Error::NullPtr("owned module"))?,
        namespace: env
            .new_weak_ref(namespace)?
            .ok_or(jni::errors::Error::NullPtr("runtime namespace"))?,
        foreign: env
            .new_weak_ref(&foreign[0])?
            .ok_or(jni::errors::Error::NullPtr("foreign callback"))?,
    })
}

fn identity(
    env: &mut JNIEnv<'_>,
    before: &Objects,
    after: &Objects,
    kind: Kind,
) -> jni::errors::Result<Value> {
    let old = before.owned.upgrade_local(env)?;
    let new = after.owned.upgrade_local(env)?;
    let same_owned = match (old, new) {
        (Some(a), Some(b)) => env.is_same_object(a, b)?,
        _ => false,
    };
    let old = before.foreign.upgrade_local(env)?;
    let new = after.foreign.upgrade_local(env)?;
    let same_foreign = match (old, new) {
        (Some(a), Some(b)) => env.is_same_object(a, b)?,
        _ => false,
    };
    let old = before.namespace.upgrade_local(env)?;
    let new = after.namespace.upgrade_local(env)?;
    let same_namespace = match (old, new) {
        (Some(a), Some(b)) => env.is_same_object(a, b)?,
        _ => false,
    };
    let old = before.module.upgrade_local(env)?;
    let new = after.module.upgrade_local(env)?;
    let same_module = match (old, new) {
        (Some(a), Some(b)) => env.is_same_object(a, b)?,
        _ => false,
    };
    if kind != Kind::Reconnect
        && (!same_foreign
            || !same_namespace
            || same_owned != (kind == Kind::Refresh)
            || same_module != (kind == Kind::Refresh))
    {
        return Err(jni::errors::Error::NullPtr(
            "Native callback or namespace identity contract failed",
        ));
    }
    if kind == Kind::Reconnect && (same_namespace || same_owned || same_foreign || same_module) {
        return Err(jni::errors::Error::NullPtr(
            "Reconnect retained an old active callback or namespace",
        ));
    }
    Ok(
        json!({"ownedCallbackSame":same_owned,"foreignCallbackSame":same_foreign,"ownedExportTableSame":same_module,"namespaceSame":same_namespace}),
    )
}

fn publish(mut value: Value, serial: usize) -> Result<(), String> {
    value["sequence"] = json!(serial);
    let config = CONFIG.get().ok_or("Missing configuration")?;
    let value = serde_json::to_string(&value).map_err(|e| e.to_string())?;
    fs::write(&config.progress, &value).map_err(|e| e.to_string())?;
    let mut state = STATE
        .get()
        .ok_or("Missing observer state")?
        .lock()
        .map_err(|_| "Observer lock poisoned")?;
    state.last_progress = value;
    Ok(())
}

fn key(serial: usize) -> Result<(), String> {
    publish(
        json!({"id":"Growth.open-pause","action":"key","key":"Escape","pid":std::process::id()}),
        serial,
    )
}

fn click(
    env: &mut JNIEnv<'_>,
    object: &JObject<'_>,
    serial: usize,
    name: &str,
) -> Result<bool, String> {
    if let Some(mut action) =
        button(env, object, "Growth.native-button").map_err(|e| e.to_string())?
    {
        action["name"] = json!(name);
        publish(action, serial)?;
        return Ok(true);
    }
    Ok(false)
}

fn native_field<'a>(
    env: &mut JNIEnv<'a>,
    table: &JObject<'_>,
    field: &str,
) -> Result<JObject<'a>, String> {
    rawget(env, table, field).map_err(|e| e.to_string())
}

fn main<'a>(env: &mut JNIEnv<'a>) -> Result<JObject<'a>, String> {
    let root = environment(env).map_err(|e| e.to_string())?;
    let class = native_field(env, &root, "MainScreen")?;
    native_field(env, &class, "instance")
}

fn options_modal<'a>(env: &mut JNIEnv<'a>) -> Result<JObject<'a>, String> {
    let root = environment(env).map_err(|e| e.to_string())?;
    let class = native_field(env, &root, "MainOptions")?;
    let options = native_field(env, &class, "instance")?;
    native_field(env, &options, "modal")
}

fn return_world(env: &mut JNIEnv<'_>, serial: usize) -> Result<bool, String> {
    let main = main(env)?;
    let modal = options_modal(env)?;
    if !modal.is_null() && ui_visible(env, &modal).map_err(|e| e.to_string())? {
        let ok = native_field(env, &modal, "ok")?;
        if click(env, &ok, serial, "dismiss-result")? {
            return Ok(false);
        }
    }
    let options = native_field(env, &main, "mainOptions")?;
    if !options.is_null() && ui_visible(env, &options).map_err(|e| e.to_string())? {
        let back = native_field(env, &options, "backButton")?;
        click(env, &back, serial, "options-back")?;
        return Ok(false);
    }
    if ui_visible(env, &main).map_err(|e| e.to_string())? {
        let resume = native_field(env, &main, "returnOption")?;
        click(env, &resume, serial, "resume")?;
        return Ok(false);
    }
    Ok(true)
}

fn option<'a>(env: &mut JNIEnv<'a>, id: &str) -> Result<JObject<'a>, String> {
    let root = environment(env).map_err(|e| e.to_string())?;
    let pz = native_field(env, &root, "PZAPI")?;
    let api = native_field(env, &pz, "ModOptions")?;
    let dict = native_field(env, &api, "Dict")?;
    let options = native_field(env, &dict, "LuaBuddy")?;
    let dict = native_field(env, &options, "dict")?;
    let option = native_field(env, &dict, id)?;
    native_field(env, &option, "element")
}

fn mods_tab(env: &mut JNIEnv<'_>, options: &JObject<'_>, serial: usize) -> Result<(), String> {
    let tabs = native_field(env, options, "tabs")?;
    let views = native_field(env, &tabs, "viewList")?;
    let java = native_field(env, &tabs, "javaObject")?;
    let x = env
        .call_method(&java, "getAbsoluteX", "()Ljava/lang/Double;", &[])
        .and_then(|v| v.l())
        .map_err(|e| e.to_string())?;
    let y = env
        .call_method(&java, "getAbsoluteY", "()Ljava/lang/Double;", &[])
        .and_then(|v| v.l())
        .map_err(|e| e.to_string())?;
    let origin = number(env, &x).map_err(|e| e.to_string())?;
    let y = number(env, &y).map_err(|e| e.to_string())?;
    let scroll = native_field(env, &tabs, "scrollX")?;
    let scroll = number(env, &scroll).map_err(|e| e.to_string())?;
    let max = native_field(env, &tabs, "maxLength")?;
    let max = number(env, &max).map_err(|e| e.to_string())?;
    let equal = native_field(env, &tabs, "equalTabWidth")?;
    let equal = !equal.is_null()
        && env
            .call_method(equal, "booleanValue", "()Z", &[])
            .and_then(|v| v.z())
            .map_err(|e| e.to_string())?;
    let height = native_field(env, &tabs, "tabHeight")?;
    let height = number(env, &height).map_err(|e| e.to_string())?;
    let target = option(env, "refresh")?;
    let mut records = Vec::new();
    for i in 1..=32 {
        let record = index(env, &views, i).map_err(|e| e.to_string())?;
        if record.is_null() {
            break;
        }
        let name = native_field(env, &record, "name")?;
        let name = text(env, &name).map_err(|e| e.to_string())?;
        let width = native_field(env, &record, "tabWidth")?;
        let width = if equal {
            max
        } else {
            number(env, &width).map_err(|e| e.to_string())?
        };
        let view = native_field(env, &record, "view")?;
        let mut parent = env.new_local_ref(&target).map_err(|e| e.to_string())?;
        let mut matched = false;
        for _ in 0..20 {
            if parent.is_null() {
                break;
            }
            if env
                .is_same_object(&parent, &view)
                .map_err(|e| e.to_string())?
            {
                matched = true;
                break;
            }
            parent = native_field(env, &parent, "parent")?;
        }
        records.push((name, width, matched));
    }
    let width = env
        .call_method(&java, "getWidth", "()Ljava/lang/Double;", &[])
        .and_then(|v| v.l())
        .map_err(|e| e.to_string())?;
    let width = number(env, &width).map_err(|e| e.to_string())?;
    let center = native_field(env, &tabs, "centerTabs")?;
    let center = !center.is_null()
        && env
            .call_method(center, "booleanValue", "()Z", &[])
            .and_then(|v| v.z())
            .map_err(|e| e.to_string())?;
    let total =
        records.iter().map(|(_, w, _)| w).sum::<f64>() + records.len().saturating_sub(1) as f64;
    let mut left = origin
        + if center && width >= total {
            (width - total) / 2.0
        } else {
            1.0 + scroll
        };
    let names = records
        .iter()
        .map(|(n, _, _)| n.clone())
        .collect::<Vec<_>>();
    for (_name, tab_width, matched) in records {
        if matched {
            let x = left + tab_width / 2.0;
            let y = y + height / 2.0;
            if total > width && (x >= origin + width - 22.0 || x < origin + 22.0) {
                let right = x >= origin + width - 22.0;
                return publish(
                    json!({"id":"Growth.native-button","name":if right {"mods-scroll-right"} else {"mods-scroll-left"},"action":"click","x":origin+if right {width-5.0} else {5.0},"y":y,"pid":std::process::id()}),
                    serial,
                );
            }
            if !(0.0..1280.0).contains(&x) || !(0.0..800.0).contains(&y) {
                return Err("Native Mods tab is outside the fixed viewport".into());
            }
            return publish(
                json!({"id":"Growth.native-button","name":"mods-tab","action":"click","x":x,"y":y,"pid":std::process::id()}),
                serial,
            );
        }
        left += tab_width + 1.0;
    }
    Err(format!(
        "Refresh button has no matching native tab pane; observed tabs: {names:?}"
    ))
}

fn operate(env: &mut JNIEnv<'_>, kind: Kind, serial: usize) -> Result<(), String> {
    let main = main(env)?;
    if kind == Kind::Reconnect {
        let in_game = native_field(env, &main, "inGame")?;
        let in_game = !in_game.is_null()
            && env
                .call_method(in_game, "booleanValue", "()Z", &[])
                .and_then(|v| v.z())
                .map_err(|e| e.to_string())?;
        if in_game {
            let window = GAME_WINDOW.get().ok_or("Missing prepared GameWindow")?;
            let class: &jni::objects::JClass<'_> = window.as_obj().into();
            let states = env
                .get_static_field(class, "states", "Lzombie/gameStates/GameStateMachine;")
                .and_then(|v| v.l())
                .map_err(|e| e.to_string())?;
            if states.is_null() {
                return Ok(());
            }
            let current = env
                .get_field(states, "current", "Lzombie/gameStates/GameState;")
                .and_then(|v| v.l())
                .map_err(|e| e.to_string())?;
            if current.is_null() {
                return Ok(());
            }
            let class = env.get_object_class(current).map_err(|e| e.to_string())?;
            let name = env
                .call_method(class, "getName", "()Ljava/lang/String;", &[])
                .and_then(|v| v.l())
                .map_err(|e| e.to_string())?;
            if text(env, &name).map_err(|e| e.to_string())? != "zombie.gameStates.IngameState" {
                return Ok(());
            }
            let options = native_field(env, &main, "mainOptions")?;
            if !options.is_null() && ui_visible(env, &options).map_err(|e| e.to_string())? {
                let back = native_field(env, &options, "backButton")?;
                click(env, &back, serial, "options-back")?;
                return Ok(());
            }
            let exit = native_field(env, &main, "exitOption")?;
            if !click(env, &exit, serial, "leave-world")? {
                key(serial)?;
            }
            return Ok(());
        }
        let multiplayer = native_field(env, &main, "multiplayer")?;
        if !multiplayer.is_null() && ui_visible(env, &multiplayer).map_err(|e| e.to_string())? {
            let list = native_field(env, &multiplayer, "accountList")?;
            let items = native_field(env, &list, "items")?;
            let expected = CONFIG
                .get()
                .and_then(|c| c.account.as_deref())
                .ok_or("Missing fixture identity")?;
            for i in 1..=32 {
                let record = index(env, &items, i).map_err(|e| e.to_string())?;
                if record.is_null() {
                    break;
                }
                let item = native_field(env, &record, "item")?;
                let account = native_field(env, &item, "account")?;
                if account.is_null() {
                    continue;
                }
                let name = env
                    .call_method(&account, "getUserName", "()Ljava/lang/String;", &[])
                    .and_then(|v| v.l())
                    .map_err(|e| e.to_string())?;
                if text(env, &name).map_err(|e| e.to_string())? == expected {
                    if !env
                        .call_method(&account, "isSavePwd", "()Z", &[])
                        .and_then(|v| v.z())
                        .map_err(|e| e.to_string())?
                    {
                        return Err("The fixed native Favorites fixture requires its generated local-test password to be saved".into());
                    }
                    let connect = native_field(env, &item, "connectButton")?;
                    click(env, &connect, serial, "reconnect-account")?;
                    return Ok(());
                }
            }
            return Err("The primed native Favorites list lacks the exact fixture account".into());
        }
        let delay = native_field(env, &main, "delay")?;
        if number(env, &delay).map_err(|e| e.to_string())? > 0.0 {
            return Ok(());
        }
        let online = native_field(env, &main, "onlineOption")?;
        click(env, &online, serial, "multiplayer")?;
        return Ok(());
    }
    let modal = options_modal(env)?;
    if !modal.is_null() && ui_visible(env, &modal).map_err(|e| e.to_string())? {
        let entry = native_field(env, &modal, "entry")?;
        if !entry.is_null() {
            let java = native_field(env, &entry, "javaObject")?;
            let value = env
                .call_method(java, "getText", "()Ljava/lang/String;", &[])
                .and_then(|v| v.l())
                .map_err(|e| e.to_string())?;
            if text(env, &value).map_err(|e| e.to_string())? != "LuaBuddyGrowth" {
                if let Some(mut action) =
                    button(env, &entry, "Growth.profile-id").map_err(|e| e.to_string())?
                {
                    action["action"] = json!("profile-input");
                    publish(action, serial)?;
                }
            } else {
                let yes = native_field(env, &modal, "yes")?;
                click(env, &yes, serial, "submit-reload")?;
            }
        }
        return Ok(());
    }
    let options = native_field(env, &main, "mainOptions")?;
    if !options.is_null() && ui_visible(env, &options).map_err(|e| e.to_string())? {
        let element = option(env, kind.name())?;
        if !click(env, &element, serial, kind.name())? {
            mods_tab(env, &options, serial)?;
        }
        return Ok(());
    }
    let options = native_field(env, &main, "optionsOption")?;
    if !click(env, &options, serial, "open-options")? {
        key(serial)?;
    }
    Ok(())
}

pub(crate) fn reconnect(env: &mut JNIEnv<'_>, serial: usize) -> Result<(), String> {
    operate(env, Kind::Reconnect, serial)
}

fn now_ms() -> Result<f64, String> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_secs_f64()
        * 1000.0)
}

pub(crate) fn observe(env: &mut JNIEnv<'_>, jvmti: &crate::jvmti::Jvmti) -> Result<(), String> {
    let config = CONFIG.get().ok_or("Missing configuration")?;
    let storage = GROWTH.get_or_init(|| {
        Mutex::new(Growth {
            started: Instant::now(),
            phase: Phase::AwaitWorld,
            current: None,
            retired: Vec::new(),
            records: Vec::new(),
            position: None,
            settle_ticks: None,
            last_report: None,
        })
    });
    let mut growth = storage.lock().map_err(|_| "Growth lock poisoned")?;
    let now = Instant::now();
    let phase = std::mem::replace(&mut growth.phase, Phase::Done);
    let serial = growth.records.len() + 1;
    growth.phase = match phase {
        Phase::AwaitWorld => {
            if growth.started.elapsed() > Duration::from_secs(300) {
                return Err("Growth fixture did not reach the initial usable native world within300 seconds".into());
            }
            if let Readiness::Usable(ready) = readiness(env, jvmti, Mode::World, true)? {
                if ready.role.as_deref() != Some("user")
                    || ready.screen != [1280, 800]
                    || ready.active_native_mods != 0
                {
                    return Err("Growth requires the ordinary fixed native scene".into());
                }
                growth.current = Some(objects(env).map_err(|e| e.to_string())?);
                growth.position = ready.player;
                let before = report()?;
                eprintln!(
                    "[LuaBuddy performance] growth world ready; initial warmup {} seconds",
                    config.warmup_seconds
                );
                Phase::Warmup { at: now, before }
            } else {
                Phase::AwaitWorld
            }
        }
        Phase::Warmup { at, before } => {
            if now.duration_since(at) < Duration::from_secs(config.warmup_seconds) {
                Phase::Warmup { at, before }
            } else {
                Phase::Operate {
                    kind: Kind::Refresh,
                    cycle: 0,
                    before,
                    started: now,
                }
            }
        }
        Phase::Operate {
            kind,
            cycle,
            before,
            started,
        } => {
            if started.elapsed() > Duration::from_secs(240) {
                return Err(format!(
                    "Native growth {} cycle {cycle} exceeded its bounded deadline",
                    kind.name()
                ));
            }
            let current = report().ok();
            let complete = current.as_ref().is_some_and(|r| {
                if kind == Kind::Reconnect {
                    r["session"] != before["session"] && r["operation"] == "startup"
                } else {
                    r["startedAt"] != before["startedAt"] && r["operation"] == kind.name()
                }
            });
            if complete {
                let current = current.ok_or("Missing completed report")?;
                growth.last_report = Some(current.clone());
                validate_transition(&before, &current, kind)?;
                if return_world(env, serial)?
                    && matches!(
                        readiness(env, jvmti, Mode::World, true)?,
                        Readiness::Usable(_)
                    )
                {
                    let after = objects(env).map_err(|e| e.to_string())?;
                    let before_objects =
                        growth.current.take().ok_or("Missing live native objects")?;
                    let identities =
                        identity(env, &before_objects, &after, kind).map_err(|e| e.to_string())?;
                    if kind != Kind::Refresh {
                        growth
                            .retired
                            .extend([before_objects.owned, before_objects.module]);
                        if kind == Kind::Reconnect {
                            growth.retired.push(before_objects.namespace);
                            growth.retired.push(before_objects.foreign);
                        }
                    }
                    growth.current = Some(after);
                    let mut current = current;
                    current["verificationIdentity"] = identities;
                    Phase::Gc {
                        kind,
                        cycle,
                        report: current,
                    }
                } else {
                    Phase::Operate {
                        kind,
                        cycle,
                        before,
                        started,
                    }
                }
            } else {
                if let Some(window) = GAME_WINDOW.get()
                    && initialized(jvmti, window)?
                {
                    operate(env, kind, serial)?;
                }
                Phase::Operate {
                    kind,
                    cycle,
                    before,
                    started,
                }
            }
        }
        Phase::Gc {
            kind,
            cycle,
            report,
        } => {
            growth.settle_ticks = Some(ticks(env)?);
            env.call_static_method("java/lang/System", "gc", "()V", &[])
                .map_err(|e| e.to_string())?;
            Phase::Settle {
                kind,
                cycle,
                at: now,
                report,
            }
        }
        Phase::Settle {
            kind,
            cycle,
            at,
            report,
        } => {
            if now.duration_since(at) >= SETTLE {
                Phase::Sample {
                    kind,
                    cycle,
                    at: now,
                    report,
                    rss: Vec::new(),
                    last: now - Duration::from_secs(1),
                }
            } else {
                Phase::Settle {
                    kind,
                    cycle,
                    at,
                    report,
                }
            }
        }
        Phase::Sample {
            kind,
            cycle,
            at,
            report,
            rss: mut samples,
            mut last,
        } => {
            if now.duration_since(last) >= Duration::from_millis(250) {
                samples.push(rss()?);
                last = now;
            }
            if now.duration_since(at) < SAMPLE {
                Phase::Sample {
                    kind,
                    cycle,
                    at,
                    report,
                    rss: samples,
                    last,
                }
            } else {
                let Readiness::Usable(ready) = readiness(env, jvmti, Mode::World, true)? else {
                    return Err("Native world was not usable during settled growth sampling".into());
                };
                if ready.player != growth.position
                    || ready.role.as_deref() != Some("user")
                    || ready.screen != [1280, 800]
                    || ready.active_native_mods != 0
                {
                    return Err("Fixed ordinary growth scene changed".into());
                }
                let (owned, foreign, total) = callbacks(env).map_err(|e| e.to_string())?;
                if owned.len() != 1 || foreign.len() != 1 {
                    return Err(
                        "Native owned or foreign callbacks grew beyond one live fixture callback"
                            .into(),
                    );
                }
                let retired_live = growth
                    .retired
                    .iter()
                    .map(|w| w.upgrade_local(env).map(|v| usize::from(v.is_some())))
                    .collect::<jni::errors::Result<Vec<_>>>()
                    .map_err(|e| e.to_string())?
                    .iter()
                    .sum::<usize>();
                let counts = ticks(env)?;
                let baseline = growth
                    .settle_ticks
                    .ok_or("Missing post-operation tick baseline")?;
                validate_work(baseline, counts)?;
                let ticks = counts.owned;
                let foreign_ticks = counts.foreign;
                let snapshot = json!({"kind":kind.name(),"cycle":cycle,"warmup":cycle==0,"unixMs":now_ms()?,"rssBytes":samples,"heap":heap(env).map_err(|e|e.to_string())?,"ownedCallbacks":owned.len(),"foreignCallbacks":foreign.len(),"totalNativeTickCallbacks":total,"retiredWeakReferences":growth.retired.len(),"retiredWeakReferencesStillLive":retired_live,"fixtureTicks":ticks,"foreignFixtureTicks":foreign_ticks,"postResumeBeforeGcTicks":baseline,"nativeState":ready,"nativeReport":report});
                growth.records.push(snapshot);
                growth.settle_ticks = None;
                let partial = config.output.with_extension("growth-progress.json");
                crate::publish_json(&partial, &json!(growth.records))?;
                eprintln!(
                    "[LuaBuddy performance] growth {} cycle {cycle} captured; retired_live={retired_live}",
                    kind.name()
                );
                if cycle + 1 < kind.count() {
                    Phase::Operate {
                        kind,
                        cycle: cycle + 1,
                        before: report,
                        started: now,
                    }
                } else if kind != Kind::Reconnect {
                    Phase::Operate {
                        kind: if kind == Kind::Refresh {
                            Kind::Reload
                        } else {
                            Kind::Reconnect
                        },
                        cycle: 0,
                        before: report,
                        started: now,
                    }
                } else {
                    let result = json!({"schema":1,"pid":std::process::id(),"workload":"fixed two-profile native lifecycle","policy":{"initialWarmupSeconds":config.warmup_seconds,"refreshCycles":11,"reloadCycles":11,"reconnectCycles":6,"discardFirstCycleOfEachKind":true,"gc":"one System.gc request after each completed operation","settleSeconds":SETTLE.as_secs(),"rssSampleSeconds":SAMPLE.as_secs(),"rssSampleIntervalMs":250},"referenceScope":"OnTick callback objects and prototype filenames; weak references to replaced fixture callback/module and retired runtime namespace/foreign callback; callback identity uses temporary local upgrades cleared before later-poll GC","records":growth.records,"error":null});
                    crate::publish_json(&config.output, &result)?;
                    FINISHED.store(true, Ordering::Release);
                    Phase::Done
                }
            }
        }
        Phase::Done => Phase::Done,
    };
    Ok(())
}

pub(crate) fn failure(error: &str) {
    if let Some(config) = CONFIG.get() {
        let (records, last_report) = GROWTH.get().and_then(|s| s.lock().ok()).map_or_else(
            || (Vec::new(), None),
            |s| (s.records.clone(), s.last_report.clone()),
        );
        let value = json!({"schema":1,"error":error,"records":records,"lastNativeReport":last_report,"complete":false});
        let _ = crate::publish_json(&config.output, &value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn native_report(session: &str, owned: u64, foreign: u64, target: Option<&str>) -> Value {
        json!({"session":session,"target":target,"mods":{"LuaBuddyGrowth":{"generation":owned},"LuaBuddyGrowthForeign":{"generation":foreign}}})
    }

    #[test]
    fn shared_generations_and_skipped_native_sessions_are_valid_without_foreign_mutation() {
        let before = native_report("fixture-process-2", 1, 2, None);
        let reload = native_report("fixture-process-2", 3, 2, Some("LuaBuddyGrowth"));
        assert!(validate_transition(&before, &reload, Kind::Reload).is_ok());
        assert!(validate_transition(&before, &before, Kind::Refresh).is_ok());
        let reconnected = native_report("fixture-process-5", 1, 2, None);
        assert!(validate_transition(&reload, &reconnected, Kind::Reconnect).is_ok());
        assert!(validate_transition(&before, &before, Kind::Reconnect).is_err());
        let foreign_changed = native_report("fixture-process-2", 3, 4, Some("LuaBuddyGrowth"));
        assert!(validate_transition(&before, &foreign_changed, Kind::Reload).is_err());
    }

    #[test]
    fn positive_stale_tick_totals_do_not_prove_post_operation_work() {
        let baseline = TickCounts {
            owned: 100.0,
            foreign: 200.0,
        };
        assert!(validate_work(baseline, baseline).is_err());
        assert!(
            validate_work(
                baseline,
                TickCounts {
                    owned: 101.0,
                    foreign: 200.0
                }
            )
            .is_err()
        );
        assert!(
            validate_work(
                baseline,
                TickCounts {
                    owned: 101.0,
                    foreign: 201.0
                }
            )
            .is_ok()
        );
        assert!(
            validate_work(
                TickCounts {
                    owned: 0.0,
                    foreign: 0.0
                },
                TickCounts {
                    owned: 1.0,
                    foreign: 1.0
                }
            )
            .is_ok()
        );
    }
}
