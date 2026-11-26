> 2026-11-24 配置入口更新：4.19 底包统一使用 devices/jason.toml。
> 已合并 MIUI 12 明文 data 处理，并按已安装系统 SDK 选择旧 FBE 服务。
> 下文分阶段记录保留作排查依据；jason-miui12.toml 是此前专用配置快照。
> LOS 23.2 / 4.4 底包仍使用 devices/jason-los23.toml。

# 小米 Note 3 / jason 配置说明

本配置参考 polaris.toml、wayne.toml 的结构，参数来自本机
jason/boot.img 与 jason/recovery.img；适用于这两份镜像对应的类原生布局。
编写日期：2026-11-24。初版为镜像离线分析；后续实机诊断见文末。

## 已核对的镜像信息

- 两份镜像均为 Android boot header v0，版本 15.0.0 / 2026-06。
- recovery 的 prop.default 标明设备为 jason、型号 Mi Note 3、
  SDK 35、安全补丁 2026-06-01、密度 420。
- 内核为 4.19.325-cip132-st16-perf-gca289b26c558。
- 两份镜像的内核完全一致，boot fstab 与 recovery fstab 完全一致。
- 内核附带 Jason 专用设备树，board-id 为 30,0，SoC 为 SDM660。
- JDI R63452、Sharp TD4322 两种 Jason 面板均为 1080×1920、
  60 Hz，亮度最大值为 4095。显示使用 fbdev，未启用 DRM。
- Synaptics DSX FORCE、ST 触控、USB storage、ext4、f2fs 均内置于内核，
  不照搬其他设备的外部触控模块或 sysfs 写入。

证据提取目录：target/diagnostics/jason-config-20261124/。

## 分区布局的关键区别

这是非 A/B 表，system 直接使用固定分区；没有 super 或 metadata 条目。
cust 被这份 ROM 当作 vendor，挂载在 /vendor，不能照搬 polaris/wayne
的独立 vendor 分区，也不能再把同一 cust 同时作为独立 /cust 分区使用。
备份列表使用实际分区名 cust；格式化页面的 vendor 则通过 /vendor
挂载条目解析为 cust。格式化 vendor 会清空 cust 中的 vendor 内容。

data 和 cache 均保留 f2fs、ext4 两种记录；data 保留
fileencryption=ice、quota，属于该 ROM 声明的旧式 FBE。
不添加 metadata，不把原系统的 FDE 或其他 ROM 当成同一种加密布局。
配置的 formattable 是清除页面的授权标记；其余挂载选项来自原表。

后续已在 Note 3 的 Android 中通过 root 只读查询确认 recovery 容量为
67,108,864 字节（64 MiB）。打包命令使用 --partition-size 67108864
检查最终镜像；该值来自实机，而非从镜像文件长度推断。

## 功能验证范围

- 振动：实机使用 qcom,qpnp-haptic 的 timed_output/enable 接口。
  本次新增 timed_output 后端，在 [haptics] 指向实际驱动目录。
  drive=60，强度滑块控制 30–60 毫秒的持续时间；沿用驱动原来的
  电压、波形校准。直接写入 60 毫秒已由用户确认有振动，
  新主程序的启动/停止验证结果见文末。没有复制 FF waveform 或 LED 接口。
- 显示与触控：本次实测首页稳定；首次 fbdev 供电循环会让触控无事件，
  一次息屏唤醒可恢复。现已设置 fbdev_skip_initial_power_cycle=true，
  冷启动后触控 IRQ=1，用户确认点击及滑动正常。后续发现长期息屏恢复时
  面板已亮而触控 IRQ 仍为 0；现对 Jason 设置 keep_powered=true，
  息屏只关背光并显示黑帧，避免面板断电及系统挂起，耗电会增加。
- 解密：后续已在 LineageOS 22.2、无锁屏密码的实机上验证 v1 FBE
  解密成功，见文末。需要本次配套 Rust 修复和更新后的服务配置，
  不能仅把 TOML 复制到旧版程序。设置锁屏密码或 Weaver 的布局尚未支持。
- Wi-Fi：未设置 [wlan]，没有承诺 Wi-Fi 可用。单纯省略此配置不会把
  Wi-Fi 代码从主程序移除，需要精简程序时使用 --no-default-features。
- OTG：内核有主机及存储驱动，原表带 vold 的 USB 设备通配符，
  但枚举、供电、文件系统挂载仍需实机验证。NTFS 需保留 ntfs3g 工具包。
- fastbootd：保留底包和当前源码的默认行为；非 A/B 并不自动关闭它。

## 本机编译及打包

在 /home/skyward-diting 中执行。先编译主程序和打包器：

    export ANDROID_NDK_HOME=/home/ndk29/android-ndk-r29
    cargo ndk -t arm64-v8a --platform 30 build --release --locked -p skyward
    cargo build --release --locked -p skyward-pack

如需不含 Wi-Fi 代码的主程序，将第一条 cargo 命令改为：

    cargo ndk -t arm64-v8a --platform 30 build --release --locked --no-default-features -p skyward

