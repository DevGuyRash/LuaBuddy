//! Writes LuaBuddy's journal and echoes it into the game's own log.
//!
//! The process's standard error is not part of the game's logs, so a message
//! sent only there is lost for anyone reading `console.txt`.

use crate::{
    game,
    lua::{Context, Result},
};
use jni::{JNIEnv, JavaVM, objects::JValue};
use luabuddy_core::{
    ModId,
    journal::{Entry, Journal, Level},
};
use serde_json::Value as Json;
use std::{
    cell::Cell,
    path::PathBuf,
    sync::{
        LazyLock, Mutex, OnceLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

static JOURNAL: LazyLock<Mutex<Journal>> = LazyLock::new(Mutex::default);
static VM: OnceLock<JavaVM> = OnceLock::new();
static GAME_LOG: AtomicBool = AtomicBool::new(false);
static VERBOSE: AtomicBool = AtomicBool::new(false);
static WRITE_FAILED: AtomicBool = AtomicBool::new(false);
static NEXT_OPERATION: AtomicU64 = AtomicU64::new(1);

thread_local! { static OPERATION: Cell<Option<u64>> = const { Cell::new(None) }; }

/// Remembers the VM so a record can reach the game's log from any of its threads.
pub fn start(env: &JNIEnv<'_>) {
    if let Ok(vm) = env.get_java_vm() {
        let _ = VM.set(vm);
    }
}

/// Directs the journal to the active cache directory for one Lua environment.
pub fn attach(context: &mut Context<'_, '_>, session: u64, verbose: bool) -> Result<()> {
    VERBOSE.store(verbose, Ordering::Relaxed);
    GAME_LOG.store(true, Ordering::Relaxed);
    let directory = directory(context)?;
    let identity = crate::report::session_identity(context, session)?;
    let mut journal = JOURNAL.lock().map_err(|_| "Journal lock is poisoned")?;
    journal.set_session(identity);
    journal
        .attach(&directory)
        .map_err(|error| format!("Open journal in {}: {error}", directory.display()))
}

pub fn directory(context: &mut Context<'_, '_>) -> Result<PathBuf> {
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
        env.new_global_ref(cache)
            .map(|value| crate::lua::Value(Some(value)))
    })?;
    Ok(PathBuf::from(context.text(&cache)?).join("LuaBuddy"))
}

pub fn recent(limit: usize) -> Vec<Entry> {
    JOURNAL
        .lock()
        .map(|journal| journal.recent().rev().take(limit).cloned().collect())
        .unwrap_or_default()
}

pub fn revision() -> u64 {
    JOURNAL
        .lock()
        .map(|journal| journal.revision())
        .unwrap_or_default()
}

/// One record under construction. Nothing is written until [`Record::write`].
pub struct Record(Entry);

pub fn record(level: Level, event: &str, message: impl Into<String>) -> Record {
    Record(Entry {
        time: String::new(),
        seq: 0,
        session: String::new(),
        operation: OPERATION.get(),
        level,
        event: event.to_owned(),
        module: None,
        message: message.into(),
        duration_ms: None,
        data: Json::Null,
    })
}

pub fn debug(event: &str, message: impl Into<String>) -> Record {
    record(Level::Debug, event, message)
}
pub fn info(event: &str, message: impl Into<String>) -> Record {
    record(Level::Info, event, message)
}
pub fn warn(event: &str, message: impl Into<String>) -> Record {
    record(Level::Warn, event, message)
}
pub fn error(event: &str, message: impl Into<String>) -> Record {
    record(Level::Error, event, message)
}

impl Record {
    pub fn about(mut self, id: &ModId) -> Self {
        self.0.module = Some(id.clone());
        self
    }
    pub fn with(mut self, data: Json) -> Self {
        self.0.data = data;
        self
    }
    pub fn took(mut self, elapsed: Duration) -> Self {
        self.0.duration_ms = Some((elapsed.as_secs_f64() * 1_000_000.0).round() / 1000.0);
        self
    }
    pub fn write(self) {
        let (entry, result) = match JOURNAL.lock() {
            Ok(mut journal) => journal.append(self.0),
            Err(_) => return eprintln!("[LuaBuddy] {}", self.0.line()),
        };
        if let Err(error) = result
            && !WRITE_FAILED.swap(true, Ordering::Relaxed)
        {
            eprintln!("[LuaBuddy] The journal could not be written: {error}");
        }
        if entry.level > Level::Debug || VERBOSE.load(Ordering::Relaxed) {
            echo(entry.level, &entry.line());
        }
    }
}

/// Groups every record written until it is finished or dropped under one intent.
pub struct Operation {
    event: String,
    module: Option<ModId>,
    started: Instant,
    previous: Option<u64>,
    finished: bool,
}

impl Operation {
    pub fn begin(
        event: &str,
        intent: impl Into<String>,
        about: Option<&ModId>,
        data: Json,
    ) -> Self {
        let previous = OPERATION.replace(Some(NEXT_OPERATION.fetch_add(1, Ordering::Relaxed)));
        let mut start = info(&format!("{event}.intent"), intent).with(data);
        start.0.module = about.cloned();
        start.write();
        Self {
            event: event.to_owned(),
            module: about.cloned(),
            started: Instant::now(),
            previous,
            finished: false,
        }
    }

    /// Ends the operation. A failure is named `<event>.failed`, so its line
    /// says so wherever it is read; anything else is `<event>.outcome`.
    pub fn finish(mut self, level: Level, outcome: impl Into<String>, data: Json) {
        self.finished = true;
        let ending = if level == Level::Error {
            "failed"
        } else {
            "outcome"
        };
        let mut end = record(level, &format!("{}.{ending}", self.event), outcome)
            .with(data)
            .took(self.started.elapsed());
        end.0.module = self.module.clone();
        end.write();
    }
}

impl Drop for Operation {
    fn drop(&mut self) {
        if !self.finished {
            let mut end = warn(
                &format!("{}.outcome", self.event),
                "Ended without a recorded outcome",
            )
            .took(self.started.elapsed());
            end.0.module = self.module.clone();
            end.write();
        }
        OPERATION.set(self.previous);
    }
}

fn echo(level: Level, line: &str) {
    let line = format!("[LuaBuddy] {line}");
    if !GAME_LOG.load(Ordering::Relaxed) || !game_log(level, &line) {
        eprintln!("{line}");
    }
}

fn game_log(level: Level, line: &str) -> bool {
    let Some(mut env) = VM.get().and_then(|vm| vm.get_env().ok()) else {
        return false;
    };
    // A pending exception belongs to the game. Reporting must not disturb it.
    if env.exception_check().unwrap_or(true) {
        return false;
    }
    let result = env.with_local_frame(8, |env| -> jni::errors::Result<()> {
        let kind = env
            .get_static_field("zombie/debug/DebugType", "Mod", "Lzombie/debug/DebugType;")?
            .l()?;
        let text = env.new_string(line)?;
        let (method, signature) = match level {
            Level::Debug | Level::Info => ("println", "(Ljava/lang/String;)V"),
            Level::Warn => ("warn", "(Ljava/lang/Object;)V"),
            Level::Error => ("error", "(Ljava/lang/Object;)V"),
        };
        env.call_method(kind, method, signature, &[JValue::Object(&text)])?;
        Ok(())
    });
    if result.is_err() {
        let _ = env.exception_clear();
    }
    result.is_ok()
}
