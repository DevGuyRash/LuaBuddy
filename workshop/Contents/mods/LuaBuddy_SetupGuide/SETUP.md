# Install LuaBuddy on native Linux

This Workshop item contains documentation. Subscribing or enabling `LuaBuddy_SetupGuide` does not install the native runtime. You do not need to enable the guide to use LuaBuddy.

The current release targets Linux x86_64 with glibc 2.34 or newer and the native Linux Project Zomboid client. It was tested on game version 42.22.2. Windows, macOS, and Proton execution are not supported by this release.

## Install the native package

1. Quit Project Zomboid.
2. Open the [Linux release](https://github.com/DevGuyRash/LuaBuddy/releases/tag/v0.1.0-rc.1). Download `LuaBuddy-0.1.0-linux-x86_64.tar.gz` and `SHA256SUMS` into the same directory.
3. Open a terminal in that directory and run the following commands. Stop if either verification command fails.

```sh
sha256sum --check SHA256SUMS &&
mkdir -p LuaBuddy &&
tar -xzf LuaBuddy-0.1.0-linux-x86_64.tar.gz -C LuaBuddy &&
chmod +x LuaBuddy/luabuddy &&
LuaBuddy/luabuddy verify-package --package LuaBuddy &&
LuaBuddy/luabuddy install --package LuaBuddy
```

Steam discovery normally locates the game. If it fails, use the directory containing `projectzomboid.jar` and `ProjectZomboid64`. For a standard Steam layout, the explicit command is:

```sh
LuaBuddy/luabuddy install --package LuaBuddy --game-dir "$HOME/.local/share/Steam/steamapps/common/ProjectZomboid/projectzomboid"
```

Supply your actual game path if your Steam library is elsewhere. The installer probes the bundled JVM, copies an integrity-checked native generation, and writes a reversible `ProjectZomboid64.site.json` launcher override. It preserves the base launcher JSON and unrelated settings. A checksum checks downloaded bytes, not the publisher's identity. Use a release from the linked project you trust.

Launch the game normally through Steam. Open **Options > Mods > LuaBuddy > Manage mods**. A fresh installation selects no mods. You need neither ZombieBuddy nor a Rust toolchain, Python, or another Lua interpreter to use the prebuilt package.

## Choose a mod

Install the upstream mod and its actual dependencies. Search **Installed** in LuaBuddy's manager. Inspect its description, dependency information, and observed states before selecting an action.

- To load eligible client/shared Lua during play, use **This session > Enable late**, then **Load enabled**. Read the result for loaded, failed, reconnect, or restart instructions.
- To use the normal startup lifecycle, use **Next launch > Startup enable**, then restart the game.
- To reload, use **Reload a profile** and search the installed list or type an ID with suggestions. Reload requires an explicit cleanup contract. Otherwise reconnect.

Apply or discard pending Options edits first. Disabling a selection does not hot-unload running code. Late loading cannot supply missing server code, bypass server policy, or register every native asset. Bundled profiles are optional integrations, and their upstream mods are not included.

Use **Help & guides** for full in-game workflows. The [workflow reference](https://github.com/DevGuyRash/LuaBuddy/blob/main/docs/workflows.md) covers local mods, one-launch testing, downloads, and troubleshooting.

## Verify or diagnose

From the directory containing the extracted package, run:

```sh
LuaBuddy/luabuddy verify
LuaBuddy/luabuddy doctor
LuaBuddy/luabuddy diagnostics
```

Add `--game-dir /your/game/directory` when discovery needs help. Diagnostics accepts `--cache-dir /your/Zomboid/cache` when you use a custom game cache. Commands return JSON. Installation verification checks files. In-game diagnostics establishes whether profiles loaded.

## Update or remove

Workshop updates refresh this guide. They do not update an installed native library. Close the game, download and verify the replacement package, extract it to a new directory, and use that package's executable:

```sh
NewLuaBuddy/luabuddy update --package NewLuaBuddy
```

An update retains the previous native generation for rollback. From an extracted package, these commands roll back or uninstall:

```sh
LuaBuddy/luabuddy rollback
LuaBuddy/luabuddy uninstall
```

Add `--game-dir` when needed. Restart normally after installation, update, repair, rollback, or uninstall. These commands do not stop or change an already running JVM. Uninstall preserves user profiles, Workshop mods, and user-modified recovery files. Unsubscribing from this guide alone does not uninstall the native runtime.

Read the [installation and recovery reference](https://github.com/DevGuyRash/LuaBuddy/blob/main/docs/evolution/install.md) for repair and interrupted installation. Report reproducible problems in [GitHub issues](https://github.com/DevGuyRash/LuaBuddy/issues), with the game version, OS, mod IDs, and relevant diagnostic result. Remove private information before posting logs.

LuaBuddy source is available under the [MIT license](https://github.com/DevGuyRash/LuaBuddy/blob/main/LICENSE). No game files or upstream mods are included in this item.
