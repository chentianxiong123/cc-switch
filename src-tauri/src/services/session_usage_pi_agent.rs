//! PiAgent 会话日志使用追踪
//!
//! 从 ~/.pi/agent/sessions/ 下的 JSONL 会话文件中提取 token 使用数据。
//!
//! ## 数据流
//! ```text
//! ~/.pi/agent/sessions/*/*.jsonl → 增量解析 → 去重 → 费用计算 → proxy_request_logs 表
//! ```
//!
//! ## PiAgent Session 格式
//! ```json
//! {"type":"session","version":3,"id":"uuid","timestamp":"...","cwd":"/path"}
//! {"type":"message","id":"...","parentId":"...","timestamp":"...",
//!  "message":{"role":"assistant","provider":"anthropic","model":"claude-sonnet-4-5",
//!             "usage":{...},"stopReason":"stop"}}
//! ```
//!
//! 解析方式与 Claude Code session_usage.rs 类似，区别在于 session 头格式和
//! 会话文件存放路径不同。

use crate::database::{lock_conn, Database};
use crate::error::AppError;
use crate::pi_agent_config::get_pi_agent_dir;
use crate::proxy::usage::calculator::CostCalculator;
use crate::proxy::usage::parser::TokenUsage;
use crate::services::session_usage::{
    get_sync_state, metadata_modified_nanos, update_sync_state, SessionSyncResult,
};
use crate::services::usage_stats::{find_model_pricing, should_skip_session_insert, DedupKey};
use rust_decimal::Decimal;
use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// 从 PiAgent JSONL 中解析出的 assistant 消息使用数据
#[derive(Debug)]
struct PiAgentAssistantUsage {
    message_id: String,
    model: String,
    provider: Option<String>,
    input_tokens: u32,
    output_tokens: u32,
    cache_read_tokens: u32,
    cache_creation_tokens: u32,
    stop_reason: Option<String>,
    timestamp: Option<String>,
    session_id: Option<String>,
}

/// 同步 PiAgent 会话日志到使用统计数据库
pub fn sync_pi_agent_usage(db: &Database) -> Result<SessionSyncResult, AppError> {
    let sessions_dir = get_pi_agent_dir().join("sessions");
    if !sessions_dir.exists() {
        return Ok(SessionSyncResult {
            imported: 0,
            skipped: 0,
            files_scanned: 0,
            errors: vec![],
        });
    }

    let mut result = SessionSyncResult {
        imported: 0,
        skipped: 0,
        files_scanned: 0,
        errors: vec![],
    };

    // 收集所有 .jsonl 文件
    let jsonl_files = collect_jsonl_files(&sessions_dir);

    for file_path in &jsonl_files {
        result.files_scanned += 1;

        match sync_single_file(db, file_path) {
            Ok((imported, skipped)) => {
                result.imported += imported;
                result.skipped += skipped;
            }
            Err(e) => {
                let msg = format!("{}: {e}", file_path.display());
                log::warn!("[PIAGENT-SYNC] 文件解析失败: {msg}");
                result.errors.push(msg);
            }
        }
    }

    if result.imported > 0 {
        log::info!(
            "[PIAGENT-SYNC] 同步完成: 导入 {} 条, 跳过 {} 条, 扫描 {} 个文件",
            result.imported,
            result.skipped,
            result.files_scanned
        );
    }

    Ok(result)
}

/// 收集目录下所有 .jsonl 文件（仅一层子目录）
///
/// PiAgent 的 session 文件结构：
///   sessions_dir/<path-hash>/<timestamp>_<uuid>.jsonl
fn collect_jsonl_files(sessions_dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();

    let entries = match fs::read_dir(sessions_dir) {
        Ok(e) => e,
        Err(_) => return files,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        // 每个子目录下的 .jsonl 文件
        if let Ok(sub_entries) = fs::read_dir(&path) {
            for sub_entry in sub_entries.flatten() {
                let sub_path = sub_entry.path();
                if sub_path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                    files.push(sub_path);
                }
            }
        }
    }

    files
}

