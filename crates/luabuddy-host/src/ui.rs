use crate::{
    callables, game,
    lua::{Context, Result, Value},
    runtime, steam,
};
use jni::{
    JNIEnv,
    objects::{JObject, JObjectArray, JValue},
};
use luabuddy_core::lifecycle::Operation;
use luabuddy_core::{ModId, Profile, read_profiles};
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    rc::Rc,
    time::{Duration, Instant},
};

const TIMEOUT: Duration = Duration::from_secs(45);
const UI_OWNER: &str = "LuaBuddy.UI";

#[derive(Clone, Copy)]
#[repr(u8)]
enum Action {
    Open = 1,
    Refresh,
    Reload,
    Search,
    Installed,
    Subscribed,
    Select,
    Add,
    Download,
    Enable,
    Disable,
    Load,
    Cancel,
    Close,
    Key,
    Destroy,
    ReloadSubmit,
    ReloadEnter,
    Restart,
    Consumed,
    Previous,
    Next,
    EntryKey,
    Update,
    StartupEnable,
    StartupDisable,
    PatchDiagnostics,
    ApplyApprovedPatches,
    SearchSubmit,
    ReloadSelect,
    Help,
    JoypadGain,
    JoypadLose,
    JoypadDown,
    JoypadDeactivate,
    OpenWorkshop,
    ModalPrerender,
    ReloadJoypadGain,
    ReloadJoypadLose,
}

impl Action {
    fn from(value: u8) -> Option<Self> {
        Some(match value {
            1 => Self::Open,
            2 => Self::Refresh,
            3 => Self::Reload,
            4 => Self::Search,
            5 => Self::Installed,
            6 => Self::Subscribed,
            7 => Self::Select,
            8 => Self::Add,
            9 => Self::Download,
            10 => Self::Enable,
            11 => Self::Disable,
            12 => Self::Load,
            13 => Self::Cancel,
            14 => Self::Close,
            15 => Self::Key,
            16 => Self::Destroy,
            17 => Self::ReloadSubmit,
            18 => Self::ReloadEnter,
            19 => Self::Restart,
            20 => Self::Consumed,
            21 => Self::Previous,
            22 => Self::Next,
            23 => Self::EntryKey,
            24 => Self::Update,
            25 => Self::StartupEnable,
            26 => Self::StartupDisable,
            27 => Self::PatchDiagnostics,
            28 => Self::ApplyApprovedPatches,
            29 => Self::SearchSubmit,
            30 => Self::ReloadSelect,
            31 => Self::Help,
            32 => Self::JoypadGain,
            33 => Self::JoypadLose,
            34 => Self::JoypadDown,
            35 => Self::JoypadDeactivate,
            36 => Self::OpenWorkshop,
            37 => Self::ModalPrerender,
            38 => Self::ReloadJoypadGain,
            39 => Self::ReloadJoypadLose,
            _ => return None,
        })
    }
}

#[derive(Clone)]
struct Item {
    id: Option<ModId>,
    workshop: Option<steam::ItemId>,
    title: String,
    description: String,
    required: Vec<ModId>,
    workshop_dependencies: Vec<u64>,
    source: String,
    available: bool,
    restart_assets: bool,
    activation_explanation: String,
    native_startup_available: bool,
    mod_version: Option<String>,
    minimum_game_version: Option<String>,
    maximum_game_version: Option<String>,
    workshop_updated_at: Option<u32>,
    size_bytes: Option<u64>,
    unavailable_reason: Option<String>,
}

enum Pending {
    Query(steam::Query),
    SubscriptionSearch(SubscriptionSearch),
    Subscribe(steam::Subscription),
    Download(steam::Download),
}

struct SubscriptionSearch {
    query: steam::Query,
    ids: Vec<steam::ItemId>,
    next: usize,
    matches: Vec<Item>,
    filter: String,
}

#[derive(Clone)]
enum BrowseMode {
    Installed,
    Search {
        text: String,
        page: u32,
    },
    Subscribed {
        items: Vec<steam::ItemId>,
        page: u32,
    },
    FilteredSubscriptions {
        items: Rc<Vec<Item>>,
        page: u32,
        filter: String,
    },
}

impl BrowseMode {
    fn search_action(&self) -> Action {
        match self {
            Self::Installed => Action::Installed,
            Self::Search { .. } => Action::Search,
            Self::Subscribed { .. } | Self::FilteredSubscriptions { .. } => Action::Subscribed,
        }
    }

    fn select_source(&mut self, action: Action, text: &str) {
        match action {
            Action::Installed => *self = Self::Installed,
            Action::Search => {
                *self = Self::Search {
                    text: text.into(),
                    page: 1,
                }
            }
            Action::Subscribed => {
                *self = Self::Subscribed {
                    items: Vec::new(),
                    page: 1,
                }
            }
            _ => (),
        }
    }
}

struct Browser {
    panel: Value,
    search: Value,
    list: Value,
    details: Value,
    status: Value,
    items: Vec<Item>,
    selected: usize,
    mode: BrowseMode,
    total_matching: u32,
    steam: std::result::Result<steam::SteamWorkshop, String>,
    pending: Option<Pending>,
    next_poll: Instant,
    buttons: Vec<(Action, Value)>,
    installed_catalog: Vec<Item>,
    search_input: String,
    search_changed: Option<Instant>,
}

struct Ui {
    session: u64,
    environment: Value,
    browser: Option<Browser>,
    next_focus_check: Instant,
}

thread_local! { static UI: RefCell<Option<Ui>> = const { RefCell::new(None) }; }

fn first(values: Vec<Value>) -> Value {
    values.into_iter().next().unwrap_or_default()
}

fn callback(context: &mut Context<'_, '_>, session: u64, action: Action) -> Result<Value> {
    let token = session
        .checked_mul(256)
        .and_then(|n| n.checked_add(action as u64))
        .ok_or("UI session identifier exhausted")?;
    callables::create(context.env, UI_OWNER, session, token, dispatch)
        .map(|reference| Value(Some(reference)))
}

pub fn install(context: &mut Context<'_, '_>, session: u64) -> Result<()> {
    UI.with(|state| {
        *state.borrow_mut() = Some(Ui {
            session,
            environment: context.environment.clone(),
            browser: None,
            next_focus_check: Instant::now(),
        })
    });
    let api = context.global("PZAPI")?;
    if api.is_nil() {
        return Err("The game's Mod Options API is unavailable".into());
    }
    let options = context.get(&api, "ModOptions")?;
    if options.is_nil() {
        return Err("The game's Mod Options API is unavailable".into());
    }
    let name = context.string("LuaBuddy")?;
    let existing = first(context.invoke(&options, "getOptions", std::slice::from_ref(&name))?);
    if !existing.is_nil() {
        return Ok(());
    }
    for module in [
        "ISUI/ISPanelJoypad",
        "ISUI/ISScrollingListBox",
        "ISUI/ISTextEntryBox",
        "ISUI/ISRichTextPanel",
        "ISUI/ISButton",
        "ISUI/ISModalRichText",
        "ISUI/ISTextBox",
        "ISUI/ISComboBox",
        "ISUI/ISTabPanel",
    ] {
        let name = context.string(module)?;
        context.call_global("require", &[name])?;
    }
    let options = first(context.invoke(&options, "create", &[name.clone(), name])?);
    let description = context.string("Browse installed and Workshop mods. Subscription, installation, selection and loading have separate states. Apply pending Options changes first.")?;
    context.invoke(&options, "addDescription", &[description])?;
    for (id, title, tooltip, action) in [
        (
            "manage",
            "Manage mods",
            "Browse installed mods and Workshop, then inspect, enable or load a selection.",
            Action::Open,
        ),
        (
            "refresh",
            "Load new profiles",
            "Load newly enabled profiles without rerunning loaded mods.",
            Action::Refresh,
        ),
        (
            "reload",
            "Reload a profile",
            "Reload requires explicit cooperative cleanup support. Other profiles require reconnecting.",
            Action::Reload,
        ),
        (
            "patchDiagnostics",
            "Patch diagnostics",
            "Inspect patch capabilities, approved extensions, installed targets, and rejection reasons.",
            Action::PatchDiagnostics,
        ),
        (
            "applyApprovedPatches",
            "Apply approved patches",
            "Retry already approved extensions after their host targets become available.",
            Action::ApplyApprovedPatches,
        ),
    ] {
        let args = [
            context.string(id)?,
            context.string(title)?,
            context.string(tooltip)?,
            callback(context, session, action)?,
        ];
        context.invoke(&options, "addButton", &args)?;
    }
    Ok(())
}

pub fn pending_options(context: &mut Context<'_, '_>) -> Result<bool> {
    let class = context.global("MainOptions")?;
    if class.is_nil() {
        return Ok(false);
    }
    let main = context.get(&class, "instance")?;
    if main.is_nil() {
        return Ok(false);
    }
    let options = context.get(&main, "gameOptions")?;
    if options.is_nil() {
        return Ok(false);
    }
    let changed = context.get(&options, "changed")?;
    context.truth(&changed)
}

fn require_clean_options(context: &mut Context<'_, '_>) -> Result<()> {
    if pending_options(context)? {
        Err("Apply or discard pending Options changes before changing or loading profiles.".into())
    } else {
        Ok(())
    }
}

pub fn refresh_options(context: &mut Context<'_, '_>) -> Result<()> {
    let class = context.global("MainOptions")?;
    if class.is_nil() {
        return Ok(());
    }
    let main = context.get(&class, "instance")?;
    if main.is_nil() {
        return Ok(());
    }
    let tabs = context.get(&main, "tabs")?;
    if tabs.is_nil() {
        return Ok(());
    }
    let text_id = context.string("UI_mainscreen_mods")?;
    let title = first(context.call_global("getText", &[text_id])?);
    let panel = first(context.invoke(&tabs, "getView", std::slice::from_ref(&title))?);
    if panel.is_nil() {
        return Ok(());
    }
    let view_list = context.get(&tabs, "viewList")?;
    let mut original = None;
    for (index, record) in context.entries(&view_list)? {
        let view = context.get(&record, "view")?;
        if context.same(&view, &panel)? {
            let id = context.get(&record, "id")?;
            if id.is_nil() {
                return Err("The native Mods tab has no stable ID".into());
            }
            original = Some((index, id));
            break;
        }
    }
    let (original_index, original_id) =
        original.ok_or("The native Mods tab is absent from its view registry")?;
    let scroll = first(context.invoke(&panel, "getYScroll", &[])?);
    let options = context.get(&main, "gameOptions")?;
    let records = context.get(&options, "options")?;
    let mut remove = Vec::new();
    for (key, option) in context.entries(&records)? {
        let control = context.get(&option, "control")?;
        if belongs_to(context, &control, &panel)? {
            remove.push((context.numeric(&key)? as i64, option));
        }
    }
    remove.sort_by_key(|(index, _)| std::cmp::Reverse(*index));
    for (_, option) in remove {
        context.invoke(&options, "remove", &[option])?;
    }
    let keys = context.get(&class, "keyText")?;
    let mut remove = Vec::new();
    if !keys.is_nil() {
        for (key, binding) in context.entries(&keys)? {
            let mod_binding = context.get(&binding, "isModBind")?;
            let button = context.get(&binding, "btn")?;
            if context.truth(&mod_binding)? && belongs_to(context, &button, &panel)? {
                remove.push(context.numeric(&key)? as i64);
            }
        }
        remove.sort_by_key(|index| std::cmp::Reverse(*index));
        let table = context.global("table")?;
        let table_remove = context.get(&table, "remove")?;
        for index in remove {
            let index = context.number(index as f64)?;
            context.call(&table_remove, &[keys.clone(), index])?;
        }
    }
    context.invoke(&tabs, "removeView", &[panel])?;
    context.invoke(&main, "addModOptionsPanel", &[])?;
    let panel = first(context.invoke(&tabs, "getView", std::slice::from_ref(&title))?);
    let view_list = context.get(&tabs, "viewList")?;
    let mut replacement = None;
    for (index, record) in context.entries(&view_list)? {
        let view = context.get(&record, "view")?;
        if context.same(&view, &panel)? {
            replacement = Some((index, record));
            break;
        }
    }
    let (replacement_index, replacement) =
        replacement.ok_or("The rebuilt native Mods tab is absent from its view registry")?;
    context.set(&replacement, "id", &original_id)?;
    let table = context.global("table")?;
    let table_remove = context.get(&table, "remove")?;
    let table_insert = context.get(&table, "insert")?;
    context.call(&table_remove, &[view_list.clone(), replacement_index])?;
    context.call(&table_insert, &[view_list, original_index, replacement])?;
    context.invoke(&options, "toUI", &[])?;
    context.invoke(&tabs, "activateView", std::slice::from_ref(&title))?;
    let panel = first(context.invoke(&tabs, "getView", &[title])?);
    context.invoke(&panel, "setYScroll", &[scroll])?;
    Ok(())
}

fn belongs_to(context: &mut Context<'_, '_>, control: &Value, panel: &Value) -> Result<bool> {
    let mut control = control.clone();
    for _ in 0..128 {
        if control.is_nil() {
            return Ok(false);
        }
        if context.same(&control, panel)? {
            return Ok(true);
        }
        control = context.get(&control, "parent")?;
    }
    Err("A native Options control has an invalid parent chain".into())
}

fn current(context: &mut Context<'_, '_>, session: u64) -> Result<()> {
    let environment = UI
        .with(|state| {
            state
                .borrow()
                .as_ref()
                .filter(|ui| ui.session == session)
                .map(|ui| ui.environment.clone())
        })
        .ok_or("This LuaBuddy interface is no longer active")?;
    if context.same(&environment, &context.environment.clone())? {
        Ok(())
    } else {
        Err("This LuaBuddy interface belongs to a retired game environment".into())
    }
}

