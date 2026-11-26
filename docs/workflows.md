# Use LuaBuddy for common tasks

Choose **LUABUDDY** in the game's main menu or pause menu, or **Open LuaBuddy** under **Options > Mods**. These in-game workflows describe the verified native Linux client. The [experimental Windows x64 build](windows-cli.md) has no verified game session or in-manager Workshop browsing/downloads. Select a mod to see its state; the button under its details is the action that fits it where you are, and **More...** or right-click offers the others. Apply or cancel pending Options edits before changing a selection.

## Test an installed local mod

1. Put the upstream mod in your game's local `Zomboid/mods` directory. Keep its normal `common/` and version directories beneath the mod root.
2. Join a multiplayer server, open LuaBuddy, and type part of the mod's name in **Filter your mods**. Its details show its ID, folder, requirements, and anything LuaBuddy can't load for it.
3. If the game's own mod list already starts it, right-click it and choose **Don't start with the game**, then restart; LuaBuddy does not load a mod the game already runs.
4. Choose **Load now**. It turns the mod on, with the mods it needs, and loads it. The status line and the mod's details say whether it runs, failed, or has to start with the game.
5. If it failed before any of its code ran, such as a syntax error or a missing requirement, fix the cause and choose **Try again**. A failure after its code started running needs a reconnect.
6. Exercise the mod's actual controls. Loading successfully does not establish that every feature works, especially features requiring server code or native startup assets.
7. When testing is complete, choose **Turn off**. The source stays installed, and running code stays active until you reconnect.

For a source tree outside the native mod directories, select its root through the CLI. `--profiles-dir` defaults to the directory recorded by the installation, which `luabuddy verify` reports as `profiles`. Add `--game-dir /path/to/ProjectZomboid` if discovery does not identify one installation. A directory you name must already exist. Do not point `select` at the package's template directory. A new selection starts from the installed template for that mod ID when there is one. The `path` is the root above `common/` and the selected version directory.

```sh
luabuddy select --mod Example --path /path/to/Example
luabuddy profiles
```

Choose **Load now** for it, or **Tools > Load waiting mods**, in a ready multiplayer client. To disable that future selection through the CLI, run the following command.

```sh
luabuddy deselect --mod Example
```

## Pour and Shelf Aware on a multiplayer client

These two templates are in the source catalog after v0.1.0-rc.3, not in that prerelease archive. Install the upstream Workshop item for each integration you want. With a package built from the updated source, choose **Turn on** for each in LuaBuddy, or install with `--mod PourIntoContainer --mod PinyosShelfAware`. Both are optional. If the game's own mod list starts them, choose **Don't start with the game** for each and use the next normal game launch. Enabling a profile cannot undo options or callbacks already in memory, and a mod loaded by the server is not replaced. The CLI's `select` also starts from the installed template, but an ID-only profile made by hand or by an earlier release does not copy the exclusions. Existing personal profiles are never silently rewritten: if you used the old `live=startup` templates, back them up and replace only the two files with the new complete templates.

