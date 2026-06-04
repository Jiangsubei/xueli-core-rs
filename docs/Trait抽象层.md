# Trait 抽象层

> **TODO** — 当前 Rust 实现状态：6 个 trait 定义已完成，`DefaultAIClient` 已实现 `AIClient` trait。`PromptTemplateLoader` 和 `Tokenizer` 需要默认实现。其余 trait 留给下游实现。
>
> 覆盖文件：`src/traits/ai_client.rs` `src/traits/tool_calling.rs` `src/traits/tokenizer.rs` `src/traits/prompt_template.rs` `src/traits/platform_adapter.rs` `src/traits/timing_gate.rs`

---

## 1. Trait 一览

| Trait | 文件 | 用途 | 默认实现 |
|-------|------|------|---------|
| `AIClient` | `ai_client.rs` | AI 模型调用（chat completion） | `DefaultAIClient` (HTTP) |
| `ToolCallingStrategy` | `tool_calling.rs` | Tool-calling 协议解析 | 下游实现 |
| `Tokenizer` | `tokenizer.rs` | 中文分词 | `JiebaTokenizer` |
| `PromptTemplateLoader` | `prompt_template.rs` | 提示词模板加载（i18n） | `FileTemplateLoader` |
| `PlatformAdapter` | `platform_adapter.rs` | 平台发送动作适配 | 下游实现 |
| `TimingGateStrategy` | `timing_gate.rs` | 群聊时机判断策略 | `DefaultTimingGate` |

---

## 2. AIClient

AI 模型调用 trait，下游通过实现此 trait 接入不同的 AI 服务（OpenAI、Claude、本地模型等）。

### 2.1 接口定义

```rust
#[async_trait]
pub trait AIClient: Send + Sync {
    async fn chat_completion(
        &self,
        request: &ChatCompletionRequest,
    ) -> Result<ChatCompletionResponse, String>;
}
```

### 2.2 数据类型

```rust
pub struct ChatMessage {
    pub role: String,       // "system" | "user" | "assistant" | "tool"
    pub content: String,
}

pub struct ChatCompletionRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    pub temperature: Option<f64>,
    pub max_tokens: Option<u32>,
}

pub struct ChatCompletionResponse {
    pub content: String,
    pub finish_reason: Option<String>,
    pub usage: Option<TokenUsage>,
}

pub struct TokenUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}
```

### 2.3 默认实现