system-init 是独立的静态 Android 程序。现有本机文件在：
target/system-init/aarch64-linux-android/release/system-init。
缺失时按仓库 README 对 skyward-system-init 的要求单独编译。

Jason 首次测试中约 44.4 MiB 的原镜像无法启动；约 42.64 MiB 的固定字重
候选镜像可启动。64 MiB 是分区容量，不代表该启动链可启动所有小于它的镜像。
目前对照实验支持“载荷大小相关的早期启动失败”，尚未取得大镜像的内核
崩溃日志，不能断言是某个已确认的 bootloader 内存地址冲突。

先用仓库自带脚本生成固定 400 字重的精简字体（保留 UI 字符、GB2312）：

    python3 -m pip install -r tools/requirements-fonts.txt
    python3 tools/subset-font.py --weight 400 --out target/fonts/MiSans-jason-400.ttf

--weight 是可选参数；其他设备原来的调用保持可变字重行为。
静态字体约 2,006,056 字节；某些罕见文件名字形可能需要 --extra-text。
打包示例保留本机已有 NTFS 工具包及 system-init：

    target/release/skyward-pack recovery \
      --stock jason/recovery.img \
      --partition-size 67108864 \
      --binary target/aarch64-linux-android/release/skyward \
      --system-init target/system-init/aarch64-linux-android/release/system-init \
      --config devices/jason.toml \
      --font target/fonts/MiSans-jason-400.ttf \
      --mono resources/fonts/KirkMono-Regular.ttf \
      --mono-license resources/fonts/KirkMono-OFL.txt \
      --ntfs3g target/ntfs3g \
      --out target/skyward-jason.img

精简字体、system-init 和 NTFS 工具包是本机既有产物，不是 TOML 内嵌资源；
重新下载源码时应按项目工具说明重新生成。Jason 暂不建议换回完整字体，需先验证镜像能正常启动。
上述命令已加入实测 recovery 分区容量检查。
使用这里的类原生 recovery 需要本机打包器对
/system/etc/init/hw/init.rc 的兼容支持。

## 实机首次启动失败的诊断

用户报告初版镜像尚未显示首页就自动回到 Android，而原始 Lineage
recovery 可以保持运行。已通过 Android root 读取确认：
recovery 分区前 46,587,904 字节与本机 target/skyward-jason.img
的 SHA-256 完全一致（70ff3eb288732b1ee3ea7a954e380653231b8096c7c83d3a8aaafb7ba2198095）。
因此不是刷入了另一份镜像或超出 64 MiB 分区。

同一主程序在 Android 中执行 --version、info --config 均成功。
这不能证明 recovery 启动阶段正常。原有 /cache/recovery/last_log、
last_kmsg 属于原版 recovery，不是 Skyward 的故障记录。
该内核的 ramoops 启动时报 reserved-memory 资源缺失，pstore 为空；
目前尚无足够日志确认自动重启的根因。

已另行生成仅用于采集的
target/skyward-jason-startup-diagnostic-20261124.img。
它保留初版的内核、主程序、配置及属性，只增加启动日志服务，
在 init 阶段挂载已确认的现有 cache（mmcblk0p67），将内核日志、
属性、进程及 Skyward 日志复制到 /cache/recovery/skyward-jason-startup/。
没有格式化操作或 data 挂载操作。日志在失败重启后可从 Android root
或正常的其他 recovery 中读取。此镜像是诊断产物，不是已验证的修复。

证据及生成脚本位于 target/diagnostics/jason-reboot-20261124/。
修复前不要根据其他机型的经验随意修改供电、密钥版本或分区布局。


### 2026-11-24 临时启动采集结果

经用户授权，已使用 fastboot boot 临时运行启动日志诊断镜像；
fastboot 返回发送和启动成功，随后回到 Android。Android root 中未发现
/cache/recovery/skyward-jason-startup/，所以没有拿到该次 Skyward 启动日志。
不能据此将故障定性为 UI、触控或某个分区挂载错误。

又生成 target/skyward-jason-early-diagnostic-20261124.img（47,558,656 字节），
只将 /init 替换为静态日志入口，之后执行原有 /system/bin/init；
内核、Skyward、配置、属性及 init 服务文件未改。静态入口尝试将日志写入
现有 cache 的 /recovery/skyward-jason-early/。cache 的实际设备号已在
当前 Android 中只读确认是 259:35（不是按 mmc 分区名猜测 179:67）。

第二次 fastboot boot 也返回成功，但用户观察停在 MI 标志或黑屏，
没有 ADB，fastboot 查询报 no link。该镜像仍是诊断产物，不能当作修复版使用。
后续对照结果见下一节。上述两次均未执行 flash、format、
槽位切换或 data 清除，也未据这两次结果猜测调整配置。


### 已验证的启动及首次触控调整

对照结果已保存到 target/diagnostics/jason-reboot-20261124/：

