use crate::{CoreError, ModId};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub enum DownloadState {
    Unsubscribed,
    Subscribed,
    Downloading { downloaded: u64, total: Option<u64> },
    Installed,
    Failed { operation: String, reason: String },
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub enum ActivationState {
    Disabled,
    Enabled,
    Loaded { generation: u64 },
    RestartRequired { reason: String },
    ReconnectRequired { reason: String },
    Failed { reason: String },
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct ManagedMod {
    pub id: ModId,
    pub workshop: Option<u64>,
    pub download: DownloadState,
    pub activation: ActivationState,
    pub requires: BTreeSet<ModId>,
    pub pending: Option<PendingAction>,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
pub enum PendingAction {
    Subscribe,
    Download,
    Enable,
    Load,
    Disable,
    Update,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum Observation {
    SubscriptionVerified,
    DownloadProgress { downloaded: u64, total: Option<u64> },
    InstallationVerified,
    EnabledVerified,
    LoadedVerified { generation: u64 },
    DisabledVerified,
    RestartRequired(String),
    ReconnectRequired(String),
    Failure(String),
}

impl ManagedMod {
    pub fn request(
        &mut self,
        action: PendingAction,
        available: &BTreeSet<ModId>,
    ) -> Result<(), CoreError> {
        if self.pending == Some(action) {
            return Ok(());
        }
        if self.pending.is_some() {
            return Err(CoreError::new(
                "management",
                "Wait for the current mod operation to finish",
            ));
        }
        if matches!(action, PendingAction::Enable | PendingAction::Load) {
            if self.download != DownloadState::Installed {
                return Err(CoreError::new(
                    "management",
                    "Wait for a verified installation before enabling or loading",
                ));
            }
            let missing: Vec<_> = self
                .requires
                .difference(available)
                .map(ToString::to_string)
                .collect();
            if !missing.is_empty() {
                return Err(CoreError::new(
                    "management",
                    format!("Missing required mods: {}", missing.join(", ")),
                ));
            }
        }
        if action == PendingAction::Load && self.activation != ActivationState::Enabled {
            return Err(CoreError::new(
                "management",
                "Enable the mod before requesting a load",
            ));
        }
        if action == PendingAction::Download
            && !matches!(
                self.download,
                DownloadState::Subscribed | DownloadState::Failed { .. }
            )
        {
            return Err(CoreError::new("management", "Subscribe before downloading"));
        }
        if action == PendingAction::Subscribe && self.workshop.is_none() {
            return Err(CoreError::new(
                "management",
                "This installed mod has no verified Workshop identity",
            ));
        }
        self.pending = Some(action);
        Ok(())
    }

    pub fn observe(&mut self, observation: Observation) -> Result<(), CoreError> {
        let required = match &observation {
            Observation::SubscriptionVerified => Some(PendingAction::Subscribe),
            Observation::DownloadProgress { .. } => self
                .pending
                .filter(|p| matches!(p, PendingAction::Download | PendingAction::Update)),
            Observation::InstallationVerified => self
                .pending
                .filter(|p| matches!(p, PendingAction::Download | PendingAction::Update)),
            Observation::EnabledVerified => Some(PendingAction::Enable),
            Observation::LoadedVerified { .. } => Some(PendingAction::Load),
            Observation::DisabledVerified => Some(PendingAction::Disable),
            _ => self.pending,
        };
        if required.is_none() || required != self.pending {
            return Err(CoreError::new(
                "management",
                "Observation does not match the pending operation",
            ));
        }
        match observation {
            Observation::SubscriptionVerified => self.download = DownloadState::Subscribed,
            Observation::DownloadProgress { downloaded, total } => {
                if total.is_some_and(|total| downloaded > total) {
                    return Err(CoreError::new(
                        "download",
                        "Downloaded byte count exceeds the reported total",
                    ));
                }
                self.download = DownloadState::Downloading { downloaded, total };
                return Ok(());
            }
            Observation::InstallationVerified => self.download = DownloadState::Installed,
            Observation::EnabledVerified => self.activation = ActivationState::Enabled,
            Observation::LoadedVerified { generation } => {
                self.activation = ActivationState::Loaded { generation }
            }
            Observation::DisabledVerified => {
                self.activation = if matches!(self.activation, ActivationState::Loaded { .. }) {
                    ActivationState::ReconnectRequired {
                        reason: "The running mod remains active until reconnect or restart"
                            .to_owned(),
                    }
                } else {
                    ActivationState::Disabled
                };
            }
            Observation::RestartRequired(reason) => {
                self.activation = ActivationState::RestartRequired { reason }
            }
            Observation::ReconnectRequired(reason) => {
                self.activation = ActivationState::ReconnectRequired { reason }
            }
            Observation::Failure(reason) => {
                if matches!(
                    self.pending,
                    Some(
                        PendingAction::Subscribe | PendingAction::Download | PendingAction::Update
                    )
                ) {
                    self.download = DownloadState::Failed {
                        operation: format!("{:?}", self.pending),
                        reason,
                    };
                } else {
                    self.activation = ActivationState::Failed { reason };
                }
            }
        }
        self.pending = None;
        Ok(())
    }

    pub fn cancellation_unsupported(&self) -> CoreError {
        CoreError::new(
            "management",
            "The selected host API cannot cancel this operation. Dismiss the dialog to leave the download running; its verified state will be checked again on reopening.",
        )
    }
}
