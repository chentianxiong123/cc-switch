//! OpenCode 会话日志使用追踪
//!
//! 从 ~/.local/share/opencode/opencode.db (SQLite) 中提取精确 token 使用数据。
//!
//! ## 数据流
//! ```text
//! ~/.local/share/opencode/opencode.db
//!   → session 表获取所有会话
//!   → message 表获取 assistant 消息
//!   → 解析 data JSON 提取 tokens/cost/model
//!   → proxy_request_logs 表
//! ```

use crate::database::{lock_conn, Database};
use crate::error::AppError;
use crate::opencode_config::get_opencode_db_path;
use crate::proxy::usage::calculator::CostCalculator;
use crate::proxy::usage::parser::TokenUsage;
use crate::services::session_usage::{
    cached_model_pricing, get_all_sync_states, metadata_modified_nanos, update_sync_state_conn,
    PricingCache, SessionSyncResult, SESSION_LOG_COMMIT_BATCH,
};
use crate::services::usage_stats::{should_skip_session_insert, DedupKey};
use rust_decimal::Decimal;
use std::fs;
use std::time::SystemTime;

/// 从 opencode message.data JSON 中提取的 token 和费用数据
struct OpenCodeMessageData {
    input_tokens: u32,
    output_tokens: u32,
    reasoning_tokens: u32,
    cache_read_tokens: u32,
    cache_write_tokens: u32,
    cost: f64,
    model_id: String,
    /// 入库时间戳（Unix 秒），在阶段一（解析/收集）就定死：`time.created` 缺失
    /// 或 <=0 时回退 now()。两阶段批量写库下不推迟到 insert 才取 now()，避免
    /// 退化输入（缺时间戳）的时间戳后移。
    created_at: i64,
}

/// 当前 Unix 时间（秒）。缺失/非法时间戳时的回退来源。
fn now_unix_secs() -> i64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

struct OpenCodeMessageQueryResult {
    messages: Vec<(String, OpenCodeMessageData)>,
    has_incomplete_usage: bool,
}

/// 一个待写入主库的会话（有界分批的批次元素）。
struct PendingOpenCodeSession {
    session_id: String,
    sync_key: String,
    time_updated: i64,
    /// 消息查询完整且无 incomplete usage 时才推进会话级同步状态
    advance_state: bool,
    messages: Vec<(String, OpenCodeMessageData)>,
}

/// 把一个批次写入主库：一个短事务覆盖批内全部消息插入与会话级状态更新，
/// 提交后清空批次释放内存。
fn flush_opencode_batch(
    db: &Database,
    pricing_cache: &mut PricingCache,
    batch: &mut Vec<PendingOpenCodeSession>,
    result: &mut SessionSyncResult,
    has_sync_errors: &mut bool,
) -> Result<(), AppError> {
    if batch.is_empty() {
        return Ok(());
    }

    let mut guard = lock_conn!(db.conn);
    let tx = guard
        .transaction()
        .map_err(|e| AppError::Database(format!("开启事务失败: {e}")))?;

    for session in batch.iter() {
        let mut session_had_error = false;

        for (request_id, msg_data) in &session.messages {
            match insert_opencode_message(
                &tx,
                pricing_cache,
                request_id,
                msg_data,
                &session.session_id,
            ) {
                Ok(true) => result.imported += 1,
                Ok(false) => result.skipped += 1,
                Err(e) => {
                    let msg = format!("OpenCode 消息插入失败 {request_id}: {e}");
                    log::warn!("[OPENCODE-SYNC] {msg}");
                    result.errors.push(msg);
                    result.skipped += 1;
                    session_had_error = true;
                }
            }
        }

        if session_had_error {
            *has_sync_errors = true;
            continue;
        }

        if !session.advance_state {
            continue;
        }

        // 更新会话级同步状态。失败时不要推进文件级状态，确保下次可重试。
        if let Err(e) = update_sync_state_conn(&tx, &session.sync_key, session.time_updated, 0) {
            let msg = format!("OpenCode 会话同步状态更新失败 {}: {e}", session.sync_key);
            log::warn!("[OPENCODE-SYNC] {msg}");
            result.errors.push(msg);
            *has_sync_errors = true;
        }
    }

    tx.commit()
        .map_err(|e| AppError::Database(format!("提交事务失败: {e}")))?;
    drop(guard);

    batch.clear();
    Ok(())
}

