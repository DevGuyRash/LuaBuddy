use jni::sys::{JNIEnv, JavaVM, jclass, jmethodID, jobject};
use std::ffi::{c_char, c_void};

type RawEnv = *mut *const *const c_void;
pub(crate) type ClassLoad = unsafe extern "system" fn(
    RawEnv,
    *mut JNIEnv,
    jclass,
    jobject,
    *const c_char,
    jobject,
    i32,
    *const u8,
    *mut i32,
    *mut *mut u8,
);
pub(crate) type ClassPrepare = unsafe extern "system" fn(RawEnv, *mut JNIEnv, jobject, jclass);

/// Borrowed JVMTI environment supplied by a live JVM.
pub(crate) struct Jvmti(RawEnv);

impl Jvmti {
    pub(crate) fn raw(&self) -> CallbackEnv {
        self.0
    }

    /// # Safety
    /// `vm` must point to the current live JVM's invocation interface.
    pub(crate) unsafe fn from_vm(vm: *mut JavaVM) -> Result<Self, String> {
        if !cfg!(all(target_os = "linux", target_arch = "x86_64")) {
            return Err("This native JVMTI ABI has been verified only on Linux x86_64".into());
        }
        let mut env = std::ptr::null_mut();
        // SAFETY: The caller owns a valid JVM pointer. GetEnv initializes env.
        let status =
            unsafe { ((**vm).GetEnv.ok_or("missing JNI GetEnv")?)(vm, &mut env, 0x3001_0200) };
        if status != 0 || env.is_null() {
            return Err(format!("JVMTI 1.2 negotiation failed ({status})"));
        }
        Ok(Self(env.cast()))
    }

    /// # Safety
    /// Only a JVMTI callback may construct a wrapper from its environment.
    pub(crate) unsafe fn from_raw(env: RawEnv) -> Self {
        Self(env)
    }

    fn entry(&self, index: usize) -> *const c_void {
        // SAFETY: JVMTI 1.2 defines every used index. Construction negotiates it.
        unsafe { *(*self.0).add(index - 1) }
    }

    pub(crate) fn enable_proof_capabilities(&self) -> Result<(), String> {
        let caps = [
            (1_u32 << 3) | (1_u32 << 14) | (1_u32 << 19) | (1_u32 << 26),
            1_u32 << 1,
            0,
            0,
        ];
        // SAFETY: JVMTI slot 142 is AddCapabilities. The four words match the
        // JVMTI capability bit layout on the explicitly verified Linux x86_64 ABI.
        let call: unsafe extern "system" fn(RawEnv, *const u32) -> i32 =
            unsafe { std::mem::transmute(self.entry(142)) };
        // SAFETY: The borrowed bitfield is initialized and remains live in-call.
        check(unsafe { call(self.0, caps.as_ptr()) }, "AddCapabilities")
    }

    pub(crate) fn set_callbacks(
        &self,
        init: unsafe extern "system" fn(RawEnv, *mut JNIEnv, jobject),
        breakpoint: unsafe extern "system" fn(RawEnv, *mut JNIEnv, jobject, jmethodID, i64),
    ) -> Result<(), String> {
        self.set_callbacks_full(init, breakpoint, None, None)
    }

    pub(crate) fn set_callbacks_full(
        &self,
        init: unsafe extern "system" fn(RawEnv, *mut JNIEnv, jobject),
        breakpoint: unsafe extern "system" fn(RawEnv, *mut JNIEnv, jobject, jmethodID, i64),
        load: Option<ClassLoad>,
        prepare: Option<ClassPrepare>,
    ) -> Result<(), String> {
        let mut callbacks = [std::ptr::null::<c_void>(); 37];
        callbacks[0] = init as *const c_void;
        callbacks[12] = breakpoint as *const c_void;
        if let Some(callback) = load {
            callbacks[4] = callback as *const c_void;
        }
        if let Some(callback) = prepare {
            callbacks[6] = callback as *const c_void;
        }
        // SAFETY: JVMTI slot 122 consumes the event callback struct synchronously.
        let call: unsafe extern "system" fn(RawEnv, *const c_void, i32) -> i32 =
            unsafe { std::mem::transmute(self.entry(122)) };
        // SAFETY: Events 50 through 86 use one function pointer per field.
        check(
            unsafe {
                call(
                    self.0,
                    callbacks.as_ptr().cast(),
                    std::mem::size_of_val(&callbacks) as i32,
                )
            },
            "SetEventCallbacks",
        )?;
        self.event(50, true)?;
        self.event(62, true)?;
        if load.is_some() {
            self.event(54, true)?;
        }
        if prepare.is_some() {
            self.event(56, true)?;
        }
        Ok(())
    }

