# Compatibility verification

The next public PZ update is [42.22 Pre-Release](https://projectzomboid.com/blog/news/2026/10/moving-on-up-42-22-pre-release/). The available preview was downloaded through anonymous SteamCMD to an isolated directory. The user's Stable installation, selected Steam branch, running game, and saves were not changed.

| Actual game binaries | ZombieBuddy | Verification |
| --- | --- | --- |
| 42.21.0 `4a0e9546ec`, installed Linux client | Official stable JAR 2.3.4 | Compile, native integration, and instrumented startup event passed. Earlier real multiplayer smoke tests passed. |
| 42.22.0 `e013dbb529`, official Linux server preview, app 380870 build 26221949 | Official stable JAR 2.3.4 | Compile, native integration, and instrumented startup event passed. Full preview client/server play testing remains unverified. |

These checks used the downloaded JAR with its published SHA-256 `eb79b9876332010733a8e0d7ce4fe0377846059a9e28859029e2a0fb449f6cf2`. The preview's `java/projectzomboid.jar` and `linux64` layout is supported alongside the current client's flat JAR and `natives` layout.

The native gate exercises mod discovery and version directories, dependency ordering, shared/client loading, translation reflection and fallback, actual Kahlua compilation and closure metadata, diagnostics, and the ZombieBuddy-patched `OnGameStart` entry point. It runs in a temporary cache with generated fixtures, without a game window or server connection. An unsuccessful gate prevents dependency promotion and installation.

Run it against another installed build with:

```sh
python3 luabuddy.py compatibility --game-dir /path/to/game --offline
```

The [official 42.22 notes](https://theindiestone.com/forums/topic/102705-4222-patch-notes/) include removed `TimSort` and `PZArrayList` classes, multiplayer changes, and additional anti-cheat checks. LuaBuddy does not reference those removed classes. Native test success does not prove multiplayer acceptance or correctness of every third-party mod.

[The Indie Stone's Java-modding statement](https://projectzomboid.com/blog/news/2026/10/a-note-on-java-mods-and-modding/) explains that Java loaders and internal game implementation APIs are unsupported and may change without warning. Automatic ZombieBuddy updates reduce manual dependency work; they do not guarantee future game compatibility. LuaBuddy verifies the selected game and candidate framework before replacing the installed pair.
