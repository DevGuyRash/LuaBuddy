use crate::jvmti::{CallbackEnv, Jvmti};
use jni::JNIEnv;
use jni::objects::{GlobalRef, JObject, JValue};
use jni::sys::{JNIEnv as RawJni, JavaVM as RawVm, jmethodID, jobject};
use luabuddy_patcher::extension::LoadedExtension;
use luabuddy_patcher::{
    Capabilities, Error, Hook, HookControl, HookKind, InstallReceipt, Invocation, ObservedTarget,
    PatchBackend, PatchDiagnostic, PatchSpec, Registry, Result, Stage, Target, Value,
    bytecode_sha256,
};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Field {
    Int,
    Object,
}

#[derive(Clone)]
struct Method {
    id: usize,
    target: Target,
    arguments: Vec<Field>,
    return_tag: u8,
    is_static: bool,
    _class: GlobalRef,
}

#[derive(Clone)]
struct ActiveMethod {
    method: Method,
    engine: u64,
    receipts: BTreeMap<u64, String>,
    callback_error: Option<String>,
}

#[derive(Default)]
struct Active {
    methods: BTreeMap<usize, ActiveMethod>,
    registries: BTreeMap<u64, Arc<Registry>>,
    pending: BTreeSet<usize>,
}

static ACTIVE: OnceLock<Mutex<Active>> = OnceLock::new();
static NEXT: AtomicU64 = AtomicU64::new(1);
static ENVIRONMENT: AtomicUsize = AtomicUsize::new(0);

fn active() -> &'static Mutex<Active> {
    ACTIVE.get_or_init(|| Mutex::new(Active::default()))
}

pub(crate) fn initialize(jvmti: &Jvmti) -> std::result::Result<(), String> {
    let pointer = jvmti.raw() as usize;
    match ENVIRONMENT.compare_exchange(0, pointer, Ordering::SeqCst, Ordering::SeqCst) {
        Ok(_) => Ok(()),
        Err(current) if current == pointer => Ok(()),
        Err(_) => Err("The JVM patcher is already bound to a different JVMTI environment".into()),
    }
}

/// A native patch set whose Rust callbacks run on the JVM thread entering a target.
///
/// Activation and disable must be called on a JVM-attached control thread. Disable
/// prevents subsequent target entries; a callback already entered may finish.
/// Dropping this handle does not uninstall hooks. Disable each installed patch
/// explicitly before dropping it or retain it until process exit.
pub struct JvmPatcher {
    engine: u64,
    registry: Registry,
}

impl Default for JvmPatcher {
    fn default() -> Self {
        Self {
            engine: NEXT.fetch_add(1, Ordering::Relaxed),
            registry: Registry::default(),
        }
    }
}

impl JvmPatcher {
    pub fn capabilities(&self) -> Capabilities {
        capabilities()
    }

    /// Observes a method in the exact bootstrap or system classloader.
    /// Its fingerprint is a precondition, not a wildcard for later activation.
    pub fn target(
        env: &mut JNIEnv<'_>,
        class: &str,
        method: &str,
        descriptor: &str,
        loader: &str,
    ) -> Result<Target> {
        let target = Target {
            class: class.into(),
            method: method.into(),
            descriptor: descriptor.into(),
            loader: loader.into(),
            bytecode_sha256: "0".repeat(64),
        };
        let mut backend = Backend::new(env, 0)?;
        Ok(backend.resolve(&target)?.target)
    }

    pub fn register(&mut self, spec: PatchSpec, hook: Arc<dyn Hook>) -> Result<()> {
        self.registry.register(spec, hook)
    }

    pub fn register_extension(&mut self, extension: LoadedExtension) -> Result<()> {
        extension.register(&mut self.registry)
    }

    pub fn diagnostics(&self) -> Vec<PatchDiagnostic> {
        self.registry.diagnostics()
    }