fn dispatch<'vm>(
    env: &mut JNIEnv<'vm>,
    frame: &JObject<'vm>,
    count: i32,
    token: u64,
) -> Result<i32> {
    let session = token / 256;
    let action = Action::from((token % 256) as u8).ok_or("Unknown LuaBuddy interface action")?;
    let mut context = Context::current(env)?;
    current(&mut context, session)?;
    let args = context.frame_arguments(frame, count)?;
    if matches!(action, Action::Consumed) {
        let consumed = consumed(&mut context, &args)?;
        let value = context.boolean(consumed)?;
        return context.frame_return(frame, &[value]);
    }
    let result = (|| -> Result<()> {
        match action {
            Action::Open => open(&mut context, session),
            Action::Refresh => {
                let (kind, summary) = load_enabled(&mut context, session)?;
                let _ = with_browser(&mut context, session, |context, browser| {
                    if selected(context, browser).is_ok() {
                        details(context, browser)?;
                    }
                    feedback(context, browser, kind, &summary)
                });
                message(
                    &mut context,
                    session,
                    &format!("{}: {summary}", kind.label()),
                )
            }
            Action::Reload => reload_prompt(&mut context, session),
            Action::Help => help(&mut context, session),
            Action::JoypadGain
            | Action::JoypadLose
            | Action::JoypadDown
            | Action::JoypadDeactivate => joypad(&mut context, action, &args),
            Action::ReloadSelect => reload_select(&mut context, &args),
            Action::ModalPrerender => modal_prerender(&mut context, &args),
            Action::ReloadJoypadGain | Action::ReloadJoypadLose => reload_focus(
                &mut context,
                &args,
                matches!(action, Action::ReloadJoypadGain),
            ),
            Action::ReloadSubmit | Action::ReloadEnter => reload_submit(
                &mut context,
                session,
                &args,
                matches!(action, Action::ReloadEnter),
            ),
            Action::Key => key(&mut context, session, &args),
            Action::EntryKey => {
                if let Some(entry) = args.first() {
                    let parent = context.get(entry, "parent")?;
                    let mut args = args.clone();
                    args[0] = parent;
                    key(&mut context, session, &args)
                } else {
                    Ok(())
                }
            }
            Action::Destroy => destroy(&mut context, &args),
            Action::Update => {
                if let Some(panel) = args.first() {
                    let original = context.get(panel, "luabuddyOriginalUpdate")?;
                    if !original.is_nil() {
                        context.call(&original, std::slice::from_ref(panel))?;
                    }
                    modal_input(&mut context, session, panel)?;
                    let browser = context.get(panel, "luabuddyBrowser")?;
                    if context.truth(&browser)? {
                        tick(&mut context)?;
                    }
                }
                Ok(())
            }
            Action::Restart => restart(&mut context),
            Action::PatchDiagnostics => patch_diagnostics(&mut context, session, false),
            Action::ApplyApprovedPatches => patch_diagnostics(&mut context, session, true),
            action => with_browser(&mut context, session, |context, browser| {
                browser_action(context, session, browser, action)
            }),
        }
    })();
    if let Err(error) = result {
        game::log(&format!("interface action failed: {error}"));
        let _ = with_browser(&mut context, session, |context, browser| {
            feedback(context, browser, Feedback::Error, &error)
        });
        message(&mut context, session, &error)?;
    }
    Ok(0)
}

fn with_browser<T>(
    context: &mut Context<'_, '_>,
    session: u64,
    action: impl FnOnce(&mut Context<'_, '_>, &mut Browser) -> Result<T>,
) -> Result<T> {
    let browser = UI.with(|state| {
        state
            .borrow_mut()
            .as_mut()
            .filter(|ui| ui.session == session)
            .and_then(|ui| ui.browser.take())
    });
    let mut browser =
        browser.ok_or("The mod browser is not available or an action is already running")?;
    let result = action(context, &mut browser);
    UI.with(|state| {
        if let Some(ui) = state
            .borrow_mut()
            .as_mut()
            .filter(|ui| ui.session == session)
        {
            ui.browser = Some(browser);
        }
    });
    result
}

fn numbers(context: &mut Context<'_, '_>, values: &[f64]) -> Result<Vec<Value>> {
    values.iter().map(|value| context.number(*value)).collect()
}

fn construct(context: &mut Context<'_, '_>, class: &str, args: &[Value]) -> Result<Value> {
    let class = context.global(class)?;
    if class.is_nil() {
        return Err("A required native UI factory is unavailable".into());
    }
    let object = first(context.invoke(&class, "new", args)?);
    if object.is_nil() {
        return Err("The native UI factory returned no object".into());
    }
    Ok(object)
}

fn field_text(
    context: &mut Context<'_, '_>,
    object: &Value,
    field: &str,
    text: &str,
) -> Result<()> {
    let value = context.string(text)?;
    context.set(object, field, &value)
}

fn child(context: &mut Context<'_, '_>, panel: &Value, object: &Value) -> Result<()> {
    initialise(context, object)?;
    context.invoke(panel, "addChild", std::slice::from_ref(object))?;
    Ok(())
}

fn initialise(context: &mut Context<'_, '_>, object: &Value) -> Result<()> {
    context.invoke(object, "initialise", &[])?;
    if context.get(object, "javaObject")?.is_nil() {
        context.invoke(object, "instantiate", &[])?;
    }
    Ok(())
}

fn button(
    context: &mut Context<'_, '_>,
    session: u64,
    panel: &Value,
    rectangle: [f64; 4],
    title: &str,
    action: Action,
) -> Result<Value> {
    let mut args = numbers(context, &rectangle)?;
    args.push(context.string(title)?);
    args.push(panel.clone());
    args.push(callback(context, session, action)?);
    let button = construct(context, "ISButton", &args)?;
    child(context, panel, &button)?;
    field_text(context, &button, "tooltip", action_tooltip(action))?;
    Ok(button)
}

fn geometry(context: &mut Context<'_, '_>, max_width: f64, max_height: f64) -> Result<[f64; 4]> {
    let core = first(context.call_global("getCore", &[])?);
    let (width, height) = context.jni(|env| {
        Ok((
            env.call_method(core.object(), "getScreenWidth", "()I", &[])?
                .i()?,
            env.call_method(core.object(), "getScreenHeight", "()I", &[])?
                .i()?,
        ))
    })?;
    let w = max_width.min((width - 40).max(280) as f64);
    let h = max_height.min((height - 40).max(240) as f64);
    Ok([
        ((width as f64 - w) / 2.0).max(0.0),
        ((height as f64 - h) / 2.0).max(0.0),
        w,
        h,
    ])
}

type ActionGroup = (
    &'static str,
    &'static str,
    &'static [(&'static str, Action)],
);

fn action_groups() -> [ActionGroup; 4] {
    [
        (
            "This session",
            "Enable late saves a profile. Load enabled applies eligible profiles now. Disable late changes future environments.",
            &[
                ("Enable late", Action::Enable),
                ("Disable late", Action::Disable),
                ("Load enabled", Action::Load),
                ("Reload a profile", Action::Reload),
            ],
        ),
        (
            "Next launch",
            "These controls change the game's native selection for its next normal launch. They do not load or unload running code.",
            &[
                ("Startup enable", Action::StartupEnable),
                ("Startup disable", Action::StartupDisable),
                ("Restart game...", Action::Restart),
            ],
        ),
        (
            "Workshop",
            "Subscription and download deliver files. Enable and load installed mod IDs separately.",
            &[
                ("Subscribe + download", Action::Add),
                ("Download / update", Action::Download),
                ("Open Workshop", Action::OpenWorkshop),
                ("Cancel query", Action::Cancel),
            ],
        ),
        (
            "Diagnostics",
            "Inspect actual runtime and patch state. Reload and native patch activation have separate safety requirements.",
            &[
                ("Patch diagnostics", Action::PatchDiagnostics),
                ("Apply approved patches", Action::ApplyApprovedPatches),
                ("Help & guides", Action::Help),
            ],
        ),
    ]
}

fn action_tooltip(action: Action) -> &'static str {
    match action {
        Action::Enable => {
            "Save this mod and required dependencies as late-loader selections. Then choose Load enabled. Native startup-only components still need a normal restart."
        }
        Action::Disable => {
            "Disable future late loading. Code already running stays active until reconnect or restart. This does not delete mod files."
        }
        Action::Load => {
            "Apply all enabled eligible profiles now. Read the result and each mod's Runtime state. Existing loaded profiles are not rerun."
        }
        Action::Reload => {
            "Choose an installed mod ID or type one. Only a locally loaded profile with explicit upstream cleanup support can reload safely."
        }
        Action::StartupEnable => {
            "Select this mod through the game's native dependency rules for the next normal launch. Running game and server selections stay unchanged."
        }
        Action::StartupDisable => {
            "Remove this mod from the next native startup selection after native dependent checks. Does not unload running code or remove source files."
        }
        Action::Restart => {
            "Use the game's normal quit flow, then relaunch through Steam. Save your progress before restarting."
        }
        Action::Search => {
            "Search Steam Workshop. The text field filters Installed and Subscribed when those views are chosen instead."
        }
        Action::Installed => {
            "Browse installed native mod identities, including local mods. Each Workshop item can contain several mod IDs."
        }
        Action::Subscribed => {
            "Read your Steam subscriptions. A subscription does not mean the item is installed, enabled, or loaded."
        }
        Action::Add => {
            "Subscribe to this verified Workshop item and request its download after Steam confirms. Does not enable or load it."
        }
        Action::Download => {
            "Request installation or update of the selected Workshop item. The game reports success only after installation is verified."
        }
        Action::OpenWorkshop => {
            "Open this verified item's original Steam page using the game's overlay or normal browser. Local mods have no Workshop page."
        }
        Action::Cancel => {
            "Cancel an observational Workshop search. Steam subscription or download operations cannot always be cancelled."
        }
        Action::PatchDiagnostics => {
            "Inspect current native patch capabilities, approved extensions, targets, and rejection reasons. This makes no changes."
        }
        Action::ApplyApprovedPatches => {
            "Retry already approved native extensions after their targets become available. This does not approve new code."
        }
        Action::Help => {
            "Complete guides for choosing a loading path, testing local mods, one-launch selection, Workshop installation, reload, and troubleshooting."
        }
        Action::Previous | Action::Next => {
            "Change the Workshop results page. Installed mod identities are shown in one filtered list."
        }
        Action::Close => {
            "Close this manager without changing saved selections. Escape works on keyboard; the native B action appears when using a controller."
        }
        _ => "",
    }
}

#[derive(Clone, Copy)]
enum Feedback {
    Info,
    Working,
    Success,
    Warning,
    Error,
}

impl Feedback {
    fn label(self) -> &'static str {
        match self {
            Self::Info => "Info",
            Self::Working => "Working",
            Self::Success => "Done",
            Self::Warning => "Attention",
            Self::Error => "Failed",
        }
    }
}

fn feedback(
    context: &mut Context<'_, '_>,
    browser: &Browser,
    kind: Feedback,
    text: &str,
) -> Result<()> {
    let color = match kind {
        Feedback::Info => "0.8,0.85,0.95",
        Feedback::Working => "0.55,0.8,1",
        Feedback::Success => "0.5,0.95,0.6",
        Feedback::Warning => "1,0.8,0.35",
        Feedback::Error => "1,0.5,0.5",
    };
    let escaped = crate::diagnostic_text::rich(&format!("{}: {text}", kind.label()));
    field_text(
        context,
        &browser.status,
        "text",
        &format!("<RGB:{color}> {escaped}"),
    )?;
    paginate(context, &browser.status)?;
    update_buttons(context, browser)
}

fn update_buttons(context: &mut Context<'_, '_>, browser: &Browser) -> Result<()> {
    let item = selected(context, browser).ok();
    let scope = match browser.mode {
        BrowseMode::Installed => "Installed",
        BrowseMode::Search { .. } => "Workshop",
        BrowseMode::Subscribed { .. } | BrowseMode::FilteredSubscriptions { .. } => "Subscribed",
    };
    field_text(
        context,
        &browser.search,
        "tooltip",
        &format!("Search {scope}. Results update as you type. Enter searches immediately."),
    )?;
    for (action, control) in &browser.buttons {
        let available = match action {
            Action::Add | Action::Download => {
                browser.pending.is_none()
                    && item.as_ref().is_some_and(|item| {
                        item.workshop.is_some() && item.unavailable_reason.is_none()
                    })
            }
            Action::OpenWorkshop => item.as_ref().is_some_and(|item| item.workshop.is_some()),
            Action::Load => browser.pending.is_none(),
            Action::Enable | Action::Disable | Action::StartupEnable | Action::StartupDisable => {
                browser.pending.is_none()
                    && item
                        .as_ref()
                        .is_some_and(|item| item.id.is_some() && item.available)
            }
            Action::Cancel => browser.pending.is_some(),
            _ => true,
        };
        let value = context.boolean(available)?;
        context.invoke(control, "setEnable", &[value])?;
        if matches!(
            action,
            Action::Installed | Action::Search | Action::Subscribed
        ) {
            let active = match action {
                Action::Installed => matches!(browser.mode, BrowseMode::Installed),
                Action::Search => matches!(browser.mode, BrowseMode::Search { .. }),
                Action::Subscribed => matches!(
                    browser.mode,
                    BrowseMode::Subscribed { .. } | BrowseMode::FilteredSubscriptions { .. }
                ),
                _ => false,
            };
            let border = context.get(control, "borderColor")?;
            let background = context.get(control, "backgroundColor")?;
            for (channel, value) in [
                ("r", if active { 0.35 } else { 0.5 }),
                ("g", if active { 0.75 } else { 0.5 }),
                ("b", if active { 1.0 } else { 0.5 }),
            ] {
                let value = context.number(value)?;
                context.set(&border, channel, &value)?;
            }
            let alpha = context.number(if active { 0.45 } else { 0.15 })?;
            context.set(&background, "a", &alpha)?;
        }
    }
    Ok(())
}

fn runtime_feedback(
    context: &mut Context<'_, '_>,
    result: Result<String>,
) -> Result<(Feedback, String)> {
    if let Err(error) = result {
        return Ok((Feedback::Error, error));
    }
    let state = context.global("LuaBuddy")?;
    let mods = context.get(&state, "mods")?;
    let mut loaded = 0;
    let mut native = 0;
    let mut failed = Vec::new();
    let mut waiting = Vec::new();
    for profile in configured(context)?
        .iter()
        .filter(|profile| profile.enabled)
    {
        let value = context.get(&mods, profile.id.as_str())?;
        let status = if value.is_nil() {
            "not loaded".into()
        } else {
            context.text(&value)?
        };
        match status.as_str() {
            "loaded" => loaded += 1,
            "server-loaded" => native += 1,
            _ if status.starts_with("failed:") => failed.push(format!("{}: {status}", profile.id)),
            _ => waiting.push(format!("{}: {status}", profile.id)),
        }
    }
    let mut summary = format!(
        "{loaded} enabled profiles are loaded by LuaBuddy; {native} are already native/server-loaded. Existing loaded code was retained."
    );
    if !failed.is_empty() {
        summary.push_str(&format!("\nFailures: {}", failed.join("; ")));
    }
    if !waiting.is_empty() {
        summary.push_str(&format!("\nNot loaded: {}", waiting.join("; ")));
    }
    summary.push_str("\nInspect the selected mod's Runtime state. Startup-only content needs a normal restart; client loading cannot activate missing server code.");
    let kind = if !failed.is_empty() {
        Feedback::Error
    } else if !waiting.is_empty() {
        Feedback::Warning
    } else {
        Feedback::Success
    };
    Ok((kind, summary))
}

fn load_enabled(context: &mut Context<'_, '_>, session: u64) -> Result<(Feedback, String)> {
    require_clean_options(context)?;
    if !configured(context)?.iter().any(|profile| profile.enabled) {
        return Ok((Feedback::Info, "No late selections are enabled. Choose This session > Enable late for eligible client/shared Lua, or Next launch > Startup enable for normal native startup. No selection was changed.".into()));
    }
    let result = runtime::apply(context, session, Operation::Refresh);
    runtime_feedback(context, result)
}

