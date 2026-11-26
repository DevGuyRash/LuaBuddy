//! The window: what it shows, and what its controls do.

use super::{
    Event, activity, callback,
    catalog::Catalog,
    entry, kit, menu,
    model::{
        self, Act, Block, ItemView, ModView, Node, Requirement, Runtime, Section, State, Tone,
        Totals, World,
    },
    panes::{self, Face, List, Page, Row},
    set_callback,
    workshop::{Progress, Source, Workshop},
};
use crate::{
    activation, game,
    log::{self, Operation as Intent},
    lua::{Context, Result, Value},
    observe::{self, Place},
    profile_store,
    runtime::{self, RunState},
};
use kit::Font;
use luabuddy_core::{
    ModId, Profile,
    journal::{Entry, Level},
    lifecycle::Operation,
};
use serde_json::{Value as Json, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    time::{Duration, Instant},
};

const PAD: f64 = 10.0;
const POLL: Duration = Duration::from_millis(150);
const READ_BUDGET: Duration = Duration::from_millis(6);
const STATUS_LASTS: Duration = Duration::from_secs(12);
/// How often Activity is shown again so "2 min ago" stays true.
const AGES_ADVANCE: Duration = Duration::from_secs(30);

#[derive(Clone, Debug, PartialEq)]
pub enum Key {
    Header(Section),
    Mod(ModId),
    Folder(String),
    Item(u64),
    /// A Workshop result that Steam returned without an identity.
    Nameless(String),
    Entry(u64),
    More,
    Note,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Tab {
    Mods,
    Workshop,
    Activity,
}

impl Tab {
    fn name(self) -> &'static str {
        match self {
            Self::Mods => "mods",
            Self::Workshop => "workshop",
            Self::Activity => "activity",
        }
    }
}

/// The player's selections and the running state, read together.
struct Live {
    world: World,
    profiles: Vec<Profile>,
    unreadable: Vec<(std::path::PathBuf, String)>,
    /// Selections, plus bundled templates for mods not yet configured, which
    /// is what turning a mod on would use.
    effective: Vec<Profile>,
    templates: Vec<Profile>,
    statuses: BTreeMap<ModId, RunState>,
    reloadable: BTreeSet<ModId>,
    spent: BTreeSet<ModId>,
    active: BTreeSet<ModId>,
    startup: BTreeSet<ModId>,
    queued: BTreeSet<ModId>,
    temporary: BTreeSet<ModId>,
}

struct Buttons {
    back: Value,
    primary: Value,
    more: Value,
    tools: Value,
    footer: Value,
    tabs: Vec<(Tab, Value)>,
    sources: Vec<(Source, Value)>,
}

/// Window state kept when the window is rebuilt for a new screen size.
pub struct Carried {
    tab: Tab,
    /// Its source, results and any request or download still in progress.
    workshop: Workshop,
    texts: BTreeMap<&'static str, String>,
    chosen: BTreeMap<&'static str, Key>,
    folded: BTreeMap<Section, bool>,
}

/// The game's own mod list as the game process started with it, to tell
/// whether a restart is still needed however often the window reopens.
static STARTUP_AT_LAUNCH: std::sync::OnceLock<BTreeSet<ModId>> = std::sync::OnceLock::new();

/// Records the game's own mod list once, at the first menu, so later edits
/// through the game's mod selector count as needing a restart.
pub fn remember_launch_startup(context: &mut Context<'_, '_>) {
    if STARTUP_AT_LAUNCH.get().is_none()
        && let Ok(startup) = activation::selected_next_startup(context)
    {
        let _ = STARTUP_AT_LAUNCH.set(startup);
    }
}

pub struct Screen {
    pub session: u64,
    pub closed: bool,
    pub theme: kit::Theme,
    pub list: List<Key>,
    pub page_text: Page,
    root: Value,
    frame: Value,
    search: Value,
    buttons: Buttons,
    size: (f64, f64),
    tab: Tab,
    texts: BTreeMap<&'static str, String>,
    chosen: BTreeMap<&'static str, Key>,
    folded: BTreeMap<Section, bool>,
    catalog: Catalog,
    live: Live,
    startup_at_open: BTreeSet<ModId>,
    workshop: Workshop,
    entries: Vec<Entry>,
    /// The journal revision `entries` was copied at.
    journal_revision: Option<u64>,
    /// When the rows were last shown.
    shown_at: Instant,
    status: Option<(Tone, String, Instant)>,
    summary: (Tone, String),
    /// The selection's actions as of the last refresh. The main button, More
    /// and right-click all read these, so they always agree.
    actions: Vec<Node>,
    primary: Option<Node>,
    more_shown: bool,
    totals: Totals,
    /// How often the live state has been read, so an action can tell whether it read again.
    reads: u64,
    /// Folders were read since the rows were last shown.
    catalog_read: bool,
    /// Where the status line may draw, measured when the buttons are placed.
    status_span: [f64; 2],
    footer: Option<Node>,
    menu: Option<menu::Menu>,
    pictures: BTreeMap<String, Option<Value>>,
    menu_was_visible: bool,
    joypad: Value,
    next_poll: Instant,
    dirty: bool,
}

impl Screen {
    pub fn open(
        context: &mut Context<'_, '_>,
        session: u64,
        carried: Option<Carried>,
    ) -> Result<Self> {
        let started = Instant::now();
        kit::prepare(context)?;
        let theme = kit::Theme::load(context)?;
        let size = kit::screen(context)?;
        let mut live = Live::read(context)?;
        let catalog = Catalog::begin(context, false)?;
        let mut carried = carried;
        let workshop = match carried.as_mut() {
            Some(carried) => std::mem::take(&mut carried.workshop),
            None => Workshop::attach(context),
        };
        live.world.steam = workshop.available();
        let (root, frame, search, list, page, buttons) =
            build(context, session, size, workshop.available())?;
        let mut screen = Self {
            session,
            closed: false,
            theme,
            list: List::new(list),
            page_text: Page::new(page),
            root,
            frame,
            search,
            buttons,
            size,
            tab: Tab::Mods,
            texts: BTreeMap::new(),
            chosen: BTreeMap::new(),
            folded: BTreeMap::new(),
            catalog,
            startup_at_open: STARTUP_AT_LAUNCH
                .get_or_init(|| live.startup.clone())
                .clone(),
            live,
            workshop,
            entries: Vec::new(),
            journal_revision: None,
            shown_at: Instant::now(),
            status: None,
            summary: (Tone::Muted, String::new()),
            actions: Vec::new(),
            primary: None,
            more_shown: false,
            totals: Totals::default(),
            reads: 0,
            catalog_read: false,
            status_span: [0.0; 2],
            footer: None,
            menu: None,
            pictures: BTreeMap::new(),
            menu_was_visible: false,
            joypad: Value::default(),
            next_poll: Instant::now(),
            dirty: true,
        };
        if let Some(carried) = carried {
            screen.tab = carried.tab;
            screen.texts = carried.texts;
            screen.chosen = carried.chosen;
            screen.folded = carried.folded;
            let text = screen
                .texts
                .get(screen.tab.name())
                .cloned()
                .unwrap_or_default();
            let text = context.string(&text)?;
            context.invoke(&screen.search, "setText", &[text])?;
        }
        let shown = (|| {
            screen.menu_was_visible = entry::set_menu_visible(context, false)?;
            for method in ["setAlwaysOnTop", "setWantKeyEvents"] {
                kit::call_flag(context, &screen.root, method, true)?;
            }
            context.invoke(&screen.root, "addToUIManager", &[])?;
            context.invoke(&screen.root, "bringToTop", &[])?;
            screen.take_joypad(context)?;
            screen.lay_out(context)?;
            screen.refresh(context)
        })();
        if let Err(error) = shown {
            // Nothing half-built may stay registered in front of the game.
            let _ = screen.close(context);
            return Err(error);
        }
        log::info("ui.opened", "Opened the LuaBuddy window")
            .with(json!({
                "place": screen.live.world.place.name(),
                "steam": screen.live.world.steam,
                "selected": screen.live.profiles.iter().filter(|profile| profile.enabled).count(),
                "native": screen.live.active.len(),
            }))
            .took(started.elapsed())
            .write();
        Ok(screen)
    }

    fn close(&mut self, context: &mut Context<'_, '_>) -> Result<()> {
        if self.closed {
            return Ok(());
        }
        self.closed = true;
        // The panel leaves first. Every later step is best effort, so a failure
        // can't leave an invisible panel holding the player's input.
        let removed = kit::call_flag(context, &self.root, "setVisible", false).and_then(|()| {
            context
                .invoke(&self.root, "removeFromUIManager", &[])
                .map(drop)
        });
        let mut rest = self.close_menu(context);
        rest = rest.and(context.invoke(&self.search, "unfocus", &[]).map(drop));
        if self.menu_was_visible {
            rest = rest.and(entry::set_menu_visible(context, true).map(drop));
        }
        if !self.joypad.is_nil() {
            let joypad = self.joypad.clone();
            let restored = context
                .get(&self.root, "prevFocus")
                .and_then(|previous| focus_joypad(context, &joypad, &previous));
            rest = rest.and(restored);
        }
        log::debug("ui.closed", "Closed the LuaBuddy window").write();
        removed.and(rest)
    }

