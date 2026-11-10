use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct CoreError {
    pub stage: String,
    pub message: String,
    pub path: Option<PathBuf>,
}

impl CoreError {
    pub fn new(stage: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            stage: stage.into(),
            message: message.into(),
            path: None,
        }
    }

    pub fn at(mut self, path: impl Into<PathBuf>) -> Self {
        self.path = Some(path.into());
        self
    }
}

impl fmt::Display for CoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.stage, self.message)?;
        if let Some(path) = &self.path {
            write!(f, " [{}]", path.display())?;
        }
        Ok(())
    }
}

impl std::error::Error for CoreError {}

pub fn profile_filename(id: &str) -> String {
    use sha2::{Digest, Sha256};
    let reserved = id.split('.').next().unwrap_or("").to_ascii_uppercase();
    let reserved = matches!(reserved.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || reserved.len() == 4
            && (reserved.starts_with("COM") || reserved.starts_with("LPT"))
            && matches!(reserved.as_bytes()[3], b'1'..=b'9');
    if !id.is_empty()
        && id.len() <= 180
        && id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_.-".contains(&c))
        && !id.ends_with('.')
        && !reserved
    {
        format!("{id}.properties")
    } else {
        let digest = format!("{:x}", Sha256::digest(id.as_bytes()));
        format!("mod-{}.properties", &digest[..16])
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ModId(String);

impl ModId {
    pub fn new(id: impl Into<String>) -> Result<Self, CoreError> {
        let id = id.into().trim_matches(|ch| ch <= '\u{0020}').to_owned();
        if id.is_empty() || id.contains(['\0', '\n', '\r']) {
            return Err(CoreError::new(
                "profile",
                "A mod ID must be nonempty and contain no NUL or line break",
            ));
        }
        Ok(Self(id))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for ModId {
    type Error = CoreError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<ModId> for String {
    fn from(value: ModId) -> Self {
        value.0
    }
}

impl fmt::Display for ModId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
pub enum ReplayEvent {
    OnGameBoot,
    OnGameStart,
    OnLoad,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LivePolicy {
    Auto,
    ClientShared,
    Startup,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct Profile {
    pub id: ModId,
    pub file: PathBuf,
    pub enabled: bool,
    pub path: Option<PathBuf>,
    pub workshop: Option<u64>,
    pub requires: Vec<ModId>,
    pub after: Vec<ModId>,
    pub before: Vec<ModId>,
    pub preload: Vec<String>,
    pub exclude: Vec<String>,
    pub exclude_when: BTreeMap<ModId, Vec<String>>,
    pub mount: bool,
    pub replay: BTreeSet<ReplayEvent>,
    pub hook: Option<String>,
    pub cleanup: Option<String>,
    #[serde(default)]
    pub live: Option<LivePolicy>,
}

impl Profile {
    pub fn parse(file: impl Into<PathBuf>, text: &str) -> Result<Self, CoreError> {
        let file = file.into();
        let parse = || -> Result<Self, CoreError> {
            let values = parse_properties(text)?;
            const KEYS: &[&str] = &[
                "id", "path", "enabled", "exclude", "preload", "hook", "requires", "after",
                "before", "mount", "replay", "workshop", "cleanup", "live",
            ];
            for key in values.keys() {
                if !KEYS.contains(&key.as_str()) && !key.starts_with("excludeWhen.") {
                    return Err(CoreError::new(
                        "profile",
                        format!("Unknown profile option {key}"),
                    ));
                }
            }
            let value = |key: &str| {
                values
                    .get(key)
                    .map(|s| s.trim_matches(|ch| ch <= '\u{0020}'))
                    .unwrap_or("")
            };
            let boolean = |key: &str, default: bool| -> Result<bool, CoreError> {
                match values
                    .get(key)
                    .map(|s| s.trim_matches(|ch| ch <= '\u{0020}'))
                {
                    None => Ok(default),
                    Some("true") => Ok(true),
                    Some("false") => Ok(false),
                    _ => Err(CoreError::new(
                        "profile",
                        format!("{key} must be true or false"),
                    )),
                }
            };
            let ids = |key: &str| -> Result<Vec<ModId>, CoreError> {
                split_list(value(key)).into_iter().map(ModId::new).collect()
            };
            let relative = |key: &str| {
                if value(key).is_empty() {
                    None
                } else {
                    Some(file.parent().unwrap_or(Path::new(".")).join(value(key)))
                }
            };
            let workshop = if value("workshop").is_empty() {
                None
            } else {
                let raw = value("workshop");
                if raw.len() > 18 || !raw.bytes().all(|b| b.is_ascii_digit()) {
                    return Err(CoreError::new(
                        "profile",
                        "workshop must be a positive numeric item ID of at most 18 digits",
                    ));
                }
                let number = raw
                    .parse::<u64>()
                    .map_err(|_| CoreError::new("profile", "Invalid Workshop item ID"))?;
                if number == 0 {
                    return Err(CoreError::new(
                        "profile",
                        "workshop must be a positive numeric item ID",
                    ));
                }
                Some(number)
            };
            let mut replay = BTreeSet::new();
            for event in split_list(value("replay")) {
                replay.insert(match event.as_str() {
                    "OnGameBoot" => ReplayEvent::OnGameBoot, "OnGameStart" => ReplayEvent::OnGameStart, "OnLoad" => ReplayEvent::OnLoad,
                    _ => return Err(CoreError::new("profile", format!("Unsupported replay event {event}; use OnGameBoot, OnGameStart or OnLoad"))),
                });
            }
            let mut exclude_when = BTreeMap::new();
            for (key, value) in &values {
                if let Some(id) = key.strip_prefix("excludeWhen.") {
                    exclude_when.insert(ModId::new(id)?, split_list(value));
                }
            }
            let hook = if value("hook").is_empty() {
                None
            } else {
                if !value("hook")
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
                {
                    return Err(CoreError::new(
                        "profile",
                        "hook must be a stable Rust hook identifier using letters, numbers, underscores, hyphens or periods",
                    ));
                }
                Some(value("hook").to_owned())
            };
            let cleanup = if value("cleanup").is_empty() {
                None
            } else {
                let path = value("cleanup");
                if path.split('.').any(|part| {
                    part.is_empty()
                        || !part.bytes().enumerate().all(|(index, byte)| {
                            byte.is_ascii_alphabetic()
                                || byte == b'_'
                                || index > 0 && byte.is_ascii_digit()
                        })
                }) {
                    return Err(CoreError::new(
                        "profile",
                        "cleanup must name an existing upstream function with a bounded dotted identifier path",
                    ));
                }
                Some(path.to_owned())
            };
            let live = match value("live") {
                "" => None,
                "auto" => Some(LivePolicy::Auto),
                "client-shared" => Some(LivePolicy::ClientShared),
                "startup" => Some(LivePolicy::Startup),
                _ => {
                    return Err(CoreError::new(
                        "profile",
                        "live must be auto, client-shared or startup",
                    ));
                }
            };
            Ok(Self {
                id: ModId::new(value("id"))?,
                file: file.clone(),
                enabled: boolean("enabled", true)?,
                path: relative("path"),
                workshop,
                requires: ids("requires")?,
                after: ids("after")?,
                before: ids("before")?,
                preload: split_list(value("preload")),
                exclude: split_list(value("exclude")),
                exclude_when,
                mount: boolean("mount", false)?,
                replay,
                hook,
                cleanup,
                live,
            })
        };
        parse().map_err(|e| e.at(file))
    }

    pub fn exclusions(&self, active: &BTreeSet<ModId>) -> Vec<String> {
        let mut values = self.exclude.clone();
        for id in active {
            if let Some(extra) = self.exclude_when.get(id) {
                values.extend(extra.iter().cloned());
            }
        }
        values
    }
}

pub fn read_profiles(directory: &Path) -> Result<Vec<Profile>, CoreError> {
    let mut files = fs::read_dir(directory)
        .map_err(|e| CoreError::new("profile-read", e.to_string()).at(directory))?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| CoreError::new("profile-read", e.to_string()).at(directory))?;
    files.retain(|path| path.extension().is_some_and(|ext| ext == "properties"));
    files.sort();
    let mut ids = BTreeSet::new();
    let mut profiles = Vec::new();
    for file in files {
        let text = fs::read_to_string(&file)
            .map_err(|e| CoreError::new("profile-read", e.to_string()).at(&file))?;
        let profile = Profile::parse(&file, &text)?;
        if !ids.insert(profile.id.clone()) {
            return Err(
                CoreError::new("profile", format!("Duplicate profile for {}", profile.id)).at(file),
            );
        }
        profiles.push(profile);
    }
    Ok(profiles)
}

fn split_list(value: &str) -> Vec<String> {
    value
        .split(';')
        .map(|value| value.trim_matches(|ch| ch <= '\u{0020}'))
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect()
}

pub fn parse_properties(text: &str) -> Result<BTreeMap<String, String>, CoreError> {
    let mut values = BTreeMap::new();
    let mut logical = String::new();
    let mut continued = false;
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    for physical in normalized.split('\n') {
        if !continued
            && physical
                .trim_start_matches([' ', '\t', '\u{000c}'])
                .starts_with(['#', '!'])
        {
            continue;
        }
        let line = if continued {
            physical.trim_start_matches([' ', '\t', '\u{000c}'])
        } else {
            physical
        };
        logical.push_str(line);
        let backslashes = logical.chars().rev().take_while(|c| *c == '\\').count();
        if backslashes % 2 == 1 {
            logical.pop();
            continued = true;
            continue;
        }
        continued = false;
        parse_property_line(&logical, &mut values)?;
        logical.clear();
    }
    if continued || !logical.is_empty() {
        parse_property_line(&logical, &mut values)?;
    }
    Ok(values)
}

fn parse_property_line(line: &str, values: &mut BTreeMap<String, String>) -> Result<(), CoreError> {
    let line = line.trim_start_matches([' ', '\t', '\u{000c}']);
    if line.is_empty() || line.starts_with(['#', '!']) {
        return Ok(());
    }
    let mut escaped = false;
    let mut separator = line.len();
    for (index, ch) in line.char_indices() {
        if !escaped && (ch == '=' || ch == ':' || matches!(ch, ' ' | '\t' | '\u{000c}')) {
            separator = index;
            break;
        }
        escaped = !escaped && ch == '\\';
    }
    let key = unescape_property(&line[..separator])?;
    let mut value = &line[separator..];
    value = value.trim_start_matches([' ', '\t', '\u{000c}']);
    if value.starts_with(['=', ':']) {
        value = &value[1..];
    }
    value = value.trim_start_matches([' ', '\t', '\u{000c}']);
    values.insert(key, unescape_property(value)?);
    Ok(())
}

fn unescape_property(value: &str) -> Result<String, CoreError> {
    let mut chars = value.chars();
    let mut output = String::new();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            output.push(ch);
            continue;
        }
        match chars.next() {
            None => {}
            Some('t') => output.push('\t'),
            Some('n') => output.push('\n'),
            Some('r') => output.push('\r'),
            Some('f') => output.push('\u{000c}'),
            Some('u') => {
                let mut hex = String::new();
                for _ in 0..4 {
                    hex.push(
                        chars.next().ok_or_else(|| {
                            CoreError::new("profile", "Incomplete Unicode escape")
                        })?,
                    );
                }
                let code = u16::from_str_radix(&hex, 16)
                    .map_err(|_| CoreError::new("profile", "Malformed Unicode escape"))?;
                let scalar = if (0xd800..=0xdbff).contains(&code) {
                    if chars.next() != Some('\\') || chars.next() != Some('u') {
                        return Err(CoreError::new(
                            "profile",
                            "Unpaired UTF-16 surrogate in Unicode escape",
                        ));
                    }
                    let low: String = chars.by_ref().take(4).collect();
                    let low = u16::from_str_radix(&low, 16).map_err(|_| {
                        CoreError::new("profile", "Malformed Unicode surrogate escape")
                    })?;
                    if !(0xdc00..=0xdfff).contains(&low) {
                        return Err(CoreError::new("profile", "Invalid UTF-16 surrogate pair"));
                    }
                    0x10000 + ((u32::from(code) - 0xd800) << 10) + (u32::from(low) - 0xdc00)
                } else {
                    u32::from(code)
                };
                output.push(char::from_u32(scalar).ok_or_else(|| {
                    CoreError::new("profile", "Unpaired UTF-16 surrogate in Unicode escape")
                })?);
            }
            Some(other) => output.push(other),
        }
    }
    Ok(output)
}
