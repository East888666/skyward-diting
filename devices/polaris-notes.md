# MIX 2S / polaris：解密适配记录

## LOS 18.1 锁屏密码解锁（2026-11-25，实机已验证）

本次自动挂载已选择 `inlinecrypt=true`，公共密钥和 DE0 均通过硬件 AES-GCM 认证。原失败发生在 CE0 的 `stretching=none`：原程序只接受旧版 `nopassword`，且旧 FBE 流程不能返回“等待密码”的用户状态，输入密码的 UI/CLI 又固定使用 AIDL KeyMint。

本机 LOS 18.1 的 protector 为 v3、secdiscardable 绑定，采用 Android 11 文件式 keystore 的 `user_0/1000_USRPKEY_synthetic_password_*` 容器；CE 密钥是无 keymaster blob 的软件封装。修复读取此容器（严格校验版本、类型、标志和长度），支持 `stretching=none` 按原始派生 secret 解包；先开放 DE 并将有锁用户交给原有密码/图案界面。提交凭据后只进行一次 Gatekeeper 验证，将返回的原始认证令牌交给 Keymaster 3 打开 SP，再认证解包 CE 密钥、校验用户目录并显示内部存储。没有跳过锁屏验证、重新注册密码、改写磁盘密钥或格式化。

已有 keystore2 数据库的设备仍优先使用该数据库；只在数据库不存在时读取旧文件式 keystore，不掩盖现有数据库的读取错误。现代设备保持原来的 AIDL KeyMint 解锁路径。旧路径仍不支持 Weaver、v1/v2 SP blob 或关联用户密码；不能将本次结果泛化到这些布局。

**实机结果**：通过 `fastboot boot` 临时启动完整候选后，用户在手机输入当前凭据，确认“解锁成功，文件可见”；日志确认待解锁用户列表从 `[0]` 变成 `[]`，`/data` 和 `/sdcard` 按 `inlinecrypt` 正常可写挂载。本轮未正式刷写 recovery。原 AviumUI 和 Smartisan 单独验证镜像继续保留；当前同一 header 镜像对 LOS 18.1 的自动挂载和凭据解锁已验证，但更换 ROM 后仍需分别回归，不能宣称所有 Android 11/16 均兼容。

本次无需新增或更改 TOML 参数，沿用当前 `devices/polaris.toml` 和已修复 4.19 底包。**必须重新编译主程序并重新打包，仅复制 TOML 不能带上这次修复。** 本轮源码文件：

- `crates/skyward-crypto/src/data.rs`：旧 FBE 的 DE/CE 分阶段开放、凭据解锁及目录验证。
- `crates/skyward-crypto/src/keystore.rs`：旧 `none` 软件密钥存储及认证检查。
- `crates/skyward-crypto/src/keystore1.rs`（新增）、`lib.rs`：只读文件式 keystore 解析。
- `crates/skyward-crypto/src/keymaster3.rs`、`token.rs`：传递 Gatekeeper 认证令牌。
- `crates/skyward-crypto/src/user.rs`：复用 v3 SP 的凭据派生与双层认证解包。
- `crates/skyward/src/decrypt.rs`、`main.rs`：UI 和 CLI 统一按配置选择旧/新密钥服务。

候选镜像：`target/skyward-polaris-419-los18-password.img`，46,469,120 字节（约 44.3 MiB），SHA256：`6419cfce275272b041e7b512b6925005969a05c5c02f3700cabb66e9ea2cc11d`。打包命令见 `target/diagnostics/polaris-los18-password-20261125/pack.sh`，沿用下方底包和字体/NTFS 参数，只更换输出路径。

验证：138 项测试通过，覆盖正确/错误 secret、损坏或不支持的 keystore 容器、硬件拒绝及令牌字节序；Android arm64 精简 release 构建与含 Wi-Fi 默认功能检查通过。打包内核与既有底包一致，主程序、配置、system-init 与输入一致，镜像小于 64 MiB。日志和构建记录在 `target/diagnostics/polaris-los18-password-20261125/`；输入日志目录的 1970 日期来自设备时钟，实际本次诊断日期为 2026-11-25。