/// 同步单个 JSONL 文件，返回 (imported, skipped)
fn sync_single_file(db: &Database, file_path: &Path) -> Result<(u32, u32), AppError> {
    let file_path_str = file_path.to_string_lossy().to_string();

    // 获取文件元数据
    let metadata = fs::metadata(file_path)
        .map_err(|e| AppError::Config(format!("无法读取文件元数据: {e}")))?;
    let file_modified = metadata_modified_nanos(&metadata);

    // 检查同步状态
    let (last_modified, last_offset) = get_sync_state(db, &file_path_str)?;

    // 文件未变化则跳过
    if file_modified <= last_modified {
        return Ok((0, 0));
    }

    // 从上次偏移位置开始增量解析
    let file = fs::File::open(file_path)
        .map_err(|e| AppError::Config(format!("无法打开文件: {e}")))?;
    let reader = BufReader::new(file);

    let mut line_offset: i64 = 0;
    let mut messages: HashMap<String, PiAgentAssistantUsage> = HashMap::new();
    let mut current_session_id: Option<String> = None;

    for line_result in reader.lines() {
        line_offset += 1;

        // 跳过已处理的行
        if line_offset <= last_offset {
            continue;
        }

        let line = match line_result {
            Ok(l) => l,
            Err(_) => continue, // 容忍不完整的最后一行
        };

        if line.trim().is_empty() {
            continue;
        }

        let value: serde_json::Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let entry_type = match value.get("type").and_then(|t| t.as_str()) {
            Some(t) => t,
            None => continue,
        };

        // 提取 session ID（从 session 头）
        if entry_type == "session" {
            if let Some(sid) = value.get("id").and_then(|v| v.as_str()) {
                current_session_id = Some(sid.to_string());
            }
            continue;
        }

        // 只处理 message 类型的条目
        if entry_type != "message" {
            continue;
        }

        // 只处理 assistant 消息
        let message = match value.get("message") {
            Some(m) => m,
            None => continue,
        };

        let role = match message.get("role").and_then(|v| v.as_str()) {
            Some(r) => r,
            None => continue,
        };

        if role != "assistant" {
            continue;
        }

        let msg_id = match value.get("id").and_then(|v| v.as_str()) {
            Some(id) => id.to_string(),
            None => continue,
        };

        let usage = match message.get("usage") {
            Some(u) => u,
            None => continue,
        };

        let parsed = PiAgentAssistantUsage {
            message_id: msg_id.clone(),
            model: message
                .get("model")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string(),
            provider: message
                .get("provider")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            input_tokens: usage
                .get("input_tokens")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32,
            output_tokens: usage
                .get("output_tokens")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32,
            cache_read_tokens: usage
                .get("cache_read_input_tokens")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32,
            cache_creation_tokens: usage
                .get("cache_creation_input_tokens")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32,
            stop_reason: message
                .get("stopReason")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            timestamp: value
                .get("timestamp")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            session_id: current_session_id.clone(),
        };

        // 跳过全零 token 的消息
        if parsed.input_tokens == 0
            && parsed.output_tokens == 0
            && parsed.cache_read_tokens == 0
            && parsed.cache_creation_tokens == 0
        {
            continue;
        }

        // 按 message.id 去重：优先保留有 stop_reason 的条目，否则保留最新的
        let should_replace = match messages.get(&msg_id) {
            None => true,
            Some(existing) => {
                if parsed.stop_reason.is_some() && existing.stop_reason.is_none() {
                    true
                } else if parsed.stop_reason.is_some() == existing.stop_reason.is_some() {
                    parsed.output_tokens > existing.output_tokens
                } else {
                    false
                }
            }
        };

        if should_replace {
            messages.insert(msg_id, parsed);
        }
    }

    // 写入数据库
    let mut imported: u32 = 0;
    let mut skipped: u32 = 0;

    for msg in messages.values() {
        let has_billable_tokens = msg.input_tokens > 0
            || msg.output_tokens > 0
            || msg.cache_read_tokens > 0
            || msg.cache_creation_tokens > 0;
        if !has_billable_tokens {
            continue;
        }

        let request_id = format!("pi_agent_session:{}", msg.message_id);

        match insert_session_log_entry(db, &request_id, msg) {
            Ok(true) => imported += 1,
            Ok(false) => skipped += 1,
            Err(e) => {
                log::warn!("[PIAGENT-SYNC] 插入失败 ({}): {e}", msg.message_id);
                skipped += 1;
            }
        }
    }

    // 更新同步状态
    update_sync_state(db, &file_path_str, file_modified, line_offset)?;

    Ok((imported, skipped))
}