fn input_hint(context: &mut Context<'_, '_>, panel: &Value) -> Result<()> {
    let focus = context.get(panel, "joyfocus")?;
    let active = if focus.is_nil() {
        false
    } else {
        let active = context.get(&focus, "isActive")?;
        context.truth(&active)?
    };
    let close = context.get(panel, "luabuddyClose")?;
    if active {
        let previous = context.get(panel, "ISButtonB")?;
        field_text(context, &close, "title", "Close")?;
        context.invoke(panel, "setISButtonForB", &[close])?;
        if previous.is_nil() {
            context.invoke(panel, "restoreJoypadFocus", &[focus])?;
        }
    } else {
        context.invoke(panel, "clearISButtonB", &[])?;
        field_text(context, &close, "title", "Close (Esc)")?;
    }
    Ok(())
}

fn joypad(context: &mut Context<'_, '_>, action: Action, args: &[Value]) -> Result<()> {
    let panel = args.first().ok_or("Missing native input receiver")?;
    let close = context.get(panel, "luabuddyClose")?;
    if matches!(action, Action::JoypadDown) && args.len() >= 2 {
        let joypad = context.global("Joypad")?;
        let left = context.get(&joypad, "LBumper")?;
        let right = context.get(&joypad, "RBumper")?;
        let pressed = context.numeric(&args[1])?;
        if pressed == context.numeric(&left)? || pressed == context.numeric(&right)? {
            let tabs = context.get(panel, "luabuddyActionTabs")?;
            let index = first(context.invoke(&tabs, "getActiveViewIndex", &[])?);
            let index = context.numeric(&index)? as i32;
            let next = if pressed == context.numeric(&left)? {
                (index + 2) % 4 + 1
            } else {
                index % 4 + 1
            };
            let next = context.number(f64::from(next))?;
            context.invoke(&tabs, "activateViewById", &[next])?;
            context.invoke(panel, "setJoypadFocusTopLeft", &args[2..])?;
            return Ok(());
        }
    }
    let saved = match action {
        Action::JoypadGain => "luabuddyJoypadGain",
        Action::JoypadLose => "luabuddyJoypadLose",
        Action::JoypadDown => "luabuddyJoypadDown",
        Action::JoypadDeactivate => "luabuddyJoypadDeactivate",
        _ => return Ok(()),
    };
    let original = context.get(panel, saved)?;
    if !original.is_nil() {
        context.call(&original, args)?;
    }
    if matches!(action, Action::JoypadGain) {
        input_hint(context, panel)?;
        let search = context.get(panel, "luabuddySearch")?;
        context.invoke(&search, "unfocus", &[])?;
        context.invoke(panel, "setJoypadFocusTopLeft", &args[1..])?;
    } else if matches!(action, Action::JoypadLose | Action::JoypadDeactivate) {
        context.invoke(panel, "clearISButtonB", &[])?;
        context.invoke(panel, "clearJoypadFocus", &args[1..])?;
        field_text(context, &close, "title", "Close (Esc)")?;
    }
    Ok(())
}

fn help(context: &mut Context<'_, '_>, session: u64) -> Result<()> {
    message(
        context,
        session,
        "LuaBuddy guides\n\nChoose a loading path\n1. Open Installed and select the exact mod ID. Read This game, Late selection, and Next launch separately.\n2. Use This session for eligible client/shared Lua in multiplayer. Use Next launch for ordinary native startup, server code, maps, or other native assets.\n3. A Steam subscription only delivers files. It does not select or load the mod.\n\nTest an installed or local mod\n1. Install a valid local mod under Zomboid/mods, or register its root with luabuddy select --profiles-dir DIR --mod ID --path ROOT. ROOT contains common/ and the version folders.\n2. If testing only late loading, use Next launch > Startup disable first when that mod is natively selected. Preserve other selections. Reconnect or restart if it is already running natively.\n3. Choose This session > Enable late, then Load enabled. Apply or discard pending Options edits first.\n4. Confirm a green result and Runtime: loaded for this mod. Attention or Failed gives the actual remaining state. Test its controls in game.\n5. Choose Disable late to stop future late activation, then reconnect. Running code cannot generally be unloaded. Source files and settings stay intact.\n\nUse a mod for one game launch\n1. Queue it before launching with luabuddy select --profiles-dir DIR --mod ID --path ROOT --once. It must not duplicate a persistent late selection.\n2. Start the game normally. The registration is consumed for that process and remains available across reconnects. Native/server and late-loading eligibility still apply.\n3. The next new game process has no temporary selection. Original source files are never deleted. Cancel an unused queue with luabuddy deselect --profiles-dir DIR --mod ID --once.\n4. A queue added after the game process initialized is reserved for the next process. Persistent native startup selections are independent; disable one separately if it would keep loading the mod.\n\nInstall from Workshop\n1. Search Workshop or inspect Subscribed. Open Workshop shows the author's original page.\n2. Choose Subscribe + download or Download / update and wait for verified installation.\n3. Open Installed, select the actual mod ID and inspect required mods and eligibility.\n4. Choose one loading path above. Confirm the result and Runtime state. Disable the corresponding selection to undo activation for future sessions.\n\nReload a cooperating profile\n1. Apply pending Options changes. Use Reload a profile and choose an installed ID or type a custom one.\n2. The profile must already be locally loaded and declare upstream cleanup. Active dependents or unsupported cleanup are refused.\n3. Inspect the result. If reconnect is required, keep the current game until ready and reconnect normally. Reload is not a general mod uninstaller.\n\nTroubleshoot a missing feature\n1. Check Runtime, selection lifetime, dependencies, native compatibility and activation limits for the selected mod.\n2. Read the action result. Loaded client/shared Lua does not supply missing server code or every native asset.\n3. Use Diagnostics > Patch diagnostics for native extension state. Apply approved patches retries approved code only.\n4. Read LuaBuddy/diagnostics.json in the game cache and the game's console log for the exact profile failure.\n\nNavigation\nMouse and keyboard use the native controls; Escape closes. Native controller focus shows B to close. Left and right bumpers switch action tabs; the D-pad and A use normal native focus and activation.",
    )
}

fn open(context: &mut Context<'_, '_>, session: u64) -> Result<()> {
    if let Some(panel) = UI.with(|state| {
        state
            .borrow()
            .as_ref()
            .and_then(|ui| ui.browser.as_ref())
            .map(|browser| browser.panel.clone())
    }) {
        let visible = first(context.invoke(&panel, "getIsVisible", &[])?);
        if context.truth(&visible)? {
            context.invoke(&panel, "bringToTop", &[])?;
            return Ok(());
        }
        let yes = context.boolean(true)?;
        context.invoke(&panel, "setVisible", &[yes])?;
        show(context, session, &panel)?;
        let entry = context.get(&panel, "entry")?;
        context.invoke(&entry, "focus", &[])?;
        return Ok(());
    }
    let [x, y, w, h] = geometry(context, 1080.0, 760.0)?;
    if w < 700.0 || h < 500.0 {
        return Err("The mod browser requires a game window of at least 740 by 540 pixels. Increase the game's resolution and reopen it.".into());
    }
    let manager = first(context.call_global("getTextManager", &[])?);
    let font_height = f64::from(
        context
            .jni(|env| {
                let font = env
                    .get_static_field("zombie/ui/UIFont", "Small", "Lzombie/ui/UIFont;")?
                    .l()?;
                env.call_method(
                    manager.object(),
                    "getFontHeight",
                    "(Lzombie/ui/UIFont;)I",
                    &[JValue::Object(&font)],
                )?
                .i()
            })?
            .max(14),
    );
    let control_height = (font_height + 10.0).max(28.0);
    let header_height = font_height + 12.0;
    let search_y = header_height + 14.0;
    let list_y = search_y + control_height + 10.0;
    let explanation_width = f64::from(context.jni(|env| {
        let font = env
            .get_static_field("zombie/ui/UIFont", "Small", "Lzombie/ui/UIFont;")?
            .l()?;
        let mut longest = 0;
        for (_, explanation, _) in action_groups() {
            let text = env.new_string(explanation)?;
            let width = env
                .call_method(
                    manager.object(),
                    "MeasureStringX",
                    "(Lzombie/ui/UIFont;Ljava/lang/String;)I",
                    &[JValue::Object(&font), JValue::Object(&text)],
                )?
                .i()?;
            longest = longest.max(width);
        }
        Ok(longest)
    })?);
    let explanation_lines = ((explanation_width / (w - 64.0)).ceil() + 1.0).max(2.0);
    let explanation_height = font_height * explanation_lines + 6.0;
    let page_height = 8.0 + control_height + 6.0 + explanation_height + 4.0;
    let tabs_height = font_height + 6.0 + page_height;
    let footer_y = h - control_height - 10.0;
    let tabs_y = footer_y - 8.0 - tabs_height;
    let status_height = (font_height * 3.0 + 16.0).max(70.0);
    let status_y = tabs_y - 8.0 - status_height;
    let list_height = status_y - 12.0 - list_y;
    if list_height < font_height * 5.0 {
        return Err("The selected UI font needs more room. Increase the game window resolution or reduce its UI font size, then reopen the manager.".into());
    }
    let panel_args = numbers(context, &[x, y, w, h])?;
    let panel = construct(context, "ISPanelJoypad", &panel_args)?;
    let background = context.get(&panel, "backgroundColor")?;
    let opacity = context.number(1.0)?;
    context.set(&background, "a", &opacity)?;
    initialise(context, &panel)?;
    let yes = context.boolean(true)?;
    context.set(&panel, "moveWithMouse", &yes)?;
    context.set(&panel, "luabuddyBrowser", &yes)?;
    field_text(context, &panel, "title", "LuaBuddy mod manager")?;
    let header_args = numbers(context, &[12.0, 4.0, w - 190.0, header_height])?;
    let header = construct(context, "ISRichTextPanel", &header_args)?;
    let no = context.boolean(false)?;
    context.set(&header, "autosetheight", &no)?;
    context.set(&header, "clip", &yes)?;
    child(context, &panel, &header)?;
    let margins = numbers(context, &[8.0, 2.0, 8.0, 2.0])?;
    context.invoke(&header, "setMargins", &margins)?;
    rich(
        context,
        &header,
        "LuaBuddy mod manager | Browse, choose a loading path, then apply.",
    )?;
    let help_button = button(
        context,
        session,
        &panel,
        [w - 166.0, 6.0, 154.0, control_height],
        "Help & guides",
        Action::Help,
    )?;
    let mut search_args = vec![context.string("")?];
    search_args.extend(numbers(
        context,
        &[12.0, search_y, w - 410.0, control_height],
    )?);
    let search = construct(context, "ISTextEntryBox", &search_args)?;
    child(context, &panel, &search)?;
    context.set(&panel, "entry", &search)?;
    let search_callback = callback(context, session, Action::SearchSubmit)?;
    context.set(&search, "onCommandEntered", &search_callback)?;
    let other_key = callback(context, session, Action::EntryKey)?;
    context.set(&search, "onOtherKey", &other_key)?;
    let search_button = button(
        context,
        session,
        &panel,
        [w - 390.0, search_y, 116.0, control_height],
        "Workshop search",
        Action::Search,
    )?;
    let installed_button = button(
        context,
        session,
        &panel,
        [w - 266.0, search_y, 116.0, control_height],
        "Installed",
        Action::Installed,
    )?;
    let subscribed_button = button(
        context,
        session,
        &panel,
        [w - 142.0, search_y, 130.0, control_height],
        "Subscribed",
        Action::Subscribed,
    )?;
    let list_width = (w * 0.40).max(220.0);
    let list_args = numbers(context, &[12.0, list_y, list_width, list_height])?;
    let list = construct(context, "ISScrollingListBox", &list_args)?;
    let item_height = context.number(control_height)?;
    context.set(&list, "itemheight", &item_height)?;
    context.set(&list, "drawBorder", &yes)?;
    child(context, &panel, &list)?;
    let select_callback = callback(context, session, Action::Select)?;
    context.invoke(
        &list,
        "setOnMouseDownFunction",
        &[panel.clone(), select_callback],
    )?;
    let details_args = numbers(
        context,
        &[
            list_width + 24.0,
            list_y,
            w - list_width - 36.0,
            list_height,
        ],
    )?;
    let details = construct(context, "ISRichTextPanel", &details_args)?;
    let no = context.boolean(false)?;
    context.set(&details, "autosetheight", &no)?;
    context.set(&details, "clip", &yes)?;
    child(context, &panel, &details)?;
    context.invoke(&details, "addScrollBars", std::slice::from_ref(&yes))?;
    let status_args = numbers(context, &[12.0, status_y, w - 24.0, status_height])?;
    let status = construct(context, "ISRichTextPanel", &status_args)?;
    context.set(&status, "autosetheight", &no)?;
    context.set(&status, "clip", &yes)?;
    child(context, &panel, &status)?;
    let margins = numbers(context, &[8.0, 3.0, 24.0, 12.0])?;
    context.invoke(&status, "setMargins", &margins)?;
    context.invoke(&status, "addScrollBars", std::slice::from_ref(&yes))?;
    let tabs_args = numbers(context, &[12.0, tabs_y, w - 24.0, tabs_height])?;
    let tabs = construct(context, "ISTabPanel", &tabs_args)?;
    child(context, &panel, &tabs)?;
    context.set(&panel, "luabuddyActionTabs", &tabs)?;
    context.set(&panel, "luabuddySearch", &search)?;
    let mut buttons = vec![
        (Action::Search, search_button.clone()),
        (Action::Installed, installed_button.clone()),
        (Action::Subscribed, subscribed_button.clone()),
    ];
    for (name, explanation, actions) in action_groups() {
        let args = numbers(context, &[0.0, 0.0, w - 24.0, page_height])?;
        let page = construct(context, "ISPanelJoypad", &args)?;
        initialise(context, &page)?;
        context.set(&page, "background", &no)?;
        let width = (w - 48.0 - 8.0 * (actions.len() - 1) as f64) / actions.len() as f64;
        let mut row = Vec::new();
        for (index, (title, action)) in actions.iter().enumerate() {
            let control = button(
                context,
                session,
                &page,
                [
                    12.0 + index as f64 * (width + 8.0),
                    8.0,
                    width,
                    control_height,
                ],
                title,
                *action,
            )?;
            row.push(control.clone());
            buttons.push((*action, control));
        }
        context.invoke(&panel, "insertNewLineOfButtons", &row)?;
        let args = numbers(
            context,
            &[
                4.0,
                8.0 + control_height + 6.0,
                w - 40.0,
                explanation_height,
            ],
        )?;
        let description = construct(context, "ISRichTextPanel", &args)?;
        context.set(&description, "autosetheight", &no)?;
        context.set(&description, "clip", &yes)?;
        child(context, &page, &description)?;
        let margins = numbers(context, &[8.0, 2.0, 8.0, 2.0])?;
        context.invoke(&description, "setMargins", &margins)?;
        field_text(context, &description, "tooltip", explanation)?;
        rich(context, &description, explanation)?;
        let title = context.string(name)?;
        context.invoke(&tabs, "addView", &[title, page])?;
    }
    let footer_width = (w - 42.0) / 4.0;
    let mut footer = Vec::new();
    for (index, (title, action)) in [
        ("< Previous", Action::Previous),
        ("Next >", Action::Next),
        ("Help & guides", Action::Help),
        ("Close (Esc)", Action::Close),
    ]
    .into_iter()
    .enumerate()
    {
        let control = button(
            context,
            session,
            &panel,
            [
                12.0 + index as f64 * (footer_width + 6.0),
                h - 36.0,
                footer_width,
                26.0,
            ],
            title,
            action,
        )?;
        if matches!(action, Action::Close) {
            context.set(&panel, "luabuddyClose", &control)?;
        }
        footer.push(control);
    }
    context.invoke(&panel, "insertNewLineOfButtons", &[help_button])?;
    context.invoke(
        &panel,
        "insertNewLineOfButtons",
        &[
            search.clone(),
            search_button,
            installed_button,
            subscribed_button,
        ],
    )?;
    context.invoke(
        &panel,
        "insertNewLineOfButtons",
        std::slice::from_ref(&list),
    )?;
    context.invoke(&panel, "insertNewLineOfButtons", &footer)?;
    for (method, saved, action) in [
        (
            "onGainJoypadFocus",
            "luabuddyJoypadGain",
            Action::JoypadGain,
        ),
        (
            "onLoseJoypadFocus",
            "luabuddyJoypadLose",
            Action::JoypadLose,
        ),
        ("onJoypadDown", "luabuddyJoypadDown", Action::JoypadDown),
        (
            "onJoypadBeforeDeactivate",
            "luabuddyJoypadDeactivate",
            Action::JoypadDeactivate,
        ),
    ] {
        let original = context.index(&panel, method)?;
        context.set(&panel, saved, &original)?;
        let handler = callback(context, session, action)?;
        context.set(&panel, method, &handler)?;
    }
    let base = game::property(context, "user.dir")?
        .ok_or("The game's working directory is unavailable")?;
    let steam = steam::SteamWorkshop::attach_existing(
        context.env,
        &Path::new(&base).join("natives/libsteam_api.so"),
    )
    .map_err(|error| error.to_string());
    let mut browser = Browser {
        panel: panel.clone(),
        search: search.clone(),
        list,
        details,
        status,
        items: Vec::new(),
        selected: usize::MAX,
        mode: BrowseMode::Installed,
        total_matching: 0,
        steam,
        pending: None,
        next_poll: Instant::now(),
        buttons,
        installed_catalog: Vec::new(),
        search_input: String::new(),
        search_changed: None,
    };
    installed(context, &mut browser)?;
    UI.with(|state| {
        if let Some(ui) = state.borrow_mut().as_mut() {
            ui.browser = Some(browser);
        }
    });
    show(context, session, &panel)?;
    if context.get(&panel, "joyfocus")?.is_nil() {
        context.invoke(&search, "focus", &[])?;
    }
    Ok(())
}