以下为前序修复的历史记录，参数和验证状态以各段日期及本节为准。


## 自动兼容候选（2026-11-25，待实机验证）

当前 polaris.toml 增加 recovery.legacy_fbe_auto = true。默认值为 false，仅本机型启用；解析器拒绝 A/B 或未启用 legacy FBE 的配置开启该功能。其他设备的默认解锁路径不变。

相同的 ice:aes-256-cts:v1 标记不能区分旧 PFK 和软件 AES 内容路径，因此不按 ROM 名称或 PRIVATE 标记硬选参数。先按配置只读挂载（ext4 仍会正常回放已提交日志），认证解包公共密钥，再读取 DE0 密钥目录并通过 Keymaster 的 AES-GCM 认证和 64 字节长度检查。只有 PRIVATE/ICE v1 的内容解析失败才允许正常卸载并切换一次 inlinecrypt，重新只读验证；遇到 I/O、密钥服务、挂载或卸载错误立即停止。成功选定后沿用该选项解锁全部用户，并进入正常可写挂载。整个过程不改写磁盘密钥。

已经被其他流程挂载的 data 不自动切换参数，避免影响 UI、MTP 或安装器。格式化后的普通文件系统仍走原有 plain-data 路径。v2 文件系统维持原 remount 路径，不能用 PRIVATE v1 的替代探测。

版本限制仍然存在：本候选保留已验证 AviumUI 的 Android 16.0.0 / 2026-04-01 镜像头及匹配属性。没有在运行时伪造另一组版本，也没有修改 Keymaster 的版本绑定。Smartisan 的旧密钥将走现有、仅在内存中的 upgrade 路径（如 HAL 要求），该组合尚未实机确认。因此当前是自动挂载兼容候选，不能宣称一份镜像已同时验证两个 ROM；之前分别验证通过的旧镜像保留不覆盖。

候选镜像 target/skyward-polaris-419-auto-fbe.img，仍使用已验证的 target/stock/polaris/recovery-419-legacyice-base.img。本轮没有改动内核、动态分区、其他机型配置或构建工作流，也没有刷写设备。后续在两套 ROM 中分别验证内部文件、触摸及重启后解锁，才能确认单镜像兼容性。

验证：132 项测试通过，Android arm64 精简版 release 构建、含 Wi-Fi 的默认功能 cargo check 和 release 打包器构建通过，git diff --check 通过。镜像 46,465,024 字节，SHA256 为 c92bb5e482bc07b7f5309fb32e616fd25bce09d16aa4f7d6eaf5ad6bc8affc90。镜像内配置、主程序和 system-init 与当前输入一致，内核与已验证底包一致。当前电脑未检测到手机，尚未临时启动或刷写本候选。

本轮文件：crates/skyward-config/src/lib.rs、crates/skyward-crypto/src/lib.rs、crates/skyward-crypto/src/data.rs、新增 crates/skyward-crypto/src/legacy_mount.rs、crates/skyward/src/decrypt.rs、devices/polaris.toml、devices/polaris-notes.md。此前 AviumUI 单配置修复的记录见下方，按当时状态保留。


## AviumUI 16.2.1 / Android 16（2026-11-25）

用户确认系统能正常启动、读取内部存储且没有锁屏密码。本次 recovery 的具体错误为 DE0 key directory 的 version 为空。公共密钥已通过 Keymaster 3 解包并安装，但 DE/CE 的 1 字节 version 被读取为异常字节（如 0x0d / 0xff）。这是文件内容未正确解密的表现，不能跳过版本检查或改写密钥文件。

已核对 AviumUI 的 vendor/etc/fstab.qcom：data 使用 ext4，未指定 inlinecrypt。此前 Polaris 配置为 Smartisan 的 4.9 PFK 数据添加了 inlinecrypt；对本次 AviumUI 数据使用它会使 DE/CE 密钥文件内容不可读。

