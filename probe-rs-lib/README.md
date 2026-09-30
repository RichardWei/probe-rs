# probe-rs-lib

提供基于 probe-rs 的 C ABI 动态库（`cdylib`）和静态库（`staticlib`），供 C/C++ 程序枚举探针和芯片、连接目标、控制核心、读写内存与寄存器、管理硬件断点，以及擦除和烧录固件。配套的 `probe-rs-lib-cli` 仍通过动态加载该库提供命令行入口。

## 构建

- Windows（MSVC）：`powershell -ExecutionPolicy Bypass -File scripts/build-probe-rs-lib.ps1`
  - 可选：`-Clean` 清理，`-Zip` 打包到 `dist\probe-rs-lib.zip`
- 本地通用命令：`cargo build -p probe-rs-lib -p probe-rs-lib-cli --release --locked`。CLI 从自身所在目录加载动态库。
- Windows 脚本只构建库的 debug/release 版本，输出到 `dist\probe-rs-lib`；它**不构建 CLI**。macOS 可用上面的 Cargo 命令，输出 `target/release/libprobe_rs_lib.dylib` 和 `target/release/libprobe_rs_lib.a`。
- `develop-cpp-fii` 分支的 GitHub Actions 使用 `cargo build --workspace --release --locked` 构建 Windows x64 和 Intel macOS x64。两个平台构建、测试和动态/静态 C++ 冒烟测试均通过后，工作流将 `cpp-*`、`tools-*` 和 SHA-256 校验文件发布到 GitHub Release；构建日志另存为 Actions 日志产物。`cpp-*` 解包后包含 `include/` 头文件、`bin/` 动态库及 CLI，以及 `lib/` 中的静态库；Windows `lib/` 还包含 DLL 导入库、`native/` 下的 Windows Rust 依赖导入库及 `native-static-libs.txt`。macOS 使用 tar.gz 保留包内可执行权限。

## 集成

- 头文件：`probe_rs_lib.h`
- 链接：
  - Windows（通过导入库链接动态库）：链接 `probe_rs_lib.lib`；运行时仍需 `probe_rs_lib.dll`
  - Windows（静态）：C++ 代码使用 MSVC `/MD`（与本库静态链接参数中的 `/defaultlib:msvcrt` 一致），链接 `probe_rs_lib_static.lib` 并补充 `lib/native-static-libs.txt` 列出的原生库；其中 `windows.*.lib` 在下载包的 `lib/native/` 目录中，可为该目录添加 `/LIBPATH`。其他系统库由 MSVC/Windows SDK 提供；无需本项目的 `probe_rs_lib.dll`
  - 或使用 `LoadLibrary/GetProcAddress` 仅依赖 `dll`
  - macOS：链接 `libprobe_rs_lib.dylib`，运行时将动态库放在可执行文件旁并设置相应的 rpath（例如 `@loader_path`）
  - macOS（静态）：链接 `libprobe_rs_lib.a`，并补充 Rust 编译器列出的系统库及 framework；无需本项目的 `libprobe_rs_lib.dylib`
- 查询静态链接所需原生库：`cargo rustc -p probe-rs-lib --release --locked --lib -- --print native-static-libs`。静态库包含 Rust 代码及其 Rust 依赖，但不保证第三方原生库或系统组件也静态链接；上述列表随平台和依赖版本变化，请以目标平台的输出为准。
- 依赖：需要系统识别你的调试探针（CMSIS‑DAP/JLink/STLink/FTDI/ESP‑USB‑JTAG/WLink 等）

## API

