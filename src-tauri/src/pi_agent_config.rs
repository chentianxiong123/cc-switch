//! Pi-Agent 配置文件读写模块
//!
//! 处理 `~/.pi/agent/models.json` 配置文件的读写操作。
//! pi-agent 使用累加式供应商管理，所有供应商配置共存于同一配置文件中。
//!
//! ## models.json 结构
//!
//! ```json
//! {
//!   "providers": {
//!     "provider-name": {
//!       "baseUrl": "https://api.example.com/v1",
//!       "apiKey": "$API_KEY_ENV_OR_VALUE",
//!       "api": "anthropic-messages | openai-completions | ...",
//!       "headers": { ... },
//!       "compat": { ... },
//!       "models": [
//!         {
//!           "id": "model-id",
//!           "name": "Model Name",
//!           "contextWindow": 128000,
//!           "maxTokens": 16384,
//!           "reasoning": false,
//!           "cost": {
//!             "input": 0,
//!             "output": 0,
//!             "cacheRead": 0,
//!             "cacheWrite": 0
//!           }
//!         }
//!       ],
//!       "modelOverrides": {
//!         "built-in-model-id": { "baseUrl": "..." }
//!       }
//!     }
//!   }
//! }
//! ```

use crate::config::write_json_file;
use crate::error::AppError;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::path::PathBuf;

// ============================================================================
// Path Functions
// ============================================================================

/// 获取 pi-agent 配置目录
///
/// 默认路径: `~/.pi/agent/`
/// 优先级: PI_CODING_AGENT_DIR 环境变量 > ~/.pi/agent
pub fn get_pi_agent_dir() -> PathBuf {
    // 支持 PI_CODING_AGENT_DIR 环境变量覆盖
    if let Ok(env_dir) = std::env::var("PI_CODING_AGENT_DIR") {
        let trimmed = env_dir.trim().to_string();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }

    crate::config::get_home_dir().join(".pi").join("agent")
}

/// 获取 pi-agent models.json 文件路径
///
/// 返回 `~/.pi/agent/models.json`
pub fn get_models_path() -> PathBuf {
    get_pi_agent_dir().join("models.json")
}

// ============================================================================
// Type Definitions
// ============================================================================

/// pi-agent 模型成本配置
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PiAgentModelCost {
    #[serde(default)]
    pub input: f64,
    #[serde(default)]
    pub output: f64,
    #[serde(default)]
    pub cache_read: f64,
    #[serde(default)]
    pub cache_write: f64,
}

/// pi-agent 单模型定义
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PiAgentModelEntry {
    pub id: String,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    /// API 类型（如 "openai-completions", "anthropic-messages"）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api: Option<String>,

    /// 可覆盖每个模型的 baseUrl
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<bool>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u32>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost: Option<PiAgentModelCost>,

    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}

/// pi-agent 供应商配置（对应 models.providers 中的条目）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PiAgentProviderConfig {
    /// 显示名称
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    /// API 基础 URL
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,

    /// API 密钥（支持 $ENV_VAR 引用或文字值）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,

    /// API 类型
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api: Option<String>,

    /// 自定义请求头
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub headers: HashMap<String, String>,

    /// 兼容性配置
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compat: Option<Value>,

    /// 自定义模型列表
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<PiAgentModelEntry>,

    /// 内置模型覆盖
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub model_overrides: HashMap<String, Value>,

    /// 是否在请求头中自动添加 Authorization: Bearer
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_header: Option<bool>,

    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}

// ============================================================================
// Core Read/Write Functions
// ============================================================================

/// 读取 pi-agent models.json 配置
///
/// 返回完整的配置 JSON 对象。文件不存在时返回默认空结构。
pub fn read_config() -> Result<Value, AppError> {
    let path = get_models_path();

    if !path.exists() {
        return Ok(json!({
            "providers": {}
        }));
    }

    let content = std::fs::read_to_string(&path).map_err(|e| AppError::io(&path, e))?;
    serde_json::from_str(&content)
        .map_err(|e| AppError::Config(format!("Failed to parse pi-agent models.json: {e}")))
}

/// 写回 pi-agent models.json
///
/// 原子写入，确保不损坏现有文件。
fn write_config(config: &Value) -> Result<(), AppError> {
    let path = get_models_path();

    // 确保父目录存在
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| AppError::io(parent, e))?;
    }

    write_json_file(&path, config)?;
    log::debug!("Pi-Agent models.json written to {path:?}");
    Ok(())
}

// ============================================================================
// Provider Functions (Untyped - for raw JSON operations)
// ============================================================================

/// 获取所有供应商配置（原始 JSON）
///
/// 从 `providers` 读取
pub fn get_providers() -> Result<Map<String, Value>, AppError> {
    let config = read_config()?;
    Ok(config
        .get("providers")
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_default())
}

/// 获取单个供应商配置（原始 JSON）
pub fn get_provider(id: &str) -> Result<Option<Value>, AppError> {
    Ok(get_providers()?.get(id).cloned())
}

