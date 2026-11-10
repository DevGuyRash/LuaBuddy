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
}

struct Ui {
    session: u64,
    environment: Value,
    browser: Option<Browser>,
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
    let result = match action {
        Action::Open => open(&mut context, session),
        Action::Refresh => {
            let result = runtime::apply(&mut context, session, Operation::Refresh);
            message(&mut context, session, &result.unwrap_or_else(|e| e))
        }
        Action::Reload => reload_prompt(&mut context, session),
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
            }
            tick(&mut context)
        }
        Action::Restart => restart(&mut context),
        Action::PatchDiagnostics => patch_diagnostics(&mut context, session, false),
        Action::ApplyApprovedPatches => patch_diagnostics(&mut context, session, true),
        action => with_browser(&mut context, session, |context, browser| {
            browser_action(context, session, browser, action)
        }),
    };
    if let Err(error) = result {
        game::log(&format!("interface action failed: {error}"));
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
    let [x, y, w, h] = geometry(context, 1000.0, 680.0)?;
    if w < 700.0 || h < 440.0 {
        return Err("The mod browser requires a game window of at least 740 by 480 pixels. Increase the game's resolution and reopen it.".into());
    }
    let panel_args = numbers(context, &[x, y, w, h])?;
    let panel = construct(context, "ISPanelJoypad", &panel_args)?;
    let background = context.get(&panel, "backgroundColor")?;
    let opacity = context.number(0.96)?;
    context.set(&background, "a", &opacity)?;
    initialise(context, &panel)?;
    let yes = context.boolean(true)?;
    context.set(&panel, "moveWithMouse", &yes)?;
    context.set(&panel, "luabuddyBrowser", &yes)?;
    field_text(context, &panel, "title", "LuaBuddy mod manager")?;
    let update = context.index(&panel, "update")?;
    context.set(&panel, "luabuddyOriginalUpdate", &update)?;
    let update = callback(context, session, Action::Update)?;
    context.set(&panel, "update", &update)?;
    let mut search_args = vec![context.string("")?];
    search_args.extend(numbers(context, &[12.0, 12.0, w - 410.0, 28.0])?);
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
        [w - 390.0, 12.0, 116.0, 28.0],
        "Workshop search",
        Action::Search,
    )?;
    let installed_button = button(
        context,
        session,
        &panel,
        [w - 266.0, 12.0, 116.0, 28.0],
        "Installed",
        Action::Installed,
    )?;
    let subscribed_button = button(
        context,
        session,
        &panel,
        [w - 142.0, 12.0, 130.0, 28.0],
        "Subscribed",
        Action::Subscribed,
    )?;
    let list_width = (w * 0.40).max(220.0);
    let list_args = numbers(context, &[12.0, 50.0, list_width, h - 174.0])?;
    let list = construct(context, "ISScrollingListBox", &list_args)?;
    let item_height = context.number(24.0)?;
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
        &[list_width + 24.0, 50.0, w - list_width - 36.0, h - 174.0],
    )?;
    let details = construct(context, "ISRichTextPanel", &details_args)?;
    let no = context.boolean(false)?;
    context.set(&details, "autosetheight", &no)?;
    context.set(&details, "clip", &yes)?;
    child(context, &panel, &details)?;
    context.invoke(&details, "addScrollBars", &[])?;
    let status_args = numbers(context, &[12.0, h - 116.0, w - 24.0, 40.0])?;
    let status = construct(context, "ISRichTextPanel", &status_args)?;
    context.set(&status, "autosetheight", &no)?;
    context.set(&status, "clip", &yes)?;
    child(context, &panel, &status)?;
    let titles = [
        ("Add / download", Action::Add),
        ("Download / update", Action::Download),
        ("Enable late", Action::Enable),
        ("Disable late", Action::Disable),
        ("Load enabled", Action::Load),
        ("Cancel query", Action::Cancel),
    ];
    let button_width = (w - 24.0 - 5.0 * 6.0) / 6.0;
    let mut row = Vec::new();
    for (index, (title, action)) in titles.into_iter().enumerate() {
        row.push(button(
            context,
            session,
            &panel,
            [
                12.0 + index as f64 * (button_width + 6.0),
                h - 72.0,
                button_width,
                26.0,
            ],
            title,
            action,
        )?);
    }
    let bottom_width = (w - 54.0) / 6.0;
    let restart = button(
        context,
        session,
        &panel,
        [12.0, h - 38.0, bottom_width, 26.0],
        "Restart game...",
        Action::Restart,
    )?;
    let startup_enable = button(
        context,
        session,
        &panel,
        [12.0 + bottom_width + 6.0, h - 38.0, bottom_width, 26.0],
        "Startup enable",
        Action::StartupEnable,
    )?;
    let startup_disable = button(
        context,
        session,
        &panel,
        [
            12.0 + (bottom_width + 6.0) * 2.0,
            h - 38.0,
            bottom_width,
            26.0,
        ],
        "Startup disable",
        Action::StartupDisable,
    )?;
    let previous = button(
        context,
        session,
        &panel,
        [
            12.0 + (bottom_width + 6.0) * 3.0,
            h - 38.0,
            bottom_width,
            26.0,
        ],
        "< Previous",
        Action::Previous,
    )?;
    let next = button(
        context,
        session,
        &panel,
        [
            12.0 + (bottom_width + 6.0) * 4.0,
            h - 38.0,
            bottom_width,
            26.0,
        ],
        "Next >",
        Action::Next,
    )?;
    let close = button(
        context,
        session,
        &panel,
        [
            12.0 + (bottom_width + 6.0) * 5.0,
            h - 38.0,
            bottom_width,
            26.0,
        ],
        "Close",
        Action::Close,
    )?;
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
    context.invoke(&panel, "insertNewLineOfButtons", &row)?;
    context.invoke(
        &panel,
        "insertNewLineOfButtons",
        &[
            restart,
            startup_enable,
            startup_disable,
            previous,
            next,
            close.clone(),
        ],
    )?;
    context.invoke(&panel, "setISButtonForB", &[close])?;
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
    };
    installed(context, &mut browser)?;
    UI.with(|state| {
        if let Some(ui) = state.borrow_mut().as_mut() {
            ui.browser = Some(browser);
        }
    });
    show(context, session, &panel)?;
    context.invoke(&search, "focus", &[])?;
    Ok(())
}

