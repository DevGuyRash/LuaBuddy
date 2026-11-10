# Native compatibility and platform evidence

The Rust adapter uses Project Zomboid's existing JVM, Kahlua runtime, UI, discovery, metadata, translations, and Steam context. Private game APIs remain explicit compatibility dependencies. Rust does not make those APIs permanently stable.

## Observed host

The implementation proofs use Linux x86_64, Project Zomboid `42.22.2 3d39148846 integrity=pending`, and bundled Azul Java `25.0.5+8-LTS`. The game JAR SHA256 observed by the native review is `3c4a7c5f149d784758c6fb5f8fa85f3a21d572e38bc081400e89035f8120d287`. This describes one tested host, not a fixed assumption about future installations.

The installer probes the selected bundled JVM process architecture. It does not infer that architecture from the host CPU. The adapter currently refuses unsupported native platforms. Portable compilation and fixture tests on another OS do not establish game execution there.

| Capability | Linux x86_64 | Windows | macOS |
| --- | --- | --- | --- |
| Ordinary native launcher integration | Installed `.site.json`, actual menu and Rust callback, offline relaunch, update, rollback, and uninstall passed through the copied real native executable. A later normal Steam launch mapped runtime `17ffaedb`, and fresh diagnostics plus `doctor` confirmed all 24 existing selections loaded with no failed, pending, or restart-required profiles. | Not verified. | Not verified. |
| Native callable, dispatch, classloader and patcher mechanisms | Actual bundled JVM proofs with verifier and JNI checking. | Not verified. | Not verified. |
| Native discovery, translations and diagnostic files | Actual game-class consumers. | Not verified. | Not verified. |
| Menu management and read-only initialized Workshop browsing | Isolated graphical client evidence in the implementation run. | Not verified. | Not verified. |
| New subscription/download account mutation | One authorized fresh item completed through actual Steam launches and the native manager on candidate `17ffaedb`. See [Workshop evidence](evolution/workshop.md). | Not verified. | Not verified. |
| Subscribe-only native activation | Unresolved. | Unverified. | Unverified. |
| Gameplay | All 24 bundled profiles passed individually with required dependencies in distinct native environments. The all-24 combination, runtime addition, repeated no-op, reconnect, native controls, and saved preferences passed in disposable ordinary-role loopback sessions. | Not verified. | Not verified. |
| Matched performance acceptance | The earlier software-rendered five-pair comparison remains inconclusive. The later NVIDIA attempt completed only four of ten required runs before the user ended testing. Its 7,107 verified presentation intervals establish no speedup or budget pass. Scoped lifecycle references collected across refresh, reload, and reconnect. Reconnect resident growth also occurred in a native-only control; incremental loader cost remains unestablished. See [performance evidence](evolution/performance.md). | Not verified. | Not verified. |

The later test-only platform correction produced runtime `1d44f599`, which passed its own 44-check native aggregate and two-path reproducibility. It is installed for the next normal Steam launch. The running game continued with `17ffaedb`, and its actual 24-profile startup remains scoped to that binary. No new graphical or performance journey is claimed for `1d44f599`. Portable CI for the current source head does not establish Windows or macOS gameplay support.

## Native boundaries

`layouts.json` records exact class names, descriptors, defining-loader requirements, and known bytecode fingerprints. Incompatible targets are rejected before hook installation. Native patch diagnostics distinguish registration, rejection, failure, installed receipts, disable, and uncertainty.

Rust-owned Kahlua callables reuse the existing verified no-op `LuaEventManager.call` host method. Initial class loading changes that method declaration to native, removes its original Code attribute, and binds Rust with `RegisterNatives`. This transformation adds no executable Java/Lua instructions or helper class. Exact original shape and system defining-loader checks protect both transformation and registration.

Lifecycle observation targets actual synchronous dispatch. The native dispatcher can skip events with no Lua closures before reaching `Event.trigger`. The manager therefore polls through its own native UI update callback. The adapter does not advertise arbitrary empty-event support.

The general patcher supports the capabilities documented in [the JVM adapter contract](evolution/jvm-patcher.md). Unsupported after, around, reference replacement, wide numeric argument changes, and result editing are rejected. The core registry's ability to model an operation is not a claim that the native adapter implements it.

## Workshop delivery and activation

The inspected game loader discovers installed Workshop mod roots and mounts common/version content. Those operations do not turn a newly delivered native library into a JVM agent. No demonstrated Workshop metadata declaration activates LuaBuddy's native runtime through subscription alone.

The smallest demonstrated additional activation mechanism is the reversible native installer, which owns a launcher site override containing `-agentpath`. That is an additional setup step. It does not meet a subscribe-only one-click acceptance target.

A clean LuaBuddy subscription, ordinary Steam launch, interface appearance, offline relaunch, update, and removal still need a complete activation-path proof before one-click setup can be claimed. [The Workshop capability record](evolution/workshop.md) distinguishes native activation from browsing and the successful delivery of one ordinary client mod.

The current Steam adapter attaches only to the game's already initialized context. It neither initializes/shuts down Steam nor takes over its callback pump. Exact native ABI declarations follow the observed UGC021 interface. Native download-result observation still requires installed-state and path verification before success.

## Preserve game authority

LuaBuddy does not supply missing server code, change multiplayer server policy, bypass checksums, or bypass anti-cheat. A selected mod can still require server installation or normal startup. Use native loading for content whose lifecycle cannot be safely completed during play.

Upstream Workshop updates are read in place. Native metadata decides version availability. Compatibility rules and optional integration code remain in the profile catalog, and no bundled mod is selected by default.

The native-only purge package `1207d5d6` passed the exact-package 44-check native aggregate and current configuration/installer tests. Its source and package no longer contain the old migration paths or catalog. Actual normal-play startup evidence remains scoped to `1d44f599`; the newer native-only package is installed for the next normal launch.
