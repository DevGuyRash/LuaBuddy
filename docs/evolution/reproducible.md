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

The final November 10, 2026 build used Rust 1.99.1 on Linux x86_64 after the Steam fixes, installer rollback correction, and test-only platform configuration fix. Two fresh builds through the direct pinned toolchain produced identical runtime and CLI bytes, all 32 package files, and the normalized archive. The product-input digest is `e65ade59864f4a86745ea458a98808f29c0174592da09a5318335a9e3b3af220`. The resulting runtime requires GLIBC 2.34. Other Linux C-library baselines and other operating systems have not been established by this check.

| Artifact | SHA256 |
| --- | --- |
| Native runtime | `1d44f599e042c3b8ca02574cb4143a16b09ecc9fdfd909ac56e76b516c243697` |
| Installer CLI | `e4aa550b7d5d1b301c4cdccdb2fc6d6979bd4f398f9539007b98b72b61d6c0da` |
| Normalized archive | `365fcade214fbcb11d6c0365e3d30910c8d35c356ce76d0765d7afc89da0fae4` |
| Package manifest | `42fa1fbb45a932db48e533ac1bdd5916621db00e039ee6bb22370032368bb8bf` |

The earlier November 10 Steam-tested runtime `17ffaedb4f2d0a59002a536e408f53628d3a6d9d2b47f6b18a8b07951205c64a` used the same installer CLI, product-input digest `de345a31b760dca33acef91ae1c8e715ee0a31ea2926aecfcf68557294ad5883`, and archive `3d35be1aa2920e589d78d8075c78d4493cfe0e6afa6bf75e016bf349962ed4f1`. Portable CI then found an unused mock variant on Windows and macOS. Its variant and match arm now use the same Linux x86_64 test condition as their constructors. No production algorithm changed, but recompilation produced the new runtime identity above. The current package passed the 44-check native aggregate. Actual Steam delivery, normal-game startup, and GPU observations remain evidence for `17ffaedb`, not this new binary. The installed update selects `1d44f599` on the next normal launch while the existing game continues with `17ffaedb`.

The preceding November 9 candidate `832d2d86` has its own immutable reproducibility and gameplay evidence. Its performance and integration results remain labeled with that original identity; this later build does not retroactively replace those bytes.

An earlier exploratory run passed through the host's cached Cargo dispatcher. Its equal bytes do not establish independent code generation. That run was discarded as acceptance evidence before the check using the direct toolchain.

This check establishes reproducibility within the recorded build environment. It does not establish runtime acceptance, Workshop account operations, performance, or compatibility with another compiler or operating system. Those checks use the separate verification workflows.
