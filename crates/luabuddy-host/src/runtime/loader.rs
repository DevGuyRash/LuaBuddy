//! Carries out one loading operation, recording each decision as it is made.

use super::{
    CooperativeCleanup, RunState, execute_owned, generation_live, require_value, stage, status,
    validate_session, with,
};
use crate::{
    activation::{self, Eligibility},
    game, hooks,
    log::{self, Operation as Intent},
    lua::{Context, Result, Value},
    observe, profile_store,
    report::OperationReport,
    selections,
};
use luabuddy_core::{
    ModId, Profile, ReplayEvent, ResolvedProfile, SkipReason,
    diagnostics::{ModDiagnostic, ModStatus, ProfileDiagnostic},
    journal::Level,
    lifecycle::{
        ActivationPermit, Candidate, CleanupDecision, CleanupOutcome, GenerationState, Operation,
        OperationToken, PreparedGeneration, RequestContext,
    },
    resolve,
    source::{ModuleKey, Owner, Source, scan_sources},
};
use serde_json::json;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
    time::{Duration, Instant},
};

type Compiled = Vec<(ModuleKey, Value)>;
type Prepared = Candidate<Compiled>;
type Permit = ActivationPermit<Compiled>;
type Outcomes = BTreeMap<ModId, Result<()>>;

/// A file slower than this is named in its mod's record.
const SLOW_FILE: Duration = Duration::from_millis(20);

/// Threads that read mod files. They wait far more than they compute.
const READERS: usize = 16;

fn name(operation: &Operation) -> &'static str {
    match operation {
        Operation::Startup => "startup",
        Operation::Refresh => "refresh",
        Operation::Reload(_) => "reload",
    }
}

pub fn apply(
    context: &mut Context<'_, '_>,
    session_id: u64,
    operation: Operation,
) -> Result<String> {
    validate_session(context, session_id)?;
    let target = match &operation {
        Operation::Reload(id) => Some(id.clone()),
        _ => None,
    };
    let intent = Intent::begin(
        "loader",
        match &operation {
            Operation::Startup => "Restore the selected mods before the world starts".into(),
            Operation::Refresh => "Load selected mods that are not running yet".into(),
            Operation::Reload(id) => format!("Replace the running {id} with its current files"),
        },
        target.as_ref(),
        json!({ "operation": name(&operation), "world": observe::world(context) }),
    );
    let cpu = observe::thread_cpu(context);
    let result = begin(context, session_id, &operation);
    let cpu = cpu
        .zip(observe::thread_cpu(context))
        .map(|(before, after)| ms(after - before));
    let data = json!({ "cpuMs": cpu, "world": observe::world(context) });
    match &result {
        Ok(summary) => intent.finish(Level::Info, summary.clone(), data),
        Err(error) => intent.finish(Level::Error, error.clone(), data),
    }
    result
}

fn begin(context: &mut Context<'_, '_>, session_id: u64, operation: &Operation) -> Result<String> {
    let pending_options = crate::ui::pending_options(context)?;
    let multiplayer_client = game::client(context)?;
    if !multiplayer_client {
        return Err("Mods can be loaded after joining a multiplayer server. Elsewhere, start them with the game from its own mod list.".into());
    }
    let token = with(session_id, |s| {
        s.policy
            .begin(
                RequestContext {
                    environment: session_id,
                    multiplayer_client,
                    pending_options,
                },
                operation.clone(),
            )
            .map_err(|e| e.to_string())
    })?;
    // Every path from here must end the operation, or no later one could start.
    let result = tracked(context, session_id, &token, operation);
    let ended = with(session_id, |s| {
        s.policy.end(&token).map_err(|e| e.to_string())
    });
    result.and_then(|summary| ended.map(|()| summary))
}

/// Keeps `diagnostics.json` describing the state this operation leaves behind.
fn tracked(
    context: &mut Context<'_, '_>,
    session_id: u64,
    token: &OperationToken,
    operation: &Operation,
) -> Result<String> {
    // The report names the same canonical folder selections are read from.
    let directory = crate::selections::directory(context).unwrap_or_default();
    let previous = with(session_id, |s| Ok(s.details.clone()))?;
    let retained = match operation {
        Operation::Reload(target) => with(session_id, |s| {
            Ok(s.policy
                .running()
                .get(target)
                .map(|running| (running.owner.clone(), previous.get(target).cloned())))
        })?,
        _ => None,
    };
    let report = OperationReport::begin(context, session_id, operation, directory, previous);
    let mut run = Run {
        session_id,
        operation,
        failures: BTreeMap::new(),
        unchanged: Vec::new(),
        restart: Vec::new(),
        unreadable: Vec::new(),
        spent: Vec::new(),
        loaded: Vec::new(),
    };
    let result = run.carry_out(context, token);
    if result.is_err()
        && let Some((owner, Some(detail))) = retained
    {
        restore_retained(context, session_id, owner, detail)?;
    }
    match report {
        Ok(mut report) => {
            for (id, detail) in with(session_id, |s| Ok(s.details.clone()))? {
                report.record(id, detail);
            }
            if let Err(error) = report.finish(context, result.as_ref().err().map(String::as_str)) {
                log::warn(
                    "diagnostics.write",
                    format!("diagnostics.json was not updated: {error}"),
                )
                .write();
            }
        }
        Err(error) => log::warn(
            "diagnostics.begin",
            format!("diagnostics.json was not prepared: {error}"),
        )
        .write(),
    }
    let state = with(session_id, |s| Ok(s.state.clone()))?;
    if let Err(error) = crate::extensions::publish(context, &state) {
        log::warn("extensions.publish", error).write();
    }
    result
}

