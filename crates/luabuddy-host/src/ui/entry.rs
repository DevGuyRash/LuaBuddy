//! LuaBuddy's line in the main menu and pause menu.

use super::{Event, kit, set_callback};
use crate::{
    log,
    lua::{Context, Result, Value},
    runtime,
};
use kit::first;

const NAME: &str = "LUABUDDY";
const LABEL: &str = "luabuddyLabel";
const ORIGINAL_RENDER: &str = "luabuddyRender";
const ORIGINAL_GAIN: &str = "luabuddyJoypadGain";
const ORIGINAL_DOWN: &str = "luabuddyJoypadDown";

/// Adds the line to the menu that now exists, and keeps its count current.
pub fn attach(context: &mut Context<'_, '_>, session: u64) -> Result<()> {
    // Before the player can reach the game's own mod list from this menu.
    super::screen::remember_launch_startup(context);
    let Some(main) = main_screen(context)? else {
        return Ok(());
    };
    if context.get(&main, LABEL)?.is_nil() {
        add(context, session, &main)?;
        log::debug("ui.entry", "Added LuaBuddy to the menu").write();
    }
    update(context, &main)
}

fn main_screen(context: &mut Context<'_, '_>) -> Result<Option<Value>> {
    let class = context.global("MainScreen")?;
    if class.is_nil() {
        return Ok(None);
    }
    let main = context.get(&class, "instance")?;
    Ok((!main.is_nil()).then_some(main))
}

/// The line LuaBuddy's goes under: the game's own mod entries when it has them.
fn anchor(context: &mut Context<'_, '_>, main: &Value) -> Result<Option<Value>> {
    for name in ["workshopOption", "modsOption", "optionsOption"] {
        let option = context.get(main, name)?;
        if !option.is_nil() {
            return Ok(Some(option));
        }
    }
    Ok(None)
}

fn add(context: &mut Context<'_, '_>, session: u64, main: &Value) -> Result<()> {
    let panel = context.get(main, "bottomPanel")?;
    let (Some(anchor), false) = (anchor(context, main)?, panel.is_nil()) else {
        return Ok(());
    };
    let height = kit::ask_number(context, &anchor, "getHeight")?;
    let bottom = kit::ask_number(context, &anchor, "getBottom")?;
    for child in children(context, &panel)? {
        let y = kit::ask_number(context, &child, "getY")?;
        if y >= bottom {
            let y = context.number(y + height)?;
            context.invoke(&child, "setY", &[y])?;
        }
    }
    let font = context.jni(|env| {
        let font = env
            .get_static_field("zombie/ui/UIFont", "Large", "Lzombie/ui/UIFont;")?
            .l()?;
        Value::retain(env, font)
    })?;
    let mut args = kit::numbers(context, &[0.0, bottom, height])?;
    args.push(context.string(NAME)?);
    args.extend(kit::numbers(context, &[1.0, 1.0, 1.0, 1.0])?);
    args.push(font);
    args.push(context.boolean(true)?);
    let label = kit::construct(context, "ISLabel", &args)?;
    kit::initialise(context, &label)?;
    kit::set_text(context, &label, "internal", NAME)?;
    set_callback(context, session, &label, "onMouseDown", Event::Open)?;
    context.invoke(&panel, "addChild", std::slice::from_ref(&label))?;
    let width = kit::ask_number(context, &anchor, "getWidth")?;
    let width = context.number(width)?;
    context.invoke(&label, "setWidth", &[width])?;
    // The same hover fade the game gives its own lines.
    let transition = context.global("UITransition")?;
    let create = context.index(&transition, "new")?;
    let fade = first(context.call(&create, &[])?);
    kit::call_flag_native(context, &fade, "setFadeIn", false)?;
    context.set(&label, "fade", &fade)?;
    let class = context.global("MainScreen")?;
    let prerender = context.get(&class, "prerenderBottomPanelLabel")?;
    context.set(&label, "prerender", &prerender)?;
    let total = kit::ask_number(context, &panel, "getHeight")?;
    let total = context.number(total + height)?;
    context.invoke(&panel, "setHeight", &[total])?;
    context.set(main, LABEL, &label)?;
    for (method, saved, event) in [
        ("render", ORIGINAL_RENDER, Event::EntryRender),
        ("onGainJoypadFocus", ORIGINAL_GAIN, Event::EntryJoypadGain),
        ("onJoypadDown", ORIGINAL_DOWN, Event::EntryJoypadDown),
    ] {
        let original = context.index(main, method)?;
        context.set(main, saved, &original)?;
        set_callback(context, session, main, method, event)?;
    }
    Ok(())
}

fn children(context: &mut Context<'_, '_>, panel: &Value) -> Result<Vec<Value>> {
    let table = first(context.invoke(panel, "getChildren", &[])?);
    Ok(context
        .entries(&table)?
        .into_iter()
        .map(|(_, child)| child)
        .collect())
}