fn rich(context: &mut Context<'_, '_>, panel: &Value, text: &str) -> Result<()> {
    let escaped = text
        .replace('<', "[")
        .replace('>', "]")
        .replace('\n', " <LINE> ");
    field_text(context, panel, "text", &escaped)?;
    context.invoke(panel, "paginate", &[])?;
    Ok(())
}

fn set_status(context: &mut Context<'_, '_>, browser: &Browser, text: &str) -> Result<()> {
    rich(context, &browser.status, text)
}

fn browser_action(
    context: &mut Context<'_, '_>,
    session: u64,
    browser: &mut Browser,
    action: Action,
) -> Result<()> {
    let action = if matches!(action, Action::SearchSubmit) {
        match &browser.mode {
            BrowseMode::Installed => Action::Installed,
            BrowseMode::Subscribed { .. } | BrowseMode::FilteredSubscriptions { .. } => {
                Action::Subscribed
            }
            BrowseMode::Search { .. } => Action::Search,
        }
    } else {
        action
    };
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
        Action::Select => details(context, browser),
        Action::Previous | Action::Next => {
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
                    "Selection saved. This mod supplies startup-only assets. Restart and use the game's normal loading path; live activation is unavailable."
                } else {
                    "Selection saved. Choose Load enabled to request live loading. Dependency checks still apply."
                }
            } else {
                "Selection disabled for the next environment. Any running code stays active until reconnect or restart."
            };
            details(context, browser)?;
            set_status(context, browser, message)
        }
        Action::Load => {
            require_clean_options(context)?;
            let item = selected(context, browser)?;
            if item.id.is_none() {
                return Err(
                    "Select an installed mod. A Workshop item may contain several mod identities."
                        .into(),
                );
            }
            if item.restart_assets {
                return Err("This mod supplies startup-only assets. Its selection is saved; restart through the game's normal loading path.".into());
            }
            let result = runtime::apply(context, session, Operation::Refresh);
            details(context, browser)?;
            set_status(context, browser, &result.unwrap_or_else(|e| e))
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
            set_status(
                context,
                browser,
                "Native startup selection saved. Current game and server activations are unchanged. Restart before using the changed selection.",
            )
        }
        Action::Cancel => match browser.pending.take() {
            Some(Pending::Query(mut query)) => {
                query.cancel().map_err(|e| e.to_string())?;
                set_status(context, browser, "Workshop query cancelled.")
            }
            Some(Pending::SubscriptionSearch(mut search)) => {
                search.query.cancel().map_err(|error| error.to_string())?;
                set_status(context, browser, "Subscribed-item search cancelled.")
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
    let steam = browser.steam.as_ref().map_err(Clone::clone)?;
    let query = match &mode {
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
    browser.mode = mode;
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
        context.invoke(&browser.list, "addItem", &[title, id])?;
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
    let mut lines = vec![item.title, format!("Source: {}", item.source)];
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
        lines.push(format!(
            "Late loader selection: {}",
            if enabled { "enabled" } else { "disabled" }
        ));
        let startup = crate::activation::selected_next_startup(context)?;
        lines.push(format!(
            "Native selection for next startup: {}",
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
                    "enabled, not loaded".to_owned()
                } else {
                    "disabled".to_owned()
                }
            } else {
                context.text(&loaded)?
            }
        ));
    }
    if let Some(id) = item.workshop {
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
    lines.push(item.description);
    rich(context, &browser.details, &lines.join("\n\n"))
}

fn configured(context: &mut Context<'_, '_>) -> Result<Vec<Profile>> {
    let directory =
        game::property(context, "luabuddy.config")?.ok_or("Missing LuaBuddy profile directory")?;
    read_profiles(Path::new(&directory)).map_err(|e| e.to_string())
}