* **PourIntoContainer:** The original quick-pour and empty entries remain visible; showing an entry does not mean a server can complete its custom action. The late profile omits the upstream options module so it does not enable **Hide Vanilla Liquid Menus** in a fresh Lua environment. On a server without those actions, use the game's **Fluid > Transfer Liquid** or **Fluid > Empty** instead.
* **Shelf Aware!:** Open the reference window with the numeric keypad’s multiply key (the mod's default binding) or the book icon. Its manual marks, memos and read progress are available; automatic sightings, shelf locations, scanner-based loot feedback and death recovery are not. The original Settings tab's scan toggles do not enable the excluded scanner. The profile does not run its registry backup sender or server handler. It adds no local save file and cannot promise that manual entries survive reconnect. For searched-book locations, the separate optional **Loot Tracker** can search its own local records from **Map > Items**.

## Test a mod for one game launch

Queue an eligible mod for the next game process. This does not change normal profiles or copy the mod's source.

```sh
luabuddy select --mod Example --path /path/to/Example --once
luabuddy profiles
```

1. Confirm the CLI lists the mod under `queuedForNextLaunch`.
2. Launch the game normally with the current LuaBuddy runtime installed.
3. Join the multiplayer session in which you want to test eligible client or shared Lua.
4. Check the mod's runtime state and exercise its controls. Normal eligibility, dependencies, ordering, and server authority still apply.
5. Reconnect as needed. The selection belongs to this game process and survives a new Lua environment.
6. Quit normally. The next game process does not select this consumed registration.

LuaBuddy consumes queued registrations when it first initializes in that game process, including the main menu. It keeps their parsed profiles in memory across reconnects. Exiting from the menu or crashing also consumes that launch. Registrations queued after initialization belong to a later process. LuaBuddy removes its registration files before source execution, so cleanup needs no exit monitor. It leaves the original source and persistent selections intact.

An existing persistent profile with the same ID is a conflict. LuaBuddy refuses the one-launch registration rather than replacing that profile. If the persistent profile is added after queuing, the launch skips that registration and leaves it queued; `luabuddy profiles` lists it under `skippedAtNextLaunch`. A mod already selected by the native game remains governed by that separate selection. Disable its native startup selection if you want the mod absent from later launches.

If you queue a registration while the game is already running, its manager details show that it belongs to the next LuaBuddy launch. Ordinary late-selection edits refuse to convert the queued registration into a persistent profile. Cancel the queue first if you want to change its lifetime.

To cancel a queued registration before the next process claims it, run the following command. It cannot unload code from an existing process.

```sh
luabuddy deselect --mod Example --once
```

## Download and select a Workshop mod

1. Open **Workshop**. **Subscribed** lists your subscriptions and filters them as you type; **All of the Workshop** searches Steam. Scroll to the end for more results.
2. Select an item to read its description, size, and what it needs. **Open its page** shows the author's page in the Steam overlay or your browser; when neither is available the link is copied.
3. Choose **Subscribe and download**, or **Download the update** when one is available. The status line follows the download. A download that stops advancing for 90 seconds is reported; Steam may still continue it.
4. When it finishes, LuaBuddy reads your mod folders again. Choose **Show in My mods** to find what the item installed; one item can contain several mods.
5. Turn the mod on, or have it start with the game when its details say it must. Downloading alone does not turn anything on, and turning a mod off does not unsubscribe it.

## Reload a cooperating mod

**Reload** appears only for a mod that LuaBuddy loaded and whose profile names an upstream `cleanup` function. LuaBuddy compiles the current files first, then asks the running mod to clean up. If the mod declines, the old code keeps running. If cleanup fails partway, the mod is set aside until you reconnect. Reload waits while another running mod is built on top of it. See [the cleanup contract](runtime.md#refresh-and-reload).

## Investigate a failed action

1. Select the mod. **What went wrong** gives the reason, with the file and line for a script error. **Copy > The full error** copies it with its Lua and Java traces.
2. Open **Activity**. Each action is one row with its outcome; search to see every step, such as what was decided, how long each part took, which callbacks and globals a mod added, and the player's state at the time.
3. Use **Tools > Open the log folder** for `journal.jsonl`, the full record across game sessions, and `diagnostics.json`, the state the last operation left.
4. **Tools > Native extensions** appears when you have approved extensions. **Retry waiting patches** retries code you already approved; it approves nothing new.

```sh
luabuddy diagnostics --cache-dir /path/to/game-cache
luabuddy doctor --game-dir /path/to/ProjectZomboid --cache-dir /path/to/game-cache
```

Keep the actual error, mod ID, game version, and reproduction steps when reporting a problem. A loaded diagnostic status verifies source initialization, not every upstream gameplay feature. The [runtime reference](runtime.md) describes the limits of late activation.
