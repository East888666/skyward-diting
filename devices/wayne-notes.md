# 小米 6X / wayne 本机验证记录

## 打包容量

recovery 分区为 **64 MiB / 67,108,864 字节**（2026-11-23）。
用户提供的容量已通过 recovery 中的 blockdev --getsize64 只读查询核实。

在原打包命令中增加：

    --partition-size 67108864

此参数检查最终镜像是否超过分区，不会扩容、刷写或要求填充到分区大小。
现有 TOML 没有 partition_size 字段，不要加入无效配置项。
本机 target/skyward-wayne.img 当前为 35,180,544 字节（约 33.55 MiB），
低于该容量；此测量不代表运行验证。

## 底包发生变化

wayne.toml 原注释基于 MIUI V12.0.2.0.PDCCNXM 的 boot/recovery/vendor。
用户后来改用类原生 recovery。本次日志为
Linux 4.19.243-Melon-Kernel，recovery 属性 Android 12 / 2021-12-05，
不能把旧 MIUI 内核与加密说明当作新底包的验证结论。

打包器已兼容非 A/B 镜像的两个 init 脚本位置：
/init.rc 和 /system/etc/init/hw/init.rc。无需改 ab=false。

## USB OTG 未识别（2026-11-23）

证据备份：target/diagnostics/wayne-usb-20261123/。

- 940 秒和 963 秒附近插入设备时，内核注册 xHCI 控制器及 USB 2/3 根集线器。
- usb1/usb2 的 1d6b:0002/0003 是主机的根集线器，并非 U 盘。
- 日志没有下游 U 盘枚举、usb-storage SCSI 磁盘容量或 sda/sda1。
- usb-storage 驱动在启动时已注册。
- 所以当前证据停在 USB 外设枚举前，不能归因于 NTFS/exFAT 挂载，
  也不能仅靠增加 fstab 行解决。
- OTG 供电、转接头/设备连接、底包 USB 驱动状态是待验证方向，尚未确认根因。
- init 日志中的 MTP 链接已存在错误属于手机连接电脑的 gadget 路径，
  不能据此断定 U 盘枚举失败原因。

后续用同一 U 盘和转接头对照原版类原生 recovery/Android；
记录插入后指示灯状态。可对照使用有独立电源的合规 OTG 集线器，
避免自行接线或向手机反向供电。

插入 U 盘后，在 recovery 终端只读检查：

    ls -l /sys/bus/usb/devices
    cat /proc/partitions
    dmesg | grep -iE 'usb|xhci|scsi|storage|vbus|type-c'

进一步需要 USB 角色及供电属性快照；不要盲目强制写 host/peripheral
或电源寄存器。拿到证据后再确定按设备配置的修复。
USB 排查仅更新容量说明及诊断记录，未修改 USB 运行源码或刷写手机。

## 振动及启动闪屏（2026-11-23）

证据备份：target/diagnostics/wayne-haptics-display-20261123/。

当前 Melon 内核提供 qti-haptics 的 /dev/input/event2，支持 FF_PERIODIC；
没有旧 MIUI 使用的 timed-output 振动节点。此前配置没有 [haptics]，
界面因此没有可用的点击波形。设备树 waveform 0 对应 15 ms 点击。

已使用现有 recovery 的 haptic 子命令、临时配置 click=0、
strength={lightest=0.5,strongest=1.0} 实测，用户确认能感觉到振动。
相同设置已写入 wayne.toml。此验证证明波形可用；当前运行界面仍使用
旧配置，持久生效需要打包新镜像。

用户确认屏幕仅在启动时闪一次，此后首页稳定。日志显示 fbdev
首次呈现后执行了 BLANK_POWERDOWN/BLANK_UNBLANK。
新增 [display].fbdev_skip_initial_power_cycle=true，使 wayne 在
首次呈现时保留面板供电，仍执行原来的模式协商及内存映射。
真正息屏后的唤醒仍走完整供电恢复流程。其他设备缺省为 false，
保留原行为；KMS 显示路径不受此选项影响。

