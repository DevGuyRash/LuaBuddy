//! ABI v1 uses fixed-size C-layout structures and host-owned callback frames.

use crate::{
    Error, Hook, HookControl, HookKind, Invocation, PatchSpec, Registry, Result, Stage, Value,
};
use libloading::Library;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fs;
use std::mem::ManuallyDrop;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub const ABI_VERSION: u32 = 1;
pub const MAX_PATCHES: usize = 32;
pub const MAX_ARGUMENTS: usize = 32;
pub const CONTINUE: i32 = 0;
pub const RETURN: i32 = 1;

/// An opaque object token is valid only for this callback and is not a JNI pointer.
#[derive(Clone, Copy, Debug, Default)]
#[repr(C)]
pub struct ValueV1 {
    pub tag: u32,
    pub reserved: u32,
    pub bits: u64,
}

impl ValueV1 {
    fn encode(value: &Value) -> Self {
        let (tag, bits) = match *value {
            Value::Void => (0, 0),
            Value::Int(value) => (1, value as u32 as u64),
            Value::Long(value) => (2, value as u64),
            Value::Float(value) => (3, value.to_bits() as u64),
            Value::Double(value) => (4, value.to_bits()),
            Value::Object(value) => (5, value),
        };
        Self {
            tag,
            reserved: 0,
            bits,
        }
    }

    fn decode(self) -> Result<Value> {
        if self.reserved != 0 {
            return Err(Error::Abi("Value reserved field must be zero".into()));
        }
        match self.tag {
            0 if self.bits == 0 => Ok(Value::Void),
            1 if self.bits <= u32::MAX as u64 => Ok(Value::Int(self.bits as u32 as i32)),
            2 => Ok(Value::Long(self.bits as i64)),
            3 if self.bits <= u32::MAX as u64 => Ok(Value::Float(f32::from_bits(self.bits as u32))),
            4 => Ok(Value::Double(f64::from_bits(self.bits))),
            5 => Ok(Value::Object(self.bits)),
            _ => Err(Error::Abi(format!(
                "Invalid ABI value tag {} or payload",
                self.tag
            ))),
        }
    }
}

/// Callback memory belongs to the host and cannot be retained past the call.
#[repr(C)]
pub struct FrameV1 {
    pub abi_version: u32,
    pub struct_size: u32,
    pub stage: u32,
    pub argument_count: u32,
    pub arguments: [ValueV1; MAX_ARGUMENTS],
    pub has_result: u32,
    pub reserved: u32,
    pub result: ValueV1,
}

/// A callback returns 0 to continue, 1 to replace, or a negative error code.
///
/// # Safety
/// Implementations accept a non-null aligned `FrameV1` valid for the call, do not
/// retain any pointer, and never unwind. They support concurrent calls on distinct
/// frames and nested calls. Object tokens must come from this frame.
pub type CallbackV1 = unsafe extern "C" fn(*mut FrameV1) -> i32;

#[derive(Clone, Copy)]
#[repr(C)]
pub struct ExtensionV1 {
    pub abi_version: u32,
    pub struct_size: u32,
    pub id: [u8; 64],
    pub patch_count: u32,
    pub reserved: u32,
    pub callbacks: [Option<CallbackV1>; MAX_PATCHES],
}

