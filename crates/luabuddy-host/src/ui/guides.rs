use super::*;

struct Guide {
    label: &'static str,
    goal: &'static str,
    steps: &'static [(&'static str, &'static str)],
    check: &'static str,
    undo: &'static str,
}

struct Card<'a> {
    title: &'a str,
    text: &'a str,
    color: &'static str,
}

const GUIDES: &[Guide] = &[
    Guide {
        label: "Choose a loading path",
        goal: "Decide whether the mod can load in this multiplayer session or needs the game's next normal startup.",
        steps: &[
            (
                "Inspect the installed mod",
                "Choose Installed and select its exact mod ID. Read its loading state, requirements, and activation limits.",
            ),
            (
                "Choose this session or next launch",
                "Use Enable late followed by Load enabled for eligible client/shared Lua in multiplayer. Use Startup enable for native definitions, maps, or other startup-only content. Client loading cannot supply server code.",
            ),
            (
                "Read the result",
                "A saved selection is different from loaded code. A subscription delivers files; it does not select or load the mod.",
            ),
        ],
        check: "Confirm the selected mod's Runtime state. Read both Late selection and Next normal launch so you know which path owns its activation.",
        undo: "Use Disable late or Startup disable for the selection you changed. Running code remains active until reconnect or restart.",
    },
    Guide {
        label: "Test a local mod",
        goal: "Test eligible upstream client/shared Lua without copying its features into LuaBuddy or changing other selections.",
        steps: &[
            (
                "Make the source discoverable",
                "Install the mod normally under Zomboid/mods, or register a source root with luabuddy select --profiles-dir DIR --mod ID --path ROOT. ROOT contains common/ and its version folders.",
            ),
            (
                "Select its late-loading path",
                "Open Installed and select the ID. If testing only late loading, use Startup disable first when native startup is already selected. Reconnect or restart if native code is already running.",
            ),
            (
                "Load and exercise it",
                "Choose Enable late, then Load enabled. Apply or discard pending Options edits first. Exercise the mod's actual controls in the game.",
            ),
        ],
        check: "Confirm Runtime: loaded and a successful operation result. Missing server code or native assets remain outside client/shared activation.",
        undo: "Choose Disable late and reconnect when ready. The source directory and saved preferences remain intact.",
    },
    Guide {
        label: "Use one game launch",
        goal: "Select an eligible local mod for one game process, including reconnects, while keeping its original source intact.",
        steps: &[
            (
                "Queue the registration",
                "Before launching, run luabuddy select --profiles-dir DIR --mod ID --path ROOT --once. The ID must not duplicate a persistent late profile.",
            ),
            (
                "Start the game normally",
                "LuaBuddy consumes the registration when it first initializes, including the menu. Join the multiplayer session and test the eligible client/shared code. Reconnects retain this process's selection.",
            ),
            (
                "Finish the process",
                "Quit normally when finished. The next new game process has no consumed temporary selection. A registration queued after initialization belongs to a future process.",
            ),
        ],
        check: "The manager distinguishes a queued next LuaBuddy launch from the current process's temporary selection. Native startup selections remain separate.",
        undo: "Cancel an unclaimed queue with luabuddy deselect --profiles-dir DIR --mod ID --once. Cancel before changing it into a persistent late selection. Nothing hot-unloads already running code.",
    },
    Guide {
        label: "Install from Workshop",
        goal: "Deliver upstream files, inspect their actual mod IDs, and then choose the appropriate loading path.",
        steps: &[
            (
                "Find the upstream item",
                "Choose Workshop search or Subscribed and type a query. The highlighted view determines the search source. Open Workshop requests the author's original page.",
            ),
            (
                "Wait for verified delivery",
                "Choose Subscribe + download or Download / update. Wait for confirmed installation. Closing observation can leave Steam downloading when its native API cannot cancel.",
            ),
            (
                "Select an installed mod ID",
                "Open Installed. One Workshop item can contain several IDs. Inspect dependencies and activation limits before using Enable late and Load enabled, or Startup enable.",
            ),
        ],
        check: "Subscribed, installed, selected, and loaded are separate states. Confirm delivery and the intended Runtime or Next normal launch state.",
        undo: "Disable the corresponding late or startup selection for future sessions. Disabling a LuaBuddy selection does not unsubscribe or delete Workshop content.",
    },
    Guide {
        label: "Use native startup",
        goal: "Let Project Zomboid perform its ordinary startup loading for content that cannot activate late.",
        steps: &[
            (
                "Review the installed identity",
                "Choose Installed and read its native compatibility, required mods, and startup-discovery explanation.",
            ),
            (
                "Save the next native selection",
                "Choose Startup enable. Native dependency and dependent checks apply. This saves the next normal launch selection without changing the current world or server.",
            ),
            (
                "Restart when ready",
                "Save your progress, choose Restart game..., and launch normally through Steam. Server-controlled code still requires the server's own selection.",
            ),
        ],
        check: "Before restarting, confirm Next normal launch: enabled. After native startup, Runtime identifies code already supplied by the native game or server.",
        undo: "Choose Startup disable. The next normal startup excludes that selection after native dependent checks. Existing running code is unchanged.",
    },
    Guide {
        label: "Reload a profile",
        goal: "Replace a healthy LuaBuddy-loaded generation only when its upstream mod provides cooperative cleanup.",
        steps: &[
            (
                "Find the intended profile",
                "Choose Reload a profile. Search the installed picker, or type in the custom-ID field to see matching suggestions. Matching ignores case, while the selected exact ID is retained.",
            ),
            (
                "Confirm the exact ID",
                "Choose a suggestion with the mouse or native navigation. Unknown custom IDs can still be submitted. Ambiguous case-insensitive matches need an exact selection.",
            ),
            (
                "Submit and read the result",
                "The profile must be locally loaded and declare upstream cleanup. LuaBuddy validates the replacement before cleanup. Native-loaded targets, active dependents, and unsupported cleanup are refused.",
            ),
        ],
        check: "A refusal keeps the old generation. An uncertain cleanup or failed replacement requires reconnecting. An installed row alone does not establish safe reload support.",
        undo: "Close the result to keep playing when the old generation was retained. Reconnect to load changed files when safe reload is unavailable.",
    },
    Guide {
        label: "Troubleshoot an action",
        goal: "Use the actual reported state and failure reason to choose the next step.",
        steps: &[
            (
                "Read the action result",
                "Done confirms the stated operation. Working means observation is pending. Attention or Failed includes the remaining condition. A saved selection does not imply loaded code.",
            ),
            (
                "Inspect the selected mod",
                "Check source, dependencies, compatibility, loading lifetime, and activation limits. A loaded client/shared source does not reproduce every native startup or server feature.",
            ),
            (
                "Inspect diagnostics",
                "Open Patch diagnostics for capabilities, approved extensions, targets, and rejection reasons. Apply approved patches retries already approved code; it does not grant native-code approval.",
            ),
        ],
        check: "Read LuaBuddy/diagnostics.json and the game's console log for an actual initialization failure. Keep the mod ID, game version, exact reason, and reproduction steps.",
        undo: "Correct the reported condition, then retry. Apply or discard pending Options edits first. Disable only the selection you intend to change, preserving other profiles and source files.",
    },
];

