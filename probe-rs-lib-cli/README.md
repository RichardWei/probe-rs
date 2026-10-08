# probe-rs-lib-cli

本 CLI 动态加载同版本 `probe-rs-lib`，用 C ABI 验证 C/C++ 调用路径。默认从可执行文件所在目录加载动态库；也可用 `--dll PATH` 指定。所有硬件操作先创建 Session，操作结束后关闭。`flash`、`erase-all`、`detect`、`check`、`read16`、`write16` 的 `--probe VID:PID[:SERIAL]` 都会用于实际连接；未指定则由 probe-rs 自动选探针。`--programmer-type TYPE` 是可选筛选条件，作用于本次 Session 创建，不保存进程级选择状态。

## 常用命令

```text
probe-rs-lib-cli --op list
probe-rs-lib-cli --op detect --probe 0483:374b --protocol swd --speed 2000
probe-rs-lib-cli --op check --chip STM32H750VBTx --probe 0483:374b
probe-rs-lib-cli --op erase-all --chip STM32H750VBTx --probe 0483:374b
probe-rs-lib-cli --op flash --chip STM32H750VBTx --probe 0483:374b --file firmware.hex
probe-rs-lib-cli --op chips
probe-rs-lib-cli --op spec --chip STM32H750VBTx
```

`detect` 不传 `--chip` 时使用 probe-rs 的 `TargetSelector::Auto`；`check` 也可按同样方式自动识别。`chips` 和 `spec` 只读内置数据库，不需要探针或 `--programmer-type`。`chips` 列出所有型号，不人为截断。

## 烧录选项与默认值

`flash` 对应 `pr_session_flash`，再对应 `download_file_with_options`。CLI 根据文件扩展名识别 `.elf`/`.axf`、`.hex`/`.ihex`、`.bin`；`--format elf|hex|bin` 可明确指定并覆盖扩展名。`--base` 是否提供会独立传递，因此 `--base 0` 有效。`--skip` 是 BIN 文件开头要跳过的字节数；`--skip-section NAME` 可重复，用于 ELF。

下列开关默认均为**关闭**，与本工作区 `DownloadOptions::default()` 一致。需要时显式传入：

| CLI 参数 | 传递给 probe-rs 的选项 |
| --- | --- |
| `--verify` | 烧录后校验 |
| `--preverify` | 烧录前校验以跳过已有内容 |
| `--chip-erase` | 允许烧录时尝试芯片级擦除 |
| `--keep-unwritten-bytes` | 保留未写入的扇区内容 |
| `--dry-run` | 仅准备烧录，不写入 Flash |
| `--skip-erase` | 跳过烧录阶段擦除 |
| `--disable-double-buffering` | 关闭双缓冲 |
| `--preferred-algo NAME` | 指定优先算法，可重复 |
| `--ram-chunk-size N` | 指定 RAM 分块大小 |

`--chip-erase` 使 CLI 在创建 Session 时设置 `Permissions::allow_erase_all()`，并将 `DownloadOptions.do_chip_erase` 设为真。单独的 `erase-all` 操作也在创建 Session 时设置此权限。`--verify`、`--preverify` 等开关直接传给 probe-rs，CLI 不添加重试或额外擦除。

示例：

```text
probe-rs-lib-cli --op flash --chip STM32H750VBTx --file firmware.bin --format bin --base 0x08000000 --verify
probe-rs-lib-cli --op flash --chip STM32H750VBTx --file firmware.elf --preverify --chip-erase
```

## 事件、错误和生命周期

CLI 把收到的 `FlashProgress` 原始事件显示为布局就绪、总量、开始、字节增量、完成、失败及诊断文本。芯片级擦除可能不报告中间百分比；CLI 不自行合成百分比或 ETA。烧录或擦除失败时，CLI 在当前线程读取 `pr_last_error`，然后关闭 Session。错误文本保留底层错误链。

`read16`、`write16` 使用 `--base` 指定地址、`--core` 指定核心索引；`read16` 使用 `--len` 指定 16 位字数，`write16` 用逗号分隔的 `--data` 指定数值。所有硬件操作的 `--protocol auto|swd|jtag` 和 `--speed KHZ` 都用于创建同一个 Session。要验证擦除或烧录结果，应在相同探针与目标上对照官方 probe-rs CLI；无硬件测试只覆盖参数、接口和错误路径。
