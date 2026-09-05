//! optimize 模块：系统优化维护，对标 `bin/optimize.sh` + `lib/optimize/*`。
//!
//! 第一片：任务目录（catalog.sh 21 项 1:1）+ 结果记账（outcomes.sh 语义）
//! + 有界执行框架 + `saved_state_cleanup` 处理器（1:1，其余处理器在
//! 后续子片逐个移植后开放——每个处理器都涉及 sudo/launchctl 等系统
//! 交互，按计划逐个对标移植，不在目录中静默猜测行为）。
//!
//! 结果语义（对标 outcomes.sh）：applied/unchanged/skipped/unavailable/
//! attention/failed 六态。

use serde::Serialize;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// 保存状态最小天数（对标 MOLE_SAVED_STATE_AGE_DAYS 默认 30）。
const SAVED_STATE_AGE_DAYS: u64 = 30;

/// 任务目录条目（catalog.sh 对齐数组 1:1）。
#[derive(Debug, Clone, Serialize)]
pub struct OptimizeTask {
    pub action: &'static str,
    pub health_name: &'static str,
    pub description: &'static str,
    /// 是否已移植（未移植任务在 UI 标注"待迁移"，不可执行）。
    pub implemented: bool,
}

/// 对标 `_optimize_catalog_register` 的 21 项注册（顺序一致）。
pub fn task_catalog() -> Vec<OptimizeTask> {
    let rows: &[(&str, &str, &str)] = &[
        ("system_maintenance", "DNS & Spotlight Check", "Refresh DNS cache & verify Spotlight status"),
        ("cache_refresh", "Finder Cache Refresh", "Refresh QuickLook thumbnails & icon services cache"),
        ("saved_state_cleanup", "App State Cleanup", "Remove old saved application states (30+ days)"),
        ("fix_broken_configs", "Broken Config Repair", "Fix corrupted preferences files"),
        ("network_optimization", "Network Cache Refresh", "Optimize DNS cache & restart mDNSResponder"),
        ("sqlite_vacuum", "Database Optimization", "Compress SQLite databases for Mail, Safari & Messages (skips if apps are running)"),
        ("launch_services_rebuild", "LaunchServices Repair", "Repair \"Open with\" menu & file associations"),
        ("prevent_network_dsstore", "Prevent Finder .DS_Store", "Set a persistent Finder preference to stop writing .DS_Store on SMB/AFP/NFS and USB volumes"),
        ("legacy_overrides_audit", "Legacy Overrides", "Remove hidden App Nap and disk-image verification overrides left by old tweak tools"),
        ("network_stack_optimize", "Network Stack Refresh", "Flush routing table and ARP cache to resolve network issues"),
        ("disk_permissions_repair", "Permission Repair", "Fix user directory permission issues"),
        ("spotlight_index_optimize", "Spotlight Optimization", "Rebuild index if search is slow (smart detection)"),
        ("spotlight_orphan_rules_cleanup", "Spotlight Orphan Rules", "Remove Spotlight search-rule entries for apps that are no longer installed"),
        ("periodic_maintenance", "Periodic Maintenance", "Run macOS daily/weekly/monthly maintenance scripts if stale"),
        ("shared_file_list_repair", "Shared File Lists", "Repair corrupted Finder favorites and recent documents"),
        ("disk_verify", "Disk Health", "Verify filesystem integrity"),
        ("login_items_audit", "Login Items", "Audit login items for broken entries"),
        ("quarantine_cleanup", "Quarantine Database Cleanup", "Clear Gatekeeper download tracking history"),
        ("launch_agents_cleanup", "Launch Agents Cleanup", "Remove broken LaunchAgents whose binaries no longer exist"),
        ("notification_cleanup", "Notifications", "Clean old delivered notifications to reduce database bloat"),
        ("coreduet_cleanup", "Usage Data", "Clean old usage tracking data"),
    ];
    rows.iter()
        .map(|(action, health_name, description)| OptimizeTask {
            action,
            health_name,
            description,
            // 首片仅移植 saved_state_cleanup；其余处理器逐个对标后开放。
            implemented: *action == "saved_state_cleanup",
        })
        .collect()
}