pub(super) fn open(context: &mut Context<'_, '_>, session: u64) -> Result<()> {
    let geometry = geometry(context, 1040.0, 720.0)?;
    let mut args = numbers(context, &geometry)?;
    args.push(context.string("")?);
    args.push(context.boolean(false)?);
    let panel = construct(context, "ISModalRichText", &args)?;
    opaque(context, &panel)?;
    initialise(context, &panel)?;
    let chat = context.get(&panel, "chatText")?;
    let no = context.boolean(false)?;
    context.invoke(&chat, "setVisible", std::slice::from_ref(&no))?;
    let [_, _, width, height] = geometry;
    let font = context.jni(|env| {
        let value = env
            .get_static_field("zombie/ui/UIFont", "Small", "Lzombie/ui/UIFont;")?
            .l()?;
        env.new_global_ref(value).map(|value| Value(Some(value)))
    })?;
    let manager = first(context.call_global("getTextManager", &[])?);
    let small_height = f64::from(context.jni(|env| {
        env.call_method(
            manager.object(),
            "getFontHeight",
            "(Lzombie/ui/UIFont;)I",
            &[JValue::Object(font.object())],
        )?
        .i()
    })?);
    let medium_height = f64::from(context.jni(|env| {
        let medium = env
            .get_static_field("zombie/ui/UIFont", "Medium", "Lzombie/ui/UIFont;")?
            .l()?;
        env.call_method(
            manager.object(),
            "getFontHeight",
            "(Lzombie/ui/UIFont;)I",
            &[JValue::Object(&medium)],
        )?
        .i()
    })?);
    let header_height = medium_height + 8.0;
    let content_y = 8.0 + header_height + 8.0;
    let button_height = (small_height + 6.0).max(25.0);
    let footer_y = height - 10.0 - button_height;
    let content_height = footer_y - content_y - 12.0;
    let widest_topic = context.jni(|env| {
        let mut widest = 0;
        for guide in GUIDES {
            let text = env.new_string(guide.label)?;
            widest = widest.max(
                env.call_method(
                    manager.object(),
                    "MeasureStringX",
                    "(Lzombie/ui/UIFont;Ljava/lang/String;)I",
                    &[JValue::Object(font.object()), JValue::Object(&text)],
                )?
                .i()?,
            );
        }
        Ok(widest)
    })?;
    let topic_width = (f64::from(widest_topic) + 30.0).clamp(180.0, width * 0.4);
    let header_args = numbers(context, &[16.0, 8.0, width - 32.0, header_height])?;
    let header = construct(context, "ISRichTextPanel", &header_args)?;
    context.set(&header, "autosetheight", &no)?;
    child(context, &panel, &header)?;
    let margins = numbers(context, &[0.0, 0.0, 0.0, 0.0])?;
    context.invoke(&header, "setMargins", &margins)?;
    let mut title = presentation::Document::default();
    title.title("Help & guides");
    presentation::set_document(context, &header, &title)?;
    let list_args = numbers(context, &[16.0, content_y, topic_width, content_height])?;
    let topics = construct(context, "ISScrollingListBox", &list_args)?;
    child(context, &panel, &topics)?;
    bounded_list_tooltip(context, &topics)?;
    let padding = context.number(7.0)?;
    context.invoke(&topics, "setFont", &[font, padding])?;
    for (index, guide) in GUIDES.iter().enumerate() {
        let text = context.string(guide.label)?;
        let value = context.number(index as f64)?;
        let tooltip =
            presentation::tooltip(context, &format!("{}\n{}", guide.label, guide.goal), true)?;
        let tooltip = context.string(&tooltip)?;
        context.invoke(&topics, "addItem", &[text, value, tooltip])?;
    }
    let select = callback(context, session, Action::GuideSelect)?;
    context.invoke(&topics, "setOnMouseDownFunction", &[panel.clone(), select])?;
    context.set(&topics, "joypadParent", &panel)?;
    context.set(&panel, "luabuddyGuideTopics", &topics)?;
    let body_args = numbers(
        context,
        &[
            topic_width + 28.0,
            content_y,
            width - topic_width - 44.0,
            content_height,
        ],
    )?;
    let body = construct(context, "ISPanel", &body_args)?;
    context.set(&body, "background", &no)?;
    child(context, &panel, &body)?;
    let yes = context.boolean(true)?;
    context.invoke(&body, "setScrollChildren", std::slice::from_ref(&yes))?;
    context.invoke(&body, "addScrollBars", &[])?;
    for (method, saved, action) in [
        (
            "prerender",
            "luabuddyGuidePreRender",
            Action::GuidePreRender,
        ),
        ("render", "luabuddyGuideRender", Action::GuideRender),
    ] {
        let original = context.index(&body, method)?;
        context.set(&body, saved, &original)?;
        let function = callback(context, session, action)?;
        context.set(&body, method, &function)?;
    }
    let wheel = callback(context, session, Action::GuideWheel)?;
    context.set(&body, "onMouseWheel", &wheel)?;
    context.set(&panel, "luabuddyGuideBody", &body)?;
    let scroll_width = (width - 156.0) / 2.0;
    let up = button(
        context,
        session,
        &panel,
        [16.0, footer_y, scroll_width, button_height],
        "Scroll up",
        Action::GuideScrollUp,
    )?;
    let down = button(
        context,
        session,
        &panel,
        [
            width - 16.0 - scroll_width,
            footer_y,
            scroll_width,
            button_height,
        ],
        "Scroll down",
        Action::GuideScrollDown,
    )?;
    let ok = context.get(&panel, "ok")?;
    context.invoke(&panel, "clearJoypadButtonsList", &[])?;
    context.invoke(
        &panel,
        "insertNewLineOfButtons",
        std::slice::from_ref(&topics),
    )?;
    context.invoke(&panel, "insertNewLineOfButtons", &[up, ok, down])?;
    for (method, action) in [
        ("onGainJoypadFocus", Action::GuideGain),
        ("onLoseJoypadFocus", Action::GuideLose),
    ] {
        let function = callback(context, session, action)?;
        context.set(&panel, method, &function)?;
    }
    let base = context.global("ISPanelJoypad")?;
    let down = context.index(&base, "onJoypadDown")?;
    context.set(&panel, "onJoypadDown", &down)?;
    let first = context.number(1.0)?;
    context.set(&topics, "selected", &first)?;
    populate(context, &panel, 0)?;
    show(context, session, &panel)
}

