use crate::{CoreError, ModId, Profile};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path, PathBuf};

#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ModuleKey(String);

impl ModuleKey {
    pub fn new(name: &str) -> Result<Self, CoreError> {
        let mut name = name.replace('\\', "/").to_lowercase();
        if name.ends_with(".lua") {
            name.truncate(name.len() - 4);
        }
        if name.is_empty() || name.contains('\0') {
            return Err(CoreError::new(
                "module",
                "Module name must be nonempty and contain no NUL",
            ));
        }
        Ok(Self(name))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
    pub fn alias(&self) -> Option<Self> {
        self.0
            .split_once('/')
            .map(|(_, tail)| Self(tail.to_owned()))
    }
    pub fn shared(&self) -> bool {
        self.0.starts_with("shared/")
    }
}

impl TryFrom<String> for ModuleKey {
    type Error = CoreError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(&value)
    }
}
impl From<ModuleKey> for String {
    fn from(value: ModuleKey) -> Self {
        value.0
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct Source {
    pub name: String,
    pub filename: PathBuf,
    pub text: String,
}

pub fn scan_sources(
    profile: &Profile,
    roots: &[PathBuf],
    active: &BTreeSet<ModId>,
) -> Result<Vec<Source>, CoreError> {
    let patterns = profile
        .exclusions(active)
        .iter()
        .map(|s| Glob::parse(s))
        .collect::<Result<Vec<_>, _>>()?;
    let mut sources = BTreeMap::new();
    for root in roots {
        for phase in ["shared", "client"] {
            let directory = root.join("media/lua").join(phase);
            if !directory.is_dir() {
                continue;
            }
            let mut files = Vec::new();
            source_files(&directory, &mut files)?;
            files.sort();
            for file in files {
                let relative = file
                    .strip_prefix(&directory)
                    .map_err(|e| CoreError::new("source-scan", e.to_string()).at(&file))?;
                let name = format!("{phase}/{}", relative.to_string_lossy().replace('\\', "/"));
                let lower = name.to_lowercase();
                if !lower.ends_with(".lua")
                    || lower.starts_with("shared/translate/")
                    || patterns.iter().any(|p| p.matches(&name))
                {
                    continue;
                }
                let text = fs::read_to_string(&file)
                    .map_err(|e| CoreError::new("source-read", e.to_string()).at(&file))?;
                let filename = absolute_normalized_path(&file)?;
                sources.insert(
                    lower,
                    Source {
                        name,
                        filename,
                        text,
                    },
                );
            }
        }
    }
    if sources.is_empty() {
        return Err(CoreError::new(
            "source-scan",
            format!("No client or shared Lua files for {}", profile.id),
        )
        .at(&profile.file));
    }
    Ok(sources.into_values().collect())
}

pub fn absolute_normalized_path(path: &Path) -> Result<PathBuf, CoreError> {
    let absolute = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()
            .map_err(|e| CoreError::new("path", e.to_string()))?
            .join(path)
    };
    let mut normalized = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    Ok(normalized)
}

fn source_files(directory: &Path, files: &mut Vec<PathBuf>) -> Result<(), CoreError> {
    for entry in fs::read_dir(directory)
        .map_err(|e| CoreError::new("source-scan", e.to_string()).at(directory))?
    {
        let entry =
            entry.map_err(|e| CoreError::new("source-scan", e.to_string()).at(directory))?;
        let kind = entry
            .file_type()
            .map_err(|e| CoreError::new("source-scan", e.to_string()).at(entry.path()))?;
        if kind.is_dir() {
            source_files(&entry.path(), files)?;
        } else if entry.path().is_file() {
            files.push(entry.path());
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
pub struct Generation(pub u64);

#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
pub struct Owner {
    pub id: ModId,
    pub generation: Generation,
}

#[derive(Debug)]
pub enum ModuleState<T> {
    Prepared,
    Loading,
    Loaded(Option<T>),
    Failed(String),
}

#[derive(Debug)]
pub struct Module<T> {
    pub source: Source,
    pub owner: Owner,
    pub state: ModuleState<T>,
}

#[derive(Debug)]
pub enum Require<'a, T> {
    Delegate,
    Execute(ModuleKey),
    Cached(Option<&'a T>),
}

#[derive(Debug)]
pub struct ModuleRegistry<T> {
    modules: BTreeMap<ModuleKey, Module<T>>,
    shadowed: BTreeMap<(Owner, ModuleKey), Module<T>>,
    aliases: BTreeMap<ModuleKey, ModuleKey>,
}

impl<T> Default for ModuleRegistry<T> {
    fn default() -> Self {
        Self {
            modules: BTreeMap::new(),
            shadowed: BTreeMap::new(),
            aliases: BTreeMap::new(),
        }
    }
}

impl<T> ModuleRegistry<T> {
    pub fn get(&self, key: &ModuleKey) -> Option<&Module<T>> {
        self.modules.get(key)
    }
    pub fn iter(&self) -> impl Iterator<Item = (&ModuleKey, &Module<T>)> {
        self.modules.iter()
    }
    pub fn provider(&self, name: &ModuleKey) -> Option<&ModuleKey> {
        self.aliases.get(name)
    }

    pub fn register(&mut self, owner: Owner, sources: Vec<Source>) -> Result<(), CoreError> {
        let entries = sources
            .into_iter()
            .map(|source| ModuleKey::new(&source.name).map(|key| (key, source)))
            .collect::<Result<Vec<_>, _>>()?;
        for (key, source) in entries {
            if let Some(previous) = self.modules.insert(
                key.clone(),
                Module {
                    source,
                    owner: owner.clone(),
                    state: ModuleState::Prepared,
                },
            ) {
                self.shadowed
                    .insert((previous.owner.clone(), key), previous);
            }
        }
        self.rebuild_aliases();
        Ok(())
    }

    pub fn require(&self, name: &str) -> Result<Require<'_, T>, CoreError> {
        let name = ModuleKey::new(name)?;
        let Some(key) = self.aliases.get(&name) else {
            return Ok(Require::Delegate);
        };
        let Some(module) = self.modules.get(key) else {
            return Err(CoreError::new("module", "Alias has no registered source"));
        };
        match &module.state {
            ModuleState::Prepared => Ok(Require::Execute(key.clone())),
            ModuleState::Loading => Err(CoreError::new(
                "require",
                format!("Circular Lua dependency: {}", key.as_str()),
            )),
            ModuleState::Loaded(value) => Ok(Require::Cached(value.as_ref())),
            ModuleState::Failed(message) => Err(CoreError::new("require", message)),
        }
    }

    pub fn begin(&mut self, key: &ModuleKey) -> Result<(), CoreError> {
        let owner = self
            .modules
            .get(key)
            .ok_or_else(|| CoreError::new("require", "Unknown source"))?
            .owner
            .clone();
        self.begin_owned(&owner, key)
    }

    pub fn owned(&self, owner: &Owner, key: &ModuleKey) -> Option<&Module<T>> {
        self.modules
            .get(key)
            .filter(|module| &module.owner == owner)
            .or_else(|| self.shadowed.get(&(owner.clone(), key.clone())))
    }

    pub fn require_owned(
        &self,
        owner: &Owner,
        key: &ModuleKey,
    ) -> Result<Require<'_, T>, CoreError> {
        let module = self
            .owned(owner, key)
            .ok_or_else(|| CoreError::new("require", "Unknown owned source"))?;
        match &module.state {
            ModuleState::Prepared => Ok(Require::Execute(key.clone())),
            ModuleState::Loading => Err(CoreError::new(
                "require",
                format!("Circular Lua dependency: {}", key.as_str()),
            )),
            ModuleState::Loaded(value) => Ok(Require::Cached(value.as_ref())),
            ModuleState::Failed(message) => Err(CoreError::new("require", message)),
        }
    }

    fn owned_mut(&mut self, owner: &Owner, key: &ModuleKey) -> Option<&mut Module<T>> {
        if self
            .modules
            .get(key)
            .is_some_and(|module| &module.owner == owner)
        {
            self.modules.get_mut(key)
        } else {
            self.shadowed.get_mut(&(owner.clone(), key.clone()))
        }
    }

    pub fn begin_owned(&mut self, owner: &Owner, key: &ModuleKey) -> Result<(), CoreError> {
        let module = self
            .owned_mut(owner, key)
            .ok_or_else(|| CoreError::new("require", "Unknown source"))?;
        if !matches!(module.state, ModuleState::Prepared) {
            return Err(CoreError::new(
                "require",
                "Source is already executing, loaded, or failed",
            ));
        }
        module.state = ModuleState::Loading;
        Ok(())
    }

    pub fn finish(
        &mut self,
        key: &ModuleKey,
        value: Result<Option<T>, String>,
    ) -> Result<(), CoreError> {
        let owner = self
            .modules
            .get(key)
            .ok_or_else(|| CoreError::new("require", "Unknown source"))?
            .owner
            .clone();
        self.finish_owned(&owner, key, value)
    }

    pub fn finish_owned(
        &mut self,
        owner: &Owner,
        key: &ModuleKey,
        value: Result<Option<T>, String>,
    ) -> Result<(), CoreError> {
        let module = self
            .owned_mut(owner, key)
            .ok_or_else(|| CoreError::new("require", "Unknown source"))?;
        if !matches!(module.state, ModuleState::Loading) {
            return Err(CoreError::new(
                "require",
                "Only an executing source can finish",
            ));
        }
        module.state = match value {
            Ok(value) => ModuleState::Loaded(value),
            Err(message) => ModuleState::Failed(message),
        };
        Ok(())
    }

    pub fn fail_owner(&mut self, owner: &Owner, error: &str) {
        for module in self
            .modules
            .values_mut()
            .chain(self.shadowed.values_mut())
            .filter(|module| &module.owner == owner)
        {
            module.state = ModuleState::Failed(format!("Mod unavailable: {}: {error}", owner.id));
        }
    }

    pub fn remove_owner(&mut self, owner: &Owner) {
        let removed: Vec<_> = self
            .modules
            .iter()
            .filter(|(_, module)| &module.owner == owner)
            .map(|(key, _)| key.clone())
            .collect();
        self.modules.retain(|_, module| &module.owner != owner);
        self.shadowed.retain(|(previous, _), _| previous != owner);
        for key in removed {
            let replacement = self
                .shadowed
                .keys()
                .filter(|(_, previous)| previous == &key)
                .max_by_key(|(owner, _)| owner.generation)
                .cloned();
            if let Some(replacement) = replacement
                && let Some(module) = self.shadowed.remove(&replacement)
            {
                self.modules.insert(key, module);
            }
        }
        self.rebuild_aliases();
    }

    fn rebuild_aliases(&mut self) {
        self.aliases.clear();
        for key in self.modules.keys() {
            self.aliases.insert(key.clone(), key.clone());
        }
        for key in self.modules.keys() {
            if let Some(alias) = key.alias() {
                let previous = self.aliases.get(&alias);
                if previous.is_none_or(|previous| key.shared() || !previous.shared()) {
                    self.aliases.insert(alias, key.clone());
                }
            }
        }
    }
}

#[derive(Debug, Clone)]
enum Token {
    Literal(char),
    Star,
    Recursive,
    Any,
    Class {
        inverted: bool,
        ranges: Vec<(char, char)>,
    },
}

#[derive(Debug, Clone)]
pub struct Glob(Vec<Vec<Token>>);

impl Glob {
    pub fn parse(pattern: &str) -> Result<Self, CoreError> {
        let alternatives = expand_braces(pattern)?;
        alternatives
            .into_iter()
            .map(|pattern| {
                let mut chars = pattern.chars().peekable();
                let mut tokens = Vec::new();
                while let Some(ch) = chars.next() {
                    tokens.push(match ch {
                        '*' if chars.peek() == Some(&'*') => {
                            chars.next();
                            Token::Recursive
                        }
                        '*' => Token::Star,
                        '?' => Token::Any,
                        '\\' => Token::Literal(
                            chars
                                .next()
                                .ok_or_else(|| CoreError::new("exclude", "Trailing glob escape"))?,
                        ),
                        '[' => {
                            let inverted = chars.peek() == Some(&'!');
                            if inverted {
                                chars.next();
                            }
                            let mut members = Vec::new();
                            loop {
                                let first = chars.next().ok_or_else(|| {
                                    CoreError::new("exclude", "Unclosed glob character class")
                                })?;
                                if first == ']' && !members.is_empty() {
                                    break;
                                }
                                if chars.peek() == Some(&'-') {
                                    chars.next();
                                    let last = chars.next().ok_or_else(|| {
                                        CoreError::new("exclude", "Unclosed glob range")
                                    })?;
                                    if last == ']' || first > last {
                                        return Err(CoreError::new(
                                            "exclude",
                                            "Invalid glob character range",
                                        ));
                                    }
                                    members.push((first, last));
                                } else {
                                    members.push((first, first));
                                }
                            }
                            Token::Class {
                                inverted,
                                ranges: members,
                            }
                        }
                        other => Token::Literal(other),
                    });
                }
                Ok(tokens)
            })
            .collect::<Result<Vec<_>, CoreError>>()
            .map(Self)
    }

    pub fn matches(&self, value: &str) -> bool {
        let value: Vec<_> = value.chars().collect();
        self.0.iter().any(|tokens| {
            let mut row = vec![false; value.len() + 1];
            row[0] = true;
            for token in tokens {
                let mut next = vec![false; row.len()];
                for i in 0..row.len() {
                    match token {
                        Token::Star | Token::Recursive => {
                            next[i] = row[i]
                                || (i > 0
                                    && next[i - 1]
                                    && (matches!(token, Token::Recursive) || value[i - 1] != '/'));
                        }
                        _ if i > 0 && row[i - 1] => {
                            next[i] = match token {
                                Token::Literal(ch) => value[i - 1] == *ch,
                                Token::Any => value[i - 1] != '/',
                                Token::Class { inverted, ranges } => {
                                    value[i - 1] != '/'
                                        && (ranges
                                            .iter()
                                            .any(|(a, b)| *a <= value[i - 1] && value[i - 1] <= *b)
                                            != *inverted)
                                }
                                _ => false,
                            }
                        }
                        _ => {}
                    }
                }
                row = next;
            }
            row[value.len()]
        })
    }
}

fn expand_braces(pattern: &str) -> Result<Vec<String>, CoreError> {
    let mut escaped = false;
    let mut start = None;
    for (i, ch) in pattern.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            continue;
        }
        if ch == '{' {
            if start.is_some() {
                return Err(CoreError::new(
                    "exclude",
                    "Nested glob groups are unsupported by the Java glob contract",
                ));
            }
            start = Some(i);
        } else if ch == '}' {
            let Some(start) = start else {
                return Err(CoreError::new("exclude", "Unmatched glob group delimiter"));
            };
            let mut result = Vec::new();
            for alternative in pattern[start + 1..i].split(',') {
                let expanded = format!("{}{alternative}{}", &pattern[..start], &pattern[i + 1..]);
                result.extend(expand_braces(&expanded)?);
                if result.len() > 1024 {
                    return Err(CoreError::new(
                        "exclude",
                        "Glob groups expand to more than 1024 alternatives",
                    ));
                }
            }
            return Ok(result);
        }
    }
    if start.is_some() {
        return Err(CoreError::new("exclude", "Unclosed glob group"));
    }
    Ok(vec![pattern.to_owned()])
}