    pub fn callback_failures(&self) -> Result<Vec<(String, String)>> {
        let active = active()
            .lock()
            .map_err(|_| Error::Backend("Native patch registry poisoned".into()))?;
        Ok(active
            .methods
            .values()
            .filter(|method| method.engine == self.engine)
            .flat_map(|method| {
                method.receipts.values().filter_map(|id| {
                    method
                        .callback_error
                        .as_ref()
                        .map(|error| (id.clone(), error.clone()))
                })
            })
            .collect())
    }

    pub fn activate(&mut self, env: &mut JNIEnv<'_>, requested: &[String]) -> Result<Vec<String>> {
        let mut backend = Backend::new(env, self.engine)?;
        let result = self.registry.activate(requested, &mut backend);
        self.publish()?;
        result
    }

    pub fn disable(&mut self, env: &mut JNIEnv<'_>, id: &str) -> Result<()> {
        let mut backend = Backend::new(env, self.engine)?;
        let result = self.registry.disable(id, &mut backend);
        self.publish()?;
        result
    }

    fn publish(&self) -> Result<()> {
        let snapshot = Arc::new(self.registry.clone());
        let previous = {
            let mut active = active()
                .lock()
                .map_err(|_| Error::Backend("Native patch registry poisoned".into()))?;
            if active
                .methods
                .values()
                .any(|method| method.engine == self.engine)
            {
                active.registries.insert(self.engine, snapshot)
            } else {
                active.registries.remove(&self.engine)
            }
        };
        drop(previous);
        Ok(())
    }
}

struct Backend<'a, 'vm> {
    env: &'a mut JNIEnv<'vm>,
    jvmti: Jvmti,
    engine: u64,
}

impl<'a, 'vm> Backend<'a, 'vm> {
    fn new(env: &'a mut JNIEnv<'vm>, engine: u64) -> Result<Self> {
        let raw = ENVIRONMENT.load(Ordering::SeqCst);
        if raw == 0 {
            return Err(Error::Backend(
                "The native agent has not initialized its patcher environment".into(),
            ));
        }
        // SAFETY: Agent initialization publishes its live, never-disposed JVMTI
        // environment. Every method operation uses that same environment.
        let jvmti = unsafe { Jvmti::from_raw(raw as CallbackEnv) };
        Ok(Self { env, jvmti, engine })
    }

    fn resolve(&mut self, target: &Target) -> Result<Method> {
        self.env.push_local_frame(32).map_err(jni_error)?;
        let result = self.resolve_checked(target);
        if result.is_err() && self.env.exception_check().unwrap_or(false) {
            let _ = self.env.exception_clear();
        }
        // SAFETY: resolve_checked returned only a GlobalRef-backed Method or an
        // owned error. No local reference from this frame escapes the call.
        unsafe { self.env.pop_local_frame(&JObject::null()) }.map_err(jni_error)?;
        result
    }

