use crate::lua::{Context, Result, Value};

pub const IDS: &[&str] = &[
    "AllInfo",
    "AutoAll",
    "CraftPlanner",
    "EURY_CRAFTING",
    "KADropnLoot",
    "LootSenseQuickStack",
    "LootSenseStorage",
    "LootTrackerB42",
    "NeatUI_Inventory",
    "NeatUI_Mechanics",
    "NutritionHelper",
    "P4AlarmSyndrome",
    "QuickBuildMenu",
    "RetroDashboard",
    "TrueLineOfSight",
    "WalkLootForage",
];

pub fn after(
    context: &mut Context<'_, '_>,
    id: &str,
    runtime: bool,
    owner: &luabuddy_core::source::Owner,
) -> Result<()> {
    if !IDS.contains(&id) {
        return Err(format!("Unknown Rust profile hook {id}"));
    }
    if id != "AutoAll" && id != "RetroDashboard" {
        let api = context.global("PZAPI")?;
        let options = context.get(&api, "ModOptions")?;
        context.invoke(&options, "load", &[])?;
    }
    match id {
        "AutoAll" => {
            let auto = context.global("AutoAll")?;
            if auto.is_nil() {
                return Err("AutoAll did not initialize".into());
            }
            for field in ["coreLoaded", "mechLoaded", "mechUIHooked"] {
                let value = context.get(&auto, field)?;
                if !context.truth(&value)? {
                    return Err(format!("AutoAll Mechanics did not initialize: {field}"));
                }
            }
        }
        "AllInfo" => {
            let all = context.global("AllInfo")?;
            let providers = context.get(&all, "providers")?;
            if providers.is_nil()
                || context.jni(|env| env.call_method(providers.object(), "len", "()I", &[])?.i())?
                    <= 0
            {
                return Err("AllInfo providers did not initialize".into());
            }
            if runtime {
                let options = context.global("MainOptions")?;
                let main = if options.is_nil() {
                    Value::default()
                } else {
                    context.get(&options, "instance")?
                };
                if !main.is_nil() {
                    let tabs = context.get(&main, "tabs")?;
                    let built = context.get(&main, "allInfoTabBuilt")?;
                    if !tabs.is_nil() && !context.truth(&built)? {
                        let options_tab = context.get(&all, "OptionsTab")?;
                        let build = context.get(&options_tab, "build")?;
                        context.call(&build, std::slice::from_ref(&main))?;
                        let yes = context.boolean(true)?;
                        context.set(&main, "allInfoTabBuilt", &yes)?;
                    }
                }
            }
        }
        "EURY_CRAFTING" => {
            let api = context.global("PZAPI")?;
            let options = context.get(&api, "ModOptions")?;
            let name = context.string("EURY_CRAFTING")?;
            let group = first(context.invoke(&options, "getOptions", &[name])?)?;
            context.invoke(&group, "apply", &[])?;
        }
        "CraftPlanner" => {
            let planner = context.global("CraftPlanner")?;
            for i in players(context)? {
                let player =
                    first(context.call_global("getSpecificPlayer", std::slice::from_ref(&i))?)?;
                let inventory =
                    first(context.call_global("getPlayerInventory", std::slice::from_ref(&i))?)?;
                let loot = first(context.call_global("getPlayerLoot", &[i])?)?;
                if player.is_nil() || inventory.is_nil() || loot.is_nil() {
                    continue;
                }
                let get_state = context.get(&planner, "getState")?;
                let state = first(context.call(&get_state, std::slice::from_ref(&player))?)?;
                let visible = context.get(&state, "visible")?;
                if context.truth(&visible)? {
                    let function = context.get(&planner, "setVisible")?;
                    let yes = context.boolean(true)?;
                    context.call(&function, &[player, yes])?;
                }
            }
        }
        "NeatUI_Inventory" => inventory(context, runtime)?,
        "NeatUI_Mechanics" => mechanics(context)?,
        "RetroDashboard" => dashboard(context)?,
        "QuickBuildMenu" if !runtime => crate::runtime::schedule_favorites(context, owner)?,
        _ => (),
    }
    Ok(())
}

pub fn first(values: Vec<Value>) -> Result<Value> {
    Ok(values.into_iter().next().unwrap_or_default())
}

fn players(context: &mut Context<'_, '_>) -> Result<Vec<Value>> {
    let count = first(context.call_global("getNumActivePlayers", &[])?)?;
    let count = context.numeric(&count)? as i32;
    (0..count).map(|i| context.number(i as f64)).collect()
}

