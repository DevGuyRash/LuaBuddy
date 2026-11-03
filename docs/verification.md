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
| Dependency updater | Sixty-six portable Python tests cover CLI behavior, verified downloads/cache reuse, update checks, offline and corrupt-cache behavior, failed promotion, and source/game/framework/helper provenance. Real official 2.3.4 download and instrumented native checks pass on 42.21 and 42.22. See [compatibility](compatibility.md). |

The diagnostics pass kept its selected evidence in `~/.local/state/app-verification/LuaBuddy/2026-11-02-diagnostics/`. Its deliberately failing fixture was confined to the disposable client, not the user's configuration.

The workflow in `.github/workflows/check.yml` runs the installer fixtures on Windows and Linux with Python 3.10 and 3.14. It does not distribute or run the proprietary game. A Windows game session and every third-party mod or public-server combination remain outside this verification.