fn rich(context: &mut Context<'_, '_>, panel: &Value, text: &str) -> Result<()> {
    let escaped = crate::diagnostic_text::rich(text);
    field_text(context, panel, "text", &escaped)?;
    paginate(context, panel)
}

fn paginate(context: &mut Context<'_, '_>, panel: &Value) -> Result<()> {
    context.invoke(panel, "paginate", &[])?;
    let lines = context.get(panel, "lines")?;
    let positions = context.get(panel, "lineX")?;
    let positions = indexed(context, &positions)?;
    let fonts = context.get(panel, "fonts")?;
    let fonts = indexed(context, &fonts)?;
    let mut font = context.get(panel, "defaultFont")?;
    let width = first(context.invoke(panel, "getWidth", &[])?);
    let mut width = context.numeric(&width)?;
    let left = context.get(panel, "marginLeft")?;
    let right = context.get(panel, "marginRight")?;
    let margins = context.numeric(&left)? + context.numeric(&right)? + 16.0;
    let manager = first(context.call_global("getTextManager", &[])?);
    for (index, line) in indexed(context, &lines)? {
        if let Some(selected) = fonts.get(&index) {
            font = selected.clone();
        }
        let position = match positions.get(&index) {
            Some(position) => context.numeric(position)?,
            None => 0.0,
        };
        let text = context.text(&line)?;
        let measured = context.jni(|env| {
            let text = env.new_string(text.trim())?;
            env.call_method(
                manager.object(),
                "MeasureStringX",
                "(Lzombie/ui/UIFont;Ljava/lang/String;)I",
                &[JValue::Object(font.object()), JValue::Object(&text)],
            )?
            .i()
        })?;
        width = width.max(position + f64::from(measured) + margins);
    }
    let width = context.number(width)?;
    context.invoke(panel, "setScrollWidth", &[width])?;
    Ok(())
}

fn indexed(context: &mut Context<'_, '_>, table: &Value) -> Result<BTreeMap<i64, Value>> {
    context
        .entries(table)?
        .into_iter()
        .map(|(index, value)| context.numeric(&index).map(|index| (index as i64, value)))
        .collect()
}

fn set_status(context: &mut Context<'_, '_>, browser: &Browser, text: &str) -> Result<()> {
    feedback(
        context,
        browser,
        if browser.pending.is_some() {
            Feedback::Working
        } else {
            Feedback::Info
        },
        text,
    )
}

fn cancel_observation(browser: &mut Browser) -> bool {
    match browser.pending.take() {
        Some(Pending::Query(_)) | Some(Pending::SubscriptionSearch(_)) | None => true,
        Some(operation) => {
            browser.pending = Some(operation);
            false
        }
    }
}

fn search_as_you_type(
    context: &mut Context<'_, '_>,
    session: u64,
    browser: &mut Browser,
) -> Result<()> {
    let value = first(context.invoke(&browser.search, "getText", &[])?);
    let text = context.text(&value)?;
    if text != browser.search_input {
        browser.search_input = text;
        browser.search_changed = Some(Instant::now());
        if matches!(browser.mode, BrowseMode::Installed) {
            return filter_installed(context, browser);
        }
        if cancel_observation(browser) {
            browser.items.clear();
            render(context, browser)?;
            feedback(
                context,
                browser,
                Feedback::Working,
                "Search text changed. Waiting briefly before requesting the latest results.",
            )?;
        }
    }
    let Some(changed) = browser.search_changed else {
        return Ok(());
    };
    if changed.elapsed() < Duration::from_millis(350) {
        return Ok(());
    }
    if !cancel_observation(browser) {
        return Ok(());
    }
    browser.search_changed = None;
    let action = browser.mode.search_action();
    if matches!(action, Action::Installed) {
        return Ok(());
    }
    browser_action(context, session, browser, action)
}

fn browser_action(
    context: &mut Context<'_, '_>,
    session: u64,
    browser: &mut Browser,
    action: Action,
) -> Result<()> {
    let action = if matches!(action, Action::SearchSubmit) {
        browser.mode.search_action()
    } else {
        action
    };
    if matches!(
        action,
        Action::Installed | Action::Search | Action::Subscribed
    ) {
        if !cancel_observation(browser) {
            return Err(
                "Wait for Steam's subscription or download before changing the search source."
                    .into(),
            );
        }
        let value = first(context.invoke(&browser.search, "getText", &[])?);
        browser.search_input = context.text(&value)?;
        browser.search_changed = None;
        browser.mode.select_source(action, &browser.search_input);
        browser.total_matching = 0;
        browser.items.clear();
        render(context, browser)?;
        update_buttons(context, browser)?;
    }
    if browser.pending.is_some()
        && matches!(
            action,
            Action::Add
                | Action::Download
                | Action::Enable
                | Action::Disable
                | Action::Load
                | Action::StartupEnable
                | Action::StartupDisable
        )
    {
        return Err(
            "Wait for the pending Workshop operation before changing or loading the selected mod."
                .into(),
        );
    }
    if matches!(action, Action::Add | Action::Download)
        && let Some(reason) = selected(context, browser)?.unavailable_reason
    {
        return Err(format!(
            "Workshop item details are not verified, so this action is unavailable: {reason}"
        ));
    }
    match action {
        Action::Installed => installed(context, browser),
        Action::Search => {
            if browser.pending.is_some() {
                return Err("Wait for the current operation or cancel its query.".into());
            }
            let search = first(context.invoke(&browser.search, "getText", &[])?);
            let text = context.text(&search)?;
            start_page(browser, BrowseMode::Search { text, page: 1 })?;
            set_status(
                context,
                browser,
                "Searching Workshop. Results are not installed or enabled until those operations are verified.",
            )
        }
        Action::Subscribed => {
            if browser.pending.is_some() {
                return Err("Wait for the current operation or cancel its query.".into());
            }
            let steam = browser.steam.as_ref().map_err(Clone::clone)?;
            let items = steam.subscribed_items().map_err(|e| e.to_string())?;
            browser.mode = BrowseMode::Subscribed {
                items: items.clone(),
                page: 1,
            };
            if items.is_empty() {
                browser.items.clear();
                render(context, browser)?;
                return set_status(context, browser, "No Workshop subscriptions were returned.");
            }
            let filter = first(context.invoke(&browser.search, "getText", &[])?);
            let filter = context.text(&filter)?.trim().to_lowercase();
            if filter.is_empty() {
                start_page(browser, BrowseMode::Subscribed { items, page: 1 })?;
            } else {
                let last = items.len().min(50);
                let query = steam
                    .start_details(&items[..last], TIMEOUT)
                    .map_err(|error| error.to_string())?;
                browser.pending = Some(Pending::SubscriptionSearch(SubscriptionSearch {
                    query,
                    ids: items,
                    next: last,
                    matches: Vec::new(),
                    filter,
                }));
            }
            set_status(
                context,
                browser,
                "Reading subscribed Workshop item details.",
            )
        }
        Action::Select => {
            details(context, browser)?;
            update_buttons(context, browser)
        }
        Action::OpenWorkshop => {
            let item = selected(context, browser)?;
            let id = item
                .workshop
                .ok_or("This local mod has no verified Workshop page.")?;
            let url = context.string(&format!(
                "https://steamcommunity.com/sharedfiles/filedetails/?id={}",
                id.get()
            ))?;
            let overlay = first(context.call_global("isSteamOverlayEnabled", &[])?);
            let opener = if context.truth(&overlay)? {
                "activateSteamOverlayToWebPage"
            } else {
                "openUrl"
            };
            context.call_global(opener, &[url])?;
            feedback(
                context,
                browser,
                Feedback::Success,
                "Opened the selected item's original Workshop page.",
            )
        }
        Action::Previous | Action::Next => {
            if browser.search_changed.is_some() {
                return set_status(
                    context,
                    browser,
                    "Wait for the latest search text to finish before changing result pages.",
                );
            }
            if browser.pending.is_some() {
                return Err("Wait for the current operation or cancel its query.".into());
            }
            let mode = match &browser.mode {
                BrowseMode::Installed => {
                    return set_status(
                        context,
                        browser,
                        "All installed mod identities are in this list.",
                    );
                }
                BrowseMode::Search { text, page } => {
                    let next = if matches!(action, Action::Next) {
                        if *page * 50 >= browser.total_matching {
                            return set_status(context, browser, "This is the last result page.");
                        }
                        *page + 1
                    } else {
                        page.saturating_sub(1).max(1)
                    };
                    BrowseMode::Search {
                        text: text.clone(),
                        page: next,
                    }
                }
                BrowseMode::Subscribed { items, page } => {
                    let next = if matches!(action, Action::Next) {
                        if *page as usize * 50 >= items.len() {
                            return set_status(
                                context,
                                browser,
                                "This is the last subscription page.",
                            );
                        }
                        *page + 1
                    } else {
                        page.saturating_sub(1).max(1)
                    };
                    BrowseMode::Subscribed {
                        items: items.clone(),
                        page: next,
                    }
                }
                BrowseMode::FilteredSubscriptions {
                    items,
                    page,
                    filter,
                } => {
                    let next = if matches!(action, Action::Next) {
                        if *page as usize * 50 >= items.len() {
                            return set_status(
                                context,
                                browser,
                                "This is the last matching subscription page.",
                            );
                        }
                        *page + 1
                    } else {
                        page.saturating_sub(1).max(1)
                    };
                    let items = Rc::clone(items);
                    let filter = filter.clone();
                    show_filtered_subscriptions(context, browser, items, next, &filter)?;
                    return Ok(());
                }
            };
            start_page(browser, mode)?;
            set_status(context, browser, "Reading the requested Workshop page.")
        }
        Action::Add | Action::Download => {
            if browser.pending.is_some() {
                return Err("A Workshop operation is already running. Repeated clicks do not submit another operation.".into());
            }
            let item = selected(context, browser)?;
            let id = item
                .workshop
                .ok_or("This item has no verified Workshop identity.")?;
            let steam = browser.steam.as_ref().map_err(Clone::clone)?;
            browser.pending = Some(if matches!(action, Action::Add) {
                Pending::Subscribe(
                    steam
                        .subscribe(steam::UserIntent::subscribe(id), TIMEOUT)
                        .map_err(|e| e.to_string())?,
                )
            } else {
                Pending::Download(
                    steam
                        .download(steam::UserIntent::download(id), TIMEOUT)
                        .map_err(|e| e.to_string())?,
                )
            });
            set_status(
                context,
                browser,
                if matches!(action, Action::Add) {
                    "Subscription requested. Download starts only after Steam confirms the subscription."
                } else {
                    "Download requested. Installation, enablement and loading remain separate."
                },
            )
        }
        Action::Enable | Action::Disable => {
            require_clean_options(context)?;
            let item = selected(context, browser)?;
            let id = item.id.ok_or(
                "Select an installed mod identity from Installed after the download completes.",
            )?;
            if !item.available {
                return Err(
                    "The game reports this mod incompatible with the current version.".into(),
                );
            }
            let enabled = matches!(action, Action::Enable);
            if enabled && game::active(context)?.contains(&id) {
                return set_status(
                    context,
                    browser,
                    "The native game or server already owns this active mod. LuaBuddy will not load it again or change that selection.",
                );
            }
            if !enabled {
                ensure_no_enabled_dependents(context, &id)?;
            }
            let updates = if enabled {
                dependency_selection(context, &id)?
            } else {
                vec![id.clone()]
            };
            persist(context, &updates, enabled)?;
            let message = if enabled {
                if item.restart_assets {
                    "Late selection saved. This mod has startup-only components that late loading cannot activate. To enable them, use Next launch > Startup enable, then restart normally. Native startup selection is a separate setting."
                } else {
                    "Selection saved. Choose Load enabled to request live loading. Dependency checks still apply."
                }
            } else {
                "Selection disabled for the next environment. Any running code stays active until reconnect or restart."
            };
            details(context, browser)?;
            feedback(context, browser, Feedback::Success, message)
        }
        Action::Load => {
            let (kind, summary) = load_enabled(context, session)?;
            if selected(context, browser).is_ok() {
                details(context, browser)?;
            }
            feedback(context, browser, kind, &summary)
        }
        Action::StartupEnable | Action::StartupDisable => {
            require_clean_options(context)?;
            let item = selected(context, browser)?;
            if matches!(action, Action::StartupEnable) && !item.native_startup_available {
                return Err("This installed item uses a flat Workshop layout that the game's normal startup discovery does not expose. Use its reviewed client/shared profile or the upstream supported packaging.".into());
            }
            let id = item.id.ok_or(
                "Select an installed mod identity before changing the native startup selection.",
            )?;
            crate::activation::select_next_startup(
                context,
                std::slice::from_ref(&id),
                matches!(action, Action::StartupEnable),
            )?;
            details(context, browser)?;
            feedback(
                context,
                browser,
                Feedback::Success,
                "Native startup selection saved for the next normal launch. Restart when ready. The running game and server selections are unchanged.",
            )
        }
        Action::Cancel => match browser.pending.take() {
            Some(Pending::Query(mut query)) => {
                query.cancel().map_err(|e| e.to_string())?;
                feedback(
                    context,
                    browser,
                    Feedback::Success,
                    "Workshop query cancelled.",
                )
            }
            Some(Pending::SubscriptionSearch(mut search)) => {
                search.query.cancel().map_err(|error| error.to_string())?;
                feedback(
                    context,
                    browser,
                    Feedback::Success,
                    "Subscribed-item search cancelled.",
                )
            }
            Some(operation) => {
                browser.pending = Some(operation);
                Err("Steam cannot cancel this subscription or download. Close the browser to leave it running; verified progress remains available when reopened.".into())
            }
            None => set_status(context, browser, "There is no active Workshop query."),
        },
        Action::Close => close(context, &browser.panel),
        _ => Err("This action is unavailable in the browser".into()),
    }
}

