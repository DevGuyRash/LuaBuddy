use crate::{CONFIG, FINISHED, Mode, Readiness, growth, jvmti::Jvmti, readiness};
use jni::{JNIEnv, objects::WeakRef};
use serde_json::{Value, json};
use std::{
    sync::{Mutex, OnceLock, atomic::Ordering},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

enum Phase {
    AwaitWorld,
    Warmup(Instant),
    Reconnect(Instant),
    Gc,
    Settle(Instant),
    Sample {
        at: Instant,
        last: Instant,
        rss: Vec<u64>,
    },
    Done,
}

struct Control {
    started: Instant,
    phase: Phase,
    current: Option<WeakRef>,
    retired: Vec<WeakRef>,
    position: Option<[f64; 3]>,
    first_frame: usize,
    records: Vec<Value>,
    saw_menu: bool,
}

static CONTROL: OnceLock<Mutex<Control>> = OnceLock::new();

pub(crate) fn sequence() -> usize {
    CONTROL
        .get()
        .and_then(|s| s.lock().ok())
        .map_or(1, |s| s.records.len() + 1)
}

fn current(env: &mut JNIEnv<'_>) -> Result<WeakRef, String> {
    let value = growth::environment(env).map_err(|e| e.to_string())?;
    env.new_weak_ref(value)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "Missing native Lua environment".into())
}

fn same(env: &mut JNIEnv<'_>, before: &WeakRef, after: &WeakRef) -> Result<bool, String> {
    match (
        before.upgrade_local(env).map_err(|e| e.to_string())?,
        after.upgrade_local(env).map_err(|e| e.to_string())?,
    ) {
        (Some(a), Some(b)) => env.is_same_object(a, b).map_err(|e| e.to_string()),
        _ => Ok(false),
    }
}

fn in_game(env: &mut JNIEnv<'_>) -> Result<Option<bool>, String> {
    let root = growth::environment(env).map_err(|e| e.to_string())?;
    let class = crate::rawget(env, &root, "MainScreen").map_err(|e| e.to_string())?;
    let main = crate::rawget(env, &class, "instance").map_err(|e| e.to_string())?;
    let flag = crate::rawget(env, &main, "inGame").map_err(|e| e.to_string())?;
    if flag.is_null() {
        Ok(None)
    } else {
        env.call_method(flag, "booleanValue", "()Z", &[])
            .and_then(|v| v.z())
            .map(Some)
            .map_err(|e| e.to_string())
    }
}

