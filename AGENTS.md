# AGENTS.md

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