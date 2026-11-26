//! Carrying out what the player chose, and recording it in the journal.

use super::*;

impl Screen {
    pub(super) fn run(&mut self, context: &mut Context<'_, '_>, act: Act) -> Result<()> {
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

    pub(super) fn describe(&mut self, id: Option<&ModId>) -> Json {
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

    pub(super) fn perform(
        &mut self,
        context: &mut Context<'_, '_>,
        act: Act,
    ) -> Result<(Tone, String)> {
        match act {
            Act::TurnOn(id) => {
                let changed = self.turn_on(context, &id)?;
                Ok((
                    Tone::Good,
                    format!(
                        "{} is on{}. {}",
                        self.catalog.title(&id),
                        with_others(changed),
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
                        with_others(selection.changed.len())
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
                // Selections can change outside the window too, by the command line.
                self.reread(context)?;
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
                self.texts.insert(Tab::Mods, String::new());
                self.chosen.insert(Tab::Mods, Key::Mod(id.clone()));
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
                self.texts.insert(Tab::Mods, workshop.to_string());
                if let Some((id, _)) = self.catalog.contained(workshop).into_iter().next() {
                    self.chosen.insert(Tab::Mods, Key::Mod(id));
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
                self.journal_revision = Some(log::revision());
                self.entries = log::recent(usize::MAX);
                if let Some(entry) = self.entries.first() {
                    self.chosen.insert(Tab::Activity, Key::Entry(entry.seq));
                }
                self.texts.insert(Tab::Activity, String::new());
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
                let Some(main) = entry::main_screen(context)? else {
                    return Err("Quit from the game's own menu.".into());
                };
                self.close(context)?;
                context.invoke(&main, "quitToDesktopFunc", &[])?;
                Ok((Tone::Normal, "Asked the game to quit.".into()))
            }
        }
    }

    pub(super) fn go_to_mods(&mut self, context: &mut Context<'_, '_>) -> Result<()> {
        if self.tab == Tab::Mods {
            let value = context.string(self.text(Tab::Mods))?;
            context.invoke(&self.search, "setText", &[value])?;
            self.refresh(context)
        } else {
            self.show_tab(context, Tab::Mods)
        }
    }

    pub(super) fn turn_on(&mut self, context: &mut Context<'_, '_>, id: &ModId) -> Result<usize> {
        self.require_no_pending_options(context)?;
        self.catalog.finish(context);
        let ids = self
            .catalog
            .with_requirements(id, &self.live.effective, &self.live.active)?;
        let changes = profile_store::select(context, &ids, true)?;
        record_changes(&changes, true);
        Ok(changes.len())
    }

    pub(super) fn require_no_pending_options(&self, context: &mut Context<'_, '_>) -> Result<()> {
        if crate::ui::pending_options(context)? {
            Err(model::OPTIONS_PENDING.into())
        } else {
            Ok(())
        }
    }
}

pub(super) fn record_changes(changes: &[profile_store::Change], enabled: bool) {
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

pub(super) fn record_startup(selection: &activation::StartupSelection) {
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

pub(super) fn subject(act: &Act) -> Option<ModId> {
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

pub(super) fn with_others(changed: usize) -> String {
    match changed.saturating_sub(1) {
        0 => String::new(),
        others => format!(
            ", with {}",
            model::plural(others, "mod it needs", "mods it needs")
        ),
    }
}

pub(super) fn open_page(context: &mut Context<'_, '_>, id: u64) -> Result<(Tone, String)> {
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

pub(super) fn open_folder(context: &mut Context<'_, '_>, directory: &str) -> Result<()> {
    let value = context.string(directory)?;
    context.call_global("showFolderInDesktop", &[value])?;
    Ok(())
}

pub(super) fn containing_folder(path: &str) -> String {
    let path = std::path::Path::new(path);
    if path.is_dir() {
        path.to_string_lossy().into_owned()
    } else {
        path.parent().unwrap_or(path).to_string_lossy().into_owned()
    }
}
