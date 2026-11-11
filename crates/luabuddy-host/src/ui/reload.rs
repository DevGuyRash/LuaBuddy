use super::*;

fn choices(context: &mut Context<'_, '_>, panel: &Value) -> Result<BTreeMap<ModId, String>> {
    let value = context.get(panel, "luabuddyReloadChoices")?;
    serde_json::from_str(&context.text(&value)?).map_err(|error| error.to_string())
}

fn resolve_id(text: &str, choices: &BTreeMap<ModId, String>) -> Result<ModId> {
    let typed = ModId::new(text).map_err(|error| error.to_string())?;
    if choices.contains_key(&typed) {
        return Ok(typed);
    }
    let folded = typed.as_str().to_lowercase();
    let mut matches = choices
        .keys()
        .filter(|id| id.as_str().to_lowercase() == folded);
    let first = matches.next();
    if matches.next().is_some() {
        return Err(format!(
            "More than one installed mod ID matches {typed} when case is ignored. Choose the exact ID from the installed list."
        ));
    }
    Ok(first.cloned().unwrap_or(typed))
}

fn label(context: &mut Context<'_, '_>, panel: &Value, text: &str, y: f64) -> Result<f64> {
    let font = context.jni(|env| {
        let font = env
            .get_static_field("zombie/ui/UIFont", "Small", "Lzombie/ui/UIFont;")?
            .l()?;
        env.new_global_ref(font)
            .map(|reference| Value(Some(reference)))
    })?;
    let manager = first(context.call_global("getTextManager", &[])?);
    let height = context.jni(|env| {
        env.call_method(
            manager.object(),
            "getFontHeight",
            "(Lzombie/ui/UIFont;)I",
            &[JValue::Object(font.object())],
        )?
        .i()
    })?;
    let mut args = numbers(context, &[11.0, y, f64::from(height)])?;
    args.push(context.string(text)?);
    args.extend(numbers(context, &[0.9, 0.9, 0.9, 1.0])?);
    args.push(font);
    args.push(context.boolean(true)?);
    let label = construct(context, "ISLabel", &args)?;
    child(context, panel, &label)?;
    Ok(f64::from(height))
}

fn focus_entry(context: &mut Context<'_, '_>, panel: &Value, entry: &Value) -> Result<()> {
    if context.get(panel, "joyfocus")?.is_nil() {
        context.invoke(entry, "focus", &[])?;
    }
    Ok(())
}

pub(super) fn typed(context: &mut Context<'_, '_>, args: &[Value]) -> Result<()> {
    let panel = args.first().ok_or("Missing reload dialog")?;
    if !top_keyboard_modal(context, panel)? {
        return Ok(());
    }
    let entry = args.get(1).ok_or("Missing custom mod ID entry")?;
    let value = first(context.invoke(entry, "getText", &[])?);
    let text = context.text(&value)?;
    let previous = context.get(panel, "luabuddyReloadLastInput")?;
    if !previous.is_nil() && context.text(&previous)? == text {
        return Ok(());
    }
    context.set(panel, "luabuddyReloadLastInput", &value)?;
    show_suggestions(context, panel, &text)
}

fn popup_open(context: &mut Context<'_, '_>, picker: &Value) -> Result<bool> {
    let expanded = context.get(picker, "expanded")?;
    if !context.truth(&expanded)? {
        return Ok(false);
    }
    let popup = context.get(picker, "popup")?;
    let owner = context.get(&popup, "parentCombo")?;
    context.same(&owner, picker)
}

fn show_suggestions(context: &mut Context<'_, '_>, panel: &Value, text: &str) -> Result<()> {
    let query = text
        .trim_matches(|character| character <= ' ')
        .to_lowercase();
    let has_matches = !query.is_empty()
        && choices(context, panel)?
            .iter()
            .any(|(id, title)| format!("{id} | {title}").to_lowercase().contains(&query));
    let picker = context.get(panel, "luabuddyReloadPicker")?;
    let zero = context.number(0.0)?;
    context.set(&picker, "selected", &zero)?;
    if !has_matches {
        dismiss(context, panel)?;
        return Ok(());
    }
    let query = context.string(text)?;
    context.invoke(&picker, "setFilterText", &[query])?;
    let popup = context.get(&picker, "popup")?;
    let open = popup_open(context, &picker)?;
    let yes = context.boolean(true)?;
    context.set(&picker, "expanded", &yes)?;
    if open {
        context.invoke(&popup, "setComboBox", std::slice::from_ref(&picker))?;
    } else {
        context.invoke(&picker, "showPopup", &[])?;
    }
    let entry = context.get(panel, "entry")?;
    let top = first(context.invoke(&entry, "getAbsoluteY", &[])?);
    let top = context.numeric(&top)?;
    let height = first(context.invoke(&entry, "getHeight", &[])?);
    let bottom = top + context.numeric(&height)?;
    let height = first(context.invoke(&popup, "getHeight", &[])?);
    let height = context.numeric(&height)?;
    let core = first(context.call_global("getCore", &[])?);
    let screen = first(context.invoke(&core, "getScreenHeight", &[])?);
    let screen = context.numeric(&screen)?;
    let y = if bottom + height <= screen {
        bottom
    } else {
        (top - height).max(0.0)
    };
    let y = context.number(y)?;
    context.invoke(&popup, "setY", &[y])?;
    Ok(())
}

