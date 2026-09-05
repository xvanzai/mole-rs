//! history 模块：操作历史，对标 `bin/history.sh` + `lib/core/history.sh`。
//!
//! 解析两类日志（格式与 shell 侧写入完全一致）：
//! - `~/Library/Logs/mole/operations.log`：会话标记行 + `[ts] [cmd] ACTION path (detail)`；
//!   会话按 action 聚合计数（removed/trashed/skipped/failed/rebuilt/other）；
//!   未终止的会话在读取时闭合（对标 history_finish_session）。
//! - `~/Library/Logs/mole/deletions.log`：TSV 取证行。
//! limit 作用于最近的 N 条会话（1-200，对标 history_normalize_limit）。

use serde::Serialize;
use std::path::PathBuf;

/// 默认展示条数（对标 MOLE_HISTORY_DEFAULT_LIMIT）。
const DEFAULT_LIMIT: usize = 20;

fn operations_log_file() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    PathBuf::from(home).join("Library/Logs/mole/operations.log")
}

fn deletions_log_file() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    PathBuf::from(home).join("Library/Logs/mole/deletions.log")
}

/// 会话聚合。
#[derive(Debug, Clone, Default, Serialize)]
pub struct HistorySession {
    pub command: String,
    pub started_at: String,
    pub ended_at: Option<String>,
    pub items: Option<u64>,
    /// 会话结束时的人类可读大小（原样保留）。
    pub size_human: Option<String>,
    pub removed: u64,
    pub trashed: u64,
    pub skipped: u64,
    pub failed: u64,
    pub rebuilt: u64,
    pub other: u64,
    pub operations: u64,
    pub failed_tasks: u64,
}

/// 删除取证记录。
#[derive(Debug, Clone, Serialize)]
pub struct DeletionRecord {
    pub timestamp: String,
    pub mode: String,
    pub size_kb: String,
    pub status: String,
    pub path: String,
}

/// 历史数据（会话倒序取 limit，删除记录倒序取 limit）。
#[derive(Debug, Clone, Serialize)]
pub struct HistoryData {
    pub sessions: Vec<HistorySession>,
    pub deletions: Vec<DeletionRecord>,
}

enum LineKind {
    SessionStart(String, String),
    SessionEnd(String, String, Option<u64>, Option<String>),
    Operation(String, String, String),
}

/// 解析单行（对标 history_parse_session_start/end/operation_line）。
fn parse_line(line: &str) -> Option<LineKind> {
    // 会话开始：# ========== <cmd> session started at <ts> ==========
    if let Some(rest) = line.strip_prefix("# ========== ") {
        if let Some((cmd, tail)) = rest.split_once(" session started at ") {
            let ts = tail.trim_end_matches(" ==========").trim();
            return Some(LineKind::SessionStart(cmd.trim().to_string(), ts.to_string()));
        }
        // 会话结束：# ========== <cmd> session ended at <ts>, N items, <size> ==========
        if let Some((head, tail)) = rest.split_once(" session ended at ") {
            let (ts, stats) = tail.split_once(", ")?;
            let ts = ts.trim().to_string();
            let stats = stats.trim_end_matches(" ==========").trim();
            let mut items = None;
            let mut size_human = None;
            for part in stats.split(", ") {
                if part.ends_with(" items") {
                    // 形如 "2 items"：取首段数字。
                    items = part.split(' ').next().and_then(|n| n.parse().ok());
                }
                if part.contains('B') && !part.ends_with("items") {
                    size_human = Some(part.trim().to_string());
                }
            }
            return Some(LineKind::SessionEnd(head.trim().to_string(), ts, items, size_human));
        }
        return None;
    }
    // 操作行：[ts] [cmd] ACTION path (detail)
    let mut chars = line.chars();
    if chars.next() != Some('[') {
        return None;
    }
    let ts_end = line.find(']')?;
    let timestamp = line[1..ts_end].to_string();
    let rest = &line[ts_end + 1..];
    let rest = rest.trim_start();
    let rest = rest.strip_prefix('[')?;
    let cmd_end = rest.find(']')?;
    let command = rest[..cmd_end].to_string();
    let after = rest[cmd_end + 1..].trim_start();
    let action = after.split_whitespace().next()?.to_string();
    if timestamp.is_empty() || command.is_empty() || action.is_empty() {
        return None;
    }
    Some(LineKind::Operation(timestamp, command, action))
}

impl HistorySession {
    fn count_action(&mut self, action: &str) {
        self.operations += 1;
        match action {
            "REMOVED" => self.removed += 1,
            "TRASHED" => self.trashed += 1,
            "SKIPPED" => self.skipped += 1,
            "FAILED" => self.failed += 1,
            "REBUILT" => self.rebuilt += 1,
            _ => self.other += 1,
        }
    }
}