pub(super) fn select(context: &mut Context<'_, '_>, args: &[Value]) -> Result<()> {
    let panel = args.first().ok_or("Missing guide window")?;
    let topics = context.get(panel, "luabuddyGuideTopics")?;
    let selected = context.get(&topics, "selected")?;
    let selected = context.numeric(&selected)? as usize;
    populate(context, panel, selected.saturating_sub(1))
}

fn populate(context: &mut Context<'_, '_>, panel: &Value, index: usize) -> Result<()> {
    let guide = GUIDES.get(index).ok_or("Select an available workflow")?;
    let body = context.get(panel, "luabuddyGuideBody")?;
    let old = context.get(&body, "luabuddyGuideCards")?;
    if !old.is_nil() {
        for (_, card) in context.entries(&old)? {
            context.invoke(&body, "removeChild", &[card])?;
        }
    }
    let cards = context.table()?;
    let mut position = 4.0;
    let mut index = 1;
    card(
        context,
        &body,
        &cards,
        &mut position,
        &mut index,
        Card {
            title: guide.label,
            text: guide.goal,
            color: presentation::ACTION,
        },
    )?;
    for (number, (title, text)) in guide.steps.iter().enumerate() {
        let title = format!("{}. {title}", number + 1);
        card(
            context,
            &body,
            &cards,
            &mut position,
            &mut index,
            Card {
                title: &title,
                text,
                color: presentation::TEXT,
            },
        )?;
    }
    card(
        context,
        &body,
        &cards,
        &mut position,
        &mut index,
        Card {
            title: "Check the result",
            text: guide.check,
            color: presentation::SUCCESS,
        },
    )?;
    card(
        context,
        &body,
        &cards,
        &mut position,
        &mut index,
        Card {
            title: "Undo or leave safely",
            text: guide.undo,
            color: presentation::WARNING,
        },
    )?;
    context.set(&body, "luabuddyGuideCards", &cards)?;
    let height = context.number(position + 12.0)?;
    context.invoke(&body, "setScrollHeight", &[height])?;
    let top = context.number(0.0)?;
    context.invoke(&body, "setYScroll", &[top])?;
    Ok(())
}