此显示选项需要同时重新编译主程序，单独给旧程序更换 TOML 不足以
生效。新候选镜像尚未刷入，启动闪屏是否消失及息屏唤醒是否正常，
仍需用户实机验证。此次没有自动刷写、重启手机或重启运行中的界面。

重新编译主程序与打包器：

    export ANDROID_NDK_HOME=/home/ndk29/android-ndk-r29
    cargo ndk -t arm64-v8a --platform 30 build --release --locked -p skyward
    cargo build --release --locked -p skyward-pack

候选镜像使用现有 system-init、精简字体及 target/ntfs3g 工具包，
打包容量上限为 67108864 字节。NTFS 工具被保留；USB OTG 尚未
完成外设枚举的问题仍待排查，不能据此宣称已修复。

## LOS 18.1 密码解锁修复（2026-11-25，4.4 内核已实测）

设备 ab494c3，MI 6X / wayne。用户确认：在临时启动的 4.4 内核
Skyward 中输入当前锁屏密码后，解锁成功、原有文件可见、触摸正常。
未刷写 recovery，未格式化 data，未修改密码或密钥。

### 诊断与适用范围

- 安装的 LOS 18.1 使用 Keymaster 3、Gatekeeper 1、qseecomd，
  data 为直接挂载的 ext4 FBE，模式 ice:aes-256-cts:v1。
- wayne.toml 已启用 legacy_fbe；仅在 SDK 30 启动上述已核对的
  安全服务、hwservicemanager，并匹配 qseecom/ion 的设备权限。
- 安装系统 boot 头为 Android 11 / 2022-10，系统补丁为 2022-10-05。
  原 recovery 头 Android 13 / 2023-02 导致 Keymaster configure -38。
  legacy_version 对齐版本后服务可启动，但仍不足以解密。
- Melon 4.19.243 内核提供 /dev/mmcblk0rpmb 字符节点，旧 vendor
  查找 /dev/block/mmcblk0rpmb。配置在 qseecomd 之前条件创建别名。
  补齐后 RPMB 初始化恢复，但全新启动仍无法打开 /data/unencrypted/key，
  Keymaster 返回 -33。失败发生在密码校验之前。
- 正常 LOS 内核为 4.4.302-lineageos-g2e69f2ae550d。换用该 boot
  内核并保留 recovery ramdisk 后，公共密钥探针与界面密码解锁均成功。
  证据指向两个底包的内核安全驱动环境差异，尚未定位具体内核缺陷。
  **本次没有修复 Melon 4.19 内核；只换配置而沿用该内核仍会复现。**
- MIUI 的 FDE、其他 Android 版本及其他内核不在此次解密验证范围。

### 必须保留的构建输入

- 原 wayne/recovery.img：保留原 ramdisk、布局及启动环境，未修改。
- wayne/boot-los18.img：从当前正常 LOS 18.1 只读提取的 boot。
  SHA256：81f557b2649410cafd40fc17c17ab136122b7b81a9535427f197401b5d80385d。
- 当前修复源码：旧版 FBE、Gatekeeper、文件型 keystore 的实现不可省略。
  单把 TOML 复制到旧版本源码不能保证解密。
- 当前字体 target/fonts/MiSans-400-GB2312.ttf、NTFS 工具 target/ntfs3g，
  以及常规编译生成的 skyward、skyward-pack、system-init。

4.4 内核的振动接口为 /sys/class/timed_output/vibrator，当前配置改为
40 ms 上限的 timed_output；现有 haptic 子命令已成功解析配置并播放
28 ms 点击。尚未由用户确认该次振动体感。此前 Melon 4.19 的 click=0
配置属于另一驱动接口，不能直接作为 4.4 振动配置。

### 重新构建

在 /home/skyward-diting 下运行，NDK 路径按本机安装调整：