/// 插入单条 PiAgent 会话日志到 proxy_request_logs
fn insert_session_log_entry(
    db: &Database,
    request_id: &str,
    msg: &PiAgentAssistantUsage,
) -> Result<bool, AppError> {
    let conn = lock_conn!(db.conn);

    let created_at = msg
        .timestamp
        .as_ref()
        .and_then(|ts| {
            chrono::DateTime::parse_from_rfc3339(ts)
                .ok()
                .map(|dt| dt.timestamp())
        })
        .unwrap_or_else(|| {
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0)
        });

    let dedup_key = DedupKey {
        app_type: "pi-agent",
        model: &msg.model,
        input_tokens: msg.input_tokens,
        output_tokens: msg.output_tokens,
        cache_read_tokens: msg.cache_read_tokens,
        cache_creation_tokens: msg.cache_creation_tokens,
        created_at,
    };
    if should_skip_session_insert(&conn, request_id, &dedup_key)? {
        return Ok(false);
    }

    // 计算费用
    let usage = TokenUsage {
        input_tokens: msg.input_tokens,
        output_tokens: msg.output_tokens,
        cache_read_tokens: msg.cache_read_tokens,
        cache_creation_tokens: msg.cache_creation_tokens,
        model: Some(msg.model.clone()),
        message_id: None,
    };

    let pricing = find_model_pricing(&conn, &msg.model);
    let multiplier = Decimal::from(1);
    let (input_cost, output_cost, cache_read_cost, cache_creation_cost, total_cost) = match pricing {
        Some(p) => {
            let cost = CostCalculator::calculate(&usage, &p, multiplier);
            (
                cost.input_cost.to_string(),
                cost.output_cost.to_string(),
                cost.cache_read_cost.to_string(),
                cost.cache_creation_cost.to_string(),
                cost.total_cost.to_string(),
            )
        }
        None => (
            "0".to_string(),
            "0".to_string(),
            "0".to_string(),
            "0".to_string(),
            "0".to_string(),
        ),
    };

    let inserted_rows = conn
        .execute(
            "INSERT OR IGNORE INTO proxy_request_logs (
            request_id, provider_id, app_type, model, request_model,
            input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens,
            input_cost_usd, output_cost_usd, cache_read_cost_usd, cache_creation_cost_usd, total_cost_usd,
            latency_ms, first_token_ms, status_code, error_message, session_id,
            provider_type, is_streaming, cost_multiplier, created_at, data_source
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24)",
            rusqlite::params![
                request_id,
                msg.provider.as_deref().unwrap_or("_pi_agent_session"),
                "pi-agent",
                msg.model,
                msg.model,
                msg.input_tokens,
                msg.output_tokens,
                msg.cache_read_tokens,
                msg.cache_creation_tokens,
                input_cost,
                output_cost,
                cache_read_cost,
                cache_creation_cost,
                total_cost,
                0i64,
                Option::<i64>::None,
                200i64,
                Option::<String>::None,
                msg.session_id,
                Some("pi_agent_session"),
                1i64,
                "1.0",
                created_at,
                "pi_agent_session",
            ],
        )
        .map_err(|e| AppError::Database(format!("插入 PiAgent 会话日志失败: {e}")))?;

    if inserted_rows > 0 {
        crate::usage_events::notify_log_recorded();
    }

    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_usage_from_jsonl_line() {
        let line = r#"{"type":"message","id":"msg_test123","parentId":"prev456","timestamp":"2026-07-14T12:00:00Z","message":{"role":"assistant","provider":"anthropic","model":"claude-opus-4-8","usage":{"input_tokens":3,"output_tokens":150,"cache_read_input_tokens":5000,"cache_creation_input_tokens":10000},"stopReason":"end_turn"}}"#;

        let value: serde_json::Value = serde_json::from_str(line).unwrap();
        assert_eq!(
            value.get("type").and_then(|t| t.as_str()),
            Some("message")
        );

        let message = value.get("message").unwrap();
        let usage = message.get("usage").unwrap();

        assert_eq!(usage.get("input_tokens").unwrap().as_u64().unwrap(), 3);
        assert_eq!(usage.get("output_tokens").unwrap().as_u64().unwrap(), 150);
        assert_eq!(
            usage
                .get("cache_read_input_tokens")
                .unwrap()
                .as_u64()
                .unwrap(),
            5000
        );
        assert_eq!(
            usage
                .get("cache_creation_input_tokens")
                .unwrap()
                .as_u64()
                .unwrap(),
            10000
        );
        assert_eq!(
            message.get("stopReason").unwrap().as_str().unwrap(),
            "end_turn"
        );
    }

    #[test]
    fn test_parse_session_header() {
        let line = r#"{"type":"session","version":3,"id":"uuid-abc-123","timestamp":"2026-07-14T12:00:00Z","cwd":"/home/user/project"}"#;
        let value: serde_json::Value = serde_json::from_str(line).unwrap();
        assert_eq!(
            value.get("type").and_then(|t| t.as_str()),
            Some("session")
        );
        assert_eq!(
            value.get("id").and_then(|v| v.as_str()),
            Some("uuid-abc-123")
        );
    }

    #[test]
    fn test_skip_user_messages() {
        let line = r#"{"type":"message","id":"msg_user","parentId":"prev","timestamp":"2026-07-14T12:00:00Z","message":{"role":"user","content":"Hello"}}"#;
        let value: serde_json::Value = serde_json::from_str(line).unwrap();
        let message = value.get("message").unwrap();
        let role = message.get("role").and_then(|v| v.as_str()).unwrap();
        assert_eq!(role, "user");
        // user 消息没有 usage，应该跳过
        assert!(message.get("usage").is_none());
    }

    #[test]
    fn test_dedup_by_message_id() {
        let mut messages: HashMap<String, PiAgentAssistantUsage> = HashMap::new();

        let intermediate = PiAgentAssistantUsage {
            message_id: "msg_1".to_string(),
            model: "claude-opus-4-8".to_string(),
            provider: Some("anthropic".to_string()),
            input_tokens: 3,
            output_tokens: 26,
            cache_read_tokens: 5000,
            cache_creation_tokens: 10000,
            stop_reason: None,
            timestamp: Some("2026-07-14T12:00:00Z".to_string()),
            session_id: None,
        };
        messages.insert("msg_1".to_string(), intermediate);

        let final_entry = PiAgentAssistantUsage {
            message_id: "msg_1".to_string(),
            model: "claude-opus-4-8".to_string(),
            provider: Some("anthropic".to_string()),
            input_tokens: 3,
            output_tokens: 1349,
            cache_read_tokens: 5000,
            cache_creation_tokens: 10000,
            stop_reason: Some("end_turn".to_string()),
            timestamp: Some("2026-07-14T12:00:00Z".to_string()),
            session_id: None,
        };

        let should_replace = final_entry.stop_reason.is_some()
            && messages.get("msg_1").unwrap().stop_reason.is_none();
        assert!(should_replace);

        messages.insert("msg_1".to_string(), final_entry);
        assert_eq!(messages.get("msg_1").unwrap().output_tokens, 1349);
    }

    #[test]
    fn test_skip_non_message_entries() {
        // branch_summary 和 custom_message 等非 message 条目应被跳过
        let branch = r#"{"type":"branch_summary","id":"g7h8i9j0","parentId":"a1b2c3d4","timestamp":"2026-07-14T12:15:00Z","fromId":"f6g7h8i9","summary":"Branch explored approach A..."}"#;
        let value: serde_json::Value = serde_json::from_str(branch).unwrap();
        assert_eq!(
            value.get("type").and_then(|t| t.as_str()),
            Some("branch_summary")
        );
        // 不应该被当成消息处理
        assert!(value.get("message").is_none());
    }

    #[test]
    fn test_insert_pi_agent_session_skips_matching_proxy_log() -> Result<(), AppError> {
        let db = Database::memory()?;
        {
            let conn = lock_conn!(db.conn);
            conn.execute(
                "INSERT INTO proxy_request_logs (
                    request_id, provider_id, app_type, model, request_model,
                    input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens,
                    total_cost_usd, latency_ms, status_code, created_at, data_source
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                rusqlite::params![
                    "pi_agent_session:msg_1",
                    "anthropic",
                    "pi-agent",
                    "claude-opus-4-8",
                    "claude-opus-4-8",
                    100,
                    20,
                    10,
                    5,
                    "0.10",
                    100,
                    200,
                    1000,
                    "pi_agent_session"
                ],
            )?;
        }

        let msg = PiAgentAssistantUsage {
            message_id: "msg_1".to_string(),
            model: "claude-opus-4-8".to_string(),
            provider: Some("anthropic".to_string()),
            input_tokens: 100,
            output_tokens: 20,
            cache_read_tokens: 10,
            cache_creation_tokens: 5,
            stop_reason: Some("end_turn".to_string()),
            timestamp: Some("1970-01-01T00:16:45Z".to_string()),
            session_id: Some("session-1".to_string()),
        };

        let inserted = insert_session_log_entry(&db, "pi_agent_session:msg_1", &msg)?;
        assert!(!inserted);

        let conn = lock_conn!(db.conn);
        let count: i64 = conn.query_row("SELECT COUNT(*) FROM proxy_request_logs", [], |row| {
            row.get(0)
        })?;
        assert_eq!(count, 1);

        Ok(())
    }

    #[test]
    fn test_collect_jsonl_files() {
        let tmp = std::env::temp_dir().join(format!("cc-switch-test-{}", uuid::Uuid::new_v4()));
        let session_dir = tmp.join("project-hash");
        fs::create_dir_all(&session_dir).unwrap();

        fs::write(session_dir.join("20260714_abc123.jsonl"), "{}").unwrap();
        fs::write(session_dir.join("20260714_def456.jsonl"), "{}").unwrap();

        let files = collect_jsonl_files(&tmp);
        assert_eq!(files.len(), 2);

        fs::remove_dir_all(&tmp).ok();
    }
}
