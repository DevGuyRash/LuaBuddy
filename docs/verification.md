# Verify LuaBuddy

Run the portable installer tests without a game installation.

```sh
python3 -m unittest discover -s tests -p 'test_*.py' -v
```

On a machine with Project Zomboid installed, run the full check. This compiles a temporary candidate and uses the selected game's Java classes, bundled Java runtime, and the actual ZombieBuddy agent patch.

```sh
python3 tests/check.py
```

Pass `--game-dir /path/to/ProjectZomboid` if discovery cannot select one installation. Pass `--offline` to require an existing verified dependency cache. On Windows, use `py -3` instead of `python3`. The extra Lua failure checks run when LuaJIT is on `PATH`. The native Kahlua integration checks run independently of LuaJIT.

The check runner creates and deletes its own temporary profiles and mod fixtures. It can download a verified framework dependency and writes `.local/compatibility.log`. It does not install LuaBuddy, replace the published helper build, change the active launcher, contact a game server, or modify Workshop files.

## Exercise the installed loader

Use a separate client cache and a disposable loopback server. Keep `Mods=` and `WorkshopItems=` empty and `DoLuaChecksum=true` in that server's settings. Use the game's bundled Java 25. On Linux, the normal Steam script puts `jre64/bin` on `PATH` before running `ProjectZomboid64`. Calling that executable with the host's newer Java can break ZombieBuddy's bytecode patches.

1. Build LuaBuddy and run `luabuddy.py install --mod AllInfo --mod AutoAll`.
2. Start the isolated client through the normal launcher configuration. If Steam is disabled for the test, make the test cache's local `mods` directory contain the installed mod roots. This lets native discovery find the mods without Steam.
3. Join the disposable server. Confirm that its Lua checksum completes and each mod has one `[LuaBuddy] ... restored before OnGameStart` line.
4. Hover a weapon and inspect AllTheInfo's numeric details. Select Auto All's **Read Everything Automatically** action on an unread magazine. Wait for its completion message.
5. Return to the main menu and rejoin in the same process. Confirm that each mod initializes once in the fresh environment and that the menus remain functional.
6. Stop the test client, server, and display. Remove their disposable caches. Keep the screenshots and selected runtime log outside the repository.

## Verification on November 2, 2026

The Linux check used Project Zomboid 42.21.0, bundled Java 25.0.1, and ZombieBuddy 2.3.4. The user's existing game session stayed running. Evidence is in `~/.local/state/app-verification/LuaBuddy/2026-11-02/` on the implementation machine.

| Check | Result |
| --- | --- |
| Portable CLI | Twenty-six CLI checks pass. They cover install/remove/idempotence, original backups, staged writes and rollback, launcher ownership, filename collisions, debug settings, read-only diagnostics, malformed reports, selected-field output, and the reported running framework version. |
| Native game integration | Passes dependency ordering and hook completion, compile and dependency failures, common/version overlays, shared/client phases, late imports, server-enabled skips, single-player/server isolation, and fresh Lua environments. |
| Native translations | Passes version precedence, German and English, fallback, and `%1` substitution. |
| Optional AllInfo profile | Excludes SelfTest and overlapping trait screens while retaining inventory providers. |
| Native diagnostics | Passes real Kahlua Lua/Java traces, nested closure source metadata, uppercase `.LUA` imports, empty/disabled/malformed configurations, report replacement on reconnect, and report-write failure isolation. The CLI reads the actual native JSON report. |
| Extra Lua failure checks | Passes `nil` and `false` caching, native delegation, failure without retry, unrelated continuation, circular imports, path overrides, and invalid hooks. |
| Arbitrary mod IDs in the real client | Two temporary mods load and assert native dependency order, common/version replacement, shared/client files with the same name, and a late cross-mod import. |
| Installed launcher | Loads the staged JARs through the normal launcher JSON. AllInfo restores 39 files and AutoAll restores 35 files. Native mod discovery finds the installed roots without pinned version paths. |
| Gameplay | AllTheInfo's weapon tooltip is visible. Auto All completes its normal read action with the ordinary user role and reports one item read. |
| Reconnect | Two world entries in the same normal-launcher process each pass checksums and initialize each mod exactly once. The second entry uses the ordinary user role. |
| Multiplayer diagnostic report | A separate client loads AllInfo, AutoAll, and an uppercase-extension probe while isolating a deliberately broken mod. The JSON report records three loaded mods and one failed mod with its actual filename, stage, Lua trace, and Java cause. The real diagnostic CLI reads that report and returns failure as expected. |
| Dependency updater | Sixty-seven portable Python tests cover CLI behavior, verified downloads/cache reuse, update checks, offline and corrupt-cache behavior, failed promotion, and source/game/Java-runtime/framework/helper provenance. A changed Java runtime invalidates cached verification without changing the game JAR. Real official 2.3.4 download and instrumented native checks pass on 42.21 and 42.22. See [compatibility](compatibility.md). |

