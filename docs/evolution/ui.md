# In-game mod management

The Rust interface adds `Manage mods`, `Load new profiles`, `Reload a profile`, `Patch diagnostics`, and `Apply approved patches` to the game's native Mod Options group. It uses the game's existing Kahlua runtime and UI factories. Its callbacks, profile changes, dependency selection, asynchronous operation handling, and navigation behavior are Rust code in `crates/luabuddy-host/src/ui.rs`.

## Player workflow

Open `Options > Mods > LuaBuddy > Manage mods`. The installed list uses native mod discovery and metadata. Select a row to inspect its identity, source, required mods, native version availability, current selection, and loader status. Startup-only definitions, maps, or animation directories produce a restart requirement before a live-load request.

Installed Workshop items with a root `mod.info` are discovered through Steam's actual installed-folder API and the game's metadata parser. Their source files stay in place. A flat item absent from the game's normal startup discovery can use its reviewed late profile, but `Startup enable` refuses to promise normal activation for that unsupported packaging.

The text entry applies to the selected discovery action. `Installed` filters all installed identities by title, description, mod ID, or Workshop ID. `Subscribed` with a search term reads every subscription page before presenting the complete filtered result. `Workshop search` reads available Workshop content through the initialized game's Steam context. Subscription and Workshop views have previous and next page controls. Workshop item dependencies remain Workshop identities until installed-mod discovery establishes their mod IDs. A Workshop item can contain several mod identities, so downloading an item does not choose every contained mod.

`Add / download` records that particular button action as the intent for the selected item. The download begins after Steam confirms the subscription. `Download / update` requests that selected item's content. The status area shows pending, progress, installed, and failed results. A verified installed directory is required before presenting download completion. The interface does not initialize, shut down, or take over the game's Steam callback pump.

After installation, `Enable late` selects the concrete mod identity for LuaBuddy's client/shared loader. Required installed mods are selected with it. Missing or incompatible dependencies and dependency cycles are refused. Existing profile text, permissions, logical property values, and Rust hooks are preserved. Immutable bundled templates are read from the installer's `luabuddy.templates` setting for newly selected integrations. An arbitrary mod without a bundled template receives an ID-only profile.

`Load enabled` requests the same runtime refresh as the native Options button. Existing loaded generations remain in place. The runtime validates dependencies, ordering, compilation, and live-load eligibility. `Disable late` changes the next environment's selection. Running third-party side effects remain until reconnect or restart, and enabled dependent profiles must be disabled first. Reviewed client/shared integrations display the startup components they intentionally leave inactive.

`Startup enable` and `Startup disable` change the game's native selection for the next normal startup. Required native dependencies are resolved before saving. Current loaded mods and server selections remain unchanged. This path supports content that requires native definitions, maps, animations, or other startup facilities. Restart remains explicit, and multiplayer server content still requires the server's approval.

`Reload a profile` accepts a configured mod ID and uses the runtime's explicit cooperative cleanup contract. A profile without that cleanup support receives an explanation requiring reconnect. A held Return key cannot submit the same reload dialog twice. `Restart game...` opens the game's existing quit confirmation. The player then relaunches normally through Steam.

`Patch diagnostics` displays the actual patch capabilities, extension approvals, target states, and failure reasons in the bounded native result dialog. `Apply approved patches` retries targets for extensions already loaded with exact approval. It does not reread new binaries or turn a successful retry request into a claim that every target was installed.

## Interface state

| Observation | Meaning |
| --- | --- |
| Subscribed | Steam confirmed the account subscription. Content can still be absent. |
| Downloading | Steam reports an active or pending download and available byte counts. |
| Installed | The adapter independently verified the item state and installation directory. |
| Enabled | The profile selection was persisted and reread successfully by the profile parser. |
| Loaded | The runtime's actual mod state reports a loaded generation. |
| Disabled | The next environment's profile selection is disabled. Running code can remain active. |
| Failed | The corresponding request failed or timed out. The interface retains the reason and allows a new request. |
| Restart required | Native startup assets cannot be activated through this late loader. |
| Reconnect required | Existing loaded code or cleanup policy prevents a safe operation in this environment. |

Repeated Workshop clicks are refused while an operation is pending. Queries can be cancelled through their native release path. Steam subscription and download cancellation is unavailable through this adapter, so the interface explains that closing the browser leaves the operation running. Reopening the browser retains its current operation and verified state.

One unavailable subscribed item does not discard healthy results. The list includes an unavailable row with the Workshop identity and Steam's failure reason. Subscription and download actions require verified result metadata. Descriptions remain inside a bounded native viewport with scrollbars, and changing pages restores the first result to view.

Pending native Options edits block profile changes and runtime operations. When new controls are added, `refresh_options` removes only records belonging to the old Mods panel, rebuilds it through native factories, and restores its scroll position. The runtime calls this after a changed activation, preserving a no-op refresh's panel identity.

The browser uses the native joypad navigation rows and a B-button close action. Native text-entry and button controls keep their keyboard/controller behavior. Rust handles Escape, dialog Return consumption, one-time reload submission, modal ownership, and restoration of the previous focus. Result messages use the native bounded rich-text viewport. The browser requires a game window of at least 740 by 480 pixels and refuses smaller layouts with an explicit explanation.

## Verification scope

The graphical verification run uses Project Zomboid `42.22.2`, bundled Java `25.0.5`, Linux x86_64, private game caches, and isolated displays. Recorded candidate hashes distinguish the artifacts used for each check. Actual native Options buttons, installed discovery, dependency selection, invalid disable refusal, pending Options preservation, keyboard dismissal, reload entry, restart confirmation, and read-only Workshop search have passed. The saved-profile checks preserve comments, permissions, and a final continued Java-properties value.

An initialized Steam client returned existing subscriptions across multiple pages. A native unavailable result `15` remained visible while healthy items and subsequent pages loaded. The actual long descriptions stayed inside their scrollable viewport after the rendering fix.

The installed filter found all three disposable fixture identities among the installed mods. Subscription search found a known item from the second subscription page, and Return repeated the selected subscription mode. Cancelling a pending subscription search preserved the previous results. Flat Easy Pickings metadata was found without a path override or file changes.

Native startup controls selected a disposable consumer and its required library, refused removal of the required library while the consumer remained selected, and preserved the late-profile bytes. A full normal client relaunch loaded both through the game's native path. Their fixture code proved execution and dependency order, and the interface showed `native-loaded` while the late profiles stayed disabled. Clearing the next startup selection left the current native activation intact. The native patch diagnostic controls also displayed the default empty approval state and actual supported capabilities.

The final graphical candidate for those checks was `7a29e2f6831b795df53cff8822c138dd399abc72e4e90982fbcb8d1d47c8d25e`. New subscription and download requests remain untested because the user explicitly chose read-only checks on already subscribed items. A physical controller has not been supplied. Windows and macOS graphical execution remain unverified. Evidence remains outside Git in the implementation run's graphical verification directory.

The final integration run additionally reproduced a foreign-tab ID collision after runtime AllInfo activation. Rebuilding the Mods view now preserves its original native record ID and position. Actual AllInfo-to-Mods switching and a subsequent profile load passed; no-op refresh retained tab order and IDs.