- 原版 recovery + 当前主机 ADB 公钥：菜单和 ADB 正常。
- 原版 recovery + Skyward 属性：ADB 正常。
- 原版 recovery + Skyward MTP 脚本：ADB 正常。
- 全部 Skyward 载荷 + 原版 recovery 程序及原版 init 脚本：无 ADB，
  仍卡在早期启动，说明不能仅归因于 Skyward UI 或 MTP 脚本。
- 缩小字体并暂去辅助组件：Skyward 首页出现；首次触摸无事件，
  一次正常息屏唤醒后 IRQ 从 0 变为 1，用户确认触摸恢复。
- 恢复全部 NTFS 及 system-init，改用固定 400 字重的同字符集字体，
  并在 Jason 配置开启 fbdev_skip_initial_power_cycle：
  44,769,280 字节的含主机公钥诊断镜像成功启动，IRQ=1，
  用户确认“首页稳定，触摸正常”。持续 ADB 连接下未再自动重启。

已用更新后的 tools/subset-font.py 从完整字体重新生成可重现的静态字体，
并使用常规 skyward-pack recovery 命令生成：

    target/skyward-jason-fixed-20261124.img
    44,707,840 字节（约 42.64 MiB）
    SHA-256: 827e808f27addcf5fbf4e753018aeb0701c0ebf5b8f83ba08a8a57b13f519672

该常规输出尚未直接实机启动验证；已确认首页和触摸正常的是上述含公钥诊断镜像。
常规输出保留原版 ADB 认证策略；本轮可读日志的诊断镜像额外加入了当前
电脑的公钥。原版 /adb_keys 是符号链接，当前 --adb-key 参数会拒绝它，
所以临时公钥覆盖只存在 target 中的诊断产物，没有把个人公钥提交进配置。

上述启动/触控阶段的持久化改动限于 devices/jason.toml、devices/jason-notes.md、
tools/subset-font.py；该阶段没有新增 Rust 运行时改动。随后解密阶段的 Rust
改动见下节。其他机型无需开启此供电选项或改用静态字体。
未刷写 recovery、未格式化、未清除 data。

## 2026-11-24 LineageOS 22.2 解密适配

用户确认当前系统为 LineageOS 22.2，未设置锁屏密码。
日志在挂载 F2FS 后提示 AIDL KeyMint 未运行；实机 vendor 实际提供
Keymaster 3.0 和 Gatekeeper 1.0，并非 AIDL KeyMint。
/data/unencrypted/mode 是 ice:aes-256-cts:v1，记录的公共
key descriptor 为 8 字节。CE key 则由 v3 synthetic password 保护，
所以只添加 HAL 或只尝试硬件 nopassword CE 都不够。

修复包括：

- devices/jason.toml 显式设置 [recovery] legacy_fbe = true，保留原
  Android 15 header 和从已安装系统读取的安全版本，不借用 legacy_version
  去强制指定或改变系统版本。
- 按本机 RC 的实际路径、用户、组启动 hwservicemanager、qseecomd、
  Keymaster 3.0、Gatekeeper 1.0。hwservicemanager 位于
  /system_root/system/system_ext/bin/；不能使用其原始指向 recovery
  不存在目录的 /system_root/system/bin/hwservicemanager 符号链接。
- crates/skyward-config/src/lib.rs 新增默认关闭的 legacy_fbe；
  crates/skyward/src/main.rs 在建立 UI/解密线程前准备旧式 fscrypt keyring；
  crates/skyward/src/decrypt.rs 用该选项选择 v1 FBE 解密。
  原有 Polaris legacy_version 所选择的路径保持兼容，A/B 设备拒绝此选项。
- crates/skyward/src/secure.rs 修正 HIDL-only 启动时也应规范 recovery
  VINTF fragments 的类型，并避免重复声明 HIDL manager。否则实际日志会报
  device manifest 无法加入 framework manifest，hwservicemanager 自行停用。
  同时支持从已安装 system/system_ext 暴露服务管理器可执行文件。
- crates/skyward-crypto/src/data.rs、user.rs、keystore.rs 增加
  无锁屏、secdiscardable 绑定的 v3 synthetic-password CE 解密。
  通过 Keymaster 3 解开外层，校验软件层，再安装 v1 fscrypt key；
  受密码保护、Weaver 或与其他用户绑定的凭据会明确拒绝。
- crates/skyward-crypto/src/keymaster3.rs 对未绑定 application-id 的密钥
  省略空 APPLICATION_ID 参数。本机旧 Qualcomm HAL 对显式空参数返回 -41；
  去除后成功。公共/DE 密钥有非空绑定，保留原参数。
  没有生成、删除或写回升级后的密钥。

临时独立程序实际返回：

    /dev/block/bootdevice/by-name/userdata holds f2fs at /data
    every user's own files can be read

/sdcard 已存在并可枚举，data 以正常 fstab 选项读写挂载。
本次没有格式化 data、没有修改锁屏或刷写分区。冷启动验证结果另行记录。
源码测试覆盖 v3 双层校验、错误绑定/损坏标签的拒绝、密码/Weaver 的拒绝、
空 application-id 参数、旧 Polaris 路径和 A/B 默认行为。