    /// Runs on each interface update. Returns what a replacement window keeps
    /// when the screen size changed.
    pub fn update(&mut self, context: &mut Context<'_, '_>) -> Result<Option<Carried>> {
        if self.closed {
            return Ok(None);
        }
        if kit::screen(context)? != self.size {
            self.close(context)?;
            return Ok(Some(Carried {
                tab: self.tab,
                workshop: std::mem::take(&mut self.workshop),
                texts: std::mem::take(&mut self.texts),
                chosen: std::mem::take(&mut self.chosen),
                folded: std::mem::take(&mut self.folded),
            }));
        }
        // New folders show at the poll's pace, not with a full refresh every frame.
        if self.catalog.progress().is_some() && self.catalog.step(context, READ_BUDGET) {
            self.catalog_read = true;
        }
        let text = self.search_text(context)?;
        if self.texts.get(self.tab.name()) != Some(&text) {
            self.close_menu(context)?;
            self.texts.insert(self.tab.name(), text.clone());
            if self.tab == Tab::Workshop {
                self.workshop.type_text(&text);
            }
            self.dirty = true;
        }
        if Instant::now() >= self.next_poll {
            self.next_poll = Instant::now() + POLL;
            self.poll(context)?;
        }
        if self.dirty {
            self.refresh(context)?;
        }
        Ok(None)
    }

    fn poll(&mut self, context: &mut Context<'_, '_>) -> Result<()> {
        self.dirty |= std::mem::take(&mut self.catalog_read);
        for progress in self.workshop.poll() {
            match progress {
                Progress::Items => self.dirty = true,
                Progress::Status(text) => self.say(Tone::Waiting, text),
                Progress::Failed(text) => self.say(Tone::Bad, text),
                Progress::Delivered(id) => {
                    self.rescan_catalog(context)?;
                    let contained = self.catalog.contained(id);
                    self.say(
                        Tone::Good,
                        match contained.len() {
                            0 => "Downloaded.".into(),
                            1 => format!("Downloaded {}. Turn it on in My mods.", contained[0].1),
                            count => format!("Downloaded {count} mods. Turn them on in My mods."),
                        },
                    );
                    self.dirty = true;
                }
            }
        }
        if self.tab == Tab::Workshop && self.workshop.has_more() && self.list.at_end(context)? {
            self.workshop.more()?;
            self.dirty = true;
        }
        // New entries show at once; the ages beside the old ones advance slowly.
        if self.tab == Tab::Activity
            && (Some(log::revision()) != self.journal_revision
                || self.shown_at.elapsed() > AGES_ADVANCE)
        {
            self.dirty = true;
        }
        if self
            .status
            .as_ref()
            .is_some_and(|(_, _, at)| at.elapsed() > STATUS_LASTS)
        {
            self.status = None;
        }
        // A menu dismissed by a click elsewhere hides itself but stays registered.
        if let Some(menu) = &self.menu
            && !menu.visible(context)?
        {
            self.close_menu(context)?;
        }
        Ok(())
    }

    fn search_text(&self, context: &mut Context<'_, '_>) -> Result<String> {
        let value = kit::first(context.invoke(&self.search, "getText", &[])?);
        context.text(&value)
    }

    fn filter(&self) -> String {
        self.texts
            .get(self.tab.name())
            .map(|text| text.trim().to_lowercase())
            .unwrap_or_default()
    }

    fn say(&mut self, tone: Tone, text: impl Into<String>) {
        self.status = Some((tone, text.into(), Instant::now()));
    }

    pub fn failed(&mut self, context: &mut Context<'_, '_>, error: &str) -> Result<()> {
        self.say(Tone::Bad, first_line(error));
        self.reread(context)?;
        self.refresh(context)
    }

    // ---- Observed state ----

    /// Reads every mod folder again, now, with the posters they show.
    fn rescan_catalog(&mut self, context: &mut Context<'_, '_>) -> Result<()> {
        self.catalog = Catalog::begin(context, true)?;
        self.catalog.finish(context);
        self.pictures.clear();
        self.dirty = true;
        Ok(())
    }

    fn reread(&mut self, context: &mut Context<'_, '_>) -> Result<()> {
        self.reads += 1;
        self.live = Live::read(context)?;
        self.live.world.steam = self.workshop.available();
        self.dirty = true;
        Ok(())
    }

    fn view(&mut self, id: &ModId, detailed: bool) -> ModView {
        let eligibility = if detailed {
            self.catalog
                .eligibility(id, &self.live.profiles, &self.live.templates)
        } else {
            None
        };
        let found = self.catalog.mods.get(id).cloned();
        let profile = self.live.profiles.iter().find(|profile| &profile.id == id);
        let requirements = self
            .catalog
            .direct_requirements(id, &self.live.effective)
            .into_iter()
            .map(|needed| Requirement {
                title: self.catalog.title(&needed),
                installed: self.catalog.mods.contains_key(&needed)
                    || self.live.active.contains(&needed)
                    || self.catalog.progress().is_some(),
                on: self.live.active.contains(&needed)
                    || self
                        .live
                        .profiles
                        .iter()
                        .any(|profile| profile.enabled && profile.id == needed),
                id: needed,
            })
            .collect();
        let native_startup = found.as_ref().is_some_and(|found| found.native_startup);
        // A selection for this game only, or queued by the command line, can't be
        // turned off here, so it isn't offered as part of turning this one off.
        let enabled_dependents = if detailed {
            self.catalog.enabled_dependents(id, &self.live.profiles)
        } else {
            Vec::new()
        };
        // Anything running on top of this mod blocks reloading it, however it
        // was selected.
        let running_dependents = enabled_dependents
            .iter()
            .filter(|dependent| {
                self.live
                    .statuses
                    .get(*dependent)
                    .is_some_and(RunState::is_loaded)
            })
            .map(|dependent| self.catalog.title(dependent))
            .collect();
        // A selection for this game only, or queued by the command line, can't be
        // turned off here, so it isn't offered as part of turning this one off.
        let dependents = enabled_dependents
            .into_iter()
            .filter(|dependent| {
                !self.live.temporary.contains(dependent) && !self.live.queued.contains(dependent)
            })
            .map(|dependent| {
                let title = self.catalog.title(&dependent);
                (dependent, title)
            })
            .collect::<Vec<_>>();
        ModView {
            title: self.catalog.title(id),
            description: found
                .as_ref()
                .map(|found| found.description.clone())
                .unwrap_or_default(),
            author: found.as_ref().and_then(|found| found.author.clone()),
            version: found.as_ref().and_then(|found| found.version.clone()),
            minimum: found.as_ref().and_then(|found| found.minimum.clone()),
            maximum: found.as_ref().and_then(|found| found.maximum.clone()),
            directory: found.as_ref().map(|found| found.directory.clone()),
            workshop: found.as_ref().and_then(|found| found.workshop),
            workshop_state: found
                .as_ref()
                .filter(|_| detailed)
                .and_then(|found| found.workshop)
                .and_then(|id| self.workshop.state(id)),
            installed: found.is_some() || self.catalog.progress().is_some(),
            compatible: found.as_ref().is_none_or(|found| found.compatible),
            native_startup,
            requirements,
            dependents,
            running_dependents,
            selected: profile.is_some_and(|profile| profile.enabled),
            temporary: self.live.temporary.contains(id),
            queued: self.live.queued.contains(id),
            next_startup: self.live.startup.contains(id),
            native_active: self.live.active.contains(id),
            runtime: Runtime::of(self.live.statuses.get(id)),
            spent: self.live.spent.contains(id),
            can_reload: self.live.reloadable.contains(id),
            eligibility,
            id: id.clone(),
        }
    }

    fn listed(&self) -> BTreeSet<ModId> {
        self.catalog
            .mods
            .keys()
            .cloned()
            .chain(
                self.live
                    .profiles
                    .iter()
                    .filter(|profile| profile.enabled)
                    .map(|profile| profile.id.clone()),
            )
            .collect()
    }

    fn item(&self, id: u64) -> Option<ItemView> {
        let text = self.texts.get(Tab::Workshop.name());
        let mut view = self
            .workshop
            .items(text.map(String::as_str).unwrap_or_default())
            .into_iter()
            .find(|view| view.id == Some(id))?;
        view.contains = self.catalog.contained(id);
        Some(view)
    }