`DefaultAIClient`（`src/services/ai_client.rs`）基于 HTTP + OpenAI 兼容 API。详见 [服务层](./服务层.md#2-defaultaiclient--ai-客户端)。

---

## 3. ToolCallingStrategy

Tool-calling 协议解析 trait，下游实现不同 LLM 的 tool-calling 协议（OpenAI function calling、Claude tool use 等）。

### 3.1 接口定义

```rust
#[async_trait]
pub trait ToolCallingStrategy: Send + Sync {
    /// 解析 LLM 响应中的 tool call
    fn parse_tool_calls(&self, response: &str) -> Result<Vec<ToolCall>, String>;

    /// 构建 tool result 消息
    fn build_tool_result(&self, call_id: &str, result: &str) -> ChatMessage;
}

pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
}
```

### 3.3 待实现

- [ ] OpenAI function calling 协议解析实现
- [ ] 柔性 JSON 解析（serde 严格模式 → 手动修复 → regex 提取）

---

## 4. Tokenizer

中文分词 trait，默认使用 `jieba-rs`。

### 4.1 接口定义

```rust
pub trait Tokenizer: Send + Sync {
    /// 分词
    fn tokenize(&self, text: &str) -> Vec<String>;

    /// 切分为 n-gram
    fn ngrams(&self, text: &str, n: usize) -> Vec<String>;
}
```

### 4.2 默认实现

`JiebaTokenizer` 使用 `jieba-rs` 进行中文分词。用于 BM25 索引构建。

### 4.3 待实现

- [ ] `JiebaTokenizer` 默认实现

---

## 5. PromptTemplateLoader

提示词模板加载 trait，支持 i18n。

### 5.1 接口定义

```rust
pub trait PromptTemplateLoader: Send + Sync {
    /// 加载指定模板
    fn load(&self, name: &str) -> Result<String, String>;

    /// 加载并渲染模板（替换变量）
    fn load_and_render(&self, name: &str, vars: &HashMap<String, String>)
        -> Result<String, String>;

    /// 验证必需模板是否存在
    fn validate_required(&self, names: &[&str]) -> Result<(), String>;
}
```

### 5.2 默认实现

`FileTemplateLoader` 从 `prompts/{locale}/` 目录加载 `.prompt` 文件。也可使用 `include_str!()` 编译期嵌入。

### 5.3 模板文件

当前 `prompts/zh-CN/` 下有 25 个 `.prompt` 文件：

| 模板文件 | 用途 |
|---------|------|
| `timing_gate.prompt` | 时机判断 |
| `timing_gate_identity.prompt` | 时机判断身份声明 |
| `planner.prompt` | 回复规划 |
| `planner_emoji_section.prompt` | 规划中的表情部分 |
| `planner_reminder.prompt` | 规划提醒 |
| `prompt_notes.prompt` | 提示词注意事项 |
| `reply_agent_system_base.prompt` | ReplyAgent system prompt |
| `reply_style_guidance.prompt` | 回复风格指南 |
| `identity.prompt` | 角色身份声明 |
| `vision.prompt` | 图片理解 |
| `vision_emotion.prompt` | 表情分类（VLM） |
| `vision_user_prompt.prompt` | 图片理解 user prompt |
| `vision_sticker_prompt.prompt` | 贴纸提示 |
| `rerank.prompt` | 记忆重排 |
| `reflection.prompt` | 记忆反思 |
| `insight_digestion.prompt` | 离线消化 |
| `narrative_self.prompt` | 长期相处脉络 |
| `feedback_triage.prompt` | 反馈分流 |
| `relationship_tone.prompt` | 关系语气 |
| `scene_guidance_group.prompt` | 群聊场景指南 |
| `scene_guidance_private.prompt` | 私聊场景指南 |
| `character_adaptation.prompt` | 角色自适应 |
| `memory_extraction.prompt` | 记忆提取 |
| `memory_reliability.prompt` | 记忆可靠性 |
| `emoji_reply.prompt` | 表情追评决策 |

### 5.4 待实现

- [ ] `FileTemplateLoader` 默认实现
- [ ] 模板变量替换
- [ ] 模板验证

---

## 6. PlatformAdapter

平台适配器 trait，下游实现各 IM 平台特有的消息收发。

### 6.1 接口定义

```rust
#[async_trait]
pub trait PlatformAdapter: Send + Sync {
    /// 发送回复动作
    async fn send_action(&self, action: &ReplyAction) -> Result<(), String>;

    /// 去除消息中的 @提及
    fn strip_mentions(&self, text: &str) -> String;

    /// 获取平台名称标识
    fn platform_name(&self) -> &str;

    /// 解析原始事件为统一格式
    fn parse_event(&self, raw: &str) -> Result<InboundEvent, String>;
}
```

### 6.2 下游实现指南

适配器层负责：
1. 将平台特定协议（OneBot、API HTTP 等）转换为 `InboundEvent`
2. 将 `ReplyAction` 翻译为平台特定发送操作
3. 处理 @提及解析、引用回复等平台特有逻辑

平台适配器不在 core 内实现，留给下游项目。原 Python 项目有 NapCat（QQ）和 API 两种适配器实现供参考。

---

## 7. TimingGateStrategy

群聊时机判断策略 trait。

### 7.1 接口定义

```rust
#[async_trait]
pub trait TimingGateStrategy: Send + Sync {
    async fn should_reply(&self, ctx: &TimingContext) -> Result<TimingDecision, String>;
}

pub struct TimingContext {
    pub event: InboundEvent,
    pub is_mentioned: bool,
    pub conversation_active: bool,
    pub time_since_last_reply_secs: f64,
    pub message_count_in_window: u32,
}

pub enum TimingDecision {
    Reply,
    Wait,
    Ignore,
}
```

### 7.2 默认实现

`DefaultTimingGate`（`src/handlers/timing_gate.rs`）实现基于 LLM 的时机判断。详见 [消息处理管线](./消息处理管线.md#3-timinggate--时机判断)。

---

## 8. 设计原则

- **Trait 隔离**：所有可替换组件通过 trait 抽象，core 不依赖具体实现
- **泛型 trait bound**：使用 `impl<A: AIClient, P: PlatformAdapter>` 确保编译期检查
- **默认实现在 services/ 和 handlers/**：提供开箱即用的默认行为
- **下游替换**：下游项目可以实现任意 trait 替换默认行为

---

*对应原项目文档：`xueli-qq-bot/docs/Rust 核心库移植方案.md` 第 3.1 节*