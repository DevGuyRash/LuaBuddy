use crate::{CoreError, ModId, ReplayEvent};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ModStatus {
    Prepared,
    Loaded,
    ServerLoaded,
    Disabled,
    Failed,
    Quarantined,
    RestartRequired,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModDiagnostic {
    pub status: ModStatus,
    pub stage: String,
    pub profile: PathBuf,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub traceback: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub java_traceback: Option<String>,
    pub source_count: usize,
    pub generation: u64,
    #[serde(flatten)]
    pub configuration: ProfileDiagnostic,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ProfileDiagnostic {
    pub roots: Vec<PathBuf>,
    pub requires: Vec<ModId>,
    pub before: Vec<ModId>,
    pub after: Vec<ModId>,
    pub preload: Vec<String>,
    pub replay: Vec<ReplayEvent>,
    pub hook: Option<String>,
    pub cleanup: Option<String>,
    pub live_policy: Option<crate::profile::LivePolicy>,
    pub activation_limit: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticReport {
    pub schema_version: u32,
    pub kind: String,
    pub version: String,
    #[serde(default)]
    pub debug: bool,
    pub session: String,
    pub operation: String,
    pub target: Option<ModId>,
    pub game_version: String,
    pub java_version: String,
    pub config_directory: PathBuf,
    pub started_at: String,
    pub completed_at: Option<String>,
    pub duration_ms: Option<f64>,
    pub mods: BTreeMap<ModId, ModDiagnostic>,
    pub summary: BTreeMap<String, usize>,
    pub error: Option<String>,
}

impl DiagnosticReport {
    pub fn finish(&mut self, completed_at: String, duration_ms: f64) -> Result<(), CoreError> {
        if !duration_ms.is_finite() || duration_ms < 0.0 {
            return Err(CoreError::new(
                "diagnostics",
                "Duration must be finite and nonnegative",
            ));
        }
        self.completed_at = Some(completed_at);
        self.duration_ms = Some(duration_ms);
        self.summary = [
            "loaded",
            "serverLoaded",
            "failed",
            "disabled",
            "pending",
            "restartRequired",
        ]
        .into_iter()
        .map(|key| (key.to_owned(), 0))
        .collect();
        for detail in self.mods.values() {
            let key = match detail.status {
                ModStatus::Loaded => "loaded",
                ModStatus::ServerLoaded => "serverLoaded",
                ModStatus::Failed | ModStatus::Quarantined => "failed",
                ModStatus::Disabled => "disabled",
                ModStatus::Prepared => "pending",
                ModStatus::RestartRequired => "restartRequired",
            };
            if let Some(count) = self.summary.get_mut(key) {
                *count += 1;
            }
        }
        Ok(())
    }

    pub fn write(&self, destination: &Path) -> Result<(), CoreError> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let directory = destination.parent().ok_or_else(|| {
            CoreError::new("diagnostics", "Report destination must have a parent")
        })?;
        fs::create_dir_all(directory)
            .map_err(|e| CoreError::new("diagnostics", e.to_string()).at(directory))?;
        let temporary = directory.join(format!(
            ".diagnostics-{}-{}.tmp",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let result = (|| {
            let mut file = OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&temporary)
                .map_err(|e| CoreError::new("diagnostics", e.to_string()).at(&temporary))?;
            serde_json::to_writer_pretty(&mut file, self)
                .map_err(|e| CoreError::new("diagnostics", e.to_string()).at(&temporary))?;
            file.write_all(b"\n")
                .and_then(|()| file.sync_all())
                .map_err(|e| CoreError::new("diagnostics", e.to_string()).at(&temporary))?;
            fs::rename(&temporary, destination)
                .map_err(|e| CoreError::new("diagnostics", e.to_string()).at(destination))
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }
}