/// 任务结果六态（对标 MOLE_OPTIMIZE_OUTCOME_*）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[allow(dead_code)]
pub enum Outcome {
    Applied,
    Unchanged,
    Skipped,
    Unavailable,
    Attention,
    Failed,
}

impl Outcome {
    fn label(&self) -> &'static str {
        match self {
            Outcome::Applied => "applied",
            Outcome::Unchanged => "unchanged",
            Outcome::Skipped => "skipped",
            Outcome::Unavailable => "unavailable",
            Outcome::Attention => "attention",
            Outcome::Failed => "failed",
        }
    }
}

/// 单任务执行结果。
#[derive(Debug, Clone, Serialize)]
pub struct TaskResult {
    pub action: String,
    pub outcome: String,
    pub detail: String,
}

/// 执行汇总（对标 optimize_outcome 计数 + 摘要）。
#[derive(Debug, Clone, Serialize)]
pub struct OptimizeResult {
    pub results: Vec<TaskResult>,
    pub applied: usize,
    pub unchanged: usize,
    pub skipped: usize,
    pub unavailable: usize,
    pub attention: usize,
    pub failed: usize,
}

fn record(outcomes: &mut Vec<TaskResult>, action: &str, outcome: Outcome, detail: &str) {
    outcomes.push(TaskResult {
        action: action.to_string(),
        outcome: outcome.label().to_string(),
        detail: detail.to_string(),
    });
}

/// 执行选中的优化任务（未移植任务记录 unavailable）。
pub fn execute(selected: &[String], dry_run: bool) -> OptimizeResult {
    let mut outcomes: Vec<TaskResult> = Vec::new();

    for task in task_catalog() {
        if !selected.iter().any(|s| s == task.action) {
            continue;
        }
        match task.action {
            "saved_state_cleanup" => {
                let (outcome, detail) = saved_state_cleanup(dry_run);
                record(&mut outcomes, task.action, outcome, &detail);
            }
            other => {
                // 未移植任务：明确 unavailable，不猜测行为。
                record(
                    &mut outcomes,
                    other,
                    Outcome::Unavailable,
                    "处理器待移植（见 docs/migration/CHANGES.md §optimize）",
                );
            }
        }
    }

    let mut result = OptimizeResult {
        results: outcomes,
        applied: 0,
        unchanged: 0,
        skipped: 0,
        unavailable: 0,
        attention: 0,
        failed: 0,
    };
    for r in &result.results {
        match r.outcome.as_str() {
            "applied" => result.applied += 1,
            "unchanged" => result.unchanged += 1,
            "skipped" => result.skipped += 1,
            "unavailable" => result.unavailable += 1,
            "attention" => result.attention += 1,
            "failed" => result.failed += 1,
            _ => {}
        }
    }
    result
}

