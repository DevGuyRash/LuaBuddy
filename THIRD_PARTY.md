# Third-party software

LuaBuddy uses [ZombieBuddy 2.3.4](https://github.com/zed-0xff/ZombieBuddy/releases/tag/v2.3.4) without modifying it. Its MIT license is in [lib/ZombieBuddy-LICENSE.txt](lib/ZombieBuddy-LICENSE.txt). The pinned framework JAR is in `lib/ZombieBuddy.jar` so builds and installations use the same version.

Project Zomboid supplies the game classes, Kahlua compiler, mod discovery, and translation parser at build time and runtime. Game files are not distributed here. AllTheInfo and Auto All stay in their original Workshop installations. LuaBuddy distributes only compatibility profiles for those mods.

The multiplayer loading approach follows [Viewpoint Compass](https://github.com/Maeiro/ViewpointCompass#how-client-side-multiplayer-works). LuaBuddy uses ZombieBuddy's existing [patches_jar mechanism](https://github.com/zed-0xff/ZombieBuddy/blob/v2.3.4/doc/CommandLine.md#patches_jar) for its one Java patch.