fn installed(context: &mut Context<'_, '_>, browser: &mut Browser) -> Result<()> {
    if browser.pending.is_some() {
        return Err("Wait for the current operation or cancel its query.".into());
    }
    game::reset_discovery(context)?;
    browser.items = installed_items(context)?;
    let installed_count = browser.items.len();
    let filter = first(context.invoke(&browser.search, "getText", &[])?);
    let filter = context.text(&filter)?.trim().to_lowercase();
    if !filter.is_empty() {
        browser.items.retain(|item| matches_search(item, &filter));
    }
    browser.mode = BrowseMode::Installed;
    render(context, browser)?;
    let suffix = browser
        .steam
        .as_ref()
        .err()
        .map(|error| format!(" Workshop unavailable: {error}"))
        .unwrap_or_default();
    set_status(
        context,
        browser,
        &format!(
            "{} matching installed mod identities from {installed_count}.{}",
            browser.items.len(),
            suffix
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
        if let Some(error) = candidate.hook_migration_error() {
            return Err(error.to_string());
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
        if Instant::now() < browser.next_poll {
            return Ok(());
        }
        browser.next_poll = Instant::now() + Duration::from_millis(150);
        let pending = browser.pending.take();
        if let Some(mut pending) = pending {
            let result = match &mut pending {
                Pending::Query(query) => match query.poll().map_err(|e| e.to_string())? {
                    steam::Poll::Pending => {
                        browser.pending = Some(pending);
                        None
                    }
                    steam::Poll::Ready(results) => {
                        let count = results.items.len();
                        let unavailable = results.unavailable.len();
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
                set_status(context, browser, &text)?;
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
            set_status(
                context,
                browser,
                &format!("Failed: {error}. Retry the requested action after correcting the cause."),
            )
        })?;
    }
    Ok(())
}

fn show(context: &mut Context<'_, '_>, session: u64, panel: &Value) -> Result<()> {
    let original = context.index(panel, "destroy")?;
    if !original.is_nil() {
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
            let joyfocus = context.get(&main, "joyfocus")?;
            if !joyfocus.is_nil() {
                let previous = context.get(&joyfocus, "focus")?;
                context.set(panel, "prevFocus", &previous)?;
                context.set(&joyfocus, "focus", panel)?;
                context.call_global("updateJoypadFocus", &[joyfocus])?;
            }
        }
    }
    Ok(())
}

fn close(context: &mut Context<'_, '_>, panel: &Value) -> Result<()> {
    let visible = first(context.invoke(panel, "getIsVisible", &[])?);
    if !context.truth(&visible)? {
        return Ok(());
    }
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
    let joyfocus = context.get(&main, "joyfocus")?;
    if !joyfocus.is_nil() {
        let focus = context.get(&joyfocus, "focus")?;
        if context.same(&focus, panel)? {
            let previous = context.get(panel, "prevFocus")?;
            context.set(&joyfocus, "focus", &previous)?;
            context.call_global("updateJoypadFocus", &[joyfocus])?;
        }
    }
    Ok(())
}

fn destroy(context: &mut Context<'_, '_>, args: &[Value]) -> Result<()> {
    let panel = args.first().ok_or("Missing dialog receiver")?;
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
    let geometry = geometry(context, 680.0, 280.0)?;
    let mut args = numbers(context, &geometry)?;
    args.push(
        context.string(
            &text
                .replace('<', "[")
                .replace('>', "]")
                .replace('\n', " <LINE> "),
        )?,
    );
    args.push(context.boolean(false)?);
    let panel = construct(context, "ISModalRichText", &args)?;
    let ignore_return = context.boolean(ignore_return)?;
    context.set(&panel, "luabuddyIgnoreReturn", &ignore_return)?;
    initialise(context, &panel)?;
    show(context, session, &panel)
}

fn reload_prompt(context: &mut Context<'_, '_>, session: u64) -> Result<()> {
    require_clean_options(context)?;
    let geometry = geometry(context, 560.0, 190.0)?;
    let mut args = numbers(context, &geometry)?;
    args.push(context.string(
        "Configured mod ID to reload. Explicit cooperative cleanup support is required.",
    )?);
    args.push(context.string("")?);
    args.push(Value::default());
    args.push(callback(context, session, Action::ReloadSubmit)?);
    let panel = construct(context, "ISTextBox", &args)?;
    initialise(context, &panel)?;
    show(context, session, &panel)?;
    let entry = context.get(&panel, "entry")?;
    let enter = callback(context, session, Action::ReloadEnter)?;
    context.set(&entry, "onCommandEntered", &enter)?;
    let other_key = callback(context, session, Action::EntryKey)?;
    context.set(&entry, "onOtherKey", &other_key)?;
    context.invoke(&entry, "focus", &[])?;
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
    let yes = context.boolean(true)?;
    context.set(&panel, "luabuddySubmitted", &yes)?;
    let entry = context.get(&panel, "entry")?;
    let text = first(context.invoke(&entry, "getText", &[])?);
    let id = ModId::new(context.text(&text)?).map_err(|e| e.to_string())?;
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
            "Selection saved. Exit using the game's normal menu, then relaunch through Steam."
                .into(),
        );
    }
    context.invoke(&main, "quitToDesktopFunc", &[])?;
    Ok(())
}