/// 同步 OpenCode 使用数据
pub fn sync_opencode_usage(db: &Database) -> Result<SessionSyncResult, AppError> {
    let db_path = get_opencode_db_path();

    if !db_path.exists() {
        return Ok(SessionSyncResult {
            imported: 0,
            skipped: 0,
            files_scanned: 0,
            errors: vec![],
        });
    }

    // OpenCode 是单个外部数据库，进度上按 1 个数据源计；guard 确保
    // 提前 return（未变化跳过/出错）也会计入 done。
    crate::services::session_usage::sync_progress::add_total(1);
    struct ProgressDoneGuard;
    impl Drop for ProgressDoneGuard {
        fn drop(&mut self) {
            crate::services::session_usage::sync_progress::add_done(1);
        }
    }
    let _done_on_exit = ProgressDoneGuard;

    let db_path_str = db_path.to_string_lossy().to_string();

    // 检查文件修改时间。
    // opencode 的数据库运行在 WAL 模式：新提交先落在 -wal 文件里，
    // 主库文件只有在 checkpoint 时才更新。因此必须同时考虑 -wal 的
    // mtime，否则会在 checkpoint 之前漏掉刚写入的会话。
    let metadata = fs::metadata(&db_path)
        .map_err(|e| AppError::Config(format!("无法读取 opencode.db 元数据: {e}")))?;
    let mut file_modified = metadata_modified_nanos(&metadata);

    let wal_path = db_path.with_extension("db-wal");
    if let Ok(wal_meta) = fs::metadata(&wal_path) {
        file_modified = file_modified.max(metadata_modified_nanos(&wal_meta));
    }

    // 一次性预载全部同步状态（文件级 + 会话级），避免逐会话查库；也让下方所有
    // 写入能在同一个写事务内完成，而不会与逐次 get_sync_state 的加锁互相自锁。
    let sync_states = get_all_sync_states(db)?;
    let (last_modified, _last_offset) = sync_states.get(&db_path_str).copied().unwrap_or((0, 0));

    // 文件未变化则跳过
    if file_modified <= last_modified {
        return Ok(SessionSyncResult {
            imported: 0,
            skipped: 0,
            files_scanned: 1,
            errors: vec![],
        });
    }

    // 打开 opencode 的 SQLite 数据库（只读）
    let opencode_conn =
        rusqlite::Connection::open_with_flags(&db_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|e| AppError::Database(format!("无法打开 opencode.db: {e}")))?;

    let mut result = SessionSyncResult {
        imported: 0,
        skipped: 0,
        files_scanned: 1,
        errors: vec![],
    };
    let mut has_sync_errors = false;

    // 本次同步周期共享的定价缓存，避免每条消息重复查 model_pricing 表。
    let mut pricing_cache = PricingCache::new();

    // 查询所有会话
    let sessions = query_sessions(&opencode_conn)?;

    // 有界分批：逐会话读外部 opencode.db（期间不持主库锁），凑满约
    // SESSION_LOG_COMMIT_BATCH 条消息就作为一个批次写入主库短事务并释放
    // 内存。内存占用有界于单批，主库写锁窗口只覆盖每个批次的写入；批次
    // 之间崩溃由 request_id 去重兜底，未推进的会话级状态让下次重试。
    let mut batch: Vec<PendingOpenCodeSession> = Vec::new();
    let mut batch_messages: usize = 0;

    for (session_id, time_updated) in &sessions {
        // 检查会话是否需要重新同步（从预载快照读取）
        let sync_key = format!("{db_path_str}:{session_id}");
        let (sess_last_modified, _) = sync_states.get(&sync_key).copied().unwrap_or((0, 0));
        if *time_updated <= sess_last_modified {
            continue; // 会话未更新，跳过
        }

        match query_assistant_messages(&opencode_conn, session_id) {
            Ok(query_result) => {
                let messages: Vec<(String, OpenCodeMessageData)> = query_result
                    .messages
                    .into_iter()
                    .map(|(message_id, msg_data)| {
                        (
                            format!("opencode_session:{session_id}:{message_id}"),
                            msg_data,
                        )
                    })
                    .collect();
                batch_messages += messages.len();
                batch.push(PendingOpenCodeSession {
                    session_id: session_id.clone(),
                    sync_key,
                    time_updated: *time_updated,
                    advance_state: !query_result.has_incomplete_usage,
                    messages,
                });

                if batch_messages >= SESSION_LOG_COMMIT_BATCH as usize {
                    flush_opencode_batch(
                        db,
                        &mut pricing_cache,
                        &mut batch,
                        &mut result,
                        &mut has_sync_errors,
                    )?;
                    batch_messages = 0;
                }
            }
            Err(e) => {
                let msg = format!("OpenCode 会话消息查询失败 {session_id}: {e}");
                log::warn!("[OPENCODE-SYNC] {msg}");
                result.errors.push(msg);
                has_sync_errors = true;
            }
        }
    }
    drop(opencode_conn);

    flush_opencode_batch(
        db,
        &mut pricing_cache,
        &mut batch,
        &mut result,
        &mut has_sync_errors,
    )?;

    // 仅在本轮完全成功时推进文件级状态；否则保留下次重试入口。
    if !has_sync_errors {
        let mut guard = lock_conn!(db.conn);
        let tx = guard
            .transaction()
            .map_err(|e| AppError::Database(format!("开启事务失败: {e}")))?;
        update_sync_state_conn(&tx, &db_path_str, file_modified, 0)?;
        tx.commit()
            .map_err(|e| AppError::Database(format!("提交事务失败: {e}")))?;
    }

    if result.imported > 0 {
        log::info!(
            "[OPENCODE-SYNC] 同步完成: 导入 {} 条, 跳过 {} 条, 扫描 {} 个会话",
            result.imported,
            result.skipped,
            sessions.len()
        );
    }

    Ok(result)
}

