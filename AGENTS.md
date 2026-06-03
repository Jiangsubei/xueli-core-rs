# AGENTS.md

---

## 通用原则

### 实现新规则前必须检查复用可能性

添加任何新的决策逻辑、条件分支或启发式规则之前，必须完成以下检查步骤：

- 检索已有规则
- 确认无等价或重叠规则
- 优先扩展而非新建

### 编码陷阱（必读）

- **禁止在 async 上下文中使用同步阻塞 I/O**：`std::fs::read_to_string()` / `std::fs::write()` 等会阻塞 tokio 事件循环，应使用 `tokio::fs` 或 `tokio::task::spawn_blocking` 包裹
- **文件写入必须原子化**：所有持久化先写 `.tmp` 再 `std::fs::rename()`，禁止直接覆写目标文件，防止写入中断导致文件损坏
- **禁止在持有 `tokio::sync::Mutex` / `std::sync::Mutex` 锁时调用 `.await`**：可能死锁。如需在异步中持有锁，使用 `tokio::sync::Mutex` 并注意锁的临界区不含 `.await` 即可；跨 `.await` 持锁需评估用 `Actor` 模型或 `mpsc` channel 替代
- **`unwrap()` / `expect()` 仅允许在初始化阶段和测试中使用**：核心逻辑路径必须返回 `Result<_, XueliError>`
- **避免 `String` 作为错误类型**：核心模块使用 `XueliError`（基于 `thiserror`），仅在 trait 边界或原型阶段允许 `String`

### 依赖协议

MIT 许可证。引入新依赖必须 permissive 协议（MIT、BSD-3-Clause、Apache-2.0、ISC、Zlib、Unicode）。**禁止** GPL-3.0/AGPL-3.0/LGPL-3.0。

**引入新依赖前需评估**：是否已有等价功能的标准库或现有依赖、对编译时间/二进制体积的影响、维护活跃度（最近 6 个月内有无提交）；评审通过后方可引入。

### 日志规范

#### 格式要求
- `[模块]` 格式日志仅输出简单内容，**禁止**结构化参数（如 `key=value` 形式的 tracing field 仅用于 Debug 级别）
- 日志级别：`INFO` 用于关键节点，`DEBUG` 用于详细调试
- 使用 `tracing` crate 而非 `println!()` 或 `log`

#### 必须保留的日志
- `LOG_PROMPT_FULL` — 完整提示词（便于排查 AI 输出异常）
- `LOG_PROMPT_DIGEST` — 提示词摘要
- HTTP 访问日志（标准格式）
- AI 重试日志（重试次数/延迟）
- `LOG_STARTUP_INFO` — 启动信息

日志标签常量定义在 `src/core/log_labels.rs`，使用这些常量而非硬编码字符串。

#### 禁止出现的日志
- 规划原始 DEBUG 日志（包含 `plan.action` / `plan.reason` 等），防止泄漏内部决策细节
- 非关键路径的 DEBUG/INFO 心跳日志
- 用户侧异常解释性文字（静默失败原则，见关键约束第 9 条）

### 提示词模板规范

**模板内容**（如风格指南、工具说明、身份声明等固定文本块）必须抽象到 `prompts/{locale}/` 目录下的 `.prompt` 模板文件，**禁止**在 Rust 代码中硬编码提示词字符串。

涉及提示词生成的模块必须使用 `PromptTemplateLoader` trait 加载模板，不得内嵌字符串。

**组合逻辑**（如根据是否有视觉结果决定是否注入某段提示词、根据场景拼接不同的 system prompt 区块）允许留在 Rust 代码中动态构建。模板内容与组合逻辑的边界：模板文件定义"说什么"，代码定义"选哪块说"。

#### 提示词内容一致性原则（强制）

- **所见即所得**：日志中出现的占位符必须与模型看到的文字一致
- **口径统一**：Timing Gate / Planner / Reply 三个模型的同一语义描述核心语义必须一致
- **图片描述口径**：
  - 单张图片：使用该图的逐图描述 `[图片] {per_image_description}`
  - 多张图片：优先使用合并描述 `[图片] {merged_description}`；若合并描述缺失，将逐图描述拼接
  - 识别失败：所有描述均缺失时回退为 `[图片]未成功识别`
- **空文本占位符**：用户发送空文本时统一表述为 `用户发送了空文本`，不用 `[空]` 等缩写

---

## 项目规范

### 常用命令

