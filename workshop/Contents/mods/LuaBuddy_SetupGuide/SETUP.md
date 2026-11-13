# Install LuaBuddy through graphical setup

Steam supplies the native Linux package and this guide. One-time graphical setup activates LuaBuddy for normal Steam launches. Subscribing or enabling `LuaBuddy_SetupGuide` alone does not activate the runtime. You do not need to enable the guide to use LuaBuddy.

The package targets Linux x86_64 with glibc 2.34 or newer and the native Linux Project Zomboid client. The runtime was tested on game version 42.22.2. Graphical setup uses an existing Zenity or KDialog utility. A Linux desktop without either utility cannot use this graphical flow. Windows, macOS, and Proton execution are not supported by this release.

## Find the setup archive

1. Subscribe to [LuaBuddy on Steam Workshop](https://steamcommunity.com/sharedfiles/filedetails/?id=3838682402) and let Steam finish downloading it.
2. Quit Project Zomboid.
3. In your Steam Library, right-click **Project Zomboid** and choose **Manage > Browse local files**.
4. In the file manager, move up through the parent folders until you reach `steamapps`.
5. Open `workshop/content/108600/3838682402/mods/LuaBuddy_SetupGuide/common/setup`.
6. Open `LuaBuddy-Setup-linux-x86_64.tar.gz` with your desktop archive manager and extract it to a folder you can keep.

The archive contains a folder named `LuaBuddy-` followed by a package identifier. Each package has its own identifier, so a new update can be extracted separately from an earlier package. Use the archive manager's normal extraction action to preserve the executable permissions.

## Activate LuaBuddy

1. Open the extracted `LuaBuddy-...` folder and double-click `luabuddy`. If your file manager asks whether to run the executable, choose **Run**.
2. Check the game folder shown in setup. If discovery needs help, choose the folder containing `projectzomboid.jar` and `ProjectZomboid64`, or its enclosing `ProjectZomboid` folder.
3. Choose **Install / update** and read the result.
4. Close setup and launch Project Zomboid normally through Steam.
5. Open **Options > Mods > LuaBuddy > Manage mods**.

Setup verifies the adjacent package before installation. Keep its executable, native library, manifests, profiles, and licenses together. Do not move `luabuddy` out of the extracted folder. A failed package check is an error to resolve before installation, not a successful activation.

The installer uses the game's bundled JVM, copies an integrity-checked native generation, and writes a reversible `ProjectZomboid64.site.json` launcher override. It preserves the base launcher JSON and unrelated settings. A fresh installation selects no mods. Existing profiles and settings survive an update. Setup does not stop or restart a running game, and native runtime changes apply after the next normal Steam launch.

The prebuilt package needs no ZombieBuddy, Rust toolchain, Python, or additional Lua interpreter. The [GitHub prerelease](https://github.com/DevGuyRash/LuaBuddy/releases/tag/v0.1.0-rc.2) is an optional download source if you do not use Workshop delivery.

## Choose a mod

Install the upstream mod and its actual dependencies. Search **Installed** in LuaBuddy's manager. Inspect its description, dependency information, and observed states before selecting an action.

- To load eligible client/shared Lua during play, use **This session > Enable late**, then **Load enabled**. Read the result for loaded, failed, reconnect, or restart instructions.
- To use the normal startup lifecycle, use **Next launch > Startup enable**, then restart the game.
- To reload, use **Reload a profile** and search the installed list or type an ID with suggestions. Reload requires an explicit cleanup contract. Otherwise reconnect.

Apply or discard pending Options edits first. Disabling a selection does not hot-unload running code. Late loading cannot supply missing server code, bypass server policy, or register every native asset. Bundled profiles are optional integrations, and their upstream mods are not included.

Use **Help & guides** for full in-game workflows. The [workflow reference](https://github.com/DevGuyRash/LuaBuddy/blob/main/docs/workflows.md) covers local mods, one-launch testing, downloads, and troubleshooting.

## Update the native runtime

1. Let Steam download the updated Workshop item, then quit Project Zomboid.
2. Open the Workshop setup archive again and extract the newly supplied package to its own folder.
3. Open that new folder, double-click `luabuddy`, and choose **Install / update**.
4. Read the result and relaunch the game normally through Steam.

Steam updates the delivered archive. It does not replace the native generation already installed in the game. Opening an older extracted package still selects that older package. Keep the current package for maintenance and use the newly delivered package when applying an update.

## Verify, repair, recover, or roll back

Close the game, then open setup from an intact extracted package. Choose **Maintenance**, select the action you need, and read its result.

- **Verify installed files** checks installed file integrity and launcher ownership. It does not establish that a mod loaded in the current game session.
- **Repair** restores package files while preserving modified owned files in recovery storage.
- **Recover interrupted setup** handles an interrupted installation transaction. It preserves an independently changed file and reports when manual attention is needed.
- **Roll back** restores the prior native generation when one is available. Review the confirmation before proceeding. Existing profile files remain intact.

For actual mod loading state, inspect the in-game manager's runtime state and action results. Read the [installation and recovery reference](https://github.com/DevGuyRash/LuaBuddy/blob/main/docs/evolution/install.md) for the command-line interface and detailed ownership rules.

## Remove LuaBuddy

1. Quit Project Zomboid and open setup from an intact extracted package.
2. Choose **Maintenance**, then **Remove LuaBuddy**. Review the confirmation and confirm removal.
3. Read the result, then launch through Steam normally if you want to play without LuaBuddy.
4. Unsubscribe from the Workshop item if you also want Steam to stop delivering its files.

Removal preserves user profiles, Workshop mods, and user-modified recovery files. Unsubscribing alone does not uninstall a native runtime already installed in the game.

Report reproducible problems in [GitHub issues](https://github.com/DevGuyRash/LuaBuddy/issues), with the game version, OS, mod IDs, and relevant diagnostic result. Remove private information before posting logs. LuaBuddy source is available under the [MIT license](https://github.com/DevGuyRash/LuaBuddy/blob/main/LICENSE). No game files or upstream mods are included in this item.
