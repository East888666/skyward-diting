# Bundled Android AWK

`skyward-pack` embeds `awk-aarch64-linux-android`, `LICENSE` and `VERSION`
with Rust's `include_bytes!`. When `[recovery] awk = true`, it installs these
bytes in the recovery ramdisk. No AWK download, NDK invocation or separate
AWK build happens during normal Cargo compilation or recovery packing.
`--awk <bundle>` remains available as an explicit override.

This is a prebuilt One True Awk executable, not a Rust implementation.
It is static AArch64 Android ELF, built with NDK r29's API 30 compiler.
The source license is reproduced in `LICENSE` and also included in each
recovery that uses the tool. It is separate from Skyward's license.

Provenance:

- One True Awk version: `20250116`
- Upstream commit: `0eeec2359f4cce389ae4e6163959c1fda3dcb9ad`
- Source archive: <https://codeload.github.com/onetrueawk/awk/tar.gz/0eeec2359f4cce389ae4e6163959c1fda3dcb9ad>
- Source SHA-256: `c38a59d41f255594d4c0d1d0a7de5f80d2e19d3489e247b9a173ceda4f8648fe`
- Executable SHA-256: `2663b5f150ee14da50def2a83f5f6f8f9df03ba649861df3b392e3c26f1dc50a`

The executable was tested in MIX 2S recovery against HydrogenOS install
preflight expressions, numeric operations and failure exit codes. The packer
validates static ARM64 ELF before installation; its tests pin this checksum.

Only maintainers refreshing this asset need the optional build script and
its host dependencies (C compiler, make, bison, tar and Python 3). From the
repository root on Linux/WSL, with `ANDROID_NDK_HOME` pointing to NDK r29:

```sh
bash tools/build-awk.sh
cp target/awk/awk crates/skyward-pack/assets/awk/awk-aarch64-linux-android
cp target/awk/LICENSE target/awk/VERSION crates/skyward-pack/assets/awk/
sha256sum crates/skyward-pack/assets/awk/awk-aarch64-linux-android
cargo test --locked -p skyward-pack -p skyward-config
```

When refreshing, update provenance and the pinned test checksum, review the
license, and repeat device validation. Commit the executable, license,
version record and code together; do not rely on ignored `target/` files.