```bash
# 编译检查
cargo check

# 全量编译
cargo build

# 运行测试
cargo test

# 运行特定测试
cargo test test_default_config

# 代码格式化
cargo fmt --all

# Lint 检查
cargo clippy --all-targets

# 检查格式 + lint
cargo fmt --all -- --check && cargo clippy --all-targets
```

- **代码检查**：使用 `cargo fmt`（格式化）和 `cargo clippy`（lint），CI 中强制执行 fmt 检查
- 测试框架是 Rust 内置 `#[test]` + `tokio::test`（异步测试）
- import 路径为 `xueli_core::*`（如 `use xueli_core::core::config::XueliConfig`）
- 配置文件 `config.toml` 为本地私有配置（gitignore），首次运行需从 `config.example.toml` 复制
- `data/` 目录是运行时产物，已 gitignore，不提交

---

### 核心架构

详细架构说明见 `docs/项目架构.md`（与 Python 原项目共享同一架构文档）。

**此文件仅包含架构相关的核心规范（不在架构文档中）：**

#### 主消息处理链

```
InboundEvent
  → TimingGate (时机判断: reply/wait/ignore)
  → Planner (回复规划 + 情绪信号)
  → ContextBuilder (上下文构造: 对话历史 + 记忆检索 + 角色卡)
  → ReplyAgent (Tool-calling 工具循环)
  → ReplyAction → PlatformAdapter.send_action()
```

#### Trait 隔离原则
- 所有可替换组件通过 `src/traits/` 中的 trait 抽象：`AIClient`、`ToolCallingStrategy`、`Tokenizer`、`PromptTemplateLoader`、`PlatformAdapter`、`TimingGateStrategy`
- 默认实现在 `src/services/` 和 `src/handlers/` 中，下游项目可以替换任意组件
- `PlatformAdapter` 负责将 `ReplyAction` 翻译为具体平台操作，core 不应被 QQ/NapCat/API 等平台细节污染

#### 上下文窗口统一原则（强制）
- 群聊上下文的**主事实源**是 conversation store 中的 `group_messages`
- 上下文由 `TokenCounter` 基于 token 预算统一管理，`context_token_budget_ratio` 控制输入预算比例；`max_context_length` 降级为兼容性兜底，不再作为截断唯一依据

#### 主路径提示词模板（供参考）
- `prompts/zh-CN/timing_gate.prompt` — 时机判断（reply/wait/ignore + planning_signals）
- `prompts/zh-CN/planner.prompt` — 回复规划
- `prompts/zh-CN/narrative_self.prompt` — 长期相处脉络低频更新
- `prompts/zh-CN/reply_agent_system_base.prompt` — ReplyAgent system prompt 基础

---

### 记忆系统

- 记忆写入统一经由 `MemoryFlowService`（**不在** ReplyAgent 内）
- 三层记忆：person_fact / chat_summary / conversation_recall
- 存储：统一写入 SQLite（`rusqlite`，bundled 模式），路径由 `MemoryConfig.db_path` 配置
- 拟人化特性：动态遗忘（用进废退）、软遗忘（归档记忆打折召回）、情绪标记、离线消化、语义联想、脆性期保护、离线巩固、提取诱发遗忘、记忆合并、时段匹配检索

#### 记忆隔离原则（强制）

**正常情况下，存储路径与检索路径必须对称**
- 写入时 `storage_user_id` 格式为 `{platform}:{scope}:{scope_id}:{user_id}`
- 检索层（`get_scope_user_ids`）必须根据 `ChatScope` 和 `scope_id` 返回正确的存储 ID 列表
- 若因数据迁移或临时回退需要打破对称性，迁移方案需经过评审，确保过渡期间两个路径行为一致

**索引重建必须覆盖全量用户**
- `rebuild_all_indices()` 必须使用 `storage.get_user_ids()` 获取完整列表，不能自己扫描文件系统
- `get_user_ids()` 返回什么，索引就必须扫什么

**作用域匹配不能产生歧义**
- `Group("")` 在 group_id 为空字符串时会错误匹配（如私聊场景）
- 无有效 `source_group_id` 时应回退到 `Private` scope，不能制造空字符串的 Group scope

### ChatScope — 会话模态抽象

定义在 `src/core/scope.rs`：

```rust
pub enum ChatScope {
    Private,           // 一对一对话
    Group(String),     // 多用户会话（群聊，含群 ID）
}
```

**强制规则**：
- 所有表示会话模态的逻辑**必须**使用 `ChatScope` 枚举及其方法（`is_group()` / `is_private()` / `group_id()`），**禁止**硬编码字符串比较
- Adapter 层（下游实现 `PlatformAdapter`）负责输出 `ChatScope` 作为 SessionRef 的 scope
- API 适配器需显式映射层输出，不与内部枚举耦合

