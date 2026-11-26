use crate::jvmti::{CallbackEnv, Jvmti};
use jni::JNIEnv;
use jni::sys::{JNIEnv as RawJni, JavaVM, jmethodID, jobject};

/// # Safety
/// The invocation pointer is supplied by this JVM's Agent_OnLoad call.
pub(crate) unsafe fn start(vm: *mut JavaVM) -> Result<(), String> {
    // SAFETY: Agent_OnLoad's JVM pointer remains live until JVM shutdown.
    // from_vm checks the native ABI before accessing the invocation table.
    let jvmti = unsafe { Jvmti::from_vm(vm) }?;
    jvmti.enable_required_capabilities()?;
    if let Err(error) = jvmti.enable_breakpoint_capabilities() {
        crate::log::warn(
            "agent.limited",
            format!("Native patches and Workshop downloads are unavailable: another agent holds the JVM's breakpoints ({error})"),
        )
        .write();
    }
    crate::patcher::initialize(&jvmti)?;
    jvmti.set_callbacks_full(
        vm_init,
        breakpoint,
        Some(crate::bridge::class_load),
        Some(crate::bridge::class_prepare),
    )
}

unsafe extern "system" fn vm_init(raw: CallbackEnv, jni: *mut RawJni, _thread: jobject) {
    let outcome = std::panic::catch_unwind(|| -> Result<(), String> {
        // SAFETY: The JVM provides live callback-local JNI and JVMTI environments.
        let mut env = unsafe { JNIEnv::from_raw(jni) }.map_err(|e| e.to_string())?;
        // SAFETY: raw belongs to the active VMInit callback.
        let jvmti = unsafe { Jvmti::from_raw(raw) };
        crate::log::start(&env);
        crate::bridge::expect_system_loader(&mut env)?;
        crate::events::prepare(
            &mut env,
            crate::runtime::event,
            &crate::runtime::EVENTS,
            |error| crate::log::error("lifecycle.failed", error).write(),
        )?;
        if let Err(error) = crate::download_events::prepare(&mut env, &jvmti) {
            if matches!(env.exception_check(), Ok(true)) {
                let _ = env.exception_clear();
            }
            crate::log::warn(
                "workshop.downloads",
                format!("Workshop downloads are unavailable: {error}"),
            )
            .write();
        }
        crate::log::info("agent.started", "Waiting for the game's Lua to start")
            .with(serde_json::json!({ "version": env!("CARGO_PKG_VERSION") }))
            .write();
        Ok(())
    });
    report(outcome, jni, "startup");
}

unsafe extern "system" fn breakpoint(
    raw: CallbackEnv,
    jni: *mut RawJni,
    thread: jobject,
    method: jmethodID,
    location: i64,
) {
    let outcome = std::panic::catch_unwind(|| -> Result<(), String> {
        // SAFETY: The download observer receives the same callback-local environment and event thread.
        if unsafe { crate::download_events::dispatch(raw, thread, method) }? {
            return Ok(());
        }
        // SAFETY: The patch backend receives the JVM's callback-local event handles.
        if unsafe { crate::patcher::dispatch(raw, jni, thread, method, location) }? {
            return Ok(());
        }
        // SAFETY: These are the same callback-local handles checked above.
        unsafe { crate::callables::dispatch(raw, jni, thread, method) }?;
        Ok(())
    });
    report(outcome, jni, "lifecycle");
}

fn report(outcome: std::thread::Result<Result<(), String>>, jni: *mut RawJni, stage: &str) {
    let error = match outcome {
        Ok(Ok(())) => return,
        Ok(Err(error)) => error,
        Err(_) => "Rust panic contained at JVM callback".to_owned(),
    };
    crate::log::error(&format!("agent.{stage}"), error).write();
    // SAFETY: report is called before the native JVM callback returns.
    if let Ok(env) = unsafe { JNIEnv::from_raw(jni) }
        && matches!(env.exception_check(), Ok(true))
    {
        let _ = env.exception_describe();
        let _ = env.exception_clear();
    }
}