- 错误与版本：`pr_last_error`、`pr_version`
- 探针枚举与目标识别：`pr_probe_count`、`pr_probe_info`、`pr_probe_features`、`pr_probe_check_target`、`pr_probe_detect_target_info`
- 会话管理：`pr_session_open_auto`、`pr_session_open_with_probe`、`pr_session_close`、`pr_core_count`
- 调试控制：`pr_core_halt`、`pr_core_run`、`pr_core_step`、`pr_core_reset`、`pr_core_reset_and_halt`、`pr_core_status`
- 内存读写：`pr_read_8/16/32`、`pr_write_8/16/32`；长度参数分别表示字节、16 位字和 32 位字的个数
- 寄存器访问：`pr_registers_count`、`pr_register_info`、`pr_read_reg_u64`、`pr_write_reg_u64`
- 断点：`pr_available_breakpoint_units`、`pr_set_hw_breakpoint`、`pr_clear_hw_breakpoint`、`pr_clear_all_hw_breakpoints`
- 擦除与烧录：`pr_chip_erase`（整片擦除）、`pr_flash_elf`、`pr_flash_hex`、`pr_flash_bin`、`pr_flash_auto`

返回值并不统一：会话打开成功返回非零句柄，失败返回 `0`；普通控制和读写接口通常以 `0` 表示成功；烧录接口以 `0` 表示成功、`1` 表示参数或连接失败、`2` 表示下载失败；`pr_chip_erase` 失败返回 `-1`。`pr_probe_check_target` 返回 `1` 表示可连接、`0` 表示未能连接、`-1` 表示探针索引或打开失败。不要把所有非零返回值都当作成功；失败后读取 `pr_last_error`。

### 芯片枚举与探测（Chip Listing & Detection）

- 枚举 API（基于整数索引，适合 C 调用）：
  - `pr_chip_manufacturer_count()`：返回支持的制造商数量
  - `pr_chip_manufacturer_name(index, buf, buf_len)`：按索引返回制造商名称（UTF‑8）。当 `buf==NULL` 或 `buf_len==0` 时返回所需长度（包含 NUL）
  - `pr_chip_model_count(manu_index)`：返回该制造商下的芯片型号数量
  - `pr_chip_model_name(manu_index, chip_index, buf, buf_len)`：返回对应芯片型号名称（UTF‑8）
  - `pr_chip_model_specs(manu_index, chip_index, buf, buf_len)`：返回 JSON 规格，字段包括 `manufacturer`、`chip`、`architecture`、`cores`、`ram_bytes`、`nvm_bytes`、`regions`、`flash_algorithms`、`default_format`；其中 `cores`、`regions`、`flash_algorithms` 当前是拼接后的字符串，不是 JSON 数组
  - `pr_chip_specs_by_name(name, buf, buf_len)`：按芯片名返回 JSON 规格
- 探测 API：
  - `pr_probe_detect_target_info(probe_index, &out_manu_index, &out_chip_index, name_buf, name_buf_len)`：打开指定索引的探针并使用底层自动识别；返回芯片名所需的字节数（含 NUL，允许先传 `NULL, 0` 查询），失败返回 `0` 并可用 `pr_last_error()` 读取错误。未能映射到枚举数据库的索引为 `UINT32_MAX`。查询长度和读取名称是两次独立调用，均会重新打开并连接探针；探针顺序或目标状态改变时，应重新核对结果。
- 设计说明：
  - 制造商分组来自内置目标族的 JEP106（JEDEC）编码；缺少编码的目标归入 `Generic`，不代表支持所有通用芯片。`Registry::from_builtin_families()` 提供内置目标数据库。
  - 所有字符串均为 UTF‑8；C 调用可按需两段式分配（先请求长度、再写入）
  - 多数失败可通过 `pr_last_error()` 读取英文提示。错误字符串是进程内共享的最近一次错误，不是线程局部的，也不会在每次成功调用后自动清空；返回 `0` 的计数函数可能同时表示“没有结果”或“出错”。
  - `pr_probe_features` 会打开探针并尝试切换 SWD/JTAG、设置 1000 kHz；它报告的是这些尝试是否成功，并非纯静态能力查询。探针枚举索引来自当次列表，设备插拔后可能变化。

### 进度回调（Progress Callback）

- 用于在擦除/烧录/校验阶段上报英文状态、百分比与 ETA（毫秒）
- 函数：
  - `void pr_set_progress_callback(pr_progress_cb cb);`
  - `void pr_clear_progress_callback(void);`