The diagnostics pass kept its selected evidence in `~/.local/state/app-verification/LuaBuddy/2026-11-02-diagnostics/`. Its deliberately failing fixture was confined to the disposable client, not the user's configuration.

The workflow in `.github/workflows/check.yml` runs the installer fixtures on Windows and Linux with Python 3.10 and 3.14. It does not distribute or run the proprietary game. A Windows game session and every third-party mod or public-server combination remain outside this verification.

## Optional-profile verification on November 4, 2026

The full check passed 69 portable tests and the native ZombieBuddy-instrumented startup gate on Project Zomboid 42.21.0 with Java 25 and ZombieBuddy 2.3.4. New native cases exercise opt-in file mounting, version-over-common assets, unchanged active mod IDs, exclusion of server Lua, replay after client initialization, native callback registration/removal, nested and foreign imports, callback failure attribution, flat Workshop metadata, recovery after a failed lookup, native registered-directory precedence, and preservation of native symlink aliases. Fresh installation selects no profiles; every bundled addition installs independently and preserves existing AllInfo/AutoAll files, including user edits.

A separate native Steam probe resolved Easy Pickings from its bundled profile without a `path` override, read its two upstream Lua sources in place, and verified that subsequent native metadata lookup still succeeds. This covers the default Workshop fallback rather than only the explicit-path fixture.

An isolated Linux multiplayer client joined an unmodified loopback server with `Mods=` and `WorkshopItems=` empty and `DoLuaChecksum=true`. It loaded all 13 bundled profiles, including AutoAll, AllInfo, ReloadAllAmmo, AutoFishing, and the NeatUI dependency, plus one independent verification mod: 14 loaded, zero failures. Checks inside the live game confirmed the existing inventory pages have NeatUI controls, the saved forage key and alarm setting are restored, the five UI images and AutoFishing icon resolve through native texture loading, AutoAll mechanics and AllInfo providers remain initialized, and the active server mod list stays empty.

The no-Steam client used installed mod roots linked into its disposable cache's native `mods` directory. Versioned profiles used native discovery; only the flat Easy Pickings profile needed its documented explicit path. Native image-prefix validation remains intact. Evidence is retained outside the repository under `~/.local/state/app-verification/LuaBuddy/2026-11-04-optional-profiles/` on the verification machine.

These checks establish loading, initialization, resource access, settings restoration, and the tested combination. They do not prove every upstream gameplay feature, optional third-party combination, or server configuration. AutoFishing's sandbox-page registration and custom seated bait animation require normal loading as documented in the README. NeatUI grid views can omit other mods' vanilla-row overlays; use its List view for those overlays. Windows gameplay and full preview-client gameplay remain unverified.

## Runtime loading verification on November 6, 2026

