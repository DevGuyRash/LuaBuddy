use super::*;

pub(super) fn open(
    context: &mut Context<'_, '_>,
    session: u64,
    browser: &mut Browser,
) -> Result<()> {
    let id = selected(context, browser)?
        .workshop
        .ok_or("This local mod has no verified Workshop page.")?;
    let overlay = first(context.call_global("isSteamOverlayEnabled", &[])?);
    if context.truth(&overlay)? {
        let item = context.string(&id.get().to_string())?;
        context.call_global("activateSteamOverlayToWorkshopItem", &[item])?;
        return feedback(
            context,
            browser,
            Feedback::Info,
            "Workshop page requested in the Steam overlay. Press Shift+Tab to view the overlay if it is hidden.",
        );
    }
    let url = format!(
        "https://steamcommunity.com/sharedfiles/filedetails/?id={}",
        id.get()
    );
    let launched = context.jni(|env| {
        let value = env.new_string(&url)?;
        env.call_static_method(
            "zombie/network/DesktopBrowser",
            "openURL",
            "(Ljava/lang/String;)Z",
            &[JValue::Object(&value)],
        )?
        .z()
    })?;
    if launched {
        return feedback(
            context,
            browser,
            Feedback::Info,
            "Workshop page sent to your web browser. The Steam overlay is unavailable in this session.",
        );
    }
    feedback(
        context,
        browser,
        Feedback::Warning,
        "The Steam overlay is unavailable and the game could not start a browser. Copy the Workshop link instead.",
    )?;
    let geometry = geometry(context, 700.0, 320.0)?;
    let mut args = numbers(context, &geometry)?;
    args.push(context.string("")?);
    args.push(context.boolean(true)?);
    args.push(Value::default());
    args.push(callback(context, session, Action::CopyWorkshopLink)?);
    let panel = construct(context, "ISModalRichText", &args)?;
    field_text(context, &panel, "yesText", "Copy link")?;
    field_text(context, &panel, "noText", "Close")?;
    field_text(context, &panel, "luabuddyWorkshopLink", &url)?;
    initialise(context, &panel)?;
    opaque(context, &panel)?;
    let text = context.get(&panel, "chatText")?;
    rich(
        context,
        &text,
        &format!(
            "Open Workshop\n\nThe game could not open a browser. Choose Copy link, then paste this address into your browser.\n\n{url}"
        ),
    )?;
    show(context, session, &panel)
}

pub(super) fn copy(context: &mut Context<'_, '_>, args: &[Value]) -> Result<()> {
    let button = args.get(1).ok_or("Missing Workshop link action")?;
    let internal = context.get(button, "internal")?;
    if context.text(&internal)? != "YES" {
        return Ok(());
    }
    let panel = context.get(button, "parent")?;
    let url = context.get(&panel, "luabuddyWorkshopLink")?;
    let url = context.text(&url)?;
    context.jni(|env| {
        let value = env.new_string(&url)?;
        env.call_static_method(
            "zombie/core/Clipboard",
            "setClipboard",
            "(Ljava/lang/String;)V",
            &[JValue::Object(&value)],
        )?;
        Ok(())
    })?;
    let session = UI.with(|state| state.borrow().as_ref().map(|ui| ui.session));
    if let Some(session) = session {
        with_browser(context, session, |context, browser| {
            feedback(
                context,
                browser,
                Feedback::Success,
                "Workshop link copied. Paste it into your browser.",
            )
        })?;
    }
    Ok(())
}
