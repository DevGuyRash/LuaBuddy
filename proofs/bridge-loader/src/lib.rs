#![deny(unsafe_op_in_unsafe_fn)]
#![allow(dead_code)]

#[path = "../../../crates/luabuddy-host/src/bridge.rs"]
mod bridge;
#[path = "../../../crates/luabuddy-host/src/callables.rs"]
mod callables;
#[path = "../../../crates/luabuddy-host/src/classfile.rs"]
mod classfile;
#[path = "../../../crates/luabuddy-host/src/events.rs"]
mod events;
#[path = "../../../crates/luabuddy-host/src/jvmti.rs"]
mod jvmti;

use jni::JNIEnv;
use jni::objects::{GlobalRef, JClass, JObject, JString, JValue};
use jni::sys::{JNIEnv as RawJni, JavaVM, jclass, jmethodID, jobject};
use std::ffi::{CStr, c_char, c_void};
use std::sync::OnceLock;

static SYSTEM: OnceLock<GlobalRef> = OnceLock::new();
static SECOND: OnceLock<GlobalRef> = OnceLock::new();
static SECOND_CLASS: OnceLock<GlobalRef> = OnceLock::new();

/// # Safety
/// A disposable JVM supplies its live invocation interface for this proof only.
// SAFETY: The JVM resolves the agent entrypoint using the standard invocation ABI.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn Agent_OnLoad(
    vm: *mut JavaVM,
    _options: *mut c_char,
    _reserved: *mut c_void,
) -> i32 {
    let result = std::panic::catch_unwind(|| -> Result<(), String> {
        if vm.is_null() {
            return Err("Null proof JVM".into());
        }
        // SAFETY: Agent_OnLoad received the active JVM invocation pointer.
        let jvmti = unsafe { jvmti::Jvmti::from_vm(vm) }?;
        jvmti.enable_proof_capabilities()?;
        jvmti.set_callbacks_full(init, breakpoint, Some(load), Some(bridge::class_prepare))
    });
    match result {
        Ok(Ok(())) => 0,
        _ => {
            eprintln!("[LuaBuddy bridge loader proof] FAIL initialization");
            -1
        }
    }
}

unsafe extern "system" fn breakpoint(
    _raw: jvmti::CallbackEnv,
    _jni: *mut RawJni,
    _thread: jobject,
    _method: jmethodID,
    _location: i64,
) {
}

unsafe extern "system" fn init(raw: jvmti::CallbackEnv, jni: *mut RawJni, _thread: jobject) {
    let result = std::panic::catch_unwind(|| -> Result<(), String> {
        // SAFETY: VMInit supplies callback-local JNI and JVMTI environments.
        let mut env = unsafe { JNIEnv::from_raw(jni) }.map_err(|e| e.to_string())?;
        // SAFETY: The callback owns the live JVMTI environment.
        let jvmti = unsafe { jvmti::Jvmti::from_raw(raw) };
        run(&mut env, &jvmti).map_err(|e| e.to_string())
    });
    match result {
        Ok(Ok(())) => eprintln!(
            "[LuaBuddy bridge loader proof] PASS untransformed second loader preserved during native registration"
        ),
        error => {
            eprintln!("[LuaBuddy bridge loader proof] FAIL {error:?}");
            std::process::exit(1);
        }
    }
}

