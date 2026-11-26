//! The window's panels and buttons, and the measurements that place them.

use super::*;

/// The gap between stacked controls.
pub(super) const ROW_GAP: f64 = 6.0;

/// The label of the button that opens a selection's other actions.
pub(super) const MORE: &str = "More...";

/// A button's room beyond its label.
pub(super) const BUTTON_INSET: f64 = 28.0;

pub(super) fn button_width(context: &mut Context<'_, '_>, label: &str) -> Result<f64> {
    Ok(kit::width(context, Font::Small, label)? + BUTTON_INSET)
}

pub(super) fn content_top() -> f64 {
    PAD + header_height() + PAD
}

pub(super) fn list_width(frame_width: f64) -> f64 {
    (frame_width * 0.40).clamp(260.0, 640.0)
}

pub(super) struct Buttons {
    pub(super) back: Value,
    pub(super) primary: Value,
    pub(super) more: Value,
    pub(super) tools: Value,
    pub(super) footer: Value,
    pub(super) tabs: Vec<(Tab, Value)>,
    pub(super) sources: Vec<(Source, Value)>,
}

pub(super) fn header_height() -> f64 {
    kit::line_height(Font::Large).max(kit::control_height())
}

pub(super) fn frame_size((width, height): (f64, f64)) -> (f64, f64) {
    let part = if width <= 1280.0 { 0.96 } else { 0.86 };
    (
        (width * part).min(1800.0).floor(),
        (height * 0.92).min(1200.0).floor(),
    )
}

pub(super) fn mark(context: &mut Context<'_, '_>, button: &Value, active: bool) -> Result<()> {
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
pub(super) fn place_action(
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
    let label = kit::fit(context, Font::Small, &node.label, limit - BUTTON_INSET)?;
    let title = context.string(&label)?;
    context.invoke(button, "setTitle", &[title])?;
    let width = button_width(context, &label)?;
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

pub(super) type Built = (Value, Value, Value, Value, Value, Buttons);

pub(super) fn build(
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
    let mut x = PAD + kit::width(context, Font::Large, "LuaBuddy")? + BUTTON_INSET;
    let mut tabs = Vec::new();
    // The Workshop tab exists only when the game's Steam client does.
    for tab in Tab::ALL
        .into_iter()
        .filter(|tab| steam || *tab != Tab::Workshop)
    {
        let button = button(
            context,
            &frame,
            &click,
            tab.control(),
            tab.label(),
            [x, header],
        )?;
        x += kit::ask_number(context, &button, "getWidth")? + ROW_GAP;
        tabs.push((tab, button));
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
    let more = button(context, &frame, &click, "more", MORE, [PAD, 0.0])?;
    let footer = button(context, &frame, &click, "footer", "", [PAD, 0.0])?;
    let top = content_top();
    let list_width = list_width(w);
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
            [x, top + height + ROW_GAP],
        )?;
        x += kit::ask_number(context, &button, "getWidth")? + ROW_GAP;
        sources.push((source, button));
    }
    let list = surface(context, &frame)?;
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
    let page = surface(context, &frame)?;
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

pub(super) fn panel(context: &mut Context<'_, '_>, bounds: [f64; 4]) -> Result<Value> {
    let args = kit::numbers(context, &bounds)?;
    let panel = kit::construct(context, "ISPanelJoypad", &args)?;
    kit::set_flag(context, &panel, "background", false)?;
    Ok(panel)
}

pub(super) fn surface(context: &mut Context<'_, '_>, frame: &Value) -> Result<Value> {
    let surface = panel(context, [0.0, 0.0, 100.0, 100.0])?;
    kit::add(context, frame, &surface)?;
    context.invoke(&surface, "addScrollBars", &[])?;
    Ok(surface)
}

pub(super) fn button(
    context: &mut Context<'_, '_>,
    frame: &Value,
    click: &Value,
    name: &str,
    label: &str,
    [x, y]: [f64; 2],
) -> Result<Value> {
    let width = button_width(context, label)?;
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
