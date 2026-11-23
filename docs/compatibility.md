# Native compatibility and platform evidence

The Rust adapter uses Project Zomboid's existing JVM, Kahlua runtime, UI, discovery, metadata, translations, and Steam context. Private game APIs remain explicit compatibility dependencies. The native loader is a third-party integration, not part of the game's supported Lua modding interface. [The Indie Stone’s modding support statement](https://projectzomboid.com/blog/news/2026/10/a-note-on-java-mods-and-modding/) explains why game updates can break integrations that depend on internal code.

The [Windows x64 setup guide](windows-cli.md) distinguishes the packaged EXE/DLL and native fixture tests from gameplay. The Indie Stone [documents the site override](https://theindiestone.com/forums/topic/16470-released-build-3230/) read by the normal Windows launcher. This is not evidence for the BAT alternate launch, live game layouts, Windows UI or the Steam SDK ABI. Windows Workshop browsing and downloads remain disabled in LuaBuddy; the Linux native Steam library layout is not used as a Windows fallback.

## Observed host

The implementation proofs use Linux x86_64, Project Zomboid `42.22.2 3d39148846 integrity=pending`, and bundled Azul Java `25.0.5+8-LTS`. The game JAR SHA256 observed by the native review is `3c4a7c5f149d784758c6fb5f8fa85f3a21d572e38bc081400e89035f8120d287`. This describes one tested host, not a fixed assumption about future installations.

The installer probes the selected bundled JVM process architecture. It does not infer that architecture from the host CPU. The adapter currently refuses unsupported native platforms. Portable compilation and fixture tests on another OS do not establish game execution there.

| Capability | Linux x86_64 | Windows | macOS |
| --- | --- | --- | --- |
| Ordinary native launcher integration | Installed `.site.json`, actual menu and Rust callback, offline relaunch, update, rollback, and uninstall passed through the copied real native executable. A later normal Steam launch mapped runtime `17ffaedb`, and fresh diagnostics plus `doctor` confirmed all 24 existing selections loaded with no failed, pending, or restart-required profiles. | Experimental x64 CLI, supported site override mechanism and hosted installer/stock-JVM checks. Ordinary licensed game launch not verified. | Not verified. |
| Native callable, dispatch, classloader and patcher mechanisms | Actual bundled JVM proofs with verifier and JNI checking. | Stock x64 JDK 25 checked DLL startup and rejection of absent or incompatible game classes; the game-specific mechanisms have not been verified on Windows. | Not verified. |
| Native discovery, translations and diagnostic files | Actual game-class consumers. | Not verified. | Not verified. |
| Menu management and read-only initialized Workshop browsing | Isolated graphical client evidence in the implementation run. | In-game UI not verified; in-manager Workshop access is disabled. | Not verified. |
| New subscription/download account mutation | One authorized fresh item completed through actual Steam launches and the native manager on candidate `17ffaedb`. See [Workshop evidence](evolution/workshop.md). | Disabled in LuaBuddy; use the Steam client. | Not verified. |
| Subscribe-only native activation | Unresolved. | Unverified. | Unverified. |
| Gameplay | The 24 profiles in the original test catalog passed individually with required dependencies in distinct native environments. The all-24 combination, runtime addition, repeated no-op, reconnect, native controls, and saved preferences passed in disposable ordinary-role loopback sessions. | Not verified. | Not verified. |
| Matched performance acceptance | The earlier software-rendered five-pair comparison remains inconclusive. The later NVIDIA attempt completed only four of ten required runs before the user ended testing. Its 7,107 verified presentation intervals establish no speedup or budget pass. Scoped lifecycle references collected across refresh, reload, and reconnect. Reconnect resident growth also occurred in a native-only control; incremental loader cost remains unestablished. See [performance evidence](evolution/performance.md). | Not verified. | Not verified. |

These Linux results describe the named artifacts and the 24-profile test set, not every combination in the source catalog. For the published Linux prerelease, see the [v0.1.0-rc.3 release and provenance](https://github.com/DevGuyRash/LuaBuddy/releases/tag/v0.1.0-rc.3) and its [headless event measurement](evolution/event-dispatch.md). The three [post-release profile templates](../README.md#select-optional-integrations) were not in that package. Portable CI on another OS is not evidence of gameplay there.

## Native boundaries

`layouts.json` records exact class names, descriptors, defining-loader requirements, and known bytecode fingerprints. Incompatible targets are rejected before hook installation. Native patch diagnostics distinguish registration, rejection, failure, installed receipts, disable, and uncertainty.

Rust-owned Kahlua callables reuse the existing verified no-op `LuaEventManager.call` host method. Initial class loading changes that method declaration to native, removes its original Code attribute, and binds Rust with `RegisterNatives`. This transformation adds no executable Java/Lua instructions or helper class. Exact original shape and system defining-loader checks protect both transformation and registration.

Lifecycle observation targets actual synchronous dispatch. The native dispatcher can skip events with no Lua closures before reaching `Event.trigger`. The manager therefore polls through its own native UI update callback. The adapter does not advertise arbitrary empty-event support.

The general patcher supports the capabilities documented in [the JVM adapter contract](evolution/jvm-patcher.md). Unsupported after, around, reference replacement, wide numeric argument changes, and result editing are rejected. The core registry's ability to model an operation is not a claim that the native adapter implements it.

## Workshop delivery and activation

The inspected game loader discovers installed Workshop mod roots and mounts common/version content. Those operations do not turn a newly delivered native library into a JVM agent. No demonstrated Workshop metadata declaration activates LuaBuddy's native runtime through subscription alone.

The smallest demonstrated additional activation mechanism is the reversible native installer, which owns a launcher site override containing `-agentpath`. That is an additional setup step. It does not meet a subscribe-only one-click acceptance target.

The accepted player workflow adds a graphical front end to that installer and delivers its complete package through Workshop as `LuaBuddy-Setup-linux-x86_64.tar.gz`. Players extract the archive and open `luabuddy`, which uses an existing Zenity or KDialog utility. The native runtime, package verifier, installation ownership, and profile-preservation rules remain in Rust. The graphical flow requires a compatible Linux desktop with one of those utilities; portable compilation does not establish that every desktop provides it. [The setup guide](../workshop/Contents/mods/LuaBuddy_SetupGuide/SETUP.md) describes the player path without terminal commands or a separate download.

Steam refreshing the archive does not activate a new installed native generation. Players run the newly extracted setup package to apply an update. Unsubscribing removes Workshop delivery, while the installer's removal action owns native uninstallation. These boundaries remain distinct from the runtime and Workshop evidence recorded here.

A clean LuaBuddy subscription, ordinary Steam launch, interface appearance, offline relaunch, update, and removal still need a complete activation-path proof before one-click setup can be claimed. [The Workshop capability record](evolution/workshop.md) distinguishes native activation from browsing and the successful delivery of one ordinary client mod.

The current Steam adapter attaches only to the game's already initialized context. It neither initializes/shuts down Steam nor takes over its callback pump. Exact native ABI declarations follow the observed UGC021 interface. Native download-result observation still requires installed-state and path verification before success.

## Preserve game authority

LuaBuddy does not supply missing server code, change multiplayer server policy, bypass checksums, or bypass anti-cheat. A selected mod can still require server installation or normal startup. Use native loading for content whose lifecycle cannot be safely completed during play.

Upstream Workshop updates are read in place. Native metadata decides version availability. Compatibility rules and optional integration code remain in the profile catalog, and no bundled mod is selected by default.

See the [migration verification report](evolution/verification-report.md) for earlier runtime hashes and the checks each one passed. A historical installation record is not the state of a player's current game.
