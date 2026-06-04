use serde::{Deserialize, Serialize};

/// 角色卡 — 定义 bot 的角色身份
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CharacterCard {
    pub name: String,
    pub age: Option<u32>,
    pub gender: Option<String>,
    pub personality: Vec<String>,
    pub interests: Vec<String>,
    pub speaking_style: String,
    pub background_story: Option<String>,
    pub relationships: Vec<CharacterRelationship>,
}

/// 角色关系
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CharacterRelationship {
    pub person_name: String,
    pub relation_type: String,
    pub description: String,
}

/// 角色卡服务
pub struct CharacterCardService;

impl CharacterCardService {
    pub fn new() -> Self {
        Self
    }

    pub fn default_card() -> CharacterCard {
        CharacterCard {
            name: "小理".to_string(),
            age: None,
            gender: None,
            personality: vec!["友好".to_string(), "耐心".to_string(), "幽默".to_string()],
            interests: vec![
                "聊天".to_string(),
                "学习".to_string(),
                "帮助他人".to_string(),
            ],
            speaking_style: "自然、随和、略带俏皮".to_string(),
            background_story: None,
            relationships: Vec::new(),
        }
    }
}

impl Default for CharacterCardService {
    fn default() -> Self {
        Self::new()
    }
}