fn selected(context: &mut Context<'_, '_>, browser: &Browser) -> Result<Item> {
    let index = context.get(&browser.list, "selected")?;
    let index = context.numeric(&index)? as usize;
    browser
        .items
        .get(index.saturating_sub(1))
        .cloned()
        .ok_or("Select a mod first".into())
}

fn start_page(browser: &mut Browser, mode: BrowseMode) -> Result<()> {
    browser.mode = mode;
    let steam = browser.steam.as_ref().map_err(Clone::clone)?;
    let query = match &browser.mode {
        BrowseMode::Search { text, page } => steam.start_search(text, *page, TIMEOUT),
        BrowseMode::Subscribed { items, page } => {
            let first = (*page as usize - 1) * 50;
            let last = (first + 50).min(items.len());
            steam.start_details(&items[first..last], TIMEOUT)
        }
        BrowseMode::Installed | BrowseMode::FilteredSubscriptions { .. } => {
            return Err("Installed mod identities do not need a Workshop query".into());
        }
    }
    .map_err(|e| e.to_string())?;
    browser.pending = Some(Pending::Query(query));
    Ok(())
}

fn render(context: &mut Context<'_, '_>, browser: &mut Browser) -> Result<()> {
    context.invoke(&browser.list, "clear", &[])?;
    for item in &browser.items {
        let title = context.string(&item.title)?;
        let id = context.string(
            item.id
                .as_ref()
                .map(ModId::as_str)
                .unwrap_or("Workshop item"),
        )?;
        let tooltip = context.string(&format!(
            "{}\nMod ID: {}\n{}",
            item.title,
            item.id
                .as_ref()
                .map(ModId::as_str)
                .unwrap_or("Workshop item"),
            item.source
        ))?;
        context.invoke(&browser.list, "addItem", &[title, id, tooltip])?;
    }
    browser.selected = usize::MAX;
    if browser.items.is_empty() {
        rich(context, &browser.details, "No matching mods.")?;
    } else {
        let one = context.number(1.0)?;
        context.set(&browser.list, "selected", &one)?;
        context.invoke(&browser.list, "ensureVisible", std::slice::from_ref(&one))?;
        details(context, browser)?;
    }
    Ok(())
}

