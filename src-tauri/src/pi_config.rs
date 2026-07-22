use crate::config::write_json_file;
use crate::error::AppError;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::path::PathBuf;

pub fn get_pi_dir() -> PathBuf {
    if let Ok(env_dir) = std::env::var("PI_CODING_AGENT_DIR") {
        let trimmed = env_dir.trim().to_string();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }
    crate::config::home_dir()
        .unwrap_or_else(|| PathBuf::from("~"))
        .join(".pi")
        .join("agent")
}

pub fn get_models_path() -> PathBuf {
    get_pi_dir().join("models.json")
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PiModelCost {
    #[serde(default)]
    pub input: f64,
    #[serde(default)]
    pub output: f64,
    #[serde(default)]
    pub cache_read: f64,
    #[serde(default)]
    pub cache_write: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PiModelEntry {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost: Option<PiModelCost>,
    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PiProviderConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api: Option<String>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub headers: HashMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compat: Option<Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<PiModelEntry>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub model_overrides: HashMap<String, Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_header: Option<bool>,
    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}

pub fn read_config() -> Result<Value, AppError> {
    let path = get_models_path();
    if !path.exists() {
        return Ok(json!({ "providers": {} }));
    }
    let content = std::fs::read_to_string(&path).map_err(|e| AppError::io(&path, e))?;
    serde_json::from_str(&content)
        .map_err(|e| AppError::Config(format!("Failed to parse pi models.json: {e}")))
}

fn write_config(config: &Value) -> Result<(), AppError> {
    let path = get_models_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| AppError::io(parent, e))?;
    }
    write_json_file(&path, config)?;
    log::debug!("Pi models.json written to {path:?}");
    Ok(())
}

pub fn get_providers() -> Result<Map<String, Value>, AppError> {
    let config = read_config()?;
    Ok(config
        .get("providers")
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_default())
}

pub fn get_provider(id: &str) -> Result<Option<Value>, AppError> {
    Ok(get_providers()?.get(id).cloned())
}

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

pub fn get_typed_providers() -> Result<HashMap<String, PiProviderConfig>, AppError> {
    let providers = get_providers()?;
    let mut result = HashMap::new();
    for (id, value) in providers {
        match serde_json::from_value::<PiProviderConfig>(value) {
            Ok(config) => {
                result.insert(id, config);
            }
            Err(e) => {
                log::warn!("Failed to parse pi provider '{id}': {e}");
            }
        }
    }
    Ok(result)
}

pub fn set_typed_provider(id: &str, config: &PiProviderConfig) -> Result<(), AppError> {
    let value = serde_json::to_value(config).map_err(|e| AppError::JsonSerialize { source: e })?;
    set_provider(id, value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn read_config_returns_empty_providers_when_file_missing() {
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("PI_CODING_AGENT_DIR", tmp.path());
        let config = read_config().unwrap();
        let providers = config.get("providers").unwrap();
        assert!(providers.as_object().unwrap().is_empty());
        std::env::remove_var("PI_CODING_AGENT_DIR");
    }

    #[test]
    fn provider_crud_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("PI_CODING_AGENT_DIR", tmp.path());
        let providers = get_providers().unwrap();
        assert!(providers.is_empty());

        let config = json!({
            "baseUrl": "https://api.example.com/v1",
            "apiKey": "sk-test",
            "api": "openai-completions",
        });
        set_provider("test", config).unwrap();
        let providers = get_providers().unwrap();
        assert_eq!(providers.len(), 1);

        remove_provider("test").unwrap();
        let providers = get_providers().unwrap();
        assert!(providers.is_empty());
        std::env::remove_var("PI_CODING_AGENT_DIR");
    }
}
