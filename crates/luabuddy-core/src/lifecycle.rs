use crate::source::{Generation, ModuleKey, Owner, Source};
use crate::{CoreError, ModId, ResolvedProfile};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::thread::{self, ThreadId};

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub enum Operation {
    Startup,
    Refresh,
    Reload(ModId),
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub enum GenerationState {
    Loaded,
    Failed(String),
    Quarantined(String),
}

#[derive(Debug, Clone)]
pub struct RunningGeneration {
    pub owner: Owner,
    pub profile: ResolvedProfile,
    pub sources: Vec<Source>,
    pub cleanup_supported: bool,
    pub activation: Operation,
    pub state: GenerationState,
}

/// `compiled` is host-owned prepared code. The host must compile every source before submitting a candidate.
#[derive(Debug, Clone)]
pub struct Candidate<C> {
    pub profile: ResolvedProfile,
    pub sources: Vec<Source>,
    pub compiled: C,
    pub cleanup_supported: bool,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct OperationToken {
    sequence: u64,
    environment: u64,
}

#[derive(Debug)]
pub struct PreparedGeneration<C> {
    candidate: Candidate<C>,
    owner: Owner,
    replaces: Option<Owner>,
    token: OperationToken,
}

impl<C> PreparedGeneration<C> {
    pub fn id(&self) -> &ModId {
        &self.owner.id
    }
    pub fn owner(&self) -> &Owner {
        &self.owner
    }
    pub fn replaces(&self) -> Option<&Owner> {
        self.replaces.as_ref()
    }
    pub fn candidate(&self) -> &Candidate<C> {
        &self.candidate
    }
}

#[derive(Debug)]
pub enum CleanupOutcome {
    Accepted,
    Refused(String),
    Uncertain(String),
}

#[derive(Debug)]
pub enum CleanupDecision<C> {
    Activate(ActivationPermit<C>),
    Retained(CoreError),
    Quarantined(CoreError),
}

#[derive(Debug)]
pub struct ActivationPermit<C> {
    prepared: Box<PreparedGeneration<C>>,
}

impl<C> ActivationPermit<C> {
    pub fn candidate(&self) -> &Candidate<C> {
        &self.prepared.candidate
    }
    pub fn owner(&self) -> &Owner {
        &self.prepared.owner
    }
}

#[derive(Debug)]
pub struct RequestContext {
    pub environment: u64,
    pub multiplayer_client: bool,
    pub pending_options: bool,
}

#[derive(Debug, Clone)]
pub struct Session {
    environment: u64,
    owner_thread: ThreadId,
    ready: bool,
    retired: bool,
    sequence: u64,
    generation: u64,
    operation: Option<(OperationToken, Operation)>,
    native_loaded: BTreeSet<ModId>,
    running: BTreeMap<ModId, RunningGeneration>,
}

impl Session {
    pub fn new(environment: u64, native_loaded: BTreeSet<ModId>) -> Self {
        Self {
            environment,
            owner_thread: thread::current().id(),
            ready: false,
            retired: false,
            sequence: 0,
            generation: 0,
            operation: None,
            native_loaded,
            running: BTreeMap::new(),
        }
    }
    pub fn running(&self) -> &BTreeMap<ModId, RunningGeneration> {
        &self.running
    }
    pub fn ready(&self) -> bool {
        self.ready
    }

    pub fn is_busy(&self) -> bool {
        self.operation.is_some()
    }
    pub fn set_ready(&mut self, ready: bool) -> Result<(), CoreError> {
        self.check_thread()?;
        self.ready = ready;
        Ok(())
    }
    pub fn retire(&mut self) -> Result<(), CoreError> {
        self.check_thread()?;
        self.retired = true;
        self.ready = false;
        Ok(())
    }

    pub fn begin(
        &mut self,
        context: RequestContext,
        operation: Operation,
    ) -> Result<OperationToken, CoreError> {
        self.check_thread()?;
        if self.retired || context.environment != self.environment || !context.multiplayer_client {
            return Err(CoreError::new(
                "lifecycle",
                "This LuaBuddy session is no longer active",
            ));
        }
        if self.operation.is_some() {
            return Err(CoreError::new(
                "lifecycle",
                "A LuaBuddy operation is already running",
            ));
        }
        if operation != Operation::Startup && !self.ready {
            return Err(CoreError::new(
                "lifecycle",
                "Wait until game startup dispatch has finished before loading profiles",
            ));
        }
        if context.pending_options && operation != Operation::Startup {
            return Err(CoreError::new(
                "options",
                "Apply or discard pending Options changes before loading profiles",
            ));
        }
        self.sequence = self
            .sequence
            .checked_add(1)
            .ok_or_else(|| CoreError::new("lifecycle", "Operation counter exhausted; restart"))?;
        let token = OperationToken {
            sequence: self.sequence,
            environment: self.environment,
        };
        self.operation = Some((token.clone(), operation));
        Ok(token)
    }

    pub fn prepare<C>(
        &mut self,
        token: &OperationToken,
        candidates: Vec<Candidate<C>>,
    ) -> Result<Vec<PreparedGeneration<C>>, CoreError> {
        let operation = self.check_operation(token)?.clone();
        if let Operation::Reload(target) = &operation {
            if candidates.len() != 1 || &candidates[0].profile.profile.id != target {
                return Err(CoreError::new(
                    "reload",
                    format!("Reload requires exactly the enabled configured target {target}"),
                ));
            }
            if !candidates[0].profile.profile.enabled {
                return Err(CoreError::new(
                    "reload",
                    format!("Reload requires an enabled profile: {target}"),
                ));
            }
        }
        let replacing = match &operation {
            Operation::Reload(id) => Some(id),
            _ => None,
        };
        let mut providers = BTreeMap::new();
        let mut paths = BTreeMap::new();
        let mut available = self.native_loaded.clone();
        for (id, running) in &self.running {
            if running.state != GenerationState::Loaded {
                continue;
            }
            available.insert(id.clone());
            if replacing == Some(id) {
                continue;
            }
            for source in &running.sources {
                let key = ModuleKey::new(&source.name)?;
                paths.insert(key.clone(), running.owner.clone());
                if let Some(alias) = key.alias() {
                    let previous: Option<&(ModuleKey, Owner)> = providers.get(&alias);
                    if previous.is_none_or(|(previous, _)| key.shared() || !previous.shared()) {
                        providers.insert(alias, (key, running.owner.clone()));
                    }
                }
            }
        }
        let mut prepared = Vec::new();
        let mut batch_ids = BTreeSet::new();
        for candidate in candidates {
            let id = candidate.profile.profile.id.clone();
            if !batch_ids.insert(id.clone()) {
                return Err(CoreError::new(
                    "prepare",
                    format!("Duplicate candidate {id}"),
                ));
            }
            if !candidate.profile.profile.enabled {
                continue;
            }
            if self.native_loaded.contains(&id) {
                if replacing.is_some() {
                    return Err(CoreError::new(
                        "reload",
                        "Server-loaded profiles cannot reload",
                    ));
                }
                continue;
            }
            let previous = self.running.get(&id);
            if replacing.is_none() && previous.is_some() {
                continue;
            }
            if let Some(target) = replacing {
                let old = previous.ok_or_else(|| {
                    CoreError::new(
                        "reload",
                        format!("Only a locally loaded profile can reload: {target}"),
                    )
                })?;
                if old.state != GenerationState::Loaded {
                    return Err(CoreError::new(
                        "reload",
                        "Failed or quarantined generations require reconnecting",
                    ));
                }
                if !old.cleanup_supported {
                    return Err(CoreError::new(
                        "reload",
                        format!("Profile has no Rust cleanup hook; reconnect to reload {target}"),
                    ));
                }
                for running in self
                    .running
                    .values()
                    .filter(|r| r.owner.id != *target && r.state == GenerationState::Loaded)
                {
                    if running.profile.requires.contains(target)
                        || old.profile.before.contains(&running.owner.id)
                    {
                        return Err(CoreError::new(
                            "reload",
                            format!(
                                "Running profile {} depends on the current generation; reconnect",
                                running.owner.id
                            ),
                        ));
                    }
                    if candidate.sources.iter().chain(&old.sources).any(|source| {
                        running
                            .sources
                            .iter()
                            .any(|other| source.filename == other.filename)
                    }) {
                        return Err(CoreError::new(
                            "reload",
                            format!("Source is shared with {}; reconnect", running.owner.id),
                        ));
                    }
                }
            }
            let missing: Vec<_> = candidate
                .profile
                .requires
                .difference(&available)
                .map(ToString::to_string)
                .collect();
            if !missing.is_empty() {
                return Err(CoreError::new(
                    "dependencies",
                    format!(
                        "Missing running dependencies for {id}: {}",
                        missing.join(", ")
                    ),
                ));
            }
            for existing in &available {
                if Some(existing) == replacing {
                    continue;
                }
                if candidate.profile.before.contains(existing)
                    || self
                        .running
                        .get(existing)
                        .is_some_and(|running| running.profile.after.contains(&id))
                {
                    return Err(CoreError::new(
                        "order",
                        format!(
                            "Order requires {id} before already loaded {existing}; reconnect with a compatible selection"
                        ),
                    ));
                }
            }
            for source in &candidate.sources {
                let key = ModuleKey::new(&source.name)?;
                if let Some(owner) = paths.get(&key) {
                    return Err(CoreError::new(
                        "source",
                        format!(
                            "Runtime source {} conflicts with loaded {}; reconnect",
                            source.name, owner.id
                        ),
                    ));
                }
                if let Some(alias) = key.alias()
                    && let Some((previous, owner)) = providers.get(&alias)
                    && (key.shared() || !previous.shared())
                {
                    return Err(CoreError::new(
                        "source",
                        format!(
                            "Runtime import {} would replace provider {}; reconnect",
                            alias.as_str(),
                            owner.id
                        ),
                    ));
                }
            }
            self.generation = self.generation.checked_add(1).ok_or_else(|| {
                CoreError::new("lifecycle", "Generation counter exhausted; restart")
            })?;
            let owner = Owner {
                id: id.clone(),
                generation: Generation(self.generation),
            };
            available.insert(id);
            prepared.push(PreparedGeneration {
                candidate,
                owner,
                replaces: previous.map(|r| r.owner.clone()),
                token: token.clone(),
            });
        }
        Ok(prepared)
    }

    /// Cleanup executes in the host before this call. Only accepted cleanup permits replacement.
    pub fn after_cleanup<C>(
        &mut self,
        prepared: PreparedGeneration<C>,
        outcome: CleanupOutcome,
    ) -> Result<CleanupDecision<C>, CoreError> {
        self.check_operation(&prepared.token)?;
        if prepared.replaces.is_none() {
            if !matches!(outcome, CleanupOutcome::Accepted) {
                return Err(CoreError::new(
                    "cleanup",
                    "A new generation has no prior cleanup to refuse",
                ));
            }
            return Ok(CleanupDecision::Activate(ActivationPermit {
                prepared: Box::new(prepared),
            }));
        }
        match outcome {
            CleanupOutcome::Accepted => {
                if let Some(old) = self.running.get_mut(&prepared.owner.id) {
                    old.state = GenerationState::Quarantined(
                        "Cleanup completed; replacement activation has not completed".to_owned(),
                    );
                }
                Ok(CleanupDecision::Activate(ActivationPermit {
                    prepared: Box::new(prepared),
                }))
            }
            CleanupOutcome::Refused(reason) => Ok(CleanupDecision::Retained(CoreError::new(
                "cleanup",
                format!("Cleanup refused; running generation retained: {reason}"),
            ))),
            CleanupOutcome::Uncertain(reason) => {
                if let Some(old) = self.running.get_mut(&prepared.owner.id) {
                    old.state = GenerationState::Quarantined(reason.clone());
                }
                Ok(CleanupDecision::Quarantined(CoreError::new(
                    "cleanup",
                    format!(
                        "Cleanup partially changed state or failed: {reason}; reconnect before retrying"
                    ),
                )))
            }
        }
    }

    pub fn finish_activation<C>(
        &mut self,
        permit: ActivationPermit<C>,
        result: Result<(), String>,
    ) -> Result<C, CoreError> {
        let PreparedGeneration {
            candidate,
            owner,
            replaces: _,
            token,
        } = *permit.prepared;
        let operation = self.check_operation(&token)?.clone();
        let state = match result {
            Ok(()) => GenerationState::Loaded,
            Err(error) => GenerationState::Quarantined(error),
        };
        self.running.insert(
            owner.id.clone(),
            RunningGeneration {
                owner,
                profile: candidate.profile,
                sources: candidate.sources,
                cleanup_supported: candidate.cleanup_supported,
                activation: operation,
                state,
            },
        );
        Ok(candidate.compiled)
    }

    pub fn end(&mut self, token: &OperationToken) -> Result<(), CoreError> {
        self.check_operation(token)?;
        self.operation = None;
        Ok(())
    }

    fn check_thread(&self) -> Result<(), CoreError> {
        if thread::current().id() != self.owner_thread {
            return Err(CoreError::new(
                "thread",
                "Run LuaBuddy commands on the owning game thread",
            ));
        }
        Ok(())
    }

    fn check_operation(&self, token: &OperationToken) -> Result<&Operation, CoreError> {
        self.check_thread()?;
        if self.retired {
            return Err(CoreError::new(
                "lifecycle",
                "This LuaBuddy session is retired",
            ));
        }
        match &self.operation {
            Some((current, operation)) if current == token => Ok(operation),
            _ => Err(CoreError::new(
                "lifecycle",
                "Stale or inactive operation token",
            )),
        }
    }
}
