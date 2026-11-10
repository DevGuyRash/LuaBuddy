use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Target {
    pub class: String,
    pub method: String,
    pub descriptor: String,
    pub loader: String,
    pub bytecode_sha256: String,
}

impl Target {
    pub fn validate(&self) -> Result<()> {
        if self.class.is_empty()
            || self.class.contains('.')
            || self.class.starts_with('/')
            || self.class.ends_with('/')
            || self.method.is_empty()
            || self.loader.is_empty()
            || !valid_method_descriptor(&self.descriptor)
            || self.bytecode_sha256.len() != 64
            || !self.bytecode_sha256.bytes().all(|c| c.is_ascii_hexdigit())
        {
            return Err(Error::Invalid(
                "Invalid exact JVM target or SHA256 fingerprint".into(),
            ));
        }
        Ok(())
    }

    fn same_method(&self, other: &Self) -> bool {
        self.class == other.class
            && self.method == other.method
            && self.descriptor == other.descriptor
            && self.loader == other.loader
    }
}

pub fn bytecode_sha256(bytecode: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytecode))
}

fn valid_method_descriptor(value: &str) -> bool {
    if value.len() > 65_535 || value.contains('\0') {
        return false;
    }
    fn field(bytes: &[u8], position: &mut usize, allow_void: bool) -> bool {
        let Some(&tag) = bytes.get(*position) else {
            return false;
        };
        *position += 1;
        match tag {
            b'V' => allow_void,
            b'B' | b'C' | b'D' | b'F' | b'I' | b'J' | b'S' | b'Z' => true,
            b'L' => {
                let begin = *position;
                while let Some(&byte) = bytes.get(*position) {
                    *position += 1;
                    if byte == b';' {
                        return *position > begin + 1;
                    }
                    if byte == b'.' || byte == b'[' || byte == b'(' || byte == b')' {
                        return false;
                    }
                }
                false
            }
            b'[' => {
                let mut dimensions = 1;
                while bytes.get(*position) == Some(&b'[') {
                    dimensions += 1;
                    *position += 1;
                }
                dimensions <= 255 && field(bytes, position, false)
            }
            _ => false,
        }
    }
    let bytes = value.as_bytes();
    if bytes.first() != Some(&b'(') {
        return false;
    }
    let mut position = 1;
    while bytes.get(position) != Some(&b')') {
        if !field(bytes, &mut position, false) {
            return false;
        }
    }
    position += 1;
    field(bytes, &mut position, true) && position == bytes.len()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookKind {
    Before,
    AfterNormalReturn,
    Replace,
    ModifyArguments,
    ModifyResult,
    Around,
    AfterExceptionalReturn,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Capabilities {
    pub hooks: BTreeSet<HookKind>,
    pub reversible: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reentrancy {
    #[default]
    SkipSelf,
    Reject,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PatchSpec {
    pub id: String,
    pub owner: String,
    pub target: Target,
    pub kind: HookKind,
    #[serde(default)]
    pub requires: Vec<String>,
    #[serde(default)]
    pub priority: i32,
    #[serde(default)]
    pub reentrancy: Reentrancy,
}

impl PatchSpec {
    pub fn validate(&self) -> Result<()> {
        for value in std::iter::once(&self.id)
            .chain(std::iter::once(&self.owner))
            .chain(&self.requires)
        {
            if value.is_empty()
                || !value
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._:/-".contains(&b))
            {
                return Err(Error::Invalid(format!(
                    "Invalid stable patch identity {value:?}"
                )));
            }
        }
        if self.requires.iter().any(|id| id == &self.id) {
            return Err(Error::Invalid(format!("{} requires itself", self.id)));
        }
        self.target.validate()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum Value {
    Void,
    Int(i32),
    Long(i64),
    Float(f32),
    Double(f64),
    /// A host-managed reference token. It is never a process address.
    Object(u64),
}

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Void, Self::Void) => true,
            (Self::Int(first), Self::Int(second)) => first == second,
            (Self::Long(first), Self::Long(second)) => first == second,
            (Self::Float(first), Self::Float(second)) => first.to_bits() == second.to_bits(),
            (Self::Double(first), Self::Double(second)) => first.to_bits() == second.to_bits(),
            (Self::Object(first), Self::Object(second)) => first == second,
            _ => false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    Entry,
    NormalReturn,
    ExceptionalReturn,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Invocation {
    pub target: Target,
    pub stage: Stage,
    pub arguments: Vec<Value>,
    pub result: Option<Value>,
    pub exception: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum HookControl {
    Continue,
    Return(Value),
}

pub trait Hook: Send + Sync {
    fn invoke(&self, invocation: &mut Invocation) -> Result<HookControl>;
}

impl<F> Hook for F
where
    F: Fn(&mut Invocation) -> Result<HookControl> + Send + Sync,
{
    fn invoke(&self, invocation: &mut Invocation) -> Result<HookControl> {
        self(invocation)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObservedTarget {
    pub target: Target,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstallReceipt {
    pub token: u64,
}

/// The adapter must validate class identity and perform each operation atomically.
/// A failed install leaves no hook behind. A failed uninstall leaves the receipt live.
pub trait PatchBackend {
    fn capabilities(&self) -> Capabilities;
    fn inspect(&mut self, target: &Target) -> Result<ObservedTarget>;
    fn install(&mut self, patch: &PatchSpec) -> Result<InstallReceipt>;
    fn uninstall(&mut self, receipt: &InstallReceipt) -> Result<()>;
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", content = "reason", rename_all = "snake_case")]
pub enum PatchState {
    Registered,
    Installed,
    Rejected(String),
    Failed(String),
    Disabled,
    Quarantined(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PatchDiagnostic {
    pub id: String,
    pub owner: String,
    pub target: Target,
    pub state: PatchState,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    Invalid(String),
    Duplicate(String),
    MissingDependency(String),
    Cycle(Vec<String>),
    Conflict(String),
    Unsupported(String),
    StaleTarget(String),
    Backend(String),
    Hook(String),
    Panic(String),
    Reentrant(String),
    Permission(String),
    Abi(String),
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{self:?}")
    }
}
impl std::error::Error for Error {}
pub type Result<T> = std::result::Result<T, Error>;

#[derive(Clone)]
struct Record {
    spec: PatchSpec,
    hook: Arc<dyn Hook>,
    state: PatchState,
    receipt: Option<InstallReceipt>,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct RegistryId(u64);

impl RegistryId {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self(
            NEXT.try_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
                .expect("Registry identity space exhausted; restart the process"),
        )
    }
}

#[derive(Clone)]
pub struct Registry {
    identity: RegistryId,
    records: BTreeMap<String, Record>,
}

impl Default for Registry {
    fn default() -> Self {
        Self {
            identity: RegistryId::new(),
            records: BTreeMap::new(),
        }
    }
}

impl Registry {
    pub fn register(&mut self, spec: PatchSpec, hook: Arc<dyn Hook>) -> Result<()> {
        spec.validate()?;
        if self.records.contains_key(&spec.id) {
            return Err(Error::Duplicate(spec.id));
        }
        self.records.insert(
            spec.id.clone(),
            Record {
                spec,
                hook,
                state: PatchState::Registered,
                receipt: None,
            },
        );
        Ok(())
    }

    pub fn diagnostics(&self) -> Vec<PatchDiagnostic> {
        self.records
            .values()
            .map(|record| PatchDiagnostic {
                id: record.spec.id.clone(),
                owner: record.spec.owner.clone(),
                target: record.spec.target.clone(),
                state: record.state.clone(),
            })
            .collect()
    }

    pub fn plan(&self, requested: &[String], capabilities: &Capabilities) -> Result<Vec<String>> {
        if let Some(record) = self
            .records
            .values()
            .find(|record| matches!(record.state, PatchState::Quarantined(_)))
        {
            return Err(Error::Conflict(format!(
                "{} has an uncertain native receipt; disable it successfully or restart before enabling patches",
                record.spec.id
            )));
        }
        let mut selected = BTreeSet::new();
        let mut pending = requested.to_vec();
        while let Some(id) = pending.pop() {
            let record = self
                .records
                .get(&id)
                .ok_or_else(|| Error::MissingDependency(id.clone()))?;
            if matches!(record.state, PatchState::Quarantined(_)) {
                return Err(Error::Conflict(format!("{id} is quarantined")));
            }
            if record.state == PatchState::Installed || !selected.insert(id) {
                continue;
            }
            if record.spec.kind == HookKind::Around {
                return Err(Error::Unsupported(
                    "Around hooks have no proven dispatcher".into(),
                ));
            }
            if !capabilities.hooks.contains(&record.spec.kind) {
                return Err(Error::Unsupported(format!(
                    "{} requires {:?}",
                    record.spec.id, record.spec.kind
                )));
            }
            pending.extend(record.spec.requires.iter().cloned());
        }
        let mut all: Vec<&Record> = self
            .records
            .values()
            .filter(|r| r.state == PatchState::Installed || selected.contains(&r.spec.id))
            .collect();
        all.sort_by_key(|r| &r.spec.id);
        for (i, first) in all.iter().enumerate() {
            for second in &all[i + 1..] {
                if first.spec.target.same_method(&second.spec.target)
                    && (first.spec.target.bytecode_sha256 != second.spec.target.bytecode_sha256
                        || (exclusive(first.spec.kind)
                            && exclusive(second.spec.kind)
                            && first.spec.kind == second.spec.kind))
                {
                    return Err(Error::Conflict(format!(
                        "{} conflicts with {}",
                        first.spec.id, second.spec.id
                    )));
                }
            }
        }
        let mut ordered = Vec::new();
        while !selected.is_empty() {
            let next = selected
                .iter()
                .filter(|id| {
                    self.records[*id]
                        .spec
                        .requires
                        .iter()
                        .all(|dependency| !selected.contains(dependency))
                })
                .min_by_key(|id| (self.records[*id].spec.priority, *id))
                .cloned();
            let Some(next) = next else {
                return Err(Error::Cycle(selected.into_iter().collect()));
            };
            selected.remove(&next);
            ordered.push(next);
        }
        Ok(ordered)
    }

    pub fn activate(
        &mut self,
        requested: &[String],
        backend: &mut dyn PatchBackend,
    ) -> Result<Vec<String>> {
        let capabilities = backend.capabilities();
        let order = match self.plan(requested, &capabilities) {
            Ok(order) => order,
            Err(error) => {
                self.reject_requested(requested, &error);
                return Err(error);
            }
        };
        for id in &order {
            let expected = &self.records[id].spec.target;
            let observed = match backend.inspect(expected) {
                Ok(observed) => observed,
                Err(error) => {
                    self.reject_requested(requested, &error);
                    return Err(error);
                }
            };
            if observed.target != *expected {
                let error = Error::StaleTarget(id.clone());
                self.reject_requested(requested, &error);
                return Err(error);
            }
        }
        if order.len() > 1 && !capabilities.reversible {
            let error = Error::Unsupported(
                "Atomic multi-patch activation needs reversible installation".into(),
            );
            self.reject_requested(requested, &error);
            return Err(error);
        }
        let mut installed = Vec::new();
        for id in &order {
            match backend.install(&self.records[id].spec) {
                Ok(receipt) => {
                    let record = self.records.get_mut(id).expect("planned patch exists");
                    record.receipt = Some(receipt);
                    record.state = PatchState::Installed;
                    installed.push(id.clone());
                }
                Err(error) => {
                    self.records
                        .get_mut(id)
                        .expect("planned patch exists")
                        .state = PatchState::Failed(error.to_string());
                    let mut rollback_failures = Vec::new();
                    for installed_id in installed.iter().rev() {
                        let record = self
                            .records
                            .get_mut(installed_id)
                            .expect("installed patch exists");
                        match backend.uninstall(
                            record
                                .receipt
                                .as_ref()
                                .expect("installed patch has receipt"),
                        ) {
                            Ok(()) => {
                                record.receipt = None;
                                record.state = PatchState::Registered;
                            }
                            Err(rollback) => {
                                record.state = PatchState::Quarantined(rollback.to_string());
                                rollback_failures.push(installed_id.clone());
                            }
                        }
                    }
                    if !rollback_failures.is_empty() {
                        return Err(Error::Backend(format!(
                            "{error}; rollback uncertain for {rollback_failures:?}; restart required"
                        )));
                    }
                    return Err(error);
                }
            }
        }
        Ok(installed)
    }

    fn reject_requested(&mut self, requested: &[String], error: &Error) {
        for id in requested {
            if let Some(record) = self.records.get_mut(id)
                && record.state != PatchState::Installed
                && !matches!(record.state, PatchState::Quarantined(_))
            {
                record.state = PatchState::Rejected(error.to_string());
            }
        }
    }

    pub fn disable(&mut self, id: &str, backend: &mut dyn PatchBackend) -> Result<()> {
        let record = self
            .records
            .get(id)
            .ok_or_else(|| Error::Invalid(format!("Unknown patch {id}")))?;
        if record.state == PatchState::Disabled {
            return Ok(());
        }
        if self.records.values().any(|other| {
            other.receipt.is_some()
                && other
                    .spec
                    .requires
                    .iter()
                    .any(|dependency| dependency == id)
        }) {
            return Err(Error::Conflict(format!("Installed patches require {id}")));
        }
        if let Some(receipt) = &record.receipt {
            if !backend.capabilities().reversible {
                return Err(Error::Unsupported(
                    "Backend cannot disable a live patch".into(),
                ));
            }
            backend.uninstall(receipt)?;
        }
        let record = self.records.get_mut(id).expect("checked patch exists");
        record.receipt = None;
        record.state = PatchState::Disabled;
        Ok(())
    }

    pub fn dispatch(&self, invocation: &mut Invocation) -> Result<HookControl> {
        self.dispatch_filtered(invocation, None)
    }

    /// Dispatches an immutable registry snapshot through the backend's current
    /// native receipts. Already-entered callbacks may finish after disable.
    pub fn dispatch_selected(
        &self,
        invocation: &mut Invocation,
        installed: &BTreeSet<String>,
    ) -> Result<HookControl> {
        self.dispatch_filtered(invocation, Some(installed))
    }

    fn dispatch_filtered(
        &self,
        invocation: &mut Invocation,
        installed: Option<&BTreeSet<String>>,
    ) -> Result<HookControl> {
        validate_invocation(invocation)?;
        let mut records: Vec<&Record> = self
            .records
            .values()
            .filter(|record| {
                record.state == PatchState::Installed
                    && installed.is_none_or(|ids| ids.contains(&record.spec.id))
                    && record.spec.target == invocation.target
                    && applies(record.spec.kind, invocation.stage)
            })
            .collect();
        let mut pending: BTreeSet<_> = records
            .iter()
            .map(|record| record.spec.id.clone())
            .collect();
        let mut order = BTreeMap::new();
        while !pending.is_empty() {
            let next = records
                .iter()
                .filter(|record| {
                    pending.contains(&record.spec.id)
                        && record
                            .spec
                            .requires
                            .iter()
                            .all(|dependency| !pending.contains(dependency))
                })
                .min_by_key(|record| (record.spec.priority, &record.spec.id))
                .ok_or_else(|| Error::Cycle(pending.iter().cloned().collect()))?;
            pending.remove(&next.spec.id);
            order.insert(next.spec.id.clone(), order.len());
        }
        records.sort_by_key(|record| order[&record.spec.id]);
        if invocation.stage == Stage::NormalReturn {
            records.reverse()
        }
        for record in records {
            let guard = match InvocationGuard::enter(
                self.identity,
                &record.spec.id,
                record.spec.reentrancy,
            )? {
                Some(guard) => guard,
                None => continue,
            };
            let original = invocation.clone();
            let result = catch_unwind(AssertUnwindSafe(|| record.hook.invoke(invocation)));
            drop(guard);
            let control = match result {
                Ok(Ok(control)) => control,
                Ok(Err(error)) => {
                    *invocation = original;
                    return Err(error);
                }
                Err(_) => {
                    *invocation = original;
                    return Err(Error::Panic(record.spec.id.clone()));
                }
            };
            if let Err(error) = validate_changes(&record.spec, &original, invocation, &control) {
                *invocation = original;
                return Err(error);
            }
            if let HookControl::Return(value) = control {
                return Ok(HookControl::Return(value));
            }
        }
        Ok(HookControl::Continue)
    }
}

fn exclusive(kind: HookKind) -> bool {
    matches!(
        kind,
        HookKind::Replace | HookKind::Around | HookKind::ModifyArguments | HookKind::ModifyResult
    )
}

fn applies(kind: HookKind, stage: Stage) -> bool {
    matches!(
        (kind, stage),
        (
            HookKind::Before | HookKind::Replace | HookKind::ModifyArguments,
            Stage::Entry
        ) | (
            HookKind::AfterNormalReturn | HookKind::ModifyResult,
            Stage::NormalReturn
        ) | (HookKind::AfterExceptionalReturn, Stage::ExceptionalReturn)
    )
}

fn validate_changes(
    spec: &PatchSpec,
    original: &Invocation,
    changed: &Invocation,
    control: &HookControl,
) -> Result<()> {
    if changed.target != original.target
        || changed.stage != original.stage
        || changed.exception != original.exception
    {
        return Err(Error::Hook(format!(
            "{} changed immutable invocation metadata",
            spec.id
        )));
    }
    if changed.arguments != original.arguments && spec.kind != HookKind::ModifyArguments {
        return Err(Error::Hook(format!(
            "{} did not declare argument mutation",
            spec.id
        )));
    }
    if changed.result != original.result && spec.kind != HookKind::ModifyResult {
        return Err(Error::Hook(format!(
            "{} did not declare result mutation",
            spec.id
        )));
    }
    if matches!(control, HookControl::Return(_)) && spec.kind != HookKind::Replace {
        return Err(Error::Hook(format!(
            "{} did not declare replacement",
            spec.id
        )));
    }
    validate_invocation(changed)?;
    if let HookControl::Return(value) = control {
        let (_, return_tag) = descriptor_tags(&changed.target.descriptor)?;
        if !matches_tag(value, return_tag) {
            return Err(Error::Hook(format!(
                "{} returned a value with the wrong JVM type",
                spec.id
            )));
        }
    }
    Ok(())
}

fn descriptor_tags(descriptor: &str) -> Result<(Vec<u8>, u8)> {
    if !valid_method_descriptor(descriptor) {
        return Err(Error::Invalid("Invalid invocation descriptor".into()));
    }
    let bytes = descriptor.as_bytes();
    let mut tags = Vec::new();
    let mut position = 1;
    while bytes[position] != b')' {
        let start = position;
        while bytes[position] == b'[' {
            position += 1
        }
        if bytes[position] == b'L' {
            while bytes[position] != b';' {
                position += 1
            }
        }
        position += 1;
        tags.push(bytes[start]);
    }
    Ok((tags, bytes[position + 1]))
}

fn matches_tag(value: &Value, tag: u8) -> bool {
    matches!(
        (value, tag),
        (Value::Void, b'V')
            | (Value::Int(_), b'Z' | b'B' | b'C' | b'S' | b'I')
            | (Value::Long(_), b'J')
            | (Value::Float(_), b'F')
            | (Value::Double(_), b'D')
            | (Value::Object(_), b'L' | b'[')
    )
}

fn validate_invocation(invocation: &Invocation) -> Result<()> {
    let (tags, return_tag) = descriptor_tags(&invocation.target.descriptor)?;
    if tags.len() != invocation.arguments.len()
        || invocation
            .arguments
            .iter()
            .zip(tags)
            .any(|(value, tag)| !matches_tag(value, tag))
        || invocation
            .result
            .as_ref()
            .is_some_and(|value| !matches_tag(value, return_tag))
        || (invocation.stage == Stage::Entry
            && (invocation.result.is_some() || invocation.exception.is_some()))
        || (invocation.stage == Stage::NormalReturn && invocation.exception.is_some())
    {
        return Err(Error::Hook(
            "Invocation values do not match the exact JVM signature or stage".into(),
        ));
    }
    Ok(())
}

thread_local! {
    static ACTIVE_PATCHES: RefCell<BTreeSet<(RegistryId, String)>> = const { RefCell::new(BTreeSet::new()) };
}

struct InvocationGuard((RegistryId, String));
impl InvocationGuard {
    fn enter(registry: RegistryId, id: &str, policy: Reentrancy) -> Result<Option<Self>> {
        ACTIVE_PATCHES.with(|active| {
            let key = (registry, id.to_owned());
            if !active.borrow_mut().insert(key.clone()) {
                return match policy {
                    Reentrancy::SkipSelf => Ok(None),
                    Reentrancy::Reject => Err(Error::Reentrant(id.to_owned())),
                };
            }
            Ok(Some(Self(key)))
        })
    }
}
impl Drop for InvocationGuard {
    fn drop(&mut self) {
        ACTIVE_PATCHES.with(|active| {
            active.borrow_mut().remove(&self.0);
        });
    }
}
