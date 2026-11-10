# Install and activate Rust extensions

LuaBuddy's native extension interface is ABI v1. An extension is a compiled Rust library and a fixed-schema manifest. Native code runs with the game's full operating-system privileges. Privilege declarations constrain patch registration. They are not a native-code sandbox.

Review the artifact and its native dependencies before approving it. Obtain its expected artifact SHA256, exact manifest-file SHA256, and provenance independently. Installation checks those values against the manifest without loading the library. Workshop delivery does not create an approval record.

Use the same profile directory that the game receives through `luabuddy.config`. The extension store is its `extensions` subdirectory. An explicit `luabuddy.extensions` property can select another store for a developer-controlled runtime.

```sh
luabuddy extension-install --profiles-dir config --artifact /path/to/libexample.so --manifest /path/to/manifest.json --sha256 EXPECTED_SHA256 --manifest-sha256 EXPECTED_MANIFEST_SHA256 --provenance REVIEWED_SOURCE --allow observe_methods --trust-native-code
luabuddy extensions --profiles-dir config
```

Installation creates an immutable generation under `extensions/<id>/<manifest-sha256>/` and a separate schema-v1 `approval.json` binding both that manifest and the existing artifact consent. It leaves execution disabled. Repeating installation of the exact approved bundle preserves its enabled setting. A changed manifest creates a separate immutable bundle even when the Rust binary is unchanged. Installation preserves the prior approval in `approval-history` and selects the new bundle with execution disabled. Enabling it requires the new exact manifest digest. Listing shows retained generations as configuration history, without treating them as active code.

Enable the exact reviewed artifact separately. Supply the complete comma-separated privilege set from its reviewed manifest. The supported privilege names are `observe_methods`, `replace_methods`, `modify_arguments`, and `modify_results`.

```sh
luabuddy extension-enable --profiles-dir config --extension-id example --sha256 EXPECTED_SHA256 --manifest-sha256 EXPECTED_MANIFEST_SHA256 --provenance REVIEWED_SOURCE --allow observe_methods --trust-native-code
```

The saved enable takes effect in the next JVM process. At normal LuaBuddy initialization, the runtime checks the approval, artifact bytes, ABI, provenance, platform, privileges, exact target signatures, and bytecode fingerprints before activation. The native loader validates the exported descriptor. Approved libraries must obey ABI v1 and never unwind across it. Library initializers may execute as soon as a trusted library is loaded.

Target classes must reach their normal prepared lifecycle. LuaBuddy checks target signatures, fingerprints, and supported hook kinds before loading an approved library. It reports unavailable or unprepared targets instead of initializing classes just to inspect them. An explicit in-game retry can retry the original approved generation after that lifecycle has advanced. It does not reread new approval records or activate changed approvals in the running process.

`LuaBuddy.patches` publishes actual capabilities, extension approval identities, installed or rejected patch states, target descriptors, and callback failures. Its separate `runtimeHooks` inventory reports the actual callable bridge, lifecycle breakpoints, and Workshop completion observer. These built-in adapter mechanisms are not represented as fictitious reusable-registry receipts. An enabled approval alone is not evidence that its patches installed successfully.

Disable execution for the next process through the saved approval.

```sh
luabuddy extension-disable --profiles-dir config --extension-id example
```

Restart the game to apply this saved disable. LuaBuddy keeps trusted native libraries mapped for the process lifetime because arbitrary library workers and side effects cannot be safely unloaded. This command does not claim to stop code already running in the current JVM. Profiles, approval history, and native extension artifacts remain configuration across ordinary runtime updates and uninstall. An interrupted approval stage is preserved in `approval-history` before a later mutation retries. A changed immutable artifact is preserved and refused rather than overwritten.

The reusable Rust API and ABI are documented in [patcher.md](patcher.md). Native JVM capabilities and their explicit unsupported operations are documented in [jvm-patcher.md](jvm-patcher.md). The two independent extension examples are under `examples/extensions/`.