/// A refused reload leaves the old generation running, so it must read as such.
fn restore_retained(
    context: &mut Context<'_, '_>,
    session_id: u64,
    owner: Owner,
    detail: ModDiagnostic,
) -> Result<()> {
    let live = with(session_id, |s| {
        let live = generation_live(s, &owner);
        if live {
            s.details.insert(owner.id.clone(), detail);
        }
        Ok(live)
    })?;
    if live {
        status(context, session_id, &owner.id, RunState::Loaded)?;
    }
    Ok(())
}

struct Run<'r> {
    session_id: u64,
    operation: &'r Operation,
    failures: BTreeMap<ModId, String>,
    /// Failures that were already known before this operation.
    unchanged: Vec<ModId>,
    restart: Vec<ModId>,
    /// Selection files that could not be read.
    unreadable: Vec<(PathBuf, String)>,
    /// Selected mods whose code already ran and failed in this environment.
    spent: Vec<ModId>,
    loaded: Vec<ModId>,
}

/// Everything the operation reads before it changes anything.
struct Inputs {
    templates: Vec<Profile>,
    active: BTreeSet<ModId>,
    available: BTreeSet<ModId>,
    discovered: game::Discovered,
    ordered: Vec<ResolvedProfile>,
}

impl Run<'_> {
    fn target(&self) -> Option<&ModId> {
        match self.operation {
            Operation::Reload(target) => Some(target),
            _ => None,
        }
    }

    fn startup(&self) -> bool {
        *self.operation == Operation::Startup
    }

    /// Records why this mod is not running.
    fn fail(
        &mut self,
        context: &mut Context<'_, '_>,
        id: &ModId,
        error: impl Into<String>,
    ) -> Result<()> {
        let error = error.into();
        let (at, source) = with(self.session_id, |s| {
            Ok(s.details
                .get(id)
                .map(|detail| (detail.stage.clone(), detail.source.clone()))
                .unwrap_or_default())
        })?;
        // A failure seen before is retried with every load; saying so again is noise.
        let repeated = with(self.session_id, |s| {
            Ok(matches!(s.statuses.get(id), Some(RunState::Failed(before)) if *before == error))
        })?;
        let (level, message) = if repeated {
            (Level::Debug, format!("Still failing: {error}"))
        } else {
            (Level::Error, error.clone())
        };
        log::record(level, "mod.failed", message)
            .about(id)
            .with(json!({ "stage": at, "source": source }))
            .write();
        if repeated {
            self.unchanged.push(id.clone());
        }
        status(
            context,
            self.session_id,
            id,
            RunState::Failed(error.clone()),
        )?;
        self.failures.insert(id.clone(), error);
        Ok(())
    }

    fn carry_out(
        &mut self,
        context: &mut Context<'_, '_>,
        token: &OperationToken,
    ) -> Result<String> {
        let inputs = self.read(context)?;
        let wanted = inputs
            .ordered
            .iter()
            .filter(|profile| {
                self.target()
                    .is_none_or(|target| profile.profile.id == *target)
            })
            .collect::<Vec<_>>();
        let mut candidates = Vec::new();
        for (profile, files) in wanted.iter().zip(read_files(&wanted, &inputs)) {
            if let Some(candidate) = self.prepare(context, profile, files, &inputs)? {
                candidates.push(candidate);
            }
        }
        self.drop_unmet(context, &mut candidates)?;
        let prepared = self.accept(context, token, candidates)?;
        let mut permits = Vec::new();
        for prepared in prepared {
            permits.push(self.replace(context, prepared)?);
        }
        let mut outcomes = self.activate(context, &permits, &inputs)?;
        if !self.startup() {
            self.replay_start(context, &permits, &mut outcomes)?;
        }
        self.conclude(context, permits, outcomes)
    }

    fn read(&mut self, context: &mut Context<'_, '_>) -> Result<Inputs> {
        let started = Instant::now();
        let (mut profiles, unreadable) = selections::readable(context)?;
        for (file, error) in unreadable {
            let known = with(self.session_id, |s| {
                Ok(!s.unreadable.insert((file.clone(), error.clone())))
            })?;
            let level = if known { Level::Debug } else { Level::Error };
            log::record(level, "profile.unreadable", error.clone())
                .with(json!({ "file": file }))
                .write();
            self.unreadable.push((file, error));
        }
        with(self.session_id, |session| {
            for profile in &profiles {
                session
                    .details
                    .entry(profile.id.clone())
                    .or_insert_with(|| discovered(profile));
            }
            Ok(())
        })?;
        let templates = match profile_store::templates(context) {
            Ok(templates) => templates,
            Err(error) => {
                // Templates only supply defaults, so selections still load without them.
                log::warn("profile.templates", error).write();
                Vec::new()
            }
        };
        if !self.startup() {
            game::reset_discovery(context)?;
        }
        let active = game::active(context)?;
        let (existing, spent) = with(self.session_id, |s| {
            let spent: BTreeSet<_> = s
                .policy
                .running()
                .iter()
                .filter(|(_, running)| running.state != GenerationState::Loaded)
                .map(|(id, _)| id.clone())
                .collect();
            Ok((s.statuses.clone(), spent))
        })?;
        // Code that ran and failed can't be retried here. Anything that failed
        // before running can, because the cause may have been corrected.
        self.spent = profiles
            .iter()
            .filter(|profile| profile.enabled && spent.contains(&profile.id))
            .map(|profile| profile.id.clone())
            .collect();
        profiles.retain(|profile| !spent.contains(&profile.id));
        let loaded = |id: &ModId| existing.get(id).is_some_and(RunState::is_loaded);
        let mut available = active.clone();
        available.extend(existing.keys().filter(|id| loaded(id)).cloned());
        if let Operation::Reload(target) = self.operation {
            if active.contains(target) || !loaded(target) {
                return Err(format!(
                    "Reload requires a mod that LuaBuddy loaded: {target}"
                ));
            }
            available.remove(target);
        }
        let discovered = game::discover(context, &profiles, &active)?;
        let resolution = resolve(&profiles, &discovered.metadata, &available);
        log::debug("loader.resolved", "Decided what to load, and in which order")
            .took(started.elapsed())
            .with(json!({
                "order": resolution.ordered.iter().map(|p| p.profile.id.to_string()).collect::<Vec<_>>(),
                "skipped": resolution.skipped.iter().map(|(id, reason)| (id.to_string(), match reason {
                    SkipReason::Disabled => "disabled",
                    SkipReason::NativeLoaded => "already loaded by the game or server",
                })).collect::<BTreeMap<_, _>>(),
                "alreadyRunning": existing.iter().filter(|(_, state)| state.is_loaded()).map(|(id, _)| id.to_string()).collect::<Vec<_>>(),
                "needReconnect": spent.iter().map(ToString::to_string).collect::<Vec<_>>(),
                "nativeMods": active.len(),
            }))
            .write();
        // A reload describes only its target. Other mods keep what is known of them.
        if self.target().is_none() {
            for (id, reason) in &resolution.skipped {
                if !loaded(id) {
                    let state = match reason {
                        SkipReason::Disabled => RunState::Disabled,
                        SkipReason::NativeLoaded => RunState::ServerLoaded,
                    };
                    status(context, self.session_id, id, state)?;
                }
            }
        }
        for (id, error) in &resolution.failures {
            let reason = discovered
                .failures
                .get(id)
                .cloned()
                .unwrap_or_else(|| error.message.clone());
            if let Some(target) = self.target() {
                if id == target {
                    return Err(reason);
                }
                continue;
            }
            stage(self.session_id, id, &error.stage, error.path.as_deref())?;
            self.fail(context, id, reason)?;
        }
        Ok(Inputs {
            templates,
            active,
            available,
            discovered,
            ordered: resolution.ordered,
        })
    }

    /// Reads and compiles one mod without running any of it.
    fn prepare(
        &mut self,
        context: &mut Context<'_, '_>,
        profile: &ResolvedProfile,
        files: Files,
        inputs: &Inputs,
    ) -> Result<Option<Prepared>> {
        let id = &profile.profile.id;
        with(self.session_id, |session| {
            if let Some(detail) = session.details.get_mut(id) {
                detail.profile = profile.profile.file.clone();
                detail.configuration = configuration(profile);
            }
            Ok(())
        })?;
        match self.compile(context, profile, files, inputs) {
            Ok(candidate) => Ok(candidate),
            Err(error) if self.target().is_some() => Err(error),
            Err(error) => {
                self.fail(context, id, error)?;
                Ok(None)
            }
        }
    }

    fn compile(
        &mut self,
        context: &mut Context<'_, '_>,
        profile: &ResolvedProfile,
        files: Files,
        inputs: &Inputs,
    ) -> Result<Option<Prepared>> {
        let id = &profile.profile.id;
        let cpu = observe::thread_cpu(context);
        let started = Instant::now();
        stage(self.session_id, id, "eligibility", None)?;
        let eligibility = files.eligibility?;
        if let Eligibility::RestartRequired { reason } = &eligibility {
            log::warn("mod.needs-game-start", reason.clone())
                .about(id)
                .write();
            status(
                context,
                self.session_id,
                id,
                RunState::RestartRequired(reason.clone()),
            )?;
            self.restart.push(id.clone());
            return Ok(None);
        }
        if let Some(hook) = &profile.profile.hook
            && !hooks::IDS.contains(&hook.as_str())
        {
            return Err(format!("Rust profile hook {hook} is not installed"));
        }
        stage(self.session_id, id, "scan", None)?;
        let mut sources = files.sources?;
        let bytes: usize = sources.iter().map(|source| source.text.len()).sum();
        let compiling = Instant::now();
        let mut compiled = Vec::new();
        for source in &sources {
            let filename = script_name(&source.filename);
            stage(self.session_id, id, "compile", Some(&source.filename))?;
            compiled.push((
                ModuleKey::new(&source.name).map_err(|e| e.to_string())?,
                context.compile(&source.text, &filename)?,
            ));
        }
        for source in &mut sources {
            source.text.clear();
            source.text.shrink_to_fit();
        }
        let limitations = match &eligibility {
            Eligibility::Live { limitations } if !limitations.is_empty() => {
                Some(eligibility.explanation())
            }
            _ => None,
        };
        let timings = BTreeMap::from([
            ("assess".to_owned(), ms(files.assessed)),
            ("scan".to_owned(), ms(files.scanned)),
            ("compile".to_owned(), ms(compiling.elapsed())),
        ]);
        let cpu = cpu
            .zip(observe::thread_cpu(context))
            .map(|(before, after)| ms(after - before));
        log::debug(
            "mod.prepared",
            limitations
                .clone()
                .unwrap_or_else(|| "Compiled, ready to run".into()),
        )
        .about(id)
        .took(started.elapsed())
        .with(json!({
            "files": sources.len(),
            "bytes": bytes,
            "cpuMs": cpu,
            "timingsMs": timings,
            "roots": profile.roots,
            "requires": profile.requires,
            "before": profile.before,
            "after": profile.after,
            "excluded": profile.profile.exclusions(&inputs.active),
            "profile": profile.profile.file,
        }))
        .write();
        with(self.session_id, |session| {
            if let Some(detail) = session.details.get_mut(id) {
                detail.source_count = sources.len();
                detail.timings_ms = timings;
                detail.configuration.activation_limit = limitations;
            }
            Ok(())
        })?;
        Ok(Some(Candidate {
            profile: profile.clone(),
            sources,
            compiled,
            cleanup_supported: profile.profile.cleanup.is_some(),
        }))
    }

    /// Removes candidates whose requirements did not survive preparation.
    fn drop_unmet(
        &mut self,
        context: &mut Context<'_, '_>,
        candidates: &mut Vec<Prepared>,
    ) -> Result<()> {
        loop {
            let unavailable: BTreeSet<_> =
                self.failures.keys().chain(&self.restart).cloned().collect();
            let unmet: Vec<_> = candidates
                .iter()
                .filter_map(|candidate| {
                    let needed = candidate
                        .profile
                        .requires
                        .intersection(&unavailable)
                        .next()?;
                    Some((candidate.profile.profile.id.clone(), needed.clone()))
                })
                .collect();
            if unmet.is_empty() {
                return Ok(());
            }
            for (id, needed) in &unmet {
                self.fail(context, id, unmet_requirement(needed))?;
            }
            candidates.retain(|candidate| {
                !unmet
                    .iter()
                    .any(|(id, _)| *id == candidate.profile.profile.id)
            });
        }
    }

    fn accept(
        &mut self,
        context: &mut Context<'_, '_>,
        token: &OperationToken,
        candidates: Vec<Prepared>,
    ) -> Result<Vec<PreparedGeneration<Compiled>>> {
        if self.target().is_some() {
            return with(self.session_id, |s| {
                s.policy
                    .prepare(token, candidates)
                    .map_err(|e| e.to_string())
            });
        }
        let mut accepted: Vec<Prepared> = Vec::new();
        for candidate in candidates {
            let id = candidate.profile.profile.id.clone();
            let mut trial = accepted.clone();
            trial.push(candidate.clone());
            let result = with(self.session_id, |s| {
                s.policy
                    .clone()
                    .prepare(token, trial)
                    .map(|_| ())
                    .map_err(|e| e.to_string())
            });
            match result {
                Ok(()) => accepted.push(candidate),
                Err(error) => self.fail(context, &id, error)?,
            }
        }
        with(self.session_id, |s| {
            s.policy.prepare(token, accepted).map_err(|e| e.to_string())
        })
    }

    /// Lets the running generation clean up, then clears the way for its replacement.
    fn replace(
        &mut self,
        context: &mut Context<'_, '_>,
        prepared: PreparedGeneration<Compiled>,
    ) -> Result<Permit> {
        let id = prepared.id().clone();
        let previous = prepared.replaces().cloned();
        let outcome = match &previous {
            Some(owner) => self.clean_up(context, owner)?,
            None => CleanupOutcome::Accepted,
        };
        let decision = with(self.session_id, |s| {
            s.policy
                .after_cleanup(prepared, outcome)
                .map_err(|e| e.to_string())
        })?;
        match decision {
            CleanupDecision::Activate(permit) => match previous {
                Some(owner) => self.remove_old(context, owner, permit),
                None => Ok(permit),
            },
            CleanupDecision::Retained(error) => Err(format!("{id}: {error}")),
            CleanupDecision::Quarantined(error) => {
                if let Some(owner) = previous {
                    self.abandon(&owner, &error.to_string())?;
                }
                self.fail(context, &id, error.to_string())?;
                Err(format!("{id}: {error}"))
            }
        }
    }

    fn clean_up(&mut self, context: &mut Context<'_, '_>, owner: &Owner) -> Result<CleanupOutcome> {
        stage(self.session_id, &owner.id, "cleanup", None)?;
        let cleanup = with(self.session_id, |s| {
            s.cleanup.get(owner).cloned().ok_or_else(|| {
                "The running generation has no captured upstream cleanup callable".to_owned()
            })
        })?;
        let started = Instant::now();
        let outcome = match context.call(&cleanup.function, &[cleanup.context]) {
            Ok(values) => {
                let value = values.first().cloned().unwrap_or_default();
                match exact_boolean(context, &value)? {
                    Some(true) => CleanupOutcome::Accepted,
                    Some(false) => CleanupOutcome::Refused(match values.get(1) {
                        Some(reason) => context.text(reason)?,
                        None => "Finish the current action first".into(),
                    }),
                    None => CleanupOutcome::Uncertain(
                        "Upstream cleanup must return exactly true after completing cleanup".into(),
                    ),
                }
            }
            Err(error) => CleanupOutcome::Uncertain(error),
        };
        let (level, text) = match &outcome {
            CleanupOutcome::Accepted => (Level::Info, "The mod cleaned up after itself".into()),
            CleanupOutcome::Refused(reason) => (
                Level::Info,
                format!("The mod declined to clean up now: {reason}"),
            ),
            CleanupOutcome::Uncertain(reason) => (
                Level::Error,
                format!("The mod's cleanup did not complete: {reason}"),
            ),
        };
        log::record(level, "mod.cleanup", text)
            .about(&owner.id)
            .took(started.elapsed())
            .write();
        Ok(outcome)
    }

    fn remove_old(
        &mut self,
        context: &mut Context<'_, '_>,
        owner: Owner,
        permit: Permit,
    ) -> Result<Permit> {
        let files = with(self.session_id, |s| {
            Ok(s.policy
                .running()
                .get(&owner.id)
                .ok_or("Missing running generation")?
                .sources
                .iter()
                .map(|source| script_name(&source.filename))
                .collect())
        })?;
        match game::remove_callbacks(context, &files) {
            Ok(removed) => {
                log::debug(
                    "mod.callbacks-removed",
                    format!("Removed {removed} callbacks of the old generation"),
                )
                .about(&owner.id)
                .write();
            }
            Err(error) => {
                with(self.session_id, |s| {
                    s.modules.fail_owner(&owner, &error);
                    s.policy
                        .finish_activation(permit, Err(error.clone()))
                        .map_err(|e| e.to_string())?;
                    Ok(())
                })?;
                self.fail(
                    context,
                    &owner.id,
                    format!("Callback cleanup is uncertain: {error}; reconnect"),
                )?;
                return Err(error);
            }
        }
        with(self.session_id, |s| {
            s.modules.remove_owner(&owner);
            s.chunks.retain(|(previous, _), _| previous != &owner);
            s.cleanup.remove(&owner);
            s.after_start.retain(|(previous, _)| previous != &owner);
            s.after_tick.retain(|(previous, _)| previous != &owner);
            Ok(())
        })?;
        Ok(permit)
    }

    fn register(
        &self,
        context: &mut Context<'_, '_>,
        permit: &Permit,
        active_table: &Value,
    ) -> Result<Value> {
        let owner = permit.owner().clone();
        let sources = permit.candidate().sources.clone();
        let compiled = permit.candidate().compiled.clone();
        let state = with(self.session_id, |s| {
            if let Some(detail) = s.details.get_mut(&owner.id) {
                detail.generation = owner.generation.0;
            }
            s.modules
                .register(owner.clone(), sources)
                .map_err(|e| e.to_string())?;
            s.chunks.extend(
                compiled
                    .into_iter()
                    .map(|(key, chunk)| ((owner.clone(), key), chunk)),
            );
            Ok(s.state.clone())
        })?;
        let activation = context.table()?;
        for (key, value) in [
            ("id", context.string(owner.id.as_str())?),
            ("runtime", context.boolean(!self.startup())?),
            ("operation", context.string(name(self.operation))?),
            ("state", state),
            ("active", active_table.clone()),
        ] {
            context.set(&activation, key, &value)?;
        }
        Ok(activation)
    }

    /// Runs every mod's shared phase, then every mod's client phase.
    fn activate(
        &mut self,
        context: &mut Context<'_, '_>,
        permits: &[Permit],
        inputs: &Inputs,
    ) -> Result<Outcomes> {
        let active_table = context.table()?;
        for id in &inputs.available {
            let yes = context.boolean(true)?;
            context.set(&active_table, id.as_str(), &yes)?;
        }
        let mut contexts = BTreeMap::new();
        for permit in permits {
            let activation = self.register(context, permit, &active_table)?;
            contexts.insert(permit.owner().clone(), activation);
        }
        let mut outcomes = Outcomes::new();
        let mut timings: BTreeMap<ModId, BTreeMap<String, f64>> = BTreeMap::new();
        let mut footprint = observe::footprint(context);
        for phase in ["shared", "client"] {
            for permit in permits {
                let id = &permit.owner().id;
                if outcomes.get(id).is_some_and(|result| result.is_err()) {
                    continue;
                }
                let started = Instant::now();
                let cleanup = contexts.get(permit.owner()).cloned().unwrap_or_default();
                let mut steps = Steps::default();
                let unmet = permit.candidate().profile.requires.iter().find(|required| {
                    self.failures.contains_key(*required)
                        || outcomes
                            .get(*required)
                            .is_some_and(|result| result.is_err())
                });
                let result = match unmet {
                    Some(required) => Err(unmet_requirement(required)),
                    None if phase == "shared" => {
                        self.run_shared(context, permit, inputs, &mut steps)
                    }
                    None => self.run_client(context, permit, cleanup, &mut steps),
                };
                // The mod's own time, before LuaBuddy looks at what it changed.
                let took = started.elapsed();
                timings.entry(id.clone()).or_default().extend(
                    steps
                        .timings
                        .iter()
                        .map(|(step, took)| (step.clone(), *took)),
                );
                let after = observe::footprint(context);
                let changed = after.since(&footprint, context);
                self.record_phase(permit, phase, took, &steps, &result, changed);
                footprint = after;
                match &result {
                    Ok(()) if phase == "client" => {
                        let yes = context.boolean(true)?;
                        context.set(&active_table, id.as_str(), &yes)?;
                    }
                    Ok(()) => (),
                    Err(error) => {
                        self.abandon(permit.owner(), error)?;
                        context.set(&active_table, id.as_str(), &Value::default())?;
                    }
                }
                outcomes.insert(id.clone(), result);
            }
        }
        with(self.session_id, |s| {
            for (id, steps) in timings {
                if let Some(detail) = s.details.get_mut(&id) {
                    detail.timings_ms.extend(steps);
                }
            }
            Ok(())
        })?;
        Ok(outcomes)
    }

    #[allow(clippy::too_many_arguments)]
    /// Drops the modules this generation registered, so `require` can't return them.
    fn abandon(&self, owner: &Owner, error: &str) -> Result<()> {
        with(self.session_id, |s| {
            s.modules.fail_owner(owner, error);
            Ok(())
        })
    }

    fn record_phase(
        &self,
        permit: &Permit,
        phase: &str,
        took: Duration,
        steps: &Steps,
        result: &Result<()>,
        changed: serde_json::Value,
    ) {
        let id = &permit.owner().id;
        let (level, text) = match result {
            Ok(()) => (Level::Debug, format!("Ran its {phase} code")),
            Err(error) => (Level::Debug, format!("Its {phase} code stopped: {error}")),
        };
        log::record(level, &format!("mod.{phase}"), text)
            .about(id)
            .took(took)
            .with(json!({
                "generation": permit.owner().generation.0,
                "timingsMs": steps.timings,
                "slowFiles": steps.slow,
                "notes": steps.notes,
                "changed": changed,
            }))
            .write();
    }

    fn run_shared(
        &self,
        context: &mut Context<'_, '_>,
        permit: &Permit,
        inputs: &Inputs,
        steps: &mut Steps,
    ) -> Result<()> {
        let id = &permit.owner().id;
        let candidate = permit.candidate();
        let profile = &candidate.profile.profile;
        stage(self.session_id, id, "preload", None)?;
        steps.time("preload", || {
            for module in &profile.preload {
                require_value(context, self.session_id, module)
                    .map_err(|error| format!("Preload {module}: {error}"))?;
            }
            Ok(())
        })?;
        stage(self.session_id, id, "translations", None)?;
        let added = steps.time("translations", || {
            game::translations(context, &candidate.profile.roots)
        })?;
        steps.note("translations", json!(added));
        if profile.mount {
            stage(self.session_id, id, "file-mount", None)?;
            let info = inputs
                .discovered
                .infos
                .get(id)
                .ok_or("Missing native mod metadata")?;
            steps.time("mount", || game::mount(context, id, info))?;
        }
        self.run_files(context, permit, "shared", steps)
    }

    fn run_client(
        &self,
        context: &mut Context<'_, '_>,
        permit: &Permit,
        activation: Value,
        steps: &mut Steps,
    ) -> Result<()> {
        let owner = permit.owner();
        let id = &owner.id;
        let candidate = permit.candidate();
        let profile = &candidate.profile.profile;
        let options = option_groups(context)?;
        self.run_files(context, permit, "client", steps)?;
        if profile.replay.contains(&ReplayEvent::OnGameBoot) {
            stage(self.session_id, id, "replay:OnGameBoot", None)?;
            let ran = steps.time("replay", || {
                replay(context, "OnGameBoot", &candidate.sources)
            })?;
            steps.note("replayedOnGameBoot", json!(ran));
        }
        let added = option_groups(context)?.saturating_sub(options);
        if added > 0 {
            // New settings start at their defaults until the saved values are read.
            stage(self.session_id, id, "options", None)?;
            steps.time("options", || load_saved_options(context))?;
            steps.note("optionGroups", json!(added));
        }
        if let Some(hook) = &profile.hook {
            stage(self.session_id, id, "profile-after", None)?;
            steps.time("hook", || {
                hooks::after(context, hook, !self.startup(), owner)
            })?;
            steps.note("hook", json!(hook));
        }
        if let Some(path) = &profile.cleanup {
            stage(self.session_id, id, "cleanup-lookup", None)?;
            let function = resolve_function(context, path)?;
            with(self.session_id, |s| {
                s.cleanup.insert(
                    owner.clone(),
                    CooperativeCleanup {
                        function,
                        context: activation,
                    },
                );
                Ok(())
            })?;
        }
        Ok(())
    }

    fn run_files(
        &self,
        context: &mut Context<'_, '_>,
        permit: &Permit,
        phase: &str,
        steps: &mut Steps,
    ) -> Result<()> {
        let id = &permit.owner().id;
        let prefix = format!("{phase}/");
        let started = Instant::now();
        for source in &permit.candidate().sources {
            if !source.name.to_lowercase().starts_with(&prefix) {
                continue;
            }
            stage(self.session_id, id, phase, Some(&source.filename))?;
            let file = Instant::now();
            let result = execute_owned(context, self.session_id, permit.owner(), &source.name);
            if file.elapsed() >= SLOW_FILE {
                steps.slow.insert(source.name.clone(), ms(file.elapsed()));
            }
            result?;
        }
        steps
            .timings
            .insert(phase.to_owned(), ms(started.elapsed()));
        Ok(())
    }

    /// Delivers start events that the mod missed, to its own callbacks only.
    fn replay_start(
        &mut self,
        context: &mut Context<'_, '_>,
        permits: &[Permit],
        outcomes: &mut Outcomes,
    ) -> Result<()> {
        let requirements: BTreeMap<_, _> = permits
            .iter()
            .map(|permit| {
                (
                    permit.owner().id.clone(),
                    permit.candidate().profile.requires.clone(),
                )
            })
            .collect();
        for (event, replay_event) in [
            ("OnGameStart", ReplayEvent::OnGameStart),
            ("OnLoad", ReplayEvent::OnLoad),
        ] {
            let mut owners = BTreeMap::new();
            for permit in permits {
                let wanted = permit
                    .candidate()
                    .profile
                    .profile
                    .replay
                    .contains(&replay_event);
                if wanted && outcomes.get(&permit.owner().id).is_some_and(|r| r.is_ok()) {
                    for source in &permit.candidate().sources {
                        owners.insert(script_name(&source.filename), permit.owner().clone());
                    }
                }
            }
            let files = owners.keys().cloned().collect();
            for callback in game::callbacks(context, event, &files)? {
                let filename = game::callback_filename(context, &callback)?;
                let Some(owner) = owners.get(&filename) else {
                    continue;
                };
                stage(
                    self.session_id,
                    &owner.id,
                    &format!("replay:{event}"),
                    Some(Path::new(&filename)),
                )?;
                let failed = failed_requirement(&owner.id, &requirements, outcomes)
                    .map(|required| format!("It needs {required}, which failed"));
                let result = match failed {
                    Some(error) => Err(error),
                    None if outcomes.get(&owner.id).is_some_and(|r| r.is_err())
                        || !game::callback_registered(context, event, &callback)? =>
                    {
                        continue;
                    }
                    None => {
                        let started = Instant::now();
                        let result = context.call(&callback, &[]).map(|_| ());
                        log::debug("mod.replay", format!("Delivered {event}"))
                            .about(&owner.id)
                            .took(started.elapsed())
                            .with(json!({ "file": filename, "ok": result.is_ok() }))
                            .write();
                        result
                    }
                };
                if let Err(error) = result {
                    self.abandon(owner, &error)?;
                    outcomes.insert(owner.id.clone(), Err(error));
                }
            }
        }
        Ok(())
    }

    fn conclude(
        &mut self,
        context: &mut Context<'_, '_>,
        permits: Vec<Permit>,
        mut outcomes: Outcomes,
    ) -> Result<String> {
        let activated = !permits.is_empty();
        for permit in permits {
            let owner = permit.owner().clone();
            if let Some(required) = permit
                .candidate()
                .profile
                .requires
                .iter()
                .find(|required| self.failures.contains_key(*required))
            {
                outcomes.insert(owner.id.clone(), Err(unmet_requirement(required)));
            }
            let result = outcomes
                .remove(&owner.id)
                .unwrap_or_else(|| Err("No activation outcome".into()));
            match &result {
                Ok(()) => {
                    status(context, self.session_id, &owner.id, RunState::Loaded)?;
                    log::info("mod.loaded", "Running").about(&owner.id).write();
                    self.loaded.push(owner.id.clone());
                }
                Err(error) => {
                    self.abandon(&owner, error)?;
                    self.fail(context, &owner.id, error.clone())?;
                }
            }
            with(self.session_id, |s| {
                s.policy
                    .finish_activation(permit, result)
                    .map(|_| ())
                    .map_err(|e| e.to_string())
            })?;
        }
        if activated {
            crate::ui::refresh_options(context)?;
        }
        self.summarize()
    }

    /// Succeeds only when every selection now runs. A reload is judged by
    /// its target alone, since nothing else was asked of it.
    fn summarize(&self) -> Result<String> {
        let mut parts = vec![match self.loaded.len() {
            0 => "Nothing new was loaded.".to_owned(),
            1 => format!("Loaded {}.", self.loaded[0]),
            count if count <= 4 => format!("Loaded {}.", join(&self.loaded)),
            count => format!("Loaded {count} mods."),
        }];
        if let Some(target) = self.target() {
            return match self.failures.get(target) {
                Some(error) => Err(format!(
                    "{target} failed: {}",
                    error.lines().next().unwrap_or(error)
                )),
                None if self.loaded.contains(target) => Ok(parts.join(" ")),
                None => Err(format!("{target} was not reloaded.")),
            };
        }
        let mut complete = true;
        for (file, error) in &self.unreadable {
            complete = false;
            let name = file.file_name().unwrap_or_default().to_string_lossy();
            parts.push(format!("{name} can't be read: {error}."));
        }
        if !self.restart.is_empty() {
            complete = false;
            parts.push(format!(
                "Has to start with the game: {}.",
                join(&self.restart)
            ));
        }
        for (id, error) in &self.failures {
            complete = false;
            parts.push(format!(
                "{id} failed: {}",
                error.lines().next().unwrap_or(error)
            ));
        }
        if !self.spent.is_empty() {
            complete = false;
            parts.push(format!(
                "Needs a reconnect to try again: {}.",
                join(&self.spent)
            ));
        }
        if complete {
            Ok(parts.join(" "))
        } else {
            Err(parts.join(" "))
        }
    }
}