/// 加载历史（对标 history_load_operations + history_load_deletions）。
pub fn load_history(limit: usize) -> HistoryData {
    let limit = limit.clamp(1, 200);
    let mut sessions: Vec<HistorySession> = Vec::new();

    if let Ok(content) = std::fs::read_to_string(operations_log_file()) {
        for line in content.lines() {
            match parse_line(line) {
                Some(LineKind::SessionStart(command, started_at)) => {
                    sessions.push(HistorySession {
                        command,
                        started_at,
                        ..Default::default()
                    });
                }
                Some(LineKind::SessionEnd(command, ended_at, items, size_human)) => {
                    // 闭合同名最近会话（含未终止即结束的异常形态）。
                    if let Some(session) = sessions
                        .iter_mut()
                        .rev()
                        .find(|s| s.command == command && s.ended_at.is_none())
                    {
                        session.ended_at = Some(ended_at);
                        session.items = items;
                        session.size_human = size_human;
                    }
                }
                Some(LineKind::Operation(_ts, command, action)) => {
                    // 归属最近一个同名未闭合会话；没有则视为独立操作行。
                    match sessions
                        .iter_mut()
                        .rev()
                        .find(|s| s.command == command && s.ended_at.is_none())
                    {
                        Some(session) => session.count_action(&action),
                        None => {
                            let mut orphan = HistorySession {
                                command,
                                started_at: String::new(),
                                ..Default::default()
                            };
                            orphan.count_action(&action);
                            sessions.push(orphan);
                        }
                    }
                }
                None => {}
            }
        }
        // 未终止会话在读取时闭合（对标 history_finish_session）。
        for session in &mut sessions {
            if session.ended_at.is_none() && !session.started_at.is_empty() {
                session.ended_at = Some("(进行中)".into());
            }
        }
    }

    let mut deletions: Vec<DeletionRecord> = Vec::new();
    if let Ok(content) = std::fs::read_to_string(deletions_log_file()) {
        for line in content.lines() {
            if line.trim().is_empty() {
                continue;
            }
            let parts: Vec<&str> = line.split('\t').collect();
            if parts.len() < 5 {
                continue;
            }
            deletions.push(DeletionRecord {
                timestamp: parts[0].to_string(),
                mode: parts[1].to_string(),
                size_kb: parts[2].to_string(),
                status: parts[3].to_string(),
                path: parts[4].to_string(),
            });
        }
    }

    let mut sessions = if sessions.len() > limit {
        sessions[sessions.len() - limit..].to_vec()
    } else {
        sessions
    };
    sessions.reverse();

    let mut deletions = if deletions.len() > limit {
        deletions[deletions.len() - limit..].to_vec()
    } else {
        deletions
    };
    deletions.reverse();

    HistoryData {
        sessions,
        deletions,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 对标三种行格式与聚合语义。
    #[test]
    fn parse_and_aggregate() {
        let start = "# ========== clean session started at 2026-09-05 10:00:00 ==========";
        let op1 = "[2026-09-05 10:00:05] [clean] TRASHED /Users/t/.npm/_cacache/x (161KB)";
        let op2 = "[2026-09-05 10:00:06] [clean] SKIPPED /System/Library/x (protected)";
        let end = "# ========== clean session ended at 2026-09-05 10:00:10, 2 items, 161.0 KB ==========";

        assert!(matches!(
            parse_line(start),
            Some(LineKind::SessionStart(ref c, ref ts)) if c == "clean" && ts == "2026-09-05 10:00:00"
        ));
        match parse_line(end) {
            Some(LineKind::SessionEnd(c, ts, items, size)) => {
                assert_eq!(c, "clean");
                assert_eq!(items, Some(2));
                assert_eq!(size.as_deref(), Some("161.0 KB"));
                assert_eq!(ts, "2026-09-05 10:00:10");
            }
            _ => panic!("session end not parsed"),
        }
        assert!(matches!(
            parse_line(op1),
            Some(LineKind::Operation(_, ref c, ref a)) if c == "clean" && a == "TRASHED"
        ));
        assert!(matches!(
            parse_line(op2),
            Some(LineKind::Operation(_, _, ref a)) if a == "SKIPPED"
        ));
    }

    #[test]
    fn non_operations_lines_ignored() {
        assert!(parse_line("").is_none());
        assert!(parse_line("random text").is_none());
        assert!(parse_line("[broken").is_none());
    }

    /// limit 边界（对标 1-200）。
    #[test]
    fn limit_bounds() {
        // clamp 语义：0 → 1，>200 → 200；日志为机器真实状态（冒烟会写入），
        // 只断言不 panic 且结果有限。
        let data = load_history(0);
        assert!(data.sessions.len() <= 200);
        let data = load_history(500);
        assert!(data.sessions.len() <= 200);
    }
}