重新编译必须重新构建主程序和 skyward-pack，再使用上面的固定字重字体
打包命令。仅替换配置、继续使用旧主程序会复现缺失此处理路径的问题。
诊断目录：target/diagnostics/jason-decrypt-20261124/。

### 完整启动时的 APEX loop 权限修复

上一候选 target/skyward-jason-decrypt-fixed-20261124.img 仅通过了独立程序
测试，完整启动时仍失败，已被下列镜像替代。内核的实际拒绝是：

    scontext=u:r:kernel:s0 tcontext=u:r:recovery:s0 tclass=fd { use } permissive=0

loop 线程不能使用 recovery 打开的 APEX 文件描述符，因而 loop0 报 EIO，
运行库无法挂载，整个硬件服务启动链中断。recovery 域本身 permissive
不会豁免发起访问的 kernel 域，所以 ADB/su 中的临时解密成功不代表
init 启动的 recovery 也能工作。此处不是 data 损坏或密码错误，
也没有把 EIO 忽略后继续尝试文件系统来掩盖真正的权限错误。

devices/jason.toml 显式设置：

    [recovery]
    legacy_fbe = true
    apex_loop_context = "u:r:su:s0"

crates/skyward-config/src/lib.rs 新增默认 None 的 apex_loop_context；
crates/skyward/src/main.rs 增加内部一次性 apex-loop 子命令；
crates/skyward/src/secure.rs 仅在配置该选项时，通过底包现有的
/system/bin/runcon，在指定上下文中运行同一程序的这个子命令，
只建立只读 loop 映射，返回设备路径后立即退出。

UI 和硬件服务仍是 recovery 域；SELinux 全局仍为 enforcing=1。
没有修改系统分区、原始 policy 或持久化 SELinux 设置。
其他机型没有设置此选项，保持原来的直接 loopdev::attach 路径。
本选项依赖底包存在 runcon 和可用的指定域；Jason 此底包已验证。

完整启动实测（含主机公钥的诊断镜像，通过 fastboot boot 临时启动）：

- 启动约 3.67 秒 runtime APEX 通过 loop0 挂载为 ext4。
- hwservicemanager、qseecomd、Keymaster 3.0、Gatekeeper 1.0 均启动。
- 启动约 8.15 秒 data 成功挂载，日志显示 closed users 为 []，
  MTP 提供 /data/media/0。
- 用户确认“能看到文件，触摸正常”。
- 未执行刷写、格式化、槽位切换或锁屏更改。

35 个配置测试通过；本次解密改动的 64 个 crypto 测试已通过。
Android release 和打包器构建成功，git diff --check 通过。
完整启动日志：target/diagnostics/jason-decrypt-20261124/loop-cold-boot/。
最终状态：同目录 loop-verified-state.txt 与 loop-verified-kernel.log。

常规修复镜像（保留原版 ADB 认证，未额外内嵌本机公钥）：

    target/skyward-jason-decrypt-loop-fixed-20261124.img
    44,711,936 字节（约 42.64 MiB）
    SHA-256: 48ac7bad3ade04373d8b571c11682a355faf19da105e535cc8bf6ec716336d2c

实机完整启动验证的是同一配置和程序、仅额外包含主机公钥的诊断镜像：

    target/skyward-jason-decrypt-loop-adb-diagnostic-20261124.img
    44,773,376 字节
    SHA-256: 636fc4c7dbf6478be50f41ba5c64ae1e3fc984f502e7c0d704f0eea1791e49ad

常规镜像本身尚未另行启动；不能把含公钥诊断产物当作可公开发布的镜像。
重新编译使用本节全部配套源码、更新的 devices/jason.toml 和上文相同的
构建/打包命令即可。仅复制 TOML 到没有本次 Rust 修复的源码仍会失败。


## 2026-11-24：息屏触控与振动修复

故障实测：Synaptics DSX FORCE 触控在面板点亮后仍有 irq_enable=0，
再次电源键唤醒可恢复。现有日志尚不能定位驱动内部的具体竞态，
所以采用当前已有的设备级 keep_powered 选项绕开断电恢复流程，
没有宣称已修复内核驱动。仅 Jason 配置新增该值，其他设备保持原值。
fbdev_skip_initial_power_cycle=true 也继续保留。

注意：keep_powered=true 会在 recovery 息屏时保持显示硬件工作，并阻止
系统进入挂起，因此比正常面板断电更耗电；它仅随本 recovery 配置生效。
不在启动脚本或 [touch] 中强制写 irq_enable。一次诊断中在面板已关闭时
直接写 irq_enable=1 触发内核 enable_irq WARN，说明这不是可靠的恢复方法；
本次最终配置和源码没有包含该操作。