fn accept_suggestion(context: &mut Context<'_, '_>, panel: &Value) -> Result<bool> {
    let picker = context.get(panel, "luabuddyReloadPicker")?;
    if !popup_open(context, &picker)? {
        return Ok(false);
    }
    let popup = context.get(&picker, "popup")?;
    let selected = context.get(&popup, "selected")?;
    let id = first(context.invoke(&picker, "getOptionData", std::slice::from_ref(&selected))?);
    if id.is_nil() {
        return Ok(false);
    }
    let filter = first(context.invoke(&picker, "getFilterText", &[])?);
    if !filter.is_nil() {
        let query = context.text(&filter)?.to_lowercase();
        let label = first(context.invoke(&picker, "getOptionText", &[selected])?);
        if !context.text(&label)?.to_lowercase().contains(&query) {
            return Ok(false);
        }
    }
    context.invoke(&picker, "forceClick", &[])?;
    Ok(true)
}

pub(super) fn direction(context: &mut Context<'_, '_>, args: &[Value], down: bool) -> Result<()> {
    let entry = args.first().ok_or("Missing custom mod ID entry")?;
    let panel = context.get(entry, "parent")?;
    if !top_keyboard_modal(context, &panel)? {
        return Ok(());
    }
    let picker = context.get(&panel, "luabuddyReloadPicker")?;
    if !popup_open(context, &picker)? {
        let value = first(context.invoke(entry, "getText", &[])?);
        let text = context.text(&value)?;
        show_suggestions(context, &panel, &text)?;
    }
    if popup_open(context, &picker)? {
        context.invoke(
            &picker,
            if down {
                "onJoypadDirDown"
            } else {
                "onJoypadDirUp"
            },
            &[Value::default()],
        )?;
    }
    Ok(())
}