    fn resolve_checked(&mut self, target: &Target) -> Result<Method> {
        target.validate()?;
        let (arguments, return_tag) = descriptor(&target.descriptor)?;
        if target.method == "<init>" || target.method == "<clinit>" {
            return Err(Error::Unsupported(
                "Constructor interception is not supported".into(),
            ));
        }
        let requested_loader = match target.loader.as_str() {
            "bootstrap" => JObject::null(),
            "system" => self
                .env
                .call_static_method(
                    "java/lang/ClassLoader",
                    "getSystemClassLoader",
                    "()Ljava/lang/ClassLoader;",
                    &[],
                )
                .and_then(|v| v.l())
                .map_err(jni_error)?,
            _ => {
                return Err(Error::Unsupported(
                    "Only exact bootstrap and system classloaders are supported".into(),
                ));
            }
        };
        let name = self
            .env
            .new_string(target.class.replace('/', "."))
            .map_err(jni_error)?;
        let class = self
            .env
            .call_static_method(
                "java/lang/Class",
                "forName",
                "(Ljava/lang/String;ZLjava/lang/ClassLoader;)Ljava/lang/Class;",
                &[
                    JValue::Object(&name),
                    JValue::Bool(0),
                    JValue::Object(&requested_loader),
                ],
            )
            .and_then(|v| v.l())
            .map_err(jni_error)?;
        let loader = self
            .env
            .call_method(&class, "getClassLoader", "()Ljava/lang/ClassLoader;", &[])
            .and_then(|v| v.l())
            .map_err(jni_error)?;
        let matches = match target.loader.as_str() {
            "bootstrap" => loader.is_null(),
            "system" => {
                let system = self
                    .env
                    .call_static_method(
                        "java/lang/ClassLoader",
                        "getSystemClassLoader",
                        "()Ljava/lang/ClassLoader;",
                        &[],
                    )
                    .and_then(|v| v.l())
                    .map_err(jni_error)?;
                !loader.is_null()
                    && self
                        .env
                        .is_same_object(&loader, system)
                        .map_err(jni_error)?
            }
            _ => {
                return Err(Error::Unsupported(
                    "Only exact bootstrap and system classloaders are supported".into(),
                ));
            }
        };
        if !matches {
            return Err(Error::StaleTarget(format!(
                "Classloader identity differs for {}",
                target.class
            )));
        }
        let methods = self.jvmti.class_methods(class.as_raw().cast()).map_err(Error::Backend)?
            .ok_or_else(|| Error::Unsupported(format!("Target class {} is not prepared; wait for its normal host lifecycle before activation", target.class)))?;
        let mut matching = None;
        for method in methods {
            let (name, descriptor) = self.jvmti.method_name(method).map_err(Error::Backend)?;
            if name == target.method && descriptor == target.descriptor {
                matching = Some(method);
                break;
            }
        }
        let method = matching.ok_or_else(|| {
            Error::StaleTarget(format!(
                "{} does not declare {}{}",
                target.class, target.method, target.descriptor
            ))
        })?;
        let modifiers = self
            .jvmti
            .method_modifiers(method)
            .map_err(Error::Backend)?;
        let declaring = self
            .jvmti
            .method_declaring_class(method)
            .map_err(Error::Backend)?;
        // SAFETY: GetMethodDeclaringClass returned a local reference in this frame.
        let declaring = unsafe { JObject::from_raw(declaring) };
        if !self
            .env
            .is_same_object(&class, declaring)
            .map_err(jni_error)?
        {
            return Err(Error::StaleTarget(
                "The target class does not declare this method".into(),
            ));
        }
        if modifiers & (0x0100 | 0x0400) != 0 {
            return Err(Error::Unsupported(
                "Native and abstract target methods have no verified breakpoint bytecode".into(),
            ));
        }
        let bytes = self.jvmti.bytecodes(method).map_err(Error::Backend)?;
        if bytes.is_empty() {
            return Err(Error::Unsupported(
                "Target has no executable bytecode".into(),
            ));
        }
        let mut observed = target.clone();
        observed.bytecode_sha256 = bytecode_sha256(&bytes);
        Ok(Method {
            id: method as usize,
            target: observed,
            arguments,
            return_tag,
            is_static: modifiers & 0x0008 != 0,
            _class: self.env.new_global_ref(class).map_err(jni_error)?,
        })
    }
}

impl PatchBackend for Backend<'_, '_> {
    fn capabilities(&self) -> Capabilities {
        capabilities()
    }

    fn inspect(&mut self, target: &Target) -> Result<ObservedTarget> {
        Ok(ObservedTarget {
            target: self.resolve(target)?.target,
        })
    }

