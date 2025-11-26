# probe-rs-lib

提供一个基于 probe-rs 的 C 兼容动态库（cdylib），用于在 C/C++ 程序中执行固件烧录。

## 构建

- Windows（MSVC）：`powershell -ExecutionPolicy Bypass -File scripts/build-probe-rs-lib.ps1`
  - 可选：`-Clean` 清理，`-Zip` 打包到 `dist\probe-rs-lib.zip`
- 输出：`dist\probe-rs-lib` 下包含 `probe_rs_lib.dll`（和导入库 `probe_rs_lib.lib`）以及 `probe_rs_lib.h`
- Linux/macOS：生成 `libprobe_rs_lib.so` 或 `libprobe_rs_lib.dylib`，脚本会复制到 `dist/probe-rs-lib`

## 集成

- 头文件：`probe_rs_lib.h`
- 链接：
  - Windows（静态链接到导入库）：添加 `probe_rs_lib.lib`；运行时需 `probe_rs_lib.dll`
  - 或使用 `LoadLibrary/GetProcAddress` 仅依赖 `dll`
- 依赖：需要系统识别你的调试探针（CMSIS‑DAP/JLink/STLink/FTDI/ESP‑USB‑JTAG/WLink 等）

## API

- 错误与版本：`pr_last_error`、`pr_version`
- 探针枚举：`pr_probe_count`、`pr_probe_info`、`pr_probe_features`、`pr_probe_check_target`
- 会话管理：`pr_session_open_auto`、`pr_session_open_with_probe`、`pr_session_close`、`pr_core_count`
- 调试控制：`pr_core_halt`、`pr_core_run`、`pr_core_step`、`pr_core_reset`、`pr_core_reset_and_halt`、`pr_core_status`
- 内存读写：`pr_read_8`、`pr_write_8`、`pr_read_32`、`pr_write_32`
- 寄存器访问：`pr_registers_count`、`pr_register_info`、`pr_read_reg_u64`、`pr_write_reg_u64`
- 断点：`pr_available_breakpoint_units`、`pr_set_hw_breakpoint`、`pr_clear_hw_breakpoint`、`pr_clear_all_hw_breakpoints`
- 烧录：`pr_flash_elf`、`pr_flash_hex`、`pr_flash_bin`、`pr_flash_auto`

### 芯片枚举与探测（Chip Listing & Detection）

- 枚举 API（基于整数索引，适合 C 调用）：
  - `pr_chip_manufacturer_count()`：返回支持的制造商数量
  - `pr_chip_manufacturer_name(index, buf, buf_len)`：按索引返回制造商名称（UTF‑8）。当 `buf==NULL` 或 `buf_len==0` 时返回所需长度（包含 NUL）
  - `pr_chip_model_count(manu_index)`：返回该制造商下的芯片型号数量
  - `pr_chip_model_name(manu_index, chip_index, buf, buf_len)`：返回对应芯片型号名称（UTF‑8）
  - `pr_chip_model_specs(manu_index, chip_index, buf, buf_len)`：返回 JSON 格式的详细规格信息（架构、核心、内存区域、闪存算法等）
  - `pr_chip_specs_by_name(name, buf, buf_len)`：按芯片名返回 JSON 规格
- 探测 API：
  - `pr_probe_detect_target_info(probe_index, &out_manu_index, &out_chip_index, name_buf, name_buf_len)`：尝试通过已设置的编程器类型附着并识别目标芯片；成功后返回芯片名，并尽可能给出制造商与型号索引；失败时返回 `<=0` 并可用 `pr_last_error()` 读取错误
- 设计说明：
  - 制造商信息来源于 JEP106（JEDEC）编码；`Registry::from_builtin_families()` 提供内置目标数据库
  - 所有字符串均为 UTF‑8；C 调用可按需两段式分配（先请求长度、再写入）
  - 错误统一通过 `pr_last_error()` 返回英文提示，便于日志与国际化

### 进度回调（Progress Callback）

- 用于在擦除/烧录/校验阶段上报英文状态、百分比与 ETA（毫秒）
- 函数：
  - `void pr_set_progress_callback(pr_progress_cb cb);`
  - `void pr_clear_progress_callback(void);`
- 回调签名：`typedef void (*pr_progress_cb)(int32_t operation, float percent, const char* status, int32_t eta_ms);`
  - `operation`：1=Erase，2=Program，3=Verify，0=Fill
  - `percent`：0.0..100.0（可能为稀疏事件，客户端可平滑显示）
  - `status`：英文状态字符串（如 `"erasing"`、`"programming"`、`"verifying"`）
  - `eta_ms`：剩余时间估计，未知时为 `-1`