源码 crates/skyward-config/src/lib.rs 为 [haptics] 新增可选 timed_output，
与 FF 和原 timed 接口互斥；绝对路径、正持续时间和强度范围均会校验。
crates/skyward/src/haptics.rs 新增独立后端：向 enable 写毫秒数启动，
向 enable 写 0 停止，复用原来的点击替换和异步停止逻辑。
未设置 timed_output 的设备继续使用各自原后端。

本机 devices/jason.toml：
    
    [display]
    fbdev_skip_initial_power_cycle = true
    keep_powered = true
    
    [haptics]
    timed_output = "/sys/class/timed_output/vibrator"
    drive = 60
    strength = { lightest = 0.5, strongest = 1.0 }

需要重新构建主程序及打包器，再使用上文相同的打包命令。
只替换 TOML、仍打包旧版二进制无法新增 timed_output 支持。
本次诊断及测试结果保存在 target/diagnostics/jason-sleep-haptics-20261124/。

本轮验证结果：

- 36 个 skyward-config 测试通过，Android release 与 skyward-pack 构建通过。
- 主程序 haptic 命令实测 position=1 播放 60 毫秒，position=0 播放
  30 毫秒；播放后 enable 回到 0，没有改写驱动电压或校准参数。
- fastboot boot 临时启动含主机公钥的诊断镜像；没有刷写 recovery。
- 实际完整启动日志显示 APEX loop helper 成功、timed_output 后端加载成功，
  打开 data 后 f2fs 挂载成功，MTP 提供 /data/media/0。
- 约第 92、107、122、137 秒采样：背光为 0，panel_power_on=1，
  触控 IRQ=1；约第 152 秒唤醒后背光恢复 410，IRQ 仍为 1。
- 用户完成息屏静置约一分钟后的验证，确认“触摸、振动、内部存储均正常”。
  该结果是本次短期实测；没有把它写成长时间压力测试的结论。

常规镜像（保留原版 ADB 认证）：

    target/skyward-jason-sleep-haptics-fixed-20261124.img
    44,711,936 字节，约 42.64 MiB，小于实机 64 MiB recovery 分区
    SHA-256: 17aa32feca29ece9a02e6df7125d45028d47fc7349abf04199557d7f32deab94

本次实机临时启动的镜像：

    target/skyward-jason-sleep-haptics-adb-diagnostic-20261124.img
    44,773,376 字节
    SHA-256: babaea880f867c89dbfc4d03138bc8e8da5963e4a6070ce54c535256ea959538

诊断镜像额外含本机 ADB 公钥，不作为公开发布镜像。
常规镜像与其配置和程序相同，但尚未单独启动验证。
重新编译沿用上文命令，将 --out 改为所需文件名即可。


## 更换 LOS 23.2 底包时的启动限制

2026-11-24，jason/recovery.img 被替换为
lineage-23.2-20260215-recovery-jason (1).img，
SHA-256 为 e13bfa1600e19c049a8d1ddf934f38fd4828b860b98d1e55549c064fb3d79f0f。
它使用 Android 16、4.4.302-perf-g4f5944cce207，
并非原先已验证的 Android 15 / 4.19.325 底包。
前文关于两份 boot/recovery 内核相同的判断只适用于原来的 LOS 22.2 文件；
当前 jason/boot.img 未随之更换，不能再按同一内核对待。

对照结果：

- 新底包加入本机 ADB 公钥后可以通过 fastboot boot 正常启动。
- 相同 Skyward 二进制能在原始新底包中运行 --version、info，
  因此没有把 4.4 内核笼统判定为不支持 Rust。
- 完整 gzip Skyward 镜像 36,327,424 字节，启动失败并返回 Android；
  加入启动跟踪的 gzip 版本同样失败，没有取得其 PID 1 日志。
- 减少字体和附加载荷的 gzip 对照为 33,329,152 字节，
  可以进入 Skyward：首页已有 frame Home 和 IRQ=1。
- 更关键的对照：把完整跟踪镜像的同一份 44,133,808 字节 cpio
  改成 LZMA-alone，镜像降到 30,830,592 字节，可以进入 Skyward；
  中文字体、NTFS、system-init 均完整保留。
  这说明启动失败与压缩后的镜像载荷/启动链有关，尚无崩溃堆栈确认
  具体 bootloader 内存冲突或固定上限。64 MiB 分区容量并不等同于此限制。
- 新内核配置明确有 CONFIG_RD_LZMA=y，没有 CONFIG_RD_XZ。
  必须使用 LZMA-alone；不能用 XZ 容器替代。

为保留此前 4.19 的已验证配置，新建 devices/jason-los23.toml。
该配置 [recovery] ramdisk_compression="lzma"，
并把振动从 timed_output 改为本机 qti-haptics 的 FF waveform 0。
haptic 命令使用驱动波形 0，单次 26 毫秒。用户最初没有感到振动；
重新测试后确认 100% 强度有振动，连续三次 50% 强度也均可感知。
因此保留 strength={lightest=0.5,strongest=1.0}，无需修改驱动电压、
波形或校准参数。用户随后确认界面设置振动、息屏约一分钟后唤醒触摸均正常。
原 jason.toml 保留 4.19 的 timed_output 以及原默认压缩方式。

