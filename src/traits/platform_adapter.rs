use async_trait::async_trait;

use crate::core::platform_types::InboundEvent;
use crate::prelude::XueliResult;

/// 平台适配器 trait — 下游实现各 IM 平台特有的消息收发
///
/// # 最小实现契约
///
/// 1. **消息发送**：`send_action` 必须将 `ReplyAction` 翻译为平台原生调用并发出。
/// 2. **事件解析**：`parse_event` 必须将平台原始事件字符串转换为平台无关的
///    [`InboundEvent`]；无法识别的事件应返回错误，由调用方决定是丢弃还是走兼容路径。
/// 3. **提及处理**：`strip_mentions`、`extract_mentions`、
///    `resolve_mention_placeholders` 负责平台特定的 @提及 剥离、提取与占位符替换。
/// 4. **生命周期（可选）**：`run`、`disconnect`、`is_ready` 提供适配器的启动、
///    断开与就绪状态抽象。core 层不强制要求适配器必须实现事件循环，因此这三个
///    方法都带有默认空实现：
///    - `run` 默认立即返回 `Ok(())`，表示 core 不托管适配器的事件循环。
///    - `disconnect` 默认立即返回 `Ok(())`。
///    - `is_ready` 默认返回 `true`。
///
///    若下游适配器需要被 core 统一启动/关闭（例如 WebSocket / HTTP 长连接），
///    应覆盖这些方法；否则事件循环可由下游进程自行负责，core 只通过本 trait 的
///    `send_action` / `parse_event` 与适配器交互。
///
/// # 平台无关性保证
///
/// 本 trait 的所有方法签名只能使用 core 层定义的统一类型
/// （`ReplyAction`、`InboundEvent`、`XueliResult` 等），不得出现 QQ / OneBot /
/// Discord 等平台专有类型，确保 `xueli-core` 不被具体平台细节污染。
#[async_trait]
pub trait PlatformAdapter: Send + Sync {
    /// 发送回复动作
    async fn send_action(
        &self,
        action: &crate::core::platform_types::ReplyAction,
    ) -> XueliResult<()>;

    /// 去除消息中的 @提及
    fn strip_mentions(&self, text: &str) -> String;

    /// 提取消息中提到的用户 ID 列表
    fn extract_mentions(&self, event: &InboundEvent) -> Vec<String>;

    /// 将平台特定的 mention 占位符替换为显示名称
    fn resolve_mention_placeholders(&self, text: &str, mentions: &[String]) -> String;

    /// 获取平台名称标识
    fn platform_name(&self) -> &str;

    /// 解析原始事件为统一格式
    fn parse_event(&self, raw: &str) -> XueliResult<InboundEvent>;

    /// 启动适配器事件循环
    ///
    /// 默认实现为空操作（立即返回 `Ok(())`）。下游若需要 core 统一启动连接
    /// （如 WebSocket / HTTP 上报监听），应覆盖此方法。
    async fn run(&self) -> XueliResult<()> {
        Ok(())
    }

    /// 断开适配器连接并释放相关资源
    ///
    /// 默认实现为空操作（立即返回 `Ok(())`）。
    async fn disconnect(&self) -> XueliResult<()> {
        Ok(())
    }

    /// 检查适配器是否已就绪（连接已建立）
    ///
    /// 默认实现返回 `true`，表示适配器始终就绪。下游若需要核心层感知连接状态，
    /// 应覆盖此方法。
    fn is_ready(&self) -> bool {
        true
    }
}
