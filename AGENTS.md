# AGENTS.md

原项目路径在 `/home/nyara/文档/project/xueli-qq-bot`

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
> 最后更新：2026-06-08（大规模移植补全后）

### 完整（功能对等，有测试覆盖）

| 模块 | 文件 | 说明 |
|------|------|------|
| 信号缓存（SignalCache） | `src/signals/cache.rs` | 泛型 TTL 缓存，有测试 |
| 信号标签映射（SignalLabelMapper） | `src/signals/label_mapper.rs` | 完整映射函数，有测试 |
| 元认知监控（MetacognitionMonitor） | `src/signals/metacognition.rs` | 滑动窗口趋势分析，有测试 |
| 临时上下文（TemporalContext） | `src/signals/temporal.rs` | 事件时间归一化/间隔划分/连续性提示，有测试 |
| 消息观察（EngagementSignals） | `src/signals/engagement.rs` | 消息长度/快速回复/延续检测，有测试 |
| 信号编排（SignalOrchestrator） | `src/signals/orchestrator.rs` | L1/L2 双层缓存 + LLM 信号计算（narrative_self/character_adaptation/feedback_triage），有测试 |
| SQLite 存储层（8 个 Store） | `src/memory/stores/*.rs` | 对话/事实证据/人物事实/心情/信号/重要记忆/记忆项 |
| BM25 索引 | `src/memory/retrieval/bm25_index.rs` | jieba 中文分词 BM25 |
| 向量索引 | `src/memory/retrieval/vector_index.rs` | 字符 n-gram 向量索引 |
| 两阶段检索（TwoStageRetriever） | `src/memory/retrieval/two_stage_retriever.rs` | BM25+向量融合 + 多因素本地排序，有测试 |
| 检索协调器（RetrievalCoordinator） | `src/memory/retrieval/coordinator.rs` | prompt 上下文组装/预算管理/重要记忆检索/动态排序/情绪增强，有测试 |
| 回溯渲染器（RecallRenderer） | `src/memory/retrieval/recall_renderer.rs` | 模糊回忆渲染 |
| 访问策略（AccessPolicy） | `src/memory/internal/access_policy.rs` | 内容分类/Scope 匹配/可见性/共享检测/去重 |
| 索引协调器（IndexCoordinator） | `src/memory/internal/index_coordinator.rs` | 全局索引重建/更新 |
| 后台任务管理 | `src/memory/internal/task_manager.rs` | 异步任务创建/取消/统计，有测试 |
| Patch 合并（PatchMerger） | `src/memory/extraction/patch_merger.rs` | 记忆冲突合并 |
| 记忆反思（MemoryReflection） | `src/memory/extraction/reflection.rs` | 记忆冲突分析，有测试 |
| 提取缓冲区（ExtractionBuffer） | `src/memory/extraction/buffer.rs` | 每会话 200 轮追踪 |
| 对话摘要（ChatSummaryService） | `src/memory/extraction/chat_summary.rs` + `src/memory/chat_summary_service.rs` | LLM + 规则双模式摘要 |
| 记忆冲突解决 | `src/memory/memory_dispute_resolver.rs` | 阈值置信度分析 + ReflectionPayload，有测试 |
| 会话恢复服务 | `src/memory/session_restore_service.rs` | 构建恢复条目，有测试 |
| 对话回溯服务 | `src/memory/recall_service.rs` | 按轮评分 + 对话键解析 |
| 人物事实提取 | `src/memory/extraction/person_fact.rs` | LLM 从对话提取人物事实 |
| 人物事实服务 | `src/memory/person_fact_service.rs` | 从记忆同步人物事实，有测试 |
| TimingGate | `src/handlers/timing_gate.rs` | LLM 决策 + TTL 缓存 + 重试 + 规则回退 |
| ReplyAgent 工具循环 | `src/handlers/reply_agent.rs` | Tool trait 系统、5 内置工具、重试、分段提取、config 驱动模型名 |
| 回复风格策略 | `src/handlers/reply/style_policy.rs` | 11 维度 + 反模式检测 |
| 回复效果追踪 | `src/handlers/reply/effect_tracker.rs` | 待评估记录 + 过期，有测试 |
| 回复副作用处理 | `src/handlers/reply/side_effects.rs` | LLM 反馈评分 + 构建信号 |
| 命令系统 | `src/handlers/command/*.rs` | 注册/匹配/帮助，有测试 |
| 共享工具层 | `src/handlers/shared/*.rs`（6 文件） | 显示工具/身份/历史渲染/PromptPlanner，有测试 |
| 规划器（ConversationPlanner） | `src/handlers/planner.rs` | LLM 规划 + JSON 解析 + PromptPlanner schema |
| 规划协调器（PlanCoordinator） | `src/handlers/plan_coordinator.rs` | 完整上下文构建 + 窗口消息格式化 + 统一历史 + 关系摘要，有测试 |
| 上下文构建器（ContextBuilder） | `src/handlers/context_builder.rs` | 记忆层/角色卡/叙事/警示信号加载 |
| 提示词构建器（PromptBuilder） | `src/handlers/prompt_builder.rs` | 15 个动态上下文块注入 |
| 会话管理器（SessionManager） | `src/handlers/session_manager.rs` | 会话恢复 + 双重检查锁 + 消息管理 |
| 时间线格式化 | `src/handlers/timeline_formatter.rs` | LRU 缓存时间线渲染 |
| 消息文本工具 | `src/handlers/message_text.rs` | 消息提取/分割/格式化 |
| 图像管线（ImagePipeline） | `src/handlers/image_pipeline.rs` | 泛型 VisionClient 集成 + 图片分析 |
| 角色卡服务（CharacterCardService） | `src/character/card_service.rs` | 反馈分类/交互信号/情感历史/亲密度/效果包，有测试 |
| 叙事服务（NarrativeService） | `src/character/narrative.rs` | 事件追踪 + 主题/摘要 |
| Emoji 数据库 | `src/emoji/database.rs` | SQLite 存储 + SHA256 去重 + 分类生命周期 |
| Emoji 管理器 | `src/emoji/manager.rs` | 采集/推荐/格式检测/VLM 分类 |
| ProactiveShare 调度器 | `src/proactive_share/scheduler.rs` | 定时轮询 + 冷却 + 每日上限 |
| ProactiveShare 存储 | `src/proactive_share/store.rs` | CRUD + 去重 + 计数 |
| ContextRecorder | `src/core/context_recorder.rs` | 上下文录制与快照 |
| ImmutableMessageLog | `src/core/immutable_message_log.rs` | 不可变消息日志（内存 + SQLite），有测试 |
| MoodEngine | `src/core/mood_engine.rs` | 情绪引擎，有测试 |
| MoodStore | `src/memory/stores/mood_store.rs` | SQLite 心情存储 |
| SessionPipeline | `src/core/session_pipeline.rs` | 按会话串行消息处理 |
| ChatScope | `src/core/scope.rs` | 群聊/私聊枚举 |
| SessionResolver | `src/core/session_resolver.rs` | 事件到 SessionRef 转换 |
| MessageTrace | `src/core/message_trace.rs` | Trace ID / 执行键 |
| LogText | `src/core/log_text.rs` | 日志预览截断 |
| LogLabels | `src/core/log_labels.rs` | 日志标签常量 |
| Error types | `src/core/errors.rs` | 12 错误变体 + 4 子错误枚举 |
| Bootstrap | `src/core/bootstrap.rs` | 依赖装配/组件构建/存储器初始化 |
| BotRuntime | `src/core/runtime.rs` | 群状态机（动态阈值/防抖/延迟触发/冷却/饱和/空闲补偿），有测试 |
| EventDispatcher | `src/core/dispatcher.rs` | 事件路由/预处理器/后处理器/统计 |
| RuntimeMetrics | `src/core/metrics.rs` | 50+ 计数器（消息/命令/视觉/表情/记忆/系统/信号） |
| ReplySender | `src/core/reply_sender.rs` | 分段发送编排 + 延迟 + 自动分段 |
| XueliConfig | `src/core/config.rs` | 完整配置（Vision/Character/GroupReply/Memory/extraction/rerank/Emoji/Plugin/ContentSection） |
| 平台类型（PlatformTypes） | `src/core/platform_types.rs` | InboundEvent/ReplyAction/SessionRef/GroupState |
| 默认 AIClient HTTP | `src/services/ai_client.rs` | 整合 Python ai/* 子模块 + tools/tool_choice 序列化 |
| TokenCounter | `src/services/token_counter.rs` | trim_messages_to_budget + tool_calls/多模态计数 |
| VisionClient | `src/services/vision_client.rs` | 多模态消息构建 + 多图分析 + 贴纸情绪分类 + ImageAnalysisResult |
| ImageClient | `src/services/image_client.rs` | 图片下载/Base64 编码 |
| InvocationRouter | `src/services/invocation_router.rs` | 模型路由 + 超时控制 |
| PromptTemplateLoader | `src/services/prompt_loader.rs` | trait 抽象 + 文件加载 + 渲染 |

### 部分（核心功能可用，有显著缺失）

| 模块 | 文件 | 缺失内容 |
|------|------|----------|
| **MessageHandler** | `src/handlers/message_handler.rs` | MemoryFlowService/EmojiService/CommandHandler 等子组件已具备，但管线集成仍需完善（如回复后副作用、命令路由等） |
| **MemoryManager** | `src/memory/manager.rs` | 基本 CRUD + search + apply_patch 完整，缺少 IndexCoordinator/BackgroundCoordinator/ImportantStore 编排集成 |
| **MemoryFlowService** | `src/memory/flow_service.rs` | 异步队列 + apply_patch 完成，角色成长/关系追踪/记忆争议处理为 TODO 存根 |
| **MemoryExtractor** | `src/memory/extraction/extractor.rs` | LLM 调用 + JSON 解析完成，prompt 模板加载已改用外部模板（非硬编码） |
| **MemoryBackgroundCoordinator** | `src/memory/internal/background.rs` | tick timer + 回调架构完整，缺少消化/合并/对话保存实际逻辑 |
| **GroupMessageCollector** | `src/handlers/group_collector.rs` | 内存缓冲完成，缺少 SQLite 对话存储写入管线集成 |
| **ConversationStore** | `src/memory/stores/conversation.rs` | 扁平消息模型，缺少会话生命周期管理（active_session_ids、close_session、add_turn） |
| **MemoryItemStore** | `src/memory/stores/memory_item.rs` | 缺少衰减/归档/抑制/元数据更新/迁移 |
| **ImportantMemoryStore** | `src/memory/stores/important.rs` | 缺少 `replace_memories()` / `clear_memories()` / `mark_recalled()` |
| **EmojiReplyService** | `src/emoji/reply_service.rs` | 基本规则启发式完成，缺少 LLM 驱动的 emoji 回复决策和 ModelInvocationRouter 集成 |
| **ImageClient** | `src/services/image_client.rs` | 缺少 `process_image_segment()` / `get_mface_image_url()` 高级功能 |
| **ReplyPipeline** | `src/handlers/reply/pipeline.rs` | 记忆层 + 格式化完成，缺少视觉上下文集成和统一搜索 API |

### 骨架（仅有数据结构和 `// TODO` 占位）

无 — 所有模块至少达到"部分"级别。

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
| 存储 Scope | `storage_scope.py` | 200 | **低** — v3 键格式管理 |
| TOML 工具 | `toml_utils.py` | 95 | **低** — WebUI 配置编辑辅助 |
| 提取解析器 | `extraction/parser.py` | 203 | **低** — 行基解析/锚点验证（Rust 使用 JSON 解析替代） |

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