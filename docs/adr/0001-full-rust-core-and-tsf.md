# ADR-0001 · 内核与 TSF 全部用 Rust

**状态**：已接受 · **日期**：M0

## 背景

Windows 输入法必须是 TSF COM DLL；后续要支持 Android（`InputMethodService`）。test.md 图 5 的核心主张是「语音、物理键盘、触摸键盘共用同一个大脑」——这要求内核尽可能跨端复用。

候选方案：

1. **全 Rust**：内核 + TSF TIP 都用 Rust（`windows` crate 直接实现 COM）
2. C++ TSF 外壳 + Rust 内核（C ABI FFI）
3. 纯 C++（Android 端用 Kotlin/C++ 重写内核）

## 决定

**采用方案 1（全 Rust）**。

## 理由

- **一门语言贯穿 core + Windows 平台层**，没有 FFI 边界的序列化/生命周期/错误码翻译成本，也没有 C++ 构建系统（CMake + vcpkg）负担。
- `windows` crate 对 TSF 的绑定是**完整的**（`ITfTextInputProcessorEx`、`ITfKeyEventSink`、`ITfEditSession`… 全都有），且 `#[implement]` 能正确生成 vtable。已用 probe crate 编译验证（见 [windows-tsf.md](../windows-tsf.md#已验证事实)）。
- Rust 的内存安全 + `Result` 强制错误处理，正好对上 [ARCHITECTURE.md](../../ARCHITECTURE.md) 的 P2（优雅降级）：TSF 跑在别人进程里，我们崩溃就是宿主崩溃。
- Android 端复用路径清晰：同一份 `core/`，通过 `retype-ffi`（C ABI）给 Kotlin 调用。

## 代价与缓解

| 风险 | 缓解 |
|---|---|
| Rust 写 TSF 的公开资料少，遇坑只能啃 `windows` crate 生成码 | 已把关键 trait 签名/feature 名验证并记录在 [windows-tsf.md](../windows-tsf.md)；保留「加一层 C++ shim」的退路（只需替换 `platforms/windows/tsf`，`core/` 不动） |
| COM 引用计数/聚合语义容易写错 | 只用 `#[implement]` 生成，不手写 vtable；`windows_core::Ref`/`IUnknown` 全权交给 crate |
| 需要同时产出 x86 与 x64 | `rustup target add i686-pc-windows-msvc`，CI 双目标构建 |

## 备选方案为何被否

- **方案 2**：多一门语言 + 一层 FFI，收益（「C++ 样例更多」）在 probe 验证通过后显著缩水。若 M1 在 TSF 上撞到无法逾越的坑，可局部退回此方案，`core/` 无需改动。
- **方案 3**：Android 端要重写整个内核，直接违背 test.md 图 5。
