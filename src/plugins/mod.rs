/// WASM 插件系统（架构设计阶段）
///
/// 设计目标：
/// - 安全隔离：插件在 WASM 沙箱中运行，无法直接访问宿主系统资源
/// - 语言无关：插件可用 Rust/AssemblyScript/Go 等编译为 WASM
/// - 热插拔：支持运行时加载/卸载/重载插件
/// - 最小权限：通过 capability 模型控制插件可访问的 API
///
/// 核心架构：
/// ┌─────────────────────────────────────────────┐
/// │  Host (xueli-core)                          │
/// │  ┌─────────────┐  ┌──────────────────────┐ │
/// │  │ PluginHost  │  │ PluginRegistry       │ │
/// │  │ (wasmtime)  │  │ (插件元数据管理)      │ │
/// │  └──────┬──────┘  └──────────────────────┘ │
/// │         │ WASI / custom ABI                 │
/// ├─────────┼───────────────────────────────────┤
/// │  WASM   │  Guest (plugin.wasm)              │
/// │  Sandbox│  ┌──────────┐ ┌──────────────┐   │
/// │         │  │ Plugin   │ │ PluginAPI    │   │
/// │         │  │ (业务逻辑)│ │ (宿主提供的API)│  │
/// │         │  └──────────┘ └──────────────┘   │
/// └─────────┴───────────────────────────────────┘
///
/// 扩展点（Extension Points）：
/// - MessageInterceptor: 拦截并修改入站/出站消息
/// - CommandProvider:    注册自定义命令（如 /weather）
/// - MemoryProcessor:    自定义记忆提取/处理逻辑
/// - ReplyModifier:      修改生成回复前的上下文或回复后处理
/// - PlatformBridge:     对接新平台协议
use std::collections::HashMap;
use std::sync::Arc;

use crate::prelude::XueliResult;

/// 插件元数据
#[derive(Debug, Clone)]
pub struct PluginManifest {
    /// 插件唯一标识（反向域名格式，如 com.example.weather）
    pub id: String,
    /// 插件名称
    pub name: String,
    /// 版本号（semver）
    pub version: String,
    /// 作者
    pub author: String,
    /// 描述
    pub description: String,
    /// 声明的扩展点
    pub extension_points: Vec<ExtensionPoint>,
    /// 请求的权限列表
    pub permissions: Vec<Permission>,
    /// 依赖的其他插件
    pub dependencies: Vec<String>,
}

/// 扩展点类型
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ExtensionPoint {
    /// 消息拦截器
    MessageInterceptor,
    /// 命令提供者
    CommandProvider,
    /// 记忆处理器
    MemoryProcessor,
    /// 回复修饰器
    ReplyModifier,
    /// 平台桥接器
    PlatformBridge,
    /// 定时任务
    ScheduledTask,
}

/// 插件权限
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Permission {
    /// 读取消息内容
    ReadMessages,
    /// 发送消息
    SendMessages,
    /// 访问记忆系统（只读）
    ReadMemory,
    /// 修改记忆系统
    WriteMemory,
    /// 访问配置
    ReadConfig,
    /// 调用外部 HTTP API
    HttpRequest,
    /// 访问文件系统（通过 WASI）
    FileSystem,
}

/// 插件状态
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginState {
    /// 已注册但未加载
    Registered,
    /// 正在加载
    Loading,
    /// 已激活运行
    Active,
    /// 加载/初始化失败
    Failed,
    /// 已卸载
    Unloaded,
}

/// 插件实例（运行时句柄）
pub struct PluginInstance {
    pub manifest: PluginManifest,
    pub state: PluginState,
    // 实际实现中将持有 wasmtime::Instance 或类似句柄
    // _wasm_instance: wasmtime::Instance,
}

/// 插件注册表 — 管理所有插件的生命周期
pub struct PluginRegistry {
    plugins: HashMap<String, Arc<PluginInstance>>,
}

impl PluginRegistry {
    pub fn new() -> Self {
        Self {
            plugins: HashMap::new(),
        }
    }

    /// 注册插件（仅记录元数据，不加载 WASM）
    pub fn register(&mut self, manifest: PluginManifest) {
        let id = manifest.id.clone();
        let instance = Arc::new(PluginInstance {
            manifest,
            state: PluginState::Registered,
        });
        self.plugins.insert(id, instance);
    }