The full check passes 74 portable tests, the native ZombieBuddy-instrumented startup/runtime gate on Project Zomboid 42.21.0 with Java 25 and ZombieBuddy 2.3.4, and the extra Lua require/failure checks. Runtime cases exercise append/no-op resolver and object identity, cached `nil` and `false`, dependency reuse, scoped startup after the new batch, original cleanup contexts, native discovery-cache refresh, ordering/path/import-provider refusal, candidate compilation before cleanup and mounting, cooperative callback removal, cleanup/activation quarantine, startup failure propagation, reentrancy, owner-thread checks, retired environments, and cumulative diagnostics with stable session IDs.

An isolated ordinary-role multiplayer client joined a disposable server with `Mods=` and `WorkshopItems=` empty and `DoLuaChecksum=true`. Its initial selection included AutoAll, AllInfo, NeatUI Inventory and Framework, a cooperative test mod, and a read-only observation fixture. Through the real native Options buttons, it loaded Loot Tracker's 18 sources without reconnecting. Saved preferences, all nine images, its local multiplayer save initialization, map overlay and vanilla countdown worked. A repeated refresh did not rerun existing code or add UI records. A dirty native checkbox refused loading without discarding the pending edit.

The same session replaced the cooperative mod's version 1 with version 2, retaining one resolver and one upstream callback while leaving foreign callbacks and priority-mod initialization intact. A syntax-invalid replacement was refused before cleanup; version 2 remained callable and native mappings were not published. Fresh clients also exercised later generations through the keyboard path. Holding Enter submits one operation and leaves the result visible. Result dialogs use a bounded native rich-text viewport. Mod option record counts remained stable across repeated refresh/reload; new Loot Tracker controls appeared once.

The implementation was built before the new tests were written. Gameplay checks preceded native and portable additions for the remaining failure cases. Evidence and selected screenshots/logs stay outside Git under `~/.local/state/app-verification/LuaBuddy/2026-11-06T112117Z-runtime-loading/` on the verification machine. Only owned caches, clients, display and server were used. No user game was signaled, controlled, evaluated or restarted. The initially recorded user PID was later absent for an unknown reason, which was reported separately.

These checks cover the documented runtime loading and cooperative-reload contract. They do not prove arbitrary Workshop unload, undeclared foreign-reference cleanup, every mod combination, native asset-cache replacement, Windows gameplay, or runtime behavior on the preview build. Existing bundled profiles do not claim safe reload merely because they load successfully. See [runtime loading](runtime.md) for the boundaries.

## True Line of Sight verification on November 7, 2026

The optional `TrueLineOfSight` integration uses native file mounting and the existing saved-preferences hook pattern. No loader, event replay, ordering rule, exclusion, or custom rendering code was added. The production profile and hook were built before test changes. The full offline check passes 74 portable tests, the instrumented Project Zomboid 42.21.0 / Java 25 / ZombieBuddy 2.3.4 native gate, and the extra Lua checks. The installer matrix checks this profile independently and alongside the existing selections, including preservation of edited AutoAll hooks and a fresh default with no selected mods.

An isolated Linux client joined a disposable vanilla server with `Mods=` and `WorkshopItems=` empty and `DoLuaChecksum=true`. True Line of Sight loaded on its own. A fresh connection loaded AutoAll and AllInfo, then the native **Load new profiles** button added True Line of Sight without reconnecting. Saved proximity, darkness, linger, and colour values seeded under the private cache's `Lua/ModOptions.ini` were restored. Translated native controls and the loose fallback image resolved. The environment session, require resolver, priority-mod objects, and callback counts remained intact across the addition and a no-op refresh.

Native outline flags and the configured blue colour were observed on two zombies in a controlled scene. The server console protected and positioned the disposable character and created the scene's crawlers; its role remained `user`. Ongoing callbacks did not trip the upstream safety circuit or disable fallback drawing. Native Options displayed the saved values after loading. Evidence, including fixture corrections and selected screenshots/logs, stays outside Git under `~/.local/state/app-verification/LuaBuddy/2026-11-07-true-line-of-sight/` on the verification machine. All input and process diagnostics targeted owned test processes and a private display.