    fn event(&self, event: i32, enabled: bool) -> Result<(), String> {
        // SAFETY: JVMTI slot 2 is the variadic SetEventNotificationMode ABI.
        let call: unsafe extern "C" fn(RawEnv, i32, i32, jobject, ...) -> i32 =
            unsafe { std::mem::transmute(self.entry(2)) };
        // SAFETY: Null event_thread selects global delivery for this event type.
        check(
            unsafe { call(self.0, i32::from(enabled), event, std::ptr::null_mut()) },
            "SetEventNotificationMode",
        )
    }

    pub(crate) fn breakpoint(&self, method: jmethodID, location: i64) -> Result<(), String> {
        // SAFETY: JVMTI slot 38 is SetBreakpoint with a jlocation of signed 64 bits.
        let call: unsafe extern "system" fn(RawEnv, jmethodID, i64) -> i32 =
            unsafe { std::mem::transmute(self.entry(38)) };
        // SAFETY: JNI resolved this live method and location zero is its entry.
        check(unsafe { call(self.0, method, location) }, "SetBreakpoint")
    }

    pub(crate) fn clear_breakpoint(&self, method: jmethodID, location: i64) -> Result<(), String> {
        // SAFETY: JVMTI slot 39 is ClearBreakpoint.
        let call: unsafe extern "system" fn(RawEnv, jmethodID, i64) -> i32 =
            unsafe { std::mem::transmute(self.entry(39)) };
        // SAFETY: The method and location match an installed breakpoint.
        check(unsafe { call(self.0, method, location) }, "ClearBreakpoint")
    }

    pub(crate) fn bytecodes(&self, method: jmethodID) -> Result<Vec<u8>, String> {
        let mut count = 0_i32;
        let mut bytes = std::ptr::null_mut::<u8>();
        // SAFETY: JVMTI slot 75 is GetBytecodes and initializes both output fields.
        let call: unsafe extern "system" fn(RawEnv, jmethodID, *mut i32, *mut *mut u8) -> i32 =
            unsafe { std::mem::transmute(self.entry(75)) };
        // SAFETY: JNI resolved a live method. JVMTI owns the returned allocation.
        check(
            unsafe { call(self.0, method, &mut count, &mut bytes) },
            "GetBytecodes",
        )?;
        if count < 0 || (count != 0 && bytes.is_null()) {
            return Err("invalid JVMTI bytecode allocation".to_owned());
        }
        let result = if count == 0 {
            Vec::new()
        } else {
            // SAFETY: Successful GetBytecodes initialized exactly count bytes.
            unsafe { std::slice::from_raw_parts(bytes, count as usize) }.to_vec()
        };
        if !bytes.is_null() {
            self.deallocate(bytes)?;
        }
        Ok(result)
    }

    pub(crate) fn deallocate(&self, allocation: *mut u8) -> Result<(), String> {
        // SAFETY: JVMTI slot 47 is Deallocate.
        let call: unsafe extern "system" fn(RawEnv, *mut u8) -> i32 =
            unsafe { std::mem::transmute(self.entry(47)) };
        // SAFETY: Callers pass allocations returned by this JVMTI environment.
        check(unsafe { call(self.0, allocation) }, "Deallocate")
    }

    pub(crate) fn allocate(&self, size: usize) -> Result<*mut u8, String> {
        let mut allocation = std::ptr::null_mut();
        let size = i64::try_from(size).map_err(|_| "JVMTI allocation size overflow")?;
        // SAFETY: JVMTI slot 46 is Allocate with a signed 64-bit size.
        let call: unsafe extern "system" fn(RawEnv, i64, *mut *mut u8) -> i32 =
            unsafe { std::mem::transmute(self.entry(46)) };
        // SAFETY: The positive bounded size and writable output satisfy Allocate.
        check(unsafe { call(self.0, size, &mut allocation) }, "Allocate")?;
        if allocation.is_null() {
            return Err("JVMTI returned null allocation".to_owned());
        }
        Ok(allocation)
    }