pub(super) fn prompt(context: &mut Context<'_, '_>, session: u64) -> Result<()> {
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
    let initial_geometry = geometry(context, 620.0, 300.0)?;
    let mut args = numbers(context, &initial_geometry)?;
    args.push(context.string("Reload a profile")?);
    args.push(context.string("")?);
    args.push(Value::default());
    args.push(Value::default());
    let panel = construct(context, "ISTextBox", &args)?;
    initialise(context, &panel)?;
    let color_picker = context.get(&panel, "colorPicker")?;
    context.invoke(&color_picker, "removeFromUIManager", &[])?;
    opaque(context, &panel)?;
    let prerender = context.index(&panel, "prerender")?;
    context.set(&panel, "luabuddyModalPrerender", &prerender)?;
    let prerender = callback(context, session, Action::ModalPrerender)?;
    context.set(&panel, "prerender", &prerender)?;
    let yes = context.boolean(true)?;
    context.set(&panel, "noEmpty", &yes)?;
    let snapshot = context.string(&serde_json::to_string(&choices).map_err(|e| e.to_string())?)?;
    context.set(&panel, "luabuddyReloadChoices", &snapshot)?;
    let width = first(context.invoke(&panel, "getWidth", &[])?);
    let width = context.numeric(&width)?;
    let entry = context.get(&panel, "entry")?;
    let height = first(context.invoke(&entry, "getHeight", &[])?);
    let height = context.numeric(&height)?;
    let title_height = first(context.invoke(&panel, "titleBarHeight", &[])?);
    let search_label_y = context.numeric(&title_height)? + 12.0;
    let small_height = label(context, &panel, "Search installed mods", search_label_y)?;
    let picker_y = search_label_y + small_height + 6.0;
    let mut picker_args = numbers(context, &[11.0, picker_y, width - 22.0, height])?;
    picker_args.push(panel.clone());
    picker_args.push(callback(context, session, Action::ReloadSelect)?);
    let picker = construct(context, "ISComboBox", &picker_args)?;
    child(context, &panel, &picker)?;
    context.invoke(&picker, "setEditable", std::slice::from_ref(&yes))?;
    field_text(
        context,
        &picker,
        "noSelectionText",
        "Search installed mods by name or ID",
    )?;
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
        let tooltip = context.string(&presentation::instructions(reason))?;
        context.invoke(&picker, "addOptionWithData", &[label, data, tooltip])?;
    }
    let zero = context.number(0.0)?;
    context.set(&picker, "selected", &zero)?;
    let tooltip = context.string(&presentation::instructions("Open this list and type to search installed names and IDs. Choosing a match copies its exact mod ID below. Reload still requires a loaded profile with cooperative cleanup."))?;
    let tooltips = context.table()?;
    context.set(&tooltips, "defaultTooltip", &tooltip)?;
    context.set(&picker, "tooltip", &tooltips)?;
    context.set(&panel, "luabuddyReloadPicker", &picker)?;
    let id_label_y = picker_y + height + 16.0;
    let label_height = label(context, &panel, "Mod ID to reload", id_label_y)?;
    let entry_y = context.number(id_label_y + label_height + 6.0)?;
    context.invoke(&entry, "setY", std::slice::from_ref(&entry_y))?;
    let button_y_value = context.numeric(&entry_y)? + height + 16.0;
    let button_y = context.number(button_y_value)?;
    let accept = context.get(&panel, "yes")?;
    let cancel = context.get(&panel, "no")?;
    context.invoke(&accept, "setY", std::slice::from_ref(&button_y))?;
    context.invoke(&cancel, "setY", std::slice::from_ref(&button_y))?;
    let button_height = first(context.invoke(&accept, "getHeight", &[])?);
    let required_height = context.numeric(&button_y)? + context.numeric(&button_height)? + 16.0;
    let layout = geometry(context, width, required_height)?;
    if layout[2] < width || layout[3] < required_height {
        context.invoke(&panel, "destroy", &[])?;
        return Err("Reload needs more space for the selected UI font. Increase the game window size or reduce its UI font size, then reopen it.".into());
    }
    for (method, value) in [
        ("setX", layout[0]),
        ("setY", layout[1]),
        ("setHeight", required_height),
    ] {
        let value = context.number(value)?;
        context.invoke(&panel, method, &[value])?;
    }
    let tooltip = context.string(&presentation::instructions("Type a mod ID to see installed matches. Use the arrows and Enter, or click a suggestion, to complete it. Unique ID matches ignore case. A custom ID can still be entered; runtime cleanup checks remain required."))?;
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
    let submit = callback(context, session, Action::ReloadSubmit)?;
    context.set(&accept, "onclick", &submit)?;
    context.set(&cancel, "onclick", &submit)?;
    context.invoke(&panel, "insertNewLineOfButtons", &[accept, cancel])?;
    show(context, session, &panel)?;
    let enter = callback(context, session, Action::ReloadEnter)?;
    context.set(&entry, "onCommandEntered", &enter)?;
    context.set(&entry, "target", &panel)?;
    let changed = callback(context, session, Action::ReloadTyped)?;
    context.set(&entry, "onTextChangeFunction", &changed)?;
    let down = callback(context, session, Action::ReloadDown)?;
    context.set(&entry, "onPressDown", &down)?;
    let up = callback(context, session, Action::ReloadUp)?;
    context.set(&entry, "onPressUp", &up)?;
    let other_key = callback(context, session, Action::EntryKey)?;
    context.set(&entry, "onOtherKey", &other_key)?;
    focus_entry(context, &panel, &entry)?;
    Ok(())
}

pub(super) fn select(context: &mut Context<'_, '_>, args: &[Value]) -> Result<()> {
    let panel = args.first().ok_or("Missing reload dialog")?;
    if !top_keyboard_modal(context, panel)? {
        return Ok(());
    }
    let picker = args.get(1).ok_or("Missing installed profile picker")?;
    let selected = context.get(picker, "selected")?;
    let id = first(context.invoke(picker, "getOptionData", &[selected])?);
    if !id.is_nil() {
        let entry = context.get(panel, "entry")?;
        context.invoke(picker, "setFilterText", &[Value::default()])?;
        context.set(panel, "luabuddyReloadLastInput", &id)?;
        context.invoke(&entry, "setText", &[id])?;
        focus_entry(context, panel, &entry)?;
    }
    Ok(())
}

pub(super) fn focus(context: &mut Context<'_, '_>, args: &[Value], gain: bool) -> Result<()> {
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
        dismiss(context, panel)?;
    }
    Ok(())
}