    // ---- Showing it ----

    fn refresh(&mut self, context: &mut Context<'_, '_>) -> Result<()> {
        self.dirty = false;
        self.shown_at = Instant::now();
        let place = self.live.world.place;
        let (mut running, mut attention, mut waiting) = (0, 0, 0);
        let mut mods = Vec::new();
        let listed = self.listed();
        for id in &listed {
            let view = self.view(id, false);
            match view.section(place) {
                Section::Running => running += 1,
                Section::Attention => attention += 1,
                // Ready is what loading would take now; off a server nothing is.
                _ if view.state(place) == State::Ready => waiting += 1,
                _ => (),
            }
            mods.push(view);
        }
        self.summary = match self.catalog.progress() {
            Some((done, total)) => (
                Tone::Muted,
                format!("Reading your mods, {done} of {total}..."),
            ),
            None => model::summary(running, attention, waiting, place),
        };
        if self.tab == Tab::Workshop {
            let text = self
                .texts
                .get(Tab::Workshop.name())
                .cloned()
                .unwrap_or_default();
            let shown = self.workshop.items(&text).len();
            self.summary = (Tone::Muted, self.workshop.summary(shown));
        }
        let rows = match self.tab {
            Tab::Mods => self.mod_rows(mods),
            Tab::Workshop => self.item_rows(),
            Tab::Activity => {
                let revision = log::revision();
                if self.journal_revision != Some(revision) {
                    self.journal_revision = Some(revision);
                    self.entries = log::recent(usize::MAX);
                }
                let rows = activity::rows(&self.entries, &self.filter());
                if rows.is_empty() {
                    vec![note("Nothing recorded matches.")]
                } else {
                    rows
                }
            }
        };
        self.list.show(context, rows)?;
        // A remembered choice that is filtered or folded away is not acted on.
        self.list.selected = self
            .chosen
            .get(self.tab.name())
            .filter(|key| self.list.index_of(key).is_some())
            .cloned();
        self.totals = Totals {
            waiting,
            restart: usize::from(self.live.startup != self.startup_at_open),
            ..extension_totals()
        };
        self.footer = model::footer(self.totals, self.live.world);
        self.show_selection(context, &listed)?;
        self.show_buttons(context)
    }

    fn mod_rows(&self, mods: Vec<ModView>) -> Vec<Row<Key>> {
        let place = self.live.world.place;
        let filter = self.filter();
        let mut sections: BTreeMap<Section, Vec<Row<Key>>> = BTreeMap::new();
        let mut views = mods
            .into_iter()
            .filter(|view| filter.is_empty() || view.matches(&filter))
            .collect::<Vec<_>>();
        views.sort_by_cached_key(ModView::sort_key);
        for view in views {
            let (status, quiet) = view.row(place);
            sections.entry(view.section(place)).or_default().push(Row {
                key: Key::Mod(view.id.clone()),
                text: view.title.clone(),
                face: Face::Item {
                    status,
                    tone: view.tone(place),
                    quiet,
                },
            });
        }
        for (file, _) in &self.live.unreadable {
            let name = file
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            if !filter.is_empty() && !name.to_lowercase().contains(&filter) {
                continue;
            }
            sections.entry(Section::Attention).or_default().push(Row {
                key: Key::Folder(file.to_string_lossy().into_owned()),
                text: name,
                face: Face::Item {
                    status: "This selection file can't be read".into(),
                    tone: Tone::Bad,
                    quiet: false,
                },
            });
        }
        for broken in &self.catalog.broken {
            let name = broken
                .directory
                .rsplit(['/', '\\'])
                .next()
                .unwrap_or(&broken.directory);
            if filter.is_empty() || name.to_lowercase().contains(&filter) {
                sections.entry(Section::Unavailable).or_default().push(Row {
                    key: Key::Folder(broken.directory.clone()),
                    text: name.to_owned(),
                    face: Face::Item {
                        status: "The game can't read this folder".into(),
                        tone: Tone::Warning,
                        quiet: false,
                    },
                });
            }
        }
        if sections.is_empty() {
            return vec![note(if !filter.is_empty() {
                "No mod matches."
            } else if self.catalog.progress().is_some() {
                "Reading your mods..."
            } else {
                "No mods are installed."
            })];
        }
        let mut rows = Vec::new();
        for (section, items) in sections {
            let folded = filter.is_empty() && self.is_folded(section);
            rows.push(Row {
                key: Key::Header(section),
                text: section.title(place).to_uppercase(),
                face: Face::Header {
                    folded,
                    count: items.len(),
                },
            });
            if !folded {
                rows.extend(items);
            }
        }
        rows
    }

    fn item_rows(&self) -> Vec<Row<Key>> {
        let text = self
            .texts
            .get(Tab::Workshop.name())
            .map(String::as_str)
            .unwrap_or_default();
        let items = self.workshop.items(text);
        let mut rows = items
            .iter()
            .map(|view| Row {
                key: view
                    .id
                    .map_or_else(|| Key::Nameless(view.title.clone()), Key::Item),
                text: view.title.clone(),
                face: Face::Item {
                    status: view.status(),
                    tone: view.tone(),
                    quiet: false,
                },
            })
            .collect::<Vec<_>>();
        if self.workshop.loading() {
            rows.push(note("Asking Steam..."));
        } else if self.workshop.has_more() {
            rows.push(Row {
                key: Key::More,
                text: "Show more".into(),
                face: Face::More,
            });
        } else if let Some(problem) = &self.workshop.problem {
            rows.push(note(first_line(problem)));
        } else if rows.is_empty() {
            rows.push(note(if self.workshop.source == Source::Subscribed {
                "No subscribed item matches."
            } else {
                "Type to search the Workshop."
            }));
        }
        rows
    }

    /// Shows what is selected, or orients the player when nothing is, and
    /// settles the actions it offers. Each is built once per refresh.
    fn show_selection(
        &mut self,
        context: &mut Context<'_, '_>,
        listed: &BTreeSet<ModId>,
    ) -> Result<()> {
        let world = self.live.world;
        let mut picture = None;
        let mut menu = Vec::new();
        let mut primary = None;
        let blocks = match self.list.selected.clone() {
            Some(Key::Mod(id)) if listed.contains(&id) => {
                let view = self.view(&id, true);
                let path = self
                    .catalog
                    .mods
                    .get(&id)
                    .and_then(|found| found.poster.clone());
                picture = path.and_then(|path| self.picture(context, &path));
                menu = model::mod_menu(&view, world);
                primary = model::primary(&menu);
                model::mod_document(&view, world)
            }
            // Pressing a section title folds or opens it; the button says which.
            // A filter shows every match, so there is nothing to fold meanwhile.
            Some(Key::Header(section)) => {
                if self.filter().is_empty() {
                    let label = if self.is_folded(section) {
                        "Show"
                    } else {
                        "Hide"
                    };
                    primary = Some(Node::label(label));
                }
                self.orientation()
            }
            Some(Key::Folder(directory)) => {
                let selection = self
                    .live
                    .unreadable
                    .iter()
                    .find(|(file, _)| file.to_string_lossy() == directory.as_str());
                let (status, reason) = match selection {
                    Some((_, reason)) => ("This selection file can't be read", reason.clone()),
                    None => (
                        "The game can't read this folder",
                        self.catalog
                            .broken
                            .iter()
                            .find(|broken| broken.directory == directory)
                            .map(|broken| broken.reason.clone())
                            .unwrap_or_default(),
                    ),
                };
                menu = vec![
                    Node::act(
                        "Open folder",
                        Act::OpenFolder(containing_folder(&directory)),
                    ),
                    Node::act("Copy path", Act::Copy(directory.clone(), "Path copied.")),
                ];
                vec![
                    Block::Title(
                        directory
                            .rsplit(['/', '\\'])
                            .next()
                            .unwrap_or(&directory)
                            .to_owned(),
                    ),
                    Block::Status(Tone::Warning, status.into()),
                    Block::Text(Tone::Normal, reason),
                    Block::Heading("Details".into()),
                    Block::Fact("Path".into(), Tone::Muted, directory),
                ]
            }
            Some(Key::Item(id)) => match self.item(id) {
                Some(view) => {
                    menu = model::item_menu(&view, world);
                    model::item_document(&view)
                }
                None => self.orientation(),
            },
            Some(Key::Entry(seq)) => match self.entries.iter().find(|entry| entry.seq == seq) {
                Some(entry) => {
                    menu.push(Node::act(
                        "Copy this entry",
                        Act::Copy(activity::copy_text(entry), "Entry copied."),
                    ));
                    if let Some(id) = &entry.module {
                        menu.push(Node::act(
                            format!("Show {}", self.catalog.title(id)),
                            Act::ShowMod(id.clone()),
                        ));
                    }
                    activity::document(entry, &self.entries)
                }
                None => self.orientation(),
            },
            _ => self.orientation(),
        };
        if primary.is_none() && !matches!(self.list.selected, Some(Key::Mod(_) | Key::Header(_))) {
            primary = menu.iter().find(|node| node.act.is_some()).cloned();
        }
        self.actions = menu;
        self.primary = primary;
        self.page_text.show(context, &self.theme, blocks, picture)
    }