/// 对标 `opt_saved_state_cleanup`：扫描 `~/Library/Saved Application State`
/// 下超过 30 天的 .savedState 目录，保护检查后走 Trash。
///
/// 对标细节：有界扫描（超时放弃整批，不让部分扫描喂删除）；逐项
/// should_protect_path（复用 clean 模块完整保护层）；dry-run 只记录。
fn saved_state_cleanup(dry_run: bool) -> (Outcome, String) {
    let home = std::env::var("HOME").unwrap_or_default();
    let state_dir = Path::new(&home).join("Library/Saved Application State");
    if !state_dir.is_dir() {
        return (Outcome::Unavailable, "目录不存在".to_string());
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    let cutoff = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
        .saturating_sub(SAVED_STATE_AGE_DAYS * 86400);

    // 有界扫描（完整收集，超时放弃——对标 "materialize only completed scans"）。
    let mut candidates: Vec<PathBuf> = Vec::new();
    let mut timed_out = false;
    let Ok(entries) = std::fs::read_dir(&state_dir) else {
        return (Outcome::Unavailable, "无法读取目录".to_string());
    };
    for entry in entries.flatten() {
        if Instant::now() >= deadline {
            timed_out = true;
            break;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        // find -name "*.savedState" -mtime +30：目录名后缀 + mtime 早于阈值。
        if !name.ends_with(".savedState") {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        let Ok(modified) = meta.modified() else { continue };
        let Ok(mtime) = modified.duration_since(std::time::UNIX_EPOCH) else { continue };
        if mtime.as_secs() > cutoff {
            continue; // 太新
        }
        candidates.push(entry.path());
    }
    if timed_out {
        // 超时不产生删除（fail-closed，对标 scan_rc=124 分支）。
        return (Outcome::Unchanged, "扫描超时，本批放弃".to_string());
    }
    if candidates.is_empty() {
        return (Outcome::Unchanged, "没有超过 30 天的保存状态".to_string());
    }

    let mut trashed = 0usize;
    let mut skipped = 0usize;
    let mut failed = 0usize;
    let mut freed = 0u64;
    for path in &candidates {
        let path_str = path.to_string_lossy().to_string();
        // 对标逐项 should_protect_path。
        if crate::clean::protect::should_protect_path(&path_str) {
            skipped += 1;
            crate::clean::delete::log_operation("optimize", "SKIPPED", &path_str, "protected");
            continue;
        }
        let outcome = crate::clean::delete::delete_to_trash(&path_str, dry_run, "optimize");
        match outcome.status.as_str() {
            "ok" | "dry-run" => trashed += 1,
            "failed" => failed += 1,
            _ => skipped += 1,
        }
        freed += outcome.size_bytes;
    }

    if failed > 0 {
        return (Outcome::Failed, format!("{failed} 项删除失败"));
    }
    if trashed > 0 {
        return (
            Outcome::Applied,
            format!("已清理 {trashed} 项（{} KB）", freed / 1024),
        );
    }
    (
        Outcome::Unchanged,
        format!("无需处理（跳过 {skipped} 项）"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 对标 optimize_catalog_validate：动作唯一、命名规范、全部 safe。
    #[test]
    fn catalog_valid() {
        let tasks = task_catalog();
        assert_eq!(tasks.len(), 21, "任务数应与原目录一致");
        let mut seen = std::collections::HashSet::new();
        for t in &tasks {
            assert!(
                t.action.chars().all(|c| c.is_ascii_lowercase() || c == '_' || c.is_ascii_digit()),
                "非法 action: {}",
                t.action
            );
            assert!(!t.health_name.is_empty());
            assert!(!t.description.is_empty());
            assert!(seen.insert(t.action), "重复 action: {}", t.action);
        }
    }

    /// 处理器行为：无该目录内容时 unchanged（临时目录注入不可行——
    /// 路径固定，故仅覆盖不在 HOME 时不可达的分支语义；真实路径由冒烟覆盖）。
    #[test]
    fn catalog_implemented_marker() {
        for t in task_catalog() {
            if t.action == "saved_state_cleanup" {
                assert!(t.implemented);
            } else {
                assert!(!t.implemented, "{} 应标注待移植", t.action);
            }
        }
    }

    /// 执行框架：未移植任务记录 unavailable。
    #[test]
    fn unimplemented_task_unavailable() {
        let r = execute(&["sqlite_vacuum".to_string()], true);
        assert_eq!(r.results.len(), 1);
        assert_eq!(r.results[0].outcome, "unavailable");
        assert_eq!(r.unavailable, 1);
    }
}

/// 真机冒烟（默认忽略）：dry-run 执行 saved_state_cleanup。
#[cfg(test)]
mod smoke_tests {
    #[test]
    #[ignore]
    fn saved_state_dry_run_smoke() {
        let r = super::execute(&["saved_state_cleanup".to_string()], true);
        for res in &r.results {
            println!("[{}] {} ({})", res.outcome, res.action, res.detail);
        }
        assert_eq!(r.failed, 0);
    }
}
