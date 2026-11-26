//! The in-game window: entry points, callbacks and native input.

mod activity;
mod catalog;
mod entry;
mod kit;
mod markup;
mod menu;
mod model;
mod options;
mod panes;
mod screen;
mod workshop;

pub use entry::attach;
pub use options::{install, pending as pending_options, refresh as refresh_options};

use crate::{
    callables, log,
    lua::{Context, Result, Value},
};
use jni::{JNIEnv, objects::JObject};
use screen::Screen;
use std::cell::RefCell;

const OWNER: &str = "LuaBuddy.UI";

macro_rules! events {
    ($($name:ident = $value:literal),+ $(,)?) => {
        #[derive(Clone, Copy, Debug, Eq, PartialEq)]
        #[repr(u8)]
        enum Event { $($name = $value),+ }

        impl Event {
            fn from(value: u8) -> Option<Self> {
                match value { $($value => Some(Self::$name),)+ _ => None }
            }
        }
    };
}

events! {
    Open = 1,
    EntryRender = 2,
    EntryJoypadGain = 3,
    EntryJoypadDown = 4,
    Update = 10,
    Draw = 11,
    KeyRelease = 12,
    KeyConsumed = 13,
    Button = 14,
    SearchKey = 15,
    SearchUp = 16,
    SearchDown = 17,
    JoypadDown = 18,
    JoypadUp = 19,
    JoypadPrevious = 20,
    JoypadNext = 21,
    JoypadLeft = 22,
    JoypadRight = 23,
    ListDraw = 30,
    ListPress = 31,
    ListMenu = 32,
    ListWheel = 33,
    ListMove = 34,
    ListLeave = 35,
    PageDraw = 40,
    PageWheel = 41,
    MenuPick = 50,
    MenuChild = 51,
    MenuTopmost = 52,
    Swallow = 60,
}

struct Ui {
    session: u64,
    environment: Value,
    screen: Option<Box<Screen>>,
    /// What a window rebuilt for a new screen size picks up again.
    carried: Option<screen::Carried>,
}

thread_local! { static UI: RefCell<Option<Ui>> = const { RefCell::new(None) }; }

fn begin(context: &Context<'_, '_>, session: u64) {
    UI.set(Some(Ui {
        session,
        environment: context.environment.clone(),
        screen: None,
        carried: None,
    }));
}

fn callback(context: &mut Context<'_, '_>, session: u64, event: Event) -> Result<Value> {
    let token = session
        .checked_mul(256)
        .and_then(|token| token.checked_add(event as u64))
        .ok_or("UI session identifier exhausted")?;
    callables::create(context.env, OWNER, session, token, dispatch)
        .map(|reference| Value(Some(reference)))
}

fn set_callback(
    context: &mut Context<'_, '_>,
    session: u64,
    object: &Value,
    method: &str,
    event: Event,
) -> Result<()> {
    let function = callback(context, session, event)?;
    context.set(object, method, &function)
}

fn current(context: &mut Context<'_, '_>, session: u64) -> Result<bool> {
    let environment = UI.with_borrow(|ui| {
        ui.as_ref()
            .filter(|ui| ui.session == session)
            .map(|ui| ui.environment.clone())
    });
    match environment {
        Some(environment) => {
            let active = context.environment.clone();
            context.same(&environment, &active)
        }
        None => Ok(false),
    }
}

/// Runs with the open window. Nothing runs when it is closed, or when a
/// callback arrives while one of its own actions is still running.
fn with_screen<T>(action: impl FnOnce(&mut Screen) -> Result<T>) -> Result<Option<T>> {
    let Some(mut screen) = UI.with_borrow_mut(|ui| ui.as_mut().and_then(|ui| ui.screen.take()))
    else {
        return Ok(None);
    };
    let session = screen.session;
    let result = action(&mut screen);
    if !screen.closed {
        UI.with_borrow_mut(|ui| {
            if let Some(ui) = ui.as_mut().filter(|ui| ui.session == session) {
                ui.screen = Some(screen);
            }
        });
    }
    result.map(Some)
}

fn dispatch<'vm>(
    env: &mut JNIEnv<'vm>,
    frame: &JObject<'vm>,
    count: i32,
    token: u64,
) -> Result<i32> {
    let session = token / 256;
    let event = Event::from((token % 256) as u8).ok_or("Unknown LuaBuddy interface event")?;
    let mut context = Context::current(env)?;
    let args = context.frame_arguments(frame, count)?;
    if !current(&mut context, session)? {
        return Ok(0);
    }
    let result = handle(&mut context, session, event, &args);
    let value = match result {
        Ok(value) => value,
        Err(error) => {
            report(&mut context, event, &error);
            None
        }
    };
    match value {
        Some(value) => context.frame_return(frame, &[value]),
        None => Ok(0),
    }
}