fn card(
    context: &mut Context<'_, '_>,
    body: &Value,
    cards: &Value,
    position: &mut f64,
    index: &mut i32,
    content: Card<'_>,
) -> Result<()> {
    let area = first(context.invoke(body, "getScrollAreaWidth", &[])?);
    let width = context.numeric(&area)? - 16.0;
    let args = numbers(context, &[4.0, *position, width, 10.0])?;
    let card = construct(context, "ISRichTextPanel", &args)?;
    child(context, body, &card)?;
    let yes = context.boolean(true)?;
    context.set(&card, "autosetheight", &yes)?;
    context.set(&card, "blockMouseWheel", &yes)?;
    context.set(&card, "background", &yes)?;
    let background = context.get(&card, "backgroundColor")?;
    let border = context.get(&card, "borderColor")?;
    for (table, r, g, b, a) in [
        (&background, 0.025, 0.035, 0.045, 1.0),
        (&border, 0.26, 0.30, 0.34, 1.0),
    ] {
        for (key, value) in [("r", r), ("g", g), ("b", b), ("a", a)] {
            let value = context.number(value)?;
            context.set(table, key, &value)?;
        }
    }
    let margins = numbers(context, &[14.0, 10.0, 20.0, 14.0])?;
    context.invoke(&card, "setMargins", &margins)?;
    let mut document = presentation::Document::default();
    document.heading(content.title, content.color);
    document.instruction(content.text);
    presentation::set_document(context, &card, &document)?;
    let height = first(context.invoke(&card, "getHeight", &[])?);
    *position += context.numeric(&height)? + 10.0;
    context.set(cards, &format!("card-{index}"), &card)?;
    *index += 1;
    Ok(())
}