---

### 关键约束

1. 私聊和群聊共用一条 conversation 主链，**不要**分裂两套逻辑
2. 回复后副作用（记忆写入、信号更新）走 `MemoryFlowService`，不在 ReplyAgent 内
3. 命名用 `conversation_*`、`platform_*` 等中性命名，**不要**扩散 `group_*`、`napcat_*`
4. 会话永不过期，重启后从历史存储恢复并保留原始时间信息
5. 分段发送：LLM 通过 `ReplyAction` 的 `segments` 字段控制分段；未传 segments 时按空行兜底分段
6. 普通图片（image sub_type=0）只做视觉理解，不入表情仓库；表情贴纸（image sub_type≠0,4,9）自动采集入库，SHA256 去重，VLM 标注情绪标签
7. `data/` 目录是运行时产物，已 gitignore，不提交
8. `TimingGateConfig` 未配置 group_reply_decision 时，群聊退回规则路径（通常只在被 @ 时回复）
9. **用户侧异常提醒原则**：处理失败时不要发送解释性文字给用户，静默失败即可
10. **语义信号失败原则**：用户情绪、反馈偏好、叙事、自我监控等语义信号只接受 LLM 或已有结构化信号；LLM 失败时不使用关键词/规则伪造语义信号，直接不注入该部分提示词，主回复继续正常执行
11. **主动分享**（`ProactiveShareScheduler` + `ProactiveShareStore`）默认关闭（`proactive_share.enabled = false`）。开启后：后台记忆消化每产生一条 insight 时，自动写入一条主动分享，由调度器定时发送。Send 路径在 `BotRuntime.send_proactive_share()`，创建 `ReplyAction` 经由 PlatformAdapter 发出；发送失败不会 `mark_sent`

---

## 组件初始化与测试规范

### 1. 组件初始化原则

**优先使用构造器注入依赖（通过 trait object 或泛型）**

```rust
// 推荐：泛型 trait bound 依赖注入
pub struct MessageHandler<A: AIClient, P: PlatformAdapter> {
    config: Arc<XueliConfig>,
    ai_client: Arc<A>,
    platform: Arc<P>,
    // ...
}

impl<A: AIClient, P: PlatformAdapter> MessageHandler<A, P> {
    pub fn new(config: Arc<XueliConfig>, ai_client: Arc<A>, platform: Arc<P>) -> Self {
        // ...
    }
}
```

**原因：** 使用 trait bound 泛型确保编译期检查所有依赖，测试中可用 Mock 实现替代。

### 2. 异步资源初始化规范

```rust
// 推荐：显式 async init 方法（Builder 模式）
impl BotRuntime {
    pub async fn init(&self) -> Result<(), XueliError> {
        // 初始化锁、连接池等异步资源
    }
}

// 或使用 Builder 模式
let runtime = BotRuntime::builder()
    .config(config)
    .platform_adapter(adapter)
    .ai_client(client)
    .build()?;
```

**锁管理：** 所有 `tokio::sync::Mutex` / `RwLock` 必须在 `init()` 或构造器中初始化，避免在持有锁时调用 `.await` 导致死锁。

### 3. JSON 解析与 fallback 规范

**问题背景：** 多层 fallback 可能导致已解析数据被覆盖。

```rust
// 推荐：解析阶段与 fallback 阶段分离，优先使用结构化数据
let data: Option<ParsedData> = extract_json_object(&content);
if let Some(parsed) = data {
    // 使用结构化数据
} else {
    // 仅在结构化数据完全缺失时触发 fallback
    fallback_to_raw_content(&content);
}

// 避免：fallback 覆盖已解析的数据
```

**原则：** fallback 仅在结构化数据完全缺失时触发，已解析的数据不能被覆盖。

### 4. 状态机/滑动平均类组件的测试规范

**问题背景：** 使用滑动平均（如 `MoodEngine.tick()`）的组件，单次调用状态变化小，断言精确值易失败。

```rust
// 推荐：测试边界条件和方向，不测精确值
#[test]
fn test_mood_engine_extreme_input() {
    let mut engine = MoodEngine::new(true, 0.0);
    engine.set_valence(0.0);
    let result = engine.tick(1.0);
    assert!(result.state.valence > 0.0); // 验证方向
}

// 避免：断言精确相等
assert_eq!(result.valence, expected_valence); // 滑动平均特性决定几乎不会精确相等
```