- 行为说明（擦除阶段）：当底层未提供擦除阶段的细粒度进度事件时，库不再模拟中间进度，仅在开始上报 `0%`，结束上报 `100%`；CLI 显示将直接从 `0%` 跳到 `100%`。


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
-- 使用要求：
  - 在执行会话建立与烧录前必须调用 `pr_set_programmer_type_code` 明确指定类型
  - 自动选择探针时，会按已设置的类型过滤匹配的探针；未设置类型时保持向后兼容（按旧逻辑自动检测）
  - 指定探针打开时（`pr_session_open_with_probe`），库将验证该探针类型与当前配置是否一致

### 自动文件格式检测（Auto Format Detection）

- 新增 API：`pr_flash_auto(const char* chip, const char* path, uint64_t base_address, uint32_t skip, int32_t verify, int32_t preverify, int32_t chip_erase, uint32_t speed_khz, int32_t protocol_code)`
- 检测规则：
  - `.elf`/`.axf` => ELF
  - `.hex`/`.ihex` => Intel HEX
  - `.bin` => 二进制（需要 `base_address`，否则报错）
- CLI 已移除 `--format` 选项，直接根据文件扩展名自动选择格式；`--base` 仅在 `.bin` 时使用

`protocol_code`：1=SWD，2=JTAG，其他/0=不指定（自动）

## 已知限制

- Windows 构建 `cdylib` 会同时产生 `.dll` 与 `.lib` 导入库
- 某些探针需要额外驱动或权限（Linux 需 udev 配置）
- `bin` 烧录需提供正确的起始地址与对齐
- 进度事件由底层驱动与目标决定；当擦除阶段缺少细粒度事件时，库与 CLI 均仅在开始与结束上报进度（不进行模拟中间进度）
- 所有日志与错误输出（库与 CLI）统一为英文，便于国际化集成

## 使用示例

```c
// Halt, read memory, set breakpoint, run
#include "probe_rs_lib.h"
#include <stdio.h>

int main() {
  uint64_t sess = pr_session_open_auto("nRF52840_xxAA", 4000, 1);
  if (sess == 0) {
    size_t need = pr_last_error(NULL, 0);
    char *buf = (char*)malloc(need);
    pr_last_error(buf, need);
    fprintf(stderr, "open failed: %s\n", buf);
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

以下命令使用 `probe-rs-lib-cli` 对 HEX 文件烧录，自动格式检测，编程器类型为 CMSIS‑DAP：

```
cargo run -p probe-rs-lib-cli -- --op flash --chip stm32f407zet6 --protocol swd --speed 4000 --file "c:\Users\weizh\Desktop\probe-rs\iap_debug.hex" --programmer-type cmsis-dap
```

将 `.bin` 文件烧录时需要提供基地址：

```
cargo run -p probe-rs-lib-cli -- --op flash --chip <chip> --file firmware.bin --base 0x08000000 --programmer-type stlink
```

枚举支持的制造商与芯片型号：

```
cargo run -p probe-rs-lib-cli -- --op chips --programmer-type cmsis-dap
```

识别连接的目标芯片：

```
cargo run -p probe-rs-lib-cli -- --op detect --programmer-type stlink
```

按名称查询芯片详细规格（JSON）：

```
cargo run -p probe-rs-lib-cli -- --op spec --chip nrf51822_Xxaa --programmer-type cmsis-dap
```

## 测试

- 包含无硬件的单元测试（错误处理与版本号）
- 硬件相关集成测试需要真实探针与目标设备，建议在 CI 外部环境执行

## 兼容性

- Rust Edition 2024：导出函数使用 `#[unsafe(no_mangle)]`
- 适配 `probe-rs 0.30.0`，支持 ARM/RISC‑V/Xtensa 与多种探针
- 本库版本随工作区同步：`0.30.0`

## 变更日志（Changelog）

- 0.30.0
  - 新增：自动文件格式检测 `pr_flash_auto`
  - 新增：芯片枚举与探测 API（制造商/型号列表、规格查询、目标识别）
  - 新增：编程器类型 API（设置/校验/获取）
  - 新增：进度回调 API（状态/百分比/ETA，英文）
  - CLI：移除 `--format`，严格要求 `--programmer-type`，日志统一英文
  - 兼容性：保留 `pr_flash_elf/pr_flash_hex/pr_flash_bin` 旧接口以兼容现有调用
  - 调整：擦除阶段在无底层进度事件时不再模拟中间进度，仅 `0%`→`100%`
