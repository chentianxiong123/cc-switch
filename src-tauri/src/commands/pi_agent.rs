use tauri::State;

use crate::pi_agent_config;
use crate::store::AppState;

// ============================================================================
// Pi-Agent Provider Commands
// ============================================================================

/// Import providers from pi-agent live config to database.
///
/// pi-agent uses additive mode — users may already have providers
/// configured in models.json.
#[tauri::command]
pub fn import_pi_agent_providers_from_live(state: State<'_, AppState>) -> Result<usize, String> {
    crate::services::provider::import_pi_agent_providers_from_live(state.inner())
        .map_err(|e| e.to_string())
}

/// Get provider IDs in the pi-agent live config.
#[tauri::command]
pub fn get_pi_agent_live_provider_ids() -> Result<Vec<String>, String> {
    pi_agent_config::get_providers()
        .map(|providers| providers.keys().cloned().collect())
        .map_err(|e| e.to_string())
}

/// Get a single pi-agent provider from live config.
#[tauri::command]
pub fn get_pi_agent_live_provider(
    #[allow(non_snake_case)] providerId: String,
) -> Result<Option<serde_json::Value>, String> {
    pi_agent_config::get_provider(&providerId).map_err(|e| e.to_string())
}

/// Write a provider to pi-agent's models.json.
#[tauri::command]
pub fn set_pi_agent_live_provider(
    #[allow(non_snake_case)] providerId: String,
    #[allow(non_snake_case)] providerConfig: serde_json::Value,
) -> Result<(), String> {
    pi_agent_config::set_provider(&providerId, providerConfig).map_err(|e| e.to_string())
}

/// Remove a provider from pi-agent's models.json.
#[tauri::command]
pub fn remove_pi_agent_live_provider(
    #[allow(non_snake_case)] providerId: String,
) -> Result<(), String> {
    pi_agent_config::remove_provider(&providerId).map_err(|e| e.to_string())
}

/// Read pi-agent's full models.json config.
#[tauri::command]
pub fn get_pi_agent_config() -> Result<serde_json::Value, String> {
    pi_agent_config::read_config().map_err(|e| e.to_string())
}