/// A drawing failure would repeat every frame, so each is reported once.
fn report(context: &mut Context<'_, '_>, event: Event, error: &str) {
    thread_local! { static REPORTED: RefCell<Vec<(u8, String)>> = const { RefCell::new(Vec::new()) }; }
    let drawing = matches!(
        event,
        Event::Draw
            | Event::ListDraw
            | Event::PageDraw
            | Event::Update
            | Event::EntryRender
            | Event::ListMove
            | Event::MenuTopmost
            | Event::MenuChild
    );
    let repeated = REPORTED.with_borrow_mut(|reported| {
        let key = (event as u8, error.to_owned());
        let repeated = reported.contains(&key);
        if !repeated && reported.len() < 64 {
            reported.push(key);
        }
        repeated
    });
    if !(drawing && repeated) {
        log::error("ui.failed", error)
            .with(serde_json::json!({ "event": format!("{event:?}") }))
            .write();
    }
    if !drawing {
        let _ = with_screen(|screen| screen.failed(context, error));
    }
}

fn handle(
    context: &mut Context<'_, '_>,
    session: u64,
    event: Event,
    args: &[Value],
) -> Result<Option<Value>> {
    let number = |context: &mut Context<'_, '_>, index: usize| -> Result<f64> {
        let value = args.get(index).ok_or("Missing native argument")?;
        context.numeric(value)
    };
    match event {
        Event::Open => open(context, session)?,
        Event::EntryRender => entry::render(context, args)?,
        Event::EntryJoypadGain => entry::joypad_gain(context, args)?,
        Event::EntryJoypadDown => {
            if entry::joypad_down(context, args)? {
                open(context, session)?;
            }
        }
        Event::Update => {
            if let Some(Some(carried)) = with_screen(|screen| screen.update(context))? {
                UI.with_borrow_mut(|ui| {
                    if let Some(ui) = ui.as_mut() {
                        ui.carried = Some(carried);
                    }
                });
                open(context, session)?;
            }
        }
        Event::Draw => {
            with_screen(|screen| screen.draw(context))?;
        }
        Event::ListDraw => {
            with_screen(|screen| screen.list.draw(context, &screen.theme))?;
        }
        Event::PageDraw => {
            with_screen(|screen| screen.page_text.draw(context))?;
        }
        Event::KeyRelease => {
            let key = number(context, 1)?;
            with_screen(|screen| screen.key(context, key))?;
        }
        // Input is held only by a live window; a leftover panel must never trap it.
        Event::KeyConsumed | Event::Swallow => {
            let live = UI.with_borrow(|ui| ui.as_ref().is_some_and(|ui| ui.screen.is_some()));
            return context.boolean(live).map(Some);
        }
        Event::SearchKey => {
            let key = number(context, 1)?;
            with_screen(|screen| screen.search_key(context, key))?;
        }
        Event::SearchUp | Event::SearchDown => {
            with_screen(|screen| screen.step(context, event == Event::SearchDown))?;
        }
        Event::Button => {
            let button = args.get(1).ok_or("Missing native button")?;
            let name = context.get(button, "internal")?;
            let name = context.text(&name)?;
            with_screen(|screen| screen.press(context, &name))?;
        }
        Event::JoypadDown => {
            let button = number(context, 1)?;
            with_screen(|screen| screen.joypad(context, button))?;
        }
        Event::JoypadPrevious | Event::JoypadNext => {
            with_screen(|screen| screen.step(context, event == Event::JoypadNext))?;
        }
        Event::JoypadUp | Event::JoypadLeft | Event::JoypadRight => {
            let right = event == Event::JoypadRight;
            with_screen(|screen| screen.sideways(context, right))?;
        }
        Event::ListPress => {
            let y = number(context, 2)?;
            with_screen(|screen| screen.list_press(context, y))?;
            return context.boolean(true).map(Some);
        }
        Event::ListMenu => {
            let y = number(context, 2)?;
            with_screen(|screen| screen.list_menu(context, y))?;
            return context.boolean(true).map(Some);
        }
        Event::ListWheel | Event::PageWheel => {
            let steps = number(context, 1)?;
            with_screen(|screen| {
                if event == Event::ListWheel {
                    screen.list.wheel(context, steps)
                } else {
                    screen.page_text.wheel(context, steps)
                }
            })?;
            return context.boolean(true).map(Some);
        }
        Event::ListMove | Event::ListLeave => {
            with_screen(|screen| {
                let y = if event == Event::ListMove {
                    Some(kit::ask_number(context, &screen.list.panel, "getMouseY")?)
                } else {
                    None
                };
                screen.list.point(y);
                Ok(())
            })?;
        }
        Event::MenuPick => {
            let index = number(context, 1)?;
            with_screen(|screen| screen.pick(context, index as usize))?;
        }
        Event::MenuChild => {
            let index = number(context, 1)?;
            return with_screen(|screen| Ok(screen.menu_child(index as usize)));
        }
        Event::MenuTopmost => {
            let (x, y) = (number(context, 1)?, number(context, 2)?);
            return with_screen(|screen| screen.menu_topmost(context, x, y));
        }
    }
    Ok(None)
}

fn open(context: &mut Context<'_, '_>, session: u64) -> Result<()> {
    let already = UI.with_borrow(|ui| ui.as_ref().is_some_and(|ui| ui.screen.is_some()));
    if already {
        return Ok(());
    }
    let carried = UI.with_borrow_mut(|ui| ui.as_mut().and_then(|ui| ui.carried.take()));
    let opened = Screen::open(context, session, carried)?;
    UI.with_borrow_mut(|ui| {
        if let Some(ui) = ui.as_mut().filter(|ui| ui.session == session) {
            ui.screen = Some(Box::new(opened));
        }
    });
    Ok(())
}
