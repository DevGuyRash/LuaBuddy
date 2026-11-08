# Load profiles during play

LuaBuddy provides manual runtime loading through the game's existing Mod Options. It keeps one Lua module registry and require resolver per multiplayer environment. It uses Project Zomboid for discovery, metadata, mounting and translations, and ZombieBuddy for callback ownership. There is no additional service, file watcher, network listener or dependency.

## Load a new selection

1. Install the upstream mod and select its profile using the existing installer or configuration files.
2. While connected, open **Options > Mods > LuaBuddy**. Apply or discard pending Options edits.
3. Select **Load new profiles** and read the result. New native mod preferences appear on the rebuilt Mods page.

The loader skips mods already loaded locally or by the server. Repeating a refresh does not reexecute them or replace cached module values. Removing or disabling a running profile does not unload it; those changes take effect in the next environment. A failed execution remains blocked until reconnecting because its effects cannot be rolled back.

Framework and JAR updates still require a full manual game restart. Installing the new helper cannot replace classes in an already-running JVM. Existing profiles and hook files are preserved by reinstalling; template changes do not overwrite local edits.

## Initialize a mod after startup

Code registered on `OnGameStart` has missed that event when a new profile loads during play. If upstream initialization is safe at that point, the profile can opt in:

```properties
id=Example
replay=OnGameStart
```

During normal initial restoration, the real event runs once. During runtime loading, only callbacks from that mod's restored upstream source files run, after the entire new batch's client scripts and `after` hooks. No global startup event is triggered. Other events, arguments and earlier game state are not reconstructed. Profile-hook and shared-helper registrations are outside this automatic ownership set; initialize those explicitly in hooks and clean up their exact registrations when needed.

The bundled Loot Tracker and Nutrition Helper profiles request this runtime initialization. A previously installed profile needs the additive `replay=OnGameStart` opt-in before loading Loot Tracker mid-session. Add it only after starting a runtime-capable loader; older loaded helpers reject that new replay value. An already-loaded Loot Tracker is skipped by refresh.

## Reload a cooperating profile

Select **Reload a profile** and enter its configured mod ID. The loader validates and compiles the replacement before calling the running generation's cleanup hook. It rejects missing or disabled selections, server-loaded targets, failed generations, active declared dependents, incompatible ordering, and conflicts with other loaded module providers.

An integration must provide an explicit cleanup hook using upstream APIs. For example:

```lua
return {
    after = function(context)
        Example.restorePreferences()
    end,
    unload = function(context)
        if not Example.canStop() then
            return false, "Finish the current action first."
        end
        Example.stop()
        return true
    end,
}
```

The **running** generation's hook receives its original activation context and owns stopping actions, closing windows, restoring wrappers, disposing owned globals, updating external references, and removing callbacks registered by the profile or shared helpers. Its `context.runtime` and `context.operation` still describe how it was activated, so cleanup of a startup-loaded generation sees `false` and `startup`. Return `false, reason` before changing anything to refuse cleanup. Return `true` only after successful cleanup. An exception or another return value makes cleanup uncertain and requires reconnecting.

After successful cooperation, LuaBuddy removes callbacks originating in that generation's uniquely owned upstream Lua files using ZombieBuddy's by-file callback snapshots and native event removal. It evicts only that generation's own registry entries and executes the replacement through the ordinary compiler and phase pipeline. Shared profile/helper callbacks require explicit native `Events.Name.Remove(callback)` calls in `unload`; the loader cannot assign their source filename to one profile safely.

If cleanup or activation fails, there is no automatic rollback or retry. The affected generation is marked failed and requires reconnecting. Unknown cross-mod references and wrapper chains remain the integration author's responsibility. Compilation alone does not prove that an arbitrary Workshop mod can reload. Bundled profiles do not gain cleanup hooks automatically.

Reload replaces client/shared Lua. It does not reverse native mounts, clear every texture cache, remove old asset mappings, load server code, or register startup-only maps, item scripts or animation sets. Use normal installation and startup paths for those components.

## Lua API

Trusted code on the owning game thread can use the same operations:

```lua
local ok, message = LuaBuddy.refresh()
local ok, message = LuaBuddy.reload("Example")
```

Each returns a boolean and a result message. Requests are refused before mutation during startup, from another thread, through a retired Lua environment, while an operation is already running, or while Options has pending changes. Native controls use these same functions. Loading is synchronous and may briefly pause the client while files compile.

Dependencies already running locally remain available. A newly requested `before` relationship or another declared order that would need to precede running code requires reconnecting. Runtime loading also refuses to replace an existing module path or change an existing effective import provider. It preserves shared/client identity, shared-preferred aliases, cached tables, `nil` and `false`, and require closures captured by already-running code.

Diagnostics retain the environment session and actual mod records, while recording the latest operation, reload target, timing and errors. A preflight refusal leaves the old generation callable. The report describes initialization operations, not every later gameplay callback.

## Design choices

The implementation extends the existing loader rather than adding a second registry or mod-specific core branches. Native Mod Options supply the interface and preference rendering. The native watcher is deliberately unavailable in ordinary non-debug multiplayer; a CLI mailbox would add polling, request lifetimes and dispatch code. ZombieBuddy's experimental HTTP subsystem is unnecessary for two local actions.

The game's native file-reload path helps development but cannot undo a whole mod's UI, globals, wrappers or LuaBuddy's cache. Cooperative reload therefore reuses the existing compile/load pipeline and explicit cleanup. See the [ZombieBuddy Lua API](https://github.com/zed-0xff/ZombieBuddy/blob/v2.3.4/doc/LuaAPI.md) and [Project Zomboid Lua API](https://projectzomboid.com/modding/zombie/Lua/LuaManager.GlobalObject.html) for the reused primitives.
