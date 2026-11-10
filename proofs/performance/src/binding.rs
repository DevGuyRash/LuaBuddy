use crate::jvmti::{CallbackEnv, Jvmti};
use jni::{
    JNIEnv,
    objects::{JObject, JValue},
    sys::{JNIEnv as RawJni, jobject},
};
use std::{
    ffi::c_void,
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::Duration,
};

static DYING: AtomicBool = AtomicBool::new(false);
type Start = unsafe extern "system" fn(CallbackEnv, *mut RawJni, *mut c_void);

pub(crate) fn install(jvmti: &Jvmti) -> Result<(), String> {
    let raw = jvmti.raw();
    let mut callbacks = [std::ptr::null::<c_void>(); 37];
    callbacks[0] = vm_init as *const c_void;
    callbacks[1] = vm_death as *const c_void;
    callbacks[6] = crate::class_prepare as *const c_void;
    // SAFETY: JVMTI1.2 slot122 synchronously copies this initialized standard callback table.
    let set: unsafe extern "system" fn(CallbackEnv, *const c_void, i32) -> i32 =
        unsafe { std::mem::transmute(*(*raw).add(121)) };
    // SAFETY: The initialized table matches event50..86 function pointer positions.
    let status = unsafe {
        set(
            raw,
            callbacks.as_ptr().cast(),
            std::mem::size_of_val(&callbacks) as i32,
        )
    };
    if status != 0 {
        return Err(format!("Observer callbacks failed ({status})"));
    }
    // SAFETY: JVMTI1.2 slot2 is the standard variadic event notification function.
    let event: unsafe extern "C" fn(CallbackEnv, i32, i32, jobject, ...) -> i32 =
        unsafe { std::mem::transmute(*(*raw).add(1)) };
    for kind in [50, 51, 56] {
        // SAFETY: The valid environment and callback table provide each enabled global event.
        let status = unsafe { event(raw, 1, kind, std::ptr::null_mut()) };
        if status != 0 {
            return Err(format!("Observer event {kind} failed ({status})"));
        }
    }
    Ok(())
}

/// # Safety
/// Handles belong to this synchronous JVM initialization callback.
unsafe extern "system" fn vm_init(raw: CallbackEnv, jni: *mut RawJni, _thread: jobject) {
    let outcome = std::panic::catch_unwind(|| -> Result<(), String> {
        // SAFETY: The VM supplies this active callback-local JNI environment.
        let mut env = unsafe { JNIEnv::from_raw(jni) }.map_err(|e| e.to_string())?;
        let name = env
            .new_string("LuaBuddy readiness verification")
            .map_err(|e| e.to_string())?;
        let agent = env
            .new_object(
                "java/lang/Thread",
                "(Ljava/lang/String;)V",
                &[JValue::Object(&name)],
            )
            .map_err(|e| e.to_string())?;
        // SAFETY: JVMTI1.2 slot12 creates a VM-managed daemon agent thread with this exact start ABI.
        let run: unsafe extern "system" fn(CallbackEnv, jobject, Start, *const c_void, i32) -> i32 =
            unsafe { std::mem::transmute(*(*raw).add(11)) };
        // SAFETY: The VM retains the new unstarted Thread and owns its JNI attachment through agent_start's lifetime.
        let status = unsafe { run(raw, agent.as_raw(), agent_start, std::ptr::null(), 5) };
        if status != 0 {
            return Err(format!("RunAgentThread failed ({status})"));
        }
        Ok(())
    });
    if let Err(error) =
        outcome.unwrap_or_else(|_| Err("Readiness initialization panic contained".into()))
    {
        // SAFETY: Error handling is still inside this active JNI callback.
        unsafe { crate::event_error(jni, &error) };
    }
}

/// # Safety
/// This VM-managed daemon callback owns its valid thread-local JNI environment until return.
unsafe extern "system" fn agent_start(raw: CallbackEnv, jni: *mut RawJni, _argument: *mut c_void) {
    // SAFETY: RunAgentThread supplies this attached agent thread's JNI environment.
    let Ok(env) = (unsafe { JNIEnv::from_raw(jni) }) else {
        crate::fail("Readiness agent has no JNI environment");
        return;
    };
    while !DYING.load(Ordering::Acquire) {
        if env.push_local_frame(256).is_err() {
            crate::fail("Readiness JNI local frame allocation failed");
            return;
        }
        // SAFETY: Both environments belong to this current live VM-managed agent callback.
        let result = std::panic::catch_unwind(|| unsafe { crate::observe(raw, jni) });
        // SAFETY: No local reference escapes observe; only owned Rust data or explicit GlobalRefs are retained.
        let _ = unsafe { env.pop_local_frame(&JObject::null()) };
        if DYING.load(Ordering::Acquire) {
            return;
        }
        if let Err(error) =
            result.unwrap_or_else(|_| Err("Readiness observation panic contained".into()))
        {
            // SAFETY: The VM-managed agent environment is still live here.
            unsafe { crate::event_error(jni, &error) };
        }
        let quitting = crate::FINISHED.load(Ordering::Acquire)
            && crate::STATE
                .get()
                .and_then(|state| state.lock().ok())
                .is_some_and(|state| {
                    state
                        .last_progress
                        .contains("MainScreen.quitToDesktopDialog.yes")
                });
        if quitting {
            return;
        }
        thread::sleep(Duration::from_millis(50));
    }
}

/// # Safety
/// This synchronous VMDeath event supplies live callback handles; no JNI operation is performed.
unsafe extern "system" fn vm_death(_raw: CallbackEnv, _jni: *mut RawJni) {
    DYING.store(true, Ordering::Release);
}
