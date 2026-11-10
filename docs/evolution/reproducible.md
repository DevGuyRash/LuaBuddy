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

The November 9, 2026 run used Rust 1.99.1 on Linux x86_64. Two fresh builds through the direct pinned toolchain produced identical runtime and CLI bytes, all 32 package files, and the normalized archive. The resulting runtime requires GLIBC 2.34. Other Linux C-library baselines and other operating systems have not been established by this check.

| Artifact | SHA256 |
| --- | --- |
| Native runtime | `832d2d869d648a5bd304e03831df6eca4ce78543301efceabab962141ff0178c` |
| Installer CLI | `20f550a70c23b7976eeaf269bab2fd644097a6808a9201ec0d1dc7b6306d9e66` |
| Normalized archive | `f28101dc1df66ca31d8558c25ea75b4d26d427e113323b673302ec1a007692a6` |

An earlier exploratory run passed through the host's cached Cargo dispatcher. Its equal bytes do not establish independent code generation. That run was discarded as acceptance evidence before the check using the direct toolchain.

This check establishes reproducibility within the recorded build environment. It does not establish runtime acceptance, Workshop account operations, performance, or compatibility with another compiler or operating system. Those checks use the separate verification workflows.
