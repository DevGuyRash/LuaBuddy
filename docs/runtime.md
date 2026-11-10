# Runtime loading and profile reference

LuaBuddy restores selected shared and client Lua through Project Zomboid's existing compiler, Kahlua interpreter, mod discovery, translation parser, and file mounting APIs. First-party lifecycle and integration behavior is Rust. Third-party Workshop Lua remains managed host content.

## Activation and session ownership

The native JVM agent creates the management interface for a valid client menu environment. Live client/shared restoration remains a multiplayer-client operation. Dedicated servers retain normal native loading. Native server-loaded mods are skipped, and their declared ordering remains a constraint.

Initial restoration runs before the real `OnGameStart` dispatch. `install --early-ui` opts into restoration before `OnCreateUI`; `OnGameStart` remains a fallback. The early option applies to the selected batch and takes effect after a normal game restart. Hooks must tolerate player UI that does not exist yet.

Runtime commands remain unavailable until actual startup dispatch finishes. The adapter hooks the synchronous `Event.trigger` boundary, including owner-thread queued dispatch. A method entry/exit pair around the public queueing API would not establish that timing. An exceptional completion does not falsely mark startup ready.

Each Lua environment owns a session, one retained require resolver, module values, generation metadata, and callback ownership. Reconnect creates a fresh environment. A captured resolver from a retired environment is refused. Foreign-thread and reentrant operations are refused before mutation.

## Profile fields

A profile is a UTF-8 Java-properties file with one native mod ID. Empty installations select no profiles. The Rust reader preserves Java escaping, continuations, and ASCII trimming semantics. Unknown keys, duplicate IDs, invalid booleans, cycles, and invalid metadata receive source-scoped errors.

| Field | Meaning |
| --- | --- |
| `id` | Exact native mod ID. |
| `enabled` | `true` by default. `false` prevents activation in a new session. It does not unload an existing generation. |
| `path` | Optional mod root relative to this profile's directory, or an absolute root. Native discovery remains the default. |
| `workshop` | Positive Workshop item ID used for the documented flat-root discovery fallback. |
| `requires` | Additional required IDs. Native `require=` metadata still applies. |
| `before`, `after` | Optional coexistence order, combined with native metadata. These do not create requirements. |
| `exclude` | Semicolon-separated globs relative to `media/lua`. |
| `excludeWhen.ModID` | Additional exclusions while that ID is natively active. |
| `preload` | Native or registered modules required before this profile's sources execute. |
| `mount` | `false` by default. Opts into publishing common/version file paths through the native virtual filesystem. |
| `replay` | Opt-in `OnGameBoot`, `OnGameStart`, or `OnLoad` callbacks from this profile's restored source files. |
| `hook` | A supported compiled Rust integration identifier. Unknown identifiers are refused. |
| `cleanup` | Bounded dotted path to an existing upstream callable, such as `Example.cleanup`. It is metadata, not evaluated source. |
| `live` | `auto`, `client-shared`, or `startup`. Controls whether source restoration is eligible or native startup is required. |

Only the fields in this table and scoped `excludeWhen.ModID` keys are accepted. The obsolete `plugin` key is an unknown option, including an empty `plugin=` assignment. The runtime has no executable Lua-hook compatibility path. Use a supported compiled Rust `hook` or an explicitly enabled Rust extension.

For example, a cooperating upstream mod can declare its native cleanup function without supplying first-party executable Lua.

```properties
id=Example
replay=OnGameStart
cleanup=Example.cleanup
```

Bundled integration rules and Rust hook behavior remain under `profiles/`. Adding a bundled template does not enable it in an existing user configuration.

`live=auto` is conservative. Content with native definitions, maps, animation assets, texture packs, or server Lua requires normal startup. `live=client-shared` explicitly requests only the client/shared portion and reports the native components that remain outside LuaBuddy's activation. `live=startup` refuses late restoration. The bundled profiles declare their partial client/shared scope explicitly; that does not claim complete activation of their server or startup-only features.

**Startup enable** and **Startup disable** edit the native default selection for the next ordinary loading path, with native dependency and dependent checks. They preserve the running world's selection and multiplayer server IDs. Restart confirmation uses the game's ordinary exit flow. Saving a future selection is distinct from activating the content in the current session.