- 回调签名：`typedef void (*pr_progress_cb)(int32_t operation, float percent, const char* status, int32_t eta_ms);`
  - `operation`：0=Fill，1=Erase，2=Program，3=Verify，4=Ram（写入 RAM）
  - `percent`：0.0..100.0（可能为稀疏事件，客户端可平滑显示）
  - `status`：英文状态字符串（如 `"erasing"`、`"programming"`、`"verifying"`）
  - `eta_ms`：剩余时间估计，未知时为 `-1`

- 回调仅由烧录接口安装的 `FlashProgress` 触发；`pr_chip_erase` 当前不向此回调报告进度。库只保存一个进程级回调，调用方应在不再使用前清除它，且不能在回调返回后继续使用 `status` 指针。
- 擦除阶段若底层没有细粒度事件，库不模拟中间进度，可能只报告开始 `0%` 和结束 `100%`。


### 烧录器类型（Programmer Type）

- 枚举 API：
  - `pr_set_programmer_type_code(int32_t type_code)`：设置当前烧录器类型（返回 0 表示成功，否则失败）
  - `pr_get_programmer_type_code(void)`：获取当前配置类型的枚举编码（返回 -1 表示未设置）
  - `pr_programmer_type_is_supported_code(int32_t type_code)`：验证枚举编码是否受支持（返回 1/0）
- 字符串转换（仅用于 UI 显示或解析）：
  - `pr_programmer_type_to_string(int32_t type_code, char* buf, size_t buf_len)`：枚举编码转字符串
  - `pr_programmer_type_from_string(const char* type_name, int32_t* out_code)`：字符串解析为枚举编码
- 支持的类型（不区分大小写）：
  - `cmsis-dap`
  - `stlink`
  - `jlink`
  - `ftdi`
  - `esp-usb-jtag`
  - `wch-link`
  - `sifli-uart`
  - `glasgow`
  - `ch347-usb-jtag`
- 使用要求：
  - **C API 不强制设置类型**：未调用 `pr_set_programmer_type_code` 时，自动会话使用 probe-rs 原有的自动探针选择；设置后，自动会话挑选第一个匹配类型的探针。
  - `pr_session_open_with_probe` 和 `pr_probe_detect_target_info` 在已设置类型时会校验探针类型。该设置是进程级状态，当前没有清空/恢复“未设置”的 API。
  - **CLI 另有约束**：除 `--op list` 外，CLI 要求传入 `--programmer-type`，包括不需要硬件的 `chips` 和 `spec` 操作。

### 自动文件格式检测（Auto Format Detection）

- 新增 API：`pr_flash_auto(const char* chip, const char* path, uint64_t base_address, uint32_t skip, int32_t verify, int32_t preverify, int32_t chip_erase, uint32_t speed_khz, int32_t protocol_code)`
- 检测规则：
  - `.elf`/`.axf` => ELF
  - `.hex`/`.ihex` => Intel HEX
  - `.bin` => 二进制（需要 `base_address`，否则报错）
- CLI 的 `--op flash` 根据文件扩展名自动选择格式。旧 `--format` 参数仍被解析，但其值被忽略，不会覆盖自动检测；`--base` 对 `.bin` 是必需的且不能为 `0`。直接调用 `pr_flash_bin` 时可传入 `0` 作为基地址。

`protocol_code`：1=SWD，2=JTAG，其他/0=不指定（自动）

## 已知限制

- Windows 构建同时产生 `.dll`、原始导入库 `probe_rs_lib.dll.lib` 和静态库 `probe_rs_lib.lib`；打包时导入库重命名为 `probe_rs_lib.lib`，静态库重命名为 `probe_rs_lib_static.lib`，避免混淆
- 某些探针需要额外驱动或权限（Linux 需 udev 配置）
- `pr_flash_*` 和 `pr_chip_erase` 每次都会自己选择探针并新建会话，不使用先前通过 `pr_session_open_with_probe` 打开的会话。CLI 的 `--probe` 只用于 `detect`（数字索引）和 `check`/`read16`/`write16`（探针选择器），**不用于** `flash`/`erase-all`；多探针场景应谨慎操作。
- `pr_flash_auto` 的 `.bin` 文件必须提供非零基地址；二进制写入还需确保地址、`skip` 和目标内存布局正确。显式 `pr_flash_bin` 不会拒绝零基地址。
- `pr_probe_check_target` 只是尝试 SWD/JTAG 连接，成功不等于已经识别出具体芯片；要获取芯片名称使用 `pr_probe_detect_target_info`。
- 进度事件取决于底层驱动与目标；`pr_chip_erase` 不使用已注册的进度回调。FFI 错误消息大多为英文，但底层错误文本不保证固定格式。