    fn picture(&mut self, context: &mut Context<'_, '_>, path: &str) -> Option<Value> {
        self.pictures
            .entry(path.to_owned())
            .or_insert_with(|| kit::texture(context, path))
            .clone()
    }

    fn orientation(&self) -> Vec<Block> {
        match self.tab {
            Tab::Mods => vec![
                Block::Title("Your mods".into()),
                Block::Status(self.summary.0, self.summary.1.clone()),
                Block::Text(
                    Tone::Normal,
                    match self.live.world.place {
                        Place::Server => {
                            "Choose a mod to see what it does here. Right-click one for its other actions."
                        }
                        Place::SinglePlayer | Place::MainMenu => {
                            "Mods you turn on here load when you join a multiplayer server. For single player, the game's own mod list starts mods with the game."
                        }
                    }
                    .into(),
                ),
            ],
            Tab::Workshop => vec![
                Block::Title("Workshop".into()),
                Block::Text(
                    Tone::Normal,
                    "Choose an item to see it. Downloading one only delivers its files; turn its mods on in My mods."
                        .into(),
                ),
            ],
            Tab::Activity => vec![
                Block::Title("Activity".into()),
                Block::Text(
                    Tone::Normal,
                    "Everything LuaBuddy did in this game, newest first. Type to search, including steps not listed here."
                        .into(),
                ),
            ],
        }
    }

    fn show_buttons(&mut self, context: &mut Context<'_, '_>) -> Result<()> {
        let [list_x, list_y, list_w, list_h] = self.list_bounds();
        let left = list_x + list_w + PAD;
        let y = self.actions_top();
        let has_menu = self.primary.is_some() || !self.actions.is_empty();
        let top = self.content_top();
        let bottom = if has_menu { y - PAD } else { list_y + list_h };
        let width = self.frame_size().0 - PAD - left;
        self.page_text
            .resize(context, &self.theme, [left, top, width, bottom - top])?;
        let mut x = left;
        let primary = self.primary.clone();
        let more_width = kit::width(context, Font::Small, "More...")? + 28.0 + PAD;
        x += place_action(
            context,
            &self.buttons.primary,
            primary.as_ref(),
            [x, y],
            width - more_width,
        )?;
        self.more_shown = !self.other_actions().is_empty();
        kit::call_flag(context, &self.buttons.more, "setVisible", self.more_shown)?;
        let more = kit::width(context, Font::Small, "More...")? + 28.0;
        kit::set_bounds(
            context,
            &self.buttons.more,
            [x, y, more, kit::control_height()],
        )?;
        let footer = self.footer.clone();
        let (w, h) = self.frame_size();
        let limit = w / 3.0;
        let width = footer
            .as_ref()
            .map(|node| kit::width(context, Font::Small, &node.label))
            .transpose()?
            .unwrap_or(0.0)
            .min(limit - 28.0)
            + 28.0;
        place_action(
            context,
            &self.buttons.footer,
            footer.as_ref(),
            [w - PAD - width, h - PAD - kit::control_height()],
            limit,
        )?;
        // The status line runs between the Close button and the footer action.
        let back = kit::ask_number(context, &self.buttons.back, "getWidth")?;
        let right = if footer.is_some() {
            w - 2.0 * PAD - width
        } else {
            w - PAD
        };
        self.status_span = [PAD + back + 2.0 * PAD, right];
        for (tab, button) in &self.buttons.tabs {
            mark(context, button, *tab == self.tab)?;
        }
        for (source, button) in &self.buttons.sources {
            kit::call_flag(context, button, "setVisible", self.tab == Tab::Workshop)?;
            mark(context, button, *source == self.workshop.source)?;
        }
        Ok(())
    }

    /// What More and right-click offer: every action but the one on the main button.
    fn other_actions(&self) -> Vec<Node> {
        let mut nodes = self.actions.clone();
        if let Some(primary) = &self.primary
            && let Some(index) = nodes.iter().position(|node| node == primary)
        {
            nodes.remove(index);
        }
        nodes
    }

    // ---- Layout ----

    fn frame_size(&self) -> (f64, f64) {
        frame_size(self.size)
    }

    fn content_top(&self) -> f64 {
        PAD + header_height() + PAD
    }

    fn list_bounds(&self) -> [f64; 4] {
        let (w, h) = self.frame_size();
        let rows = if self.tab == Tab::Workshop { 2.0 } else { 1.0 };
        let top = self.content_top() + rows * (kit::control_height() + 6.0);
        let bottom = h - 2.0 * PAD - kit::control_height();
        [PAD, top, (w * 0.40).clamp(260.0, 640.0), bottom - top]
    }

    fn actions_top(&self) -> f64 {
        let [_, y, _, h] = self.list_bounds();
        y + h - kit::control_height()
    }

    fn lay_out(&mut self, context: &mut Context<'_, '_>) -> Result<()> {
        let list = self.list_bounds();
        panes::place(context, &self.list.panel, list)?;
        let placeholder = match self.tab {
            Tab::Mods => "Filter your mods",
            Tab::Workshop => "Search",
            Tab::Activity => "Search activity",
        };
        let placeholder = context.string(placeholder)?;
        context.invoke(&self.search, "setPlaceholderText", &[placeholder])?;
        Ok(())
    }

    fn show_tab(&mut self, context: &mut Context<'_, '_>, tab: Tab) -> Result<()> {
        if tab == self.tab {
            return Ok(());
        }
        self.close_menu(context)?;
        self.tab = tab;
        let text = self.texts.get(tab.name()).cloned().unwrap_or_default();
        let value = context.string(&text)?;
        context.invoke(&self.search, "setText", &[value])?;
        self.texts.insert(tab.name(), text.clone());
        self.lay_out(context)?;
        if tab == Tab::Workshop
            && let Err(error) = self.workshop.open(self.workshop.source, &text)
        {
            self.workshop.problem = Some(error);
        }
        self.reread(context)?;
        self.refresh(context)
    }

    /// Runs every frame, so it only paints what layout already measured.
    pub fn draw(&mut self, context: &mut Context<'_, '_>) -> Result<()> {
        let (w, h) = self.frame_size();
        let [footer_left, footer_right] = self.status_span;
        let (tone, text) = match &self.status {
            Some((tone, text, _)) => (*tone, text.as_str()),
            None => (self.summary.0, self.summary.1.as_str()),
        };
        let text = kit::fit(
            context,
            Font::Small,
            first_line(text),
            footer_right - footer_left,
        )?;
        let color = self.theme.tone(tone);
        let mut painter = kit::Painter::new(context, &self.root)?;
        painter.rect(0.0, 0.0, self.size.0, self.size.1, [0.0, 0.0, 0.0, 0.6])?;
        let mut painter = kit::Painter::new(context, &self.frame)?;
        painter.rect(0.0, 0.0, w, h, [0.0, 0.0, 0.0, 0.94])?;
        painter.border(0.0, 0.0, w, h, kit::BORDER)?;
        painter.text(Font::Large, PAD + 2.0, PAD, "LuaBuddy", [1.0; 4])?;
        painter.text(
            Font::Small,
            footer_left,
            h - PAD - kit::control_height() + 5.0,
            &text,
            color,
        )
    }

    // ---- Input ----

    pub fn press(&mut self, context: &mut Context<'_, '_>, name: &str) -> Result<()> {
        // An open menu was built for the state before this press.
        self.close_menu(context)?;
        match name {
            "back" => self.close(context),
            "primary" => match (self.list.selected.clone(), self.primary.clone()) {
                (Some(Key::Header(section)), _) => self.toggle(context, section),
                (_, Some(node)) => self.run_node(context, node),
                _ => Ok(()),
            },
            "footer" => match self.footer.clone() {
                Some(node) => self.run_node(context, node),
                None => Ok(()),
            },
            "more" => {
                let nodes = self.other_actions();
                let at = self.above(context, &self.buttons.more.clone())?;
                self.show_menu(context, nodes, at)
            }
            "tools" => {
                let nodes = model::tools_menu(self.totals, self.live.world);
                let button = self.buttons.tools.clone();
                let x = kit::ask_number(context, &button, "getAbsoluteX")?;
                let y = kit::ask_number(context, &button, "getAbsoluteY")?;
                self.show_menu(context, nodes, [x, y + kit::control_height()])
            }
            "tab-mods" => self.show_tab(context, Tab::Mods),
            "tab-workshop" => self.show_tab(context, Tab::Workshop),
            "tab-activity" => self.show_tab(context, Tab::Activity),
            "source-subscribed" => self.show_source(context, Source::Subscribed),
            "source-everything" => self.show_source(context, Source::Everything),
            other => Err(format!("Unknown LuaBuddy control {other}")),
        }
    }