pub(super) fn wheel(context: &mut Context<'_, '_>, args: &[Value]) -> Result<()> {
    let body = args.first().ok_or("Missing guide viewport")?;
    let delta = args.get(1).ok_or("Missing guide scroll direction")?;
    let delta = context.numeric(delta)?;
    scroll_body(context, body, -delta * 32.0)
}

pub(super) fn scroll(context: &mut Context<'_, '_>, args: &[Value], down: bool) -> Result<()> {
    let panel = args.first().ok_or("Missing guide window")?;
    let body = context.get(panel, "luabuddyGuideBody")?;
    let height = first(context.invoke(&body, "getHeight", &[])?);
    let distance = context.numeric(&height)? * if down { -0.8 } else { 0.8 };
    scroll_body(context, &body, distance)
}

fn scroll_body(context: &mut Context<'_, '_>, body: &Value, distance: f64) -> Result<()> {
    let current = first(context.invoke(body, "getYScroll", &[])?);
    let value = context.numeric(&current)? + distance;
    let value = context.number(value)?;
    context.invoke(body, "setYScroll", &[value])?;
    Ok(())
}

pub(super) fn prerender(context: &mut Context<'_, '_>, args: &[Value]) -> Result<()> {
    let body = args.first().ok_or("Missing guide viewport")?;
    let original = context.get(body, "luabuddyGuidePreRender")?;
    context.call(&original, std::slice::from_ref(body))?;
    let width = context.get(body, "width")?;
    let height = context.get(body, "height")?;
    let zero = context.number(0.0)?;
    context.invoke(body, "setStencilRect", &[zero.clone(), zero, width, height])?;
    Ok(())
}

pub(super) fn render(context: &mut Context<'_, '_>, args: &[Value]) -> Result<()> {
    let body = args.first().ok_or("Missing guide viewport")?;
    let original = context.get(body, "luabuddyGuideRender")?;
    context.call(&original, std::slice::from_ref(body))?;
    context.invoke(body, "clearStencilRect", &[])?;
    let width = context.get(body, "width")?;
    let height = context.get(body, "height")?;
    let zero = context.number(0.0)?;
    context.invoke(
        body,
        "repaintStencilRect",
        &[zero.clone(), zero, width, height],
    )?;
    Ok(())
}

pub(super) fn focus(context: &mut Context<'_, '_>, args: &[Value], gain: bool) -> Result<()> {
    let panel = args.first().ok_or("Missing guide window")?;
    let base = context.global("ISPanelJoypad")?;
    let method = context.index(
        &base,
        if gain {
            "onGainJoypadFocus"
        } else {
            "onLoseJoypadFocus"
        },
    )?;
    context.call(&method, args)?;
    if gain {
        let close = context.get(panel, "ok")?;
        context.invoke(panel, "setISButtonForB", &[close])?;
        context.invoke(panel, "setJoypadFocusTopLeft", &args[1..])?;
    } else {
        context.invoke(panel, "clearISButtons", &[])?;
        context.invoke(panel, "clearJoypadFocus", &args[1..])?;
    }
    Ok(())
}
