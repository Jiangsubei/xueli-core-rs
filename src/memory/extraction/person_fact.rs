use crate::memory::stores::person_fact::PersonFact;

/// 人物事实服务 — 从对话中提取人物事实
pub struct PersonFactService;

impl PersonFactService {
    pub fn new() -> Self {
        Self
    }

    pub async fn extract_facts(
        &self,
        _user_id: &str,
        _messages: &[String],
    ) -> Result<Vec<PersonFact>, String> {
        // TODO: 集成 LLM 调用提取人物事实
        Ok(Vec::new())
    }
}

impl Default for PersonFactService {
    fn default() -> Self {
        Self::new()
    }
}