源码变化：
- crates/skyward-config/src/lib.rs：可选 ramdisk_compression 枚举；
  默认 None，未配置的设备继续保留其底包原压缩方式。
- crates/skyward-pack/src/main.rs：按显式配置选择最终 ramdisk 编码。
- crates/skyward-bootimg/src/ramdisk.rs：LZMA-alone 编码、识别及解码，
  保证 pack list/cat/repack 和运行时读取 recovery 时也认识该格式。
- crates/skyward-bootimg/Cargo.toml 与 Cargo.lock：使用已有版本
  lzma-rs 0.3.0 作进程内解码；主机高压缩编码调用 xz 工具。
  gzip/LZ4 路径保持原逻辑，没有对其他配置自动切换压缩格式。

在 Ubuntu/WSL 需要 xz-utils（本机已安装），随后重新编译主程序和打包器：

    sudo apt-get install xz-utils
    export ANDROID_NDK_HOME=/home/ndk29/android-ndk-r29
    cargo ndk -t arm64-v8a --platform 30 build --release --locked -p skyward
    cargo build --release --locked -p skyward-pack

使用新底包时改用下列配置，其余功能及固定字重字体保留：

    target/release/skyward-pack recovery \
      --stock jason/recovery.img \
      --partition-size 67108864 \
      --binary target/aarch64-linux-android/release/skyward \
      --system-init target/system-init/aarch64-linux-android/release/system-init \
      --config devices/jason-los23.toml \
      --font target/fonts/MiSans-jason-400.ttf \
      --mono resources/fonts/KirkMono-Regular.ttf \
      --mono-license resources/fonts/KirkMono-OFL.txt \
      --ntfs3g target/ntfs3g \
      --out target/skyward-jason-los23-lzma-fixed-20261124.img

以上新配置只验证特定底包及启动，并不表示能解密任何已安装 ROM。
本轮设备实际返回的 Android 系统版本为 13；当前服务启动日志提示
安装分区未找到 com.android.runtime，因此本轮不能声称已验证 data 解密。
旧配置里的 Keymaster 服务路径来源于此前 LineageOS 22.2 vendor；
更换系统时应按实际已安装 vendor/system 单独核对，不能靠更改底包版本号解决。
本轮没有格式化 data，也没有刷写分区。
诊断证据：target/diagnostics/jason-los23-20261124/。


本轮最终验证与产物（2026-11-24）：

- 新源码和新配置的诊断镜像已通过 fastboot boot 临时启动；用户确认
  首页稳定、触摸正常，设置中振动正常，息屏约一分钟再唤醒触摸正常。
- 配置、bootimg、packer 合计 85 项测试通过，Android release 与主机
  packer 构建通过；git diff --check 通过。
- 最终保存 final-kernel.log、final-recovery.log、ff-haptic-final.txt。
- 常规产物保持原有 ADB 认证；诊断产物额外含本机公钥。
  两份产物中的配置和 Skyward 二进制均与工作区逐字节比对一致。
  常规产物尚未单独临时启动；当前实机使用下列诊断产物。

    target/skyward-jason-los23-lzma-fixed-20261124.img
    30838784 字节
    SHA-256: 961b4015c76eab6a494a234cdf1e4ba6ed952f3b4c57330f50dcb731d637ee44

    target/skyward-jason-los23-lzma-adb-diagnostic-20261124.img
    30838784 字节
    SHA-256: 680894f9f22e7dc4ae319069110b9468824b3bc7de5b5159c73c50c940a5477f

振动配置无需进一步修改，不应因为最初错过短振动就更改驱动参数。
这次新增的 LZMA 支持必须随源码一起保留；只把 TOML 复制到不支持该字段
的旧源码无法复现这套打包结果。不要混用旧 4.19 底包的 timed_output 配置。


## 2026-11-24：Android 13 解密修复

对照 logs/20261124-210150 的日志和实机，安装系统是 Android 13、API 33。
上一轮只确认 LOS 23.2 底包的启动、触摸和振动，旧底包的解密结果不能用于
证明此镜像也能解密。此次日志已证实该组合的原配置存在以下问题：

- 服务路径指向不存在的 system_ext/bin/hwservicemanager，实际位于 system/bin。
- 系统把 runtime 与 VNDK 放在扁平 APEX 目录，旧代码只查找 .apex 文件。
- 用 recovery 中 Android 16 linker 启动 Android 13 hwservicemanager，
  在创建线程的 __init_static_tls 处 SIGSEGV；现场寄存器及映射已保存。
- vendor 服务若先查 system/lib64，会把平台 libxml2 当作其 VNDK 版本，
  进而缺少 libandroidicu，导致 qseecomd 不能启动。
- 旧 ext4 内核对没有加密策略的目录返回 ENOENT；旧 legacy FBE 检查直接报错。

修复位置：

- crates/skyward/src/bundles.rs：保留现有压缩 APEX 查找顺序，增加带 manifest
  的扁平目录查找，以及从服务 loader/库路径提取 APEX 名称。