    fn install(&mut self, patch: &PatchSpec) -> Result<InstallReceipt> {
        let method = self.resolve(&patch.target)?;
        if method.target != patch.target {
            return Err(Error::StaleTarget(patch.id.clone()));
        }
        match patch.kind {
            HookKind::Before => (),
            HookKind::Replace if b"ZBCSI".contains(&method.return_tag) => (),
            HookKind::ModifyArguments
                if method.arguments.iter().all(|field| *field == Field::Int) => {}
            _ => {
                return Err(Error::Unsupported(format!(
                    "{:?} is unsupported for {}",
                    patch.kind, patch.target.descriptor
                )));
            }
        }
        let receipt = NEXT.fetch_add(1, Ordering::Relaxed);
        {
            let mut active = active()
                .lock()
                .map_err(|_| Error::Backend("Native patch registry poisoned".into()))?;
            if active.pending.contains(&method.id) {
                return Err(Error::Conflict(
                    "The target has a native operation in progress".into(),
                ));
            }
            if let Some(existing) = active.methods.get_mut(&method.id) {
                if existing.engine != self.engine || existing.method.target != method.target {
                    return Err(Error::Conflict(
                        "This native target belongs to another patch engine".into(),
                    ));
                }
                existing.receipts.insert(receipt, patch.id.clone());
                return Ok(InstallReceipt { token: receipt });
            }
            active.pending.insert(method.id);
        }
        let result = self
            .jvmti
            .breakpoint(method.id as jmethodID, 0)
            .map_err(Error::Backend);
        let mut active = active()
            .lock()
            .map_err(|_| Error::Backend("Native patch registry poisoned".into()))?;
        active.pending.remove(&method.id);
        result?;
        active.methods.insert(
            method.id,
            ActiveMethod {
                method,
                engine: self.engine,
                receipts: BTreeMap::from([(receipt, patch.id.clone())]),
                callback_error: None,
            },
        );
        Ok(InstallReceipt { token: receipt })
    }

    fn uninstall(&mut self, receipt: &InstallReceipt) -> Result<()> {
        let id = {
            let mut active = active()
                .lock()
                .map_err(|_| Error::Backend("Native patch registry poisoned".into()))?;
            let id = active
                .methods
                .iter()
                .find(|(_, method)| {
                    method.engine == self.engine && method.receipts.contains_key(&receipt.token)
                })
                .map(|(id, _)| *id)
                .ok_or_else(|| Error::Backend("Unknown native installation receipt".into()))?;
            if active.pending.contains(&id) {
                return Err(Error::Backend(
                    "This native receipt has an operation in progress".into(),
                ));
            }
            let method = active.methods.get_mut(&id).expect("receipt resolved above");
            if method.receipts.len() > 1 {
                method.receipts.remove(&receipt.token);
                return Ok(());
            }
            if !active.pending.insert(id) {
                return Err(Error::Backend(
                    "This native receipt has an operation in progress".into(),
                ));
            }
            id
        };
        let result = self
            .jvmti
            .clear_breakpoint(id as jmethodID, 0)
            .map_err(Error::Backend);
        let removed = {
            let mut active = active()
                .lock()
                .map_err(|_| Error::Backend("Native patch registry poisoned".into()))?;
            active.pending.remove(&id);
            result?;
            active.methods.remove(&id)
        };
        drop(removed);
        Ok(())
    }
}

fn capabilities() -> Capabilities {
    Capabilities {
        hooks: BTreeSet::from([
            HookKind::Before,
            HookKind::Replace,
            HookKind::ModifyArguments,
        ]),
        reversible: true,
    }
}