    pub(crate) fn class_signature(&self, class: jclass) -> Result<String, String> {
        let mut name = std::ptr::null_mut::<c_char>();
        // SAFETY: JVMTI slot 48 is GetClassSignature.
        let call: unsafe extern "system" fn(
            RawEnv,
            jclass,
            *mut *mut c_char,
            *mut *mut c_char,
        ) -> i32 = unsafe { std::mem::transmute(self.entry(48)) };
        // SAFETY: ClassPrepare supplies a live jclass and JVMTI initializes name.
        check(
            unsafe { call(self.0, class, &mut name, std::ptr::null_mut()) },
            "GetClassSignature",
        )?;
        if name.is_null() {
            return Err("JVMTI returned null class signature".to_owned());
        }
        // SAFETY: Successful GetClassSignature returns a NUL-terminated JVMTI allocation.
        let result = unsafe { std::ffi::CStr::from_ptr(name) }
            .to_string_lossy()
            .into_owned();
        self.deallocate(name.cast())?;
        Ok(result)
    }

    pub(crate) fn local_object(&self, thread: jobject, slot: i32) -> Result<jobject, String> {
        let mut value = std::ptr::null_mut();
        // SAFETY: JVMTI slot 21 is GetLocalObject.
        let call: unsafe extern "system" fn(RawEnv, jobject, i32, i32, *mut jobject) -> i32 =
            unsafe { std::mem::transmute(self.entry(21)) };
        // SAFETY: Breakpoint callbacks run with the event thread suspended at depth zero.
        check(
            unsafe { call(self.0, thread, 0, slot, &mut value) },
            "GetLocalObject",
        )?;
        Ok(value)
    }

    pub(crate) fn local_int(&self, thread: jobject, slot: i32) -> Result<i32, String> {
        let mut value = 0;
        // SAFETY: JVMTI slot 22 is GetLocalInt.
        let call: unsafe extern "system" fn(RawEnv, jobject, i32, i32, *mut i32) -> i32 =
            unsafe { std::mem::transmute(self.entry(22)) };
        // SAFETY: Breakpoint callbacks run with the event thread suspended at depth zero.
        check(
            unsafe { call(self.0, thread, 0, slot, &mut value) },
            "GetLocalInt",
        )?;
        Ok(value)
    }

    pub(crate) fn local_long(&self, thread: jobject, slot: i32) -> Result<i64, String> {
        let mut value = 0;
        // SAFETY: JVMTI 1.2 slot 23 is GetLocalLong with a writable jlong output.
        let call: unsafe extern "system" fn(RawEnv, jobject, i32, i32, *mut i64) -> i32 =
            unsafe { std::mem::transmute(self.entry(23)) };
        // SAFETY: The caller supplies the current suspended breakpoint thread and a verified long argument slot.
        check(
            unsafe { call(self.0, thread, 0, slot, &mut value) },
            "GetLocalLong",
        )?;
        Ok(value)
    }

    pub(crate) fn method_modifiers(&self, method: jmethodID) -> Result<i32, String> {
        let mut modifiers = 0;
        // SAFETY: JVMTI slot 66 is GetMethodModifiers.
        let call: unsafe extern "system" fn(RawEnv, jmethodID, *mut i32) -> i32 =
            unsafe { std::mem::transmute(self.entry(66)) };
        // SAFETY: Callers supply a live JNI-resolved method ID and writable output.
        check(
            unsafe { call(self.0, method, &mut modifiers) },
            "GetMethodModifiers",
        )?;
        Ok(modifiers)
    }

    pub(crate) fn method_declaring_class(&self, method: jmethodID) -> Result<jclass, String> {
        let mut class = std::ptr::null_mut();
        // SAFETY: JVMTI slot 65 is GetMethodDeclaringClass.
        let call: unsafe extern "system" fn(RawEnv, jmethodID, *mut jclass) -> i32 =
            unsafe { std::mem::transmute(self.entry(65)) };
        // SAFETY: A live JNI-resolved method ID is supplied; output is initialized by JVMTI.
        check(
            unsafe { call(self.0, method, &mut class) },
            "GetMethodDeclaringClass",
        )?;
        if class.is_null() {
            return Err("JVMTI returned null declaring class".to_owned());
        }
        Ok(class)
    }