```sh
export ANDROID_NDK_HOME=/home/ndk29/android-ndk-r29
cargo ndk -t arm64-v8a --platform 30 build --release --locked -p skyward --no-default-features
cargo build --release --locked -p skyward-pack
RUSTFLAGS="-C target-feature=+crt-static" cargo ndk -t arm64-v8a --platform 30 build --release --locked -p skyward-system-init --target-dir target/system-init

mkdir -p target/stock/wayne
target/release/skyward-pack boot \
  --kernel wayne/boot-los18.img \
  --ramdisk wayne/recovery.img \
  --out target/stock/wayne/recovery-los18-kernel44.img

target/release/skyward-pack recovery \
  --stock target/stock/wayne/recovery-los18-kernel44.img \
  --partition-size 67108864 \
  --binary target/aarch64-linux-android/release/skyward \
  --system-init target/system-init/aarch64-linux-android/release/system-init \
  --config devices/wayne.toml \
  --font target/fonts/MiSans-400-GB2312.ttf \
  --mono resources/fonts/KirkMono-Regular.ttf \
  --mono-license resources/fonts/KirkMono-OFL.txt \
  --ntfs3g target/ntfs3g \
  --out target/skyward-wayne-los18-password.img
```

no-default-features 保留本次关闭 Wi-Fi 的构建方式，不是解密修复开关。
精简字体和 NTFS 工具需要提前生成，可参照仓库 README 与 tools 脚本。
两步打包命令也保存在诊断目录 pack.sh 中；保留 boot-los18.img 是重现
此次兼容结果的必要条件，不能直接恢复使用原 4.19 stock 的打包命令。

### 产物与验证

- 最终整理镜像：target/skyward-wayne-los18-password.img。
  32,694,272 字节，约 31.18 MiB，小于 64 MiB。
  SHA256：dc2d326dd31069d5d2c5a92fa3f9cff243a2382b084ef70c83423e0352e7d121。
- 已实机验证的对照镜像：target/skyward-wayne-los18-kernel44.img。
  SHA256：2947363de9a25a2d7ff6c94bd0b9d10ca500a826873f8aafa42c901a62f43b82。
  最终整理版在该方案上更新 timed_output 配置和密码日志脱敏，
  没有再修改解密流程；最终整理版尚未重新临时启动。
- 配置测试 38 项通过；最终 Android arm64 release 构建与打包通过。
  镜像头为 Android 11 / 2022-10，包含 system-init、NTFS 工具和当前配置。
- crates/skyward/src/ui.rs 屏蔽 Unlock effect 中的密码字节，避免其写入
  recovery 日志；不改变解密行为。
- 诊断目录：target/diagnostics/wayne-los18-password-20261125/。
  其中 .log 的 Unlock code 已脱敏；临时密钥探针源码移入该目录，
  不属于生产构建输入。不要提交含个人凭据的原始日志。

旧失败镜像的 SHA256 为
15ac79579ac54d2fc40b36f72cc4f4d42dde3e03ca49d472cefcc9ce3233da7b，
该路径已被最终镜像替换；不要将旧镜像当作此次修复版。

## 保留 Melon 4.19 的后续验证（2026-11-26，尚未完整修复）

用户要求保留 Melon 4.19，且只有提取的 recovery，没有对应内核源码。
本轮只临时启动诊断镜像，原 wayne/recovery.img 和正式 wayne.toml 未改。

新增证据：旧 vendor 的 librpmb.so 发出 MMC_IOC_RPMB_CMD 0xc0d8b300，
Melon 的 RPMB 字符节点返回 EINVAL(22)。因此此前的路径别名不够。
诊断兼容层在 qseecomd 内将该请求转换为 MMC_IOC_MULTI_CMD，保留命令
顺序、可靠写标志、缓冲区指针和响应；仅匹配对应 RPMB 字符设备。
从首次启动加载后，同一公共密钥的 getKeyCharacteristics、AES-GCM begin、
abort 都返回 0，内核仍是原 Melon 4.19.243，未替换内核或设备树。

