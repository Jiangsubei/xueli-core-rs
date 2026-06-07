# AGENTS.md

原项目路径在 `/home/jiangsubei/Documents/xueli-qq-bot`

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
- **持久化存储统一使用 SQLite**：所有持久化数据（记忆、信号、状态、会话等）均使用 SQLite 存储，少量配置型数据可用 JSON 文件（如 `config.toml` 的补充配置）。SQL 操作使用 `rusqlite` 或等效 crate。禁止在 async 上下文中阻塞事件循环执行 SQL 操作，应使用 `tokio::task::spawn_blocking` 包裹同步 SQL 调用。
- **禁止在持有 `tokio::sync::Mutex` / `std::sync::Mutex` 锁时调用 `.await`**：可能死锁。如需在异步中持有锁，使用 `tokio::sync::Mutex` 并注意锁的临界区不含 `.await` 即可；跨 `.await` 持锁需评估用 `Actor` 模型或 `mpsc` channel 替代

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
- `LOG_PROMPT_DIGEST` — 完整提示词
- HTTP 访问日志（标准格式）
- AI 重试日志（重试次数/延迟）
- `LOG_STARTUP_INFO` — 启动信息

日志标签常量定义在 `src/core/log_labels.rs`，使用这些常量而非硬编码字符串。

#### 禁止出现的日志
- 用户侧异常解释性文字（静默失败原则，见关键约束第 9 条）

### 提示词模板规范

**模板内容**（如风格指南、工具说明、身份声明等固定文本块）必须抽象到 `prompts/{locale}/` 目录下的 `.prompt` 模板文件，**禁止**在 Rust 代码中硬编码提示词字符串。

涉及提示词生成的模块必须使用 `PromptTemplateLoader` trait 加载模板，不得内嵌字符串。

**组合逻辑**（如根据是否有视觉结果决定是否注入某段提示词、根据场景拼接不同的 system prompt 区块）允许留在 Rust 代码中动态构建。模板内容与组合逻辑的边界：模板文件定义"说什么"，代码定义"选哪块说"。

#### 提示词内容一致性原则

- **所见即所得**：日志中出现的内容必须与模型看到的文字一致
- **口径统一**：Timing Gate / Planner / Reply 三个模型的同一语义描述核心语义必须一致，可以根据模型职责微调
- **图片描述口径**：
  - 单张图片：使用该图的逐图描述 `[图片] {per_image_description}`
  - 多张图片：优先使用合并描述 `[图片] {merged_description}`；若合并描述缺失，将逐图描述拼接
  - 识别失败：所有描述均缺失时回退为 `[图片]未成功识别`
- **空文本占位符**：用户发送空文本时统一表述为 `用户发送了空文本`

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

**优先使用构造器注入依赖（通过 trait object 或泛型）**。使用 trait bound 泛型确保编译期检查所有依赖，测试中可用 Mock 实现替代。

### 2. 异步资源初始化规范

**显式 async init 方法（Builder 模式）**。所有 `tokio::sync::Mutex` / `RwLock` 必须在 `init()` 或构造器中初始化，避免在持有锁时调用 `.await` 导致死锁。

### 3. JSON 解析与 fallback 规范

**解析阶段与 fallback 阶段分离，优先使用结构化数据**。fallback 仅在结构化数据完全缺失时触发，已解析的数据不能被覆盖。

### 4. 状态机/滑动平均类组件的测试规范

对于有状态连续性（滑动平均/指数平滑）的组件，测试应验证：
- 边界条件（输入极端值时状态不越界，如 [-1.0, 1.0]）
- 变化方向（正向输入应产生正向变化）
- 多轮迭代后的收敛性

不应断言精确相等值，因为滑动平均特性决定单次调用状态变化小，几乎不会精确相等。

### 5. 集成测试规范

抽取公共 helper 函数复用构建逻辑。测试辅助代码放在 `tests/common/mod.rs` 中。

### 6. 测试可观测性规范

测试失败时输出足够诊断信息，包含期望值、实际值和上下文，避免仅报 assertion 本身导致难以诊断。

---

## 模块实现状态