fn update(context: &mut Context<'_, '_>, main: &Value) -> Result<()> {
    let label = context.get(main, LABEL)?;
    if label.is_nil() {
        return Ok(());
    }
    let problems = runtime::problems();
    let text = if problems == 0 {
        NAME.to_owned()
    } else {
        format!("{NAME} ({problems})")
    };
    let current = context.get(&label, "name")?;
    if current.is_nil() || context.text(&current)? != text {
        kit::set_text(context, &label, "name", &text)?;
    }
    let [r, g, b, _] = if problems == 0 {
        [1.0; 4]
    } else {
        kit::WARNING
    };
    let color = kit::numbers(context, &[r, g, b])?;
    context.invoke(&label, "setColor", &color)?;
    Ok(())
}

/// Runs after the game lays out its menu, which it does on every frame while
/// connected to a server.
pub fn render(context: &mut Context<'_, '_>, args: &[Value]) -> Result<()> {
    let main = args.first().ok_or("Missing menu receiver")?;
    let original = context.get(main, ORIGINAL_RENDER)?;
    context.call(&original, std::slice::from_ref(main))?;
    let label = context.get(main, LABEL)?;
    let Some(anchor) = anchor(context, main)? else {
        return Ok(());
    };
    let shown = kit::ask_flag(context, &anchor, "isVisible")?;
    if kit::ask_flag(context, &label, "isVisible")? != shown {
        kit::call_flag(context, &label, "setVisible", shown)?;
    }
    let ingame = context.get(main, "inGame")?;
    if !context.truth(&ingame)? || !crate::game::client(context)? {
        return Ok(());
    }
    let height = kit::ask_number(context, &label, "getHeight")?;
    let bottom = kit::ask_number(context, &anchor, "getBottom")?;
    let y = context.number(bottom)?;
    context.invoke(&label, "setY", &[y])?;
    for name in ["exitOption", "quitToDesktopOption"] {
        let option = context.get(main, name)?;
        let y = kit::ask_number(context, &option, "getY")?;
        let y = context.number(y + height)?;
        context.invoke(&option, "setY", &[y])?;
    }
    let panel = context.get(main, "bottomPanel")?;
    let total = kit::ask_number(context, &panel, "getHeight")?;
    let total = context.number(total + height)?;
    context.invoke(&panel, "setHeight", &[total])?;
    Ok(())
}

pub fn joypad_gain(context: &mut Context<'_, '_>, args: &[Value]) -> Result<()> {
    let main = args.first().ok_or("Missing menu receiver")?;
    let original = context.get(main, ORIGINAL_GAIN)?;
    context.call(&original, args)?;
    let rows = context.get(main, "joypadButtonsY")?;
    let Some(anchor) = anchor(context, main)? else {
        return Ok(());
    };
    for (index, row) in context.entries(&rows)? {
        let first_in_row = context
            .entries(&row)?
            .into_iter()
            .next()
            .map(|(_, label)| label)
            .unwrap_or_default();
        if !context.same(&first_in_row, &anchor)? {
            continue;
        }
        let label = context.get(main, LABEL)?;
        let ours = context.table()?;
        let table = context.global("table")?;
        let insert = context.get(&table, "insert")?;
        context.call(&insert, &[ours.clone(), label])?;
        let position = context.numeric(&index)? + 1.0;
        let position = context.number(position)?;
        context.call(&insert, &[rows, position.clone(), ours])?;
        // Keep the game's chosen default highlighted if it moved down a place.
        let selected = context.get(main, "joypadIndexY")?;
        if !selected.is_nil() {
            let selected = context.numeric(&selected)?;
            if selected >= context.numeric(&position)? {
                kit::set_number(context, main, "joypadIndexY", selected + 1.0)?;
            }
        }
        break;
    }
    Ok(())
}

/// Whether the press chose LuaBuddy's line; otherwise the game handles it.
pub fn joypad_down(context: &mut Context<'_, '_>, args: &[Value]) -> Result<bool> {
    let main = args.first().ok_or("Missing menu receiver")?;
    let joypad = context.global("Joypad")?;
    let accept = context.get(&joypad, "AButton")?;
    let pressed = args.get(1).ok_or("Missing controller button")?;
    if context.numeric(pressed)? == context.numeric(&accept)? {
        let label = context.get(main, LABEL)?;
        let focused = context.get(&label, "joypadFocused")?;
        if context.truth(&focused)? {
            return Ok(true);
        }
    }
    let original = context.get(main, ORIGINAL_DOWN)?;
    context.call(&original, args)?;
    Ok(false)
}

/// Hides or shows the menu's own lines, returning whether they were shown.
pub fn set_menu_visible(context: &mut Context<'_, '_>, visible: bool) -> Result<bool> {
    let Some(main) = main_screen(context)? else {
        return Ok(false);
    };
    let panel = context.get(&main, "bottomPanel")?;
    if panel.is_nil() {
        return Ok(false);
    }
    let before = kit::ask_flag(context, &panel, "getIsVisible")?;
    if before != visible {
        kit::call_flag(context, &panel, "setVisible", visible)?;
    }
    if visible {
        update(context, &main)?;
    }
    Ok(before)
}