fn run(env: &mut JNIEnv<'_>, jvmti: &jvmti::Jvmti) -> Result<(), Box<dyn std::error::Error>> {
    bridge::expect_system_loader(env)?;
    let system = env
        .call_static_method(
            "java/lang/ClassLoader",
            "getSystemClassLoader",
            "()Ljava/lang/ClassLoader;",
            &[],
        )?
        .l()?;
    SYSTEM
        .set(env.new_global_ref(system)?)
        .map_err(|_| "Duplicate system loader")?;
    let property = env.new_string("luabuddy.proof.jar")?;
    let jar = env
        .call_static_method(
            "java/lang/System",
            "getProperty",
            "(Ljava/lang/String;)Ljava/lang/String;",
            &[JValue::Object(&property)],
        )?
        .l()?;
    if jar.is_null() {
        return Err("Supply the installed game JAR property".into());
    }
    let jar = env
        .get_string(&JString::from(jar))?
        .to_string_lossy()
        .into_owned();
    let path = env.new_string(jar)?;
    let file = env.new_object(
        "java/io/File",
        "(Ljava/lang/String;)V",
        &[JValue::Object(&path)],
    )?;
    let uri = env
        .call_method(file, "toURI", "()Ljava/net/URI;", &[])?
        .l()?;
    let url = env
        .call_method(uri, "toURL", "()Ljava/net/URL;", &[])?
        .l()?;
    let urls = env.new_object_array(1, "java/net/URL", JObject::null())?;
    env.set_object_array_element(&urls, 0, url)?;
    let loader = env.new_object(
        "java/net/URLClassLoader",
        "([Ljava/net/URL;Ljava/lang/ClassLoader;)V",
        &[JValue::Object(&urls), JValue::Object(&JObject::null())],
    )?;
    SECOND
        .set(env.new_global_ref(loader)?)
        .map_err(|_| "Duplicate second loader")?;
    let class = env.find_class("zombie/Lua/LuaEventManager")?;
    let system_call = env.get_method_id(&class, "call", "(Lse/krka/kahlua/vm/LuaCallFrame;I)I")?;
    if !bridge::ready() || jvmti.method_modifiers(system_call.into_raw())? & 0x0100 == 0 {
        return Err("The supported class was not registered as native".into());
    }
    let custom = SECOND_CLASS
        .get()
        .ok_or("Second loader was not prepared in the registration interval")?;
    let custom_class: &JClass<'_> = custom.as_obj().into();
    let custom_call =
        env.get_method_id(custom_class, "call", "(Lse/krka/kahlua/vm/LuaCallFrame;I)I")?;
    if jvmti.method_modifiers(custom_call.into_raw())? & 0x0100 != 0 {
        return Err("The unsupported loader's original declaration was modified".into());
    }
    let instance = env.new_object(custom_class, "()V", &[])?;
    let value = env
        .call_method(
            instance,
            "call",
            "(Lse/krka/kahlua/vm/LuaCallFrame;I)I",
            &[JValue::Object(&JObject::null()), JValue::Int(0)],
        )?
        .i()?;
    if value != 0 {
        return Err("The unsupported loader's original no-op behavior changed".into());
    }
    let event = env.find_class("zombie/Lua/Event")?;
    let entry = env.get_method_id(&event, "trigger", events::DESCRIPTOR)?;
    let original = env.get_method_id(&event, events::ORIGINAL, events::DESCRIPTOR)?;
    if !events::registered() || jvmti.method_modifiers(entry.into_raw())? & 0x0100 == 0 {
        return Err("The supported event dispatcher was not registered as native".into());
    }
    events::verify(&jvmti.bytecodes(original.into_raw())?)?;
    let name = env.new_string("zombie.Lua.Event")?;
    let custom = env
        .call_static_method(
            "java/lang/Class",
            "forName",
            "(Ljava/lang/String;ZLjava/lang/ClassLoader;)Ljava/lang/Class;",
            &[
                JValue::Object(&name),
                JValue::Bool(1),
                JValue::Object(SECOND.get().ok_or("Missing second loader")?.as_obj()),
            ],
        )?
        .l()?;
    let custom = JClass::from(custom);
    let entry = env.get_method_id(&custom, "trigger", events::DESCRIPTOR)?;
    if jvmti.method_modifiers(entry.into_raw())? & 0x0100 != 0 {
        return Err("The unsupported loader's event dispatcher was transformed".into());
    }
    events::verify(&jvmti.bytecodes(entry.into_raw())?)?;
    Ok(())
}

unsafe extern "system" fn load(
    raw: jvmti::CallbackEnv,
    jni: *mut RawJni,
    redefined: jclass,
    loader: jobject,
    name: *const c_char,
    domain: jobject,
    length: i32,
    bytes: *const u8,
    new_length: *mut i32,
    new_bytes: *mut *mut u8,
) {
    let result = std::panic::catch_unwind(|| -> Result<(), String> {
        if new_length.is_null() || new_bytes.is_null() {
            return Err("Null class hook outputs".into());
        }
        // SAFETY: This ClassFileLoadHook exclusively owns both writable outputs.
        unsafe {
            *new_length = 0;
            *new_bytes = std::ptr::null_mut();
        }
        // SAFETY: Every argument is forwarded unchanged from this JVM callback.
        unsafe {
            bridge::class_load(
                raw, jni, redefined, loader, name, domain, length, bytes, new_length, new_bytes,
            )
        };
        if name.is_null() {
            return Ok(());
        }
        // SAFETY: JVMTI supplies the NUL-terminated class name and initialized output.
        if unsafe { CStr::from_ptr(name) }.to_bytes() != b"zombie/Lua/LuaEventManager"
            || unsafe { *new_length } == 0
        {
            return Ok(());
        }
        // SAFETY: The JNI environment belongs to this class-loading callback.
        let mut env = unsafe { JNIEnv::from_raw(jni) }.map_err(|e| e.to_string())?;
        let name = env
            .new_string("zombie.Lua.LuaEventManager")
            .map_err(|e| e.to_string())?;
        let second = SECOND.get().ok_or("Second loader was not initialized")?;
        let class = env
            .call_static_method(
                "java/lang/Class",
                "forName",
                "(Ljava/lang/String;ZLjava/lang/ClassLoader;)Ljava/lang/Class;",
                &[
                    JValue::Object(&name),
                    JValue::Bool(1),
                    JValue::Object(second.as_obj()),
                ],
            )
            .and_then(|value| value.l())
            .map_err(|e| e.to_string())?;
        SECOND_CLASS
            .set(env.new_global_ref(class).map_err(|e| e.to_string())?)
            .map_err(|_| "Duplicate second class".to_owned())?;
        Ok(())
    });
    if !matches!(result, Ok(Ok(()))) {
        eprintln!("[LuaBuddy bridge loader proof] FAIL nested loader {result:?}");
        std::process::exit(1);
    }
}
