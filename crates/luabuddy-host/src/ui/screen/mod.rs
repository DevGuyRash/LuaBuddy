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

mod actions;
mod live;
mod widgets;

use actions::*;
pub use live::remember_launch_startup;
use live::*;
use widgets::*;

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

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum Tab {
    Mods,
    Workshop,
    Activity,
}

impl Tab {
    const ALL: [Self; 3] = [Self::Mods, Self::Workshop, Self::Activity];

    /// How the journal names the tab.
    fn name(self) -> &'static str {
        match self {
            Self::Mods => "mods",
            Self::Workshop => "workshop",
            Self::Activity => "activity",
        }
    }

    /// The name the game hands back when its tab button is pressed.
    fn control(self) -> &'static str {
        match self {
            Self::Mods => "tab-mods",
            Self::Workshop => "tab-workshop",
            Self::Activity => "tab-activity",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Mods => "My mods",
            Self::Workshop => "Workshop",
            Self::Activity => "Activity",
        }
    }

    fn placeholder(self) -> &'static str {
        match self {
            Self::Mods => "Filter your mods",
            Self::Workshop => "Search",
            Self::Activity => "Search activity",
        }
    }
}

/// Window state kept when the window is rebuilt for a new screen size.
pub struct Carried {
    tab: Tab,
    /// Its source, results and any request or download still in progress.
    workshop: Workshop,
    texts: BTreeMap<Tab, String>,
    chosen: BTreeMap<Tab, Key>,
    folded: BTreeMap<Section, bool>,
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
    texts: BTreeMap<Tab, String>,
    chosen: BTreeMap<Tab, Key>,
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
            let text = context.string(screen.text(screen.tab))?;
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
                .and_then(|previous| kit::focus_joypad(context, &joypad, &previous));
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
        if self.texts.get(&self.tab) != Some(&text) {
            self.close_menu(context)?;
            self.texts.insert(self.tab, text.clone());
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

    /// What the player typed in a tab's search box.
    fn text(&self, tab: Tab) -> &str {
        self.texts.get(&tab).map(String::as_str).unwrap_or_default()
    }

    fn filter(&self) -> String {
        self.text(self.tab).trim().to_lowercase()
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
        let mut view = self
            .workshop
            .items(self.text(Tab::Workshop))
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
        let (mut running, mut attention, mut waiting, mut ready) = (0, 0, 0, 0);
        let mut mods = Vec::new();
        let listed = self.listed();
        for id in &listed {
            let view = self.view(id, false);
            match view.section(place) {
                Section::Running => running += 1,
                Section::Attention => attention += 1,
                // Waiting counts toward the summary; Ready is also what
                // loading would take now, which only happens on a server.
                _ => match view.state(place) {
                    State::Ready => {
                        waiting += 1;
                        ready += 1;
                    }
                    State::Waiting => waiting += 1,
                    _ => (),
                },
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
            let shown = self.workshop.items(self.text(Tab::Workshop)).len();
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
            .get(&self.tab)
            .filter(|key| self.list.index_of(key).is_some())
            .cloned();
        self.totals = Totals {
            waiting: ready,
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
        let unreadable = self.live.unreadable.iter().map(|(file, _)| {
            (
                Section::Attention,
                file.to_string_lossy().into_owned(),
                Unread::Selection,
            )
        });
        let broken = self.catalog.broken.iter().map(|broken| {
            (
                Section::Unavailable,
                broken.directory.clone(),
                Unread::Folder,
            )
        });
        for (section, path, kind) in unreadable.chain(broken) {
            let name = last_component(&path).to_owned();
            if filter.is_empty() || name.to_lowercase().contains(&filter) {
                sections.entry(section).or_default().push(Row {
                    key: Key::Folder(path),
                    text: name,
                    face: Face::Item {
                        status: kind.status().into(),
                        tone: kind.tone(),
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
        let items = self.workshop.items(self.text(Tab::Workshop));
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
                let (kind, reason) = match selection {
                    Some((_, reason)) => (Unread::Selection, reason.clone()),
                    None => (
                        Unread::Folder,
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
                    Block::Title(last_component(&directory).to_owned()),
                    Block::Status(kind.tone(), kind.status().into()),
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
                Block::Title(Tab::Mods.label().into()),
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
                Block::Title(Tab::Workshop.label().into()),
                Block::Text(
                    Tone::Normal,
                    "Choose an item to see it. Downloading one only delivers its files; turn its mods on in My mods."
                        .into(),
                ),
            ],
            Tab::Activity => vec![
                Block::Title(Tab::Activity.label().into()),
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
        let top = content_top();
        let bottom = if has_menu { y - PAD } else { list_y + list_h };
        let width = self.frame_size().0 - PAD - left;
        self.page_text
            .resize(context, &self.theme, [left, top, width, bottom - top])?;
        let mut x = left;
        let primary = self.primary.clone();
        let more = button_width(context, MORE)?;
        x += place_action(
            context,
            &self.buttons.primary,
            primary.as_ref(),
            [x, y],
            width - more - PAD,
        )?;
        self.more_shown = !self.other_actions().is_empty();
        kit::call_flag(context, &self.buttons.more, "setVisible", self.more_shown)?;
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
            .min(limit - BUTTON_INSET)
            + BUTTON_INSET;
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

    /// What pressing the main button would do. Hints may change without changing this.
    fn press_meaning(&self) -> PressMeaning {
        let primary = self.primary.as_ref();
        PressMeaning {
            row: self.list.selected.clone(),
            act: primary.and_then(|node| node.act.clone()),
            // A button with no action of its own, such as Show or Hide, is its label.
            label: primary
                .filter(|node| node.act.is_none())
                .map(|node| node.label.clone()),
            available: primary.map(|node| node.blocked.is_none()),
        }
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

    fn list_bounds(&self) -> [f64; 4] {
        let (w, h) = self.frame_size();
        let rows = if self.tab == Tab::Workshop { 2.0 } else { 1.0 };
        let top = content_top() + rows * (kit::control_height() + ROW_GAP);
        let bottom = h - 2.0 * PAD - kit::control_height();
        [PAD, top, list_width(w), bottom - top]
    }

    fn actions_top(&self) -> f64 {
        let [_, y, _, h] = self.list_bounds();
        y + h - kit::control_height()
    }

    fn lay_out(&mut self, context: &mut Context<'_, '_>) -> Result<()> {
        let list = self.list_bounds();
        panes::place(context, &self.list.panel, list)?;
        let placeholder = context.string(self.tab.placeholder())?;
        context.invoke(&self.search, "setPlaceholderText", &[placeholder])?;
        Ok(())
    }

    fn show_tab(&mut self, context: &mut Context<'_, '_>, tab: Tab) -> Result<()> {
        if tab == self.tab {
            return Ok(());
        }
        self.close_menu(context)?;
        self.tab = tab;
        let text = self.text(tab).to_owned();
        let value = context.string(&text)?;
        context.invoke(&self.search, "setText", &[value])?;
        self.texts.insert(tab, text.clone());
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
            "primary" => {
                // Steam can change an item between frames. Show what holds now,
                // and act only if the button still means what the player saw:
                // the same row, the same action, just as available.
                let seen = self.press_meaning();
                self.refresh(context)?;
                if self.press_meaning() != seen {
                    self.say(
                        Tone::Muted,
                        "This changed just now. Look again, then press.",
                    );
                    return Ok(());
                }
                match (self.list.selected.clone(), self.primary.clone()) {
                    (Some(Key::Header(section)), _) => self.toggle(context, section),
                    (_, Some(node)) => self.run_node(context, node),
                    _ => Ok(()),
                }
            }
            "footer" => match self.footer.clone() {
                Some(node) => self.run_node(context, node),
                None => Ok(()),
            },
            "more" => {
                self.refresh(context)?;
                let nodes = self.other_actions();
                let at = self.above(context, &self.buttons.more.clone())?;
                self.show_menu(context, nodes, at)
            }
            "tools" => {
                // Patches can change state while the window sits idle.
                let totals = Totals {
                    waiting: self.totals.waiting,
                    restart: self.totals.restart,
                    ..extension_totals()
                };
                let nodes = model::tools_menu(totals, self.live.world);
                let button = self.buttons.tools.clone();
                let x = kit::ask_number(context, &button, "getAbsoluteX")?;
                let y = kit::ask_number(context, &button, "getAbsoluteY")?;
                self.show_menu(context, nodes, [x, y + kit::control_height()])
            }
            "source-subscribed" => self.show_source(context, Source::Subscribed),
            "source-everything" => self.show_source(context, Source::Everything),
            other => match Tab::ALL.into_iter().find(|tab| tab.control() == other) {
                Some(tab) => self.show_tab(context, tab),
                None => Err(format!("Unknown LuaBuddy control {other}")),
            },
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
        self.chosen.remove(&Tab::Workshop);
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
                self.chosen.insert(self.tab, row.key.clone());
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
                    .insert(self.tab, self.list.rows[index].key.clone());
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
        kit::focus_joypad(context, &data, &root)?;
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
            return closed.and(kit::focus_joypad(context, &joypad, &root));
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

use kit::first_line;

/// The row, action and availability behind the main button.
#[derive(PartialEq)]
struct PressMeaning {
    row: Option<Key>,
    act: Option<Act>,
    label: Option<String>,
    available: Option<bool>,
}

/// Something listed that could not be read.
#[derive(Clone, Copy)]
enum Unread {
    /// A selection file: the player's own choice is in doubt.
    Selection,
    /// A mod folder the game itself can't read.
    Folder,
}

impl Unread {
    fn status(self) -> &'static str {
        match self {
            Self::Selection => "This selection file can't be read",
            Self::Folder => "The game can't read this folder",
        }
    }

    fn tone(self) -> Tone {
        match self {
            Self::Selection => Tone::Bad,
            Self::Folder => Tone::Warning,
        }
    }
}

/// The last part of a path, whichever separator it uses.
fn last_component(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

fn note(text: &str) -> Row<Key> {
    Row {
        key: Key::Note,
        text: text.to_owned(),
        face: Face::Note,
    }
}
