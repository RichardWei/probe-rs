# probe-rs-lib

`probe-rs-lib` 把本工作区的 probe-rs 能力提供给 C/C++。调试探针的连接由 `Session` 句柄持有：先打开 Session，再在该句柄上读取目标、调试、擦除或烧录，最后关闭。擦除和烧录不会自行重新扫描或连接探针。

## 构建与链接

- 本地：`cargo build -p probe-rs-lib -p probe-rs-lib-cli --release --locked`。
- Windows 打包：`powershell -ExecutionPolicy Bypass -File scripts/build-probe-rs-lib.ps1`；静态链接需要 `probe_rs_lib_static.lib` 及构建产物中的 `native-static-libs.txt` 所列依赖，动态链接需要 DLL 及其导入库。
- C/C++ 声明以 `include/probe_rs_lib.h` 为准。CLI 动态加载同版本库；不同版本的头文件、CLI 与库不可混用。

## Session 与目标选择

```c
uint64_t pr_session_open_auto(const char *chip, uint32_t speed_khz,
                              int32_t protocol_code, int32_t allow_erase_all,
                              int32_t programmer_type_code);
uint64_t pr_session_open_with_probe(const char *selector, const char *chip,
                                    uint32_t speed_khz, int32_t protocol_code,
                                    int32_t allow_erase_all, int32_t programmer_type_code);
```

`chip == NULL` 对应 `TargetSelector::Auto`；提供名称时由 probe-rs 查询内置目标数据库。`protocol_code` 为 `0`（默认）、`1`（SWD）、`2`（JTAG），`speed_khz == 0` 表示不指定速度。`programmer_type_code == 0` 不筛选探针，此时自动连接使用 probe-rs 的 `Session::auto_attach`；非零值仅筛选指定类型。`selector` 使用 probe-rs 的 `VID:PID[:SERIAL]` 格式。使用指定探针时调用 `Probe::attach`，不会改选其他设备。

`allow_erase_all` 在**创建 Session 时**设置 `Permissions::allow_erase_all()`。需要调用 `pr_session_erase_all` 或将烧录选项 `do_chip_erase` 置为真时，创建 Session 就应传入 `1`。旧的“关闭 Session → 擦除/烧录时重新连接”调用顺序已移除。

`pr_session_target_info` 从已连接的 Session 读取目标名称和数据库索引；先传 `NULL, 0` 可取得包含 NUL 的缓冲区长度，随后在**同一 Session** 上读取字符串，不会再次连接。未匹配数据库索引时输出 `UINT32_MAX`。`pr_session_close` 释放句柄。传入无效句柄返回错误，可通过 `pr_last_error` 获取当前线程的错误文本。

## 擦除

`pr_session_erase_all(session, callback, context)` 直接调用 `probe_rs::flashing::erase_all(&mut session, &mut progress, false)`。非易失区遍历、Flash Algorithm 选择、芯片级擦除与逐 Sector fallback 都由 probe-rs 决定。FFI 不切换协议、不重试、不绕过权限，也不自动关闭 Session。成功返回 `0`；失败返回非零值并保存底层错误链。

全片擦除可能只有开始和结束事件，没有可计算的百分比。调用端应按收到的实际事件显示进度。

## 烧录

`pr_session_flash(session, path, image, options, callback, context)` 直接调用 `probe_rs::flashing::download_file_with_options`。镜像格式必须显式指定：`image.format` 的 `1`、`2`、`3` 分别对应 ELF、Intel HEX、BIN。ELF 可提供 `skip_sections`；BIN 可提供 `skip` 和可选基地址。`has_base_address == 1` 时 `base_address == 0` 仍是有效输入。扩展名推断只发生在 CLI，不发生在 C API。

`options == NULL` 使用 `DownloadOptions::default()`。非空 `pr_download_options_t` 对应当前 Rust 类型的全部字段：

