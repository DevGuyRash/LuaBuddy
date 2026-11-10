# Client restoration and native startup selection

LuaBuddy keeps late client restoration separate from the game's normal startup selection. Enabling a late profile does not activate native definitions, maps, animation assets, texture packs, or server Lua. A Workshop subscription does not activate any of those components by itself.

## Late profile policy

The optional `live` property selects a bounded loading policy.

| Value | Result |
| --- | --- |
| `auto` | The adapter refuses late activation when it finds startup-only resources or server Lua. |
| `client-shared` | The profile explicitly requests client and shared Lua restoration. Native startup components remain unavailable through that operation. Server code remains governed by the server. |
| `startup` | The adapter always requires the game's normal startup loading path. |
| Absent | The selected profile inherits an explicit matching immutable catalog policy, when available. Otherwise it uses `auto`. |

An explicit value in the user's profile takes precedence over catalog metadata. Catalog inheritance never selects or enables another mod. The bundled integrations declare `live=client-shared` because their documented behavior is client and shared Lua restoration. This preserves that scope for already selected legacy profiles without rewriting user files or promising their native components.

The adapter examines known resource directories beneath each native common and version root. It recognizes definitions, maps, animations, animation sets, texture packs, and server Lua. Directory-name matching is case insensitive within those known path segments, including the actual `media/AnimSets` layout on Linux. It does not recursively search unrelated mod directories.

No resource scan establishes that arbitrary Lua code can load safely. Compilation, dependency ordering, host compatibility, initialization behavior, and cooperative cleanup still apply. Use `live=startup` when a mod requires an earlier lifecycle even if no startup-only directory is present.

## Native startup preferences

The browser's **Startup enable** and **Startup disable** buttons operate on `ActiveMods.getById("default")`. They do not modify `currentGame`, the native loaded-mod snapshot, the server's active IDs, checksums, or a save's mod list. A running mod remains active until the native lifecycle ends.

Enabling a startup selection resolves the game's actual required dependencies and rejects missing mods, incompatible versions, and cycles. Disabling a required dependency is refused while a selected native dependent remains. Unrelated default mod IDs and native map order are preserved.

The adapter prepares a private `ActiveMods` candidate and writes it with the game's `ActiveModsFile` serializer. It reads the staged file through the same native parser and compares both mod IDs and map order before publication. It preserves existing permissions, checks for independent file edits before replacing `mods/default.txt`, and updates the default in-memory selection after publication. A failed stage leaves the existing preferences untouched. An unexpected publication failure restores the previous file and default selection or reports uncertainty.

The path comes from the actual `Core.getMyDocumentFolder()` API. The observed game implements that API through its configured cache directory. The adapter refuses symlinked preference paths and files larger than four MiB rather than replacing an unexpected destination.

Restart through the game's ordinary launcher after a startup selection change. The game remains responsible for normal mod discovery, startup ordering, assets, and native activation. Multiplayer server requirements still require the server's approval and content.

## Verification scope

The standalone Rust consumer in `proofs/activation` exercises the actual game classes and bundled JVM with JNI checking and class verification. It uses disposable caches and synthetic native mod metadata. The consumer covers dependency selection, a 128-node dependency chain, missing dependencies, cycles, version rejection, native preference readback, Unicode IDs and cache paths, unrelated mod and map-order preservation, file permissions, repeated no-op selection, disable refusal, write failure, and unchanged current-game and active-server state. The native adapter uses an iterative dependency walk and scoped JNI local frames for these enumerations.

The eligibility cases cover ordinary client content, native definitions, server Lua, explicit policy, catalog inheritance, and `media/AnimSets` casing. These checks establish the adapter's behavior. Graphical control operation and a complete relaunch with newly selected real mods are separate verification steps.
