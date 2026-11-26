//! Shows an action tree through the game's own right-click menu.
//!
//! `ISContextMenu` normally belongs to a player and does not exist in the main
//! menu, so the window owns one and answers the two lookups that expect a player.

use super::{
    Event, callback, kit,
    model::{Act, Node},
};
use crate::lua::{Context, Result, Value};

pub struct Menu {
    root: Value,
    subs: Vec<Value>,
    acts: Vec<Act>,
}

pub fn open(
    context: &mut Context<'_, '_>,
    session: u64,
    nodes: &[Node],
    [x, y]: [f64; 2],
) -> Result<Menu> {
    let root = element(context, session, [x, y])?;
    let nothing = Value::default();
    context.set(&root, "parent", &nothing)?;
    for field in ["instanceMap", "subMenuPool"] {
        let table = context.table()?;
        context.set(&root, field, &table)?;
    }
    kit::set_number(context, &root, "subOptionNums", 0.0)?;
    let mut menu = Menu {
        root: root.clone(),
        subs: Vec::new(),
        acts: Vec::new(),
    };
    let pick = callback(context, session, Event::MenuPick)?;
    fill(context, session, &mut menu, &root, nodes, &pick)?;
    // Registered only once complete, and taken down again if showing it fails,
    // so a failure leaves nothing on screen.
    let shown = (|| {
        for value in std::iter::once(&root).chain(&menu.subs) {
            context.invoke(value, "addToUIManager", &[])?;
        }
        kit::call_flag(context, &root, "setVisible", true)?;
        kit::call_flag(context, &root, "setAlwaysOnTop", true)?;
        context.invoke(&root, "bringToTop", &[])?;
        for sub in &menu.subs {
            context.invoke(sub, "bringToTop", &[])?;
        }
        kit::set_flag(context, &root, "visibleCheck", true)
    })();
    match shown {
        Ok(()) => Ok(menu),
        Err(error) => {
            let _ = menu.close(context);
            Err(error)
        }
    }
}

fn element(context: &mut Context<'_, '_>, session: u64, [x, y]: [f64; 2]) -> Result<Value> {
    let args = kit::numbers(context, &[x, y, 1.0, 1.0, 1.5])?;
    let menu = kit::construct(context, "ISContextMenu", &args)?;
    kit::initialise(context, &menu)?;
    context.invoke(&menu, "clear", &[])?;
    context.invoke(&menu, "setFontFromOption", &[])?;
    kit::set_number(context, &menu, "player", 0.0)?;
    kit::set_flag(context, &menu, "wantMouseEvents", true)?;
    kit::set_flag(context, &menu, "forceVisible", true)?;
    for (method, event) in [
        ("getSubMenu", Event::MenuChild),
        ("topmostMenuWithMouse", Event::MenuTopmost),
    ] {
        let function = callback(context, session, event)?;
        context.set(&menu, method, &function)?;
    }
    Ok(menu)
}

fn fill(
    context: &mut Context<'_, '_>,
    session: u64,
    menu: &mut Menu,
    target: &Value,
    nodes: &[Node],
    pick: &Value,
) -> Result<()> {
    for node in nodes {
        let label = context.string(&node.label)?;
        let option = match &node.act {
            Some(act) => {
                menu.acts.push(act.clone());
                let index = context.number((menu.acts.len() - 1) as f64)?;
                kit::first(context.invoke(
                    target,
                    "addOption",
                    &[label, target.clone(), pick.clone(), index],
                )?)
            }
            None => kit::first(context.invoke(target, "addOption", &[label, target.clone()])?),
        };
        if option.is_nil() {
            continue;
        }
        if node.blocked.is_some() {
            kit::set_flag(context, &option, "notAvailable", true)?;
        }
        if let Some(text) = node.blocked.as_ref().or(node.hint.as_ref()) {
            let tooltip = kit::construct(context, "ISToolTip", &[])?;
            kit::initialise(context, &tooltip)?;
            kit::call_flag(context, &tooltip, "setVisible", false)?;
            kit::set_text(context, &tooltip, "description", text)?;
            context.set(&option, "toolTip", &tooltip)?;
        }
        if !node.children.is_empty() {
            let x = kit::ask_number(context, target, "getX")?;
            let y = kit::ask_number(context, target, "getY")?;
            let sub = element(context, session, [x, y])?;
            context.set(&sub, "parent", target)?;
            kit::call_flag(context, &sub, "setVisible", false)?;
            kit::call_flag(context, &sub, "setAlwaysOnTop", true)?;
            menu.subs.push(sub.clone());
            let number = context.number(menu.subs.len() as f64)?;
            context.set(&sub, "subOptionNums", &number)?;
            context.set(&option, "subOption", &number)?;
            let map = context.get(&menu.root, "instanceMap")?;
            let insert = kit::table_function(context, "insert")?;
            context.call(&insert, &[map, sub.clone()])?;
            fill(context, session, menu, &sub, &node.children, pick)?;
        }
    }
    Ok(())
}

impl Menu {
    pub fn act(&self, index: usize) -> Option<Act> {
        self.acts.get(index).cloned()
    }

    pub fn take_joypad(&self, context: &mut Context<'_, '_>, joypad: &Value) -> Result<()> {
        kit::set_number(context, &self.root, "mouseOver", 1.0)?;
        kit::set_flag(context, &self.root, "mouseOut", false)?;
        kit::focus_joypad(context, joypad, &self.root)
    }

    /// The submenu a native option refers to by number.
    pub fn child(&self, number: usize) -> Value {
        number
            .checked_sub(1)
            .and_then(|index| self.subs.get(index))
            .cloned()
            .unwrap_or_default()
    }

    fn all(&self) -> impl DoubleEndedIterator<Item = &Value> {
        std::iter::once(&self.root).chain(&self.subs)
    }

    /// The frontmost of these menus under the pointer.
    pub fn topmost(&self, context: &mut Context<'_, '_>, x: f64, y: f64) -> Result<Value> {
        for menu in self.all().rev() {
            if !kit::ask_flag(context, menu, "isVisible")? {
                continue;
            }
            let left = kit::ask_number(context, menu, "getX")?;
            let top = kit::ask_number(context, menu, "getY")?;
            let width = kit::ask_number(context, menu, "getWidth")?;
            let height = kit::ask_number(context, menu, "getHeight")?;
            if (left..left + width).contains(&x) && (top..top + height).contains(&y) {
                return Ok(menu.clone());
            }
        }
        Ok(Value::default())
    }

    pub fn visible(&self, context: &mut Context<'_, '_>) -> Result<bool> {
        for menu in self.all() {
            if kit::ask_flag(context, menu, "isVisible")? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Every step runs even when one fails, so no menu stays registered; the
    /// first failure is returned.
    pub fn close(&self, context: &mut Context<'_, '_>) -> Result<()> {
        // The game's own hide path puts back the cursor it showed for the menu.
        let mut result = context.invoke(&self.root, "hideAndChildren", &[]).map(drop);
        for menu in self.all() {
            for step in [
                context.invoke(menu, "hideToolTip", &[]).map(drop),
                kit::call_flag(context, menu, "setVisible", false),
                context.invoke(menu, "removeFromUIManager", &[]).map(drop),
            ] {
                result = result.and(step);
            }
        }
        result
    }
}
