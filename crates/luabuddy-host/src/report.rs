use crate::game;
use crate::lua::{Context, Result, Value};
use luabuddy_core::ModId;
use luabuddy_core::diagnostics::{DiagnosticReport, ModDiagnostic};
use luabuddy_core::lifecycle::Operation;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Instant;

pub struct OperationReport {
    pub report: DiagnosticReport,
    started: Instant,
    destination: PathBuf,
}

impl OperationReport {
    pub fn begin(
        context: &mut Context<'_, '_>,
        session: u64,
        operation: &Operation,
        config_directory: PathBuf,
        previous_mods: BTreeMap<ModId, ModDiagnostic>,
    ) -> Result<Self> {
        let filesystem = game::instance(context)?;
        let cache = context.jni(|env| {
            let cache = env
                .call_method(
                    filesystem.object(),
                    "getCacheDir",
                    "()Ljava/lang/String;",
                    &[],
                )?
                .l()?;
            env.new_global_ref(cache).map(|value| Value(Some(value)))
        })?;
        let destination = PathBuf::from(context.text(&cache)?).join("LuaBuddy/diagnostics.json");
        let core = context.jni(|env| {
            let core = env
                .call_static_method(
                    "zombie/core/Core",
                    "getInstance",
                    "()Lzombie/core/Core;",
                    &[],
                )?
                .l()?;
            let version = env
                .call_method(core, "getVersion", "()Ljava/lang/String;", &[])?
                .l()?;
            env.new_global_ref(version).map(|value| Value(Some(value)))
        })?;
        let game_version = context.text(&core)?;
        let java_version = game::property(context, "java.version")?
            .ok_or("The host JVM has no java.version property")?;
        let (operation, target) = match operation {
            Operation::Startup => ("startup", None),
            Operation::Refresh => ("refresh", None),
            Operation::Reload(id) => ("reload", Some(id.clone())),
        };
        let report = DiagnosticReport {
            schema_version: 1,
            kind: "loader-initialization".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            debug: game::property(context, "luabuddy.debug")?.as_deref() == Some("true"),
            session: session_identity(context, session)?,
            operation: operation.into(),
            target,
            game_version,
            java_version,
            config_directory,
            started_at: timestamp(context)?,
            completed_at: None,
            duration_ms: None,
            mods: previous_mods,
            summary: BTreeMap::new(),
            error: None,
        };
        game::log(&format!(
            "Starting session={} version={} game={} java={} operation={}",
            report.session,
            report.version,
            report.game_version,
            report.java_version,
            report.operation
        ));
        Ok(Self {
            report,
            started: Instant::now(),
            destination,
        })
    }

    pub fn record(&mut self, id: ModId, detail: ModDiagnostic) {
        self.report.mods.insert(id, detail);
    }

    pub fn finish(
        mut self,
        context: &mut Context<'_, '_>,
        error: Option<&str>,
    ) -> Result<DiagnosticReport> {
        self.report.error = error.map(str::to_owned);
        self.report
            .finish(
                timestamp(context)?,
                self.started.elapsed().as_secs_f64() * 1000.0,
            )
            .map_err(|error| error.to_string())?;
        self.report
            .write(&self.destination)
            .map_err(|error| error.to_string())?;
        Ok(self.report)
    }
}

fn session_identity(context: &mut Context<'_, '_>, session: u64) -> Result<String> {
    static PROCESS: OnceLock<String> = OnceLock::new();
    if PROCESS.get().is_none() {
        let uuid = context.jni(|env| {
            let uuid = env
                .call_static_method("java/util/UUID", "randomUUID", "()Ljava/util/UUID;", &[])?
                .l()?;
            env.new_global_ref(uuid).map(|value| Value(Some(value)))
        })?;
        let _ = PROCESS.set(context.text(&uuid)?);
    }
    let process = PROCESS
        .get()
        .ok_or("Diagnostic process identity was not initialized")?;
    Ok(format!("{process}-{session}"))
}

pub fn timestamp(context: &mut Context<'_, '_>) -> Result<String> {
    let instant = context.jni(|env| {
        let instant = env
            .call_static_method("java/time/Instant", "now", "()Ljava/time/Instant;", &[])?
            .l()?;
        env.new_global_ref(instant).map(|value| Value(Some(value)))
    })?;
    context.text(&instant)
}