pub(super) fn dismiss(context: &mut Context<'_, '_>, panel: &Value) -> Result<()> {
    let picker = context.get(panel, "luabuddyReloadPicker")?;
    if !picker.is_nil() {
        if popup_open(context, &picker)? {
            context.invoke(&picker, "hidePopup", &[])?;
        }
        context.invoke(&picker, "setFilterText", &[Value::default()])?;
        let no = context.boolean(false)?;
        context.set(&picker, "expanded", &no)?;
    }
    Ok(())
}

pub(super) fn submit(
    context: &mut Context<'_, '_>,
    session: u64,
    args: &[Value],
    enter: bool,
) -> Result<()> {
    let (panel, accept) = if enter {
        let entry = args.first().ok_or("Missing reload entry")?;
        (context.get(entry, "parent")?, true)
    } else {
        let button = args.get(1).ok_or("Missing reload button")?;
        let internal = context.get(button, "internal")?;
        (
            context.get(button, "parent")?,
            context.text(&internal)? == "OK",
        )
    };
    if !top_keyboard_modal(context, &panel)? {
        return Ok(());
    }
    let submitted = context.get(&panel, "luabuddySubmitted")?;
    if context.truth(&submitted)? {
        return Ok(());
    }
    if !accept {
        let yes = context.boolean(true)?;
        context.set(&panel, "luabuddySubmitted", &yes)?;
        return close(context, &panel);
    }
    if enter && accept_suggestion(context, &panel)? {
        return Ok(());
    }
    let entry = context.get(&panel, "entry")?;
    let text = first(context.invoke(&entry, "getText", &[])?);
    let typed = context.text(&text)?;
    let id = match resolve_id(&typed, &choices(context, &panel)?) {
        Ok(id) => id,
        Err(reason) => {
            return notice(
                context,
                session,
                "Choose a mod ID",
                &reason,
                "Choose an exact installed ID from the matching list, or enter a different custom ID.",
                enter,
            );
        }
    };
    let yes = context.boolean(true)?;
    context.set(&panel, "luabuddySubmitted", &yes)?;
    close(context, &panel)?;
    let result = runtime::apply(context, session, Operation::Reload(id.clone()));
    match result {
        Ok(result) => notice_result(
            context,
            session,
            Feedback::Success,
            &format!("Reload result for {id}"),
            &result,
            "Check the profile's Runtime state and test its controls before continuing.",
            enter,
        ),
        Err(reason) => {
            let state = context.global("LuaBuddy")?;
            let statuses = context.get(&state, "mods")?;
            let status = context.get(&statuses, id.as_str())?;
            let failed = !status.is_nil() && context.text(&status)?.starts_with("failed:");
            notice_result(
                context,
                session,
                if failed {
                    Feedback::Error
                } else {
                    Feedback::Warning
                },
                &format!(
                    "Reload {} for {id}",
                    if failed { "failed" } else { "unavailable" }
                ),
                &reason,
                if failed {
                    "Reconnect before trying again. Inspect the profile's Runtime state and diagnostics; a failed replacement cannot generally be rolled back safely."
                } else {
                    "Reload requires an enabled profile already loaded by LuaBuddy and an upstream cleanup contract. Reconnect to load updated files when safe cleanup is unavailable."
                },
                enter,
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::resolve_id;
    use luabuddy_core::ModId;
    use std::collections::BTreeMap;

    fn choices(ids: &[&str]) -> BTreeMap<ModId, String> {
        ids.iter()
            .map(|id| (ModId::new(*id).unwrap(), format!("Title for {id}")))
            .collect()
    }

    #[test]
    fn typed_ids_ignore_case_when_unique_and_preserve_unknown_custom_ids() {
        let installed = choices(&["AutoMindful", "MÖD"]);
        assert_eq!(
            resolve_id("aUtOmInDfUl", &installed).unwrap().as_str(),
            "AutoMindful"
        );
        assert_eq!(resolve_id("möd", &installed).unwrap().as_str(), "MÖD");
        assert_eq!(
            resolve_id("ExternalCustom", &installed).unwrap().as_str(),
            "ExternalCustom"
        );
        assert_eq!(resolve_id("Auto", &installed).unwrap().as_str(), "Auto");
        assert!(resolve_id(" ", &installed).is_err());
    }

    #[test]
    fn exact_id_wins_and_ambiguous_case_insensitive_matches_require_selection() {
        let installed = choices(&["CaseMod", "casemod"]);
        assert_eq!(
            resolve_id("CaseMod", &installed).unwrap().as_str(),
            "CaseMod"
        );
        assert_eq!(
            resolve_id("casemod", &installed).unwrap().as_str(),
            "casemod"
        );
        let reason = resolve_id("CASEMOD", &installed).unwrap_err();
        assert!(reason.contains("More than one installed mod ID"));
        assert!(reason.contains("Choose the exact ID"));
    }
}
