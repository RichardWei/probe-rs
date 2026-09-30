# probe-rs 项目概览

- 目标与定位：一个用 Rust 开发的现代嵌入式调试工具集，既提供库（与 MCU、探针交互），也提供工具（CLI、DAP 调试器、GDB 服务器、cargo 集成）。
- 核心抽象：`Probe` → `Session` → `Core` → `Target` 四层模型，覆盖 ARM、RISC‑V、Xtensa 架构与多类探针。
- 适用场景：芯片识别与连接、烧录、基本调试（断点/步进/寄存器与内存访问）、追踪（SWO/ITM/TPIU/RTT）、与 IDE/GDB 的集成。

## 目录结构

- 根目录
  - `Cargo.toml` 工作区与版本；`Cargo.lock` 锁定依赖；`dist-workspace.toml` 分发配置；`deny.toml` 许可证/安全检查；`Cross.toml` 交叉编译
  - `.cargo/config.toml`：别名命令（如 `probe-rs`、`xtask`、`target-gen`）
  - `.github/workflows/*`：CI（check/test/fmt/clippy/cargo-deny/doc、release）
  - 文档：`README.md`、`CHANGELOG.md`、`RESOURCES.md`、`CONTRIBUTING.md`、`doc/*`
- `probe-rs/`（核心库）
  - `src/architecture/arm|riscv|xtensa`：各架构通信接口、核心寄存器、序列与组件
  - `src/config`：目标模型与注册表（内置 targets 在 build 时注入）
  - `src/flashing`：下载/擦除/验证、算法装载与布局
  - `src/probe`：探针驱动（CMSIS‑DAP、JLink、STLink、FTDI、ESP‑USB‑JTAG、WLink、Blackmagic…）
  - `src/vendor`：供应商支持与自动识别（Nordic、NXP、Espressif、ST 等）
  - `src/session.rs`：会话管理（多核、trace、断点）
- `probe-rs-tools/`（CLI 工具）
  - `src/bin/probe-rs/main.rs`：子命令入口（`download/run/erase/info/list/dap_server/gdb_server` 等）
  - `cmd/dap_server`：DAP 协议、调试服务器与适配
  - `cmd/gdb_server`：GDB stub 与目标适配
  - `cmd/cargo_flash`、`cmd/cargo_embed`：Cargo 子命令集成
- 其他 crates
  - `probe-rs-debug/`：DWARF/栈回溯/变量缓存/异常处理等调试帮助库
  - `probe-rs-target/`：YAML target schema 与序列化
  - `rtthost/`：基于 RTT 的简易主机工具
  - `target-gen/`：从 CMSIS Pack 生成 target 数据与算法
  - `smoke-tester/`：连通性与步进冒烟测试

## 技术栈与依赖

- 语言与版本：Rust 2024，工作区版本 0.30.0（`Cargo.toml`:1-4）
- 核心依赖（库层）
  - 二进制/镜像解析：`object`、`ihex`、`uf2-decode`
  - USB/HID：`nusb`、`hidapi`（`cmsisdap_v1` 特性）
  - 序列化：`serde`/`serde_yaml`、`bincode v2`
  - 并发与异步：`parking_lot`、`async-io`、`futures-lite`
  - 日志：`tracing`
  - 芯片描述：`probe-rs-target`
  - ESP 格式：`espflash`
- 特性开关：默认 `builtin-targets`（内置 targets）、`cmsisdap_v1`（启用 HID）

## 构建与运行

- 基本命令
  - 检查与测试：`cargo check --all-features --locked`；`cargo test --all-features --locked`
  - 格式与静态检查：`cargo fmt --all -- --check`；`cargo clippy --all-targets -- -D warnings`
  - 许可证与安全：`cargo deny check`
- 别名（`.cargo/config.toml`）
  - `cargo probe-rs ...` 运行 CLI 二进制（支持 `--features remote`）
  - `cargo xtask ...`、`cargo target-gen ...`

## 架构与流程

- 目标模型与注册表
  - 内置 targets 在构建期注入（`probe-rs/src/config/registry.rs:124-135`），运行时通过 `Registry::from_builtin_families` 查询（`probe-rs/src/config/registry.rs:144`）。
  - `Target` 包含内存图、闪存算法、调试序列、JTAG 扫描链等（`probe-rs/src/config/target.rs:21-48`）。
- 核心抽象与职责
  - `Probe`：统一抽象探针，选择协议/速度、桥接具体驱动（`probe-rs/src/probe.rs:317-321`、`select_protocol` 见 `probe-rs/src/probe.rs:435-441`）。
  - `Session`：连接态上下文，管理多核接口/trace/断点（`probe-rs/src/session.rs:29-53`）。
  - `Core`：具体内核访问能力（导出接口见 `probe-rs/src/lib.rs:95-103`）。
  - `Target`：芯片静态信息与调试序列（`probe-rs/src/config/target.rs:64-143`）。