对照结果：
- 既有已验证内核（LEGACY_PFK=y）+ inlinecrypt：DE0/version 异常，失败。
- 试验内核（LEGACY_PFK 关闭）+ inlinecrypt：DE0/version 仍异常，失败。
- 同两种内核分别去掉 inlinecrypt 后：正常 recovery data 流程成功，DE0/version 和 CE0/current/version 都是 0x31（1），所有用户文件可读。
- 因而本轮最终保留原已验证内核与工具；不需要新增内核补丁或禁用 LEGACY_PFK。试验性修改已撤回，保留诊断产物用于追溯。

当前 polaris.toml 的 data options 已移除 inlinecrypt，并匹配本机 AviumUI ZIP 的 boot header/property：Android 16.0.0 / 2026-04-01，header 为 0x200001a4。使用既有 target/stock/polaris/recovery-419-legacyice-base.img 作为 --stock，不需要更换内核。header 匹配与挂载修复同时存在，未单独验证旧 header 的兼容性。

有效镜像 target/skyward-polaris-419-avium-fixed.img。最终镜像已通过 fastboot boot 再次验证，无 RAM 修改，正常 recovery data 成功，DE/CE version 均为 1，RAM 配置与源码一致；用户已确认界面可看到原有文件，触摸正常。大小 46,460,928 字节，SHA256 为 6a46739085734f04c32d37fec72ac4064c7c2d1a55530c3400c66bcfd7f3023f。打包示例见下方，沿用既有修复底包，只将 --out 改为该路径；当前 polaris.toml 已是 AviumUI 设置。只运行 cargo 不会替换已打包镜像中的 TOML。切回 Smartisan 时，legacy_version 改为 12.0.0 / 2022-09-05，data options 加回 inlinecrypt，然后重新打包。该配置不自动识别 ROM 的加密实现。

下方 Axion 和 Smartisan 内容是各自历史组合，不能混用挂载选项与 header。未添加动态分区支持，未改动通用解密源码；本轮最终持久修改只有 devices/polaris.toml 和本记录。已有的其他源码修改属于前序任务，未被覆盖。

诊断和镜像校验记录位于 target/diagnostics/polaris-avium-20261125/。本轮通过 fastboot boot 临时启动，未刷写 recovery、格式化、改写磁盘密钥或更改分区布局。正常解密流程会按配置将 data 挂载可写。成功范围为当前无密码 AviumUI 数据；有密码、更换 ROM 和长期使用未经本轮验证。

## Axion 2.8 / Android 16（2026-11-25，候选待验证）

本次日志中的失败为 `unsupported legacy file encryption mode`。设备只读检查确认 `/data/unencrypted/mode` 是 `aes-256-xts:aes-256-cts:v2`，公共密钥 ref 为 16 字节；data 可直接挂载为 ext4，没有发现本次需要修复文件系统的证据。它与此前 Smartisan 的 PRIVATE/ICE v1 模式不同。

同时确认三处启动环境问题：

- recovery 域打开的 APEX loop 设备报 I/O error，导致安全服务准备中止。使用本底包已有 `u:r:su:s0` 域的只读 loop helper，可正常挂载同一 runtime APEX。只改变短暂辅助进程的域，不更改 SELinux 全局状态。
- Axion 的 vendor fstab 将 `cust` 用作 EROFS `system_ext`，`hwservicemanager` 则是指向 `/system/system_ext/bin/hwservicemanager` 的绝对符号链接。配置补充 `/cust` 的 EROFS 方式，并在 API36 下于 ramdisk 创建链接；系统分区保持只读。ext4 cust 的原有条目仍保留。
- 当前 recovery 头和属性仍是 Android 12 / 2022-09，而设备 boot 头实测 `0x200001a7`，即 Android 16.0.0 / 2026-07。临时补齐库和服务后 Keymaster 3 已注册，但现有密钥查询返回 `-62`，内存升级返回 `-38`。候选镜像将 header 和属性同时匹配实际 boot，结果尚待重启验证，不能将版本匹配本身视为解密成功。

