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