/// What one mod's folders hold, read without touching the game.
struct Files {
    eligibility: Result<Eligibility>,
    assessed: Duration,
    sources: Result<Vec<Source>>,
    scanned: Duration,
}

/// Reads every mod's folders at once. The reads mostly wait for the disk, so
/// waiting for many together takes little longer than waiting for one.
fn read_files(profiles: &[&ResolvedProfile], inputs: &Inputs) -> Vec<Files> {
    let next = AtomicUsize::new(0);
    let work = || {
        let mut done = Vec::new();
        while let Some(profile) = profiles.get(next.fetch_add(1, Ordering::Relaxed)) {
            let started = Instant::now();
            let eligibility =
                activation::assess(&profile.profile, &profile.roots, &inputs.templates);
            let assessed = started.elapsed();
            let sources = scan_sources(&profile.profile, &profile.roots, &inputs.active)
                .map_err(|error| error.to_string());
            done.push((
                profile.profile.id.clone(),
                Files {
                    eligibility,
                    assessed,
                    sources,
                    scanned: started.elapsed() - assessed,
                },
            ));
        }
        done
    };
    let mut read: BTreeMap<ModId, Files> = std::thread::scope(|scope| {
        let workers = (0..profiles.len().min(READERS))
            .map(|_| scope.spawn(work))
            .collect::<Vec<_>>();
        workers
            .into_iter()
            .filter_map(|worker| worker.join().ok())
            .flatten()
            .collect()
    });
    profiles
        .iter()
        .map(|profile| {
            read.remove(&profile.profile.id).unwrap_or_else(|| Files {
                eligibility: Err("Its files could not be read".into()),
                assessed: Duration::ZERO,
                sources: Ok(Vec::new()),
                scanned: Duration::ZERO,
            })
        })
        .collect()
}

