# xueli-core

> 可复用 AI 对话框架 Rust 标准库 · 开放 · 轻量 · 平台解耦

**xueli-core** 是 xueli 项目的 Rust 核心标准库。它将 xueli 的对话框架抽象为独立 crate，不绑定特定平台，通过 trait 多态支持下游项目自由接入 QQ、API、Discord 等任意渠道。

**与 Python 原项目的关系：** xueli-core 是平行独立的 Rust 实现，与 [xueli-qq-bot](https://github.com/Jiangsubei/xueli-qq-bot) 共享相同的架构设计，但不包含平台适配层和 WebUI。下游项目引入本库后，只需实现少量 trait 即可接入自己的平台。

---

## 设计原则

| 原则 | 说明 |
|------|------|
| **trait 多态** | AI 客户端、分词器、tool-calling、模板加载、平台适配器均可替换 |
| **中文优先** | 默认分词、提示词模板均以中文场景为首要目标，通过 trait 支持 i18n |
| **全功能管线** | TimingGate → Planner → ContextBuilder → ReplyAgent → MemoryFlow |
| **异步驱动** | 基于 `tokio`，与 Python asyncio 保持一致的并发模型 |
| **MIT 许可** | 依赖链全部为 permissive 协议 |

---

## 核心管线

```
InboundEvent
  │
  ├─ TimingGate ──── 时机判断 (reply / wait / ignore)
  │
  ├─ Planner ─────── 回复规划 + 情绪信号
  │
  ├─ ContextBuilder ─ 上下文构造 (对话历史 + 记忆检索 + 角色卡)
  │
  ├─ ReplyAgent ──── Tool-calling 工具循环 (reply / query_memory / send_emoji / …)
  │
  └─ MemoryFlow ──── 后台记忆沉淀 (摘要 / 事实提取 / 反思)
```

## 主要功能

| 模块 | 能做什么 |
|------|---------|
| **Timing Gate** | 群聊非 @ 消息的轻量规划，输出 reply/wait/ignore 及情绪、参与倾向 |
| **ReplyAgent** | Tool-calling 多轮工具循环，生成最终回复 |
| **群聊消息收集** | wait 窗口内收集同群后续消息，合并上下文 |
| **用户画像信号** | 长期风格适应和关系状态理解 |
| **长期相处脉络** | 后台低频更新 `narrative_self`，刻画长期关系 |
| **元认知谨慎度** | 聚合风险信号，事实不稳时让回复更克制 |
| **反馈学习** | ReplyAgent 声明 `expected_effect`，下一轮判断是否达成 |
| **提示词模板** | 将规划、画像、视觉、记忆等提示词放在模板文件里，方便修改 |
| **结构化分段发送** | 回复拆成多条消息，逐条发出并加随机延迟 |
| **会话连续性** | 私聊和群聊对话永不过期，重启后自动恢复 |
| **多层记忆** | 三层记忆（人物事实/对话摘要/普通回忆），用进废退、脆性期、竞争抑制 |
| **图片理解** | 普通图片视觉分析；表情贴纸自动采集入库 |
| **表情互动** | SHA256 去重，视觉模型自动标注情绪标签，按情绪意图发送匹配贴纸 |
| **可替换组件** | AI 客户端、分词器、tool-calling 协议、模板加载器、平台适配器均可替换 |

## 快速开始

### 依赖

```toml
[dependencies]
xueli-core = { git = "https://github.com/Jiangsubei/xueli-core-rs" }
```

### 最少代码示例

```rust
use xueli_core::prelude::*;
use xueli_core::core::config::XueliConfig;
use xueli_core::core::runtime::BotRuntime;

#[tokio::main]
async fn main() -> Result<(), XueliError> {
    // 加载配置
    let config = XueliConfig::load("config.toml")?;

    // 创建运行时（注入你自己的 PlatformAdapter 和 AIClient 实现）
    let runtime = BotRuntime::builder()
        .config(config)
        .platform_adapter(my_platform_adapter)
        .ai_client(my_ai_client)
        .build()?;

    // 启动
    runtime.run().await
}
```

### 实现自己的平台适配器

```rust
use async_trait::async_trait;
use xueli_core::prelude::*;

struct MyPlatformAdapter;

#[async_trait]
impl PlatformAdapter for MyPlatformAdapter {
    async fn send_action(&self, action: ReplyAction) -> Result<(), XueliError> {
        // 把 ReplyAction 翻译成自己平台的发送逻辑
        todo!()
    }

    async fn on_event(&self) -> Result<Option<InboundEvent>, XueliError> {
        // 从平台接收消息事件
        todo!()
    }
}
```

## 项目结构

```
xueli-core/
├── Cargo.toml
├── src/
│   ├── lib.rs                      # 公开 API re-export, prelude
│   │
│   ├── traits/                     # 下游可替换的核心 trait
│   │   ├── ai_client.rs            # AIClient trait
│   │   ├── tool_calling.rs         # ToolCallingStrategy
│   │   ├── tokenizer.rs            # Tokenizer
│   │   ├── prompt_template.rs      # PromptTemplateLoader
│   │   ├── platform_adapter.rs     # PlatformAdapter
│   │   └── timing_gate.rs          # TimingGateStrategy
│   │
│   ├── core/                       # 核心运行时
│   │   ├── config.rs               # 配置系统
│   │   ├── runtime.rs              # BotRuntime 主循环
│   │   ├── dispatcher.rs           # EventDispatcher 事件分发
│   │   ├── session_pipeline.rs     # 每会话串行 worker
│   │   ├── reply_sender.rs         # ReplySender
│   │   ├── mood_engine.rs          # MoodEngine 情绪引擎
│   │   ├── scope.rs                # ChatScope 枚举
│   │   ├── errors.rs               # 统一错误类型 XueliError
│   │   ├── types.rs                # Conversation, MoodState 等核心类型
│   │   ├── platform_types.rs       # InboundEvent, ReplyAction, SessionRef
│   │   ├── metrics.rs              # RuntimeMetrics
│   │   └── lifecycle.rs            # 任务/资源生命周期管理
│   │
│   ├── services/                   # 外部服务
│   │   ├── ai_client.rs            # AIClient 默认 HTTP 实现
│   │   ├── vision_client.rs        # VLM 图片分析
│   │   ├── image_client.rs         # 图片下载/编码
│   │   ├── token_counter.rs        # TokenCounter
│   │   └── invocation_router.rs    # ModelInvocationRouter
│   │
│   ├── handlers/                   # 消息处理管线
│   │   ├── timing_gate.rs          # TimingGate 默认实现
│   │   ├── planner.rs              # ConversationPlanner
│   │   ├── plan_coordinator.rs     # ConversationPlanCoordinator
│   │   ├── context_builder.rs      # ConversationContextBuilder
│   │   ├── message_handler.rs      # MessageHandler 编排
│   │   ├── prompt_builder.rs       # ReplyAgent 提示词组装
│   │   ├── reply_agent.rs          # ReplyAgent 工具循环
│   │   ├── group_collector.rs      # GroupMessageCollector
│   │   └── image_pipeline.rs       # ImagePipeline
│   │
│   ├── memory/                     # 记忆系统
│   │   ├── manager.rs              # MemoryManager
│   │   ├── flow_service.rs         # MemoryFlowService
│   │   ├── recall_service.rs       # 记忆召回
│   │   ├── stores/                 # 5 个 SQLite Store
│   │   ├── retrieval/              # BM25 + 向量混合检索
│   │   ├── extraction/             # LLM 记忆提取
│   │   └── internal/               # 访问策略、索引协调
│   │
│   ├── signals/                    # 信号系统
│   │   ├── orchestrator.rs         # 信号编排
│   │   ├── engagement.rs           # 参与度
│   │   ├── metacognition.rs        # 元认知谨慎度
│   │   └── temporal.rs             # 时间信号
│   │
│   ├── character/                  # 角色系统
│   │   ├── card_service.rs         # 角色卡服务
│   │   └── narrative.rs            # 长期相处脉络
│   │
│   ├── emoji/                      # 表情系统
│   │   ├── database.rs             # 表情数据库
│   │   ├── manager.rs              # 表情管理
│   │   └── reply_service.rs        # 表情回复
│   │
│   ├── proactive_share/            # 主动分享
│   │   ├── scheduler.rs            # 调度器
│   │   └── store.rs                # 存储
│   │
│   └── util/                       # 工具
│       ├── crypto.rs               # SHA256
│       ├── id_gen.rs               # UUID 生成
│       └── time.rs                 # 时间工具
│
├── prompts/zh-CN/                  # 25 个中文提示词模板
├── examples/                       # 使用示例
└── tests/                          # 集成测试
```

## 许可

MIT