These checks cover integration, preferences, resource access, native outline activation, runtime addition, and the tested priority-mod combination. They do not establish every upstream building, vehicle, staircase, or canopy case, split-screen support, Windows gameplay, or safe whole-mod reload. The integration adds no cleanup hook and does not change upstream visibility rules.

## Batch Craft Recipe and Nutrition Helper verification on November 7, 2026

The integrations add two optional profiles and one native saved-preferences hook. Nutrition Helper uses the existing scoped startup replay and optional `before=AllInfo` ordering. No loader code, rendering code, exclusions, or dependencies were added. The production changes were built before test changes. `python3 tests/check.py --offline` passes 74 portable tests, the instrumented Project Zomboid 42.21.0 / Java 25 / ZombieBuddy 2.3.4 native checks, and the extra Lua checks. The installer matrix covers each addition independently, preservation of existing edits, 19 combined selections, and a fresh installation with none selected.

Each mod loaded alone through the native **Load new profiles** button in an isolated ordinary-role Linux multiplayer client. The disposable server had `Mods=` and `WorkshopItems=` empty and `DoLuaChecksum=true`. Batch Craft's three menu modes consumed one, selected, and all remaining nail boxes, with the native recipe producing 100 nails per box. Multiple-recipe submenus retained all three modes. Its upstream tag changes affected all 969 craft recipes. A native blade-based canned-food recipe completed without a nearby craft surface on the vanilla server. This one case does not establish every station-dependent recipe or server rule.

Nutrition Helper attached its button to the existing Health view after runtime loading. Saved nondefault preferences, the panel toggle, nutrient bars, food ratings, and calorie-threshold tooltip worked. Eating a quarter portion through the native menu increased the live calorie value by 42.5. A repeated refresh retained the same button, panel, resolver, and session without replaying a foreign startup callback. Adding Nutrition Helper after AllInfo was running failed before changing the renderer, priority-mod globals, or Health view, as required by its ordering rule.

A fresh connection loaded both additions with AutoAll and AllInfo, plus the observation fixture: five loaded, zero failures. Nutrition Helper's food ratings appeared together with AllInfo's spoilage and cooking rows. AllInfo's weapon details, AutoAll's completed magazine-reading action, AutoAll's box-opening options, Batch Craft's batch action, and Nutrition Helper's Health panel remained functional. A no-op refresh preserved the live renderer and UI objects. No user game was controlled or restarted. Evidence stays outside Git under `~/.local/state/app-verification/LuaBuddy/2026-11-07-batch-nutrition/` on the verification machine.

The inspected Nutrition Helper source leaves its immersion-cue event registrations commented out. Missing server sandbox skill gates default to zero, so food ratings are not necessarily restricted to Nutritionist characters. The integration preserves those upstream behaviors. These checks do not establish every recipe, fetched-item return path, third-party combination, Windows gameplay, or safe whole-mod reload. Neither profile adds cleanup support.

## Quick Build Menu verification on November 8, 2026

The optional `QuickBuildMenu` profile preloads the native build action and uses a profile hook for saved options and startup favorite preservation. No core code, rendering code, replay, mounting, dependency, or ordering rule was added. The complete production change was built before new tests. `python3 tests/check.py --offline` passes 74 portable tests, the instrumented Project Zomboid 42.21.0 / Java 25 / ZombieBuddy 2.3.4 native checks, and the extra Lua checks. The installer matrix covers independent installation, hook copying, existing edits, 20 combined selections, and an empty fresh default.

