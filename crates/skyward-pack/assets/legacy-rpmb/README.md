# Legacy RPMB adapter

Opt-in ARM64 Android shared library for the legacy SDM660 qseecomd RPMB ABI.
The adjacent C/header files are its complete source. The verified binary is
bundled so the ordinary skyward-pack build needs no separate C compilation.

Only an explicitly preloaded process is affected. The original ioctl runs
first. Conversion is attempted only on EINVAL/ENOTTY, for the legacy request
on the actual /dev/mmcblk0rpmb character device. MMC buffers, ordering and
reliable-write flags are preserved. This does not bypass key authentication.

SHA-256: `2685a381cb1878d349cc96f35853d1b45ee5aad1948b5ae12f6705663640a531`

Rebuild with an Android NDK ARM64 compiler (API 30):

```sh
"$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/linux-x86_64/bin/aarch64-linux-android30-clang" \
  -shared -fPIC -O2 -Wall -Wextra -Werror rpmb-compat.c -o liblegacy-rpmb-compat.so
```

The original diagnostic names in the source are retained for binary provenance.
Validated on jason with crDroid 10.7 and the patched 4.19 legacy ICE kernel.
