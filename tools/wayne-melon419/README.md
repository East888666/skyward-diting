# Wayne / Melon 4.19 旧版 ICE 兼容补丁

状态：代码候选，尚未编译和实机验证。用户要求本轮不编译，因此没有生成
新内核、打包镜像或重启设备。不能将本补丁视为已经完成解密修复。

补丁基于 `clarencelol/kernel_xiaomi_sdm660-4.19` 的 `melon` 分支，固定提交
`73df42e5d695f16ea7445ab4d493044ffae2997a`。仓库 Makefile 为 4.19.243，
但仓库的 wayne_defconfig 与现有 recovery 内嵌配置不同，不能证明两者的
全部构建输入相同。后续应以现有 recovery 提取的配置及原 DTB 为基线。

## 本次补丁处理什么

`../wayne-419-legacy-ice.patch` 应用于上述内核源码，不是 Skyward Rust 源码。

- 新增默认关闭的 `CONFIG_QTI_CRYPTO_TZ_SDM660_LEGACY_ICE`，仅允许 SDM660
  eMMC/CQHCI、禁止同时启用 QTI UFS 路径，并选择旧版 PFK 派生。
- TrustZone set-key 改用 SDM660 旧接口 `0x2`，分别传入 32 字节 key、
  32 字节 salt 的物理地址；invalidate 使用 `0x3`。保留原密钥字节顺序，
  避免新接口的字节变换。与 Polaris UFS 的 `0x4` 补丁不同。
- 密钥编程、移除和请求描述符统一将逻辑槽位映射到硬件槽位 2–31，
  限制可用槽位数量，保留 0、1。拒绝 wrapped key 和非 64 字节 XTS 密钥。
- 启用 PRIVATE-mode 每文件 AES 派生，保留已有 ext4 的物理扇区 DUN
  路径；共享缓冲区和临时密钥在旧调用后清理。
- 新选项关闭时保留原现代接口；不更改默认 wayne_defconfig，也不修改
  其他机型的 Skyward 配置。

参考旧驱动为 LineageOS `android_kernel_xiaomi_sdm660` 的 `lineage-18.1`：
`security/pfe/pfk_ice.c`、`fs/ext4/crypto_key.c`。这套接口及派生与旧数据
布局对应；时钟、TrustZone 接受情况及最终明文必须由后续实机测试确认。

## 后续应用方式（本轮没有执行编译）

在固定提交的干净内核目录中，先检查再应用：

```sh
git apply --check /home/skyward-diting/tools/wayne-419-legacy-ice.patch
git apply /home/skyward-diting/tools/wayne-419-legacy-ice.patch
```

补丁包含 `arch/arm64/configs/wayne_skyward_legacyice.config` 配置片段。
将它合入从现有 Melon recovery 提取的配置后，仍需实际编译新内核才能生效。
只重新编译 Skyward 或复制 TOML 无法替换已编译在旧内核中的驱动。
未提供未经验证的完整内核构建命令；编译器与原配置兼容性仍需检查。

## 独立的 RPMB 用户态兼容层

`rpmb-compat.c`、`rpmb-compat.h`、`rpmb-compat-test.c` 从此前已测试的诊断
目录保存到这里，以免只留在被 Git 忽略的 target 目录中。

这部分此前已在原 Melon 内核上验证公共密钥访问成功，但没有完成文件解密。
它仅在 qseecomd 中显式预加载：旧 ioctl 被 RPMB 字符设备拒绝后，转换为
现代 multi-command ioctl，保留可靠写位、数据指针、命令顺序和响应。
没有添加密钥配置、重置或擦除 RPMB 的操作。

生成独立的实验配置（此操作不编译）：

```sh
python3 tools/wayne-melon419/make-config.py \
  --out target/wayne-melon419-legacyice.toml
```

生成器保留 LOS 18.1 的版本属性、服务条件、账户及权限，启用现有只读
`legacy_fbe_auto` 选择，切换为 Melon 振动接口，并且只在 qseecomd 中加载
`/skyward/libwayne-rpmb-compat.so`。该库必须另行编译并嵌入 ramdisk；
TOML 不会自动携带它。原诊断注入工具源码保留在
`target/diagnostics/wayne-melon419-compat/image-tool/`，本轮没有编译或调用它。

本实验配置只针对已检查的 LOS 18.1 vendor，不代表 MIUI、其他 ROM 或
其他设备已经兼容。正式 `devices/wayne.toml` 继续对应已验证的 4.4 方案。

## 检查与待验证项

本轮只做静态检查：补丁在固定源码上应用/反向应用、文件内容一致性、
内核 checkpatch、生成配置的 TOML 解析与服务/权限保留检查。
没有运行 C/Rust/内核编译，也没有设备写入或启动验证。

上述检查已通过。checkpatch 为 0 errors、0 checks；唯一 warning 是新增
配置片段触发的 MAINTAINERS 登记提示。补丁 SHA256 为
`ac521e050abeb6de09096cc9ba78c2700ebb38e6ba656d99f30cee32d2175c93`。
实验配置已生成到 `target/wayne-melon419-legacyice.toml`，仅是后续打包输入，
不能直接使现有原版 Melon 内核具备本补丁的行为。

后续实机需要依次确认：公共密钥访问、旧 SMC 密钥配置、只读 DE key
认证、手机上输入密码后的 CE 文件读取，以及触摸、振动和休眠唤醒。
在这些项目通过前，不替换已验证的 4.4 正式方案。