    /// 按扩展点查询已激活的插件
    pub fn find_by_extension(&self, point: ExtensionPoint) -> Vec<&Arc<PluginInstance>> {
        self.plugins
            .values()
            .filter(|p| p.manifest.extension_points.contains(&point))
            .collect()
    }

    /// 获取插件状态
    pub fn get_state(&self, plugin_id: &str) -> Option<PluginState> {
        self.plugins.get(plugin_id).map(|p| p.state)
    }

    /// 卸载插件
    pub fn unload(&mut self, plugin_id: &str) -> bool {
        self.plugins.remove(plugin_id).is_some()
    }
}

impl Default for PluginRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// 插件宿主 — 负责 WASM 运行时和 ABI 绑定
///
/// 未来实现将基于 wasmtime 或 wasmer：
/// - 创建 Engine + Store
/// - 实例化 Module
/// - 绑定宿主函数到 Guest 导入表
/// - 调用 Guest 导出的生命周期函数（init / on_message / on_command 等）
pub struct PluginHost;

impl PluginHost {
    pub fn new() -> Self {
        Self
    }

    /// 加载 WASM 字节码并实例化（占位实现）
    pub fn load(&self, _wasm_bytes: &[u8], _manifest: &PluginManifest) -> XueliResult<()> {
        // TODO: 接入 wasmtime / wasmer
        tracing::info!("PluginHost::load 占位 — WASM 运行时未接入");
        Ok(())
    }

    /// 调用插件初始化函数
    pub fn init(&self, _plugin_id: &str) -> XueliResult<()> {
        tracing::info!("PluginHost::init 占位");
        Ok(())
    }

    /// 调用插件卸载清理
    pub fn shutdown(&self, _plugin_id: &str) -> XueliResult<()> {
        tracing::info!("PluginHost::shutdown 占位");
        Ok(())
    }
}

impl Default for PluginHost {
    fn default() -> Self {
        Self::new()
    }
}

/// 插件配置（对应配置文件 [plugins] 段）
#[derive(Debug, Clone, Default)]
pub struct PluginConfig {
    /// 插件搜索路径列表
    pub plugin_dirs: Vec<String>,
    /// 默认启用/禁用
    pub default_enabled: bool,
    /// 按插件 ID 的显式启用配置
    pub enabled: HashMap<String, bool>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_registry_register_and_find() {
        let mut registry = PluginRegistry::new();
        let manifest = PluginManifest {
            id: "test.echo".to_string(),
            name: "Echo Plugin".to_string(),
            version: "1.0.0".to_string(),
            author: "test".to_string(),
            description: "Echo messages back".to_string(),
            extension_points: vec![ExtensionPoint::MessageInterceptor],
            permissions: vec![Permission::ReadMessages, Permission::SendMessages],
            dependencies: vec![],
        };
        registry.register(manifest);

        let found = registry.find_by_extension(ExtensionPoint::MessageInterceptor);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].manifest.id, "test.echo");

        let none = registry.find_by_extension(ExtensionPoint::CommandProvider);
        assert!(none.is_empty());
    }

    #[test]
    fn test_registry_state() {
        let mut registry = PluginRegistry::new();
        registry.register(PluginManifest {
            id: "test.plugin".to_string(),
            name: "Test".to_string(),
            version: "0.1.0".to_string(),
            author: "test".to_string(),
            description: "Test plugin".to_string(),
            extension_points: vec![],
            permissions: vec![],
            dependencies: vec![],
        });

        assert_eq!(
            registry.get_state("test.plugin"),
            Some(PluginState::Registered)
        );
        assert_eq!(registry.get_state("nonexistent"), None);
    }

    #[test]
    fn test_registry_unload() {
        let mut registry = PluginRegistry::new();
        registry.register(PluginManifest {
            id: "test.plugin".to_string(),
            name: "Test".to_string(),
            version: "0.1.0".to_string(),
            author: "test".to_string(),
            description: "Test plugin".to_string(),
            extension_points: vec![],
            permissions: vec![],
            dependencies: vec![],
        });

        assert!(registry.unload("test.plugin"));
        assert!(!registry.unload("test.plugin"));
    }
}