    fn above(&self, context: &mut Context<'_, '_>, button: &Value) -> Result<[f64; 2]> {
        let x = kit::ask_number(context, button, "getAbsoluteX")?;
        let y = kit::ask_number(context, button, "getAbsoluteY")?;
        Ok([x, y - 8.0 * kit::control_height()])
    }

    fn show_source(&mut self, context: &mut Context<'_, '_>, source: Source) -> Result<()> {
        // Reopening the shown source would drop the reads it has in progress.
        if source == self.workshop.source {
            return Ok(());
        }
        let text = self.search_text(context)?;
        self.chosen.remove(Tab::Workshop.name());
        if let Err(error) = self.workshop.open(source, &text) {
            self.workshop.problem = Some(error);
        }
        self.refresh(context)
    }

    pub fn list_press(&mut self, context: &mut Context<'_, '_>, y: f64) -> Result<()> {
        self.close_menu(context)?;
        match self.list.row_at(y) {
            Some(index) => self.choose(context, index),
            None => Ok(()),
        }
    }

    pub fn list_menu(&mut self, context: &mut Context<'_, '_>, y: f64) -> Result<()> {
        self.close_menu(context)?;
        let Some(index) = self.list.row_at(y) else {
            return Ok(());
        };
        if !matches!(self.list.rows[index].face, Face::Item { .. }) {
            return Ok(());
        }
        self.choose(context, index)?;
        let nodes = self.other_actions();
        let x = kit::first(context.call_global("getMouseX", &[])?);
        let y = kit::first(context.call_global("getMouseY", &[])?);
        let at = [context.numeric(&x)?, context.numeric(&y)?];
        self.show_menu(context, nodes, at)
    }

    fn choose(&mut self, context: &mut Context<'_, '_>, index: usize) -> Result<()> {
        let row = &self.list.rows[index];
        match (&row.key, &row.face) {
            (Key::Header(section), _) => {
                let section = *section;
                self.toggle(context, section)
            }
            (Key::More, _) => {
                self.workshop.more()?;
                self.refresh(context)
            }
            (_, Face::Item { .. }) => {
                kit::sound(context, "UISelectListItem");
                self.chosen.insert(self.tab.name(), row.key.clone());
                self.refresh(context)
            }
            _ => Ok(()),
        }
    }

    /// Moves the selection, stopping on section titles so the keyboard and a
    /// controller can open folded sections.
    pub fn step(&mut self, context: &mut Context<'_, '_>, forward: bool) -> Result<()> {
        self.close_menu(context)?;
        let Some(index) = self.list.neighbor(forward) else {
            return Ok(());
        };
        match &self.list.rows[index].face {
            Face::Item { .. } => self.choose(context, index)?,
            Face::Header { .. } => {
                self.chosen
                    .insert(self.tab.name(), self.list.rows[index].key.clone());
                self.refresh(context)?;
            }
            _ => (),
        }
        self.list.reveal(context, index)
    }

    /// Left and right switch the Workshop's source there, and scroll the page elsewhere.
    /// On the Workshop, a direction toward the other source switches to it and
    /// otherwise scrolls the page, so long pages stay reachable.
    pub fn sideways(&mut self, context: &mut Context<'_, '_>, right: bool) -> Result<()> {
        let source = if right {
            Source::Everything
        } else {
            Source::Subscribed
        };
        if self.tab == Tab::Workshop && self.workshop.source != source {
            return self.show_source(context, source);
        }
        self.page_text
            .wheel(context, if right { 2.0 } else { -2.0 })
    }

    fn running(&self) -> BTreeSet<ModId> {
        self.live
            .statuses
            .iter()
            .filter(|(_, state)| state.is_loaded())
            .map(|(id, _)| id.clone())
            .collect()
    }

    fn is_folded(&self, section: Section) -> bool {
        self.folded
            .get(&section)
            .copied()
            .unwrap_or_else(|| section.starts_folded())
    }

    fn toggle(&mut self, context: &mut Context<'_, '_>, section: Section) -> Result<()> {
        if !self.filter().is_empty() {
            return Ok(());
        }
        let folded = self.is_folded(section);
        self.folded.insert(section, !folded);
        self.refresh(context)
    }

    pub fn key(&mut self, context: &mut Context<'_, '_>, key: f64) -> Result<()> {
        let pressed = |context: &mut Context<'_, '_>, name: &str| -> Result<bool> {
            let keyboard = context.global("Keyboard")?;
            let code = context.get(&keyboard, name)?;
            Ok(!code.is_nil() && context.numeric(&code)? == key)
        };
        if pressed(context, "KEY_ESCAPE")? {
            if self.menu.is_some() {
                return self.close_menu(context);
            }
            return self.close(context);
        }
        if pressed(context, "KEY_UP")? {
            return self.step(context, false);
        }
        if pressed(context, "KEY_DOWN")? {
            return self.step(context, true);
        }
        if pressed(context, "KEY_RETURN")? {
            return self.press(context, "primary");
        }
        Ok(())
    }

    /// Escape first leaves the search box; pressing it again closes the window.
    pub fn search_key(&mut self, context: &mut Context<'_, '_>, key: f64) -> Result<()> {
        let keyboard = context.global("Keyboard")?;
        let escape = context.get(&keyboard, "KEY_ESCAPE")?;
        if context.numeric(&escape)? == key {
            context.invoke(&self.search, "unfocus", &[])?;
            // Otherwise the key's release reaches the window and closes it.
            let keyboard = context.global("GameKeyboard")?;
            let eat = context.get(&keyboard, "eatKeyPress")?;
            let key = context.number(key)?;
            context.call(&eat, &[key])?;
        }
        Ok(())
    }

    fn take_joypad(&mut self, context: &mut Context<'_, '_>) -> Result<()> {
        let state = context.global("JoypadState")?;
        if state.is_nil() {
            return Ok(());
        }
        let getter = context.get(&state, "getMainMenuJoypad")?;
        let data = kit::first(context.call(&getter, &[])?);
        if data.is_nil() {
            return Ok(());
        }
        let previous = context.get(&data, "focus")?;
        context.set(&self.root, "prevFocus", &previous)?;
        let root = self.root.clone();
        focus_joypad(context, &data, &root)?;
        self.joypad = data;
        let joypad = context.global("Joypad")?;
        let textures = context.get(&joypad, "Texture")?;
        for (button, texture) in [
            (&self.buttons.primary, "AButton"),
            (&self.buttons.back, "BButton"),
            (&self.buttons.more, "XButton"),
            (&self.buttons.tools, "YButton"),
        ] {
            let texture = context.get(&textures, texture)?;
            context.invoke(button, "setJoypadButton", &[texture])?;
        }
        Ok(())
    }

    pub fn joypad(&mut self, context: &mut Context<'_, '_>, button: f64) -> Result<()> {
        let joypad = context.global("Joypad")?;
        let pressed = |context: &mut Context<'_, '_>, name: &str| -> Result<bool> {
            let code = context.get(&joypad, name)?;
            Ok(!code.is_nil() && context.numeric(&code)? == button)
        };
        if pressed(context, "AButton")? {
            self.press(context, "primary")
        } else if pressed(context, "BButton")? {
            self.close(context)
        } else if pressed(context, "XButton")? {
            if self.more_shown {
                self.press(context, "more")
            } else {
                Ok(())
            }
        } else if pressed(context, "YButton")? {
            self.press(context, "tools")
        } else if pressed(context, "LBumper")? || pressed(context, "RBumper")? {
            let forward = pressed(context, "RBumper")?;
            let tabs = self
                .buttons
                .tabs
                .iter()
                .map(|(tab, _)| *tab)
                .collect::<Vec<_>>();
            let index = tabs.iter().position(|tab| *tab == self.tab).unwrap_or(0);
            let next = (index + if forward { 1 } else { tabs.len() - 1 }) % tabs.len();
            self.show_tab(context, tabs[next])
        } else {
            Ok(())
        }
    }

    // ---- Menus ----