- 连接流程
  - 自动连接：`Session::auto_attach`（`probe-rs/src/session.rs:466-472`）→ 打开首个探针、可选设置速度与协议 → `Probe::attach(...)`（`probe-rs/src/probe.rs:328-336`）→ `Session::new(...)`（`probe-rs/src/session.rs:153-185`）。
  - ARM UnderReset/Normal attach：`attach_arm_debug_interface`（`probe-rs/src/session.rs:187-329`），流程图见 `doc/attach_flow_arm.md`。
  - RISC‑V/Xtensa via JTAG：`attach_jtag`（`probe-rs/src/session.rs:331-436`）。
  - 目标自动识别：`vendor::auto_determine_target`（`probe-rs/src/vendor/mod.rs:280-325`）按架构顺序尝试，供应商钩子辅助。
- 闪存与验证
  - 高层：`flashing::download_file`（`probe-rs/src/flashing/download.rs:259-264`）。
  - Loader 构建与镜像解析：`build_loader`（`probe-rs/src/flashing/download.rs:237-251`）；ELF 提取 `extract_from_elf`（`393-412`）。
  - 编程提交：`FlashLoader::commit`（由 `download_file_with_options` 调用，`probe-rs/src/flashing/download.rs:277-282`）。

## 关键模块与交互

- 探针枚举与选择
  - `Lister::list_all`/`open()`：`probe-rs/src/probe/list.rs:36-43`；`probe-rs/src/probe.rs:851-856`
  - VID:PID:SN 精选：`DebugProbeSelector`（`probe-rs/src/probe.rs:948-966`）
- 会话与多核
  - 懒加载 attach：`Session::core(idx)`（`probe-rs/src/session.rs:551-570`）；列出：`list_cores`（`490-493`）
- Trace/SWO/TPIU/RTT
  - SWO/TPIU 配置：`setup_tracing`（`probe-rs/src/session.rs:779-816`）
  - RTT 工具示例：`rtthost/src/main.rs:162-176`（attach），循环读写：`rtthost/src/main.rs:202-243`

## 测试与 CI

- 单元与快照：`probe-rs-debug`、`probe-rs-tools` DAP/GDB 模块均有快照与用例
- 冒烟测试：`smoke-tester`
- CI：`.github/workflows/ci.yml`
  - 多平台 `check/test`；`fmt/clippy/cargo-deny/doc`；Windows 启用 `VCPKGRS_DYNAMIC=1`

## 典型使用

- 列出探针：`cargo probe-rs list`
- 自动连接与下载：`cargo probe-rs download --chip <target> <firmware.elf>`
- 运行 DAP 服务器并在 VSCode 调试：`cargo probe-rs dap-server --chip <target>`
- RTT：`cargo run -p rtthost -- --probe list` 或 `--probe 0 --chip <name>`

## 近期变更亮点（0.30.0）

- 新增 ESP32C5、Glasgow 支持、RISC‑V FPU 寄存器曝光、更多芯片族与算法；CLI/Flasher/Debugger 多项 UX 与稳定性改进（详见 `CHANGELOG.md`）。
- 近期 Commit 示例：nusb 0.2 升级、CH347 识别改进、Xtensa 指令轮询健壮化、RTT 字段宽度修正（`git log`）。

## 初步改进建议

- 类型复用：工具层与库层都定义了 `FormatKind`（`probe-rs/src/flashing/download.rs:46-61` 与 `probe-rs-tools/src/bin/probe-rs/main.rs:288-304`），建议统一来源减少重复维护。
- 会话模块拆分：`session.rs` 职责密集（attach/trace/erase/多核），可按架构/功能剥离以降低发散式修改风险。
- 自动识别增强：ARM DP 地址默认 `Default`（`probe-rs/src/vendor/mod.rs:131-139`），可扩充已知多 DP 配置的回退扫描列表，提升复杂平台识别率。
- 文档聚合：在网站/docs.rs 聚合常见探针/目标组合最佳实践与失败排查清单，降低一线排障成本。

## 参考与代码位置

- 自动连接：`probe-rs/src/session.rs:466`
- 会话创建：`probe-rs/src/session.rs:153`
- ARM attach：`probe-rs/src/session.rs:187`
- JTAG attach：`probe-rs/src/session.rs:331`
- 目标自动识别：`probe-rs/src/vendor/mod.rs:280`
- CLI 主入口：`probe-rs-tools/src/bin/probe-rs/main.rs:430`
- 烧录下载：`probe-rs/src/flashing/download.rs:259`