impl ExtensionV1 {
    pub const fn new(
        id: [u8; 64],
        patch_count: u32,
        callbacks: [Option<CallbackV1>; MAX_PATCHES],
    ) -> Self {
        Self {
            abi_version: ABI_VERSION,
            struct_size: std::mem::size_of::<Self>() as u32,
            id,
            patch_count,
            reserved: 0,
            callbacks,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Privilege {
    ObserveMethods,
    ReplaceMethods,
    ModifyArguments,
    ModifyResults,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExtensionManifest {
    pub schema_version: u32,
    pub abi_version: u32,
    pub id: String,
    pub artifact_sha256: String,
    pub provenance: String,
    pub target_os: String,
    pub target_arch: String,
    pub privileges: BTreeSet<Privilege>,
    pub patches: Vec<PatchSpec>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnableConsent {
    pub extension_id: String,
    pub artifact_sha256: String,
    pub provenance: String,
    pub allowed_privileges: BTreeSet<Privilege>,
    pub install: bool,
    pub enable: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApprovalRecordV1 {
    pub schema_version: u32,
    pub manifest_sha256: String,
    pub consent: EnableConsent,
}

impl ApprovalRecordV1 {
    pub fn validate(&self) -> Result<()> {
        if self.schema_version != 1
            || self.manifest_sha256.len() != 64
            || !self
                .manifest_sha256
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        {
            return Err(Error::Permission(
                "Native approval must bind one exact manifest SHA256 under schema v1".into(),
            ));
        }
        Ok(())
    }
}

pub struct VerifiedExtension {
    path: PathBuf,
    manifest: ExtensionManifest,
    bytes: Vec<u8>,
}

impl VerifiedExtension {
    pub fn manifest(&self) -> &ExtensionManifest {
        &self.manifest
    }
}

/// Verifies bytes and explicit intent before any native code can execute.
pub fn verify(
    path: &Path,
    manifest: ExtensionManifest,
    consent: &EnableConsent,
) -> Result<VerifiedExtension> {
    if manifest.schema_version != 1 || manifest.abi_version != ABI_VERSION {
        return Err(Error::Abi(
            "Unsupported extension manifest or ABI version".into(),
        ));
    }
    if manifest.id.is_empty()
        || manifest.id.len() >= 64
        || manifest
            .id
            .bytes()
            .any(|byte| !byte.is_ascii_alphanumeric() && !b"._-".contains(&byte))
    {
        return Err(Error::Invalid(
            "Extension ID must fit the ABI identity field".into(),
        ));
    }
    if manifest.provenance.trim().is_empty()
        || !consent.install
        || !consent.enable
        || consent.extension_id != manifest.id
        || consent.artifact_sha256 != manifest.artifact_sha256
        || consent.provenance != manifest.provenance
        || !manifest.privileges.is_subset(&consent.allowed_privileges)
    {
        return Err(Error::Permission(
            "Install and enable approval must match the exact artifact, provenance, and privileges"
                .into(),
        ));
    }
    if manifest.target_os != std::env::consts::OS || manifest.target_arch != std::env::consts::ARCH
    {
        return Err(Error::Abi(
            "Extension OS or process architecture does not match".into(),
        ));
    }
    if manifest.patches.is_empty() || manifest.patches.len() > MAX_PATCHES {
        return Err(Error::Abi(
            "Extension patch count is outside ABI bounds".into(),
        ));
    }
    let mut ids = BTreeSet::new();
    for patch in &manifest.patches {
        patch.validate()?;
        if patch.owner != manifest.id || !ids.insert(&patch.id) {
            return Err(Error::Permission(
                "Extension must own unique patch IDs".into(),
            ));
        }
        let privilege = match patch.kind {
            HookKind::Before | HookKind::AfterNormalReturn | HookKind::AfterExceptionalReturn => {
                Privilege::ObserveMethods
            }
            HookKind::Replace | HookKind::Around => Privilege::ReplaceMethods,
            HookKind::ModifyArguments => Privilege::ModifyArguments,
            HookKind::ModifyResult => Privilege::ModifyResults,
        };
        if !manifest.privileges.contains(&privilege) {
            return Err(Error::Permission(format!(
                "Patch {} did not declare {privilege:?}",
                patch.id
            )));
        }
    }
    let path = path
        .canonicalize()
        .map_err(|error| Error::Invalid(format!("Open extension {}: {error}", path.display())))?;
    let bytes = fs::read(&path)
        .map_err(|error| Error::Invalid(format!("Read extension {}: {error}", path.display())))?;
    let hash = format!("{:x}", Sha256::digest(&bytes));
    if hash != manifest.artifact_sha256 {
        return Err(Error::Permission(
            "Extension artifact SHA256 does not match approval".into(),
        ));
    }
    Ok(VerifiedExtension {
        path,
        manifest,
        bytes,
    })
}

pub struct LoadedExtension {
    manifest: ExtensionManifest,
    library: Arc<ManuallyDrop<Library>>,
    callbacks: Vec<CallbackV1>,
}

impl LoadedExtension {
    /// Loads an approved native library, whose initializers may execute immediately.
    ///
    /// # Safety
    /// The caller must establish that the approved binary and its transitive native
    /// dependencies implement this ABI, never unwind over it, and cannot be modified
    /// until loading finishes. Callbacks support concurrent and nested calls. The
    /// library has the process's full OS privileges.
    /// Approval is a trust decision, not a native-code sandbox.
    pub unsafe fn load(verified: VerifiedExtension) -> Result<Self> {
        let current = fs::read(&verified.path)
            .map_err(|error| Error::Invalid(format!("Recheck extension: {error}")))?;
        if current != verified.bytes {
            return Err(Error::Permission(
                "Extension changed after verification".into(),
            ));
        }
        // SAFETY: The caller guarantees the approved artifact, dependencies, and ABI.
        let library = Arc::new(ManuallyDrop::new(
            unsafe { Library::new(&verified.path) }
                .map_err(|error| Error::Abi(format!("Load approved extension: {error}")))?,
        ));
        // SAFETY: The caller guarantees the entrypoint signature and valid return value.
        let entry = unsafe {
            library.get::<unsafe extern "C" fn() -> ExtensionV1>(b"luabuddy_extension_v1\0")
        }
        .map_err(|error| Error::Abi(format!("Extension entrypoint: {error}")))?;
        // SAFETY: The caller guarantees that this entrypoint obeys ABI v1.
        let descriptor = unsafe { entry() };
        let expected = &verified.manifest;
        let id_end = descriptor
            .id
            .iter()
            .position(|byte| *byte == 0)
            .ok_or_else(|| Error::Abi("Extension ID is not terminated".into()))?;
        let id = std::str::from_utf8(&descriptor.id[..id_end])
            .map_err(|_| Error::Abi("Extension ID is not UTF-8".into()))?;
        if descriptor.abi_version != ABI_VERSION
            || descriptor.struct_size as usize != std::mem::size_of::<ExtensionV1>()
            || descriptor.reserved != 0
            || id != expected.id
            || descriptor.patch_count as usize != expected.patches.len()
            || descriptor.callbacks[descriptor.patch_count as usize..]
                .iter()
                .any(Option::is_some)
        {
            return Err(Error::Abi(
                "Extension descriptor does not match the approved manifest".into(),
            ));
        }
        let callbacks = descriptor.callbacks[..descriptor.patch_count as usize]
            .iter()
            .map(|callback| callback.ok_or_else(|| Error::Abi("Extension callback is null".into())))
            .collect::<Result<Vec<_>>>()?;
        Ok(Self {
            manifest: verified.manifest,
            library,
            callbacks,
        })
    }

    pub fn register(self, registry: &mut Registry) -> Result<()> {
        let existing: BTreeSet<_> = registry
            .diagnostics()
            .into_iter()
            .map(|entry| entry.id)
            .collect();
        if self
            .manifest
            .patches
            .iter()
            .any(|patch| existing.contains(&patch.id))
        {
            return Err(Error::Duplicate(
                "Extension patch already registered".into(),
            ));
        }
        for (spec, callback) in self.manifest.patches.into_iter().zip(self.callbacks) {
            registry.register(
                spec,
                Arc::new(ExtensionHook {
                    _library: self.library.clone(),
                    callback,
                }),
            )?;
        }
        Ok(())
    }
}

struct ExtensionHook {
    _library: Arc<ManuallyDrop<Library>>,
    callback: CallbackV1,
}

impl Hook for ExtensionHook {
    fn invoke(&self, invocation: &mut Invocation) -> Result<HookControl> {
        if invocation.arguments.len() > MAX_ARGUMENTS {
            return Err(Error::Unsupported(
                "Extension argument limit exceeded".into(),
            ));
        }
        let stage = match invocation.stage {
            Stage::Entry => 0,
            Stage::NormalReturn => 1,
            Stage::ExceptionalReturn => 2,
        };
        let mut frame = FrameV1 {
            abi_version: ABI_VERSION,
            struct_size: std::mem::size_of::<FrameV1>() as u32,
            stage,
            argument_count: invocation.arguments.len() as u32,
            arguments: [ValueV1::default(); MAX_ARGUMENTS],
            has_result: u32::from(invocation.result.is_some()),
            reserved: 0,
            result: ValueV1::encode(invocation.result.as_ref().unwrap_or(&Value::Void)),
        };
        for (destination, value) in frame.arguments.iter_mut().zip(&invocation.arguments) {
            *destination = ValueV1::encode(value)
        }
        // SAFETY: load() established the callback contract and keeps its library live.
        // The stack-owned aligned frame is valid exclusively for the callback's duration.
        let code = unsafe { (self.callback)(&mut frame) };
        if frame.abi_version != ABI_VERSION
            || frame.struct_size as usize != std::mem::size_of::<FrameV1>()
            || frame.stage != stage
            || frame.argument_count as usize != invocation.arguments.len()
            || frame.reserved != 0
            || frame.has_result > 1
        {
            return Err(Error::Abi(
                "Extension corrupted immutable frame metadata".into(),
            ));
        }
        let arguments = frame.arguments[..invocation.arguments.len()]
            .iter()
            .map(|value| value.decode())
            .collect::<Result<Vec<_>>>()?;
        let result = if frame.has_result == 1 {
            Some(frame.result.decode()?)
        } else {
            None
        };
        let known_tokens: BTreeSet<u64> = invocation
            .arguments
            .iter()
            .chain(invocation.result.iter())
            .filter_map(|value| {
                if let Value::Object(token) = value {
                    Some(*token)
                } else {
                    None
                }
            })
            .chain(std::iter::once(0))
            .collect();
        if arguments
            .iter()
            .chain(result.iter())
            .any(|value| matches!(value, Value::Object(token) if !known_tokens.contains(token)))
        {
            return Err(Error::Abi("Extension fabricated an object token".into()));
        }
        let control = match code {
            CONTINUE => HookControl::Continue,
            RETURN if frame.has_result == 1 => HookControl::Return(frame.result.decode()?),
            _ => {
                return Err(Error::Hook(format!(
                    "Extension callback failed with status {code}"
                )));
            }
        };
        invocation.arguments = arguments;
        if code == CONTINUE {
            invocation.result = result
        }
        Ok(control)
    }
}