- crates/skyward/src/secure.rs：把所需扁平 APEX 绑定到 /apex，继承安装分区
  的只读挂载状态；记录并在刷包释放时卸载，扁平目录无需分配或分离 loop。
- crates/skyward/src/main.rs：服务启动前准备显式配置的 APEX。
- devices/jason-los23.toml：改用实机 Android 13 的硬件服务管理器路径，
  显式使用该系统的 linker；vendor 服务查找 VNDK 的顺序早于系统库。
- crates/skyward-crypto/src/fscrypt.rs：策略查询返回 ENOENT/ENODATA 时，
  在同一个已打开目录上检查 inode 加密标志。没有加密标志才接受为无策略；
  仍标记加密或标志读取失败时保留错误，不能把缺 key 的目录报成已解密。

4 项 APEX 回归测试、37 项 config 测试、64 项 crypto 测试通过。
Android release 与 packer release 构建、git diff --check 通过。

临时程序首先成功打开 data；随后通过 fastboot boot 临时启动完整新镜像，
没有继续依赖手动挂载或单独启动的服务。新镜像冷启动日志报告：

    flattened APEX .../com.android.runtime is mounted at /apex/com.android.runtime
    flattened APEX .../com.android.vndk.current is mounted at /apex/com.android.vndk.current
    /dev/block/bootdevice/by-name/userdata holds ext4 at /data
    the users whose own files are still closed: []
    file transfer offers /data/media/0

用户确认“文件、触摸和振动都正常”。本轮没有刷写 recovery、没有格式化 data。
已保存 reported-*.log、hwmanager-trace.txt、final-*.log 和 final-state.txt，
位于 target/diagnostics/jason-los23-decrypt-20261124/。

本轮产物：

    target/skyward-jason-los23-decrypt-fixed-20261124.img
    30,834,688 字节
    SHA-256: 903f595ab9ecf0a5f56e04b94c05d25a645a4890ee8eea4d2b1202a5140140fc

    target/skyward-jason-los23-decrypt-adb-diagnostic-20261124.img
    30,838,784 字节
    SHA-256: ccee72d212ffe06df2aab79f1519b9f4d39237622f4959bcfbffa0b590ac96af


诊断副本额外包含当前电脑 ADB 公钥，不用于公开分发；实际冷启动验证使用
诊断副本，常规产物尚未单独启动，但其配置和 Skyward 二进制相同。
沿用上一节构建、打包命令，输出改为上述 decrypt-fixed 镜像名。
必须重建主程序并重新打包；只复制 TOML 到旧二进制不能获得 APEX/策略修复。
这个配置的服务环境对应当前 Android 13 系统，不代表已验证其他 ROM 或锁屏布局。


## 2026-11-24：4.19 底包 + MIUI 12 / Android 10 的 data 打开失败

本次处理与 LOS 23.2 底包无关。当前 jason/recovery.img 与
D:/下载/lineage-22.2-20260620-recovery-jason-4.19.img 一致：
38,199,296 字节，SHA256
ca02dede708058bbd1c09f29d1edc8344331bffcb13ea43904441e1483f8797c。
实机内核为 4.19.325-cip132-st16-perf-gca289b26c558，已安装系统为 Android 10/API29。

用户提供的 20261124-214552 日志包含两个问题：
- 安装系统的运行库目录为 com.android.runtime.release，而 manifest 名称和
  ELF 解释器路径仍使用 com.android.runtime。原先的 APEX 搜索未识别这个变体。
- 原 jason.toml 为此前类原生系统声明了 fileencryption=ice、legacy_fbe=true。
  当前 MIUI 自己的 vendor fstab 则是 encryptable=footer。原配置导致 plain-data
  检查拒绝已有内容，又错误尝试读取不存在的 /data/unencrypted/mode。

现场证据比 fstab 标记更重要：只读挂载原始 userdata 后，可直接读取已有 media/0，
未发现 unencrypted 或 misc/vold/user_keys，policy ioctl 返回 EOPNOTSUPP；
同一个打开 inode 的 GET_FLAGS 显示 root/media/media/0 没有 ENCRYPT_FL。
因此当前安装的 userdata 是明文；encryptable=footer 仅允许旧式 FDE，不能证明已经加密。
修复没有关闭或删除系统加密，也没有新增真正的 FDE 解密功能。

新增设备配置 devices/jason-miui12.toml：
- 保留 4.19 底包的分区映射、timed_output 振动、触摸与面板息屏处理。
- data 使用匹配 MIUI 的 encryptable=footer，并关闭错误的 legacy FBE 选择。
- 只支持当前已验证的明文 data，不启动对它没有用途的 FBE HAL 链。
  中间验证版尝试运行旧 Keymaster，在按 vendor/ueventd.rc 修正 /dev/qseecom、
  /dev/ion 权限后它仍以状态 1 退出；最终配置移除这套非必需服务，不能把
  当前明文挂载成功表述为 Keymaster/FDE 已修复。
