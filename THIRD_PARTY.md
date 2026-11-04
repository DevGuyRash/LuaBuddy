# Third-party software

LuaBuddy downloads stable JAR releases from [ZombieBuddy's official repository](https://github.com/zed-0xff/ZombieBuddy/releases) without modifying them. Framework binaries are not embedded in this repository. Downloads are checked against the published asset digest and cached locally. ZombieBuddy 2.3.4 was verified against PZ 42.21 and the 42.22 preview. Its MIT license is retained in [lib/ZombieBuddy-LICENSE.txt](lib/ZombieBuddy-LICENSE.txt); consult the selected upstream release for its licensing.

Project Zomboid supplies the game classes, Kahlua compiler, mod discovery, native file mounting, and translation parser at build time and runtime. Game files are not distributed here. The optional mods listed in [the profile catalog](README.md#optional-bundled-profiles) stay in their original Workshop installations, including NeatUI Framework. LuaBuddy distributes only its compatibility profiles and hooks, not those mods' Lua or assets.

The multiplayer loading approach follows [Viewpoint Compass](https://github.com/Maeiro/ViewpointCompass#how-client-side-multiplayer-works). LuaBuddy uses ZombieBuddy's existing [patches_jar mechanism](https://github.com/zed-0xff/ZombieBuddy/blob/v2.3.4/doc/CommandLine.md#patches_jar) for its one Java patch.