本次源码修改：`skyward-crypto/src/fscrypt.rs` 根据实际 mode 区分 v1 的 8 字节描述符与 v2 的 16 字节 identifier；v2 使用内核文件系统密钥 ioctl，校验公共密钥 identifier，拒绝 Keymaster 3 不支持的 wrapped-key 模式。`data.rs` 复用已有 Keymaster 3 解包与无密码 CE 路径，使用对应政策检查；v2 解锁后保留原挂载实例并正常 remount，避免完整 umount 丢失该文件系统的密钥。v1 路径及现代 AIDL KeyMint 调用选择不变。`skyward-config/src/lib.rs` 更新说明与 Polaris 配置校验。

本段历史候选配置的 `legacy_version` 面向上述 Axion boot；当前 `devices/polaris.toml` 已按顶部说明切换为 AviumUI。**切回此前 Smartisan 环境时，应改回 `12.0.0` / `2022-09-05` 并重新打包。此字段不能自动跟随更换后的 ROM；只修改 RAM 配置也不能修改本次启动传入的镜像头。**

候选输出 `target/skyward-polaris-419-axion.img`，沿用修复后的 4.19 内核、字体、NTFS 与 system-init。生成命令与下方相同，仅将 `--out` 改为该路径。配置和加密单元测试 102 项通过，主程序 arm64 Android 构建及镜像内 header、内核、fstab、配置和程序一致性校验通过。设备实际解锁及 UI 文件列表待验证。

只读诊断和构建记录保存在 `target/diagnostics/polaris-axion-20261125/`。未刷写 recovery、未格式化、未导出或改写磁盘密钥。

## 4.19 修复结果（2026-11-24）

已在原 4.19 底包上修复并验证：SmartisanOS 8.5.3 移植版、Android 11、无锁屏密码，DE/CE 密钥可读，内部存储文件可见，触摸正常。先通过只读 `ro,noload,inlinecrypt` 检查，再按正常流程打开 data。完整配置镜像又经一次 `fastboot boot` 验证，未修改内存配置即可解锁。未刷写 recovery、格式化或更换磁盘密钥。尚未测试其他 ROM、有密码解锁、长时间运行和 ROM 刷入。

根因在 4.19 内核的 legacy ICE 路径：新 TrustZone 调用在当前固件返回 `-95`，且原配置未启用旧 PFK 的按文件派生密钥。修复需要同时使用补丁后的内核和 `inlinecrypt`，仅修改 TOML 不足以修复。

- 4.19 配置已合并到 `devices/polaris.toml`，data 默认包含 `inlinecrypt`。两份配置此前唯一的运行参数差别就是该选项；不再维护单独的 `polaris-4.19.toml`。
- 该配置不会自动判断内核。如果改回此前验证的完整 4.9 底包，需先从 `polaris.toml` 的 `/data` options 中移除 `,inlinecrypt`，再打包；切回修复后的 4.19 时重新加回。其他机型配置和通用源码没有因此修改。
- `tools/polaris-419-legacy-ice.patch` 修改内核 Kconfig、`crypto-qti-tz.c/.h` 和 `ufshcd-crypto-qti.c`。新增开关默认关闭；启用后选用旧 SCM 调用及 PFK 派生，并保留硬件密钥槽 0、1。
- `tools/polaris-419-kernel.py` 提取配置、替换 Image.gz 并保留原 ramdisk 与全部 14 份追加 DTB。只接受本次验证的底包内核哈希。
- 本次未修改 Skyward 通用解密源码。此前 AVB2 尾部解析修改属于打包修复。

可用完整镜像：`target/skyward-polaris-419-legacyice.img`，47,472,640 字节（约 45.3 MiB），SHA256：

```text
9d66d3cdb9cf1bcf00c6190a9a064e02c1829b948192cfac8497318ab03e07ea
```

该文件是无 AVB footer 的 legacy 镜像。验证采用临时启动；本次没有正式刷写它。原 `polaris/recovery.img` 没有被覆盖。

## 使用本机已修复底包重新打包

