//! What a mod's state means to the player and what they can do about it.
//!
//! Everything here is a pure function of observed state, so the action button,
//! the right-click menu and the tests all read the same answer.

use crate::{activation::Eligibility, observe::Place, runtime::RunState, steam::ItemState};
use luabuddy_core::ModId;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Tone {
    Normal,
    Muted,
    Good,
    Waiting,
    Warning,
    Bad,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum Runtime {
    #[default]
    NotLoaded,
    Loaded,
    Failed(String),
    NeedsGameStart(String),
}

impl Runtime {
    /// What the interface needs from a mod's run state, if it has one.
    pub fn of(state: Option<&RunState>) -> Self {
        match state {
            Some(RunState::Loaded) => Self::Loaded,
            Some(RunState::Failed(reason)) => Self::Failed(reason.clone()),
            Some(RunState::RestartRequired(reason)) => Self::NeedsGameStart(reason.clone()),
            Some(RunState::Disabled | RunState::ServerLoaded) | None => Self::NotLoaded,
        }
    }
}

/// Why a change waits while the game's Options screen has unsaved edits.
pub const OPTIONS_PENDING: &str = "Apply or cancel your pending game Options changes first.";
const UPDATE_AVAILABLE: &str = "Update available";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct World {
    pub place: Place,
    /// Startup has finished and no other LuaBuddy operation is running.
    pub ready: bool,
    pub options_pending: bool,
    pub steam: bool,
}

impl World {
    fn change_blocked(&self) -> Option<String> {
        self.options_pending.then(|| OPTIONS_PENDING.into())
    }

    fn load_blocked(&self) -> Option<String> {
        self.change_blocked().or_else(|| {
            (!self.ready).then(|| "Available as soon as joining the server has finished.".into())
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Requirement {
    pub id: ModId,
    pub title: String,
    pub installed: bool,
    pub on: bool,
}

impl Requirement {
    /// How a required mod stands, in a word.
    pub fn state(&self) -> (Tone, &'static str) {
        if !self.installed {
            (Tone::Bad, "not installed")
        } else if self.on {
            (Tone::Good, "on")
        } else {
            (Tone::Muted, "off")
        }
    }
}

/// Everything known about one mod identity.
#[derive(Clone, Debug)]
pub struct ModView {
    pub id: ModId,
    pub title: String,
    pub description: String,
    pub author: Option<String>,
    pub version: Option<String>,
    pub minimum: Option<String>,
    pub maximum: Option<String>,
    pub directory: Option<String>,
    pub workshop: Option<u64>,
    pub workshop_state: Option<ItemState>,
    pub installed: bool,
    pub compatible: bool,
    /// The game's own mod list can select it for a normal launch.
    pub native_startup: bool,
    pub requirements: Vec<Requirement>,
    /// Mods that are turned on and need this one, directly or through another,
    /// with their names.
    pub dependents: Vec<(ModId, String)>,
    /// Names of running mods built on this one, which a reload would pull out
    /// from under.
    pub running_dependents: Vec<String>,
    pub selected: bool,
    /// Selected for this game process only.
    pub temporary: bool,
    /// Registered for one future game process.
    pub queued: bool,
    pub next_startup: bool,
    pub native_active: bool,
    pub runtime: Runtime,
    /// Its code ran and failed, so only a new connection can try again.
    pub spent: bool,
    pub can_reload: bool,
    pub eligibility: Option<Eligibility>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum State {
    Failed,
    NeedsGameStart,
    NotInstalled,
    MissingRequirement,
    Running,
    RunningNative,
    /// On, and loadable right now.
    Ready,
    /// On, and waiting for a server to be joined.
    Waiting,
    NextLaunch,
    Queued,
    Off,
    Incompatible,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Section {
    Attention,
    Running,
    On,
    NextLaunch,
    Off,
    Native,
    Unavailable,
}

impl Section {
    pub fn title(self, place: Place) -> &'static str {
        match self {
            Self::Attention => "Needs attention",
            Self::Running => "Running",
            Self::On if place == Place::Server => "On, not loaded yet",
            Self::On => "Loads when you join a server",
            Self::NextLaunch => "Next launch",
            Self::Off => "Off",
            Self::Native if place == Place::Server => "Loaded by the server",
            Self::Native => "Loaded by the game",
            Self::Unavailable => "Can't be used",
        }
    }

    /// Sections that are reference material stay folded until asked for.
    pub fn starts_folded(self) -> bool {
        matches!(self, Self::Native | Self::Unavailable)
    }
}

impl ModView {
    pub fn state(&self, place: Place) -> State {
        if self.native_active {
            return State::RunningNative;
        }
        match &self.runtime {
            Runtime::Loaded => return State::Running,
            Runtime::Failed(_) if self.selected => return State::Failed,
            Runtime::NeedsGameStart(_) if self.selected && !self.next_startup => {
                return State::NeedsGameStart;
            }
            _ => (),
        }
        if !self.installed {
            return State::NotInstalled;
        }
        if !self.compatible {
            return State::Incompatible;
        }
        if self.selected {
            if self.requirements.iter().any(|need| !need.installed) {
                return State::MissingRequirement;
            }
            if self.next_startup && matches!(self.runtime, Runtime::NeedsGameStart(_)) {
                return State::NextLaunch;
            }
            return if place == Place::Server {
                State::Ready
            } else {
                State::Waiting
            };
        }
        if self.queued {
            State::Queued
        } else if self.next_startup {
            State::NextLaunch
        } else {
            State::Off
        }
    }

    pub fn section(&self, place: Place) -> Section {
        match self.state(place) {
            State::Failed
            | State::NeedsGameStart
            | State::NotInstalled
            | State::MissingRequirement => Section::Attention,
            State::Running => Section::Running,
            State::RunningNative => Section::Native,
            State::Ready | State::Waiting => Section::On,
            State::NextLaunch | State::Queued => Section::NextLaunch,
            State::Off => Section::Off,
            State::Incompatible => Section::Unavailable,
        }
    }

    pub fn tone(&self, place: Place) -> Tone {
        match self.state(place) {
            State::Failed | State::NotInstalled | State::MissingRequirement => Tone::Bad,
            State::NeedsGameStart => Tone::Warning,
            State::Running | State::RunningNative => Tone::Good,
            State::Ready | State::Waiting | State::NextLaunch | State::Queued => Tone::Waiting,
            State::Off | State::Incompatible => Tone::Muted,
        }
    }

    /// A short statement of the state, suitable for a list row.
    pub fn status(&self, place: Place) -> String {
        match self.state(place) {
            State::Failed => match &self.runtime {
                Runtime::Failed(reason) => format!("Failed: {}", short_reason(reason)),
                _ => "Failed to load".into(),
            },
            State::NeedsGameStart => "Has to start with the game".into(),
            State::NotInstalled => "Not installed".into(),
            State::MissingRequirement => {
                let missing = self.missing();
                format!("Needs {}", missing.join(", "))
            }
            State::Running if self.temporary => "Running, this game only".into(),
            State::Running if !self.selected => "Running until you reconnect".into(),
            State::Running => "Running".into(),
            State::RunningNative if place == Place::Server => "Running from the server".into(),
            State::RunningNative => "Running with the game".into(),
            State::Ready => "On, not loaded yet".into(),
            State::Waiting => "Loads when you join a server".into(),
            State::NextLaunch => "Starts with the game next launch".into(),
            State::Queued => "Queued for one game launch".into(),
            State::Off => "Off".into(),
            State::Incompatible => "Not made for this game version".into(),
        }
    }

    /// What a list row adds to its section's title, which already says the state.
    /// The second value is false when the row states a condition of its own.
    pub fn row(&self, place: Place) -> (String, bool) {
        let plain = match self.state(place) {
            State::Running => self.selected && !self.temporary,
            State::RunningNative | State::Ready | State::Waiting | State::Off => true,
            _ => false,
        };
        if !plain {
            return (self.status(place), false);
        }
        let text = match (&self.version, &self.author) {
            (Some(version), Some(author)) => format!("{version}, by {author}"),
            (Some(version), None) => version.clone(),
            (None, Some(author)) => format!("by {author}"),
            (None, None) => self.id.to_string(),
        };
        (text, true)
    }

    /// Alphabetical by name, ignoring decoration such as "[B42]".
    pub fn sort_key(&self) -> String {
        let mut name = self.title.trim();
        while let Some(rest) = name.strip_prefix('[').and_then(|rest| rest.split_once(']')) {
            name = rest.1.trim_start();
        }
        let name = name.trim_start_matches(|ch: char| !ch.is_alphanumeric());
        if name.is_empty() {
            self.title.to_lowercase()
        } else {
            name.to_lowercase()
        }
    }

    /// Why the state is what it is, and what resolves it.
    pub fn explanation(&self, place: Place) -> Option<String> {
        Some(match self.state(place) {
            State::Failed if self.spent => {
                "Part of it already ran, so it can only be tried again after you reconnect.".into()
            }
            State::Failed => "None of its code ran. Fix the cause, then try again.".into(),
            State::NeedsGameStart => match &self.runtime {
                Runtime::NeedsGameStart(reason) => reason.clone(),
                _ => return None,
            },
            State::NotInstalled => {
                "It is turned on, but the game can't find its files. Install it, or turn it off."
                    .into()
            }
            State::MissingRequirement => {
                "It can't load without the mods it needs. Install them, or turn it off.".into()
            }
            State::Running if self.temporary => {
                "It was selected for this game only and is gone after you quit.".into()
            }
            State::Running if !self.selected => {
                "It is turned off from now on. Code that is already running can't be unloaded."
                    .into()
            }
            State::RunningNative => {
                "The game loaded it normally, so there is nothing for LuaBuddy to load.".into()
            }
            State::NextLaunch => "Quit and start the game to use it.".into(),
            State::Queued => {
                "The luabuddy command queued it. It loads in the next game you start, once.".into()
            }
            State::Ready if !self.off_requirements().is_empty() => {
                format!("Also needs {}.", self.off_requirements().join(", "))
            }
            State::Off => match &self.eligibility {
                Some(Eligibility::RestartRequired { reason }) if self.native_startup => {
                    reason.clone()
                }
                Some(Eligibility::RestartRequired { reason }) => format!(
                    "{reason} Its Workshop files use a layout the game's own mod list doesn't show."
                ),
                _ => return None,
            },
            State::Incompatible => match (&self.minimum, &self.maximum) {
                (Some(minimum), Some(maximum)) => {
                    format!("It needs game version {minimum} to {maximum}.")
                }
                (Some(minimum), None) => format!("It needs game version {minimum} or later."),
                (None, Some(maximum)) => format!("It needs game version {maximum} or earlier."),
                (None, None) => "The game reports it as unavailable.".into(),
            },
            State::Running | State::Ready | State::Waiting => return None,
        })
    }

    pub fn problem(&self) -> Option<&str> {
        match &self.runtime {
            Runtime::Failed(reason) if self.selected => Some(reason),
            _ => None,
        }
    }

    fn missing(&self) -> Vec<String> {
        self.requirements
            .iter()
            .filter(|need| !need.installed)
            .map(|need| need.title.clone())
            .collect()
    }

    fn off_requirements(&self) -> Vec<String> {
        self.requirements
            .iter()
            .filter(|need| need.installed && !need.on)
            .map(|need| need.title.clone())
            .collect()
    }

    fn can_load_late(&self) -> bool {
        !matches!(self.eligibility, Some(Eligibility::RestartRequired { .. }))
            && !matches!(self.runtime, Runtime::NeedsGameStart(_))
    }

    pub fn matches(&self, filter: &str) -> bool {
        self.title.to_lowercase().contains(filter)
            || self.id.as_str().to_lowercase().contains(filter)
            || self
                .author
                .as_ref()
                .is_some_and(|author| author.to_lowercase().contains(filter))
            || self
                .workshop
                .is_some_and(|id| id.to_string().contains(filter))
            || self.description.to_lowercase().contains(filter)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Act {
    LoadNow(ModId),
    TurnOn(ModId),
    /// Turns off every listed mod together.
    TurnOff(Vec<ModId>),
    StartWithGame(ModId),
    DontStartWithGame(ModId),
    Reload(ModId),
    LoadWaiting,
    Rescan,
    ShowMod(ModId),
    ShowContained(u64),
    Subscribe(u64),
    Update(u64),
    OpenPage(u64),
    Copy(String, &'static str),
    OpenFolder(String),
    OpenLogs,
    ShowExtensions,
    RetryExtensions,
    Quit,
}

impl Act {
    /// Whether the action changes selections, loaded code or downloads, so the
    /// surrounding game state is worth recording with it.
    pub fn changes_state(&self) -> bool {
        matches!(
            self,
            Self::LoadNow(_)
                | Self::TurnOn(_)
                | Self::TurnOff(_)
                | Self::StartWithGame(_)
                | Self::DontStartWithGame(_)
                | Self::Reload(_)
                | Self::LoadWaiting
                | Self::Subscribe(_)
                | Self::Update(_)
                | Self::RetryExtensions
        )
    }

    /// The name recorded for this action in the journal.
    pub fn name(&self) -> &'static str {
        match self {
            Self::LoadNow(_) => "load-now",
            Self::TurnOn(_) => "turn-on",
            Self::TurnOff(_) => "turn-off",
            Self::StartWithGame(_) => "start-with-game",
            Self::DontStartWithGame(_) => "dont-start-with-game",
            Self::Reload(_) => "reload",
            Self::LoadWaiting => "load-waiting",
            Self::Rescan => "rescan",
            Self::ShowMod(_) => "show-mod",
            Self::ShowContained(_) => "show-contained",
            Self::Subscribe(_) => "subscribe",
            Self::Update(_) => "update",
            Self::OpenPage(_) => "open-page",
            Self::Copy(..) => "copy",
            Self::OpenFolder(_) => "open-folder",
            Self::OpenLogs => "open-logs",
            Self::ShowExtensions => "show-extensions",
            Self::RetryExtensions => "retry-extensions",
            Self::Quit => "quit",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Node {
    pub label: String,
    pub hint: Option<String>,
    /// Present but not possible yet, and the reason.
    pub blocked: Option<String>,
    pub act: Option<Act>,
    pub children: Vec<Node>,
    /// The action this state calls for, shown as the main button.
    pub main: bool,
}

impl Node {
    pub fn act(label: impl Into<String>, act: Act) -> Self {
        Self {
            label: label.into(),
            hint: None,
            blocked: None,
            act: Some(act),
            children: Vec::new(),
            main: false,
        }
    }
    /// A button with no action of its own, whose press the screen handles.
    pub fn label(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            hint: None,
            blocked: None,
            act: None,
            children: Vec::new(),
            main: false,
        }
    }
    fn group(label: impl Into<String>, children: Vec<Node>) -> Option<Self> {
        (!children.is_empty()).then(|| Self {
            label: label.into(),
            hint: None,
            blocked: None,
            act: None,
            children,
            main: false,
        })
    }
    fn main(mut self) -> Self {
        self.main = true;
        self
    }
    pub fn hint(mut self, text: impl Into<String>) -> Self {
        self.hint = Some(text.into());
        self
    }
    fn blocked(mut self, reason: Option<String>) -> Self {
        self.blocked = reason;
        self
    }
}

/// The action a mod's state calls for, if its menu has one. Other actions stay
/// in the menu rather than standing in for it.
pub fn primary(menu: &[Node]) -> Option<Node> {
    menu.iter().find(|node| node.main).cloned()
}

/// Every action that applies to this mod in this situation, likeliest first.
pub fn mod_menu(view: &ModView, world: World) -> Vec<Node> {
    let mut nodes = Vec::new();
    let state = view.state(world.place);
    let id = || view.id.clone();
    match state {
        State::Ready => nodes.push(
            Node::act("Load now", Act::LoadNow(id()))
                .hint("Loads it, and the mods it needs, into this game.")
                .blocked(world.load_blocked())
                .main(),
        ),
        State::Failed if !view.spent && world.place == Place::Server => nodes.push(
            Node::act("Try again", Act::LoadNow(id()))
                .hint("Checks it again and loads it if the cause is gone.")
                .blocked(world.load_blocked())
                .main(),
        ),
        State::Off if view.can_load_late() && world.place == Place::Server && !view.spent => {
            nodes.push(
                Node::act("Load now", Act::LoadNow(id()))
                    .hint("Turns it on, with the mods it needs, and loads it into this game.")
                    .blocked(world.load_blocked())
                    .main(),
            );
        }
        State::Off if view.can_load_late() => nodes.push(
            Node::act("Turn on", Act::TurnOn(id()))
                .hint("LuaBuddy loads it, with the mods it needs, whenever you join a server.")
                .blocked(world.change_blocked())
                .main(),
        ),
        State::Running if !view.selected && !view.temporary => nodes.push(
            Node::act("Turn back on", Act::TurnOn(id()))
                .hint("Keeps loading it whenever you join a server.")
                .blocked(world.change_blocked())
                .main(),
        ),
        State::Running if view.can_reload => nodes.push(
            Node::act("Reload", Act::Reload(id()))
                .hint("Runs the mod's own cleanup, then loads its current files.")
                .blocked(world.load_blocked().or_else(|| {
                    (!view.running_dependents.is_empty()).then(|| {
                        format!(
                            "{} runs on top of it, so reconnect to reload it.",
                            view.running_dependents.join(", ")
                        )
                    })
                }))
                .main(),
        ),
        _ => (),
    }
    let start = start_nodes(view, world);
    // Starting with the game is the main action when it is the only way to use the mod.
    let (promoted, start): (Vec<_>, Vec<_>) = start.into_iter().partition(|node| {
        matches!(node.act, Some(Act::StartWithGame(_)))
            && (state == State::NeedsGameStart || state == State::Off && !view.can_load_late())
    });
    nodes.extend(promoted.into_iter().map(Node::main));
    if state == State::NextLaunch
        && let Some(quit) = start.iter().find(|node| node.act == Some(Act::Quit))
    {
        nodes.push(quit.clone().main());
    }
    nodes.extend(turn_off(view, world));
    nodes.extend(
        start
            .into_iter()
            .filter(|node| state != State::NextLaunch || node.act != Some(Act::Quit)),
    );
    nodes.extend(Node::group(
        "Needs",
        view.requirements
            .iter()
            .map(|need| {
                let node = Node::act(
                    format!("{} ({})", need.title, need.state().1),
                    Act::ShowMod(need.id.clone()),
                );
                node.blocked((!need.installed).then(|| format!("{} is not installed.", need.title)))
            })
            .collect(),
    ));
    nodes.extend(Node::group(
        "Needed by",
        view.dependents
            .iter()
            .map(|(dependent, title)| Node::act(title.clone(), Act::ShowMod(dependent.clone())))
            .collect(),
    ));
    if let Some(workshop) = view.workshop {
        nodes.extend(Node::group(
            "Workshop",
            workshop_nodes(workshop, view.workshop_state, world),
        ));
    }
    let mut copy = Vec::new();
    if let Some(problem) = view.problem() {
        copy.push(Node::act(
            "The full error",
            Act::Copy(problem.to_owned(), "Error copied."),
        ));
    }
    copy.push(Node::act(
        "Mod ID",
        Act::Copy(view.id.to_string(), "Mod ID copied."),
    ));
    if let Some(directory) = &view.directory {
        copy.push(Node::act(
            "Folder path",
            Act::Copy(directory.clone(), "Folder path copied."),
        ));
    }
    if let Some(workshop) = view.workshop {
        copy.push(Node::act(
            "Workshop link",
            Act::Copy(workshop_url(workshop), "Workshop link copied."),
        ));
    }
    nodes.extend(Node::group("Copy", copy));
    if let Some(directory) = &view.directory {
        nodes.push(Node::act("Open folder", Act::OpenFolder(directory.clone())));
    }
    nodes
}

fn start_nodes(view: &ModView, world: World) -> Vec<Node> {
    let mut nodes = Vec::new();
    if view.installed && view.compatible && view.native_startup {
        nodes.push(if view.next_startup {
            Node::act(
                "Don't start with the game",
                Act::DontStartWithGame(view.id.clone()),
            )
            .hint("Changes the game's own mod list. Running code is not affected.")
            .blocked(world.change_blocked())
        } else {
            Node::act(
                "Start with the game",
                Act::StartWithGame(view.id.clone()),
            )
            .hint(
                "Adds it, with the mods it needs, to the game's own mod list for the next launch.",
            )
            .blocked(world.change_blocked())
        });
    }
    if view.next_startup && !view.native_active {
        nodes.push(
            Node::act("Quit the game...", Act::Quit)
                .hint("Opens the game's own quit confirmation."),
        );
    }
    nodes
}

fn turn_off(view: &ModView, world: World) -> Option<Node> {
    if !view.selected {
        return None;
    }
    if view.temporary {
        return None;
    }
    let hint = if view.runtime == Runtime::Loaded {
        "Won't load again. Code that is already running stays until you reconnect."
    } else {
        "Won't load any more. Files and saved settings are kept."
    };
    // A one-launch queue entry would load it once more anyway; only the
    // command line that queued it can cancel it.
    let blocked = world.change_blocked().or_else(|| {
        view.queued.then(|| {
            format!(
                "It is also queued for one future launch. Cancel that first with: luabuddy deselect --mod {} --once",
                view.id
            )
        })
    });
    let mut ids = vec![view.id.clone()];
    if view.dependents.is_empty() {
        return Some(
            Node::act("Turn off", Act::TurnOff(ids))
                .hint(hint)
                .blocked(blocked),
        );
    }
    ids.extend(view.dependents.iter().map(|(id, _)| id.clone()));
    let count = view.dependents.len();
    Some(
        Node::act(
            format!(
                "Turn off, with the {} that {} it",
                if count == 1 {
                    "mod".into()
                } else {
                    format!("{count} mods")
                },
                if count == 1 { "needs" } else { "need" }
            ),
            Act::TurnOff(ids),
        )
        .hint(format!(
            "{hint} Also turns off {}.",
            view.dependents
                .iter()
                .map(|(_, title)| title.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ))
        .blocked(blocked),
    )
}

/// The first line of a failure, without the script's folder or the wrapper's
/// own words, so the part that differs fits on one row.
pub fn short_reason(reason: &str) -> String {
    let line = reason.lines().next().unwrap_or(reason);
    let line = line.strip_prefix("Host Lua call failed: ").unwrap_or(line);
    match line.find(".lua:").or_else(|| line.find(".LUA:")) {
        Some(end) => {
            let start = line[..end].rfind(['/', '\\']).map_or(0, |slash| slash + 1);
            line[start..].to_owned()
        }
        None => line.to_owned(),
    }
}

pub fn workshop_url(id: u64) -> String {
    format!("https://steamcommunity.com/sharedfiles/filedetails/?id={id}")
}

fn workshop_nodes(id: u64, state: Option<ItemState>, world: World) -> Vec<Node> {
    let mut nodes = Vec::new();
    if world.steam
        && let Some(state) = state
        && !state.downloading
        && !state.download_pending
    {
        if !state.subscribed {
            nodes.push(
                Node::act("Subscribe and download", Act::Subscribe(id))
                    .hint("Subscribes with your Steam account and downloads the files."),
            );
        } else if state.needs_update || !state.installed {
            nodes.push(Node::act(
                if state.installed {
                    "Download the update"
                } else {
                    "Download"
                },
                Act::Update(id),
            ));
        }
    }
    nodes.push(Node::act("Open its page", Act::OpenPage(id)));
    nodes
}

/// One Workshop result, which can deliver several mod identities.
#[derive(Clone, Debug)]
pub struct ItemView {
    pub id: Option<u64>,
    pub title: String,
    pub description: String,
    pub updated: Option<u32>,
    pub bytes: Option<u64>,
    pub needs: usize,
    pub state: Option<ItemState>,
    /// Steam's reason when it won't describe the item.
    pub unavailable: Option<String>,
    /// Installed mod identities that came from this item.
    pub contains: Vec<(ModId, String)>,
}

impl ItemView {
    pub fn status(&self) -> String {
        if self.unavailable.is_some() {
            return "Steam can't show this item".into();
        }
        let Some(state) = self.state else {
            return "Unknown".into();
        };
        if state.downloading || state.download_pending {
            "Downloading"
        } else if state.installed && state.needs_update {
            UPDATE_AVAILABLE
        } else if state.installed && state.subscribed {
            "Installed"
        } else if state.installed {
            "Installed, not subscribed"
        } else if state.subscribed {
            "Subscribed, not downloaded"
        } else {
            "Not subscribed"
        }
        .into()
    }

    pub fn tone(&self) -> Tone {
        let Some(state) = self.state.filter(|_| self.unavailable.is_none()) else {
            return Tone::Warning;
        };
        if state.downloading || state.download_pending {
            Tone::Waiting
        } else if state.installed && state.needs_update {
            Tone::Warning
        } else if state.installed {
            Tone::Good
        } else if state.subscribed {
            Tone::Waiting
        } else {
            Tone::Muted
        }
    }

    pub fn matches(&self, filter: &str) -> bool {
        self.title.to_lowercase().contains(filter)
            || self.id.is_some_and(|id| id.to_string().contains(filter))
            || self.description.to_lowercase().contains(filter)
    }
}

pub fn item_menu(view: &ItemView, world: World) -> Vec<Node> {
    let Some(id) = view.id else {
        return Vec::new();
    };
    let mut nodes = Vec::new();
    if view.unavailable.is_none() {
        let mut actions = workshop_nodes(id, view.state, world);
        let page = actions.pop();
        nodes.extend(actions);
        if !view.contains.is_empty() {
            nodes.push(Node::act("Show in My mods", Act::ShowContained(id)));
            if view.contains.len() > 1
                && let Some(group) = Node::group(
                    "Contains",
                    view.contains
                        .iter()
                        .map(|(id, title)| Node::act(title.clone(), Act::ShowMod(id.clone())))
                        .collect(),
                )
            {
                nodes.push(group);
            }
        }
        nodes.extend(page);
    } else {
        nodes.push(Node::act("Open its page", Act::OpenPage(id)));
    }
    nodes.extend(Node::group(
        "Copy",
        vec![
            Node::act(
                "Workshop link",
                Act::Copy(workshop_url(id), "Workshop link copied."),
            ),
            Node::act(
                "Workshop ID",
                Act::Copy(id.to_string(), "Workshop ID copied."),
            ),
        ],
    ));
    nodes
}

/// Facts about the whole session that decide the window's own menu.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Totals {
    pub waiting: usize,
    pub restart: usize,
    pub extensions: usize,
    pub extensions_pending: usize,
}

pub fn load_waiting(totals: Totals, world: World) -> Option<Node> {
    (world.place == Place::Server && totals.waiting > 0).then(|| {
        Node::act(
            match totals.waiting {
                1 => "Load 1 waiting mod".to_owned(),
                count => format!("Load {count} waiting mods"),
            },
            Act::LoadWaiting,
        )
        .blocked(world.load_blocked())
    })
}

/// The action at the window's foot: loading what waits, or quitting to apply
/// changes to the game's own mod list.
pub fn footer(totals: Totals, world: World) -> Option<Node> {
    load_waiting(totals, world).or_else(|| {
        (totals.restart > 0).then(|| {
            Node::act("Quit to apply...", Act::Quit)
                .hint("Your changes to the game's mod list apply when you start it again.")
        })
    })
}

pub fn tools_menu(totals: Totals, world: World) -> Vec<Node> {
    let mut nodes = Vec::new();
    nodes.extend(load_waiting(totals, world));
    nodes.push(
        Node::act("Look for new mods", Act::Rescan)
            .hint("Reads your mod folders again, without loading anything."),
    );
    if totals.extensions > 0 {
        let mut children = vec![Node::act("Show status", Act::ShowExtensions)];
        if totals.extensions_pending > 0 {
            children.push(
                Node::act("Retry waiting patches", Act::RetryExtensions)
                    .hint("Retries code you already approved. It approves nothing new.")
                    .blocked(world.change_blocked()),
            );
        }
        nodes.extend(Node::group("Native extensions", children));
    }
    nodes.push(
        Node::act("Open the log folder", Act::OpenLogs)
            .hint("Holds the journal of everything LuaBuddy did, and the last diagnostic report."),
    );
    if totals.restart > 0 {
        nodes.push(
            Node::act("Quit the game...", Act::Quit)
                .hint("Your changes to the game's mod list apply when you start it again."),
        );
    }
    nodes
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Block {
    Title(String),
    Status(Tone, String),
    Text(Tone, String),
    Heading(String),
    Fact(String, Tone, String),
}

pub fn mod_document(view: &ModView, world: World) -> Vec<Block> {
    let mut blocks = vec![
        Block::Title(view.title.clone()),
        Block::Status(
            view.tone(world.place),
            if view.state(world.place) == State::Failed {
                "Failed to load".into()
            } else {
                view.status(world.place)
            },
        ),
    ];
    if let Some(text) = view.explanation(world.place) {
        blocks.push(Block::Text(Tone::Normal, text));
    }
    if let Some(problem) = view.problem() {
        blocks.push(Block::Heading("What went wrong".into()));
        blocks.push(Block::Text(Tone::Bad, problem.to_owned()));
    }
    if let Some(Eligibility::Live { limitations }) = &view.eligibility
        && !limitations.is_empty()
        && !view.native_active
    {
        blocks.push(Block::Heading("What LuaBuddy can't load".into()));
        blocks.push(Block::Text(
            Tone::Normal,
            format!(
                "Its {} only work when it starts with the game or runs on the server.",
                crate::activation::descriptions(limitations)
            ),
        ));
    }
    blocks.push(Block::Heading("Details".into()));
    let fact = |name: &str, value: &Option<String>| {
        value
            .as_ref()
            .map(|value| Block::Fact(name.into(), Tone::Normal, value.clone()))
    };
    blocks.extend(fact("Author", &view.author));
    blocks.extend(fact("Version", &view.version));
    blocks.push(Block::Fact(
        "Mod ID".into(),
        Tone::Normal,
        view.id.to_string(),
    ));
    if !view.requirements.is_empty() {
        for need in &view.requirements {
            let (tone, state) = need.state();
            blocks.push(Block::Fact(
                "Needs".into(),
                tone,
                format!("{} ({state})", need.title),
            ));
        }
    }
    if view.next_startup && view.state(world.place) != State::NextLaunch {
        blocks.push(Block::Fact(
            "Next launch".into(),
            Tone::Waiting,
            "Starts with the game".into(),
        ));
    }
    if let Some(state) = view.workshop_state {
        blocks.push(Block::Fact(
            "Workshop".into(),
            if state.needs_update {
                Tone::Warning
            } else {
                Tone::Normal
            },
            if state.needs_update {
                UPDATE_AVAILABLE.into()
            } else if state.subscribed {
                "Subscribed".into()
            } else {
                "Not subscribed".into()
            },
        ));
    }
    if let Some(directory) = &view.directory {
        blocks.push(Block::Fact("Folder".into(), Tone::Muted, directory.clone()));
    }
    push_description(&mut blocks, &view.description);
    blocks
}

pub fn item_document(view: &ItemView) -> Vec<Block> {
    let mut blocks = vec![
        Block::Title(view.title.clone()),
        Block::Status(view.tone(), view.status()),
    ];
    if let Some(reason) = &view.unavailable {
        blocks.push(Block::Text(Tone::Normal, reason.clone()));
    }
    if view.state.is_some_and(|state| state.installed) && !view.contains.is_empty() {
        blocks.push(Block::Text(
            Tone::Normal,
            "Downloading delivers files. Turn its mods on in My mods.".into(),
        ));
    }
    blocks.push(Block::Heading("Details".into()));
    for (_, title) in &view.contains {
        blocks.push(Block::Fact("Contains".into(), Tone::Normal, title.clone()));
    }
    if view.needs > 0 {
        blocks.push(Block::Fact(
            "Needs".into(),
            Tone::Normal,
            match view.needs {
                1 => "1 other Workshop item, listed on its page".into(),
                count => format!("{count} other Workshop items, listed on its page"),
            },
        ));
    }
    if let Some(updated) = view.updated {
        let time = std::time::UNIX_EPOCH + std::time::Duration::from_secs(u64::from(updated));
        let date = luabuddy_core::journal::timestamp(time);
        blocks.push(Block::Fact(
            "Updated".into(),
            Tone::Normal,
            date.split('T').next().unwrap_or(&date).to_owned(),
        ));
    }
    if let Some(bytes) = view.bytes {
        blocks.push(Block::Fact("Size".into(), Tone::Normal, size(bytes)));
    }
    if let Some(id) = view.id {
        blocks.push(Block::Fact(
            "Workshop ID".into(),
            Tone::Normal,
            id.to_string(),
        ));
    }
    push_description(&mut blocks, &view.description);
    blocks
}

fn push_description(blocks: &mut Vec<Block>, text: &str) {
    let text = super::markup::readable(text);
    if text.is_empty() {
        return;
    }
    blocks.push(Block::Heading("About".into()));
    for line in text.lines() {
        blocks.push(Block::Text(Tone::Normal, line.to_owned()));
    }
}

pub fn size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

pub fn plural(count: usize, one: &str, many: &str) -> String {
    format!("{count} {}", if count == 1 { one } else { many })
}

pub fn summary(running: usize, attention: usize, waiting: usize, place: Place) -> (Tone, String) {
    let mut parts = Vec::new();
    if attention > 0 {
        parts.push(format!(
            "{} attention",
            plural(attention, "mod needs", "mods need")
        ));
    }
    if running > 0 {
        parts.push(plural(running, "mod running", "mods running"));
    }
    if waiting > 0 {
        parts.push(format!(
            "{} {}",
            plural(waiting, "mod", "mods"),
            if place == Place::Server {
                "ready to load"
            } else {
                "waiting for a server"
            }
        ));
    }
    (
        if attention > 0 {
            Tone::Warning
        } else {
            Tone::Muted
        },
        if parts.is_empty() {
            "No mods are turned on.".into()
        } else {
            format!("{}.", parts.join(", "))
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activation::StartupComponent;

    fn id(text: &str) -> ModId {
        ModId::new(text).unwrap()
    }

    fn installed(name: &str) -> ModView {
        ModView {
            id: id(name),
            title: name.into(),
            description: String::new(),
            author: None,
            version: None,
            minimum: None,
            maximum: None,
            directory: Some(format!("/mods/{name}")),
            workshop: None,
            workshop_state: None,
            installed: true,
            compatible: true,
            native_startup: true,
            requirements: Vec::new(),
            dependents: Vec::new(),
            running_dependents: Vec::new(),
            selected: false,
            temporary: false,
            queued: false,
            next_startup: false,
            native_active: false,
            runtime: Runtime::NotLoaded,
            spent: false,
            can_reload: false,
            eligibility: Some(Eligibility::Live {
                limitations: Vec::new(),
            }),
        }
    }

    fn world(place: Place) -> World {
        World {
            place,
            ready: true,
            options_pending: false,
            steam: true,
        }
    }

    fn acts(nodes: &[Node]) -> Vec<Act> {
        nodes.iter().filter_map(|node| node.act.clone()).collect()
    }

    #[test]
    fn an_off_mod_is_turned_on_in_the_menu_and_loaded_on_a_server() {
        let view = installed("A");
        assert_eq!(
            primary(&mod_menu(&view, world(Place::MainMenu)))
                .unwrap()
                .act,
            Some(Act::TurnOn(id("A")))
        );
        assert_eq!(
            primary(&mod_menu(&view, world(Place::Server))).unwrap().act,
            Some(Act::LoadNow(id("A")))
        );
        // Nothing offered here can unload it, because it isn't on.
        assert!(
            !acts(&mod_menu(&view, world(Place::Server)))
                .iter()
                .any(|act| matches!(act, Act::TurnOff(_)))
        );
    }

    #[test]
    fn a_mod_that_must_start_with_the_game_offers_only_that_path() {
        let mut view = installed("Maps");
        view.eligibility = Some(Eligibility::RestartRequired {
            reason: "It has maps.".into(),
        });
        let nodes = mod_menu(&view, world(Place::Server));
        assert_eq!(nodes[0].act, Some(Act::StartWithGame(id("Maps"))));
        assert!(
            !acts(&nodes)
                .iter()
                .any(|act| matches!(act, Act::LoadNow(_) | Act::TurnOn(_)))
        );
        assert_eq!(
            view.explanation(Place::Server).as_deref(),
            Some("It has maps.")
        );
    }

    #[test]
    fn turning_off_names_and_includes_every_dependent() {
        let mut view = installed("Lib");
        view.selected = true;
        view.dependents = vec![
            (id("UsesLib"), "Uses Lib".into()),
            (id("AlsoUsesLib"), "Also Uses Lib".into()),
        ];
        let off = mod_menu(&view, world(Place::MainMenu))
            .into_iter()
            .find(|node| matches!(node.act, Some(Act::TurnOff(_))))
            .unwrap();
        assert_eq!(off.label, "Turn off, with the 2 mods that need it");
        assert_eq!(
            off.act,
            Some(Act::TurnOff(vec![
                id("Lib"),
                id("UsesLib"),
                id("AlsoUsesLib")
            ]))
        );
    }

    #[test]
    fn a_temporary_selection_cannot_be_turned_off() {
        let mut view = installed("Once");
        view.selected = true;
        view.temporary = true;
        view.runtime = Runtime::Loaded;
        assert!(
            !acts(&mod_menu(&view, world(Place::Server)))
                .iter()
                .any(|act| matches!(act, Act::TurnOff(_)))
        );
        assert_eq!(view.status(Place::Server), "Running, this game only");
    }

    #[test]
    fn a_failure_is_retried_only_when_no_code_ran() {
        let mut view = installed("Broken");
        view.selected = true;
        view.runtime =
            Runtime::Failed("/mods/Broken/media/lua/client/Broken.lua:1: unexpected symbol".into());
        assert_eq!(view.section(Place::Server), Section::Attention);
        assert_eq!(
            view.status(Place::Server),
            "Failed: Broken.lua:1: unexpected symbol"
        );
        assert_eq!(
            primary(&mod_menu(&view, world(Place::Server))).unwrap().act,
            Some(Act::LoadNow(id("Broken")))
        );
        view.spent = true;
        assert!(
            !acts(&mod_menu(&view, world(Place::Server)))
                .iter()
                .any(|act| matches!(act, Act::LoadNow(_)))
        );
        assert!(
            view.explanation(Place::Server)
                .unwrap()
                .contains("reconnect")
        );
    }

    #[test]
    fn reload_waits_for_mods_running_on_top_and_for_pending_options() {
        let mut view = installed("Base");
        view.selected = true;
        view.runtime = Runtime::Loaded;
        view.can_reload = true;
        let first = primary(&mod_menu(&view, world(Place::Server))).unwrap();
        assert_eq!(first.act, Some(Act::Reload(id("Base"))));
        assert_eq!(first.blocked, None);
        view.running_dependents = vec!["Top".into()];
        let blocked = primary(&mod_menu(&view, world(Place::Server)))
            .unwrap()
            .blocked;
        assert!(blocked.unwrap().starts_with("Top runs on top of it"));
        view.running_dependents.clear();
        let mut pending = world(Place::Server);
        pending.options_pending = true;
        assert!(
            primary(&mod_menu(&view, pending))
                .unwrap()
                .blocked
                .is_some()
        );
    }

    #[test]
    fn a_mod_the_game_already_runs_offers_no_late_loading() {
        let mut view = installed("Native");
        view.native_active = true;
        view.next_startup = true;
        let nodes = acts(&mod_menu(&view, world(Place::Server)));
        assert!(
            !nodes
                .iter()
                .any(|act| matches!(act, Act::LoadNow(_) | Act::TurnOn(_) | Act::Reload(_)))
        );
        assert!(nodes.contains(&Act::DontStartWithGame(id("Native"))));
        assert_eq!(view.section(Place::Server), Section::Native);
        assert_eq!(view.status(Place::Server), "Running from the server");
    }

    #[test]
    fn a_missing_requirement_is_named_and_needs_attention() {
        let mut view = installed("Top");
        view.selected = true;
        view.requirements = vec![Requirement {
            id: id("Gone"),
            title: "Gone mod".into(),
            installed: false,
            on: false,
        }];
        assert_eq!(view.state(Place::MainMenu), State::MissingRequirement);
        assert_eq!(view.status(Place::MainMenu), "Needs Gone mod");
        let needs = mod_menu(&view, world(Place::MainMenu))
            .into_iter()
            .find(|node| node.label == "Needs")
            .unwrap();
        assert!(needs.children[0].blocked.is_some());
    }

    #[test]
    fn only_limits_that_matter_are_described() {
        let mut view = installed("Partial");
        view.eligibility = Some(Eligibility::Live {
            limitations: vec![
                StartupComponent::Animations,
                StartupComponent::SandboxOptions,
            ],
        });
        let text = mod_document(&view, world(Place::MainMenu))
            .into_iter()
            .find_map(|block| match block {
                Block::Text(_, text) if text.starts_with("Its ") => Some(text),
                _ => None,
            })
            .unwrap();
        assert_eq!(
            text,
            "Its animation assets and sandbox settings only work when it starts with the game or runs on the server."
        );
        view.native_active = true;
        assert!(!mod_document(&view, world(Place::MainMenu)).iter().any(
            |block| matches!(block, Block::Heading(text) if text == "What LuaBuddy can't load")
        ));
    }

    #[test]
    fn workshop_items_offer_delivery_only_when_it_would_change_something() {
        let mut item = ItemView {
            id: Some(42),
            title: "Item".into(),
            description: String::new(),
            updated: None,
            bytes: None,
            needs: 0,
            state: Some(ItemState::default()),
            unavailable: None,
            contains: Vec::new(),
        };
        assert_eq!(
            item_menu(&item, world(Place::MainMenu))[0].act,
            Some(Act::Subscribe(42))
        );
        item.state = Some(ItemState {
            subscribed: true,
            installed: true,
            ..ItemState::default()
        });
        item.contains = vec![(id("A"), "A".into())];
        let first = item_menu(&item, world(Place::MainMenu))[0].act.clone();
        assert_eq!(first, Some(Act::ShowContained(42)));
        let mut offline = world(Place::MainMenu);
        offline.steam = false;
        item.state = Some(ItemState::default());
        assert!(
            !acts(&item_menu(&item, offline))
                .iter()
                .any(|act| matches!(act, Act::Subscribe(_) | Act::Update(_)))
        );
    }

    #[test]
    fn rows_sort_by_name_without_decoration_and_say_only_what_the_section_does_not() {
        let mut view = installed("AutoAll");
        view.title = "[B42.20] Auto All".into();
        view.version = Some("1.0".into());
        view.author = Some("Pegasus".into());
        assert_eq!(view.sort_key(), "auto all");
        assert_eq!(view.row(Place::MainMenu), ("1.0, by Pegasus".into(), true));
        view.selected = true;
        view.next_startup = true;
        view.runtime = Runtime::NeedsGameStart("It has maps.".into());
        assert_eq!(
            view.row(Place::MainMenu),
            ("Starts with the game next launch".into(), false)
        );
    }

    #[test]
    fn the_window_menu_offers_loading_only_on_a_server_with_mods_waiting() {
        let totals = Totals {
            waiting: 2,
            ..Totals::default()
        };
        assert_eq!(
            load_waiting(totals, world(Place::Server)).unwrap().label,
            "Load 2 waiting mods"
        );
        assert!(load_waiting(totals, world(Place::MainMenu)).is_none());
        assert!(
            !tools_menu(Totals::default(), world(Place::Server))
                .iter()
                .any(|node| node.label == "Native extensions" || node.act == Some(Act::Quit))
        );
    }

    #[test]
    fn a_queued_mod_cannot_be_turned_off_here() {
        let mut view = installed("Queued");
        view.selected = true;
        view.queued = true;
        let node = mod_menu(&view, world(Place::Server))
            .into_iter()
            .find(|node| matches!(node.act, Some(Act::TurnOff(_))))
            .expect("Turn off is offered");
        assert!(node.blocked.unwrap().contains("--once"));
        view.queued = false;
        let node = mod_menu(&view, world(Place::Server))
            .into_iter()
            .find(|node| matches!(node.act, Some(Act::TurnOff(_))))
            .unwrap();
        assert_eq!(node.blocked, None);
    }

    #[test]
    fn the_footer_loads_what_waits_or_offers_to_quit() {
        let waiting = Totals {
            waiting: 2,
            ..Totals::default()
        };
        let load = footer(waiting, world(Place::Server)).unwrap();
        assert_eq!(
            (load.label.as_str(), load.act),
            ("Load 2 waiting mods", Some(Act::LoadWaiting))
        );
        assert_eq!(footer(waiting, world(Place::MainMenu)), None);
        let restart = Totals {
            restart: 1,
            ..Totals::default()
        };
        assert_eq!(
            footer(restart, world(Place::MainMenu)).unwrap().act,
            Some(Act::Quit)
        );
        assert_eq!(footer(Totals::default(), world(Place::Server)), None);
    }

    #[test]
    fn a_requirement_reads_as_its_state() {
        let need = |installed, on| Requirement {
            id: id("Needed"),
            title: "Needed".into(),
            installed,
            on,
        };
        assert_eq!(need(false, true).state(), (Tone::Bad, "not installed"));
        assert_eq!(need(true, true).state(), (Tone::Good, "on"));
        assert_eq!(need(true, false).state(), (Tone::Muted, "off"));
    }
}