**原则：** 对于有状态连续性（滑动平均/指数平滑）的组件，测试应验证：
- 边界条件（输入极端值时状态不越界，如 [-1.0, 1.0]）
- 变化方向（正向输入应产生正向变化）
- 多轮迭代后的收敛性

### 5. 集成测试规范

```rust
// 推荐：抽取公共 helper 函数复用构建逻辑
fn build_test_config() -> XueliConfig {
    XueliConfig::default()
}

fn build_test_runtime() -> BotRuntime {
    BotRuntime::new(build_test_config())
}

#[tokio::test]
async fn test_message_flow() {
    let runtime = build_test_runtime();
    // ...
}
```

测试辅助代码放在 `tests/common/mod.rs` 中。

### 6. 测试可观测性规范

```rust
// 推荐：测试失败时输出足够诊断信息
#[test]
fn test_reply_segments() {
    let segments = get_segments();
    assert_eq!(
        segments.len(),
        3,
        "期望3段，实际{}段，segments={:?}",
        segments.len(),
        segments
    );
}

// 避免：只报 assertion 本身
assert_eq!(segments.len(), 3); // 失败时难以诊断
```

---

## 遗留 / 未使用代码

以下模块在当前主路径中仅为骨架占位（`todo!()`），尚未实现完整逻辑：

| 文件 | 说明 |
|------|------|
| `src/memory/stores/*.rs` | 5 个 SQLite Store，建表 + CRUD 待实现 |
| `src/memory/retrieval/*.rs` | BM25/向量检索待实现 |
| `src/memory/extraction/*.rs` | LLM 记忆提取待实现 |
| `src/services/ai_client.rs` | 默认 AI 客户端 HTTP 实现待补全 |
| `src/handlers/planner.rs` | ConversationPlanner 待实现 |
| `src/handlers/reply_agent.rs` | ReplyAgent 工具循环待实现 |

---

### 高风险改动

以下模块改动必须连带检查测试和所有调用点：

- `ReplyAgent` — `_build_system_prompt()` 提示词结构
- `ConversationContextBuilder` — 上下文构建
- `MessageHandler` — Agent 调用入口
- `MemoryManager` / `MemoryFlowService`
- `BotRuntime` — 主处理循环
- 提示词模板文件 `prompts/zh-CN/*.prompt`
- `TokenCounter` — token 预算管理
- `traits/ai_client.rs` — AIClient trait 定义

修改上述模块的 PR，必须附带相关测试的通过证明，且至少一名其他开发者审查通过。

---

### Trait 一览（供参考）

| Trait | 文件 | 用途 |
|-------|------|------|
| `AIClient` | `src/traits/ai_client.rs` | AI 模型调用（chat completion） |
| `ToolCallingStrategy` | `src/traits/tool_calling.rs` | Tool-calling 协议解析 |
| `Tokenizer` | `src/traits/tokenizer.rs` | 分词（中文优先） |
| `PromptTemplateLoader` | `src/traits/prompt_template.rs` | 提示词模板加载（i18n） |
| `PlatformAdapter` | `src/traits/platform_adapter.rs` | 平台发送动作适配 |
| `TimingGateStrategy` | `src/traits/timing_gate.rs` | 群聊时机判断策略 |

### 已模板化的提示词（供参考）

- `timing_gate.prompt` — 时机判断
- `timing_gate_identity.prompt` — 时机判断身份声明
- `planner.prompt` — 回复规划
- `planner_emoji_section.prompt` — 规划中的表情部分
- `planner_reminder.prompt` — 规划提醒
- `prompt_notes.prompt` — 提示词注意事项
- `reply_agent_system_base.prompt` — ReplyAgent system prompt
- `reply_constraint.prompt` — 回复格式约束
- `reply_style_guidance.prompt` — 回复风格指南
- `vision.prompt` — 图片理解
- `vision_emotion.prompt` — 表情分类（VLM）
- `vision_user_prompt.prompt` — 图片理解 user prompt
- `vision_sticker_prompt.prompt` — 贴纸提示
- `rerank.prompt` — 记忆重排
- `reflection.prompt` — 记忆反思
- `insight_digestion.prompt` — 离线消化
- `narrative_self.prompt` — 长期相处脉络
- `feedback_triage.prompt` — 反馈分流
- `relationship_tone.prompt` — 关系语气
- `scene_guidance_group.prompt` — 群聊场景指南
- `scene_guidance_private.prompt` — 私聊场景指南
- `narrative_validator.prompt` — 叙事验证
- `extraction_chat_summary.prompt` — 对话摘要提取
- `extraction_person_fact.prompt` — 人物事实提取
- `emoji_reply.prompt` — 表情追评决策