    fn show_menu(
        &mut self,
        context: &mut Context<'_, '_>,
        nodes: Vec<Node>,
        at: [f64; 2],
    ) -> Result<()> {
        self.close_menu(context)?;
        if nodes.is_empty() {
            return Ok(());
        }
        // The game moves a controller between submenus through player slots that
        // the main menu doesn't have, so a controller gets one flat menu.
        let nodes = if self.joypad.is_nil() {
            nodes
        } else {
            flatten(nodes)
        };
        let joypad = self.joypad.clone();
        // Owned before focusing, so a focus failure still closes it later.
        let menu = self
            .menu
            .insert(menu::open(context, self.session, &nodes, at)?);
        if !joypad.is_nil() {
            menu.take_joypad(context, &joypad)?;
        }
        Ok(())
    }

    fn close_menu(&mut self, context: &mut Context<'_, '_>) -> Result<()> {
        let Some(menu) = self.menu.take() else {
            return Ok(());
        };
        let closed = menu.close(context);
        if !self.joypad.is_nil() && !self.closed {
            let (joypad, root) = (self.joypad.clone(), self.root.clone());
            return closed.and(focus_joypad(context, &joypad, &root));
        }
        closed
    }

    pub fn menu_child(&self, number: usize) -> Value {
        self.menu
            .as_ref()
            .map(|menu| menu.child(number))
            .unwrap_or_default()
    }

    pub fn menu_topmost(&self, context: &mut Context<'_, '_>, x: f64, y: f64) -> Result<Value> {
        match &self.menu {
            Some(menu) => menu.topmost(context, x, y),
            None => Ok(Value::default()),
        }
    }

    pub fn pick(&mut self, context: &mut Context<'_, '_>, index: usize) -> Result<()> {
        let act = self.menu.as_ref().and_then(|menu| menu.act(index));
        self.close_menu(context)?;
        match act {
            Some(act) => self.run(context, act),
            None => Ok(()),
        }
    }

    fn run_node(&mut self, context: &mut Context<'_, '_>, node: Node) -> Result<()> {
        if let Some(reason) = node.blocked {
            self.say(Tone::Warning, reason);
            return Ok(());
        }
        match node.act {
            Some(act) => self.run(context, act),
            None => Ok(()),
        }
    }

    // ---- Actions ----

    fn run(&mut self, context: &mut Context<'_, '_>, act: Act) -> Result<()> {
        let subject = subject(&act);
        let before = self.describe(subject.as_ref());
        let intent = Intent::begin(
            "ui.action",
            match &subject {
                Some(id) => format!("Player chose {} for {}", act.name(), self.catalog.title(id)),
                None => format!("Player chose {}", act.name()),
            },
            subject.as_ref(),
            json!({
                "action": act.name(),
                "tab": self.tab.name(),
                "modBefore": before,
                "world": if act.changes_state() { observe::world(context) } else { Json::Null },
            }),
        );
        let act_changed = act.changes_state();
        let reads = self.reads;
        let result = self.perform(context, act);
        // Only a change can make what was read stale, and an action that read
        // again to report its result needs no second reading. Leaving the world
        // is the one action after which nothing here is current.
        if !self.closed && act_changed && self.reads == reads {
            self.reread(context)?;
        }
        let after = self.describe(subject.as_ref());
        match &result {
            Ok((tone, text)) => {
                let level = match tone {
                    Tone::Bad => Level::Error,
                    Tone::Warning => Level::Warn,
                    _ => Level::Info,
                };
                let world = if act_changed {
                    observe::world(context)
                } else {
                    Json::Null
                };
                intent.finish(
                    level,
                    text.clone(),
                    json!({ "modAfter": after, "world": world }),
                );
                self.say(*tone, text.clone());
            }
            Err(error) => {
                intent.finish(Level::Error, error.clone(), json!({ "modAfter": after }));
                self.say(Tone::Bad, first_line(error));
            }
        }
        // Navigation has shown its result already; the status line draws itself.
        if self.closed || !self.dirty {
            return Ok(());
        }
        self.refresh(context)
    }

    fn describe(&mut self, id: Option<&ModId>) -> Json {
        let Some(id) = id else {
            return Json::Null;
        };
        let view = self.view(id, false);
        json!({
            "state": format!("{:?}", view.state(self.live.world.place)),
            "selected": view.selected,
            "temporary": view.temporary,
            "queued": view.queued,
            "startsWithGame": view.next_startup,
            "nativeActive": view.native_active,
            "installed": view.installed,
            "runtime": self.live.statuses.get(id).map(|state| first_line(&state.lua()).to_owned()),
        })
    }

    fn perform(&mut self, context: &mut Context<'_, '_>, act: Act) -> Result<(Tone, String)> {
        match act {
            Act::TurnOn(id) => {
                let changed = self.turn_on(context, &id)?;
                Ok((
                    Tone::Good,
                    format!(
                        "{} is on{}. {}",
                        self.catalog.title(&id),
                        with_others(changed, &id),
                        if self.live.world.place == Place::Server {
                            "It loads the next time you join."
                        } else {
                            "It loads when you join a server."
                        }
                    ),
                ))
            }
            Act::LoadNow(id) => {
                self.turn_on(context, &id)?;
                let before = self.running();
                let outcome = runtime::apply(context, self.session, Operation::Refresh);
                self.reread(context)?;
                // Loading takes every waiting mod, so the result names them all.
                let others = self
                    .running()
                    .difference(&before)
                    .filter(|other| **other != id)
                    .map(|other| self.catalog.title(other))
                    .collect::<Vec<_>>();
                let also = if others.is_empty() {
                    String::new()
                } else {
                    format!(" Also loaded: {}.", others.join(", "))
                };
                let view = self.view(&id, true);
                let title = self.catalog.title(&id);
                let result = match view.runtime {
                    Runtime::Loaded => Ok((Tone::Good, format!("{title} is running."))),
                    Runtime::NeedsGameStart(_) => Ok((
                        Tone::Warning,
                        format!("{title} is on, but it has to start with the game."),
                    )),
                    Runtime::Failed(reason) => Ok((
                        Tone::Bad,
                        format!("{title} failed to load. {}", first_line(&reason)),
                    )),
                    Runtime::NotLoaded => match outcome {
                        Err(error) => Err(error),
                        Ok(summary) => Ok((Tone::Warning, format!("{title}: {summary}"))),
                    },
                };
                result
                    .map(|(tone, text)| (tone, text + &also))
                    .map_err(|error| error + &also)
            }
            Act::LoadWaiting => {
                let before = self.running();
                let outcome = runtime::apply(context, self.session, Operation::Refresh);
                self.reread(context)?;
                // The summary still names older problems, which don't make this
                // load a failure when it loaded something.
                match outcome {
                    Ok(summary) => Ok((Tone::Good, summary)),
                    Err(summary) if self.running().difference(&before).next().is_some() => {
                        Ok((Tone::Warning, summary))
                    }
                    Err(error) => Err(error),
                }
            }
            Act::TurnOff(ids) => {
                self.require_no_pending_options(context)?;
                self.catalog.finish(context);
                let changes = profile_store::select(context, &ids, false)?;
                record_changes(&changes, false);
                let running = ids
                    .iter()
                    .any(|id| self.live.statuses.get(id).is_some_and(RunState::is_loaded));
                let title = self.catalog.title(&ids[0]);
                Ok((
                    Tone::Normal,
                    format!(
                        "{title} is off{}.{}",
                        match ids.len() {
                            1 => String::new(),
                            count => format!(
                                ", with {}",
                                model::plural(count - 1, "mod that needs it", "mods that need it")
                            ),
                        },
                        if running {
                            " What is already running stays until you reconnect."
                        } else {
                            ""
                        }
                    ),
                ))
            }
            Act::StartWithGame(id) => {
                self.require_no_pending_options(context)?;
                let selection =
                    activation::select_next_startup(context, std::slice::from_ref(&id), true)?;
                record_startup(&selection);
                Ok((
                    Tone::Good,
                    format!(
                        "{} starts with the game next launch{}.",
                        self.catalog.title(&id),
                        with_others(selection.changed.len(), &id)
                    ),
                ))
            }
            Act::DontStartWithGame(id) => {
                self.require_no_pending_options(context)?;
                let selection =
                    activation::select_next_startup(context, std::slice::from_ref(&id), false)?;
                record_startup(&selection);
                Ok((
                    Tone::Normal,
                    format!(
                        "{} won't start with the game next launch.",
                        self.catalog.title(&id)
                    ),
                ))
            }
            Act::Reload(id) => {
                let summary = runtime::apply(context, self.session, Operation::Reload(id.clone()))?;
                log::debug("reload.summary", summary).about(&id).write();
                Ok((
                    Tone::Good,
                    format!("{} was reloaded.", self.catalog.title(&id)),
                ))
            }
            Act::Rescan => {
                let before = self.catalog.mods.len();
                self.rescan_catalog(context)?;
                let now = self.catalog.mods.len();
                Ok((
                    Tone::Normal,
                    if now > before {
                        format!(
                            "Found {}.",
                            model::plural(now - before, "new mod", "new mods")
                        )
                    } else {
                        format!("{} installed.", model::plural(now, "mod", "mods"))
                    },
                ))
            }
            Act::ShowMod(id) => {
                self.texts.insert(Tab::Mods.name(), String::new());
                self.chosen.insert(Tab::Mods.name(), Key::Mod(id.clone()));
                let place = self.live.world.place;
                let section = self.view(&id, false).section(place);
                self.folded.insert(section, false);
                self.go_to_mods(context)?;
                if let Some(index) = self.list.index_of(&Key::Mod(id.clone())) {
                    self.list.reveal(context, index)?;
                }
                Ok((Tone::Muted, self.catalog.title(&id)))
            }
            Act::ShowContained(workshop) => {
                self.texts.insert(Tab::Mods.name(), workshop.to_string());
                if let Some((id, _)) = self.catalog.contained(workshop).into_iter().next() {
                    self.chosen.insert(Tab::Mods.name(), Key::Mod(id));
                }
                self.go_to_mods(context)?;
                Ok((Tone::Muted, "Showing what this item installed.".into()))
            }
            Act::Subscribe(id) => {
                self.workshop.subscribe(id)?;
                Ok((Tone::Waiting, "Subscribing...".into()))
            }
            Act::Update(id) => {
                self.workshop.download(id)?;
                Ok((Tone::Waiting, "Downloading...".into()))
            }
            Act::OpenPage(id) => open_page(context, id),
            Act::Copy(text, said) => {
                kit::set_clipboard(context, &text)?;
                Ok((Tone::Good, said.into()))
            }
            Act::OpenFolder(directory) => {
                open_folder(context, &directory)?;
                Ok((Tone::Normal, "Opened the folder on your desktop.".into()))
            }
            Act::OpenLogs => {
                let directory = log::directory(context)?;
                open_folder(context, &directory.to_string_lossy())?;
                Ok((
                    Tone::Normal,
                    "Opened the log folder on your desktop.".into(),
                ))
            }
            Act::ShowExtensions => {
                log::info("extensions.status", "Native extension status")
                    .with(crate::extensions::diagnostics())
                    .write();
                self.entries = log::recent(usize::MAX);
                if let Some(entry) = self.entries.first() {
                    self.chosen
                        .insert(Tab::Activity.name(), Key::Entry(entry.seq));
                }
                self.texts.insert(Tab::Activity.name(), String::new());
                self.show_tab(context, Tab::Activity)?;
                Ok((Tone::Normal, "Showing native extension status.".into()))
            }
            Act::RetryExtensions => {
                self.require_no_pending_options(context)?;
                runtime::retry_extensions(context, self.session)?;
                Ok((Tone::Good, "Every approved patch is installed.".into()))
            }
            Act::Quit => {
                // A server can hold players in the game for a while after combat.
                let cooldown = kit::first(context.call_global("isQuitCooldown", &[])?);
                if context.truth(&cooldown)? {
                    return Err(
                        "The server doesn't let you quit just yet. Try again in a moment.".into(),
                    );
                }
                let class = context.global("MainScreen")?;
                let main = if class.is_nil() {
                    Value::default()
                } else {
                    context.get(&class, "instance")?
                };
                if main.is_nil() {
                    return Err("Quit from the game's own menu.".into());
                }
                self.close(context)?;
                context.invoke(&main, "quitToDesktopFunc", &[])?;
                Ok((Tone::Normal, "Asked the game to quit.".into()))
            }
        }
    }

