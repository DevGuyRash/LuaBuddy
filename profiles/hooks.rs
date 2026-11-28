use crate::{
    game, log,
    lua::{Context, Result, Value, first},
    runtime::{self, Deferred},
};

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
    "PinyosShelfAware",
    "QuickBuildMenu",
    "RetroDashboard",
    "TrueLineOfSight",
    "WalkLootForage",
];

/// Formerly hooks that only restored saved settings.
const SETTINGS_ONLY: &[&str] = &[
    "KADropnLoot",
    "LootSenseQuickStack",
    "LootSenseStorage",
    "LootTrackerB42",
    "NutritionHelper",
    "P4AlarmSyndrome",
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
    match id {
        "AutoAll" => {
            let auto = context.global("AutoAll")?;
            if auto.is_nil() {
                return Err("AutoAll did not initialize".into());
            }
            for field in ["coreLoaded", "mechLoaded", "mechUIHooked"] {
                let value = context.get(&auto, field)?;
                if !context.truth(&value)? {
                    // An upstream rename must not turn a working mod into a failed one.
                    log::warn(
                        "mod.unexpected",
                        format!("AutoAll did not report {field}; its mechanics may be incomplete"),
                    )
                    .about(&owner.id)
                    .write();
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
                log::warn(
                    "mod.unexpected",
                    "AllInfo reported no info providers; its tooltips may be empty",
                )
                .about(&owner.id)
                .write();
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
            let group = first(context.invoke(&options, "getOptions", &[name])?);
            context.invoke(&group, "apply", &[])?;
        }
        "CraftPlanner" => {
            let planner = context.global("CraftPlanner")?;
            for i in players(context)? {
                let player =
                    first(context.call_global("getSpecificPlayer", std::slice::from_ref(&i))?);
                let inventory =
                    first(context.call_global("getPlayerInventory", std::slice::from_ref(&i))?);
                let loot = first(context.call_global("getPlayerLoot", &[i])?);
                if player.is_nil() || inventory.is_nil() || loot.is_nil() {
                    continue;
                }
                let get_state = context.get(&planner, "getState")?;
                let state = first(context.call(&get_state, std::slice::from_ref(&player))?);
                let visible = context.get(&state, "visible")?;
                if context.truth(&visible)? {
                    let function = context.get(&planner, "setVisible")?;
                    let yes = context.boolean(true)?;
                    context.call(&function, &[player, yes])?;
                }
            }
        }
        "PinyosShelfAware" => {
            // Its scanner uploads the whole registry once this timer is 5 s old. A
            // timer that never elapses keeps the scanner on this client.
            let shelf = context.global("ShelfAware")?;
            let scan = if shelf.is_nil() {
                Value::default()
            } else {
                context.get(&shelf, "Scan")?
            };
            if scan.is_nil() {
                log::warn(
                    "mod.unexpected",
                    "Shelf Aware! did not create its scanner; nothing is tracked automatically",
                )
                .about(&owner.id)
                .write();
            } else {
                let never = context.number(f64::INFINITY)?;
                context.set(&scan, "lastSyncMs", &never)?;
            }
        }
        "NeatUI_Inventory" => inventory(context, runtime)?,
        "NeatUI_Mechanics" => mechanics(context)?,
        "RetroDashboard" => dashboard(context)?,
        "QuickBuildMenu" if !runtime => runtime::after_startup(
            owner,
            Deferred::new(|context| {
                let Some((player, keys)) = favorites(context)? else {
                    return Ok(None);
                };
                Ok(Some(Deferred::new(move |context| {
                    restore_favorites(context, &player, &keys)?;
                    Ok(None)
                })))
            }),
        )?,
        // Saved settings are restored for every mod that registers some, so these
        // need nothing more. Existing profiles still name them.
        _ if SETTINGS_ONLY.contains(&id) => (),
        _ => (),
    }
    Ok(())
}

fn players(context: &mut Context<'_, '_>) -> Result<Vec<Value>> {
    let count = first(context.call_global("getNumActivePlayers", &[])?);
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
            let page = first(context.call_global(function, std::slice::from_ref(&i))?);
            if page.is_nil() {
                continue;
            }
            let search = context.get(&page, "ninvSearch")?;
            if !search.is_nil() {
                continue;
            }
            let pin = context.get(&page, "pin")?;
            let visible = first(context.invoke(&page, "getIsVisible", &[])?);
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
        let ui = first(context.call_global("getPlayerMechanicsUI", std::slice::from_ref(&i))?);
        if !ui.is_nil() {
            let width = first(context.invoke(&ui, "getWidth", &[])?);
            let height = first(context.invoke(&ui, "getHeight", &[])?);
            let setup = context.get(&window, "setup")?;
            context.call(&setup, std::slice::from_ref(&ui))?;
            context.invoke(&ui, "setWidth", &[width])?;
            context.invoke(&ui, "setHeight", &[height])?;
        }
        let data = first(context.call_global("getPlayerData", &[i])?);
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
        let data = first(context.call_global("getPlayerData", std::slice::from_ref(&i))?);
        if data.is_nil() {
            continue;
        }
        let old = context.get(&data, "vehicleDashboard")?;
        if old.is_nil() {
            continue;
        }
        context.invoke(&old, "removeFromUIManager", &[])?;
        let player = first(context.call_global("getSpecificPlayer", std::slice::from_ref(&i))?);
        let class = context.global("ISVehicleDashboard")?;
        let dashboard = first(context.invoke(&class, "new", &[i, player.clone()])?);
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
            Value::retain(env, value)
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

/// Quick Build Menu's startup cleanup clears native handcraft favorites, so they
/// are remembered before it runs and put back after.
fn favorites(context: &mut Context<'_, '_>) -> Result<Option<(Value, Vec<String>)>> {
    let player = first(context.call_global("getPlayer", &[])?);
    if player.is_nil() {
        return Ok(None);
    }
    let data = mod_data(context, &player)?;
    let recipes = context.jni(|env| {
        let manager = env
            .get_static_field(
                "zombie/scripting/ScriptManager",
                "instance",
                "Lzombie/scripting/ScriptManager;",
            )?
            .l()?;
        let value = env
            .call_method(
                manager,
                "getAllCraftRecipes",
                "()Ljava/util/ArrayList;",
                &[],
            )?
            .l()?;
        Value::retain(env, value)
    })?;
    let mut favorites = Vec::new();
    for recipe in game::java_list(context, &recipes)? {
        let key = context.jni(|env| {
            let key = env
                .call_static_method(
                    "zombie/entity/components/crafting/BaseCraftingLogic",
                    "getFavouriteModDataString",
                    "(Lzombie/scripting/entity/components/crafting/CraftRecipe;)Ljava/lang/String;",
                    &[jni::objects::JValue::Object(recipe.object())],
                )?
                .l()?;
            Value::retain(env, key)
        })?;
        let name = context.text(&key)?;
        let value = context.get(&data, &name)?;
        let exactly_true = context.jni(|env| {
            if value.is_nil() || !env.is_instance_of(value.object(), "java/lang/Boolean")? {
                return Ok(false);
            }
            env.call_method(value.object(), "booleanValue", "()Z", &[])?
                .z()
        })?;
        if exactly_true {
            favorites.push(name);
        }
    }
    Ok((!favorites.is_empty()).then_some((player, favorites)))
}

fn restore_favorites(context: &mut Context<'_, '_>, player: &Value, keys: &[String]) -> Result<()> {
    let data = mod_data(context, player)?;
    let mut restored = 0;
    for key in keys {
        if context.get(&data, key)?.is_nil() {
            let yes = context.boolean(true)?;
            context.set(&data, key, &yes)?;
            restored += 1;
        }
    }
    if restored > 0 {
        context.jni(|env| {
            env.call_method(player.object(), "transmitModData", "()V", &[])
                .map(drop)
        })?;
        log::info(
            "favorites.restored",
            format!("Put back {restored} handcraft favorites after startup cleanup"),
        )
        .write();
    }
    Ok(())
}

fn mod_data(context: &mut Context<'_, '_>, player: &Value) -> Result<Value> {
    context.jni(|env| {
        let value = env
            .call_method(
                player.object(),
                "getModData",
                "()Lse/krka/kahlua/vm/KahluaTable;",
                &[],
            )?
            .l()?;
        Value::retain(env, value)
    })
}
