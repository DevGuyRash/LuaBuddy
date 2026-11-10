use crate::jvmti::{CallbackEnv, Jvmti};
use jni::JNIEnv;
use jni::objects::{GlobalRef, JObject, WeakRef};
use jni::sys::{JNIEnv as RawJni, jmethodID, jobject};
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::{AtomicUsize, Ordering};

pub(crate) type Handler =
    for<'a> fn(&mut JNIEnv<'a>, &JObject<'a>, i32, u64) -> Result<i32, String>;

#[derive(Clone)]
struct Registration {
    id: usize,
    callable: WeakRef,
    owner: String,
    generation: u64,
    context: u64,
    handler: Handler,
}

static METHOD: AtomicUsize = AtomicUsize::new(0);
static NATIVE_READY: AtomicBool = AtomicBool::new(false);
static REGISTRY: Mutex<Vec<Registration>> = Mutex::new(Vec::new());
static NEXT: AtomicUsize = AtomicUsize::new(1);

pub(crate) fn prepare(env: &mut JNIEnv<'_>, jvmti: &Jvmti) -> Result<(), String> {
    let class = env
        .find_class("zombie/Lua/LuaEventManager")
        .map_err(|e| e.to_string())?;
    let method = env
        .get_method_id(class, "call", "(Lse/krka/kahlua/vm/LuaCallFrame;I)I")
        .map_err(|e| e.to_string())?;
    if jvmti.bytecodes(method.into_raw())? != [0x03, 0xac] {
        return Err(
            "incompatible LuaEventManager.call implementation; expected native no-op".to_owned(),
        );
    }
    if METHOD.load(Ordering::SeqCst) == method.into_raw() as usize {
        return Ok(());
    }
    jvmti.breakpoint(method.into_raw(), 0)?;
    METHOD.store(method.into_raw() as usize, Ordering::SeqCst);
    Ok(())
}

pub(crate) fn create(
    env: &mut JNIEnv<'_>,
    owner: &str,
    generation: u64,
    context: u64,
    handler: Handler,
) -> Result<GlobalRef, String> {
    if METHOD.load(Ordering::SeqCst) == 0 && !NATIVE_READY.load(Ordering::SeqCst) {
        return Err("native callable bridge is not prepared".to_owned());
    }
    sweep(env)?;
    let callable = env
        .new_object("zombie/Lua/LuaEventManager", "()V", &[])
        .and_then(|object| env.new_global_ref(object))
        .map_err(|e| e.to_string())?;
    let weak = env
        .new_weak_ref(callable.as_obj())
        .map_err(|e| e.to_string())?
        .ok_or("native callable unexpectedly null")?;
    REGISTRY
        .lock()
        .map_err(|_| "native callable registry poisoned")?
        .push(Registration {
            id: NEXT.fetch_add(1, Ordering::Relaxed),
            callable: weak,
            owner: owner.to_owned(),
            generation,
            context,
            handler,
        });
    Ok(callable)
}

pub(crate) fn native_ready() {
    NATIVE_READY.store(true, Ordering::SeqCst);
}

pub(crate) fn invoke<'a>(
    env: &mut JNIEnv<'a>,
    receiver: &JObject<'a>,
    frame: &JObject<'a>,
    arguments: i32,
) -> Result<i32, String> {
    let registrations = REGISTRY
        .lock()
        .map_err(|_| "native callable registry poisoned")?
        .clone();
    for registration in registrations {
        if registration
            .callable
            .is_same_object(env, receiver)
            .map_err(|e| e.to_string())?
        {
            let result = (registration.handler)(env, frame, arguments, registration.context)?;
            if result < 0 {
                return Err("native callable returned negative result count".to_owned());
            }
            return Ok(result);
        }
    }
    Ok(0)
}

fn sweep(env: &JNIEnv<'_>) -> Result<(), String> {
    let registrations = REGISTRY
        .lock()
        .map_err(|_| "native callable registry poisoned")?
        .clone();
    let mut collected = std::collections::BTreeSet::new();
    for registration in registrations {
        if registration
            .callable
            .is_garbage_collected(env)
            .map_err(|e| e.to_string())?
        {
            collected.insert(registration.id);
        }
    }
    if !collected.is_empty() {
        REGISTRY
            .lock()
            .map_err(|_| "native callable registry poisoned")?
            .retain(|entry| !collected.contains(&entry.id));
    }
    Ok(())
}

pub(crate) fn unregister(owner: &str, generation: u64) -> Result<usize, String> {
    let mut registry = REGISTRY
        .lock()
        .map_err(|_| "native callable registry poisoned")?;
    let original = registry.len();
    registry.retain(|registration| {
        registration.owner != owner || registration.generation != generation
    });
    Ok(original - registry.len())
}

/// # Safety
/// All raw handles must be from the current breakpoint callback.
pub(crate) unsafe fn dispatch(
    raw: CallbackEnv,
    jni: *mut RawJni,
    thread: jobject,
    method: jmethodID,
) -> Result<bool, String> {
    if METHOD.load(Ordering::SeqCst) != method as usize {
        return Ok(false);
    }
    // SAFETY: The callback supplies the live thread-local JNI and JVMTI environments.
    let mut env = unsafe { JNIEnv::from_raw(jni) }.map_err(|e| e.to_string())?;
    // SAFETY: raw is borrowed from the current breakpoint callback.
    let jvmti = unsafe { Jvmti::from_raw(raw) };
    let raw_receiver = jvmti.local_object(thread, 0)?;
    let raw_frame = jvmti.local_object(thread, 1)?;
    if raw_receiver.is_null() || raw_frame.is_null() {
        return Err("null native callable receiver or Kahlua frame".to_owned());
    }
    // SAFETY: GetLocalObject returns callback-local JNI references for these slots.
    let receiver = unsafe { JObject::from_raw(raw_receiver) };
    // SAFETY: The non-null local frame reference has the callback's local lifetime.
    let frame = unsafe { JObject::from_raw(raw_frame) };
    let arguments = jvmti.local_int(thread, 2)?;
    let result = invoke(&mut env, &receiver, &frame, arguments)?;
    jvmti.return_int(thread, result)?;
    Ok(true)
}
