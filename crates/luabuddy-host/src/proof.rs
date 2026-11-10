use crate::jvmti::{CallbackEnv, Jvmti};
use jni::JNIEnv;
use jni::objects::GlobalRef;
use jni::objects::{JObject, JString, JValue};
use jni::sys::{JNIEnv as RawJni, JavaVM, jmethodID, jobject};
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::{AtomicUsize, Ordering};

static CALLBACKS: AtomicUsize = AtomicUsize::new(0);
static NATIVE: AtomicBool = AtomicBool::new(false);
static EVENTS: Mutex<Vec<(String, bool)>> = Mutex::new(Vec::new());
static UI_ACTIONS: AtomicUsize = AtomicUsize::new(0);
static NESTED: Mutex<Option<(GlobalRef, GlobalRef)>> = Mutex::new(None);

/// # Safety
/// The VM pointer comes from Agent_OnLoad and outlives all callbacks.
pub(crate) unsafe fn start(vm: *mut JavaVM, native: bool) -> Result<(), String> {
    // SAFETY: Agent_OnLoad supplies the live VM invocation interface.
    let jvmti = unsafe { Jvmti::from_vm(vm) }?;
    jvmti.enable_proof_capabilities()?;
    NATIVE.store(native, Ordering::SeqCst);
    if native {
        jvmti.set_callbacks_full(
            vm_init,
            breakpoint,
            Some(crate::bridge::class_load),
            Some(crate::bridge::class_prepare),
        )
    } else {
        jvmti.set_callbacks(vm_init, breakpoint)
    }
}

unsafe extern "system" fn vm_init(raw: CallbackEnv, jni: *mut RawJni, _thread: jobject) {
    let outcome = std::panic::catch_unwind(|| {
        // SAFETY: The JVM provides the valid callback-local environments.
        let mut env = unsafe { JNIEnv::from_raw(jni) }.map_err(|e| e.to_string())?;
        // SAFETY: raw belongs to this live JVMTI callback.
        let jvmti = unsafe { Jvmti::from_raw(raw) };
        match run(&mut env, &jvmti) {
            Ok(()) => {
                eprintln!(
                    "[LuaBuddy native proof] PASS Rust callable, identity, arguments, return value, cleanup"
                );
                Ok(())
            }
            Err(error) => {
                if env.exception_check().unwrap_or(false) {
                    let _ = env.exception_describe();
                    let _ = env.exception_clear();
                }
                Err(error)
            }
        }
    });
    match outcome {
        Ok(Ok(())) => (),
        Ok(Err(error)) => {
            eprintln!("[LuaBuddy native proof] FAIL {error}");
            std::process::exit(1);
        }
        Err(_) => {
            eprintln!("[LuaBuddy native proof] FAIL panic contained in VMInit");
            std::process::exit(1);
        }
    }
}

fn run(env: &mut JNIEnv<'_>, jvmti: &Jvmti) -> Result<(), String> {
    run_jni(env, jvmti).map_err(|error| format!("callable proof: {error}"))
}