fn inventory(context: &mut Context<'_, '_>, runtime: bool) -> Result<()> {
    let neat = context.global("NeatInventory")?;
    let enabled = context.get(&neat, "enabled")?;
    if !context.truth(&enabled)? {
        return Ok(());
    }
    for i in players(context)? {
        let number = context.numeric(&i)? as i32;
        for (prefix, function) in [
            ("inventory", "getPlayerInventory"),
            ("loot", "getPlayerLoot"),
        ] {
            let page = first(context.call_global(function, std::slice::from_ref(&i))?)?;
            if page.is_nil() {
                continue;
            }
            let search = context.get(&page, "ninvSearch")?;
            if !search.is_nil() {
                continue;
            }
            let pin = context.get(&page, "pin")?;
            let visible = first(context.invoke(&page, "getIsVisible", &[])?)?;
            let name = context.string(&format!("{prefix}{number}"))?;
            let layout = if runtime {
                let layout = context.table()?;
                context.invoke(&page, "SaveLayout", &[name.clone(), layout.clone()])?;
                Some(layout)
            } else {
                None
            };
            context.invoke(&page, "clearChildren", &[])?;
            context.invoke(&page, "recalcSize", &[])?;
            context.invoke(&page, "createChildren", &[])?;
            let manager = context.global("ISLayoutManager")?;
            let restore = context.get(&manager, "TryRestore")?;
            context.call(&restore, std::slice::from_ref(&name))?;
            if let Some(layout) = layout {
                context.invoke(&page, "RestoreLayout", &[name, layout])?;
            }
            context.invoke(&page, "setVisible", &[visible])?;
            let pinned = context.truth(&pin)?;
            context.invoke(&page, if pinned { "setPinned" } else { "collapse" }, &[])?;
        }
    }
    Ok(())
}

fn mechanics(context: &mut Context<'_, '_>) -> Result<()> {
    let neat = context.global("NeatMechanics")?;
    let window = context.get(&neat, "Window")?;
    let sidebar = context.get(&neat, "Sidebar")?;
    for i in players(context)? {
        let ui = first(context.call_global("getPlayerMechanicsUI", std::slice::from_ref(&i))?)?;
        if !ui.is_nil() {
            let width = first(context.invoke(&ui, "getWidth", &[])?)?;
            let height = first(context.invoke(&ui, "getHeight", &[])?)?;
            let setup = context.get(&window, "setup")?;
            context.call(&setup, std::slice::from_ref(&ui))?;
            context.invoke(&ui, "setWidth", &[width])?;
            context.invoke(&ui, "setHeight", &[height])?;
        }
        let data = first(context.call_global("getPlayerData", &[i])?)?;
        if !data.is_nil() {
            let equipped = context.get(&data, "equipped")?;
            if !equipped.is_nil() {
                let add = context.get(&sidebar, "add")?;
                context.call(&add, &[equipped])?;
            }
        }
    }
    Ok(())
}

fn dashboard(context: &mut Context<'_, '_>) -> Result<()> {
    for i in players(context)? {
        let data = first(context.call_global("getPlayerData", std::slice::from_ref(&i))?)?;
        if data.is_nil() {
            continue;
        }
        let old = context.get(&data, "vehicleDashboard")?;
        if old.is_nil() {
            continue;
        }
        context.invoke(&old, "removeFromUIManager", &[])?;
        let player = first(context.call_global("getSpecificPlayer", std::slice::from_ref(&i))?)?;
        let class = context.global("ISVehicleDashboard")?;
        let dashboard = first(context.invoke(&class, "new", &[i, player.clone()])?)?;
        context.invoke(&dashboard, "initialise", &[])?;
        context.invoke(&dashboard, "instantiate", &[])?;
        context.set(&data, "vehicleDashboard", &dashboard)?;
        let vehicle = context.jni(|env| {
            let value = env
                .call_method(
                    player.object(),
                    "getVehicle",
                    "()Lzombie/vehicles/BaseVehicle;",
                    &[],
                )?
                .l()?;
            if value.is_null() {
                Ok(Value::default())
            } else {
                env.new_global_ref(value).map(|v| Value(Some(v)))
            }
        })?;
        if !vehicle.is_nil() {
            let driver = context.jni(|env| {
                env.call_method(
                    vehicle.object(),
                    "isDriver",
                    "(Lzombie/characters/IsoGameCharacter;)Z",
                    &[jni::objects::JValue::Object(player.object())],
                )?
                .z()
            })?;
            if driver {
                context.invoke(&dashboard, "setVehicle", &[vehicle])?;
            }
        }
    }
    Ok(())
}
