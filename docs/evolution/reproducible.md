# Reproduce the Linux release

The maintained Rust check builds LuaBuddy from two absolute source paths and two empty target directories. It compares the complete runtime and CLI binaries, every package file, and normalized release archives. It does not launch the game or redistribute game files.

Use the pinned Rust toolchain, cached locked dependencies, GNU tar, and gzip. Keep evidence outside the source checkout.

```sh
"$(rustc --print sysroot)/bin/cargo" run --manifest-path proofs/reproducible/Cargo.toml --locked --offline -- --source . --out dist/reproducibility --evidence /path/to/external/evidence --epoch 1794184183
```

The check verifies that the selected compiler matches `rust-toolchain.toml`, then invokes the toolchain's native Cargo and rustc executables directly. It disables compiler wrappers and incremental compilation, uses fresh target directories, and requires verbose logs containing actual production code generation and linking commands. A cached Cargo dispatcher on `PATH` is not used for either production build.

Actual rustc dependency files identify the compiled source graph. The input record also includes Cargo declarations, the lockfile, toolchain selection, and the packaged profile catalog. This includes the embedded patcher verification mode and excludes the standalone runtime driver that the production library does not compile. Changes to any selected input during verification invalidate acceptance.

Both builds map checkout paths, Cargo registry paths, and generated target paths to stable logical prefixes. They use the same `SOURCE_DATE_EPOCH`, UTC timezone, locale, declared release profile, and offline dependencies. The archive step sorts entries, normalizes ownership, modes, and timestamps, and disables gzip filename and timestamp metadata.

Successful output includes a native package directory, `LuaBuddy-linux-x86_64.tar.gz`, `SHA256SUMS`, and `build-provenance.json`. The provenance records the exact tool binaries, input digest and filenames, native artifact digests, archive digest, and all package file digests. The package's own integrity manifest remains independently verifiable through `luabuddy verify-package`.

## Observed scope

The native-only purge used Rust 1.99.1 on Linux x86_64. Two fresh direct-toolchain builds produced identical runtime and CLI bytes, all 31 package files, and normalized archives. The product-input digest is `15b984c60e29d5de9a34f391bbc0f238be6471819f1e4af2a1ae33af6f432ee3`. The package contains direct native profile templates and no executable Lua-hook migration catalog.

| Artifact | SHA256 |
| --- | --- |
| Native runtime | `1207d5d6498dcf799c4b8f4a331be606c6f1338d463d221a6b40154691ad5764` |
| Installer CLI | `cdc7896e6b6fd3fc50b70ea4331a0cbde868b5113e71938230946fa17b9afcf7` |
| Normalized archive | `843c92d254c948060b028fd04dd1f8d18ff8dfd3fe40cca802251b894395f197` |
| Package manifest | `c1dd0207836e0fa2520eb2ed81fe3819906a588d32edb2491baa9f05c2616d32` |

The native-only package passed the 44-check offline native aggregate, with four explicitly separate workflows not run by that command. Its actual CLI also completed a disposable install, update, rollback, repair, verification, uninstall, and repeated uninstall sequence while preserving current profiles and unrelated JVM settings.

Earlier `17ffaedb` and `1d44f599` gameplay, Steam, and GPU observations remain tied to those artifacts. The current normal session's 24-profile startup used `1d44f599`; the newly installed native-only build applies on the next normal launch. No graphical or performance result is inferred from this rebuild.

The preceding `832d2d86` software-rendered comparison and integration matrix retain their own evidence scope. An earlier exploratory comparison through a cached Cargo dispatcher was discarded before the direct-toolchain procedure. This procedure proves reproducibility in its recorded build environment; other compilers, operating systems, and runtime acceptance require their separate checks.