fn run_jni(env: &mut JNIEnv<'_>, jvmti: &Jvmti) -> Result<(), Box<dyn std::error::Error>> {
    if NATIVE.load(Ordering::SeqCst) {
        crate::bridge::expect_system_loader(env)?;
    }
    env.find_class("zombie/Lua/LuaEventManager")?;
    if NATIVE.load(Ordering::SeqCst) {
        if !crate::bridge::ready() {
            return Err("native declaration bridge was not registered".into());
        }
    } else {
        crate::callables::prepare(env, jvmti)?;
    }
    let callable = crate::callables::create(env, "proof", 1, 0, echo)?;
    let platform = env.new_object("se/krka/kahlua/j2se/J2SEPlatform", "()V", &[])?;
    let table = env
        .call_method(
            &platform,
            "newEnvironment",
            "()Lse/krka/kahlua/vm/KahluaTable;",
            &[],
        )?
        .l()?;
    let thread = env.new_object(
        "se/krka/kahlua/vm/KahluaThread",
        "(Lse/krka/kahlua/vm/Platform;Lse/krka/kahlua/vm/KahluaTable;)V",
        &[JValue::Object(&platform), JValue::Object(&table)],
    )?;
    let owner = env
        .call_static_method(
            "java/lang/Thread",
            "currentThread",
            "()Ljava/lang/Thread;",
            &[],
        )?
        .l()?;
    env.set_field(
        &thread,
        "debugOwnerThread",
        "Ljava/lang/Thread;",
        JValue::Object(&owner),
    )?;
    let argument = env.new_string("rust-callable-identity")?;
    let arguments = env.new_object_array(1, "java/lang/Object", JObject::null())?;
    env.set_object_array_element(&arguments, 0, &argument)?;
    for _ in 0..3 {
        let result = env
            .call_method(
                &thread,
                "call",
                "(Ljava/lang/Object;[Ljava/lang/Object;)Ljava/lang/Object;",
                &[
                    JValue::Object(callable.as_obj()),
                    JValue::Object(arguments.as_ref()),
                ],
            )?
            .l()?;
        if !env.is_same_object(&result, &argument)? {
            return Err("callback did not preserve argument identity".into());
        }
        let string = JString::from(result);
        if env.get_string(&string)?.to_str()? != "rust-callable-identity" {
            return Err("callback value changed".into());
        }
    }
    if CALLBACKS.load(Ordering::SeqCst) != 3 {
        return Err("callback count does not match repeated invocation".into());
    }
    *NESTED
        .lock()
        .map_err(|_| "nested callback context poisoned")? =
        Some((env.new_global_ref(&thread)?, callable.clone()));
    let nested_callable = crate::callables::create(env, "proof", 1, 0, nested)?;
    let nested_result = env
        .call_method(
            &thread,
            "call",
            "(Ljava/lang/Object;[Ljava/lang/Object;)Ljava/lang/Object;",
            &[
                JValue::Object(nested_callable.as_obj()),
                JValue::Object(arguments.as_ref()),
            ],
        )?
        .l()?;
    if !env.is_same_object(&nested_result, &argument)? || CALLBACKS.load(Ordering::SeqCst) != 4 {
        return Err("nested Rust callable did not preserve invocation/identity".into());
    }
    *NESTED
        .lock()
        .map_err(|_| "nested callback context poisoned")? = None;
    eprintln!(
        "[LuaBuddy native proof] PASS reentrant Rust/Kahlua/Rust callable without registry lock"
    );
    if NATIVE.load(Ordering::SeqCst) {
        let failing = crate::callables::create(env, "proof", 1, 0, fail)?;
        let result = env
            .call_method(
                &thread,
                "pcall",
                "(Ljava/lang/Object;[Ljava/lang/Object;)[Ljava/lang/Object;",
                &[
                    JValue::Object(failing.as_obj()),
                    JValue::Object(arguments.as_ref()),
                ],
            )?
            .l()?;
        let results = jni::objects::JObjectArray::from(result);
        let success = env.get_object_array_element(&results, 0)?;
        if env.call_method(success, "booleanValue", "()Z", &[])?.z()? {
            return Err("Rust callback failure became Lua success".into());
        }
        let message = JString::from(env.get_object_array_element(&results, 1)?);
        if !env
            .get_string(&message)?
            .to_string_lossy()
            .contains("intentional Rust callable failure")
        {
            return Err("Rust callback failure lost its reason".into());
        }
        eprintln!(
            "[LuaBuddy native proof] PASS native callback exception through actual Kahlua pcall"
        );
    }
    let removed = crate::callables::unregister("proof", 1)?;
    if removed != if NATIVE.load(Ordering::SeqCst) { 3 } else { 2 } {
        return Err("owned callback cleanup count mismatch".into());
    }
    let retired = env
        .call_method(
            &thread,
            "call",
            "(Ljava/lang/Object;[Ljava/lang/Object;)Ljava/lang/Object;",
            &[
                JValue::Object(callable.as_obj()),
                JValue::Object(arguments.as_ref()),
            ],
        )?
        .l()?;
    if !retired.is_null() || CALLBACKS.load(Ordering::SeqCst) != 4 {
        return Err("retired callback executed".into());
    }
    event_proof(env, jvmti, &table, &thread)?;
    ui_proof(env)?;
    download_notification_proof(env, jvmti)?;
    let core = env
        .call_static_method(
            "zombie/core/Core",
            "getInstance",
            "()Lzombie/core/Core;",
            &[],
        )?
        .l()?;
    let version = JString::from(
        env.call_method(core, "getGameAndBuildVersion", "()Ljava/lang/String;", &[])?
            .l()?,
    );
    let version: String = env.get_string(&version)?.into();
    eprintln!("[LuaBuddy native proof] tested game {version}");
    Ok(())
}