已经生成的 `target/stock/polaris/recovery-419-legacyice-base.img` 包含修复内核及原始 recovery ramdisk。重新编译 Skyward 主程序后用它作为 `--stock`，配置统一使用 `devices/polaris.toml`：

以下历史命令原先验证于 Android 12 header 设置；当前配置已切换 AviumUI 的 Android 16 header。请按上方说明匹配目标 ROM，并选用不同 `--out` 保存镜像。

```bash
cd /home/skyward-diting
target/release/skyward-pack recovery \
  --stock target/stock/polaris/recovery-419-legacyice-base.img \
  --partition-size 67108864 \
  --binary target/aarch64-linux-android/release/skyward \
  --system-init target/system-init/aarch64-linux-android/release/system-init \
  --config devices/polaris.toml \
  --font target/fonts/MiSans-400-GB2312.ttf \
  --mono resources/fonts/KirkMono-Regular.ttf \
  --mono-license resources/fonts/KirkMono-OFL.txt \
  --ntfs3g target/ntfs3g \
  --out target/skyward-polaris-419-legacyice.img
```

字体和 NTFS 包延用本机已有产物；它们与本次解密修复无关。换到其他电脑时按原流程生成这些产物。只运行 Cargo 不会重建底包里的 Linux 内核。

## 在新环境重建修复内核

源码仓库：`https://github.com/polaris-avium/kernel_xiaomi_sdm845`，提交 `31d0b8099432`，与原 4.19 内核匹配。此次下载的 codeload tar.gz SHA256 为 `cb78f282ee716657ca966c013a74456164bd8d0fd93e4daf059caf09f7fb41af`。不要直接把补丁套到其他设备或其他版本内核上。

准备该提交的干净内核源码目录 `target/kernel-polaris-419`。Ubuntu 主机构建依赖包括 make、flex、bison、cpio、libelf-dev、libssl-dev、libzstd-dev、binutils-aarch64-linux-gnu、binutils-arm-linux-gnueabi。工具链使用本机 NDK r29 的 clang/LLVM。先用当前 Skyward 源码构建 `target/release/skyward-pack`，然后执行：

```bash
cd /home/skyward-diting
patch -d target/kernel-polaris-419 -p1 < tools/polaris-419-legacy-ice.patch
mkdir -p target/kernel-polaris-419-out
python3 tools/polaris-419-kernel.py extract-config \
  --stock polaris/recovery.img --out target/kernel-polaris-419-out/.config
target/kernel-polaris-419/scripts/config \
  --file target/kernel-polaris-419-out/.config \
  --enable QTI_CRYPTO_TZ_LEGACY_ICE --enable ENABLE_LEGACY_PFK \
  --set-str LOCALVERSION '-perf-skyward-legacyice' --disable LOCALVERSION_AUTO

export PATH="$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/linux-x86_64/bin:$PATH"
make -C target/kernel-polaris-419 O="$PWD/target/kernel-polaris-419-out" \
  ARCH=arm64 CC=clang LD=ld.lld AR=llvm-ar NM=llvm-nm \
  OBJCOPY=llvm-objcopy OBJDUMP=llvm-objdump READELF=llvm-readelf STRIP=llvm-strip \
  CROSS_COMPILE=aarch64-linux-gnu- CROSS_COMPILE_ARM32=arm-linux-gnueabi- \
  LLVM_IAS=1 olddefconfig
make -C target/kernel-polaris-419 O="$PWD/target/kernel-polaris-419-out" \
  ARCH=arm64 CC=clang LD=ld.lld AR=llvm-ar NM=llvm-nm \
  OBJCOPY=llvm-objcopy OBJDUMP=llvm-objdump READELF=llvm-readelf STRIP=llvm-strip \
  CROSS_COMPILE=aarch64-linux-gnu- CROSS_COMPILE_ARM32=arm-linux-gnueabi- \
  LLVM_IAS=1 -j"$(nproc)" Image.gz

python3 tools/polaris-419-kernel.py replace \
  --stock polaris/recovery.img \
  --kernel target/kernel-polaris-419-out/arch/arm64/boot/Image.gz \
  --out target/stock/polaris/recovery-419-legacyice-base.img
```