    pub(crate) fn set_local_int(
        &self,
        thread: jobject,
        slot: i32,
        value: i32,
    ) -> Result<(), String> {
        // SAFETY: JVMTI slot 27 is SetLocalInt.
        let call: unsafe extern "system" fn(RawEnv, jobject, i32, i32, i32) -> i32 =
            unsafe { std::mem::transmute(self.entry(27)) };
        // SAFETY: The breakpoint event suspends the current thread at the validated int slot.
        check(
            unsafe { call(self.0, thread, 0, slot, value) },
            "SetLocalInt",
        )
    }

    pub(crate) fn return_int(&self, thread: jobject, result: i32) -> Result<(), String> {
        // SAFETY: JVMTI slot 82 is ForceEarlyReturnInt.
        let call: unsafe extern "system" fn(RawEnv, jobject, i32) -> i32 =
            unsafe { std::mem::transmute(self.entry(82)) };
        // SAFETY: The suspended call method returns int and runs on this thread.
        check(
            unsafe { call(self.0, thread, result) },
            "ForceEarlyReturnInt",
        )
    }

    /// Returns None when normal host lifecycle has not prepared the class yet.
    pub(crate) fn class_methods(&self, class: jclass) -> Result<Option<Vec<jmethodID>>, String> {
        let mut count = 0;
        let mut methods = std::ptr::null_mut::<jmethodID>();
        // SAFETY: JVMTI slot 52 is GetClassMethods and initializes the count and
        // allocation for the live class reference supplied by JNI.
        let call: unsafe extern "system" fn(RawEnv, jclass, *mut i32, *mut *mut jmethodID) -> i32 =
            unsafe { std::mem::transmute(self.entry(52)) };
        // SAFETY: class remains live for this call and both outputs are writable.
        let status = unsafe { call(self.0, class, &mut count, &mut methods) };
        if status == 22 {
            return Ok(None);
        }
        check(status, "GetClassMethods")?;
        if count < 0 || (count != 0 && methods.is_null()) {
            return Err("Invalid JVMTI class method allocation".into());
        }
        let result = if count == 0 {
            Vec::new()
        } else {
            // SAFETY: Successful GetClassMethods initialized count method handles.
            unsafe { std::slice::from_raw_parts(methods, count as usize) }.to_vec()
        };
        if !methods.is_null() {
            self.deallocate(methods.cast())?;
        }
        Ok(Some(result))
    }

    pub(crate) fn method_name(&self, method: jmethodID) -> Result<(String, String), String> {
        let mut name = std::ptr::null_mut::<c_char>();
        let mut signature = std::ptr::null_mut::<c_char>();
        // SAFETY: JVMTI slot 64 is GetMethodName. Both requested strings are
        // Modified UTF-8 allocations owned by this JVMTI environment.
        let call: unsafe extern "system" fn(
            RawEnv,
            jmethodID,
            *mut *mut c_char,
            *mut *mut c_char,
            *mut *mut c_char,
        ) -> i32 = unsafe { std::mem::transmute(self.entry(64)) };
        // SAFETY: A live class owns method and all requested output fields exist.
        check(
            unsafe {
                call(
                    self.0,
                    method,
                    &mut name,
                    &mut signature,
                    std::ptr::null_mut(),
                )
            },
            "GetMethodName",
        )?;
        let result = if name.is_null() || signature.is_null() {
            Err("JVMTI returned a null method name or descriptor".into())
        } else {
            // SAFETY: Successful GetMethodName produced NUL-terminated strings
            // retained until the matching Deallocate calls below.
            let name = unsafe { jni::strings::JNIStr::from_ptr(name) };
            // SAFETY: signature has the same documented allocation lifetime.
            let signature = unsafe { jni::strings::JNIStr::from_ptr(signature) };
            Ok((
                std::borrow::Cow::<str>::from(name).into_owned(),
                std::borrow::Cow::<str>::from(signature).into_owned(),
            ))
        };
        let name_result = if name.is_null() {
            Ok(())
        } else {
            self.deallocate(name.cast())
        };
        let signature_result = if signature.is_null() {
            Ok(())
        } else {
            self.deallocate(signature.cast())
        };
        name_result?;
        signature_result?;
        result
    }
}

fn check(status: i32, operation: &str) -> Result<(), String> {
    if status == 0 {
        Ok(())
    } else {
        Err(format!("{operation} failed with JVMTI error {status}"))
    }
}

pub(crate) type CallbackEnv = RawEnv;
