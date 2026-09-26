# ADR-0002 · 云端能力先用 Mock，接口先抽象

**状态**：已接受 · **日期**：M0

## 背景

test.md 的「二刷」（云端整句/热词/LLM 重排）和「三段式语音校正」都依赖云端 ASR + LLM。供应商未定（火山引擎/讯飞/自建中转/本地离线模型都可能）。

## 决定

**M0–M2 一律用 Mock 实现**，但 trait 边界按真实供应商的形状设计，后续替换只动 `core/cloud` 内部。

```rust
pub trait CloudPinyin: Send + Sync {
    fn suggest(&self, req: PinyinRequest) -> BoxFuture<'static, Result<PinyinSuggestion>>;
}
pub trait LlmReranker: Send + Sync {
    fn rerank(&self, req: RerankRequest) -> BoxFuture<'static, Result<RerankResponse>>;
}
pub trait StreamingAsr: Send + Sync {
    fn start(&self, cfg: AsrConfig) -> Result<AsrSession>;
}
```

Mock 必须能**主动制造故障**，因为降级路径才是产品价值所在（test.md 第八/九节）：

```rust
pub struct MockCloud {
    pub latency: Duration,        // 模拟慢网
    pub fail_rate: f32,           // 模拟失败
    pub hang: bool,               // 模拟服务 hang 死（验证超时与熔断）
    pub stale_gen: bool,          // 模拟返回过期代次结果（验证 gen 丢弃）
}
```

## 理由

1. **降级路径必须在第一天就被测试**，而不是等接了真实服务才发现「断网就卡键盘」。用 Mock 注入延迟/失败/hang，可以把 [ARCHITECTURE.md §7 降级矩阵](../../ARCHITECTURE.md#7-降级矩阵) 的每一行都写成单元测试。
2. **不被供应商绑死**。trait 形状稳定后，火山/讯飞/OpenAI 兼容接口都只是新增一个 impl。
3. **开发期零成本**：不需要 AK/SK、不产生费用、CI 可离线跑。
4. Mock 的确定性输出让「候选合并规则」（P3）可被精确断言。

## 代价

真实供应商的协议细节（分帧格式、鉴权、错误码语义、final pass 的信令）会在 M3/M4 暴露设计缺陷，届时可能需要调整 trait。**缓解**：trait 里只承诺「输入音频帧 / 输出 AsrEvent」这类语义级契约，不承诺传输细节；`AsrEvent` 用 test.md 的三段式命名（Interim/Stable/Final），这个划分是供应商无关的。
