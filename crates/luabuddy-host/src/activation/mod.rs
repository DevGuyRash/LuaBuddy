//! Whether a mod can load late, judged from its files alone.

use crate::lua::Result;
use luabuddy_core::{Profile, profile::LivePolicy};
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

mod startup;
pub use startup::{StartupSelection, select_next_startup, selected_next_startup};

#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd)]
pub enum StartupComponent {
    Definitions,
    Maps,
    Animations,
    TexturePacks,
    TileDefinitions,
    ServerLua,
    Clothing,
    SandboxOptions,
}

impl StartupComponent {
    fn description(self) -> &'static str {
        match self {
            Self::Definitions => "native definitions",
            Self::Maps => "maps",
            Self::Animations => "animation assets",
            Self::TexturePacks => "texture packs",
            Self::TileDefinitions => "tile definitions",
            Self::ServerLua => "server Lua",
            Self::Clothing => "clothing",
            Self::SandboxOptions => "sandbox settings",
        }
    }

    /// Whether loading only its Lua would leave the mod without its substance.
    /// Sandbox settings have script defaults, so their absence is only a limit.
    fn essential(self) -> bool {
        self != Self::SandboxOptions
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum Eligibility {
    Live { limitations: Vec<StartupComponent> },
    RestartRequired { reason: String },
}

impl Eligibility {
    pub fn explanation(&self) -> String {
        match self {
            Self::Live { limitations } if limitations.is_empty() => {
                "Its client and shared Lua can be loaded late.".into()
            }
            Self::Live { limitations } => format!(
                "Only its client and shared Lua is loaded. Its {} need the game's normal startup or the server.",
                descriptions(limitations)
            ),
            Self::RestartRequired { reason } => reason.clone(),
        }
    }
}

pub fn descriptions(components: &[StartupComponent]) -> String {
    let names = components
        .iter()
        .map(|component| component.description())
        .collect::<Vec<_>>();
    match names.split_last() {
        Some((last, [])) => (*last).to_owned(),
        Some((last, rest)) => format!("{} and {last}", rest.join(", ")),
        None => String::new(),
    }
}

pub fn assess(profile: &Profile, roots: &[PathBuf], templates: &[Profile]) -> Result<Eligibility> {
    let policy = profile
        .live
        .or_else(|| {
            templates
                .iter()
                .find(|template| template.id == profile.id)
                .and_then(|template| template.live)
        })
        .unwrap_or(LivePolicy::Auto);
    let mut components = BTreeSet::new();
    for root in roots {
        for media in resource_paths(root, "media")? {
            if contains_tile_definitions(&media)? {
                components.insert(StartupComponent::TileDefinitions);
                break;
            }
        }
        // Mods ship placeholder text files in some of these only to silence the game.
        let any: &[&str] = &[];
        let animation: &[&str] = &["xml", "x", "fbx", "glb", "gltf"];
        for (relative, kinds, component) in [
            ("media/scripts", any, StartupComponent::Definitions),
            ("media/maps", any, StartupComponent::Maps),
            ("media/animations", animation, StartupComponent::Animations),
            ("media/animsets", animation, StartupComponent::Animations),
            ("media/anims_X", animation, StartupComponent::Animations),
            ("media/texturepacks", any, StartupComponent::TexturePacks),
            ("media/lua/server", any, StartupComponent::ServerLua),
            ("media/clothing", any, StartupComponent::Clothing),
            (
                "media/sandbox-options.txt",
                any,
                StartupComponent::SandboxOptions,
            ),
        ] {
            for candidate in resource_paths(root, relative)? {
                if contains_file(&candidate, kinds, &mut BTreeSet::new())? {
                    components.insert(component);
                    break;
                }
            }
        }
    }
    let components: Vec<_> = components.into_iter().collect();
    if policy == LivePolicy::Startup {
        return Ok(Eligibility::RestartRequired {
            reason: "Its profile says it has to start with the game.".into(),
        });
    }
    if policy == LivePolicy::Auto && components.iter().any(|component| component.essential()) {
        return Ok(Eligibility::RestartRequired {
            reason: format!(
                "It has {}, which only load when it starts with the game. Server code also needs the server to run it.",
                descriptions(&components)
            ),
        });
    }
    Ok(Eligibility::Live {
        limitations: components,
    })
}

fn contains_tile_definitions(root: &Path) -> Result<bool> {
    let mut pending = vec![root.to_owned()];
    let mut visited = BTreeSet::new();
    while let Some(path) = pending.pop() {
        if !path.exists() {
            continue;
        }
        if path.is_file() {
            if path
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("tiles"))
            {
                return Ok(true);
            }
            continue;
        }
        let canonical = path
            .canonicalize()
            .map_err(|error| format!("Inspect tile definitions {}: {error}", path.display()))?;
        if !visited.insert(canonical) {
            continue;
        }
        for entry in fs::read_dir(&path)
            .map_err(|error| format!("Inspect tile definitions {}: {error}", path.display()))?
        {
            pending.push(
                entry
                    .map_err(|error| {
                        format!("Inspect tile definitions {}: {error}", path.display())
                    })?
                    .path(),
            );
        }
    }
    Ok(false)
}

fn resource_paths(root: &Path, relative: &str) -> Result<Vec<PathBuf>> {
    let mut candidates = vec![root.to_owned()];
    for segment in relative.split('/') {
        let mut matching = Vec::new();
        for directory in candidates {
            match fs::metadata(&directory) {
                Ok(_) => (),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => {
                    return Err(format!(
                        "Inspect startup resource directory {}: {error}",
                        directory.display()
                    ));
                }
            }
            for entry in fs::read_dir(&directory).map_err(|error| {
                format!(
                    "Inspect startup resource directory {}: {error}",
                    directory.display()
                )
            })? {
                let entry = entry.map_err(|error| {
                    format!(
                        "Inspect startup resource directory {}: {error}",
                        directory.display()
                    )
                })?;
                if entry
                    .file_name()
                    .to_string_lossy()
                    .eq_ignore_ascii_case(segment)
                {
                    matching.push(entry.path());
                }
            }
        }
        candidates = matching;
    }
    Ok(candidates)
}

/// Whether a file is below the path, of one of the kinds when any are named.
fn contains_file(path: &Path, kinds: &[&str], visited: &mut BTreeSet<PathBuf>) -> Result<bool> {
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(format!(
                "Inspect startup assets {}: {error}",
                path.display()
            ));
        }
    };
    if metadata.is_file() {
        return Ok(kinds.is_empty()
            || path
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| {
                    kinds
                        .iter()
                        .any(|kind| extension.eq_ignore_ascii_case(kind))
                }));
    }
    let identity = path
        .canonicalize()
        .map_err(|error| format!("Inspect startup assets {}: {error}", path.display()))?;
    if !visited.insert(identity) {
        return Ok(false);
    }
    for entry in fs::read_dir(path)
        .map_err(|error| format!("Inspect startup assets {}: {error}", path.display()))?
    {
        let entry =
            entry.map_err(|error| format!("Inspect startup assets {}: {error}", path.display()))?;
        if contains_file(&entry.path(), kinds, visited)? {
            return Ok(true);
        }
    }
    Ok(false)
}