fn descriptor(value: &str) -> Result<(Vec<Field>, u8)> {
    let bytes = value.as_bytes();
    let mut at = 1;
    let mut arguments = Vec::new();
    while bytes.get(at) != Some(&b')') {
        let tag = *bytes
            .get(at)
            .ok_or_else(|| Error::Invalid("Truncated method descriptor".into()))?;
        at += 1;
        match tag {
            b'Z' | b'B' | b'C' | b'S' | b'I' => arguments.push(Field::Int),
            b'L' | b'[' => {
                if tag == b'[' {
                    while bytes.get(at) == Some(&b'[') { at += 1; }
                }
                if tag == b'L' || bytes.get(at) == Some(&b'L') {
                    while bytes.get(at) != Some(&b';') {
                        if bytes.get(at).is_none() { return Err(Error::Invalid("Unterminated object descriptor".into())); }
                        at += 1;
                    }
                    at += 1;
                } else if tag == b'[' { at += 1; }
                arguments.push(Field::Object);
            }
            _ => return Err(Error::Unsupported("This JVM adapter supports integer and reference arguments; long, float and double arguments are not implemented".into())),
        }
    }
    let return_tag = *bytes
        .get(at + 1)
        .ok_or_else(|| Error::Invalid("Missing return descriptor".into()))?;
    Ok((arguments, return_tag))
}

fn jni_error(error: jni::errors::Error) -> Error {
    Error::Backend(format!("JNI target resolution: {error}"))
}

/// # Safety
/// All raw handles must originate in this live JVM's breakpoint callback. The
/// callback owns its local frame and suspended event thread until return.
pub(crate) unsafe fn dispatch(
    raw: CallbackEnv,
    jni: *mut RawJni,
    thread: jobject,
    method: jmethodID,
    location: i64,
) -> std::result::Result<bool, String> {
    if location != 0 {
        return Ok(false);
    }
    let snapshot = {
        let active = active()
            .lock()
            .map_err(|_| "Native patch registry poisoned")?;
        let Some(method) = active.methods.get(&(method as usize)) else {
            return Ok(false);
        };
        active
            .registries
            .get(&method.engine)
            .cloned()
            .map(|registry| {
                (
                    method.method.clone(),
                    registry,
                    method.receipts.values().cloned().collect::<BTreeSet<_>>(),
                )
            })
    };
    let Some((method, registry, installed)) = snapshot else {
        return Ok(true);
    };
    // SAFETY: The JVM owns the callback-local JNI environment and event thread.
    let env = unsafe { JNIEnv::from_raw(jni) }.map_err(|e| e.to_string())?;
    // SAFETY: raw is this callback's live JVMTI environment.
    let jvmti = unsafe { Jvmti::from_raw(raw) };
    let mut locals = Vec::new();
    let mut arguments = Vec::new();
    let first = i32::from(!method.is_static);
    for (index, field) in method.arguments.iter().enumerate() {
        let slot = first + index as i32;
        match field {
            Field::Int => arguments.push(Value::Int(jvmti.local_int(thread, slot)?)),
            Field::Object => {
                let object = jvmti.local_object(thread, slot)?;
                if object.is_null() {
                    arguments.push(Value::Object(0));
                } else {
                    // SAFETY: GetLocalObject creates a callback-local reference.
                    let object = unsafe { JObject::from_raw(object) };
                    let object = env.auto_local(object);
                    let mut token = None;
                    for (index, previous) in locals.iter().enumerate() {
                        if env
                            .is_same_object(previous, &object)
                            .map_err(|e| e.to_string())?
                        {
                            token = Some(index + 1);
                            break;
                        }
                    }
                    let token = token.unwrap_or_else(|| {
                        locals.push(object);
                        locals.len()
                    });
                    arguments.push(Value::Object(token as u64));
                }
            }
        }
    }
    let original = arguments.clone();
    let mut invocation = Invocation {
        target: method.target,
        stage: Stage::Entry,
        arguments,
        result: None,
        exception: None,
    };
    let result = registry.dispatch_selected(&mut invocation, &installed);
    let result = if env.exception_check().map_err(|e| e.to_string())? {
        env.exception_clear().map_err(|e| e.to_string())?;
        Err(Error::Hook(
            "A Rust patch left a pending JNI exception; the original host method will continue"
                .into(),
        ))
    } else {
        result
    };
    let control = result.map_err(|error| {
        let message = error.to_string();
        if let Ok(mut active) = active().lock()
            && let Some(method) = active.methods.get_mut(&(method.id))
        {
            method.callback_error = Some(message.clone());
        }
        message
    })?;
    if matches!(control, HookControl::Continue) && invocation.arguments != original {
        let mut changed = Vec::new();
        for (index, value) in invocation.arguments.iter().enumerate() {
            let Value::Int(value) = value else {
                return Err("Object argument mutation is unsupported".into());
            };
            let Value::Int(previous) = original[index] else {
                return Err("Object argument mutation is unsupported".into());
            };
            if *value == previous {
                continue;
            }
            if let Err(error) = jvmti.set_local_int(thread, first + index as i32, *value) {
                let mut rollback_errors = Vec::new();
                for (index, previous) in changed.into_iter().rev() {
                    if let Err(rollback) = jvmti.set_local_int(thread, first + index, previous) {
                        rollback_errors.push(rollback);
                    }
                }
                return Err(format!(
                    "Argument mutation failed: {error}; restoration errors: {rollback_errors:?}"
                ));
            }
            changed.push((index as i32, previous));
        }
    }
    if let HookControl::Return(value) = control {
        let Value::Int(value) = value else {
            return Err("This native target requires an integer replacement result".into());
        };
        if method.return_tag == b'Z' && !matches!(value, 0 | 1) {
            return Err("Boolean replacement must be zero or one".into());
        }
        jvmti.return_int(thread, value)?;
    }
    Ok(true)
}