| C 字段 | Rust 字段 | 默认值 |
| --- | --- | --- |
| `keep_unwritten_bytes` | `keep_unwritten_bytes` | false |
| `dry_run` | `dry_run` | false |
| `do_chip_erase` | `do_chip_erase` | false |
| `skip_erase` | `skip_erase` | false |
| `preverify` | `preverify` | false |
| `verify` | `verify` | false |
| `disable_double_buffering` | `disable_double_buffering` | false |
| `preferred_algos`、`preferred_algos_len` | `preferred_algos` | 空列表 |
| `has_ram_chunk_size`、`ram_chunk_size` | `ram_chunk_size` | `None` |

例如，烧录后校验需设置 `verify = 1`；预先校验设置 `preverify = 1`；允许烧录流程使用芯片级擦除，需同时在打开 Session 时设置 `allow_erase_all = 1`，并设置 `do_chip_erase = 1`。其他未指定开关保持 Rust 默认值。

## 原始进度事件

`pr_progress_event_cb` 接收与 `FlashProgress::new` 一一对应的事件。`kind`：`1` 布局就绪、`2` 增加进度条、`3` 开始、`4` 增量、`5` 完成、`6` 失败、`7` 诊断消息；`operation`：`0` Fill、`1` Erase、`2` Program、`3` Verify、`4` Ram，缺失时为 `-1`。增加进度条时由 `has_total` 区分未知总量与 `0`；增量事件给出本次字节数和耗时纳秒。布局事件包含各区域的 Sector、Page、Fill、Data Block；Page 数据指针也可在回调期间读取。

诊断消息是 `message` 指向的 `message_len` 个 UTF-8 字节，可包含 NUL。所有事件和内层指针只在回调执行期间有效。需要异步显示时，C++ 调用端应复制所需字段。FFI 不产生百分比、ETA 或额外的成功事件。若不需要进度，传空回调即可。不要在回调内再次调用同一 Session 的操作，因为该 Session 正由当前操作使用。

## C++ 调用示例

```cpp
#include "probe_rs_lib.h"
#include <cstdint>
#include <cstdio>

static void on_progress(const pr_progress_event_t *event, void *) {
    if (event->kind == 7 && event->message)
        std::fwrite(event->message, 1, event->message_len, stdout);
}

int main() {
    uint64_t session = pr_session_open_with_probe(
        "0483:374b", "STM32H750VBTx", 2000, 1, 1, 0);
    if (!session) return 1;
    int32_t rc = pr_session_erase_all(session, on_progress, nullptr);
    pr_session_close(session);
    return rc == 0 ? 0 : 2;
}
```

烧录时在同一个 `session` 上调用 `pr_session_flash`；工作线程应持有自己的 Session 句柄，并在取出错误信息后关闭。`pr_last_error` 是线程局部字符串，失败后应立即读取。

## 其他查询语义

`pr_probe_count`、`pr_probe_info`、`pr_probe_driver_flags` 只枚举静态探针信息；不会测试协议或更改速度。要验证某设备和目标能否连接，应按所需 selector、协议和速度打开 Session，再检查返回值。`pr_core_status` 分别报告 Rust `CoreStatus` 的 Running、Halted、LockedUp、Sleeping、Unknown，以及暂停原因和断点原因；semihosting 命令的 `Debug` 文本可由附加缓冲区读取。`pr_core_count` 和 `pr_registers_count` 通过输出参数返回数量，函数返回值区分零结果与错误。

`pr_register_info` 同时返回寄存器宽度及 `RegisterDataType` 的整型或浮点类别。其余核心控制、内存、寄存器和断点接口继续直接对指定 Session/Core 调用 probe-rs；它们不会重新连接探针。芯片数据库与规格查询属于本库提供的展示接口，不等同于底层硬件探测。规格 JSON 中的 `cores`、`regions` 和 `flash_algorithms` 是数组；未提供默认格式时 `default_format` 为 `null`，不会被改成空字符串。

## 验证

无硬件检查：`cargo test --locked -p probe-rs-lib -p probe-rs-lib-cli`，并使用项目现有 C++ 冒烟测试检查头文件、导出符号及动态/静态链接。硬件验收应对照本工作区官方 probe-rs CLI，在相同探针、协议、速度和 STM32H750 目标上比较擦除范围、Sector fallback、烧录与失败后的再次连接。无硬件测试不能证明目标操作成功。