但这不等于完整解密：不开 inlinecrypt 时，DE key 的 version 文件解读为
乱码；启用现有只读自动挂载选择后，inlinecrypt 路径返回 EIO。内核明确
记录安全调用 0x42001005 / 0x42001006 失败，硬件密钥配置返回 -95，
错误位于 eMMC/CQHCI 加密驱动。不能靠 RPMB 用户态转换同时修复此错误。
未让用户尝试密码，也未宣称该诊断镜像可以读取原有文件。

诊断及源码：target/diagnostics/wayne-melon419-compat/README.md。
辅助转换测试在 ASan/UBSan 下通过；完整文件解密仍失败。
Polaris 的 UFS 补丁不适合直接应用到 wayne eMMC/CQHCI，禁止当作已验证修复。
若必须保留当前 Melon 二进制，尚无完整、已验证的解密方案；后续需要对应
源码或另一个已修正旧版 ICE 的同设备 4.19 构建。4.4 成功方案仍保留作对照。
### 后续补充：已取得 Melon 源码线索（2026-11-26，仅静态对照）

用户提供 clarencelol/kernel_xiaomi_sdm660-4.19，已固定 melon 分支提交
73df42e5d695f16ea7445ab4d493044ffae2997a 并下载检查。Makefile 的 4.19.243
和 wayne_defconfig 的 Melon 名称与现有 recovery 内核对应，但配置差异较多，
不能认定为完全相同的构建输入。

源码确认新接口为 set-key 0x5 / invalidate 0x6；LOS 18 的 SDM660 旧驱动
实际使用 set-key 0x2（独立 key/salt 两个缓冲区）/ invalidate 0x3，
保留槽位 0、1。与 Polaris UFS 的 0x4 补丁不同，不能只替换函数编号。
后续需要联动检查 eMMC CQHCI 槽位映射及 PRIVATE/PFK 的每文件密钥派生。

用户要求不编译，本次仅下载和静态对照，没有编译、应用内核补丁或启动新镜像，
也没有更改正式 wayne.toml。完整对照在
 target/diagnostics/wayne-melon419-source/README.md。

### Melon 旧 ICE 兼容代码候选（2026-11-26，未编译验证）

用户允许继续修复，同时保留“不编译”要求。本轮已在固定 Melon 源码中
修改 5 个现有文件、增加 1 个配置片段，并导出为可同步文件：
`tools/wayne-419-legacy-ice.patch`。补丁修改内核，不是 Skyward Rust。

新增默认关闭的 `CONFIG_QTI_CRYPTO_TZ_SDM660_LEGACY_ICE`，限制 SDM660
eMMC/CQHCI 且不启用 QTI UFS；设置旧版 PFK 派生、旧 set-key 0x2 和
invalidate 0x3，以及单独的 32 字节 key/salt 地址。密钥编程、移除和请求
描述符统一映射硬件槽位 2–31，禁用 wrapped key，保留旧数据的字节顺序。

此前实测的 RPMB 辅助源码复制到 `tools/wayne-melon419/`，生成配置脚本和
完整说明也在此处。脚本生成 `target/wayne-melon419-legacyice.toml`，启用
只读 legacy_fbe_auto、Melon 振动和 qseecomd 专用 LD_PRELOAD；库本体仍需
在未来构建时单独编译并放入 ramdisk，不能仅复制 TOML。

正式 wayne.toml 本次只更新说明性注释，继续保留已验证的 4.4 配置值。
没有修改全局 Rust 行为、编译任何程序、生成新镜像或操作手机。

静态检查：从原始固定源码压缩包中取出文件，补丁应用及反向应用完整往返
通过；TOML 解析、已有服务条件/账户/权限与挂载项保留检查通过。checkpatch
0 errors、0 checks，只有新增文件的 MAINTAINERS 登记 warning。Git 空白
检查通过。以上不等于内核编译通过或 data 已成功解密。

补丁 SHA256：ac521e050abeb6de09096cc9ba78c2700ebb38e6ba656d99f30cee32d2175c93。
