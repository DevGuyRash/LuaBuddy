# Use LuaBuddy for common tasks

Open **Options > Mods > LuaBuddy > Manage mods**. The manager separates actions for **This session**, **Next launch**, **Workshop**, and **Diagnostics**. In **Help & guides**, choose a workflow to see its goal, steps, result check, and undo instructions. Action names use the same accent color in instructions and tooltips. Apply or discard pending Options edits before changing a selection.

## Test an installed local mod

1. Put the upstream mod in your game's local `Zomboid/mods` directory. Keep its normal `common/` and version directories beneath the mod root.
2. Open **Installed** and select the mod. Check its ID, source, dependencies, and eligibility in the details.
3. If you are testing late loading independently of native startup, use **Next launch > Startup disable** for that mod first. This changes the next native launch selection. It does not remove running code.
4. In a multiplayer client session, choose **This session > Enable late**. This saves a LuaBuddy selection. Saving the selection alone does not execute it.
5. Choose **Load enabled**. Read the operation result and the mod's runtime state. A refusal or failure includes its reason.
6. Exercise the mod's actual controls. Loading successfully does not establish that every feature works, especially features requiring server code or native startup assets.
7. When testing is complete, choose **Disable late**. The source stays installed, and running code stays active until the Lua environment ends. A reconnect uses the saved disabled selection.

For a source tree outside the native mod directories, select its root through the CLI. Supply the same profile directory that the installed runtime uses. The `path` is the root above `common/` and the selected version directory.

```sh
luabuddy select --profiles-dir /path/to/profiles --mod Example --path /path/to/Example
luabuddy profiles --profiles-dir /path/to/profiles
```

Use **Load enabled** in a ready multiplayer client to request loading. To disable that future selection through the CLI, run the following command.

```sh
luabuddy deselect --profiles-dir /path/to/profiles --mod Example
```

## Test a mod for one game launch

Queue an eligible mod for the next game process. This does not change normal profiles or copy the mod's source.

```sh
luabuddy select --profiles-dir /path/to/profiles --mod Example --path /path/to/Example --once
luabuddy profiles --profiles-dir /path/to/profiles
```

1. Confirm the CLI lists the mod under `queuedForNextLaunch`.
2. Launch the game normally with the current LuaBuddy runtime installed.
3. Join the multiplayer session in which you want to test eligible client or shared Lua.
4. Check the mod's runtime state and exercise its controls. Normal eligibility, dependencies, ordering, and server authority still apply.
5. Reconnect as needed. The selection belongs to this game process and survives a new Lua environment.
6. Quit normally. The next game process does not select this consumed registration.

LuaBuddy consumes queued registrations when it first initializes in that game process, including the main menu. It keeps their parsed profiles in memory across reconnects. Exiting from the menu or crashing also consumes that launch. Registrations queued after initialization belong to a later process. LuaBuddy removes its registration files before source execution, so cleanup needs no exit monitor. It leaves the original source and persistent selections intact.

An existing persistent profile with the same ID is a conflict. LuaBuddy refuses the one-launch registration rather than replacing that profile. A mod already selected by the native game remains governed by that separate selection. Disable its native startup selection if you want the mod absent from later launches.

If you queue a registration while the game is already running, its manager details show that it belongs to the next LuaBuddy launch. Ordinary late-selection edits refuse to convert the queued registration into a persistent profile. Cancel the queue first if you want to change its lifetime.

To cancel a queued registration before the next process claims it, run the following command. It cannot unload code from an existing process.

```sh
luabuddy deselect --profiles-dir /path/to/profiles --mod Example --once
```

## Download and select a Workshop mod

1. Use **Workshop search** or **Subscribed** to find the upstream item.
2. Inspect the item's actual description, dependencies, and available delivery information. Open the original Workshop page when you need the full upstream presentation.
3. Use **Workshop > Subscribe + download** or **Download / update** as appropriate.
4. Wait for confirmed download completion. A request, subscription, installed item, and loaded mod are separate states.
5. Find the installed native mod ID in **Installed**. A Workshop item can contain multiple mod IDs.
6. For eligible client or shared Lua, choose **This session > Enable late**, then **Load enabled**. Otherwise use **Next launch > Startup enable** and restart normally.
7. Check the reported runtime or next-launch state. Selecting a dependency or downloading content alone does not activate it.

**Open Workshop** requests the original item through the game's dedicated Workshop overlay action. When the overlay is unavailable, LuaBuddy uses the game's desktop-browser utility. If the game cannot start a browser, the dialog offers **Copy link**. A handoff message confirms the request, rather than claiming that a web page rendered successfully.

Cancelling observation can leave Steam downloading when the native API cannot cancel the request. Disabling a LuaBuddy selection does not unsubscribe or delete Workshop content.

## Reload a cooperating profile

1. Finish or discard pending Options edits.
2. Choose **Reload a profile**. Search the installed dropdown by name or ID, or type in the custom-ID field to see matching suggestions.
3. Choose a suggestion or keep typing a custom ID. Matching is case insensitive where it identifies one known ID. Exact IDs take precedence. If different IDs differ only in case, choose the intended ID explicitly.
4. Read its runtime state and cleanup support. The profile must be loaded by LuaBuddy and declare an upstream cleanup callable.
5. Submit the request. LuaBuddy validates and compiles the replacement before asking the old generation to clean up.
6. Read the result's reason and next step. A refusal preserves the old generation. Uncertain cleanup or failed replacement requires a reconnect.

The installed list helps identify a mod. It does not make every installed mod reloadable. Native-loaded code and arbitrary third-party side effects cannot be safely unloaded by selecting a row. See [the cleanup contract](runtime.md#refresh-and-reload).

## Investigate a failed action

1. Read the manager's action feedback and the selected mod's state. Distinguish a saved future selection from an operation that ran now.
2. Check the source and native dependency or compatibility explanation.
3. Open **Diagnostics > Patch diagnostics** for approved extensions, patch capabilities, targets, and rejection reasons. Structured reports use shared indentation and syntax colors.
4. Use **Apply approved patches** only after addressing the stated preparation or approval problem. It does not grant native-code approval.
5. Read the loader report when source initialization fails.

```sh
luabuddy diagnostics --cache-dir /path/to/game-cache
luabuddy doctor --game-dir /path/to/ProjectZomboid --cache-dir /path/to/game-cache
```

Keep the actual error, mod ID, game version, and reproduction steps when reporting a problem. A loaded diagnostic status verifies source initialization, not every upstream gameplay feature. The [runtime reference](runtime.md) describes the limits of late activation.
