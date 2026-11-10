use crate::jvmti::{CallbackEnv, Jvmti};
use jni::objects::GlobalRef;
use jni::objects::{JClass, JObject};
use jni::sys::{JNIEnv as RawJni, jclass, jobject};
use jni::{JNIEnv, NativeMethod};
use std::ffi::{CStr, c_char, c_void};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

static TRANSFORMED: AtomicBool = AtomicBool::new(false);
static REGISTERED: AtomicBool = AtomicBool::new(false);
static LOADER: OnceLock<GlobalRef> = OnceLock::new();

pub(crate) fn ready() -> bool {
    REGISTERED.load(Ordering::SeqCst)
}

pub(crate) fn expect_system_loader(env: &mut JNIEnv<'_>) -> Result<(), String> {
    let system = env
        .call_static_method(
            "java/lang/ClassLoader",
            "getSystemClassLoader",
            "()Ljava/lang/ClassLoader;",
            &[],
        )
        .and_then(|v| v.l())
        .map_err(|e| e.to_string())?;
    if system.is_null() {
        return Err("system game classloader is unavailable".to_owned());
    }
    let loader = env.new_global_ref(system).map_err(|e| e.to_string())?;
    LOADER
        .set(loader)
        .map_err(|_| "system classloader already prepared".to_owned())
}

pub(crate) unsafe extern "system" fn class_load(
    raw: CallbackEnv,
    jni: *mut RawJni,
    redefined: jclass,
    loader: jobject,
    name: *const c_char,
    _domain: jobject,
    length: i32,
    bytes: *const u8,
    new_length: *mut i32,
    new_bytes: *mut *mut u8,
) {
    let result = std::panic::catch_unwind(|| -> Result<(), String> {
        if name.is_null() {
            return Ok(());
        }
        // SAFETY: JVMTI supplies a NUL-terminated internal class name.
        if unsafe { CStr::from_ptr(name) }.to_bytes() != b"zombie/Lua/LuaEventManager" {
            return Ok(());
        }
        let expected_loader = LOADER
            .get()
            .ok_or("game class loaded before native bridge was prepared")?;
        // SAFETY: JNI IsSameObject is valid in this callback and calls no host code.
        let env = unsafe { JNIEnv::from_raw(jni) }.map_err(|e| e.to_string())?;
        // SAFETY: ClassFileLoadHook supplies this callback-local classloader handle.
        let loader = unsafe { JObject::from_raw(loader) };
        if !env
            .is_same_object(loader, expected_loader.as_obj())
            .map_err(|e| e.to_string())?
        {
            return Err("unsupported LuaEventManager classloader".to_owned());
        }
        if !redefined.is_null() {
            return Err("native callable requires initial class definition".to_owned());
        }
        if length <= 0
            || length > 16 * 1024 * 1024
            || bytes.is_null()
            || new_length.is_null()
            || new_bytes.is_null()
        {
            return Err("invalid classfile hook inputs".to_owned());
        }
        if TRANSFORMED.load(Ordering::SeqCst) {
            return Err("duplicate LuaEventManager classloader rejected".to_owned());
        }
        // SAFETY: JVMTI guarantees length initialized classfile bytes during callback.
        let original = unsafe { std::slice::from_raw_parts(bytes, length as usize) };
        let transformed = crate::classfile::native_callable(original)?;
        // SAFETY: The environment belongs to the current ClassFileLoadHook callback.
        let jvmti = unsafe { Jvmti::from_raw(raw) };
        let allocation = jvmti.allocate(transformed.len())?;
        // SAFETY: Allocate returned disjoint writable storage of transformed.len().
        // JVMTI takes ownership of that allocation through these output fields.
        unsafe {
            std::ptr::copy_nonoverlapping(transformed.as_ptr(), allocation, transformed.len());
            *new_length = transformed.len() as i32;
            *new_bytes = allocation;
        }
        TRANSFORMED.store(true, Ordering::SeqCst);
        Ok(())
    });
    match result {
        Ok(Ok(())) => (),
        Ok(Err(error)) => eprintln!("[LuaBuddy native bridge] rejected: {error}"),
        Err(_) => eprintln!("[LuaBuddy native bridge] class hook panic contained"),
    }
}