脚本拒绝覆盖已有输出；已有本机修复底包时可直接复用，或换一个输出路径。继续使用上方完整打包命令生成 Skyward 镜像。源码、配置、补丁和脚本可同步 GitHub；boot.img、ROM、内核构建树及大型镜像不应加入源码提交。

下面保留 4.9 对照排查记录；其中的“4.19 尚待验证”描述的是修复前状态。

2026-11-24，在 SmartisanOS 8.5.3 移植版（Android 11、无锁屏密码）上，完整的旧 4.9 recovery 启动环境配合当时的 Skyward 主程序及未添加 `inlinecrypt` 的 `polaris.toml`，已成功打开内部存储。用户确认原有文件可见、触摸正常。

## 已验证的组合

- 内核：`4.9.186-perf-g10af704`。
- 底包来源：本机此前的 `target/diagnostics/polaris-no-fastbootd-20261122/skyward-polaris-fastbootd-enabled.img`。
- 保留该底包的内核、启动头、init 脚本、SELinux 策略和动态库。
- 更新 `system/bin/recovery` 和 `skyward/device.toml` 为当前版本。`skyward/system-init` 经比较与当前版本相同。
- DE0、CE0/current 的密钥布局版本均读为 `1`，`/data/media/0` 可用。
- 主程序 SHA256：`2109e6effaf1b32686b56882dd8bd6b2026a430c01fd4334c4a24d8ced641773`。

验证镜像为 `target/diagnostics/polaris-smartisan-20261124/skyward-polaris-complete-49.img`，大小 66,908,160 字节（约 63.8 MiB，小于 64 MiB）。SHA256：

```text
f758d588f1b3267b8821a7f60b1a4a8027deb51cab734b20a0c801434fb79f6f
```

本轮通过 `fastboot boot` 临时启动验证，未刷写 recovery、未格式化或更换磁盘密钥。Skyward 成功解锁后会按正常流程将 data 挂载为可写。测试范围是启动、解密、文件列表及触摸；未验证长时间运行或 ROM 刷入。

## 修复前 4.19 底包的问题（历史记录）

本机 `polaris/recovery.img` 是新的 4.19 底包，并未在本轮替换为 4.9。其内核为 `4.19.325-cip130-st14-perf-g31d0b8099432`，镜像 SHA256：

```text
009f65b889843267a55dda9c61781c47612af5e16b3433edcca3d18999fbc2f5
```

相同 Skyward 主程序及配置使用该底包时，公共密钥目录可读，但 legacy ICE 加密目录中的文件内容异常。`layout Q` 是异常内容的表现，不能作为新的有效密钥布局来接受。只读添加 `inlinecrypt` 后，内核报 UFS/ICE 密钥槽编程失败 `-95`，并出现 I/O 错误。软件回退诊断也没有获得正确明文。

原版 4.19 recovery 可以启动，但它是类原生 recovery，本次未获得其解密能力的有效对照结果。仅将旧 4.9 内核放入 4.19 的启动环境也未成功进入 Skyward。因此当前证据支持使用完整的已验证 4.9 底包；4.19 内核、固件与启动环境的具体兼容修复仍待验证，不能宣称是 TOML 已解决的问题。

## 修复前的 4.9 备用方案（历史记录）

当前 `polaris/recovery.img` 仍是 4.19，继续以它作为 `--stock` 输入会保留本次失败的底包组合。只更新本配置不会改变内核或启动环境。应使用能解密该 ROM 的完整 4.9 底包重新打包，或使用上述已验证镜像。

本轮没有为此修改解密源码；`polaris.toml` 仅增加适用底包的说明。此前 `skyward-bootimg` 的 AVB2 尾部解析修改解决的是 4.19 镜像的打包报错，与 ICE 解密失败分属不同问题。

本机完整诊断结果和对照镜像构建脚本位于 `target/diagnostics/polaris-smartisan-20261124/`。这些是本地诊断产物，不应将 ROM、密钥或大型镜像提交到源码仓库。