## 使用示例

```c
// 示例需要真实探针和目标；地址应按所用芯片的内存映射调整。
#include "probe_rs_lib.h"
#include <stdio.h>
#include <stdlib.h>

int main() {
  uint64_t sess = pr_session_open_auto("nRF52840_xxAA", 4000, 1);
  if (sess == 0) {
    size_t need = pr_last_error(NULL, 0);
    char *buf = (char*)malloc(need);
    if (buf == NULL) return 2;
    pr_last_error(buf, need);
    fprintf(stderr, "open failed: %s\n", buf);
    free(buf);
    return 1;
  }
  // Halt core 0
  pr_core_halt(sess, 0, 100);
  // Read 32-bit buffer
  uint32_t data[16];
  pr_read_32(sess, 0, 0x20000000ULL, data, 16);
  // Set breakpoint and run
  pr_set_hw_breakpoint(sess, 0, 0x00001000ULL);
  pr_core_run(sess, 0);
  pr_session_close(sess);
  return 0;
}
```

### CLI 使用示例（可选）

使用 `cargo run` 前，先执行 `cargo build -p probe-rs-lib`，使 CLI 所在的 `target/debug` 目录包含动态库。下载的 `cpp-*` 产物已经将 CLI 和动态库放在同一目录。

以下命令使用 `probe-rs-lib-cli` 对 HEX 文件烧录，自动格式检测，编程器类型为 CMSIS‑DAP：

```
cargo run -p probe-rs-lib-cli -- --op flash --chip stm32f407zet6 --protocol swd --speed 4000 --file firmware.hex --programmer-type cmsis-dap
```

将 `.bin` 文件烧录时需要提供基地址：

```
cargo run -p probe-rs-lib-cli -- --op flash --chip <chip> --file firmware.bin --base 0x08000000 --programmer-type stlink
```

枚举支持的制造商与芯片型号（CLI 每个制造商最多显示前 50 个型号；C API 可枚举全部）：

```
cargo run -p probe-rs-lib-cli -- --op chips --programmer-type cmsis-dap
```

识别连接的目标芯片：

```
cargo run -p probe-rs-lib-cli -- --op detect --programmer-type stlink
```

`detect` 的 `--probe` 接受 `list` 输出中的数字索引，默认 `0`；`check`/`read16`/`write16` 的 `--probe` 则接受 `VID:PID[:SERIAL]` 形式的选择器。`flash`/`erase-all` 不使用该选项。

按名称查询芯片详细规格（JSON）：

```
cargo run -p probe-rs-lib-cli -- --op spec --chip nrf51822_Xxaa --programmer-type cmsis-dap
```

## 测试

- 无硬件测试覆盖版本字符串、格式识别、部分芯片数据库查询、CLI 参数解析和 C++ 动态/静态 ABI 链接冒烟检查；GitHub Actions 还会运行库与 CLI 的 Rust 测试。
- 这些测试**不验证真实探针连接、目标自动识别、擦除、烧录或调试操作**。这些行为需要在目标探针和芯片上单独验证。

## 兼容性

- Rust Edition 2024：导出函数使用 `#[unsafe(no_mangle)]`
- 依托当前工作区的 `probe-rs 0.32.0` 目标数据库与探针驱动；可用功能取决于具体探针、芯片和底层驱动，不能保证每种组合都支持上述所有操作。
- 本库版本随工作区同步：`0.32.0`