pub(crate) fn observe(env: &mut JNIEnv<'_>, jvmti: &Jvmti) -> Result<(), String> {
    let config = CONFIG.get().ok_or("Missing native control configuration")?;
    let mut state = CONTROL
        .get_or_init(|| {
            Mutex::new(Control {
                started: Instant::now(),
                phase: Phase::AwaitWorld,
                current: None,
                retired: Vec::new(),
                position: None,
                first_frame: 0,
                records: Vec::new(),
                saw_menu: false,
            })
        })
        .lock()
        .map_err(|_| "Native control lock poisoned")?;
    let now = Instant::now();
    let cycle = state.records.len();
    let phase = std::mem::replace(&mut state.phase, Phase::Done);
    state.phase = match phase {
        Phase::AwaitWorld => {
            if state.started.elapsed() > Duration::from_secs(300) {
                return Err(
                    "Native-only control did not reach the initial world within300 seconds".into(),
                );
            }
            if let Readiness::Usable(ready) = readiness(env, jvmti, Mode::World, false)? {
                if in_game(env)? != Some(true) {
                    state.phase = Phase::AwaitWorld;
                    return Ok(());
                }
                if ready.role.as_deref() != Some("user")
                    || ready.screen != [1280, 800]
                    || ready.active_native_mods != 0
                    || ready.selected_profiles != 0
                    || ready.runtime_present
                {
                    return Err(
                        "Native-only control fixture differs from the fixed ordinary scene".into(),
                    );
                }
                state.position = ready.player;
                state.current = Some(current(env)?);
                eprintln!(
                    "[LuaBuddy performance] native-only world ready; initial warmup {} seconds",
                    config.warmup_seconds
                );
                Phase::Warmup(now)
            } else {
                Phase::AwaitWorld
            }
        }
        Phase::Warmup(at) => {
            if now.duration_since(at) < Duration::from_secs(config.warmup_seconds) {
                Phase::Warmup(at)
            } else {
                Phase::Reconnect(now)
            }
        }
        Phase::Reconnect(started) => {
            if started.elapsed() > Duration::from_secs(240) {
                return Err(format!(
                    "Native-only reconnect cycle {cycle} exceeded its bounded deadline"
                ));
            }
            let mut completed = false;
            let active = in_game(env)?;
            state.saw_menu |= active == Some(false);
            if let Readiness::Usable(ready) = readiness(env, jvmti, Mode::World, false)? {
                let after = current(env)?;
                let before = state
                    .current
                    .as_ref()
                    .ok_or("Missing prior native environment")?;
                if state.saw_menu && active == Some(true) && !same(env, before, &after)? {
                    if ready.player != state.position
                        || ready.role.as_deref() != Some("user")
                        || ready.screen != [1280, 800]
                        || ready.active_native_mods != 0
                        || ready.selected_profiles != 0
                        || ready.runtime_present
                    {
                        return Err("Native-only reconnect changed the fixed ordinary scene".into());
                    }
                    let before = state
                        .current
                        .replace(after)
                        .ok_or("Missing retired native environment")?;
                    state.retired.push(before);
                    completed = true;
                }
            }
            if completed {
                Phase::Gc
            } else {
                growth::reconnect(env, cycle + 1)?;
                Phase::Reconnect(started)
            }
        }
        Phase::Gc => {
            state.first_frame = crate::hud_values(&config.hud)?.len();
            env.call_static_method("java/lang/System", "gc", "()V", &[])
                .map_err(|e| e.to_string())?;
            Phase::Settle(now)
        }
        Phase::Settle(at) => {
            if now.duration_since(at) < Duration::from_secs(5) {
                Phase::Settle(at)
            } else {
                Phase::Sample {
                    at: now,
                    last: now - Duration::from_secs(1),
                    rss: Vec::new(),
                }
            }
        }
        Phase::Sample {
            at,
            mut last,
            mut rss,
        } => {
            if now.duration_since(last) >= Duration::from_millis(250) {
                rss.push(growth::rss()?);
                last = now;
            }
            if now.duration_since(at) < Duration::from_secs(2) {
                Phase::Sample { at, last, rss }
            } else {
                let Readiness::Usable(ready) = readiness(env, jvmti, Mode::World, false)? else {
                    return Err("Native-only world was not usable after settling".into());
                };
                if ready.player != state.position
                    || in_game(env)? != Some(true)
                    || ready.role.as_deref() != Some("user")
                    || ready.screen != [1280, 800]
                    || ready.active_native_mods != 0
                    || ready.selected_profiles != 0
                    || ready.runtime_present
                {
                    return Err("Native-only settled scene changed".into());
                }
                let last_frame = crate::hud_values(&config.hud)?.len();
                if last_frame <= state.first_frame {
                    return Err(
                        "No real renderer work completed after native-only reconnect".into(),
                    );
                }
                let retained = state
                    .retired
                    .iter()
                    .map(|w| w.upgrade_local(env).map(|r| usize::from(r.is_some())))
                    .collect::<jni::errors::Result<Vec<_>>>()
                    .map_err(|e| e.to_string())?
                    .iter()
                    .sum::<usize>();
                let record = json!({"kind":"reconnect","cycle":cycle,"warmup":cycle==0,"unixMs":SystemTime::now().duration_since(UNIX_EPOCH).map_err(|e|e.to_string())?.as_secs_f64()*1000.0,"rssBytes":rss,"heap":growth::heap(env).map_err(|e|e.to_string())?,"retiredWeakReferences":state.retired.len(),"retiredWeakReferencesStillLive":retained,"nativeMenuObserved":state.saw_menu,"nativeEnvironmentChanged":true,"firstFrameIndex":state.first_frame,"lastFrameIndex":last_frame,"completedWorldFrames":last_frame-state.first_frame,"nativeState":ready});
                state.records.push(record);
                state.saw_menu = false;
                crate::publish_json(
                    &config.output.with_extension("growth-progress.json"),
                    &json!(state.records),
                )?;
                eprintln!(
                    "[LuaBuddy performance] native-only reconnect cycle {cycle} captured; retired_environment_live={retained}"
                );
                if cycle + 1 < 6 {
                    Phase::Reconnect(now)
                } else {
                    let result = json!({"schema":1,"pid":std::process::id(),"nativeOnly":true,"workload":"native-only reconnect diagnostic control","policy":{"initialWarmupSeconds":config.warmup_seconds,"reconnectCycles":6,"discardFirstCycle":true,"gc":"one System.gc request after each completed reconnect and world resume","settleSeconds":5,"rssSampleSeconds":2,"rssSampleIntervalMs":250},"referenceScope":"weak native LuaManager.env references, with temporary identity upgrades cleared before later-poll GC; no LuaBuddy runtime or test profiles","historyLimit":"does not repeat the enabled run's preceding Refresh/Reload UI history","records":state.records,"error":null});
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
        let records = CONTROL
            .get()
            .and_then(|s| s.lock().ok())
            .map_or_else(Vec::new, |s| s.records.clone());
        let _ = crate::publish_json(
            &config.output,
            &json!({"schema":1,"nativeOnly":true,"complete":false,"error":error,"records":records}),
        );
    }
}
