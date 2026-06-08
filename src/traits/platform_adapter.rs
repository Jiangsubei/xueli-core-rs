use async_trait::async_trait;

use crate::core::platform_types::InboundEvent;
use crate::prelude::XueliResult;

/// 平台适配器 trait — 下游实现各 IM 平台特有的消息收发
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
}