#[derive(Default)]
struct Steps {
    timings: BTreeMap<String, f64>,
    slow: BTreeMap<String, f64>,
    notes: serde_json::Map<String, serde_json::Value>,
}

impl Steps {
    fn time<T>(&mut self, step: &str, action: impl FnOnce() -> Result<T>) -> Result<T> {
        let started = Instant::now();
        let result = action();
        self.timings.insert(step.to_owned(), ms(started.elapsed()));
        result
    }
    fn note(&mut self, name: &str, value: serde_json::Value) {
        self.notes.insert(name.to_owned(), value);
    }
}

/// Why a mod can't run when something it requires didn't load.
fn unmet_requirement(required: &ModId) -> String {
    format!("It needs {required}, which did not load")
}

fn ms(elapsed: Duration) -> f64 {
    (elapsed.as_secs_f64() * 1_000_000.0).round() / 1000.0
}

fn join(ids: &[ModId]) -> String {
    ids.iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

/// The name Kahlua knows a script by.
fn script_name(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn discovered(profile: &Profile) -> ModDiagnostic {
    ModDiagnostic {
        status: ModStatus::Prepared,
        stage: "discovery".into(),
        profile: profile.file.clone(),
        source: None,
        error: None,
        traceback: None,
        java_traceback: None,
        source_count: 0,
        generation: 0,
        timings_ms: BTreeMap::new(),
        configuration: ProfileDiagnostic {
            roots: Vec::new(),
            requires: profile.requires.clone(),
            before: profile.before.clone(),
            after: profile.after.clone(),
            preload: profile.preload.clone(),
            replay: profile.replay.iter().copied().collect(),
            hook: profile.hook.clone(),
            cleanup: profile.cleanup.clone(),
            live_policy: profile.live,
            activation_limit: None,
        },
    }
}

fn configuration(profile: &ResolvedProfile) -> ProfileDiagnostic {
    ProfileDiagnostic {
        roots: profile.roots.clone(),
        requires: profile.requires.iter().cloned().collect(),
        before: profile.before.iter().cloned().collect(),
        after: profile.after.iter().cloned().collect(),
        preload: profile.profile.preload.clone(),
        replay: profile.profile.replay.iter().copied().collect(),
        hook: profile.profile.hook.clone(),
        cleanup: profile.profile.cleanup.clone(),
        live_policy: profile.profile.live,
        activation_limit: None,
    }
}

/// How many settings groups mods have registered with the game.
fn option_groups(context: &mut Context<'_, '_>) -> Result<usize> {
    let api = context.global("PZAPI")?;
    if api.is_nil() {
        return Ok(0);
    }
    let options = context.get(&api, "ModOptions")?;
    let data = context.get(&options, "Data")?;
    if data.is_nil() {
        return Ok(0);
    }
    context
        .jni(|env| env.call_method(data.object(), "len", "()I", &[])?.i())
        .map(|count| count.max(0) as usize)
}

fn load_saved_options(context: &mut Context<'_, '_>) -> Result<()> {
    let api = context.global("PZAPI")?;
    let options = context.get(&api, "ModOptions")?;
    context.invoke(&options, "load", &[])?;
    Ok(())
}

fn replay(context: &mut Context<'_, '_>, event: &str, sources: &[Source]) -> Result<usize> {
    let files = sources
        .iter()
        .map(|source| script_name(&source.filename))
        .collect();
    let mut ran = 0;
    for callback in game::callbacks(context, event, &files)? {
        if game::callback_registered(context, event, &callback)? {
            context.call(&callback, &[])?;
            ran += 1;
        }
    }
    Ok(ran)
}

fn failed_requirement(
    id: &ModId,
    requirements: &BTreeMap<ModId, BTreeSet<ModId>>,
    outcomes: &BTreeMap<ModId, Result<()>>,
) -> Option<ModId> {
    let mut pending = requirements
        .get(id)
        .into_iter()
        .flatten()
        .cloned()
        .collect::<Vec<_>>();
    let mut visited = BTreeSet::new();
    while let Some(required) = pending.pop() {
        if !visited.insert(required.clone()) {
            continue;
        }
        if outcomes
            .get(&required)
            .is_some_and(|outcome| outcome.is_err())
        {
            return Some(required);
        }
        pending.extend(requirements.get(&required).into_iter().flatten().cloned());
    }
    None
}

fn exact_boolean(context: &mut Context<'_, '_>, value: &Value) -> Result<Option<bool>> {
    if value.is_nil() {
        return Ok(None);
    }
    context.jni(|env| {
        if env.is_instance_of(value.object(), "java/lang/Boolean")? {
            env.call_method(value.object(), "booleanValue", "()Z", &[])?
                .z()
                .map(Some)
        } else {
            Ok(None)
        }
    })
}

fn resolve_function(context: &mut Context<'_, '_>, path: &str) -> Result<Value> {
    let mut parts = path.split('.');
    let mut value = context.global(parts.next().ok_or("Empty cleanup path")?)?;
    for part in parts {
        if value.is_nil() {
            return Err(format!("Upstream cleanup function is unavailable: {path}"));
        }
        value = context.get(&value, part)?;
    }
    let callable = context.jni(|env| {
        Ok(!value.is_nil()
            && (env.is_instance_of(value.object(), "se/krka/kahlua/vm/JavaFunction")?
                || env.is_instance_of(value.object(), "se/krka/kahlua/vm/LuaClosure")?))
    })?;
    if !callable {
        return Err(format!("Upstream cleanup path is not callable: {path}"));
    }
    Ok(value)
}