pub(crate) unsafe extern "system" fn class_prepare(
    raw: CallbackEnv,
    jni: *mut RawJni,
    _thread: jobject,
    class: jclass,
) {
    if !TRANSFORMED.load(Ordering::SeqCst) || REGISTERED.load(Ordering::SeqCst) {
        return;
    }
    let result = std::panic::catch_unwind(|| -> Result<(), String> {
        // SAFETY: Both handles are valid for this ClassPrepare callback.
        let mut env = unsafe { JNIEnv::from_raw(jni) }.map_err(|e| e.to_string())?;
        // SAFETY: raw is the callback's JVMTI environment.
        let jvmti = unsafe { Jvmti::from_raw(raw) };
        if jvmti.class_signature(class)? != "Lzombie/Lua/LuaEventManager;" {
            return Ok(());
        }
        // SAFETY: ClassPrepare supplies a live local jclass reference.
        let class = unsafe { JClass::from_raw(class) };
        let expected = LOADER
            .get()
            .ok_or("Native bridge has no approved defining loader")?;
        let loader = env
            .call_method(&class, "getClassLoader", "()Ljava/lang/ClassLoader;", &[])
            .and_then(|value| value.l())
            .map_err(|error| error.to_string())?;
        if !env
            .is_same_object(&loader, expected.as_obj())
            .map_err(|error| error.to_string())?
        {
            return Ok(());
        }
        env.register_native_methods(
            class,
            &[NativeMethod {
                name: "call".into(),
                sig: "(Lse/krka/kahlua/vm/LuaCallFrame;I)I".into(),
                fn_ptr: native_call as *mut c_void,
            }],
        )
        .map_err(|e| e.to_string())?;
        crate::callables::native_ready();
        REGISTERED.store(true, Ordering::SeqCst);
        Ok(())
    });
    match result {
        Ok(Ok(())) => (),
        Ok(Err(error)) => {
            // SAFETY: This is still the active ClassPrepare callback's JNI environment.
            unsafe { clear_prepare_exception(jni) };
            eprintln!("[LuaBuddy native bridge] registration failed: {error}");
        }
        Err(_) => {
            // SAFETY: This is still the active ClassPrepare callback's JNI environment.
            unsafe { clear_prepare_exception(jni) };
            eprintln!("[LuaBuddy native bridge] registration panic contained");
        }
    }
}

/// # Safety
/// `jni` is the current ClassPrepare callback's live, thread-local JNI environment.
unsafe fn clear_prepare_exception(jni: *mut RawJni) {
    // SAFETY: Called only before returning from the active ClassPrepare callback.
    if let Ok(env) = unsafe { JNIEnv::from_raw(jni) }
        && matches!(env.exception_check(), Ok(true))
    {
        let _ = env.exception_describe();
        let _ = env.exception_clear();
    }
}

unsafe extern "system" fn native_call(
    jni: *mut RawJni,
    receiver: jobject,
    frame: jobject,
    count: i32,
) -> i32 {
    let outcome = std::panic::catch_unwind(|| -> Result<i32, String> {
        // SAFETY: The JVM calls the registered native with valid JNI local handles.
        let mut env = unsafe { JNIEnv::from_raw(jni) }.map_err(|e| e.to_string())?;
        // SAFETY: Instance native methods always receive a non-null this reference.
        let receiver = unsafe { JObject::from_raw(receiver) };
        if frame.is_null() {
            return Err("null Kahlua native frame".to_owned());
        }
        // SAFETY: The JNI signature declares a LuaCallFrame local reference.
        let frame = unsafe { JObject::from_raw(frame) };
        crate::callables::invoke(&mut env, &receiver, &frame, count)
    });
    match outcome {
        Ok(Ok(count)) => count,
        error => {
            let message = match error {
                Ok(Err(message)) => message,
                Err(_) => "Rust callable panic contained".to_owned(),
                Ok(Ok(_)) => unreachable!(),
            };
            // SAFETY: The JNI native invocation remains active on this thread.
            if let Ok(mut env) = unsafe { JNIEnv::from_raw(jni) }
                && matches!(env.exception_check(), Ok(false))
            {
                let _ = env.throw_new("java/lang/IllegalStateException", message);
            }
            0
        }
    }
}