#[path = "../../../proofs/patcher/src/scenario.rs"]
mod scenario;

/// # Safety
/// The VM is the live invocation interface supplied to explicit proof-patcher
/// Agent_OnLoad. This mode runs only in a disposable JVM selected by its caller.
pub(crate) unsafe fn start(vm: *mut RawVm) -> std::result::Result<(), String> {
    // SAFETY: Agent_OnLoad supplied the live VM pointer.
    let jvmti = unsafe { Jvmti::from_vm(vm) }?;
    jvmti.enable_proof_capabilities()?;
    initialize(&jvmti)?;
    jvmti.set_callbacks(proof_init, proof_breakpoint)
}

unsafe extern "system" fn proof_init(_raw: CallbackEnv, jni: *mut RawJni, _thread: jobject) {
    let outcome = std::panic::catch_unwind(|| {
        // SAFETY: VMInit supplies this attached callback-local environment.
        let mut env = unsafe { JNIEnv::from_raw(jni) }.map_err(|e| e.to_string())?;
        proof(&mut env)
    });
    match outcome {
        Ok(Ok(())) => eprintln!(
            "[LuaBuddy patcher proof] PASS live JVM patches, Rust extensions, nested/concurrent callbacks and disable"
        ),
        Ok(Err(error)) => {
            eprintln!("[LuaBuddy patcher proof] FAIL {error}");
            std::process::exit(1);
        }
        Err(_) => {
            eprintln!("[LuaBuddy patcher proof] FAIL panic contained at VMInit");
            std::process::exit(1);
        }
    }
}

unsafe extern "system" fn proof_breakpoint(
    raw: CallbackEnv,
    jni: *mut RawJni,
    thread: jobject,
    method: jmethodID,
    location: i64,
) {
    let result = std::panic::catch_unwind(|| {
        // SAFETY: All handles belong to this live JVMTI callback.
        unsafe { dispatch(raw, jni, thread, method, location) }
    });
    match result {
        Ok(Ok(_)) => (),
        Ok(Err(error)) => {
            eprintln!("[LuaBuddy patcher proof] callback FAIL {error}");
            std::process::exit(1);
        }
        Err(_) => {
            eprintln!("[LuaBuddy patcher proof] callback FAIL panic contained");
            std::process::exit(1);
        }
    }
}

fn proof(env: &mut JNIEnv<'_>) -> std::result::Result<(), String> {
    scenario::run(env).map_err(|error| error.to_string())
}
