//! LuaBuddy's place in the game's Options screen, and rebuilding that screen's
//! Mods page when newly loaded mods add settings to it.

use super::{
    Event, callback,
    kit::{self, first},
};
use crate::{
    log,
    lua::{Context, Result, Value},
};

pub fn install(context: &mut Context<'_, '_>, session: u64) -> Result<()> {
    super::begin(context, session);
    let api = context.global("PZAPI")?;
    if api.is_nil() {
        return Err("The game's Mod Options API is unavailable".into());
    }
    let options = context.get(&api, "ModOptions")?;
    if options.is_nil() {
        return Err("The game's Mod Options API is unavailable".into());
    }
    for module in [
        "ISUI/ISPanelJoypad",
        "ISUI/ISLabel",
        "ISUI/ISToolTip",
        "ISUI/ISTextEntryBox",
        "ISUI/ISButton",
        "ISUI/ISContextMenu",
    ] {
        let name = context.string(module)?;
        context.call_global("require", &[name])?;
    }
    let name = context.string("LuaBuddy")?;
    let existing = first(context.invoke(&options, "getOptions", std::slice::from_ref(&name))?);
    if !existing.is_nil() {
        return Ok(());
    }
    let options = first(context.invoke(&options, "create", &[name.clone(), name])?);
    let args = [
        context.string("open")?,
        context.string("Open LuaBuddy")?,
        context.string("Choose which mods LuaBuddy loads, and see what is running.")?,
        callback(context, session, Event::Open)?,
    ];
    context.invoke(&options, "addButton", &args)?;
    Ok(())
}

fn main_options(context: &mut Context<'_, '_>) -> Result<Option<Value>> {
    let class = context.global("MainOptions")?;
    if class.is_nil() {
        return Ok(None);
    }
    let main = context.get(&class, "instance")?;
    Ok((!main.is_nil()).then_some(main))
}

/// Whether the player has Options edits they have neither applied nor cancelled.
pub fn pending(context: &mut Context<'_, '_>) -> Result<bool> {
    let Some(main) = main_options(context)? else {
        return Ok(false);
    };
    let options = context.get(&main, "gameOptions")?;
    if options.is_nil() {
        return Ok(false);
    }
    let changed = context.get(&options, "changed")?;
    context.truth(&changed)
}

/// Rebuilds only the Mods page, keeping its tab position, ID and scroll.
pub fn refresh(context: &mut Context<'_, '_>) -> Result<()> {
    let Some(main) = main_options(context)? else {
        return Ok(());
    };
    let class = context.global("MainOptions")?;
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
        let table_remove = kit::table_function(context, "remove")?;
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
    let table_remove = kit::table_function(context, "remove")?;
    let table_insert = kit::table_function(context, "insert")?;
    context.call(&table_remove, &[view_list.clone(), replacement_index])?;
    context.call(&table_insert, &[view_list, original_index, replacement])?;
    context.invoke(&options, "toUI", &[])?;
    context.invoke(&tabs, "activateView", std::slice::from_ref(&title))?;
    let panel = first(context.invoke(&tabs, "getView", &[title])?);
    context.invoke(&panel, "setYScroll", &[scroll])?;
    log::debug(
        "options.rebuilt",
        "Rebuilt the Mods page of the game's Options so newly loaded settings appear",
    )
    .write();
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