    fn go_to_mods(&mut self, context: &mut Context<'_, '_>) -> Result<()> {
        if self.tab == Tab::Mods {
            let text = self
                .texts
                .get(Tab::Mods.name())
                .cloned()
                .unwrap_or_default();
            let value = context.string(&text)?;
            context.invoke(&self.search, "setText", &[value])?;
            self.refresh(context)
        } else {
            self.show_tab(context, Tab::Mods)
        }
    }

    fn turn_on(&mut self, context: &mut Context<'_, '_>, id: &ModId) -> Result<usize> {
        self.require_no_pending_options(context)?;
        self.catalog.finish(context);
        let ids = self
            .catalog
            .with_requirements(id, &self.live.effective, &self.live.active)?;
        let changes = profile_store::select(context, &ids, true)?;
        record_changes(&changes, true);
        Ok(changes.len())
    }

    fn require_no_pending_options(&self, context: &mut Context<'_, '_>) -> Result<()> {
        if super::pending_options(context)? {
            Err("Apply or cancel your pending game Options changes first.".into())
        } else {
            Ok(())
        }
    }
}

impl Live {
    fn read(context: &mut Context<'_, '_>) -> Result<Self> {
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
                options_pending: super::pending_options(context)?,
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

fn flatten(nodes: Vec<Node>) -> Vec<Node> {
    let mut flat = Vec::new();
    for node in nodes {
        if node.children.is_empty() {
            flat.push(node);
            continue;
        }
        for mut child in flatten(node.children) {
            child.label = format!("{}: {}", node.label, child.label);
            flat.push(child);
        }
    }
    flat
}

fn effective(profiles: &[Profile], templates: &[Profile]) -> Vec<Profile> {
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

fn extension_totals() -> Totals {
    let (extensions, extensions_pending) = crate::extensions::counts();
    Totals {
        extensions,
        extensions_pending,
        ..Totals::default()
    }
}

fn record_changes(changes: &[profile_store::Change], enabled: bool) {
    for change in changes {
        log::info(
            "selection.saved",
            if enabled {
                "Turned on for future joins"
            } else {
                "Turned off for future joins"
            },
        )
        .about(&change.id)
        .with(json!({ "file": change.file, "created": change.created, "enabled": enabled }))
        .write();
    }
}

fn record_startup(selection: &activation::StartupSelection) {
    log::info(
        "startup.saved",
        "Changed the game's own mod list for the next launch",
    )
    .with(json!({
        "enabled": selection.enabled,
        "changed": selection.changed,
        "selected": selection.selected.len(),
    }))
    .write();
}

fn subject(act: &Act) -> Option<ModId> {
    match act {
        Act::LoadNow(id)
        | Act::TurnOn(id)
        | Act::StartWithGame(id)
        | Act::DontStartWithGame(id)
        | Act::Reload(id)
        | Act::ShowMod(id) => Some(id.clone()),
        Act::TurnOff(ids) => ids.first().cloned(),
        _ => None,
    }
}

fn with_others(changed: usize, _: &ModId) -> String {
    match changed.saturating_sub(1) {
        0 => String::new(),
        others => format!(
            ", with {}",
            model::plural(others, "mod it needs", "mods it needs")
        ),
    }
}

fn open_page(context: &mut Context<'_, '_>, id: u64) -> Result<(Tone, String)> {
    let overlay = kit::first(context.call_global("isSteamOverlayEnabled", &[])?);
    if context.truth(&overlay)? {
        let item = context.string(&id.to_string())?;
        context.call_global("activateSteamOverlayToWorkshopItem", &[item])?;
        return Ok((Tone::Normal, "Opened its page in the Steam overlay.".into()));
    }
    let url = model::workshop_url(id);
    let opened = context.jni(|env| {
        let value = env.new_string(&url)?;
        env.call_static_method(
            "zombie/network/DesktopBrowser",
            "openURL",
            "(Ljava/lang/String;)Z",
            &[jni::objects::JValue::Object(&value)],
        )?
        .z()
    })?;
    if opened {
        return Ok((Tone::Normal, "Opened its page in your browser.".into()));
    }
    kit::set_clipboard(context, &url)?;
    Ok((
        Tone::Warning,
        "No browser could be opened. The link is copied, ready to paste.".into(),
    ))
}

fn open_folder(context: &mut Context<'_, '_>, directory: &str) -> Result<()> {
    let value = context.string(directory)?;
    context.call_global("showFolderInDesktop", &[value])?;
    Ok(())
}

fn containing_folder(path: &str) -> String {
    let path = std::path::Path::new(path);
    if path.is_dir() {
        path.to_string_lossy().into_owned()
    } else {
        path.parent().unwrap_or(path).to_string_lossy().into_owned()
    }
}

fn first_line(text: &str) -> &str {
    text.lines().next().unwrap_or(text)
}

fn note(text: &str) -> Row<Key> {
    Row {
        key: Key::Note,
        text: text.to_owned(),
        face: Face::Note,
    }
}

fn header_height() -> f64 {
    kit::line_height(Font::Large).max(kit::control_height())
}

fn frame_size((width, height): (f64, f64)) -> (f64, f64) {
    let part = if width <= 1280.0 { 0.96 } else { 0.86 };
    (
        (width * part).min(1800.0).floor(),
        (height * 0.92).min(1200.0).floor(),
    )
}

fn focus_joypad(context: &mut Context<'_, '_>, joypad: &Value, target: &Value) -> Result<()> {
    context.set(joypad, "focus", target)?;
    context.call_global("updateJoypadFocus", std::slice::from_ref(joypad))?;
    Ok(())
}

fn mark(context: &mut Context<'_, '_>, button: &Value, active: bool) -> Result<()> {
    let [r, g, b, _] = if active { kit::SELECTED } else { [0.0; 4] };
    kit::set_color(
        context,
        button,
        "backgroundColor",
        [r, g, b, if active { 0.9 } else { 0.0 }],
    )?;
    kit::set_color(
        context,
        button,
        "borderColor",
        if active { kit::WARNING } else { kit::BORDER },
    )
}

/// Shows the button for an action, or hides it. Returns the space it takes.
fn place_action(
    context: &mut Context<'_, '_>,
    button: &Value,
    node: Option<&Node>,
    [x, y]: [f64; 2],
    limit: f64,
) -> Result<f64> {
    kit::call_flag(context, button, "setVisible", node.is_some())?;
    let Some(node) = node else {
        return Ok(0.0);
    };
    let label = kit::fit(context, Font::Small, &node.label, limit - 28.0)?;
    let title = context.string(&label)?;
    context.invoke(button, "setTitle", &[title])?;
    let width = kit::width(context, Font::Small, &label)? + 28.0;
    kit::set_bounds(context, button, [x, y, width, kit::control_height()])?;
    let (fill, border) = if matches!(node.act, Some(Act::TurnOff(_) | Act::Quit | Act::Copy(..))) {
        ([0.0; 4], kit::BORDER)
    } else {
        ([0.10, 0.30, 0.12, 0.9], [0.30, 0.75, 0.35, 1.0])
    };
    // setEnable puts back the colors it last saved as the enabled ones.
    for (field, color) in [
        ("backgroundColor", fill),
        ("backgroundColorEnabled", fill),
        ("borderColor", border),
        ("borderColorEnabled", border),
    ] {
        kit::set_color(context, button, field, color)?;
    }
    kit::call_flag(context, button, "setEnable", node.blocked.is_none())?;
    let tooltip = match node.blocked.as_ref().or(node.hint.as_ref()) {
        Some(text) => context.string(text)?,
        None => Value::default(),
    };
    context.invoke(button, "setTooltip", &[tooltip])?;
    Ok(width + PAD)
}

type Built = (Value, Value, Value, Value, Value, Buttons);

fn build(
    context: &mut Context<'_, '_>,
    session: u64,
    size: (f64, f64),
    steam: bool,
) -> Result<Built> {
    let root = panel(context, [0.0, 0.0, size.0, size.1])?;
    kit::initialise(context, &root)?;
    for (method, event) in [
        ("prerender", Event::Draw),
        ("update", Event::Update),
        ("onKeyRelease", Event::KeyRelease),
        ("isKeyConsumed", Event::KeyConsumed),
        ("onJoypadDown", Event::JoypadDown),
        ("onJoypadDirUp", Event::JoypadPrevious),
        ("onJoypadDirDown", Event::JoypadNext),
        ("onJoypadDirLeft", Event::JoypadLeft),
        ("onJoypadDirRight", Event::JoypadRight),
        // Clicks beside the window must not reach the game's menu behind it.
        ("onMouseDown", Event::Swallow),
        ("onMouseUp", Event::Swallow),
        ("onRightMouseDown", Event::Swallow),
        ("onRightMouseUp", Event::Swallow),
        ("onMouseWheel", Event::Swallow),
    ] {
        set_callback(context, session, &root, method, event)?;
    }
    let (w, h) = frame_size(size);
    let frame = panel(
        context,
        [
            ((size.0 - w) / 2.0).floor(),
            ((size.1 - h) / 2.0).floor(),
            w,
            h,
        ],
    )?;
    kit::add(context, &root, &frame)?;
    let click = callback(context, session, Event::Button)?;
    let height = kit::control_height();
    let header = PAD + (header_height() - height) / 2.0;
    let mut x = PAD + kit::width(context, Font::Large, "LuaBuddy")? + 28.0;
    let mut tabs = Vec::new();
    for (tab, name, label, shown) in [
        (Tab::Mods, "tab-mods", "My mods", true),
        (Tab::Workshop, "tab-workshop", "Workshop", steam),
        (Tab::Activity, "tab-activity", "Activity", true),
    ] {
        if shown {
            let button = button(context, &frame, &click, name, label, [x, header])?;
            x += kit::ask_number(context, &button, "getWidth")? + 6.0;
            tabs.push((tab, button));
        }
    }
    let tools = button(context, &frame, &click, "tools", "Tools", [0.0, header])?;
    let width = kit::ask_number(context, &tools, "getWidth")?;
    let right = context.number(w - PAD - width)?;
    context.invoke(&tools, "setX", &[right])?;
    let back = button(
        context,
        &frame,
        &click,
        "back",
        "Close",
        [PAD, h - PAD - height],
    )?;
    context.invoke(&back, "enableCancelColor", &[])?;
    let primary = button(context, &frame, &click, "primary", "", [PAD, 0.0])?;
    let more = button(context, &frame, &click, "more", "More...", [PAD, 0.0])?;
    let footer = button(context, &frame, &click, "footer", "", [PAD, 0.0])?;
    let top = PAD + header_height() + PAD;
    let list_width = (w * 0.40).clamp(260.0, 640.0);
    let mut args = vec![context.string("")?];
    let entry = kit::line_height(Font::Small) + 6.0;
    args.extend(kit::numbers(
        context,
        &[PAD, top + height - entry, list_width, entry],
    )?);
    let search = kit::construct(context, "ISTextEntryBox", &args)?;
    kit::add(context, &frame, &search)?;
    kit::call_flag(context, &search, "setClearButton", true)?;
    for (method, event) in [
        ("onOtherKey", Event::SearchKey),
        ("onPressUp", Event::SearchUp),
        ("onPressDown", Event::SearchDown),
    ] {
        set_callback(context, session, &search, method, event)?;
    }
    let mut x = PAD;
    let mut sources = Vec::new();
    for (source, name, label) in [
        (Source::Subscribed, "source-subscribed", "Subscribed"),
        (
            Source::Everything,
            "source-everything",
            "All of the Workshop",
        ),
    ] {
        let button = button(
            context,
            &frame,
            &click,
            name,
            label,
            [x, top + height + 6.0],
        )?;
        x += kit::ask_number(context, &button, "getWidth")? + 6.0;
        sources.push((source, button));
    }
    let list = surface(context, session, &frame)?;
    for (method, event) in [
        ("prerender", Event::ListDraw),
        ("onMouseDown", Event::ListPress),
        ("onMouseDoubleClick", Event::ListPress),
        ("onRightMouseUp", Event::ListMenu),
        ("onMouseWheel", Event::ListWheel),
        ("onMouseMove", Event::ListMove),
        ("onMouseMoveOutside", Event::ListLeave),
    ] {
        set_callback(context, session, &list, method, event)?;
    }
    let page = surface(context, session, &frame)?;
    for (method, event) in [
        ("prerender", Event::PageDraw),
        ("onMouseWheel", Event::PageWheel),
    ] {
        set_callback(context, session, &page, method, event)?;
    }
    Ok((
        root,
        frame,
        search,
        list,
        page,
        Buttons {
            back,
            primary,
            more,
            tools,
            footer,
            tabs,
            sources,
        },
    ))
}

fn panel(context: &mut Context<'_, '_>, bounds: [f64; 4]) -> Result<Value> {
    let args = kit::numbers(context, &bounds)?;
    let panel = kit::construct(context, "ISPanelJoypad", &args)?;
    kit::set_flag(context, &panel, "background", false)?;
    Ok(panel)
}

fn surface(context: &mut Context<'_, '_>, _session: u64, frame: &Value) -> Result<Value> {
    let surface = panel(context, [0.0, 0.0, 100.0, 100.0])?;
    kit::add(context, frame, &surface)?;
    context.invoke(&surface, "addScrollBars", &[])?;
    Ok(surface)
}

fn button(
    context: &mut Context<'_, '_>,
    frame: &Value,
    click: &Value,
    name: &str,
    label: &str,
    [x, y]: [f64; 2],
) -> Result<Value> {
    let width = kit::width(context, Font::Small, label)? + 28.0;
    let mut args = kit::numbers(context, &[x, y, width, kit::control_height()])?;
    args.push(context.string(label)?);
    args.push(frame.clone());
    args.push(click.clone());
    let button = kit::construct(context, "ISButton", &args)?;
    kit::set_text(context, &button, "internal", name)?;
    kit::set_color(context, &button, "borderColor", kit::BORDER)?;
    kit::add(context, frame, &button)?;
    Ok(button)
}
