# Windows x64: native CLI test package

This is an experimental package for **native Windows x64**, not the Linux Workshop archive, Proton, ARM64 or the 32-bit launcher. It uses the game's existing launcher configuration; it doesn't attach to a running process. The x64 Windows CI uses a real JDK and a disposable game-shaped directory. It does not run a licensed copy of Project Zomboid, test the in-game UI, or enable the native Steam Workshop browsing/download ABI. Use Steam itself for Workshop content.

1. Close Project Zomboid. Extract the **entire** Windows CI artifact to a folder you trust; do not run the EXE from inside the ZIP. Open PowerShell in the extracted folder. Windows setup is a CLI, not the Linux graphical wizard.
2. Verify the package, then ask where the game was found:

   ```powershell
   .\luabuddy.exe verify-package --package .
   .\luabuddy.exe discover
   ```

3. Install with no profiles selected, then check the files:

   ```powershell
   .\luabuddy.exe install --package .
   .\luabuddy.exe verify
   ```

   For custom Steam locations, multiple installations or discovery errors, use **Steam > Properties > Installed Files > Browse** and pass that exact directory, including quotes. These commands need write access to that game directory, not a system service:

   ```powershell
   $game = 'D:\SteamLibrary\steamapps\common\ProjectZomboid'
   .\luabuddy.exe discover --game-dir "$game"
   .\luabuddy.exe install --package . --game-dir "$game"
   .\luabuddy.exe verify --game-dir "$game"
   ```

4. Use Steam's **normal** launch option (`ProjectZomboid64.exe`); the alternate BAT launcher is not this integration path. Do not add a second `-agentpath` or change Steam launch options. The installer never edits `ProjectZomboid64.json`. It installs through a separate `ProjectZomboid64.site.json`, preserving prior overrides and unrelated JVM options. File verification is **not** confirmation that a game callback or mod ran. Check fresh in-game diagnostics; `luabuddy.exe doctor` and `luabuddy.exe diagnostics` can help distinguish file state from a live startup report.

Windows JDK 25 uses the active Windows code page for some paths. If necessary the installer uses an existing, verified short spelling for the same folder. On a volume without equivalent short names, use a representable Steam library/profile path (ASCII always works). LuaBuddy never changes system locale or 8.3 filename settings.

**Updates and removal:** close the game. From the newly extracted package use `.\luabuddy.exe update --package .`. Roll back with `.\luabuddy.exe rollback`, or uninstall with `.\luabuddy.exe uninstall` (add the same `--game-dir` when needed). Personal profiles and separately subscribed Workshop content stay in place. If a launcher or DLL is still locked, exit the game/other setup, run `.\luabuddy.exe recover`, then retry. No command kills a running game.

The Windows site override is a complete launcher configuration. If Steam changes its base JSON, run `verify`; on a base-change warning, uninstall LuaBuddy first. Compare any retained personal override with the updated base, then reinstall. A clean uninstall removes an unedited LuaBuddy-created override, exposing the new Steam defaults.

**Developers:** From a checkout with the pinned Rust toolchain on Windows x64 MSVC:

```powershell
cargo build --release --locked -p luabuddy -p luabuddy-host
.\target\release\luabuddy.exe package --runtime .\target\release\luabuddy_host.dll --profiles-dir profiles --out dist\LuaBuddy-windows-x86_64
.\dist\LuaBuddy-windows-x86_64\luabuddy.exe verify-package --package dist\LuaBuddy-windows-x86_64
```

The Windows Actions job makes a downloadable test bundle. It does not publish a Steam Workshop or GitHub release.