fn details(context: &mut Context<'_, '_>, browser: &mut Browser) -> Result<()> {
    let item = selected(context, browser)?;
    let index = context.get(&browser.list, "selected")?;
    let index = context.numeric(&index)? as usize;
    if browser.selected != index {
        let top = context.number(0.0)?;
        context.invoke(&browser.details, "setYScroll", &[top])?;
    }
    browser.selected = index;
    let mut lines = vec![item.title, "".into(), "MOD INFORMATION".into()];
    if let Some(reason) = &item.unavailable_reason {
        if let Some(id) = item.workshop {
            lines.push(format!("Workshop ID: {}", id.get()));
        }
        lines.push(
            "Details unavailable. No subscription or download action is permitted for this result."
                .to_owned(),
        );
        lines.push(reason.clone());
        return rich(context, &browser.details, &lines.join("\n\n"));
    }
    if let Some(id) = &item.id {
        lines.push(format!("Mod ID: {id}"));
        lines.push(format!(
            "Mod version: {}",
            item.mod_version.as_deref().unwrap_or("not reported")
        ));
        lines.push(format!(
            "Minimum game version: {}",
            item.minimum_game_version
                .as_deref()
                .unwrap_or("not specified")
        ));
        lines.push(format!(
            "Maximum game version: {}",
            item.maximum_game_version
                .as_deref()
                .unwrap_or("not specified")
        ));
        let profiles = configured(context)?;
        let enabled = profiles
            .iter()
            .find(|profile| &profile.id == id)
            .map(|profile| profile.enabled)
            .unwrap_or(false);
        let state = context.global("LuaBuddy")?;
        let loaded = if state.is_nil() {
            Value::default()
        } else {
            let mods = context.get(&state, "mods")?;
            context.get(&mods, id.as_str())?
        };
        lines.push("\nLOADING STATE".into());
        lines.push(format!(
            "Late selection: {}",
            if enabled { "enabled" } else { "disabled" }
        ));
        if crate::selections::temporary(context, id)? {
            lines.push("Selection lifetime: this game process, including reconnects. The temporary registration is already removed; the source mod is preserved.".to_owned());
        }
        if crate::selections::queued(context)?
            .iter()
            .any(|profile| &profile.id == id)
        {
            lines.push("Next LuaBuddy launch: queued for one game process, including reconnects. This does not activate it in the current process or select native startup.".to_owned());
        }
        let startup = crate::activation::selected_next_startup(context)?;
        lines.push(format!(
            "Next normal launch: {}",
            if startup.contains(id) {
                "enabled"
            } else {
                "disabled"
            }
        ));
        if !item.native_startup_available {
            lines.push("Native startup discovery does not expose this flat Workshop layout. A reviewed client/shared profile can still restore eligible Lua.".to_owned());
        }
        let native_active = game::active(context)?.contains(id);
        if native_active {
            lines.push(
                "Native game/server selection: active. LuaBuddy does not own this activation."
                    .into(),
            );
        }
        lines.push(format!(
            "Runtime: {}",
            if native_active {
                "native-loaded".to_owned()
            } else if loaded.is_nil() {
                if enabled {
                    "selected, waiting for Load enabled or a new environment".to_owned()
                } else {
                    "not loaded by LuaBuddy".to_owned()
                }
            } else {
                context.text(&loaded)?
            }
        ));
    }
    if let Some(id) = item.workshop {
        lines.push("\nWORKSHOP DELIVERY".into());
        lines.push(format!("Workshop ID: {}", id.get()));
        if let Some(updated) = item.workshop_updated_at {
            let timestamp = context.jni(|env| {
                let value = env
                    .call_static_method(
                        "java/time/Instant",
                        "ofEpochSecond",
                        "(J)Ljava/time/Instant;",
                        &[JValue::Long(i64::from(updated))],
                    )?
                    .l()?;
                env.new_global_ref(value)
                    .map(|reference| Value(Some(reference)))
            })?;
            lines.push(format!("Workshop updated: {}", context.text(&timestamp)?));
        }
        lines.push(match item.size_bytes {
            Some(bytes) => format!("Content size: {bytes} bytes"),
            None => "Content size: not reported".to_owned(),
        });
        if let Ok(steam) = &browser.steam {
            let state = steam.item_state(id).map_err(|e| e.to_string())?;
            lines.push(format!(
                "Delivery: {}{}{}{}",
                if state.subscribed {
                    "subscribed"
                } else {
                    "not subscribed"
                },
                if state.installed {
                    ", installed"
                } else {
                    ", not installed"
                },
                if state.downloading || state.download_pending {
                    ", downloading"
                } else {
                    ""
                },
                if state.needs_update {
                    ", update required"
                } else {
                    ""
                }
            ));
        }
    }
    lines.push("\nREQUIREMENTS & ACTIVATION".into());
    lines.push(format!(
        "Compatibility: {}",
        if item.id.is_none() {
            "inspect the installed mod identities for game-version compatibility"
        } else if item.available {
            "accepted by native metadata"
        } else {
            "incompatible with the current game version"
        }
    ));
    if !item.required.is_empty() {
        lines.push(format!(
            "Required mods: {}",
            item.required
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if !item.workshop_dependencies.is_empty() {
        lines.push(format!(
            "Workshop dependencies: {}",
            item.workshop_dependencies
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if !item.activation_explanation.is_empty() {
        lines.push(item.activation_explanation);
    }
    lines.push(format!("\nSOURCE\n{}", item.source));
    lines.push(format!(
        "\nABOUT THIS MOD\n{}",
        mod_description(&item.description)
    ));
    rich(context, &browser.details, &lines.join("\n"))
}

fn mod_description(text: &str) -> String {
    let mut result = String::new();
    let mut remaining = text;
    let mut image = false;
    while let Some(start) = remaining.find('[') {
        if !image {
            result.push_str(&remaining[..start]);
        }
        remaining = &remaining[start..];
        let Some(end) = remaining.find(']') else {
            break;
        };
        let raw = &remaining[1..end];
        let tag = raw.to_ascii_lowercase();
        let name = tag.split('=').next().unwrap_or("");
        let recognized = match name {
            "img" => {
                image = true;
                result.push_str("[Image on original Workshop page]");
                true
            }
            "/img" => {
                image = false;
                true
            }
            "h1" | "h2" | "h3" | "/h1" | "/h2" | "/h3" | "list" | "/list" | "olist" | "/olist"
            | "quote" | "/quote" | "code" | "/code" | "tr" | "/tr" => {
                result.push('\n');
                true
            }
            "*" => {
                result.push_str("\n• ");
                true
            }
            "b" | "/b" | "i" | "/i" | "u" | "/u" | "s" | "/s" | "url" | "/url" | "spoiler"
            | "/spoiler" | "table" | "/table" | "td" | "/td" => true,
            _ => false,
        };
        if !recognized && !image {
            result.push_str(&remaining[..=end]);
        }
        remaining = &remaining[end + 1..];
    }
    if !image {
        result.push_str(remaining);
    }
    let result = result.trim();
    if result.is_empty() {
        "No description was provided by the installed mod or Workshop item.".into()
    } else {
        result.into()
    }
}

fn configured(context: &mut Context<'_, '_>) -> Result<Vec<Profile>> {
    crate::selections::configured(context)
}

fn installed(context: &mut Context<'_, '_>, browser: &mut Browser) -> Result<()> {
    if browser.pending.is_some() {
        return Err("Wait for the current operation or cancel its query.".into());
    }
    game::reset_discovery(context)?;
    browser.installed_catalog = installed_items(context)?;
    browser.mode = BrowseMode::Installed;
    filter_installed(context, browser)
}

fn filter_installed(context: &mut Context<'_, '_>, browser: &mut Browser) -> Result<()> {
    let value = first(context.invoke(&browser.search, "getText", &[])?);
    let filter = context.text(&value)?.trim().to_lowercase();
    browser.items = browser
        .installed_catalog
        .iter()
        .filter(|item| filter.is_empty() || matches_search(item, &filter))
        .cloned()
        .collect();
    render(context, browser)?;
    set_status(
        context,
        browser,
        &format!(
            "Installed selected. {} matching mod identities from {}. Type to filter; select a mod and choose an action tab.",
            browser.items.len(),
            browser.installed_catalog.len()
        ),
    )
}

fn installed_items(context: &mut Context<'_, '_>) -> Result<Vec<Item>> {
    let profiles = configured(context)?;
    let templates = template_profiles(context)?;
    let filesystem = game::instance(context)?;
    let folders = context.jni(|env| {
        let list = env.new_object("java/util/ArrayList", "()V", &[])?;
        env.call_method(
            filesystem.object(),
            "getAllModFolders",
            "(Ljava/util/List;)V",
            &[JValue::Object(&list)],
        )?;
        env.new_global_ref(list)
            .map(|reference| Value(Some(reference)))
    })?;
    let mut folders = game::java_list(context, &folders)?;
    let native_roots = folders
        .iter()
        .map(|folder| context.text(folder))
        .collect::<Result<BTreeSet<_>>>()?;
    let mut flat_roots = BTreeSet::new();
    let mut unavailable = Vec::new();
    let steam = context.jni(|env| {
        env.call_static_method(
            "zombie/core/znet/SteamUtils",
            "isSteamModeEnabled",
            "()Z",
            &[],
        )?
        .z()
    })?;
    if steam {
        let installed = context.jni(|env| {
            let workshop = env
                .get_static_field(
                    "zombie/core/znet/SteamWorkshop",
                    "instance",
                    "Lzombie/core/znet/SteamWorkshop;",
                )?
                .l()?;
            let array = env
                .call_method(
                    workshop,
                    "GetInstalledItemFolders",
                    "()[Ljava/lang/String;",
                    &[],
                )?
                .l()?;
            if array.is_null() {
                return Ok(Vec::new());
            }
            let array = JObjectArray::from(array);
            let mut folders = Vec::new();
            for index in 0..env.get_array_length(&array)? {
                let folder = env.get_object_array_element(&array, index)?;
                if !folder.is_null() {
                    folders.push(Value(Some(env.new_global_ref(folder)?)));
                }
            }
            Ok(folders)
        })?;
        for folder in installed {
            let source = context.text(&folder)?;
            let root = PathBuf::from(&source);
            if !root.join("mod.info").is_file() || !flat_roots.insert(source.clone()) {
                continue;
            }
            match game::repair_flat_root(context, &root) {
                Ok(()) => folders.push(folder),
                Err(error) => unavailable.push(unavailable_installed(&source, error)),
            }
        }
    }
    let mut result = BTreeMap::new();
    for folder in folders {
        let source = context.text(&folder)?;
        let info = context.jni(|env| {
            let object = env
                .call_static_method(
                    "zombie/gameStates/ChooseGameInfo",
                    "readModInfo",
                    "(Ljava/lang/String;)Lzombie/gameStates/ChooseGameInfo$Mod;",
                    &[JValue::Object(folder.object())],
                )?
                .l()?;
            if object.is_null() {
                Ok(Value::default())
            } else {
                env.new_global_ref(object)
                    .map(|reference| Value(Some(reference)))
            }
        });
        let info = match info {
            Ok(info) => info,
            Err(error) if flat_roots.contains(&source) => {
                unavailable.push(unavailable_installed(&source, error));
                continue;
            }
            Err(error) => return Err(error),
        };
        if info.is_nil() {
            if flat_roots.contains(&source) {
                unavailable.push(unavailable_installed(
                    &source,
                    "The game's native metadata parser rejected the installed root mod.info"
                        .to_owned(),
                ));
            }
            continue;
        }
        let identity = java_string(context, &info, "getId")?;
        let id = ModId::new(identity).map_err(|e| e.to_string())?;
        if result.contains_key(&id) {
            continue;
        }
        let title = java_string(context, &info, "getName")?;
        let description = java_string(context, &info, "getDescription")?;
        let available = context.jni(|env| {
            env.call_method(info.object(), "isAvailableSelf", "()Z", &[])?
                .z()
        })?;
        let required = context.jni(|env| {
            let object = env
                .call_method(info.object(), "getRequire", "()Ljava/util/ArrayList;", &[])?
                .l()?;
            if object.is_null() {
                Ok(Value::default())
            } else {
                env.new_global_ref(object)
                    .map(|reference| Value(Some(reference)))
            }
        })?;
        let mut requires = Vec::new();
        for value in game::java_list(context, &required)? {
            requires.push(ModId::new(context.text(&value)?).map_err(|e| e.to_string())?);
        }
        let mut roots = Vec::new();
        for method in ["getCommonDir", "getVersionDir"] {
            let root = java_string(context, &info, method)?;
            let root = PathBuf::from(root);
            if !roots.contains(&root) {
                roots.push(root);
            }
        }
        let profile = match profiles.iter().find(|profile| profile.id == id) {
            Some(profile) => profile.clone(),
            None => Profile::parse(
                Path::new("LuaBuddy/browser.properties"),
                &format!("id={}\n", escape_property(id.as_str())),
            )
            .map_err(|error| error.to_string())?,
        };
        let eligibility = crate::activation::assess(&profile, &roots, &templates)?;
        let restart_assets = matches!(
            &eligibility,
            crate::activation::Eligibility::RestartRequired { .. }
        );
        let activation_explanation = eligibility.explanation();
        let workshop = java_string(context, &info, "getWorkshopID")?
            .parse()
            .ok()
            .and_then(|id| steam::ItemId::new(id).ok())
            .or_else(|| workshop_id_from_source(&source));
        let mod_version = java_string(context, &info, "getModVersion")?;
        let mod_version = (!mod_version.is_empty()).then_some(mod_version);
        let minimum_game_version = game_version(context, &info, "getVersionMin")?;
        let maximum_game_version = game_version(context, &info, "getVersionMax")?;
        let native_startup_available = native_roots.contains(&source);
        result.insert(
            id.clone(),
            Item {
                id: Some(id),
                workshop,
                title,
                description,
                required: requires,
                workshop_dependencies: Vec::new(),
                source,
                available,
                restart_assets,
                activation_explanation,
                native_startup_available,
                mod_version,
                minimum_game_version,
                maximum_game_version,
                workshop_updated_at: None,
                size_bytes: None,
                unavailable_reason: None,
            },
        );
    }
    Ok(result.into_values().chain(unavailable).collect())
}

fn unavailable_installed(source: &str, reason: String) -> Item {
    let workshop = workshop_id_from_source(source);
    Item {
        id: None,
        workshop,
        title: workshop
            .map(|id| format!("Unavailable installed item {}", id.get()))
            .unwrap_or_else(|| "Unavailable installed mod".to_owned()),
        description: String::new(),
        required: Vec::new(),
        workshop_dependencies: Vec::new(),
        source: source.to_owned(),
        available: false,
        restart_assets: false,
        activation_explanation: String::new(),
        native_startup_available: false,
        mod_version: None,
        minimum_game_version: None,
        maximum_game_version: None,
        workshop_updated_at: None,
        size_bytes: None,
        unavailable_reason: Some(reason),
    }
}

fn game_version(
    context: &mut Context<'_, '_>,
    object: &Value,
    method: &str,
) -> Result<Option<String>> {
    let value = context.jni(|env| {
        let value = env
            .call_method(object.object(), method, "()Lzombie/core/GameVersion;", &[])?
            .l()?;
        if value.is_null() {
            Ok(Value::default())
        } else {
            env.new_global_ref(value)
                .map(|reference| Value(Some(reference)))
        }
    })?;
    if value.is_nil() {
        Ok(None)
    } else {
        context.text(&value).map(Some)
    }
}

fn java_string(context: &mut Context<'_, '_>, object: &Value, method: &str) -> Result<String> {
    let value = context.jni(|env| {
        let object = env
            .call_method(object.object(), method, "()Ljava/lang/String;", &[])?
            .l()?;
        if object.is_null() {
            Ok(Value::default())
        } else {
            env.new_global_ref(object)
                .map(|reference| Value(Some(reference)))
        }
    })?;
    if value.is_nil() {
        Ok(String::new())
    } else {
        context.text(&value)
    }
}

fn workshop_id_from_source(source: &str) -> Option<steam::ItemId> {
    let normalized = source.replace('\\', "/");
    let (_, tail) = normalized.split_once("/workshop/content/108600/")?;
    steam::ItemId::new(tail.split('/').next()?.parse().ok()?).ok()
}

fn dependency_selection(context: &mut Context<'_, '_>, selected: &ModId) -> Result<Vec<ModId>> {
    let installed = installed_items(context)?;
    let available: BTreeMap<_, _> = installed
        .into_iter()
        .filter_map(|item| item.id.clone().map(|id| (id, item)))
        .collect();
    let profiles = configured(context)?;
    let active = game::active(context)?;
    let mut result = BTreeSet::new();
    let mut visiting = BTreeSet::new();
    fn visit(
        id: &ModId,
        available: &BTreeMap<ModId, Item>,
        profiles: &[Profile],
        active: &BTreeSet<ModId>,
        visiting: &mut BTreeSet<ModId>,
        result: &mut BTreeSet<ModId>,
    ) -> Result<()> {
        if active.contains(id) || result.contains(id) {
            return Ok(());
        }
        if !visiting.insert(id.clone()) {
            return Err(format!("Required mod dependency cycle includes {id}"));
        }
        let item = available
            .get(id)
            .ok_or_else(|| format!("Required mod {id} is not installed"))?;
        if !item.available {
            return Err(format!(
                "Required mod {id} is incompatible with the current game"
            ));
        }
        let configured = profiles.iter().find(|profile| &profile.id == id);
        for required in item
            .required
            .iter()
            .chain(configured.into_iter().flat_map(|profile| &profile.requires))
        {
            visit(required, available, profiles, active, visiting, result)?;
        }
        visiting.remove(id);
        result.insert(id.clone());
        Ok(())
    }
    visit(
        selected,
        &available,
        &profiles,
        &active,
        &mut visiting,
        &mut result,
    )?;
    Ok(result.into_iter().collect())
}

fn ensure_no_enabled_dependents(context: &mut Context<'_, '_>, target: &ModId) -> Result<()> {
    let profiles = configured(context)?;
    let installed = installed_items(context)?;
    let dependents = profiles
        .iter()
        .filter(|profile| {
            profile.enabled
                && profile.id != *target
                && (profile.requires.contains(target)
                    || installed
                        .iter()
                        .find(|item| item.id.as_ref() == Some(&profile.id))
                        .is_some_and(|item| item.required.contains(target)))
        })
        .map(|profile| profile.id.to_string())
        .collect::<Vec<_>>();
    if dependents.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "Disable dependent profiles first: {}",
            dependents.join(", ")
        ))
    }
}

fn persist(context: &mut Context<'_, '_>, ids: &[ModId], enabled: bool) -> Result<()> {
    let queued = crate::selections::queued(context)?;
    for id in ids {
        if crate::selections::temporary(context, id)? {
            return Err(format!(
                "{id} is selected for this game process only. It expires automatically at exit; changing a running mod is not a hot unload. Use a normal selection after the next launch if desired."
            ));
        }
        if queued.iter().any(|profile| &profile.id == id) {
            return Err(format!(
                "{id} is queued for one future game process. Cancel that registration with luabuddy deselect --profiles-dir DIR --mod ID --once before creating a persistent late selection. No selection was changed."
            ));
        }
    }
    let directory = PathBuf::from(
        game::property(context, "luabuddy.config")?.ok_or("Missing LuaBuddy profile directory")?,
    );
    for ancestor in directory.ancestors() {
        if fs::symlink_metadata(ancestor).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
            return Err(format!(
                "Refusing a symlink in profile destination {}",
                ancestor.display()
            ));
        }
    }
    let existing = configured(context)?;
    let templates = template_profiles(context)?;
    let mut changes = Vec::new();
    for id in ids {
        let profile = existing.iter().find(|profile| &profile.id == id);
        if profile.is_some_and(|profile| profile.enabled == enabled) {
            continue;
        }
        let path = match profile {
            Some(profile) => profile.file.clone(),
            None => directory.join(luabuddy_core::profile::profile_filename(id.as_str())),
        };
        let filename = path
            .file_name()
            .ok_or("Profile destination has no filename")?
            .to_string_lossy();
        for entry in fs::read_dir(&directory)
            .map_err(|error| format!("Inspect profile directory: {error}"))?
        {
            let entry = entry.map_err(|error| format!("Inspect profile filename: {error}"))?;
            if entry.path() != path
                && entry
                    .file_name()
                    .to_string_lossy()
                    .eq_ignore_ascii_case(&filename)
            {
                return Err(format!(
                    "Profile filename {} collides with {}; no selection changed",
                    path.display(),
                    entry.path().display()
                ));
            }
        }
        if path.parent() != Some(directory.as_path())
            || fs::symlink_metadata(&path)
                .is_ok_and(|metadata| !metadata.is_file() || metadata.file_type().is_symlink())
        {
            return Err(format!(
                "Refusing unsafe profile destination {}",
                path.display()
            ));
        }
        let original = match fs::read(&path) {
            Ok(value) => Some(value),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(format!("Read {}: {error}", path.display())),
        };
        if profile.is_none() && original.is_some() {
            return Err(format!(
                "Profile filename collision at {}; no selection changed",
                path.display()
            ));
        }
        let mut content = if let Some(bytes) = &original {
            String::from_utf8(bytes.clone()).map_err(|e| e.to_string())?
        } else if let Some(template) = templates.iter().find(|profile| &profile.id == id) {
            fs::read_to_string(&template.file).map_err(|e| e.to_string())?
        } else {
            format!("id={}\n", escape_property(id.as_str()))
        };
        let before = Profile::parse(&path, &content).map_err(|e| e.to_string())?;
        if !content.ends_with('\n') {
            content.push('\n');
        }
        content.push('\n');
        content.push_str(if enabled {
            "enabled=true\n"
        } else {
            "enabled=false\n"
        });
        let candidate = Profile::parse(&path, &content).map_err(|e| e.to_string())?;
        let mut expected = before;
        expected.enabled = enabled;
        if candidate.id != *id || candidate != expected {
            return Err(format!(
                "Profile settings changed unexpectedly while editing {}",
                path.display()
            ));
        }
        changes.push((path, original, content.into_bytes()));
    }
    write_profiles(changes)?;
    let saved = read_profiles(&directory).map_err(|e| e.to_string())?;
    for id in ids {
        if saved
            .iter()
            .find(|profile| &profile.id == id)
            .is_none_or(|profile| profile.enabled != enabled)
        {
            return Err(format!(
                "Profile selection for {id} changed before it could be verified. Review its current configuration."
            ));
        }
    }
    Ok(())
}

fn escape_property(value: &str) -> String {
    let mut escaped = String::new();
    for character in value.chars() {
        if matches!(character, '\\' | ' ' | ':' | '=' | '#' | '!') {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped
}

fn template_profiles(context: &mut Context<'_, '_>) -> Result<Vec<Profile>> {
    let path = game::property(context, "luabuddy.templates")?.map(PathBuf::from);
    match path {
        Some(path) if path.is_dir() => read_profiles(&path).map_err(|error| error.to_string()),
        _ => Ok(Vec::new()),
    }
}

fn write_profiles(changes: Vec<(PathBuf, Option<Vec<u8>>, Vec<u8>)>) -> Result<()> {
    let mut staged = Vec::new();
    let result = (|| {
        for (index, (path, original, content)) in changes.iter().enumerate() {
            let temporary =
                path.with_extension(format!("luabuddy-{}-{index}.tmp", std::process::id()));
            let mut file = OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&temporary)
                .map_err(|e| format!("Stage {}: {e}", path.display()))?;
            staged.push((path.clone(), temporary.clone(), original.clone()));
            if original.is_some() {
                let permissions = fs::metadata(path)
                    .map_err(|e| format!("Inspect {}: {e}", path.display()))?
                    .permissions();
                file.set_permissions(permissions)
                    .map_err(|e| format!("Preserve permissions {}: {e}", path.display()))?;
            }
            file.write_all(content)
                .and_then(|()| file.sync_all())
                .map_err(|e| format!("Stage {}: {e}", path.display()))?;
        }
        for (index, (path, temporary, _)) in staged.iter().enumerate() {
            let observed = match fs::read(path) {
                Ok(value) => Ok(Some(value)),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(error) => Err(format!("Check {}: {error}", path.display())),
            };
            let result = observed.and_then(|observed| if observed != changes[index].1 { Err(format!("Profile {} changed while the selection was being saved; retry after reviewing it", path.display())) } else { fs::rename(temporary, path).map_err(|e| e.to_string()) });
            if let Err(error) = result {
                let mut failures = Vec::new();
                for (earlier, (path, _, original)) in staged[..index].iter().enumerate().rev() {
                    if fs::read(path).ok().as_ref() != Some(&changes[earlier].2) {
                        failures.push(format!(
                            "{} was modified after saving and was preserved",
                            path.display()
                        ));
                        continue;
                    }
                    let restored = match original {
                        Some(bytes) => fs::write(path, bytes),
                        None => fs::remove_file(path),
                    };
                    if let Err(error) = restored {
                        failures.push(format!("{}: {error}", path.display()));
                    }
                }
                return Err(format!(
                    "Save {}: {error}. Rollback failures: {}",
                    path.display(),
                    failures.join("; ")
                ));
            }
        }
        Ok(())
    })();
    for (_, temporary, _) in staged {
        let _ = fs::remove_file(temporary);
    }
    result
}

fn matches_search(item: &Item, filter: &str) -> bool {
    item.title.to_lowercase().contains(filter)
        || item.description.to_lowercase().contains(filter)
        || item
            .id
            .as_ref()
            .is_some_and(|id| id.as_str().to_lowercase().contains(filter))
        || item
            .workshop
            .is_some_and(|id| id.get().to_string().contains(filter))
        || item
            .unavailable_reason
            .as_ref()
            .is_some_and(|reason| reason.to_lowercase().contains(filter))
}

fn workshop_items(results: steam::SearchResults) -> Vec<Item> {
    let mut items: Vec<_> = results
        .items
        .into_iter()
        .map(|item| Item {
            id: None,
            workshop: Some(item.id),
            title: item.title,
            description: item.description,
            required: Vec::new(),
            workshop_dependencies: item.children.into_iter().map(steam::ItemId::get).collect(),
            source: "Steam Workshop".into(),
            available: false,
            restart_assets: false,
            activation_explanation: String::new(),
            native_startup_available: false,
            mod_version: None,
            minimum_game_version: None,
            maximum_game_version: None,
            workshop_updated_at: Some(item.updated_at),
            size_bytes: item.size_bytes,
            unavailable_reason: None,
        })
        .collect();
    items.extend(results.unavailable.into_iter().map(|item| {
        Item {
            id: None,
            workshop: item.id,
            title: item
                .id
                .map(|id| format!("Unavailable Workshop item {}", id.get()))
                .unwrap_or_else(|| format!("Unavailable Workshop result {}", item.index + 1)),
            description: String::new(),
            required: Vec::new(),
            workshop_dependencies: Vec::new(),
            source: "Steam Workshop".to_owned(),
            available: false,
            restart_assets: false,
            activation_explanation: String::new(),
            native_startup_available: false,
            mod_version: None,
            minimum_game_version: None,
            maximum_game_version: None,
            workshop_updated_at: None,
            size_bytes: None,
            unavailable_reason: Some(item.reason),
        }
    }));
    items
}

fn show_filtered_subscriptions(
    context: &mut Context<'_, '_>,
    browser: &mut Browser,
    items: Rc<Vec<Item>>,
    page: u32,
    filter: &str,
) -> Result<()> {
    let first = ((page.saturating_sub(1) as usize) * 50).min(items.len());
    let last = (first + 50).min(items.len());
    browser.items = items[first..last].to_vec();
    let total = items.len();
    browser.mode = BrowseMode::FilteredSubscriptions {
        items,
        page,
        filter: filter.to_owned(),
    };
    render(context, browser)?;
    set_status(
        context,
        browser,
        &format!(
            "Page {page}, {} results for \"{filter}\" from {total} matching subscribed items. All subscription pages were searched.",
            last - first
        ),
    )
}

pub fn tick(context: &mut Context<'_, '_>) -> Result<()> {
    let session = UI.with(|state| state.borrow().as_ref().map(|ui| ui.session));
    let Some(session) = session else {
        return Ok(());
    };
    let present = UI.with(|state| {
        state
            .borrow()
            .as_ref()
            .is_some_and(|ui| ui.browser.is_some())
    });
    if !present {
        return Ok(());
    }
    let result = with_browser(context, session, |context, browser| {
        search_as_you_type(context, session, browser)?;
        if Instant::now() < browser.next_poll {
            return Ok(());
        }
        browser.next_poll = Instant::now() + Duration::from_millis(150);
        input_hint(context, &browser.panel)?;
        update_buttons(context, browser)?;
        let pending = browser.pending.take();
        if let Some(mut pending) = pending {
            let mut completion = Feedback::Success;
            let result = match &mut pending {
                Pending::Query(query) => match query.poll().map_err(|e| e.to_string())? {
                    steam::Poll::Pending => {
                        browser.pending = Some(pending);
                        None
                    }
                    steam::Poll::Ready(results) => {
                        let count = results.items.len();
                        let unavailable = results.unavailable.len();
                        if unavailable > 0 {
                            completion = Feedback::Warning;
                        }
                        browser.total_matching = results.total_matching;
                        let total_matching = results.total_matching;
                        browser.items = workshop_items(results);
                        render(context, browser)?;
                        let (page, total) = match &browser.mode {
                            BrowseMode::Search { page, .. } => (*page, total_matching as usize),
                            BrowseMode::Subscribed { items, page } => (*page, items.len()),
                            BrowseMode::Installed => (1, count),
                            BrowseMode::FilteredSubscriptions { items, page, .. } => {
                                (*page, items.len())
                            }
                        };
                        Some(format!(
                            "Page {page}, {count} available and {unavailable} unavailable results from {total} matches. Downloaded content is not automatically enabled or loaded."
                        ))
                    }
                },
                Pending::SubscriptionSearch(search) => {
                    match search.query.poll().map_err(|error| error.to_string())? {
                        steam::Poll::Pending => {
                            browser.pending = Some(pending);
                            None
                        }
                        steam::Poll::Ready(results) => {
                            search.matches.extend(
                                workshop_items(results)
                                    .into_iter()
                                    .filter(|item| matches_search(item, &search.filter)),
                            );
                            if search.next < search.ids.len() {
                                let first = search.next;
                                let last = (first + 50).min(search.ids.len());
                                search.query = browser
                                    .steam
                                    .as_ref()
                                    .map_err(Clone::clone)?
                                    .start_details(&search.ids[first..last], TIMEOUT)
                                    .map_err(|error| error.to_string())?;
                                search.next = last;
                                let message = format!(
                                    "Searching subscribed item details, {first} of {} inspected. Matching results remain pending until all pages complete.",
                                    search.ids.len()
                                );
                                browser.pending = Some(pending);
                                Some(message)
                            } else {
                                let items = Rc::new(std::mem::take(&mut search.matches));
                                show_filtered_subscriptions(
                                    context,
                                    browser,
                                    items,
                                    1,
                                    &search.filter,
                                )?;
                                None
                            }
                        }
                    }
                }
                Pending::Subscribe(subscription) => {
                    match subscription.poll().map_err(|e| e.to_string())? {
                        steam::Poll::Pending => {
                            browser.pending = Some(pending);
                            Some("Waiting for Steam to confirm the subscription.".into())
                        }
                        steam::Poll::Ready(_) => {
                            let id = subscription.item();
                            let steam = browser.steam.as_ref().map_err(Clone::clone)?;
                            browser.pending = Some(Pending::Download(
                                steam
                                    .download(steam::UserIntent::download(id), TIMEOUT)
                                    .map_err(|e| e.to_string())?,
                            ));
                            Some("Subscription confirmed. Download requested; installation is not yet verified.".into())
                        }
                    }
                }
                Pending::Download(download) => match download.poll().map_err(|e| e.to_string())? {
                    steam::Poll::Pending => {
                        let progress = download.progress().map_err(|e| e.to_string())?;
                        let text = format!(
                            "Downloading {} of {} bytes. Loading remains pending.",
                            progress
                                .downloaded_bytes
                                .map(|n| n.to_string())
                                .unwrap_or_else(|| "unknown".into()),
                            progress
                                .total_bytes
                                .map(|n| n.to_string())
                                .unwrap_or_else(|| "unknown".into())
                        );
                        browser.pending = Some(pending);
                        Some(text)
                    }
                    steam::Poll::Ready(progress) => Some(format!(
                        "Installation verified at {}. Choose Installed, inspect the mod identity and dependencies, then Enable and Load enabled if eligible.",
                        progress
                            .install_folder
                            .map(|p| p.display().to_string())
                            .unwrap_or_else(|| "the Workshop content directory".into())
                    )),
                },
            };
            if let Some(text) = result {
                feedback(
                    context,
                    browser,
                    if browser.pending.is_some() {
                        Feedback::Working
                    } else {
                        completion
                    },
                    &text,
                )?;
            }
        }
        let selected = context.get(&browser.list, "selected")?;
        let selected = context.numeric(&selected)? as usize;
        if (1..=browser.items.len()).contains(&selected) && selected != browser.selected {
            details(context, browser)?;
        }
        Ok(())
    });
    if let Err(error) = result {
        game::log(&format!("Workshop operation failed: {error}"));
        with_browser(context, session, |context, browser| {
            feedback(
                context,
                browser,
                Feedback::Error,
                &format!("{error}. Retry the requested action after correcting the cause."),
            )
        })?;
    }
    Ok(())
}

fn options_joypad(context: &mut Context<'_, '_>, main: &Value) -> Result<Value> {
    let state = context.global("JoypadState")?;
    let native = context.get(&state, "getMainMenuJoypad")?;
    let data = first(context.call(&native, &[])?);
    if data.is_nil() {
        context.get(main, "joyfocus")
    } else {
        Ok(data)
    }
}

fn modal_input(context: &mut Context<'_, '_>, session: u64, panel: &Value) -> Result<()> {
    let class = context.global("MainOptions")?;
    let main = if class.is_nil() {
        Value::default()
    } else {
        context.get(&class, "instance")?
    };
    if main.is_nil() {
        return Ok(());
    }
    let modal = context.get(&main, "modal")?;
    if !context.same(&modal, panel)? {
        return Ok(());
    }
    let due = UI.with(|state| {
        let mut state = state.borrow_mut();
        let Some(ui) = state.as_mut().filter(|ui| ui.session == session) else {
            return false;
        };
        if Instant::now() < ui.next_focus_check {
            return false;
        }
        ui.next_focus_check = Instant::now() + Duration::from_millis(150);
        true
    });
    if !due {
        return Ok(());
    }
    let data = options_joypad(context, &main)?;
    if data.is_nil() {
        return Ok(());
    }
    let active = context.get(&data, "isActive")?;
    let current = context.get(&data, "focus")?;
    let focus = context.get(panel, "joyfocus")?;
    if !context.truth(&active)? {
        if !focus.is_nil() {
            context.invoke(panel, "onLoseJoypadFocus", &[data])?;
        }
        return Ok(());
    }
    if context.same(&current, panel)? {
        if focus.is_nil() {
            context.invoke(panel, "onGainJoypadFocus", &[data])?;
        }
        return Ok(());
    }
    let previous = context.get(panel, "luabuddyPreviousModal")?;
    let mut belongs = false;
    for parent in [&main, &previous] {
        if parent.is_nil() {
            continue;
        }
        let owned = first(context.invoke(
            &data,
            "isFocusOnElementOrDescendant",
            std::slice::from_ref(parent),
        )?);
        belongs |= context.truth(&owned)?;
    }
    if belongs {
        context.set(panel, "prevFocus", &current)?;
        context.set(&data, "focus", panel)?;
        context.call_global("updateJoypadFocus", &[data])?;
    }
    Ok(())
}

fn show(context: &mut Context<'_, '_>, session: u64, panel: &Value) -> Result<()> {
    let wrapped = context.get(panel, "luabuddyUpdateWrapped")?;
    if !context.truth(&wrapped)? {
        let original = context.index(panel, "update")?;
        context.set(panel, "luabuddyOriginalUpdate", &original)?;
        let update = callback(context, session, Action::Update)?;
        context.set(panel, "update", &update)?;
        let yes = context.boolean(true)?;
        context.set(panel, "luabuddyUpdateWrapped", &yes)?;
    }
    let original = context.index(panel, "destroy")?;
    if !original.is_nil() && context.get(panel, "luabuddyOriginalDestroy")?.is_nil() {
        context.set(panel, "luabuddyOriginalDestroy", &original)?;
        let destroy = callback(context, session, Action::Destroy)?;
        context.set(panel, "destroy", &destroy)?;
    }
    let key = callback(context, session, Action::Key)?;
    context.set(panel, "onKeyRelease", &key)?;
    let consumed = callback(context, session, Action::Consumed)?;
    context.set(panel, "isKeyConsumed", &consumed)?;
    let yes = context.boolean(true)?;
    for method in ["setCapture", "setAlwaysOnTop", "setWantKeyEvents"] {
        context.invoke(panel, method, std::slice::from_ref(&yes))?;
    }
    context.invoke(panel, "addToUIManager", &[])?;
    let class = context.global("MainOptions")?;
    if !class.is_nil() {
        let main = context.get(&class, "instance")?;
        if !main.is_nil() {
            let previous = context.get(&main, "modal")?;
            context.set(panel, "luabuddyPreviousModal", &previous)?;
            context.set(&main, "modal", panel)?;
            UI.with(|state| {
                if let Some(ui) = state
                    .borrow_mut()
                    .as_mut()
                    .filter(|ui| ui.session == session)
                {
                    ui.next_focus_check = Instant::now();
                }
            });
            modal_input(context, session, panel)?;
        }
    }
    Ok(())
}

fn close(context: &mut Context<'_, '_>, panel: &Value) -> Result<()> {
    let visible = first(context.invoke(panel, "getIsVisible", &[])?);
    if !context.truth(&visible)? {
        return Ok(());
    }
    dismiss_reload_picker(context, panel)?;
    let entry = context.get(panel, "entry")?;
    if !entry.is_nil() {
        context.invoke(&entry, "unfocus", &[])?;
    }
    let original = context.get(panel, "luabuddyOriginalDestroy")?;
    if !original.is_nil() {
        context.invoke(panel, "destroy", &[])?;
        return Ok(());
    }
    let no = context.boolean(false)?;
    context.invoke(panel, "setVisible", &[no])?;
    context.invoke(panel, "removeFromUIManager", &[])?;
    restore_modal(context, panel)
}

fn restore_modal(context: &mut Context<'_, '_>, panel: &Value) -> Result<()> {
    let class = context.global("MainOptions")?;
    if class.is_nil() {
        return Ok(());
    }
    let main = context.get(&class, "instance")?;
    if main.is_nil() {
        return Ok(());
    }
    let current = context.get(&main, "modal")?;
    if context.same(&current, panel)? {
        let previous = context.get(panel, "luabuddyPreviousModal")?;
        context.set(&main, "modal", &previous)?;
    }
    let focused = context.get(panel, "joyfocus")?;
    let joyfocus = if focused.is_nil() {
        options_joypad(context, &main)?
    } else {
        focused
    };
    if !joyfocus.is_nil() {
        let focus = context.get(&joyfocus, "focus")?;
        if context.same(&focus, panel)? {
            let previous = context.get(panel, "prevFocus")?;
            context.set(&joyfocus, "focus", &previous)?;
            let active = context.get(&joyfocus, "isActive")?;
            if context.truth(&active)? {
                context.call_global("updateJoypadFocus", &[joyfocus])?;
            }
        }
    }
    Ok(())
}

fn destroy(context: &mut Context<'_, '_>, args: &[Value]) -> Result<()> {
    let panel = args.first().ok_or("Missing dialog receiver")?;
    dismiss_reload_picker(context, panel)?;
    let entry = context.get(panel, "entry")?;
    if !entry.is_nil() {
        context.invoke(&entry, "unfocus", &[])?;
    }
    restore_modal(context, panel)?;
    let original = context.get(panel, "luabuddyOriginalDestroy")?;
    if !original.is_nil() {
        context.call(&original, std::slice::from_ref(panel))?;
    }
    Ok(())
}

fn key(context: &mut Context<'_, '_>, _session: u64, args: &[Value]) -> Result<()> {
    if args.len() < 2 {
        return Ok(());
    }
    let keyboard = context.global("Keyboard")?;
    let escape = context.get(&keyboard, "KEY_ESCAPE")?;
    if context.numeric(&args[1])? == context.numeric(&escape)? {
        close(context, &args[0])?;
    }
    let enter = context.get(&keyboard, "KEY_RETURN")?;
    if context.numeric(&args[1])? == context.numeric(&enter)? {
        let entry = context.get(&args[0], "entry")?;
        if entry.is_nil() {
            let ignore = context.get(&args[0], "luabuddyIgnoreReturn")?;
            if context.truth(&ignore)? {
                let no = context.boolean(false)?;
                context.set(&args[0], "luabuddyIgnoreReturn", &no)?;
                return Ok(());
            }
            let ok = context.get(&args[0], "ok")?;
            if !ok.is_nil() {
                context.invoke(&args[0], "onClick", &[ok])?;
            }
        }
    }
    Ok(())
}

fn consumed(context: &mut Context<'_, '_>, args: &[Value]) -> Result<bool> {
    if args.len() < 2 {
        return Ok(false);
    }
    let keyboard = context.global("Keyboard")?;
    let escape = context.get(&keyboard, "KEY_ESCAPE")?;
    if context.numeric(&args[1])? == context.numeric(&escape)? {
        return Ok(true);
    }
    let enter = context.get(&keyboard, "KEY_RETURN")?;
    let entry = context.get(&args[0], "entry")?;
    let ok = context.get(&args[0], "ok")?;
    Ok(context.numeric(&args[1])? == context.numeric(&enter)? && (!entry.is_nil() || !ok.is_nil()))
}

fn message(context: &mut Context<'_, '_>, session: u64, text: &str) -> Result<()> {
    message_guard(context, session, text, false)
}

fn message_guard(
    context: &mut Context<'_, '_>,
    session: u64,
    text: &str,
    ignore_return: bool,
) -> Result<()> {
    let structured = serde_json::from_str::<serde_json::Value>(text)
        .is_ok_and(|value| value.is_object() || value.is_array());
    let (width, height) = if structured {
        (900.0, 680.0)
    } else if text.lines().count() > 12 {
        (680.0, 600.0)
    } else {
        (680.0, 280.0)
    };
    let geometry = geometry(context, width, height)?;
    let mut args = numbers(context, &geometry)?;
    args.push(context.string(&crate::diagnostic_text::rich(text))?);
    args.push(context.boolean(false)?);
    let panel = construct(context, "ISModalRichText", &args)?;
    opaque(context, &panel)?;
    let ignore_return = context.boolean(ignore_return)?;
    context.set(&panel, "luabuddyIgnoreReturn", &ignore_return)?;
    initialise(context, &panel)?;
    let text_panel = context.get(&panel, "chatText")?;
    let yes = context.boolean(true)?;
    context.invoke(&text_panel, "addScrollBars", std::slice::from_ref(&yes))?;
    context.set(&text_panel, "clip", &yes)?;
    paginate(context, &text_panel)?;
    show(context, session, &panel)
}

fn reload_prompt(context: &mut Context<'_, '_>, session: u64) -> Result<()> {
    require_clean_options(context)?;
    let profiles = configured(context)?;
    let installed = installed_items(context)?;
    let native = game::active(context)?;
    let state = context.global("LuaBuddy")?;
    let statuses = context.get(&state, "mods")?;
    let mut choices = BTreeMap::new();
    for item in installed {
        if let Some(id) = item.id {
            choices.insert(id, item.title);
        }
    }
    for profile in &profiles {
        choices
            .entry(profile.id.clone())
            .or_insert_with(|| profile.id.to_string());
    }
    let geometry = geometry(context, 620.0, 270.0)?;
    let mut args = numbers(context, &geometry)?;
    args.push(context.string("Select an installed mod or enter a custom mod ID to reload.")?);
    args.push(context.string("")?);
    args.push(Value::default());
    args.push(callback(context, session, Action::ReloadSubmit)?);
    let panel = construct(context, "ISTextBox", &args)?;
    initialise(context, &panel)?;
    opaque(context, &panel)?;
    let prerender = context.index(&panel, "prerender")?;
    context.set(&panel, "luabuddyModalPrerender", &prerender)?;
    let prerender = callback(context, session, Action::ModalPrerender)?;
    context.set(&panel, "prerender", &prerender)?;
    let yes = context.boolean(true)?;
    context.set(&panel, "noEmpty", &yes)?;
    let width = first(context.invoke(&panel, "getWidth", &[])?);
    let width = context.numeric(&width)?;
    let entry = context.get(&panel, "entry")?;
    let height = first(context.invoke(&entry, "getHeight", &[])?);
    let height = context.numeric(&height)?;
    let mut picker_args = numbers(context, &[11.0, 40.0, width - 22.0, height])?;
    picker_args.push(panel.clone());
    picker_args.push(callback(context, session, Action::ReloadSelect)?);
    let picker = construct(context, "ISComboBox", &picker_args)?;
    child(context, &panel, &picker)?;
    let prompt = context.string("Choose an installed mod, or type below")?;
    context.invoke(&picker, "addOption", &[prompt])?;
    for (id, title) in choices {
        let profile = profiles.iter().find(|profile| profile.id == id);
        let status = context.get(&statuses, id.as_str())?;
        let status = if status.is_nil() {
            None
        } else {
            Some(context.text(&status)?)
        };
        let reason = if native.contains(&id) {
            "Native-loaded. Use normal restart or reconnect."
        } else if status.as_deref() == Some("loaded") {
            if profile.is_some_and(|profile| profile.cleanup.is_some()) {
                "Loaded with a cleanup contract. Reload checks still apply."
            } else {
                "Loaded without a cleanup contract. Reconnect to reload."
            }
        } else if status.is_some() {
            "Not a healthy running profile. Inspect its status before reconnecting."
        } else if profile.is_some_and(|profile| profile.enabled) {
            "Selected but not loaded. Use Load new profiles when eligible."
        } else {
            "Installed but not selected. Enable and load it before reloading."
        };
        let label = context.string(&format!("{id} | {title}"))?;
        let data = context.string(id.as_str())?;
        let tooltip = context.string(reason)?;
        context.invoke(&picker, "addOptionWithData", &[label, data, tooltip])?;
    }
    let tooltip = context.string("Selection copies the exact mod ID below. Only enabled, locally loaded profiles with explicit cooperative cleanup can reload.")?;
    let tooltips = context.table()?;
    context.set(&tooltips, "defaultTooltip", &tooltip)?;
    context.set(&picker, "tooltip", &tooltips)?;
    context.set(&panel, "luabuddyReloadPicker", &picker)?;
    let tooltip = context.string("Enter an exact configured mod ID, including an ID not shown in the installed list. Reload still requires a healthy locally loaded profile and its cleanup contract.")?;
    context.set(&entry, "tooltip", &tooltip)?;
    let base = context.global("ISPanelJoypad")?;
    for method in ["onJoypadDirUp", "onJoypadDirDown", "onJoypadDown"] {
        let function = context.index(&base, method)?;
        context.set(&panel, method, &function)?;
    }
    for (method, action) in [
        ("onGainJoypadFocus", Action::ReloadJoypadGain),
        ("onLoseJoypadFocus", Action::ReloadJoypadLose),
        ("onJoypadDeactivate", Action::ReloadJoypadLose),
    ] {
        let function = callback(context, session, action)?;
        context.set(&panel, method, &function)?;
    }
    context.invoke(&panel, "clearJoypadButtonsList", &[])?;
    context.invoke(
        &panel,
        "insertNewLineOfButtons",
        std::slice::from_ref(&picker),
    )?;
    context.invoke(
        &panel,
        "insertNewLineOfButtons",
        std::slice::from_ref(&entry),
    )?;
    let accept = context.get(&panel, "yes")?;
    let cancel = context.get(&panel, "no")?;
    context.invoke(&panel, "insertNewLineOfButtons", &[accept, cancel])?;
    show(context, session, &panel)?;
    let enter = callback(context, session, Action::ReloadEnter)?;
    context.set(&entry, "onCommandEntered", &enter)?;
    let other_key = callback(context, session, Action::EntryKey)?;
    context.set(&entry, "onOtherKey", &other_key)?;
    context.invoke(&entry, "focus", &[])?;
    Ok(())
}

fn reload_select(context: &mut Context<'_, '_>, args: &[Value]) -> Result<()> {
    let panel = args.first().ok_or("Missing reload dialog")?;
    let picker = args.get(1).ok_or("Missing installed profile picker")?;
    let selected = context.get(picker, "selected")?;
    let id = first(context.invoke(picker, "getOptionData", &[selected])?);
    if !id.is_nil() {
        let entry = context.get(panel, "entry")?;
        context.invoke(&entry, "setText", &[id])?;
        context.invoke(&entry, "focus", &[])?;
    }
    Ok(())
}

fn reload_focus(context: &mut Context<'_, '_>, args: &[Value], gain: bool) -> Result<()> {
    let panel = args.first().ok_or("Missing reload dialog")?;
    let base = context.global("ISPanelJoypad")?;
    let function = context.index(
        &base,
        if gain {
            "onGainJoypadFocus"
        } else {
            "onLoseJoypadFocus"
        },
    )?;
    context.call(&function, args)?;
    if gain {
        let entry = context.get(panel, "entry")?;
        context.invoke(&entry, "unfocus", &[])?;
        context.invoke(panel, "setJoypadFocusTopLeft", &args[1..])?;
        let accept = context.get(panel, "yes")?;
        let cancel = context.get(panel, "no")?;
        context.invoke(panel, "setISButtonForA", &[accept])?;
        context.invoke(panel, "setISButtonForB", &[cancel])?;
    } else {
        context.invoke(panel, "clearISButtons", &[])?;
        context.invoke(panel, "clearJoypadFocus", &args[1..])?;
        dismiss_reload_picker(context, panel)?;
    }
    Ok(())
}

fn opaque(context: &mut Context<'_, '_>, panel: &Value) -> Result<()> {
    let background = context.get(panel, "backgroundColor")?;
    let yes = context.number(1.0)?;
    context.set(&background, "a", &yes)
}

fn modal_prerender(context: &mut Context<'_, '_>, args: &[Value]) -> Result<()> {
    let panel = args.first().ok_or("Missing modal receiver")?;
    let width = context.get(panel, "width")?;
    let height = context.get(panel, "height")?;
    let zero = context.number(0.0)?;
    let alpha = context.number(1.0)?;
    context.invoke(
        panel,
        "drawRect",
        &[
            zero.clone(),
            zero.clone(),
            width,
            height,
            alpha,
            zero.clone(),
            zero.clone(),
            zero,
        ],
    )?;
    let original = context.get(panel, "luabuddyModalPrerender")?;
    context.call(&original, std::slice::from_ref(panel))?;
    Ok(())
}

fn dismiss_reload_picker(context: &mut Context<'_, '_>, panel: &Value) -> Result<()> {
    let picker = context.get(panel, "luabuddyReloadPicker")?;
    if !picker.is_nil() {
        let popup = context.get(&picker, "popup")?;
        if !popup.is_nil() {
            let current = context.get(&popup, "parentCombo")?;
            if context.same(&current, &picker)? {
                context.invoke(&popup, "removeFromUIManager", &[])?;
            }
        }
    }
    Ok(())
}

fn reload_submit(
    context: &mut Context<'_, '_>,
    session: u64,
    args: &[Value],
    enter: bool,
) -> Result<()> {
    let panel = if enter {
        let entry = args.first().ok_or("Missing reload entry")?;
        context.get(entry, "parent")?
    } else {
        let button = args.get(1).ok_or("Missing reload button")?;
        let internal = context.get(button, "internal")?;
        if context.text(&internal)? != "OK" {
            return Ok(());
        }
        context.get(button, "parent")?
    };
    let submitted = context.get(&panel, "luabuddySubmitted")?;
    if context.truth(&submitted)? {
        return Ok(());
    }
    let entry = context.get(&panel, "entry")?;
    let text = first(context.invoke(&entry, "getText", &[])?);
    let id = ModId::new(context.text(&text)?).map_err(|e| e.to_string())?;
    let yes = context.boolean(true)?;
    context.set(&panel, "luabuddySubmitted", &yes)?;
    close(context, &panel)?;
    let result = runtime::apply(context, session, Operation::Reload(id));
    message_guard(context, session, &result.unwrap_or_else(|e| e), enter)
}

fn patch_diagnostics(context: &mut Context<'_, '_>, session: u64, retry: bool) -> Result<()> {
    if retry {
        require_clean_options(context)?;
        runtime::validate_session(context, session)?;
        let state = context.global("LuaBuddy")?;
        let apply = context.get(&state, "retryPatches")?;
        let result = context.call(&apply, &[])?;
        if let Some(success) = result.first()
            && !context.truth(success)?
        {
            return Err(match result.get(1) {
                Some(reason) => context.text(reason)?,
                None => "Approved patch activation was refused".into(),
            });
        }
    }
    let report = serde_json::to_string_pretty(&crate::extensions::diagnostics())
        .map_err(|error| error.to_string())?;
    message(context, session, &report)
}

fn restart(context: &mut Context<'_, '_>) -> Result<()> {
    let class = context.global("MainScreen")?;
    let main = if class.is_nil() {
        Value::default()
    } else {
        context.get(&class, "instance")?
    };
    if main.is_nil() {
        return Err(
            "Exit using the game's normal menu, then relaunch through Steam when ready.".into(),
        );
    }
    context.invoke(&main, "quitToDesktopFunc", &[])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{Action, BrowseMode, Browser, mod_description, start_page};
    use crate::lua::Value;
    use std::time::Instant;

    fn unavailable_browser() -> Browser {
        Browser {
            panel: Value::default(),
            search: Value::default(),
            list: Value::default(),
            details: Value::default(),
            status: Value::default(),
            items: Vec::new(),
            selected: usize::MAX,
            mode: BrowseMode::Installed,
            total_matching: 0,
            steam: Err("Steam is unavailable".into()),
            pending: None,
            next_poll: Instant::now(),
            buttons: Vec::new(),
            installed_catalog: Vec::new(),
            search_input: String::new(),
            search_changed: None,
        }
    }

    #[test]
    fn failed_workshop_search_keeps_the_requested_source_and_text() {
        let mut browser = unavailable_browser();
        let error = start_page(
            &mut browser,
            BrowseMode::Search {
                text: "folding".into(),
                page: 1,
            },
        )
        .unwrap_err();
        assert_eq!(error, "Steam is unavailable");
        assert!(matches!(&browser.mode, BrowseMode::Search { text, page: 1 } if text == "folding"));
        assert!(matches!(browser.mode.search_action(), Action::Search));
    }

    #[test]
    fn subscribed_scope_is_selected_before_empty_or_failed_delivery() {
        let mut browser = unavailable_browser();
        browser.mode.select_source(Action::Subscribed, "folding");
        assert!(
            matches!(&browser.mode, BrowseMode::Subscribed { items, page: 1 } if items.is_empty())
        );
        let intent = browser.mode.clone();
        assert!(start_page(&mut browser, intent).is_err());
        assert!(matches!(browser.mode.search_action(), Action::Subscribed));
        browser.mode.select_source(Action::Installed, "folding");
        assert!(matches!(browser.mode.search_action(), Action::Installed));
    }

    #[test]
    fn workshop_description_keeps_text_lists_and_unknown_metadata() {
        let description = mod_description(
            "[h1]Guide[/h1][b]Use[/b] [url=https://example.test]the original[/url][list][*]First[*]Second[/list][img]https://example.test/banner.png[/img][B42]中文",
        );
        assert!(description.starts_with("Guide\n"));
        assert!(description.contains("Use the original"));
        assert!(description.contains("\n• First\n• Second"));
        assert!(description.ends_with("[B42]中文"));
        assert!(!description.contains("banner.png"));
        assert!(description.contains("[Image on original Workshop page]"));
    }

    #[test]
    fn mod_description_does_not_interpret_native_rich_text_commands() {
        let description = mod_description("[b]<RGB:1,0,0>literal text[/b] with [unfinished");
        let rendered = crate::diagnostic_text::rich(&description);
        assert!(rendered.contains("&lt;RGB:1,0,0&gt;literal text"));
        assert!(rendered.contains("[unfinished"));
        assert!(!rendered.contains("<RGB:1,0,0>"));
    }
}
