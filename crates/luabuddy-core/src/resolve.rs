use crate::{CoreError, ModId, Profile};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

#[derive(Debug, Clone, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct NativeMetadata {
    pub id: Option<ModId>,
    pub available: bool,
    pub roots: Vec<PathBuf>,
    pub requires: Vec<ModId>,
    pub after: Vec<ModId>,
    pub before: Vec<ModId>,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct ResolvedProfile {
    pub profile: Profile,
    pub roots: Vec<PathBuf>,
    pub requires: BTreeSet<ModId>,
    pub after: BTreeSet<ModId>,
    pub before: BTreeSet<ModId>,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
pub enum SkipReason {
    Disabled,
    NativeLoaded,
}

#[derive(Debug, Clone, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct Resolution {
    pub ordered: Vec<ResolvedProfile>,
    pub skipped: BTreeMap<ModId, SkipReason>,
    pub failures: BTreeMap<ModId, CoreError>,
}

pub fn resolve(
    profiles: &[Profile],
    native: &BTreeMap<ModId, NativeMetadata>,
    active: &BTreeSet<ModId>,
) -> Resolution {
    let mut result = Resolution::default();
    let mut remaining = BTreeMap::new();
    for profile in profiles {
        if !profile.enabled {
            result
                .skipped
                .insert(profile.id.clone(), SkipReason::Disabled);
            continue;
        }
        if active.contains(&profile.id) {
            result
                .skipped
                .insert(profile.id.clone(), SkipReason::NativeLoaded);
            continue;
        }
        let Some(metadata) = native.get(&profile.id) else {
            result.failures.insert(
                profile.id.clone(),
                CoreError::new(
                    "resolve",
                    format!(
                        "Mod not installed: {}. Install it or set path to its mod directory.",
                        profile.id
                    ),
                )
                .at(&profile.file),
            );
            continue;
        };
        if metadata.id.as_ref().is_some_and(|id| id != &profile.id) || !metadata.available {
            result.failures.insert(
                profile.id.clone(),
                CoreError::new(
                    "resolve",
                    format!(
                        "Mod identity or game-version compatibility rejected for {}",
                        profile.id
                    ),
                )
                .at(&profile.file),
            );
            continue;
        }
        let requires: BTreeSet<_> = metadata
            .requires
            .iter()
            .chain(&profile.requires)
            .cloned()
            .collect();
        let after = metadata
            .after
            .iter()
            .chain(&profile.after)
            .chain(&requires)
            .cloned()
            .collect();
        let before = metadata
            .before
            .iter()
            .chain(&profile.before)
            .cloned()
            .collect();
        remaining.insert(
            profile.id.clone(),
            ResolvedProfile {
                profile: profile.clone(),
                roots: metadata.roots.clone(),
                requires,
                after,
                before,
            },
        );
    }
    for id in active {
        let native_after = native
            .get(id)
            .map(|metadata| metadata.after.as_slice())
            .unwrap_or(&[]);
        let configured_after = profiles
            .iter()
            .find(|p| p.enabled && &p.id == id)
            .map(|p| p.after.as_slice())
            .unwrap_or(&[]);
        for predecessor in native_after.iter().chain(configured_after) {
            if let Some(candidate) = remaining.get_mut(predecessor) {
                candidate.before.insert(id.clone());
            }
        }
    }
    let edges: Vec<_> = remaining
        .iter()
        .flat_map(|(id, p)| {
            p.before
                .iter()
                .map(move |other| (id.clone(), other.clone()))
        })
        .collect();
    for (id, other) in edges {
        if let Some(candidate) = remaining.get_mut(&other) {
            candidate.after.insert(id);
        }
    }
    let mut available = active.clone();
    let scan_order: Vec<_> = profiles.iter().map(|profile| profile.id.clone()).collect();
    while !remaining.is_empty() {
        let mut progress = false;
        for id in &scan_order {
            if remaining.get(id).is_none_or(|candidate| {
                candidate
                    .after
                    .iter()
                    .any(|dependency| remaining.contains_key(dependency))
            }) {
                continue;
            }
            let Some(candidate) = remaining.remove(id) else {
                continue;
            };
            progress = true;
            let id = id.clone();
            let missing: Vec<_> = candidate
                .requires
                .difference(&available)
                .map(ToString::to_string)
                .collect();
            let impossible: Vec<_> = candidate
                .before
                .intersection(active)
                .map(ToString::to_string)
                .collect();
            if !missing.is_empty() {
                result.failures.insert(
                    id,
                    CoreError::new(
                        "dependencies",
                        format!(
                            "Enable required mod profiles or server mods: {}",
                            missing.join(", ")
                        ),
                    ),
                );
            } else if !impossible.is_empty() {
                result.failures.insert(id.clone(), CoreError::new("dependencies", format!("Startup order requires {id} before already loaded native mods {}; use a compatible selection", impossible.join(", "))));
            } else {
                available.insert(id);
                result.ordered.push(candidate);
            }
        }
        if !progress {
            let cycle = remaining
                .keys()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ");
            for id in remaining.keys() {
                result.failures.insert(
                    id.clone(),
                    CoreError::new("dependencies", format!("Cyclic mod order: {cycle}")),
                );
            }
            break;
        }
    }
    result
}
