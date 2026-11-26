//! What the window reads about the player's selections and the running state.

use super::*;

/// The player's selections and the running state, read together.
pub(super) struct Live {
    pub(super) world: World,
    pub(super) profiles: Vec<Profile>,
    pub(super) unreadable: Vec<(std::path::PathBuf, String)>,
    /// Selections, plus bundled templates for mods not yet configured, which
    /// is what turning a mod on would use.
    pub(super) effective: Vec<Profile>,
    pub(super) templates: Vec<Profile>,
    pub(super) statuses: BTreeMap<ModId, RunState>,
    pub(super) reloadable: BTreeSet<ModId>,
    pub(super) spent: BTreeSet<ModId>,
    pub(super) active: BTreeSet<ModId>,
    pub(super) startup: BTreeSet<ModId>,
    pub(super) queued: BTreeSet<ModId>,
    pub(super) temporary: BTreeSet<ModId>,
}

/// The game's own mod list as it stood at the first menu, before the player
/// could edit it there, to tell whether a restart is still needed.
pub(super) static STARTUP_AT_LAUNCH: std::sync::OnceLock<BTreeSet<ModId>> =
    std::sync::OnceLock::new();

/// Records the game's own mod list once, at the first menu, so later edits
/// through the game's mod selector count as needing a restart.
pub fn remember_launch_startup(context: &mut Context<'_, '_>) {
    if STARTUP_AT_LAUNCH.get().is_none()
        && let Ok(startup) = activation::selected_next_startup(context)
    {
        let _ = STARTUP_AT_LAUNCH.set(startup);
    }
}

impl Live {
    pub(super) fn read(context: &mut Context<'_, '_>) -> Result<Self> {
        let state = runtime::snapshot();
        let (profiles, unreadable) = crate::selections::readable(context)?;
        let temporary = crate::selections::temporary_ids(context)?;
        let templates = profile_store::templates(context)?;
        let queued = crate::selections::queued(context)
            .unwrap_or_default()
            .into_iter()
            .map(|profile| profile.id)
            .collect();
        Ok(Self {
            world: World {
                place: observe::place(context)?,
                ready: state.ready,
                options_pending: crate::ui::pending_options(context)?,
                steam: true,
            },
            effective: effective(&profiles, &templates),
            profiles,
            unreadable,
            templates,
            statuses: state.statuses,
            reloadable: state.reloadable,
            spent: state.spent,
            active: game::active(context)?,
            startup: activation::selected_next_startup(context)?,
            queued,
            temporary,
        })
    }
}

pub(super) fn effective(profiles: &[Profile], templates: &[Profile]) -> Vec<Profile> {
    let mut all = profiles.to_vec();
    for template in templates {
        if !profiles.iter().any(|profile| profile.id == template.id) {
            let mut template = template.clone();
            template.enabled = false;
            all.push(template);
        }
    }
    all
}

pub(super) fn extension_totals() -> Totals {
    let (extensions, extensions_pending) = crate::extensions::counts();
    Totals {
        extensions,
        extensions_pending,
        ..Totals::default()
    }
}