fn download_notification_proof(
    env: &mut JNIEnv<'_>,
    jvmti: &Jvmti,
) -> Result<(), Box<dyn std::error::Error>> {
    use crate::steam::{ItemId, observed_download_result};
    use std::time::{Duration, Instant};
    crate::download_events::prepare(env, jvmti)?;
    let workshop = env
        .get_static_field(
            "zombie/core/znet/SteamWorkshop",
            "instance",
            "Lzombie/core/znet/SteamWorkshop;",
        )?
        .l()?;
    let item = ItemId::new(4_312_345_678)?;
    let started = Instant::now();
    env.call_method(
        &workshop,
        "onItemDownloaded",
        "(J)V",
        &[JValue::Long(item.get() as i64)],
    )?;
    if observed_download_result(item, started)? != Some(1) {
        return Err("Actual Workshop success callback did not reach Rust observation".into());
    }
    let failure = ItemId::new(4_312_345_679)?;
    env.call_method(
        &workshop,
        "onItemNotDownloaded",
        "(JI)V",
        &[JValue::Long(failure.get() as i64), JValue::Int(15)],
    )?;
    if observed_download_result(failure, started)? != Some(15) {
        return Err(
            "Actual Workshop failure callback lost its item identity or result code".into(),
        );
    }
    if observed_download_result(item, Instant::now() + Duration::from_secs(1))?.is_some() {
        return Err("A notification predating a new request was accepted".into());
    }
    eprintln!(
        "[LuaBuddy native proof] PASS actual Workshop completion callbacks, wide item identity, failure code, stale notification rejection; no account API called"
    );
    Ok(())
}

fn nested<'a>(
    env: &mut JNIEnv<'a>,
    frame: &JObject<'a>,
    count: i32,
    _context: u64,
) -> Result<i32, String> {
    if count != 1 {
        return Err("unexpected nested callable argument count".to_owned());
    }
    let (thread, callable) = NESTED
        .lock()
        .map_err(|_| "nested callback context poisoned")?
        .clone()
        .ok_or("nested callback context missing")?;
    let value = env
        .call_method(frame, "get", "(I)Ljava/lang/Object;", &[JValue::Int(0)])
        .and_then(|v| v.l())
        .map_err(|e| e.to_string())?;
    let arguments = env
        .new_object_array(1, "java/lang/Object", JObject::null())
        .map_err(|e| e.to_string())?;
    env.set_object_array_element(&arguments, 0, value)
        .map_err(|e| e.to_string())?;
    let result = env
        .call_method(
            thread.as_obj(),
            "call",
            "(Ljava/lang/Object;[Ljava/lang/Object;)Ljava/lang/Object;",
            &[
                JValue::Object(callable.as_obj()),
                JValue::Object(arguments.as_ref()),
            ],
        )
        .and_then(|v| v.l())
        .map_err(|e| e.to_string())?;
    env.call_method(
        frame,
        "push",
        "(Ljava/lang/Object;)I",
        &[JValue::Object(&result)],
    )
    .map_err(|e| e.to_string())?;
    Ok(1)
}

fn ui_proof(env: &mut JNIEnv<'_>) -> Result<(), Box<dyn std::error::Error>> {
    let platform = env.new_object("se/krka/kahlua/j2se/J2SEPlatform", "()V", &[])?;
    let table = env
        .call_method(
            platform,
            "newTable",
            "()Lse/krka/kahlua/vm/KahluaTable;",
            &[],
        )?
        .l()?;
    let callback = crate::callables::create(env, "proof-ui", 1, 0, ui_callback)?;
    let key = env.new_string("onMouseUp")?;
    env.call_method(
        &table,
        "rawset",
        "(Ljava/lang/Object;Ljava/lang/Object;)V",
        &[JValue::Object(&key), JValue::Object(callback.as_obj())],
    )?;
    let element = env.new_object(
        "zombie/ui/UIElement",
        "(Lse/krka/kahlua/vm/KahluaTable;)V",
        &[JValue::Object(&table)],
    )?;
    let result = env
        .call_method(
            element,
            "onMouseUp",
            "(DD)Ljava/lang/Boolean;",
            &[JValue::Double(12.5), JValue::Double(7.25)],
        )?
        .l()?;
    if !env.call_method(result, "booleanValue", "()Z", &[])?.z()?
        || UI_ACTIONS.load(Ordering::SeqCst) != 1
    {
        return Err("native UIElement action did not reach Rust callable".into());
    }
    crate::callables::unregister("proof-ui", 1)?;
    eprintln!("[LuaBuddy native proof] PASS actual UIElement mouse callback and return value");
    Ok(())
}