/// 查询所有会话的 (id, sync_watermark)
fn query_sessions(conn: &rusqlite::Connection) -> Result<Vec<(String, i64)>, AppError> {
    let mut stmt = conn
        .prepare(
            "SELECT s.id,
                    MAX(s.time_updated, COALESCE(MAX(m.time_updated), s.time_updated)) AS sync_watermark
             FROM session s
             LEFT JOIN message m ON m.session_id = s.id
             GROUP BY s.id
             ORDER BY sync_watermark",
        )
        .map_err(|e| AppError::Database(format!("准备会话查询失败: {e}")))?;

    let rows = stmt
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })
        .map_err(|e| AppError::Database(format!("查询会话失败: {e}")))?;

    let mut sessions = Vec::new();
    for row in rows {
        sessions.push(row.map_err(|e| AppError::Database(format!("读取会话行失败: {e}")))?);
    }

    Ok(sessions)
}

/// 查询某会话的已完成 assistant 消息，并标记是否还有未完成 usage 消息。
fn query_assistant_messages(
    conn: &rusqlite::Connection,
    session_id: &str,
) -> Result<OpenCodeMessageQueryResult, AppError> {
    let mut stmt = conn
        .prepare("SELECT id, data FROM message WHERE session_id = ?1 ORDER BY time_created")
        .map_err(|e| AppError::Database(format!("准备消息查询失败: {e}")))?;

    let rows = stmt
        .query_map([session_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|e| AppError::Database(format!("查询消息失败: {e}")))?;

    let mut messages = Vec::new();
    let mut has_incomplete_usage = false;
    for row in rows {
        let (message_id, data_json) =
            row.map_err(|e| AppError::Database(format!("读取消息行失败: {e}")))?;

        // 只处理 assistant 消息
        let value: serde_json::Value = match serde_json::from_str(&data_json) {
            Ok(v) => v,
            Err(_) => continue,
        };

        if value.get("role").and_then(|r| r.as_str()) != Some("assistant") {
            continue;
        }

        // 必须有 tokens 字段
        if value.get("tokens").is_none() {
            continue;
        }

        // 跳过未完成的消息：进行中只有半截 token，且因 INSERT OR IGNORE 无法回填
        if value.get("time").and_then(|t| t.get("completed")).is_none() {
            has_incomplete_usage = true;
            continue;
        }

        if let Some(msg_data) = parse_message_data(&value) {
            messages.push((message_id, msg_data));
        }
    }

    Ok(OpenCodeMessageQueryResult {
        messages,
        has_incomplete_usage,
    })
}

/// 解析 opencode message.data JSON 为结构化数据
fn parse_message_data(value: &serde_json::Value) -> Option<OpenCodeMessageData> {
    let tokens = value.get("tokens")?;

    let input_tokens = tokens.get("input").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
    let output_tokens = tokens.get("output").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
    let reasoning_tokens = tokens
        .get("reasoning")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u32;

    let cache_obj = tokens.get("cache");
    let cache_read_tokens = cache_obj
        .and_then(|c| c.get("read"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u32;
    let cache_write_tokens = cache_obj
        .and_then(|c| c.get("write"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u32;

    // 跳过全零 token 的消息
    if input_tokens == 0
        && output_tokens == 0
        && reasoning_tokens == 0
        && cache_read_tokens == 0
        && cache_write_tokens == 0
    {
        return None;
    }

    let cost = value.get("cost").and_then(|v| v.as_f64()).unwrap_or(0.0);

    let model_id = value
        .get("modelID")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();

    let timestamp_ms = value
        .get("time")
        .and_then(|t| t.get("created"))
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    // 在阶段一（解析/收集）就定死 created_at（秒）：缺失/<=0 回退 now()。
    let created_at = if timestamp_ms > 0 {
        timestamp_ms / 1000
    } else {
        now_unix_secs()
    };

    Some(OpenCodeMessageData {
        input_tokens,
        output_tokens,
        reasoning_tokens,
        cache_read_tokens,
        cache_write_tokens,
        cost,
        model_id,
        created_at,
    })
}

/// 插入单条 OpenCode 消息记录到 proxy_request_logs
///
/// 调用方在同一事务连接上批量调用本函数；INSERT 与去重查询走 prepare_cached，
/// 费用查询走 per-cycle 定价缓存。
fn insert_opencode_message(
    conn: &rusqlite::Connection,
    pricing_cache: &mut PricingCache,
    request_id: &str,
    msg: &OpenCodeMessageData,
    session_id: &str,
) -> Result<bool, AppError> {
    // created_at 由 parse_message_data 在阶段一定死（见其字段注释），insert 只消费。
    let created_at = msg.created_at;

    // OpenCode 使用 Anthropic 风格：input 是新鲜输入，cache 单独计
    // output 包含 reasoning tokens（按输出计费）
    let output_with_reasoning = msg.output_tokens + msg.reasoning_tokens;

    let dedup_key = DedupKey {
        app_type: "opencode",
        model: &msg.model_id,
        input_tokens: msg.input_tokens,
        output_tokens: output_with_reasoning,
        cache_read_tokens: msg.cache_read_tokens,
        cache_creation_tokens: msg.cache_write_tokens,
        created_at,
    };
    if should_skip_session_insert(conn, request_id, &dedup_key)? {
        return Ok(false);
    }

    // 如果 opencode 已经提供了费用，直接使用；否则从模型定价计算
    let (input_cost, output_cost, cache_read_cost, cache_creation_cost, total_cost) =
        if msg.cost > 0.0 {
            // opencode 已计算费用，直接使用
            // 简化处理：全部放入 total_cost（opencode 的 cost 是聚合值，无法精确拆分）
            (
                "0".to_string(),
                "0".to_string(),
                "0".to_string(),
                "0".to_string(),
                msg.cost.to_string(),
            )
        } else {
            // opencode 费用为 0（如免费模型），尝试用 cc-switch 自带的模型定价计算
            let usage = TokenUsage {
                input_tokens: msg.input_tokens,
                output_tokens: output_with_reasoning,
                cache_read_tokens: msg.cache_read_tokens,
                cache_creation_tokens: msg.cache_write_tokens,
                model: Some(msg.model_id.clone()),
                message_id: None,
            };

            match cached_model_pricing(conn, pricing_cache, &msg.model_id) {
                Some(pricing) => {
                    let cost = CostCalculator::calculate_for_app(
                        "opencode",
                        &usage,
                        &pricing,
                        Decimal::from(1),
                    );
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
            }
        };

    let mut stmt = conn
        .prepare_cached(
            "INSERT OR IGNORE INTO proxy_request_logs (
            request_id, provider_id, app_type, model, request_model,
            input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens,
            input_cost_usd, output_cost_usd, cache_read_cost_usd, cache_creation_cost_usd, total_cost_usd,
            latency_ms, first_token_ms, status_code, error_message, session_id,
            provider_type, is_streaming, cost_multiplier, created_at, data_source
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24)",
        )
        .map_err(|e| AppError::Database(format!("插入 OpenCode 会话日志失败: {e}")))?;
    let inserted_rows = stmt
        .execute(rusqlite::params![
            request_id,
            "_opencode_session", // provider_id
            "opencode",          // app_type
            msg.model_id,
            msg.model_id, // request_model = model
            msg.input_tokens,
            output_with_reasoning,
            msg.cache_read_tokens,
            msg.cache_write_tokens,
            input_cost,
            output_cost,
            cache_read_cost,
            cache_creation_cost,
            total_cost,
            0i64,                   // latency_ms
            Option::<i64>::None,    // first_token_ms
            200i64,                 // status_code
            Option::<String>::None, // error_message
            Some(session_id.to_string()),
            Some("opencode_session"), // provider_type
            1i64,                     // is_streaming
            "1.0",                    // cost_multiplier
            created_at,
            "opencode_session", // data_source
        ])
        .map_err(|e| AppError::Database(format!("插入 OpenCode 会话日志失败: {e}")))?;

    Ok(inserted_rows > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_message_data_full() {
        let json: serde_json::Value = serde_json::json!({
            "role": "assistant",
            "cost": 0.0023113,
            "tokens": {
                "total": 56554,
                "input": 3272,
                "output": 383,
                "reasoning": 419,
                "cache": {
                    "write": 0,
                    "read": 52480
                }
            },
            "modelID": "deepseek-v4-pro",
            "providerID": "deepseek",
            "time": {
                "created": 1779755333700i64,
                "completed": 1779755350639i64
            }
        });
        let data = parse_message_data(&json).unwrap();
        assert_eq!(data.input_tokens, 3272);
        assert_eq!(data.output_tokens, 383);
        assert_eq!(data.reasoning_tokens, 419);
        assert_eq!(data.cache_read_tokens, 52480);
        assert_eq!(data.cache_write_tokens, 0);
        assert!((data.cost - 0.0023113).abs() < 1e-10);
        assert_eq!(data.model_id, "deepseek-v4-pro");
        // created_at 在解析阶段定死：time.created(ms) / 1000
        assert_eq!(data.created_at, 1_779_755_333);
    }

    #[test]
    fn test_parse_message_data_missing_timestamp_falls_back_to_now() {
        // 无 time.created → created_at 在解析阶段回退 now()，落在解析前后窗口内。
        let before = now_unix_secs();
        let json: serde_json::Value = serde_json::json!({
            "role": "assistant",
            "tokens": { "input": 10, "output": 5 },
            "modelID": "m"
        });
        let data = parse_message_data(&json).unwrap();
        let after = now_unix_secs();
        assert!(before <= data.created_at && data.created_at <= after);
    }

    #[test]
    fn test_parse_message_data_missing_cache() {
        let json: serde_json::Value = serde_json::json!({
            "role": "assistant",
            "cost": 0.0,
            "tokens": {
                "input": 1000,
                "output": 200
            },
            "modelID": "mimo-v2.5-pro",
            "time": { "created": 1779755333700i64 }
        });
        let data = parse_message_data(&json).unwrap();
        assert_eq!(data.input_tokens, 1000);
        assert_eq!(data.output_tokens, 200);
        assert_eq!(data.reasoning_tokens, 0);
        assert_eq!(data.cache_read_tokens, 0);
        assert_eq!(data.cache_write_tokens, 0);
    }

    #[test]
    fn test_parse_message_data_skips_zero_tokens() {
        let json: serde_json::Value = serde_json::json!({
            "role": "assistant",
            "tokens": {
                "input": 0,
                "output": 0,
                "reasoning": 0,
                "cache": { "read": 0, "write": 0 }
            },
            "modelID": "test"
        });
        assert!(parse_message_data(&json).is_none());
    }

    #[test]
    fn test_parse_message_data_ignores_role() {
        // parse_message_data does not filter by role; that's the caller's job
        let json: serde_json::Value = serde_json::json!({
            "role": "user",
            "tokens": { "input": 100, "output": 0 }
        });
        let data = parse_message_data(&json).unwrap();
        assert_eq!(data.input_tokens, 100);
    }

    #[test]
    fn test_query_assistant_messages_skips_incomplete() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE message (id TEXT, session_id TEXT, time_created INTEGER, data TEXT);",
        )
        .unwrap();

        let done = serde_json::json!({
            "role": "assistant",
            "tokens": { "input": 1000, "output": 200 },
            "modelID": "m",
            "time": { "created": 1, "completed": 2 }
        })
        .to_string();
        let in_progress = serde_json::json!({
            "role": "assistant",
            "tokens": { "input": 500, "output": 0 },
            "modelID": "m",
            "time": { "created": 3 }
        })
        .to_string();

        conn.execute(
            "INSERT INTO message VALUES ('done', 's1', 1, ?1), ('wip', 's1', 2, ?2)",
            rusqlite::params![done, in_progress],
        )
        .unwrap();

        let result = query_assistant_messages(&conn, "s1").unwrap();
        // 只返回已完成（带 time.completed）的消息，半截的被跳过
        assert_eq!(result.messages.len(), 1);
        assert_eq!(result.messages[0].0, "done");
        assert!(result.has_incomplete_usage);
    }

    #[test]
    fn test_query_sessions_uses_message_update_watermark() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE session (id TEXT, time_updated INTEGER);
             CREATE TABLE message (
                 id TEXT,
                 session_id TEXT,
                 time_created INTEGER,
                 time_updated INTEGER,
                 data TEXT
             );
             INSERT INTO session VALUES ('s1', 100);
             INSERT INTO message VALUES ('m1', 's1', 90, 200, '{}');",
        )
        .unwrap();

        let sessions = query_sessions(&conn).unwrap();
        assert_eq!(sessions, vec![("s1".to_string(), 200)]);
    }
}