## Source and require behavior

Native metadata selects common and version roots. Version files replace matching common files. The scanner reads shared and client Lua, excludes server Lua and shared translations, and recognizes case-insensitive Lua extensions. It reads upstream files in place without editing or vendoring them.

Dependencies and order are resolved before activation. All selected shared phases precede client phases. Each profile's post-load Rust hook completes before dependent client phases. Native preloads and scoped boot replay retain their defined positions.

Paths normalize slash, case, and the optional Lua extension. Shared and client scripts with the same module name remain distinct execution entries. Later ordered providers win logical-path collisions, while each owned source executes at most once in its generation. Shared aliases take precedence over client aliases. Unknown imports delegate to the game's captured original require function.

The first returned value is cached, including nil and false. Tables and closures keep object identity. Circular imports and failed modules do not retry in the same environment. Unrelated healthy profiles can continue, while a required profile's failure propagates to its dependents.

## Refresh and reload

**Load new profiles** validates the selected configuration, discovers native metadata, and compiles candidate sources before publishing mappings. Existing loaded generations, resolver identity, and cached values remain intact. Removing or disabling a running profile changes the next session's selection and does not claim an arbitrary unload.

A runtime addition cannot replace a running logical path or an effective import provider. It cannot require code to precede an already loaded native or local profile. Those cases require a compatible new connection or native startup.

Reload requires an enabled, locally loaded cooperating profile. The runtime captures the upstream cleanup callable and original activation context after successful activation. The context records the ID, original operation, runtime flag, shared session state, and activation IDs. A replacement does not substitute its context for the original generation's cleanup context.

The runtime prepares and compiles the replacement before cleanup. It refuses active dependents and any source filename shared by either the old or candidate generation with another running profile. It rejects a missing cleanup contract and unsupported native-loaded targets.

Cleanup must return exactly `false` with a reason before changing anything to refuse. The old generation and its diagnostic record remain intact. Returning exactly `true` permits replacement only after cleanup completed. An exception, nil, or another value makes cleanup uncertain. The generation is quarantined, further attempts are refused, and reconnect is required.

After accepted cleanup, LuaBuddy removes only callbacks from the old generation's uniquely owned upstream filenames through native event removal. Removal is verified. Foreign callbacks remain intact. Profile/helper callbacks and external references require upstream cooperation. The loader cannot generally roll back arbitrary third-party globals, wrappers, actions, or other side effects.

## Scoped replay and assets

Only opted-in callbacks originating in restored upstream files are replayed. Profile/helper callbacks and foreign sources are excluded. Native registration order is preserved. Callbacks removed after a snapshot are skipped. Runtime startup replay occurs after the new batch's client phases and hooks, in `OnGameStart` then `OnLoad` order. Initial restoration uses the real events still ahead.

Replay does not reproduce event arguments, previous world state, the entire startup lifecycle, or missing server code. Successful source initialization does not establish safe whole-mod reload.

Opt-in mounting exposes loose upstream files with native common/version precedence. It does not register every texture pack, map, animation set, item definition, sandbox page, or server component. It does not change the server's active mod IDs or checksum policy. The interface must distinguish eligible client/shared loading from required native startup, reconnect, or restart.

## Interface and diagnostics

The native Options controls preserve pending edits. Apply or discard those edits before profile mutation or loading. The manager uses the game's controls and owner-thread update callbacks. Repeated opening, dismissal, and no-op refresh preserve appropriate identity and focus. A new environment retires the old interface.

The environment writes `LuaBuddy/diagnostics.json` beneath the actual game cache. Reports retain the session identity and actual running generations while recording the latest operation, timings, target, source, stage, native game/JVM versions, profile metadata, and available Lua/Java traces. A refused replacement records an operation error without replacing the old loaded record. A write failure is logged without stopping healthy profiles.

Use `luabuddy diagnostics --cache-dir /path/to/cache` for the last report. Missing, stale, incomplete, or failed reports are distinct from a verified successful game action. Installed files alone do not establish runtime success.