fn ui_callback<'a>(
    env: &mut JNIEnv<'a>,
    frame: &JObject<'a>,
    count: i32,
    _context: u64,
) -> Result<i32, String> {
    if count != 3 {
        return Err("UI callback argument count changed".to_owned());
    }
    let x = env
        .call_method(frame, "get", "(I)Ljava/lang/Object;", &[JValue::Int(1)])
        .and_then(|v| v.l())
        .map_err(|e| e.to_string())?;
    let x = env
        .call_method(x, "doubleValue", "()D", &[])
        .and_then(|v| v.d())
        .map_err(|e| e.to_string())?;
    let y = env
        .call_method(frame, "get", "(I)Ljava/lang/Object;", &[JValue::Int(2)])
        .and_then(|v| v.l())
        .map_err(|e| e.to_string())?;
    let y = env
        .call_method(y, "doubleValue", "()D", &[])
        .and_then(|v| v.d())
        .map_err(|e| e.to_string())?;
    if x != 12.5 || y != 7.25 {
        return Err("UI callback coordinates changed".to_owned());
    }
    let yes = env
        .get_static_field("java/lang/Boolean", "TRUE", "Ljava/lang/Boolean;")
        .and_then(|v| v.l())
        .map_err(|e| e.to_string())?;
    env.call_method(
        frame,
        "push",
        "(Ljava/lang/Object;)I",
        &[JValue::Object(&yes)],
    )
    .map_err(|e| e.to_string())?;
    UI_ACTIONS.fetch_add(1, Ordering::SeqCst);
    Ok(1)
}