/// 设置供应商配置（原始 JSON）
///
/// 写入到 `providers`
pub fn set_provider(id: &str, provider_config: Value) -> Result<(), AppError> {
    let mut config = read_config()?;

    if config.get("providers").is_none() {
        config["providers"] = json!({});
    }

    if let Some(providers) = config
        .get_mut("providers")
        .and_then(|v| v.as_object_mut())
    {
        providers.insert(id.to_string(), provider_config);
    }

    write_config(&config)
}

/// 删除供应商配置
pub fn remove_provider(id: &str) -> Result<(), AppError> {
    let mut config = read_config()?;

    if let Some(providers) = config
        .get_mut("providers")
        .and_then(|v| v.as_object_mut())
    {
        providers.remove(id);
    }

    write_config(&config)
}

// ============================================================================
// Provider Functions (Typed)
// ============================================================================

/// 获取所有供应商配置（类型化）
pub fn get_typed_providers() -> Result<HashMap<String, PiAgentProviderConfig>, AppError> {
    let providers = get_providers()?;
    let mut result = HashMap::new();

    for (id, value) in providers {
        match serde_json::from_value::<PiAgentProviderConfig>(value) {
            Ok(config) => {
                result.insert(id, config);
            }
            Err(e) => {
                log::warn!("Failed to parse pi-agent provider '{id}': {e}");
            }
        }
    }

    Ok(result)
}

/// 设置供应商配置（类型化）
pub fn set_typed_provider(id: &str, config: &PiAgentProviderConfig) -> Result<(), AppError> {
    let value = serde_json::to_value(config).map_err(|e| AppError::JsonSerialize { source: e })?;
    set_provider(id, value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs;

    fn with_test_home<T>(test_fn: impl FnOnce() -> T) -> T {
        let tmp = tempfile::tempdir().unwrap();
        let old_test_home = std::env::var_os("CC_SWITCH_TEST_HOME");
        let old_home = std::env::var_os("HOME");
        std::env::set_var("CC_SWITCH_TEST_HOME", tmp.path());
        std::env::set_var("HOME", tmp.path());
        let result = test_fn();
        match old_test_home {
            Some(v) => std::env::set_var("CC_SWITCH_TEST_HOME", v),
            None => std::env::remove_var("CC_SWITCH_TEST_HOME"),
        }
        match old_home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
        result
    }

    #[test]
    fn read_config_returns_empty_providers_when_file_missing() {
        with_test_home(|| {
            let config = read_config().unwrap();
            let providers = config.get("providers").unwrap();
            assert!(providers.as_object().unwrap().is_empty());
        });
    }

    #[test]
    fn provider_crud_roundtrip() {
        with_test_home(|| {
            // Initially no providers
            let providers = get_providers().unwrap();
            assert!(providers.is_empty());

            // Add a provider
            let config = json!({
                "baseUrl": "https://api.example.com/v1",
                "apiKey": "sk-test",
                "api": "openai-completions",
                "models": [
                    { "id": "test-model", "name": "Test Model" }
                ]
            });
            set_provider("test", config).unwrap();

            let providers = get_providers().unwrap();
            assert_eq!(providers.len(), 1);
            assert!(providers.contains_key("test"));

            let provider = get_provider("test").unwrap().unwrap();
            assert_eq!(provider["baseUrl"], "https://api.example.com/v1");
            assert_eq!(provider["apiKey"], "sk-test");

            // Remove the provider
            remove_provider("test").unwrap();
            let providers = get_providers().unwrap();
            assert!(providers.is_empty());
        });
    }

    #[test]
    fn typed_provider_roundtrip() {
        with_test_home(|| {
            let config = PiAgentProviderConfig {
                name: Some("Test Provider".to_string()),
                base_url: Some("https://api.test.com/v1".to_string()),
                api_key: Some("sk-test".to_string()),
                api: Some("openai-completions".to_string()),
                headers: HashMap::new(),
                compat: None,
                models: vec![PiAgentModelEntry {
                    id: "gpt-4".to_string(),
                    name: Some("GPT-4".to_string()),
                    api: None,
                    base_url: None,
                    reasoning: None,
                    context_window: Some(8192),
                    max_tokens: Some(4096),
                    cost: None,
                    extra: HashMap::new(),
                }],
                model_overrides: HashMap::new(),
                auth_header: None,
                extra: HashMap::new(),
            };

            set_typed_provider("test", &config).unwrap();

            let providers = get_typed_providers().unwrap();
            let read = providers.get("test").unwrap();
            assert_eq!(read.base_url.as_deref(), Some("https://api.test.com/v1"));
            assert_eq!(read.api_key.as_deref(), Some("sk-test"));
            assert_eq!(read.models.len(), 1);
            assert_eq!(read.models[0].id, "gpt-4");
            assert_eq!(
                read.models[0].context_window,
                Some(8192)
            );
        });
    }
}