> **状态分级说明：**
> - **完整**：功能对等于 Python 原项目，有测试覆盖
> - **部分**：核心功能可用，但有显著功能缺失（标注缺失内容）
> - **骨架**：仅有数据结构和空方法/`// TODO` 占位
> - **未移植**：尚无对应 Rust 文件（分"意图性不移植"和"待移植"）
>
> 完整对照分析见 [对照分析报告](#对照分析报告) 节。

### 完整（功能对等，有测试覆盖）

| 模块 | 文件 | 说明 |
|------|------|------|
| 信号缓存（SignalCache） | `src/signals/cache.rs` | 泛型 TTL 缓存，有测试 |
| 信号标签映射（SignalLabelMapper） | `src/signals/label_mapper.rs` | 完整映射函数，有测试 |
| 元认知监控（MetacognitionMonitor） | `src/signals/metacognition.rs` | 滑动窗口趋势分析，有测试 |
| 临时上下文（TemporalContext） | `src/signals/temporal.rs` | 事件时间归一化/间隔划分/连续性提示，有测试 |
| 消息观察（EngagementSignals） | `src/signals/engagement.rs` | 消息长度/快速回复/延续检测，有测试 |
| SQLite 存储层（8 个 Store） | `src/memory/stores/*.rs` | 对话/事实证据/人物事实/心情/信号/重要记忆/记忆项 |
| BM25 索引 | `src/memory/retrieval/bm25_index.rs` | jieba 中文分词 BM25 |
| 向量索引 | `src/memory/retrieval/vector_index.rs` | 字符 n-gram 向量索引 |
| Patch 合并（PatchMerger） | `src/memory/extraction/patch_merger.rs` | 记忆冲突合并 |
| 记忆反思（MemoryReflection） | `src/memory/extraction/reflection.rs` | 记忆冲突分析，有测试 |
| 对话摘要（ChatSummaryExtractor） | `src/memory/extraction/chat_summary.rs` | 会话摘要构建 |
| 记忆冲突解决 | `src/memory/memory_dispute_resolver.rs` | 阈值置信度分析，有测试 |
| 会话恢复服务 | `src/memory/session_restore_service.rs` | 构建恢复条目，有测试 |
| 后台任务管理 | `src/memory/internal/task_manager.rs` | 异步任务创建/取消/统计，有测试 |
| TimingGate | `src/handlers/timing_gate.rs` | LLM 决策 + TTL 缓存 + 重试，比 Python 更完善 |
| ReplyAgent 工具循环 | `src/handlers/reply_agent.rs` | Tool trait 系统、4 内置工具、重试 |
| 回复风格策略 | `src/handlers/reply/style_policy.rs` | 11 维度 + 反模式检测，比 Python 更完善 |
| 回复效果追踪 | `src/handlers/reply/effect_tracker.rs` | 待评估记录 + 过期，有测试 |
| 回复副作用处理 | `src/handlers/reply/side_effects.rs` | LLM 反馈评分 + 构建信号 |
| 命令系统 | `src/handlers/command/*.rs` | 注册/匹配/帮助，有测试 |
| 共享工具层 | `src/handlers/shared/*.rs`（4 文件） | 显示工具/身份/历史渲染，有测试 |
| ContextRecorder | `src/core/context_recorder.rs` | 上下文录制与快照 |
| ImmutableMessageLog | `src/core/immutable_message_log.rs` | 不可变消息日志（内存 + SQLite），有测试 |
| MoodEngine | `src/core/mood_engine.rs` | 情绪引擎，有测试 |
| SessionPipeline | `src/core/session_pipeline.rs` | 按会话串行消息处理 |
| ChatScope | `src/core/scope.rs` | 群聊/私聊枚举 |
| 默认 AIClient HTTP | `src/services/ai_client.rs` | 整合 Python ai/* 子模块功能 |

### 部分（核心功能可用，有显著缺失）

| 模块 | 文件 | 缺失内容 |
|------|------|----------|
| **MessageHandler** | `src/handlers/message_handler.rs`（126 行 vs Python 1807 行） | 情绪引擎、MemoryFlowService、EmojiService、SignalOrchestrator、CommandHandler、ImagePipeline、ModelInvocationRouter 均未集成；当前仅基本链式调用 |
| **BotRuntime** | `src/core/runtime.rs`（141 行 vs Python ~1300 行） | 缺少群状态机（触发阈值/防抖/中断）、消息缓冲/去重、Debounce、Interrupt 处理、Mood 夜恢复 |
| **XueliConfig** | `src/core/config.rs` | 缺少 Vision 配置、Character 成长配置（情绪参数/关系追踪/亲密度阈值）、GroupReply 决策配置、Memory extraction/rerank 配置、BotBehavior 调优参数（token 预算比例/分段回复延迟/频率限制）、PlanningWindow 配置、Plugin 配置 |
| **ReplySender** | `src/core/reply_sender.rs`（29 行 vs Python 228 行） | 仅委托 `PlatformAdapter::send_action()`，缺少分段构建、延迟处理、私聊/群聊路由、@提及、引用回复、长消息分割 |
| **EventDispatcher** | `src/core/dispatcher.rs`（25 行 vs Python 329 行） | 仅基础 channel，缺少前置/后置处理器、事件类型路由、处理器注册、统计、插件钩子 |
| **RuntimeMetrics** | `src/core/metrics.rs`（7 计数器 vs Python 50+） | 缺少视觉/表情/记忆/规划器/命令/后台任务统计 |
| **PlatformTypes** | `src/core/platform_types.rs` | 缺少 `SystemEvent`、`StickerAction`、`NoopAction`、`SenderRef`、`AttachmentRef`、`PlatformCapabilities` |
| **ConversationPlanner** | `src/handlers/planner.rs` | 核心 JSON 解析完成，缺少 PromptPlanner schema 解析、情绪表情仓储集成 |
| **ContextBuilder** | `src/handlers/context_builder.rs` | 基本存储加载完成，缺少临时上下文、视觉分析集成、时间线格式化、风格策略集成、记忆上下文加载 |
| **ReplyPipeline** | `src/handlers/reply/pipeline.rs` | 记忆层加载完成，缺少视觉上下文、系统提示构建 |
| **PromptBuilder** | `src/handlers/prompt_builder.rs` | 缺少信号标签注入（conversation_window_label、mood_decision_label）、角色卡快照渲染、场景引导选择 |
| **SignalOrchestrator** | `src/signals/orchestrator.rs`（172 行 vs Python 521 行） | 仅结构化提取，缺少 LLM 信号计算（feedback_triage、character_adaptation）、L1/L2 缓存、信号存储集成 |
| **ImagePipeline** | `src/handlers/image_pipeline.rs`（44 行 vs Python 176 行） | 仅有 `analyze_image_url()`，缺少下载/贴纸检测/表情管线集成 |
| **GroupMessageCollector** | `src/handlers/group_collector.rs` | 内存缓冲完成，但缺少对话存储写入管线集成 |
| **MemoryManager** | `src/memory/manager.rs`（167 行 vs Python 883 行） | 基本 CRUD + apply_patch，缺少 IndexCoordinator/BackgroundCoordinator/AccessPolicy/ImportantStore/PersonFactStore 编排、搜索编排、Prompt 上下文构建、迁移/压缩 |
| **MemoryFlowService** | `src/memory/flow_service.rs` | 异步队列基础完成，缺少对话注册、角色成长、叙事更新、图片描述提取、冲突处理调度 |
| **MemoryExtractor** | `src/memory/extraction/extractor.rs` | LLM 调用 + JSON 解析完成，缺少缓冲区集成、prompt 模板加载（硬编码）、`load_existing_memory_records()` |
| **PersonFactService** | `src/memory/extraction/person_fact.rs` | 直接从对话提取（与 Python 从存储同步的架构不同），缺少 `sync_user_facts()` / `format_facts_for_prompt()` |
| **ImportantMemoryStore** | `src/memory/stores/important.rs` | 缺少 `replace_memories()` / `clear_memories()` / `mark_recalled()` |
| **MemoryItemStore** | `src/memory/stores/memory_item.rs` | 缺少衰减/归档/抑制/元数据更新/迁移 |
| **ConversationStore** | `src/memory/stores/conversation.rs` | 扁平消息模型，缺少会话生命周期管理（active_session_ids、close_session、add_turn） |
| **TokenCounter** | `src/services/token_counter.rs`（32 行 vs Python 179 行） | 缺少 `trim_messages_to_budget()`、Tool 定义计数、预算管理 |
| **VisionClient** | `src/services/vision_client.rs`（49 行 vs Python 406 行） | 缺少多模态消息构建、贴纸情绪分类、ImageAnalysisResult 解析 |
| **ImageClient** | `src/services/image_client.rs` | 缺少 `process_image_segment()` / `get_mface_image_url()` |
| **InvocationRouter** | `src/services/invocation_router.rs` | 无队列/工作器/超时管理，仅静态路由函数 |
| **BackgroundCoordinator** | `src/memory/internal/background.rs`（201 行 vs Python 815 行） | 仅有 tick timer 回调，缺少消化/合并/对话保存/会话终结逻辑 |
| **RetrievalCoordinator** | `src/memory/retrieval/coordinator.rs`（161 行 vs Python 1038 行） | 仅 BM25 索引检索，缺少 prompt 上下文构建、预算管理、Scope 解析、重要记忆检索、抑制、会话召回集成 |
| **AccessPolicy** | `src/memory/internal/access_policy.rs`（49 行 vs Python 387 行） | 仅检查 MemoryType，缺少 10 内容分类、5 适用性 Scope、可见性、元数据归一化 |
| **MemoryDisputeResolver** | `src/memory/memory_dispute_resolver.rs` | 基本阈值判断，缺少 ReflectionPayload 构建集成 |

### 骨架（仅有数据结构和 `// TODO` 占位）

| 模块 | 文件 | Python 行数 | Rust 完成度 |
|------|------|-------------|-------------|
| EmojiDB | `src/emoji/database.rs` | 499 | **0%** — 全部 TODO |
| EmojiManager | `src/emoji/manager.rs` | 291 | **0%** — 全部 TODO |
| EmojiReplyService | `src/emoji/reply_service.rs` | 284 | **0%** — 全部 TODO |
| ProactiveShareStore | `src/proactive_share/store.rs` | 140 | **0%** — 全部 TODO |
| ProactiveShareScheduler | `src/proactive_share/scheduler.rs` | 131 | **0%** — 全部 TODO |
| ConversationRecallService | `src/memory/recall_service.rs` | 170 | **10%** — `recall()` 返回空 Vec |
| IndexCoordinator | `src/memory/internal/index_coordinator.rs` | 83 | **10%** — rebuild/update 空存根 |
| PlanCoordinator | `src/handlers/plan_coordinator.rs` | 874 | **3%** — 空 coordinate() |
| CharacterCardService | `src/character/card_service.rs` | 600 | **10%** — 仅有数据结构 + default_card() |
| NarrativeService | `src/character/narrative.rs` | 193 | **10%** — 仅有数据结构 |

### 未移植（尚无 Rust 对应文件）

#### 意图性不移植（按规范设计，下游实现）

| 模块 | Python 位置 | 行数 | 原因 |
|------|-------------|------|------|
| 平台适配器 | `src/adapters/`（14 文件） | 2,775 | 下游实现 `PlatformAdapter trait` 的责任 |
| WebUI 控制台 | `src/webui/`（22 文件） | 3,145 | Django Web 界面，不属于 core |
| 插件系统 | `src/core/plugin/`（6 文件） | ~500 | 插件系统不在移植范围 |
| Markdown 存储 | `src/memory/storage/markdown_store.py` | 823 | 项目统一使用 SQLite |

#### 待移植（功能缺失，需补全）

| 模块 | Python 文件 | 行数 | 影响等级 |
|------|-------------|------|----------|
| 回复发送编排 | `reply_send_orchestrator.py` | 89 | **高** — 分段规范化/去重/延迟计算 |
| 会话解析器 | `session_resolver.py` | 53 | **高** — 事件到 SessionRef 转换 |
| 会话管理器 | `conversation/session_manager.py` | 226 | **高** — 会话生命周期管理 |
| PromptPlanner | `shared/prompt_planner.py` | 296 | **高** — 决策输出 schema + plan 解析 |
| 提取缓冲区 | `extraction/buffer.py` | 103 | **高** — 每会话 200 轮追踪 |
| 启动引导 | `bootstrap.py` | 386 | **中** — 依赖装配/配置验证 |
| 消息链路追踪 | `message_trace.py` | 39 | **中** — Trace ID / 执行键 |
| 时间线格式化 | `conversation/timeline_formatter.py` | 200 | **中** — LRU 缓存时间线渲染 |
| 存储 Scope | `storage_scope.py` | 200 | **低** — v3 键格式管理 |
| 回顾渲染器 | `retrieval/recall_renderer.py` | 67 | **低** — 模糊回忆渲染 |
| 消息文本工具 | `message_text.py` | 63 | **低** — 消息分割格式化 |
| 日志文本工具 | `log_text.py` | 20 | **低** — 日志预览截断 |

---

**不移植的模块**：`adapters/`（下游实现）、`webui/`（Web 界面，含 `core/runtime_supervisor.py`）、`core/plugin/`（插件系统）、`memory/storage/markdown_store.py`（项目统一使用 SQL）。

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

修改上述模块，必须附带相关测试的通过证明。