- 原 devices/jason.toml 和 devices/jason-los23.toml 本轮均未修改。
  后续更换已安装 ROM 时应选对应配置。

源码 crates/skyward/src/bundles.rs 新增 runtime.release 的回退识别；
原 canonical APEX 优先顺序保持不变，其他模块不使用这个别名。
已有 packaged 与 flattened 两种 APEX 路径都保留。本轮没有修改 crypto 解密实现。

重新编译与打包（实际仓库 /home/skyward-diting）：

    export ANDROID_NDK_HOME=/home/ndk29/android-ndk-r29
    cargo ndk -t arm64-v8a --platform 30 build --release --locked -p skyward
    cargo build --release --locked -p skyward-pack
    target/release/skyward-pack recovery \
      --stock jason/recovery.img --partition-size 67108864 \
      --binary target/aarch64-linux-android/release/skyward \
      --system-init target/system-init/aarch64-linux-android/release/system-init \
      --config devices/jason-miui12.toml \
      --font target/fonts/MiSans-jason-400.ttf \
      --mono resources/fonts/KirkMono-Regular.ttf \
      --mono-license resources/fonts/KirkMono-OFL.txt \
      --ntfs3g target/ntfs3g \
      --out target/skyward-jason-miui12-fixed-20261124.img

构建步骤和检查记录位于 target/diagnostics/jason-miui10-20261124/。

最终验证：
- APEX 查找回归测试 5 项通过；skyward-config 37 项通过。
- Android release 编译、packer release 构建和完整打包通过。
- fastboot boot 临时启动最终诊断副本后，内核仍为 4.19.325。
- CLI data 成功打开 ext4，报告 every user's own files can be read；
  /sdcard/Android、DCIM、Download 均可读取，USB 状态为 mtp,adb。
- 最终启动日志未出现 devices not brought up 或 Keymaster 退出重试。
- timed_output 振动命令成功完成；用户对最终界面/触摸/体感振动的确认另行记录，
  不能用此前 LOS 23.2 的确认代替。
- 验证过程仅临时启动镜像并进行正常挂载，没有刷写 recovery 或格式化 data。

交付镜像 target/skyward-jason-miui12-fixed-20261124.img：
44,724,224 字节，约 42.65 MiB。
SHA256 a4ca6b8f84a7c9f51ee939e34f7a0d158fd5bbba9ee7f6cfcbda5116b3db1dd9。
实机使用增加本机 ADB 公钥的诊断副本，程序、配置和分区表与交付版一致。
target/diagnostics/jason-miui10-20261124/session-miui12-fix.patch
仅包含本轮改动，不包含本仓库此前所有历史补丁。


## 2026-11-24：将 MIUI 修复合并回统一 jason.toml

4.19 底包的配置入口现在统一为 devices/jason.toml。实现仅涉及配置与说明，
本轮没有新增源码逻辑：

- data fstab 保留挂载、安全、check、quota、formattable 选项，移除写死的
  fileencryption=ice。也不写死 encryptable=footer，以免将某个安装系统的
  加密声明强加给另一套系统。
- legacy_fbe=true 保留。程序先检查是否存在加密策略和密钥目录；
  明文 data 通过检查后直接打开；旧 FBE 数据仍走现有 Keymaster 3 流程，
  从实际 /data/unencrypted/mode 读取模式。没有关闭系统加密或新增 FDE 支持。
- 使用现有 service.when 按已安装系统 ro.build.version.sdk 选择服务。
  API29：不启动旧 FBE 服务；API35：保留此前 LOS 22.2 的四项服务，
  为 hwservicemanager 显式指定匹配的 runtime loader；
  API33：沿用之前检查过的 Android 13 服务路径和 VNDK 库顺序。
- API33 的服务环境来自此前 LOS23 底包上的系统验证；
  本轮尚未将统一配置切换回 Android13/API33 或 LOS22.2/API35 做实机回归。
  不因此扩大已有的 ROM、密码和加密格式支持范围。
- 保留显示、触摸、振动等硬件设置。LOS23/4.4 配置不变。
- devices/jason-miui12.toml 保留作此前已验证的专用配置快照，
  文件头标注优先使用统一入口；旧阶段说明保留作历史记录。

本轮验证：
37 项配置测试、64 项 crypto 测试通过；
对照原配置检查 API29/33/35 分别选择 0/4/4 项服务，硬件设置保持相同。
使用 4.19 底包完成打包，并逐字节核对镜像内程序与统一配置，
确认生成的 data 分区表没有写死加密标记。未重启或刷写手机。

重新打包时，原命令使用以下配置参数即可：

    --config devices/jason.toml

本轮产物 target/skyward-jason-unified-20261124.img，44,728,320 字节。
SHA256 a1416df3774c7dbb80fd00cbaf04e3dc8d104a0885cd409a68db860c643375a7。
此统一配置镜像本轮完成构建和内容检查，尚未单独实机启动。
检查记录与本轮增量补丁在 target/diagnostics/jason-unified-20261124/。