An isolated ordinary-role Linux client joined a disposable vanilla server with `Mods=` and `WorkshopItems=` empty and `DoLuaChecksum=true`. The native **Load new profiles** button added Quick Build Menu alone without reconnecting. Saved nondefault removal-confirmation, multiple-tracker, and F8 key settings loaded. Native building favorites, translated context menus, recipe icons, material counts, the XP estimate, Last, and the tracker worked. Two chained floor placements and four queued placements consumed exactly six planks and six nails and created six native floors. The queue exhausted its stock and drained its actions and cursor. A tracker opened from the native handcraft window crafted one nail box into 100 nails after that window closed.

An unmodified upstream startup reproduced deletion of a handcraft favorite selected through the native UI. With the profile hook, valid handcraft and building favorites survived startup, while an obsolete favorite was still removed and explicit false and unrelated values remained. A final connection with only Quick Build Menu and the observer confirmed that protection works alone and the favorites persist across reconnects. The native regression executes the actual hook in Kahlua using real recipe objects, favorite-key generation, and native event dispatch. Player, options, and recipe-list fixtures isolate the remaining cases: cleanup self-removal, an explicit unfavorite after the snapshot, unchanged and empty favorites without extra broadcasts, foreign callbacks, drained temporary callbacks, and runtime activation without protective startup registration.

A fresh connection loaded Quick Build Menu with AutoAll, AllInfo, CraftPlanner, and the observation fixture: five loaded, zero failures. A native Track button selected a floor recipe, and a Quick Build placement completed it through CraftPlanner's wrapper. AllInfo's build and weapon details and AutoAll's completed native reading action remained functional. No-op refresh retained the build wrapper, option handles, resolver, session, and callback counts. The inspected upstream files remain unchanged. Evidence and fixture corrections stay outside Git under `~/.local/state/app-verification/LuaBuddy/2026-11-08-quick-build/` on the verification machine.

These checks do not establish every recipe, rotation, workstation, fetched-item path, optional combination, Windows gameplay, or safe whole-mod reload. Queue stock is an upstream launch-time snapshot. This integration does not add cancellation of already queued native actions or independent split-screen chain state. The upstream metadata limits the current mod to game versions 42.20 through 42.21.99. LuaBuddy skips server-loaded copies, so its profile protection does not alter their upstream cleanup.

## NeatUI window-controls regression on November 8, 2026

The original late rebuild recreated anchored controls while the native page still had an unapplied saved-size change. A 620×420 loot window created from the native 455×300 size moved its replacement pin and resize controls another 165 pixels right and 120 pixels down. NeatUI's gear followed the displaced pin, while its resize grip was drawn at the visible window edge. The rebuild also reset saved pin state through native `createChildren()`.

The profile now calls native `recalcSize()` after clearing old children and before upstream recreation, then restores a previously pinned page with native `setPinned()`. No core, upstream, event, ordering, or rendering code changed. The complete production correction was built before new regression tests. The full offline runner passes 74 portable tests, the instrumented 42.21.0 / Java 25 / ZombieBuddy 2.3.4 native gate, and extra Lua checks. The regression executes the actual profile using real native `UIElement` parents, anchored children, and deferred resize, with explicit page/player/options boundaries. It checks hand-calculated final control bounds, preserved dimensions, true and false pin states, and already-built/disabled guards.

An isolated ordinary-role Linux client reproduced the original displacement and pin reset using saved layouts. With the corrected profile, actual corner drags resized both windows, loot unpin/re-pin worked, and its gear opened settings and saved a changed scrolling option. Reconnecting retained geometry, mixed pin states, and the saved setting. A combination with NeatUI Framework, AutoAll, AllInfo, Easy Drop'n'Loot, LootSense Storage, and the observer loaded seven profiles without failure. Loot resize, pinning, settings, and no-op refresh remained functional. Evidence and fixture corrections stay outside Git under `~/.local/state/app-verification/LuaBuddy/2026-11-08-neatui-controls/` on the verification machine. No user game was controlled or restarted. Windows gameplay and every optional combination remain unverified.
