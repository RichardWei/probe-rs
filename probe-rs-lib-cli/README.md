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

`detect` 不传 `--chip` 时使用 probe-rs 的 `TargetSelector::Auto`；`check` 也可按同样方式自动识别。`chips` 和 `spec` 只读内置数据库，不需要探针或 `--programmer-type`。`chips` 列出所有型号及每个型号的规格 JSON，不人为截断。

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

## 与 C API 的对接状态

以下按当前工作区的 `probe-rs-lib/include/probe_rs_lib.h` 与 CLI 的动态加载、实际调用路径核对。范围是本库公开的 C API，不代表覆盖 probe-rs 的全部 Rust API。表中未对接项是现状记录，尚无对应 CLI 命令。

CLI 保持单次命令模式：每次进程执行一个操作，硬件操作创建 Session，操作结束后关闭并退出；没有交互界面或维持连接的后台线程。当前重复传入 `--op` 时，后面的值覆盖前面的值，不会连续执行。

### 已覆盖或内部使用

| 功能 | CLI 命令或调用路径 | C API |
| --- | --- | --- |
| 探针枚举与静态驱动信息 | `list` | `pr_probe_count`、`pr_probe_info`、`pr_probe_driver_flags` |
| Session 生命周期 | 硬件命令内部打开并关闭连接 | `pr_session_open_auto`、`pr_session_open_with_probe`、`pr_session_close` |
| 已连接目标查询 | `detect`、`check` | `pr_session_target_info` |
| 烧录与全片擦除 | `flash`、`erase-all` | `pr_session_flash`、`pr_session_erase_all` |
| 16 位内存读写 | `read16`、`write16` | `pr_read_16`、`pr_write_16` |
| 芯片数据库 | `chips`、`spec` | `pr_chip_manufacturer_count`、`pr_chip_manufacturer_name`、`pr_chip_model_count`、`pr_chip_model_name`、`pr_chip_model_specs`、`pr_chip_specs_by_name` |
| 探针类型字符串转编号 | 连接时处理 `--programmer-type` | `pr_programmer_type_from_string` |
| 错误查询 | 失败路径内部使用 | `pr_last_error` |

Session 和错误接口已在内部使用，不需要独立命令才能算作对接。烧录选项字段已传递；增量烧录比较及签名校验策略由底层流程决定。

### 尚未对接

以下 22 个公开函数尚未由 CLI 加载和调用：

| 类别 | 缺失功能 | C API |
| --- | --- | --- |
| 库信息 | 查询库版本 | `pr_version` |
| 核心控制 | 核心数量、暂停、运行、单步、复位、复位并暂停、状态及 semihosting 详情 | `pr_core_count`、`pr_core_halt`、`pr_core_run`、`pr_core_step`、`pr_core_reset`、`pr_core_reset_and_halt`、`pr_core_status` |
| 内存 | 8 位及 32 位读写 | `pr_read_8`、`pr_write_8`、`pr_read_32`、`pr_write_32` |
| 寄存器 | 数量、元数据枚举、按 ID 读写 | `pr_registers_count`、`pr_register_info`、`pr_read_reg_u64`、`pr_write_reg_u64` |
| 断点 | 可用数量、设置、清除指定断点、清除全部断点 | `pr_available_breakpoint_units`、`pr_set_hw_breakpoint`、`pr_clear_hw_breakpoint`、`pr_clear_all_hw_breakpoints` |
| 探针类型查询 | 类型编号支持性查询、编号转字符串 | `pr_programmer_type_is_supported_code`、`pr_programmer_type_to_string` |

### 部分覆盖与已知缺口

- 进度输出已支持 `Compare = 5`，保留原有编号。布局事件仅输出区域数量，尚未展示 Sector、Page、Fill、Data Block 明细及 Page 数据。
- 部分字符串查询的返回值检查不完整：`chips` 中厂商名和型号名的长度查询未检查零返回值；目标名称、芯片名称和规格等字符串的第二次读取未检查返回值。失败时可能输出空白或无效内容。
- Session 关闭接口已调用，但其返回值被忽略，关闭错误不会单独反馈。

云端构建、已有单元测试和链接冒烟测试通过，仅说明当前实现通过了对应检查；不代表上述未对接功能已经实现，也不代表硬件操作已验收。