fn event_proof(
    env: &mut JNIEnv<'_>,
    jvmti: &Jvmti,
    table: &JObject<'_>,
    thread: &JObject<'_>,
) -> Result<(), Box<dyn std::error::Error>> {
    env.set_static_field(
        "zombie/Lua/LuaManager",
        (
            "zombie/Lua/LuaManager",
            "env",
            "Lse/krka/kahlua/vm/KahluaTable;",
        ),
        JValue::Object(table),
    )?;
    env.set_static_field(
        "zombie/Lua/LuaManager",
        (
            "zombie/Lua/LuaManager",
            "thread",
            "Lse/krka/kahlua/vm/KahluaThread;",
        ),
        JValue::Object(thread),
    )?;
    let platform = env
        .get_static_field(
            "zombie/Lua/LuaManager",
            "platform",
            "Lse/krka/kahlua/j2se/J2SEPlatform;",
        )?
        .l()?;
    let events = env
        .call_method(
            platform,
            "newTable",
            "()Lse/krka/kahlua/vm/KahluaTable;",
            &[],
        )?
        .l()?;
    let events_key = env.new_string("Events")?;
    env.call_method(
        table,
        "rawset",
        "(Ljava/lang/Object;Ljava/lang/Object;)V",
        &[JValue::Object(&events_key), JValue::Object(&events)],
    )?;
    crate::events::prepare(env, jvmti, event_handler)?;
    let math_key = env.new_string("math")?;
    let math = env
        .call_method(
            table,
            "rawget",
            "(Ljava/lang/Object;)Ljava/lang/Object;",
            &[JValue::Object(&math_key)],
        )?
        .l()?;
    let max_key = env.new_string("max")?;
    let host_callback = env
        .call_method(
            math,
            "rawget",
            "(Ljava/lang/Object;)Ljava/lang/Object;",
            &[JValue::Object(&max_key)],
        )?
        .l()?;
    for name in [
        "LuaBuddyNativeOuter",
        "LuaBuddyNativeNested",
        "LuaBuddyNativeQueued",
    ] {
        let name = env.new_string(name)?;
        let event = env
            .call_static_method(
                "zombie/Lua/LuaEventManager",
                "AddEvent",
                "(Ljava/lang/String;)Lzombie/Lua/Event;",
                &[JValue::Object(&name)],
            )?
            .l()?;
        let callbacks = env
            .get_field(event, "callbacks", "Ljava/util/ArrayList;")?
            .l()?;
        env.call_method(
            callbacks,
            "add",
            "(Ljava/lang/Object;)Z",
            &[JValue::Object(&host_callback)],
        )?;
    }
    let outer = env.new_string("LuaBuddyNativeOuter")?;
    env.call_static_method(
        "zombie/Lua/LuaEventManager",
        "triggerEvent",
        "(Ljava/lang/String;)V",
        &[JValue::Object(&outer)],
    )?;
    let trace = EVENTS.lock().map_err(|_| "proof trace poisoned")?.clone();
    let expected = vec![
        ("LuaBuddyNativeOuter".to_owned(), true),
        ("LuaBuddyNativeNested".to_owned(), true),
        ("LuaBuddyNativeNested".to_owned(), false),
        ("LuaBuddyNativeOuter".to_owned(), false),
    ];
    if trace != expected {
        return Err(format!("nested dispatch trace mismatch: {trace:?}").into());
    }
    let vm = env.get_java_vm()?;
    std::thread::spawn(move || -> Result<(), String> {
        let mut env = vm.attach_current_thread().map_err(|e| e.to_string())?;
        let queued = env
            .new_string("LuaBuddyNativeQueued")
            .map_err(|e| e.to_string())?;
        env.call_static_method(
            "zombie/Lua/LuaEventManager",
            "triggerEvent",
            "(Ljava/lang/String;)V",
            &[JValue::Object(&queued)],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    })
    .join()
    .map_err(|_| "queued event producer panicked")??;
    if EVENTS.lock().map_err(|_| "proof trace poisoned")?.len() != 4 {
        return Err("queued event dispatched on producer instead of owner thread".into());
    }
    env.call_static_method("zombie/Lua/LuaEventManager", "RunQueuedEvents", "()V", &[])?;
    let trace = EVENTS.lock().map_err(|_| "proof trace poisoned")?.clone();
    if trace.len() != 6
        || trace[4..]
            != [
                ("LuaBuddyNativeQueued".to_owned(), true),
                ("LuaBuddyNativeQueued".to_owned(), false),
            ]
    {
        return Err(format!("queued dispatch trace mismatch: {trace:?}").into());
    }
    eprintln!(
        "[LuaBuddy native proof] PASS actual dispatch, nested events, worker queue, owner-thread drain"
    );
    Ok(())
}

fn event_handler(env: &mut JNIEnv<'_>, name: &str, enter: bool) -> Result<(), String> {
    EVENTS
        .lock()
        .map_err(|_| "proof trace poisoned")?
        .push((name.to_owned(), enter));
    if name == "LuaBuddyNativeOuter" && enter {
        let nested = env
            .new_string("LuaBuddyNativeNested")
            .map_err(|e| e.to_string())?;
        env.call_static_method(
            "zombie/Lua/LuaEventManager",
            "triggerEvent",
            "(Ljava/lang/String;)V",
            &[JValue::Object(&nested)],
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

unsafe extern "system" fn breakpoint(
    raw: CallbackEnv,
    jni: *mut RawJni,
    thread: jobject,
    _method: jmethodID,
    _location: i64,
) {
    let outcome = std::panic::catch_unwind(|| -> Result<(), String> {
        // SAFETY: The current breakpoint supplies the active environment and suspended long/int argument slots.
        if unsafe { crate::download_events::dispatch(raw, thread, _method) }? {
            return Ok(());
        }
        // SAFETY: All handles come from the JVM's current breakpoint callback.
        unsafe { crate::callables::dispatch(raw, jni, thread, _method) }?;
        // SAFETY: These are the same callback-local handles used by the callable router.
        unsafe { crate::events::dispatch(raw, jni, thread, _method, _location) }?;
        Ok(())
    });
    match outcome {
        Ok(Ok(())) => (),
        Ok(Err(error)) => eprintln!("[LuaBuddy native proof] callback failed: {error}"),
        Err(_) => eprintln!("[LuaBuddy native proof] callback panic contained"),
    }
}

fn echo<'a>(
    env: &mut JNIEnv<'a>,
    frame: &JObject<'a>,
    arguments: i32,
    _context: u64,
) -> Result<i32, String> {
    if arguments != 1 {
        return Err("unexpected callable argument count".to_owned());
    }
    let value = env
        .call_method(frame, "get", "(I)Ljava/lang/Object;", &[JValue::Int(0)])
        .and_then(|v| v.l())
        .map_err(|e| e.to_string())?;
    env.call_method(
        frame,
        "push",
        "(Ljava/lang/Object;)I",
        &[JValue::Object(&value)],
    )
    .map_err(|e| e.to_string())?;
    CALLBACKS.fetch_add(1, Ordering::SeqCst);
    Ok(1)
}

fn fail<'a>(
    _env: &mut JNIEnv<'a>,
    _frame: &JObject<'a>,
    _arguments: i32,
    _context: u64,
) -> Result<i32, String> {
    Err("intentional Rust callable failure".to_owned())
}
