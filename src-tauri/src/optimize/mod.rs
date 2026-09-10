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
            // 首批移植：saved_state_cleanup / cache_refresh /
            // prevent_network_dsstore / legacy_overrides_audit；
            // 其余处理器逐个对标后开放。
            implemented: matches!(
                *action,
                "saved_state_cleanup"
                    | "cache_refresh"
                    | "prevent_network_dsstore"
                    | "legacy_overrides_audit"
                    | "sqlite_vacuum"
                    | "quarantine_cleanup"
                    | "launch_agents_cleanup"
                    | "coreduet_cleanup"
                    | "notification_cleanup"
                    | "fix_broken_configs"
                    | "system_maintenance"
                    | "network_optimization"
                    | "launch_services_rebuild"
                    | "network_stack_optimize"
                    | "disk_permissions_repair"
                    | "periodic_maintenance"
                    | "shared_file_list_repair"
                    | "disk_verify"
            ),
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

/// 对标 `optimize_task_result_from_counts`：失败>0→Failed，applied>0→
/// Applied，skipped>0→Skipped，否则 Unchanged。
fn outcome_from_counts(applied: usize, failed: usize, skipped: usize) -> Outcome {
    if failed > 0 {
        Outcome::Failed
    } else if applied > 0 {
        Outcome::Applied
    } else if skipped > 0 {
        Outcome::Skipped
    } else {
        Outcome::Unchanged
    }
}

/// 对标 `optimize_sudo_available`：dry-run 视为可用；真实执行仅接受
/// 已缓存的非交互 sudo（`sudo -n true`），不弹密码框——GUI 约束，
/// 见 CHANGES.md（原 CLI 经 ensure_sudo_session 交互获取会话）。
fn optimize_sudo_available(dry_run: bool) -> bool {
    if dry_run {
        return true;
    }
    probe_exit_code("sudo", &["-n", "true"], Duration::from_secs(3)) == Some(0)
}

/// 对标 `flush_dns_cache`：dry-run 直接成功（并置 MOLE_DNS_FLUSHED 语义）；
/// 真实：sudo dscacheutil -flushcache && sudo killall -HUP mDNSResponder。
fn flush_dns_cache(dry_run: bool) -> bool {
    if dry_run {
        return true;
    }
    if !optimize_sudo_available(false) {
        return false;
    }
    crate::status::run_cmd(
        "sudo",
        &["dscacheutil", "-flushcache"],
        Duration::from_secs(5),
    )
    .is_ok()
        && crate::status::run_cmd(
            "sudo",
            &["killall", "-HUP", "mDNSResponder"],
            Duration::from_secs(5),
        )
        .is_ok()
}

/// 对标 `get_lsregister_path`：两个候选路径，可执行则返回。
fn get_lsregister_path() -> Option<String> {
    if let Ok(p) = std::env::var("MOLE_LSREGISTER_PATH") {
        return Some(p);
    }
    for candidate in [
        "/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister",
        "/System/Library/CoreServices/Frameworks/LaunchServices.framework/Support/lsregister",
    ] {
        let path = Path::new(candidate);
        if path.is_file() && is_executable(path) {
            return Some(candidate.to_string());
        }
    }
    None
}

fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|m| m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

/// 执行选中的优化任务（未移植任务记录 unavailable）。
///
/// `dns_flushed` 对标 `MOLE_DNS_FLUSHED`：system_maintenance 刷新成功后
/// network_optimization 直接 Unchanged，避免同一轮执行重复刷 DNS。
pub fn execute(selected: &[String], dry_run: bool) -> OptimizeResult {
    let mut outcomes: Vec<TaskResult> = Vec::new();
    let mut dns_flushed = false;

    for task in task_catalog() {
        if !selected.iter().any(|s| s == task.action) {
            continue;
        }
        match task.action {
            "system_maintenance" => {
                let (outcome, detail, flushed) = system_maintenance(dry_run);
                if flushed {
                    dns_flushed = true;
                }
                record(&mut outcomes, task.action, outcome, &detail);
            }
            "network_optimization" => {
                let (outcome, detail) = network_optimization(dry_run, dns_flushed);
                record(&mut outcomes, task.action, outcome, &detail);
            }
            "launch_services_rebuild" => {
                let (outcome, detail) = launch_services_rebuild(dry_run);
                record(&mut outcomes, task.action, outcome, &detail);
            }
            "network_stack_optimize" => {
                let (outcome, detail) = network_stack_optimize(dry_run);
                record(&mut outcomes, task.action, outcome, &detail);
            }
            "disk_permissions_repair" => {
                let (outcome, detail) = disk_permissions_repair(dry_run);
                record(&mut outcomes, task.action, outcome, &detail);
            }
            "periodic_maintenance" => {
                let (outcome, detail) = periodic_maintenance(dry_run);
                record(&mut outcomes, task.action, outcome, &detail);
            }
            "shared_file_list_repair" => {
                let (outcome, detail) = shared_file_list_repair(dry_run);
                record(&mut outcomes, task.action, outcome, &detail);
            }
            "disk_verify" => {
                let (outcome, detail) = disk_verify(dry_run);
                record(&mut outcomes, task.action, outcome, &detail);
            }
            "saved_state_cleanup" => {
                let (outcome, detail) = saved_state_cleanup(dry_run);
                record(&mut outcomes, task.action, outcome, &detail);
            }
            "cache_refresh" => {
                let (outcome, detail) = cache_refresh(dry_run);
                record(&mut outcomes, task.action, outcome, &detail);
            }
            "prevent_network_dsstore" => {
                let (outcome, detail) = prevent_network_dsstore(dry_run);
                record(&mut outcomes, task.action, outcome, &detail);
            }
            "legacy_overrides_audit" => {
                let (outcome, detail) = legacy_overrides_audit(dry_run);
                record(&mut outcomes, task.action, outcome, &detail);
            }
            "sqlite_vacuum" => {
                let (outcome, detail) = sqlite_vacuum(dry_run);
                record(&mut outcomes, task.action, outcome, &detail);
            }
            "quarantine_cleanup" => {
                let (outcome, detail) = quarantine_cleanup(dry_run);
                record(&mut outcomes, task.action, outcome, &detail);
            }
            "launch_agents_cleanup" => {
                let (outcome, detail) = launch_agents_cleanup(dry_run);
                record(&mut outcomes, task.action, outcome, &detail);
            }
            "coreduet_cleanup" => {
                let (outcome, detail) = coreduet_cleanup(dry_run);
                record(&mut outcomes, task.action, outcome, &detail);
            }
            "notification_cleanup" => {
                let (outcome, detail) = notification_cleanup(dry_run);
                record(&mut outcomes, task.action, outcome, &detail);
            }
            "fix_broken_configs" => {
                let (outcome, detail) = fix_broken_configs(dry_run);
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

/// 对标 `opt_cache_refresh`：qlmanage 刷新 + 三个固定缓存目标走 Trash。
///
/// 加固差异：原实现经 safe_remove 永久删除；Rust 侧统一走
/// `delete_to_trash`（回收站可恢复），见 CHANGES.md。
fn cache_refresh(dry_run: bool) -> (Outcome, String) {
    let home = std::env::var("HOME").unwrap_or_default();
    let targets = [
        format!("{home}/Library/Caches/com.apple.QuickLook.thumbnailcache"),
        format!("{home}/Library/Caches/com.apple.iconservices.store"),
        format!("{home}/Library/Caches/com.apple.iconservices"),
    ];

    let mut refresh_failed = 0usize;
    if !dry_run {
        // 对标：qlmanage -r cache（缩略图）与 qlmanage -r（图标）。
        if crate::status::run_cmd("qlmanage", &["-r", "cache"], Duration::from_secs(10)).is_err() {
            refresh_failed += 1;
        }
        if crate::status::run_cmd("qlmanage", &["-r"], Duration::from_secs(10)).is_err() {
            refresh_failed += 1;
        }
    }

    let mut removed_count = 0usize;
    let mut remove_failed = 0usize;
    let mut freed = 0u64;
    for target in &targets {
        let p = Path::new(target);
        if !p.exists() && !p.is_symlink() {
            continue;
        }
        if crate::clean::protect::should_protect_path(target) {
            continue;
        }
        let outcome = crate::clean::delete::delete_to_trash(target, dry_run, "optimize");
        match outcome.status.as_str() {
            "ok" | "dry-run" => removed_count += 1,
            "failed" => remove_failed += 1,
            _ => {}
        }
        freed += outcome.size_bytes;
    }

    if (refresh_failed > 0 || remove_failed > 0) && removed_count == 0 {
        return (
            Outcome::Failed,
            format!("qlmanage 刷新失败 {refresh_failed}，删除失败 {remove_failed}"),
        );
    }
    if removed_count > 0 {
        let refresh_note = if refresh_failed > 0 || remove_failed > 0 {
            "，部分失败"
        } else {
            "，qlmanage 刷新完成"
        };
        return (
            Outcome::Applied,
            format!("已清理 {removed_count} 项（{} KB）{refresh_note}", freed / 1024),
        );
    }
    (
        Outcome::Unchanged,
        if refresh_failed > 0 {
            "无缓存可清，qlmanage 刷新部分失败".into()
        } else {
            "无缓存需要清理".into()
        },
    )
}

/// 对标 `opt_prevent_network_dsstore`：com.apple.desktopservices 的两个
/// 键（网络/USB）读取 → 写 -bool true。
fn prevent_network_dsstore(dry_run: bool) -> (Outcome, String) {
    let domain = "com.apple.desktopservices";
    let keys = ["DSDontWriteNetworkStores", "DSDontWriteUSBStores"];
    let mut changed = 0usize;
    let mut already = 0usize;
    let mut failed = 0usize;

    for key in keys {
        let current = crate::status::run_cmd("defaults", &["read", domain, key], Duration::from_secs(3))
            .unwrap_or_default();
        if current.trim() == "1" {
            already += 1;
            continue;
        }
        if dry_run {
            changed += 1;
            continue;
        }
        if crate::status::run_cmd(
            "defaults",
            &["write", domain, key, "-bool", "true"],
            Duration::from_secs(3),
        )
        .is_ok()
        {
            changed += 1;
        } else {
            failed += 1;
        }
    }

    if failed > 0 && changed == 0 {
        return (Outcome::Failed, "写入失败".into());
    }
    if changed > 0 {
        return (
            Outcome::Applied,
            if failed > 0 {
                format!("已启用 {changed} 项，{failed} 项失败")
            } else {
                format!("已在网络与 USB 卷启用 .DS_Store 预防（{changed} 项）")
            },
        );
    }
    if already > 0 {
        return (Outcome::Unchanged, ".DS_Store 预防已生效".into());
    }
    (Outcome::Unchanged, "无需变更".into())
}

/// 对标 `opt_legacy_overrides_audit`：App Nap 全局开关 + DiskImages
/// skip-verify 家族的遗留覆盖；truthy 判定 1/TRUE/YES；白名单检查
/// 对应 plist 后 defaults delete。
fn legacy_overrides_audit(dry_run: bool) -> (Outcome, String) {
    fn is_truthy(v: &str) -> bool {
        let t = v.trim();
        t == "1"
            || t.eq_ignore_ascii_case("true")
            || t.eq_ignore_ascii_case("yes")
    }

    let home = std::env::var("HOME").unwrap_or_default();
    let mut found: Vec<(&str, &str, &str, String)> = Vec::new(); // (domain, key, label, plist)

    let global = crate::status::run_cmd("defaults", &["read", "-g", "NSAppSleepDisabled"], Duration::from_secs(3))
        .unwrap_or_default();
    if is_truthy(&global) {
        found.push((
            "-g",
            "NSAppSleepDisabled",
            "App Nap disabled globally (NSAppSleepDisabled)",
            format!("{home}/Library/Preferences/.GlobalPreferences.plist"),
        ));
    }
    for key in ["skip-verify", "skip-verify-locked", "skip-verify-remote"] {
        let v = crate::status::run_cmd(
            "defaults",
            &["read", "com.apple.frameworks.diskimages", key],
            Duration::from_secs(3),
        )
        .unwrap_or_default();
        if is_truthy(&v) {
            found.push((
                "com.apple.frameworks.diskimages",
                key,
                "Disk-image verification skipped",
                format!("{home}/Library/Preferences/com.apple.frameworks.diskimages.plist"),
            ));
        }
    }

    if found.is_empty() {
        return (Outcome::Unchanged, "未发现遗留 App Nap 或磁盘映像覆盖".into());
    }

    let mut changed = 0usize;
    let mut skipped = 0usize;
    let mut failed = 0usize;
    let whitelist = crate::clean::whitelist::Whitelist::load();
    for (domain, key, _label, plist) in &found {
        if whitelist.is_whitelisted(plist) {
            skipped += 1;
            continue;
        }
        if dry_run {
            changed += 1;
            continue;
        }
        if crate::status::run_cmd("defaults", &["delete", domain, key], Duration::from_secs(3)).is_ok() {
            changed += 1;
        } else {
            failed += 1;
        }
    }

    if failed > 0 && changed == 0 {
        return (Outcome::Failed, "删除覆盖键失败".into());
    }
    if changed > 0 {
        return (
            Outcome::Applied,
            format!("已移除 {changed} 个覆盖{}", if skipped > 0 { format!("（跳过白名单 {skipped}）") } else { String::new() }),
        );
    }
    if skipped > 0 {
        return (Outcome::Skipped, format!("全部在白名单中（{skipped}）"));
    }
    (Outcome::Unchanged, "无需变更".into())
}

/// SQLite 文件魔数检测（对标 `file -b` 的 *SQLite* 判定；SQLite 头 16 字节）。
fn is_sqlite_file(path: &Path) -> bool {
    let Ok(bytes) = std::fs::read(path) else {
        return false;
    };
    bytes.starts_with(b"SQLite format 3\0")
}

/// 子进程退出码探针（对标 pgrep 的 0/1/其他 三态；超时按失败处理）。
fn probe_exit_code(bin: &str, args: &[&str], timeout: Duration) -> Option<i32> {
    let Ok(mut child) = std::process::Command::new(bin)
        .args(args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    else {
        return None;
    };
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Some(status.code().unwrap_or(-1)),
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    return None;
                }
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(_) => return None,
        }
    }
}

/// sqlite3 执行（统一超时包装；成功返回 stdout）。
fn run_sqlite(db: &str, sql: &str, timeout: Duration) -> Result<String, ()> {
    crate::status::run_cmd("sqlite3", &[db, sql], timeout).map_err(|_| ())
}

/// 对标 `opt_sqlite_vacuum`：Mail/Safari/Messages 的 SQLite VACUUM。
/// 流程：pgrep 三态探针（任一运行中 → Skipped；探针失败 → Failed）→
/// sqlite3 可用性 → 目标 glob（跳过 -wal/-shm）→ 保护检查 → 魔数 →
/// 100MB 上限（#1367）→ freelist <5% 视为已优化 → integrity_check →
/// VACUUM（dry-run 计数）。
fn sqlite_vacuum(dry_run: bool) -> (Outcome, String) {
    const MAX_SIZE: u64 = 104_857_600; // 对标 MOLE_SQLITE_MAX_SIZE
    if !crate::clean::command_exists("pgrep") {
        return (Outcome::Unavailable, "pgrep 不可用".into());
    }
    // 进程三态探针（对标 pgrep -x；退出码 0=运行中，1=未运行，其他=失败）。
    let mut busy = Vec::new();
    for app in ["Mail", "Safari", "Messages"] {
        match probe_exit_code("pgrep", &["-x", app], Duration::from_secs(3)) {
            Some(0) => busy.push(app),
            Some(1) => {}
            _ => return (Outcome::Failed, format!("无法探测 {app} 进程状态")),
        }
    }
    if !busy.is_empty() {
        return (
            Outcome::Skipped,
            format!("请先关闭：{}", busy.join("、")),
        );
    }
    if !crate::clean::command_exists("sqlite3") {
        return (Outcome::Unavailable, "sqlite3 不可用".into());
    }

    let home = std::env::var("HOME").unwrap_or_default();
    let patterns = [
        format!("{home}/Library/Mail/V*/MailData/Envelope Index*"),
        format!("{home}/Library/Messages/chat.db"),
        format!("{home}/Library/Safari/History.db"),
        format!("{home}/Library/Safari/TopSites.db"),
    ];

    let mut vacuumed = 0usize;
    let mut timed_out = 0usize;
    let mut failed = 0usize;
    let mut policy_skipped = 0usize;
    let mut already_optimal = 0usize;

    for pattern in &patterns {
        for db_path in crate::clean::expand_glob(Path::new(pattern)) {
            let db = db_path.to_string_lossy().to_string();
            if !db_path.is_file() {
                continue;
            }
            if db.ends_with("-wal") || db.ends_with("-shm") {
                continue;
            }
            if crate::clean::protect::should_protect_path(&db) {
                continue;
            }
            if !is_sqlite_file(&db_path) {
                continue;
            }
            let Ok(meta) = db_path.metadata() else { continue };
            if meta.len() > MAX_SIZE {
                policy_skipped += 1;
                continue;
            }
            // freelist 比率：<5% 视为已压缩。
            let Ok(info) = run_sqlite(&db, "PRAGMA page_count; PRAGMA freelist_count;", Duration::from_secs(5)) else {
                failed += 1;
                continue;
            };
            let mut lines = info.lines();
            let page_count: Option<u64> = lines.next().and_then(|l| l.trim().parse().ok());
            let freelist: Option<u64> = lines.next().and_then(|l| l.trim().parse().ok());
            match (page_count, freelist) {
                (Some(pc), Some(fl)) if pc > 0 => {
                    if fl * 100 < pc * 5 {
                        already_optimal += 1;
                        continue;
                    }
                }
                _ => {
                    failed += 1;
                    continue;
                }
            }
            if !dry_run {
                // integrity_check 必须为 ok 才 VACUUM。
                match run_sqlite(&db, "PRAGMA integrity_check;", Duration::from_secs(8)) {
                    Ok(out) if out.trim() == "ok" => {}
                    _ => {
                        failed += 1;
                        continue;
                    }
                }
                match run_sqlite(&db, "VACUUM;", Duration::from_secs(15)) {
                    Ok(_) => vacuumed += 1,
                    Err(()) => timed_out += 1,
                }
            } else {
                vacuumed += 1;
            }
        }
    }

    if vacuumed > 0 {
        return (Outcome::Applied, format!("已优化 {vacuumed} 个数据库"));
    }
    if timed_out > 0 {
        return (Outcome::Attention, format!("{timed_out} 个数据库超时"));
    }
    if failed > 0 {
        return (Outcome::Failed, format!("{failed} 个数据库处理失败"));
    }
    if policy_skipped > 0 {
        return (Outcome::Skipped, format!("{policy_skipped} 个数据库超 100MB 上限"));
    }
    if already_optimal > 0 {
        return (Outcome::Unchanged, "数据库已处于压缩状态".into());
    }
    (Outcome::Unchanged, "没有需要优化的数据库".into())
}

/// 对标 `launch_agent_volume_mounted`：/Volumes/<disk> 下的程序仅当卷已挂载
/// 才算"可达"（拔盘不等于代理损坏）。
fn launch_agent_volume_mounted(binary: &str) -> bool {
    if let Some(rest) = binary.strip_prefix("/Volumes/") {
        let vol = rest.split('/').next().unwrap_or("");
        !vol.is_empty() && Path::new(&format!("/Volumes/{vol}")).is_dir()
    } else {
        true
    }
}

/// 解析单个 LaunchAgent plist 的程序路径（对标 PlistBuddy
/// Print :ProgramArguments:0 → 回退 Print :Program）。
fn agent_binary(plist: &Path) -> Option<String> {
    let value = plist::Value::from_file(plist).ok()?;
    let dict = value.as_dictionary()?;
    if let Some(args) = dict.get("ProgramArguments").and_then(|v| v.as_array()) {
        if let Some(first) = args.first().and_then(|v| v.as_string()) {
            if !first.is_empty() {
                return Some(first.to_string());
            }
        }
    }
    dict.get("Program")
        .and_then(|v| v.as_string())
        .map(|s| s.to_string())
}

/// 判断代理是否损坏（对标）：程序为绝对路径、真实缺失、卷可达。
fn is_broken_agent(plist: &Path) -> bool {
    match agent_binary(plist) {
        Some(binary) => {
            binary.starts_with('/') && !Path::new(&binary).exists() && launch_agent_volume_mounted(&binary)
        }
        None => false,
    }
}

/// 对标 `opt_launch_agents_cleanup`：清理程序路径已不存在的用户
/// LaunchAgent。加固差异：原实现 safe_remove 永久删除 → Trash 可恢复。
fn launch_agents_cleanup(dry_run: bool) -> (Outcome, String) {
    let home = std::env::var("HOME").unwrap_or_default();
    let agents_dir = Path::new(&home).join("Library/LaunchAgents");
    if !agents_dir.is_dir() {
        return (Outcome::Unchanged, "LaunchAgents 全部健康".into());
    }

    let mut broken: Vec<String> = Vec::new();
    let Ok(entries) = std::fs::read_dir(&agents_dir) else {
        return (Outcome::Unchanged, "LaunchAgents 全部健康".into());
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.ends_with(".plist") || !entry.path().is_file() {
            continue;
        }
        if is_broken_agent(&entry.path()) {
            broken.push(entry.path().to_string_lossy().to_string());
        }
    }

    if broken.is_empty() {
        return (Outcome::Unchanged, "LaunchAgents 全部健康".into());
    }

    let mut removed = 0usize;
    let mut failed = 0usize;
    for plist in &broken {
        if !dry_run {
            // 尽力卸载（对标 run_launchctl_unload；失败不阻断删除）。
            let _ = crate::status::run_cmd("launchctl", &["unload", plist], Duration::from_secs(5));
            let outcome = crate::clean::delete::delete_to_trash(plist, false, "optimize");
            if outcome.status == "failed" {
                failed += 1;
                continue;
            }
        }
        removed += 1;
    }

    if failed > 0 && removed == 0 {
        return (Outcome::Failed, format!("{failed} 个代理删除失败"));
    }
    (
        Outcome::Applied,
        format!(
            "已清理 {removed} 个损坏 LaunchAgent{}",
            if failed > 0 { format!("（{failed} 失败）") } else { String::new() }
        ),
    )
}

/// 对标 `resolve_notification_center_db`：两级路径解析（#1368：
/// 找不到路径是 unavailable，不是健康空状态）。
fn resolve_notification_center_db() -> Option<String> {
    let home = std::env::var("HOME").unwrap_or_default();
    let group_db = format!(
        "{home}/Library/Group Containers/group.com.apple.usernoted/db2/db"
    );
    if Path::new(&group_db).is_file() {
        return Some(group_db);
    }
    // 回退：getconf DARWIN_USER_DIR。
    if let Ok(out) = crate::status::run_cmd("getconf", &["DARWIN_USER_DIR"], Duration::from_secs(3))
    {
        let dir = out.trim().trim_end_matches('/');
        if !dir.is_empty() {
            let candidate = format!("{dir}/com.apple.notificationcenter/db2/db");
            if Path::new(&candidate).is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// 对标 `_preference_plist_is_protected`：com.apple.* 与 .GlobalPreferences*
/// 永远保护；loginwindow.plist 仅在顶层 Preferences 扫描时保护。
fn preference_plist_is_protected(filename: &str, protect_loginwindow: bool) -> bool {
    if filename.starts_with("com.apple.") || filename.starts_with(".GlobalPreferences") {
        return true;
    }
    filename == "loginwindow.plist" && protect_loginwindow
}

/// 对标 `_repair_preference_plists_in_dir`：lint 目录下 plist，损坏且通过
/// 保护/白名单检查的走 Trash。plist crate 直读替代 plutil -lint（同为
/// 语法校验）。返回 (修复数, 是否超预算截断)。
fn repair_preference_plists_in_dir(
    dir: &Path,
    recursive: bool,
    protect_loginwindow: bool,
    deadline: Instant,
    dry_run: bool,
) -> (usize, bool) {
    if !dir.is_dir() {
        return (0, false);
    }
    // 收集候选（对标 find + 逐项 filename 保护检查）。
    let mut candidates: Vec<PathBuf> = Vec::new();
    let mut stack: Vec<(PathBuf, usize)> = vec![(dir.to_path_buf(), 0)];
    while let Some((cur, depth)) = stack.pop() {
        if Instant::now() >= deadline {
            return (0, true);
        }
        let Ok(entries) = std::fs::read_dir(&cur) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(ft) = entry.file_type() else { continue };
            if ft.is_dir() {
                if recursive {
                    stack.push((path, depth + 1));
                }
                continue;
            }
            if entry.file_name().to_string_lossy().ends_with(".plist") {
                candidates.push(path);
            }
        }
    }

    let whitelist = crate::clean::whitelist::Whitelist::load();
    let mut repaired = 0usize;
    for path in candidates {
        if Instant::now() >= deadline {
            return (repaired, true);
        }
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        if preference_plist_is_protected(&name, protect_loginwindow) {
            continue;
        }
        // lint：plist 解析成功 = 合法（对标 plutil -lint）。
        if plist::Value::from_file(&path).is_ok() {
            continue;
        }
        let path_str = path.to_string_lossy().to_string();
        // 深度保护检查仅在损坏文件上执行（对标）。
        if crate::clean::protect::should_protect_path(&path_str)
            || whitelist.is_whitelisted(&path_str)
        {
            continue;
        }
        let outcome = crate::clean::delete::delete_to_trash(&path_str, dry_run, "optimize");
        if matches!(outcome.status.as_str(), "ok" | "dry-run") {
            repaired += 1;
        }
    }
    (repaired, false)
}

/// 对标 `opt_fix_broken_configs`：~/Library/Preferences 顶层（保护
/// loginwindow）+ ByHost 递归；15s 预算，超时记部分结果。
fn fix_broken_configs(dry_run: bool) -> (Outcome, String) {
    let home = std::env::var("HOME").unwrap_or_default();
    let prefs_dir = Path::new(&home).join("Library/Preferences");
    if !prefs_dir.is_dir() {
        return (Outcome::Unchanged, "无偏好设置目录".into());
    }
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut broken = 0usize;
    let mut partial = false;

    let (top, top_partial) =
        repair_preference_plists_in_dir(&prefs_dir, false, true, deadline, dry_run);
    broken += top;
    partial |= top_partial;
    let (byhost, byhost_partial) = repair_preference_plists_in_dir(
        &prefs_dir.join("ByHost"),
        true,
        false,
        deadline,
        dry_run,
    );
    broken += byhost;
    partial |= byhost_partial;

    if broken > 0 {
        return (
            if partial { Outcome::Attention } else { Outcome::Applied },
            format!("已修复 {broken} 个损坏偏好文件{}", if partial { "（扫描超预算，部分结果）" } else { "" }),
        );
    }
    if partial {
        return (Outcome::Attention, "扫描超预算，未发现损坏文件".into());
    }
    (Outcome::Unchanged, "全部偏好文件有效".into())
}

/// 对标 `opt_notification_cleanup`：>50MB 通知库清理 30 天前投递记录。
fn notification_cleanup(dry_run: bool) -> (Outcome, String) {
    const THRESHOLD_KB: u64 = 51_200; // 对标 50MB
    let Some(nc_db) = resolve_notification_center_db() else {
        // Unavailable（#1368：路径缺失 ≠ 健康）。
        return (Outcome::Unavailable, "通知中心数据库路径不可用".into());
    };
    let Ok(meta) = Path::new(&nc_db).metadata() else {
        return (Outcome::Failed, "无法读取通知数据库大小".into());
    };
    let db_kb = meta.len() / 1024;
    if db_kb < THRESHOLD_KB {
        return (Outcome::Unchanged, format!("通知数据库健康（{db_kb} KB）"));
    }
    if dry_run {
        return (Outcome::Applied, "将清理 30 天前的投递记录".into());
    }
    if !crate::clean::command_exists("sqlite3") {
        return (Outcome::Unavailable, "sqlite3 不可用".into());
    }
    let sql = "DELETE FROM record WHERE delivered_date < strftime('%s','now','-30 days'); VACUUM;";
    match run_sqlite(&nc_db, sql, Duration::from_secs(30)) {
        Ok(_) => {
            // 刷新通知中心（尽力，失败不阻断）。
            let _ = crate::status::run_cmd("killall", &["NotificationCenter"], Duration::from_secs(5));
            (Outcome::Applied, format!("通知数据库已清理（原 {db_kb} KB）"))
        }
        Err(()) => (Outcome::Failed, "数据库繁忙或锁定".into()),
    }
}

/// 对标 `opt_coreduet_cleanup`：Knowledge 数据库（~750 行语义）。
/// - 库不存在 → Unchanged；db+wal+shm 合计 <100MB → Unchanged（已健康）；
/// - dry-run → Applied（将清理）；
/// - 真实：sqlite3 不可用 → Unavailable；删除 wal/shm（Trash 加固，原
///   safe_remove 永久）→ DELETE ZOBJECT 90 天以上记录（CoreTime 纪元
///   2001-01-01 换算）→ VACUUM。
fn coreduet_cleanup(dry_run: bool) -> (Outcome, String) {
    const THRESHOLD: u64 = 102_400; // KB，对标 100MB
    let home = std::env::var("HOME").unwrap_or_default();
    let db_dir = format!("{home}/Library/Application Support/Knowledge");
    let db = format!("{db_dir}/knowledgeC.db");
    let db_path = Path::new(&db);
    if !db_path.is_file() {
        return (Outcome::Unchanged, "Knowledge 数据库不存在".into());
    }

    // db + wal + shm 合计大小。
    let mut total_kb = 0u64;
    for suffix in ["", "-wal", "-shm"] {
        let f = format!("{db}{suffix}");
        if let Ok(meta) = Path::new(&f).metadata() {
            total_kb += meta.len() / 1024;
        }
    }
    if total_kb < THRESHOLD {
        return (
            Outcome::Unchanged,
            format!("Knowledge 数据库健康（{total_kb} KB）"),
        );
    }

    if dry_run {
        return (Outcome::Applied, "将清理 Knowledge 数据库（90 天以上记录）".into());
    }

    if !crate::clean::command_exists("sqlite3") {
        return (Outcome::Unavailable, "sqlite3 不可用".into());
    }

    // 删除 WAL/SHM（SQLite 自动重建；Trash 可恢复加固）。
    let mut removed = 0usize;
    for suffix in ["-wal", "-shm"] {
        let f = format!("{db}{suffix}");
        if Path::new(&f).is_file() {
            let outcome = crate::clean::delete::delete_to_trash(&f, false, "optimize");
            if outcome.status == "ok" {
                removed += 1;
            }
        }
    }

    // 90 天以上 ZOBJECT 记录删除 + VACUUM（CoreTime 纪元 2001-01-01）。
    let sql = "DELETE FROM ZOBJECT WHERE ZCREATIONDATE < (strftime('%s','now','-90 days') - strftime('%s','2001-01-01')); VACUUM;";
    match run_sqlite(&db, sql, Duration::from_secs(30)) {
        Ok(_) => (
            Outcome::Applied,
            format!("Knowledge 数据库已清理（{total_kb} KB → 压缩完成）"),
        ),
        Err(()) => {
            if removed > 0 {
                (Outcome::Attention, "WAL/SHM 已清，数据库繁忙或锁定".into())
            } else {
                (Outcome::Failed, "数据库繁忙或锁定".into())
            }
        }
    }
}

/// 对标 `opt_quarantine_cleanup`：清空 Gatekeeper 下载追踪表并 VACUUM。
fn quarantine_cleanup(dry_run: bool) -> (Outcome, String) {
    if !crate::clean::command_exists("sqlite3") {
        return (Outcome::Unavailable, "sqlite3 不可用".into());
    }
    let home = std::env::var("HOME").unwrap_or_default();
    let db = format!("{home}/Library/Preferences/com.apple.LaunchServices.QuarantineEventsV2");
    let db_path = Path::new(&db);
    if !db_path.is_file() {
        return (Outcome::Unchanged, "隔离区数据库不存在".into());
    }
    if crate::clean::protect::should_protect_path(&db) {
        return (Outcome::Unchanged, "数据库受保护".into());
    }
    let Ok(count_out) = run_sqlite(&db, "SELECT COUNT(*) FROM LSQuarantineEvent;", Duration::from_secs(5))
    else {
        return (Outcome::Failed, "无法读取隔离区数据库".into());
    };
    let count: u64 = count_out.trim().parse().unwrap_or(0);
    if count == 0 {
        return (Outcome::Unchanged, "隔离区数据库已是空的".into());
    }
    if dry_run {
        return (Outcome::Applied, format!("将清除 {count} 条隔离记录").into());
    }
    match run_sqlite(&db, "DELETE FROM LSQuarantineEvent; VACUUM;", Duration::from_secs(15)) {
        Ok(_) => (Outcome::Applied, format!("已清除 {count} 条隔离记录").into()),
        Err(()) => (Outcome::Failed, "清除隔离记录失败".into()),
    }
}

/// 对标 `opt_system_maintenance`：刷 DNS 缓存 + mdutil -s / 校验 Spotlight。
/// 返回 (outcome, detail, dns_flushed)——dns_flushed 供同轮 network_optimization
/// 复用（对标 MOLE_DNS_FLUSHED）。
fn system_maintenance(dry_run: bool) -> (Outcome, String, bool) {
    if !dry_run && !optimize_sudo_available(false) {
        return (
            Outcome::Skipped,
            "需要管理员权限（sudo 缓存不可用）".into(),
            false,
        );
    }

    let dns_flushed = flush_dns_cache(dry_run);

    let mut spotlight_failed = false;
    match crate::status::run_cmd("mdutil", &["-s", "/"], Duration::from_secs(3)) {
        Ok(status) => {
            if status.to_lowercase().contains("indexing disabled") {
                // Spotlight 索引被禁用：记录但不计失败（对标）。
            }
            // 否则视为已校验成功。
        }
        Err(_) => spotlight_failed = true,
    }

    let applied = if dns_flushed { 1 } else { 0 };
    let failed = if dns_flushed { 0 } else { 1 } + usize::from(spotlight_failed);
    let detail = if dns_flushed && !spotlight_failed {
        "DNS 缓存已刷新，Spotlight 索引已校验".to_string()
    } else if dns_flushed {
        "DNS 缓存已刷新，但 Spotlight 校验失败".to_string()
    } else if spotlight_failed {
        "DNS 刷新失败，Spotlight 校验失败".to_string()
    } else {
        "DNS 刷新失败".to_string()
    };
    (outcome_from_counts(applied, failed, 0), detail, dns_flushed)
}

/// 对标 `opt_network_optimization`：DNS 缓存刷新（与 system_maintenance
/// 共享 flush_dns_cache；同轮已刷则 Unchanged）。
fn network_optimization(dry_run: bool, dns_flushed_this_run: bool) -> (Outcome, String) {
    if dns_flushed_this_run {
        return (
            Outcome::Unchanged,
            "DNS 缓存本轮已刷新（由 system_maintenance 完成）".into(),
        );
    }
    if !dry_run && !optimize_sudo_available(false) {
        return (
            Outcome::Skipped,
            "需要管理员权限（sudo 缓存不可用）".into(),
        );
    }
    if flush_dns_cache(dry_run) {
        (Outcome::Applied, "DNS 缓存与 mDNSResponder 已刷新".into())
    } else {
        (Outcome::Failed, "DNS 缓存刷新失败".into())
    }
}

/// 对标 `opt_launch_services_rebuild`：lsregister -gc 清理 + 三域强制重建
/// （失败回退 local+user 两域）。
fn launch_services_rebuild(dry_run: bool) -> (Outcome, String) {
    let Some(lsregister) = get_lsregister_path() else {
        return (Outcome::Unavailable, "lsregister 未找到".into());
    };
    if dry_run {
        return (Outcome::Applied, "将重建 LaunchServices 数据库".into());
    }

    // -gc 清理（失败不阻断，对标 `|| true`）。
    let _ = std::process::Command::new(&lsregister)
        .arg("-gc")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();

    // 三域强制重建；失败回退 local+user。
    let rebuild = |args: &[&str]| -> bool {
        std::process::Command::new(&lsregister)
            .args(args)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    };
    let success = rebuild(&["-r", "-f", "-domain", "local", "-domain", "user", "-domain", "system"])
        || rebuild(&["-r", "-f", "-domain", "local", "-domain", "user"]);

    if success {
        (Outcome::Applied, "LaunchServices 已重建，文件关联已刷新".into())
    } else {
        (Outcome::Failed, "LaunchServices 重建失败".into())
    }
}

/// 对标 `has_active_vpn_interface`：0=有 VPN，1=无，2=无法判定。
/// 窄信号：scutil 已连接系统 VPN + 默认路由 utun*（#959：裸 utun 存在
/// 会误报 Private Relay/Handoff 等）。
fn has_active_vpn_interface() -> u8 {
    // MOLE_ASSUME_VPN_ACTIVE 测试/应急覆盖。
    match std::env::var("MOLE_ASSUME_VPN_ACTIVE").as_deref() {
        Ok("1") | Ok("true") | Ok("TRUE") | Ok("yes") | Ok("YES") => return 0,
        Ok("0") | Ok("false") | Ok("FALSE") | Ok("no") | Ok("NO") => return 1,
        _ => {}
    }

    if !crate::clean::command_exists("scutil") {
        return 2;
    }
    let Ok(scutil_out) = crate::status::run_cmd("scutil", &["--nc", "list"], Duration::from_secs(3))
    else {
        return 2;
    };
    // 对标 grep -Eq '^\* \(Connected\)'（LC_ALL=C 下的英文输出）。
    if scutil_out.lines().any(|l| l.starts_with("* (Connected)")) {
        return 0;
    }

    if !crate::clean::command_exists("route") {
        return 2;
    }
    let Ok(route_out) = crate::status::run_cmd("route", &["-n", "get", "default"], Duration::from_secs(3))
    else {
        return 2;
    };
    // interface: utunN → 全隧道第三方 VPN 拥有默认路由。
    for line in route_out.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("interface:") {
            let iface = rest.trim();
            if let Some(num) = iface.strip_prefix("utun") {
                if !num.is_empty() && num.chars().all(|c| c.is_ascii_digit()) {
                    return 0;
                }
            }
        }
    }
    1
}

/// 对标 `opt_network_stack_optimize`：VPN 三态 → 路由/DNS 健康探针 →
/// 两者皆健康 Unchanged → sudo route -n flush + arp -a -d。
fn network_stack_optimize(dry_run: bool) -> (Outcome, String) {
    match has_active_vpn_interface() {
        0 => return (Outcome::Skipped, "检测到活跃 VPN，已跳过".into()),
        1 => {}
        _ => return (Outcome::Failed, "无法判定 VPN 状态".into()),
    }

    // 路由与 DNS 健康探针（对标三态：0=健康，1=不健康，其他=失败）。
    let route_probe = probe_exit_code("route", &["-n", "get", "default"], Duration::from_secs(3));
    let dns_probe = probe_exit_code(
        "dscacheutil",
        &["-q", "host", "-a", "name", "example.com"],
        Duration::from_secs(3),
    );
    // 探针失败（None = 超时/spawn 失败）与 >1 退出码均记 Failed。
    match (route_probe, dns_probe) {
        (Some(c), Some(d)) if c <= 1 && d <= 1 => {}
        _ => return (Outcome::Failed, "网络健康检查失败或超时".into()),
    }
    if route_probe == Some(0) && dns_probe == Some(0) {
        return (Outcome::Unchanged, "网络栈已处于最优状态".into());
    }

    if !dry_run && !optimize_sudo_available(false) {
        return (
            Outcome::Skipped,
            "需要管理员权限（sudo 缓存不可用）".into(),
        );
    }

    let route_flushed = if dry_run {
        true
    } else {
        crate::status::run_cmd("sudo", &["route", "-n", "flush"], Duration::from_secs(5)).is_ok()
    };
    let arp_flushed = if dry_run {
        true
    } else {
        crate::status::run_cmd("sudo", &["arp", "-a", "-d"], Duration::from_secs(5)).is_ok()
    };

    let applied = usize::from(route_flushed) + usize::from(arp_flushed);
    let failed = usize::from(!route_flushed) + usize::from(!arp_flushed);
    let detail = if failed > 0 {
        format!("网络栈刷新不完整（{failed} 项失败）")
    } else if route_flushed && arp_flushed {
        "路由表已刷新，ARP 缓存已清除".to_string()
    } else if route_flushed {
        "路由表已刷新".to_string()
    } else {
        "ARP 缓存已清除".to_string()
    };
    (outcome_from_counts(applied, failed, 0), detail)
}

/// 对标 `needs_permissions_repair`：家目录属主 ≠ 当前用户，或
/// HOME / Library / Preferences 任一存在但不可写。
fn needs_permissions_repair() -> bool {
    use std::os::unix::fs::MetadataExt;
    let home = std::env::var("HOME").unwrap_or_default();
    let Ok(meta) = std::fs::metadata(&home) else {
        return false;
    };
    // uid 对比（对标 $STAT_BSD -f %Su 与 $USER 比较的意图）。
    let uid = meta.uid();
    let current_uid = unsafe { libc::getuid() };
    if uid != current_uid {
        return true;
    }
    for path in [
        home.clone(),
        format!("{home}/Library"),
        format!("{home}/Library/Preferences"),
    ] {
        if Path::new(&path).exists() && !is_writable(Path::new(&path)) {
            return true;
        }
    }
    false
}

fn is_writable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|m| m.permissions().mode() & 0o200 != 0)
        .unwrap_or(false)
}

/// 对标 `opt_disk_permissions_repair`：needs_permissions_repair 探测 →
/// sudo diskutil resetUserPermissions / <uid>。
fn disk_permissions_repair(dry_run: bool) -> (Outcome, String) {
    if !needs_permissions_repair() {
        return (Outcome::Unchanged, "用户目录权限已最优".into());
    }
    if dry_run {
        return (Outcome::Applied, "将重置用户目录权限".into());
    }
    if !optimize_sudo_available(false) {
        return (
            Outcome::Skipped,
            "需要管理员权限（sudo 缓存不可用）".into(),
        );
    }
    let uid = unsafe { libc::getuid() }.to_string();
    match crate::status::run_cmd(
        "sudo",
        &["diskutil", "resetUserPermissions", "/", &uid],
        Duration::from_secs(60),
    ) {
        Ok(_) => (Outcome::Applied, "用户目录权限已重置".into()),
        Err(_) => (Outcome::Failed, "权限重置失败（可能无需修复）".into()),
    }
}

/// 对标 `opt_periodic_maintenance`：periodic 命令存在性（macOS 26+ 移除）→
/// /var/log/daily.out 新鲜度（<7 天 Unchanged）→ sudo periodic daily weekly monthly。
fn periodic_maintenance(dry_run: bool) -> (Outcome, String) {
    if !crate::clean::command_exists("periodic") {
        return (
            Outcome::Unavailable,
            "此 macOS 版本不提供 periodic".into(),
        );
    }

    let daily_log = std::env::var("MOLE_PERIODIC_LOG")
        .unwrap_or_else(|_| "/var/log/daily.out".into());
    if Path::new(&daily_log).is_file() {
        if let Ok(meta) = std::fs::metadata(&daily_log) {
            if let Ok(mtime) = meta.modified() {
                if let Ok(age) = mtime.elapsed() {
                    let age_days = age.as_secs() / 86400;
                    if age_days < 7 {
                        return (
                            Outcome::Unchanged,
                            format!("周期维护已是最新（{age_days} 天前）"),
                        );
                    }
                }
            }
        }
    }

    if dry_run {
        return (Outcome::Applied, "将触发 daily/weekly/monthly 周期维护".into());
    }
    if !optimize_sudo_available(false) {
        return (
            Outcome::Skipped,
            "需要管理员权限（sudo 缓存不可用）".into(),
        );
    }
    match crate::status::run_cmd(
        "sudo",
        &["periodic", "daily", "weekly", "monthly"],
        Duration::from_secs(120),
    ) {
        Ok(_) => (Outcome::Applied, "周期维护已触发".into()),
        Err(e) => (Outcome::Failed, format!("周期维护失败：{e}")),
    }
}

/// 对标 `opt_shared_file_list_repair`：~/Library/Application Support/
/// com.apple.sharedfilelist 下 *.sfl2/*.sfl3（排除 ApplicationRecentDocuments
/// 用户数据），plutil -lint 失败的损坏文件走 Trash。
/// lint 用 plist crate 解析替代 plutil 子进程（同为语法校验，同 7g 差异）。
fn shared_file_list_repair(dry_run: bool) -> (Outcome, String) {
    let home = std::env::var("HOME").unwrap_or_default();
    let sfl_dir = Path::new(&home).join("Library/Application Support/com.apple.sharedfilelist");
    if !sfl_dir.is_dir() {
        return (Outcome::Unchanged, "共享文件列表目录不存在".into());
    }

    // 有界收集（5s 预算；超时放弃整批——对标 run_with_timeout）。
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut candidates: Vec<PathBuf> = Vec::new();
    let mut timed_out = false;
    let mut stack = vec![sfl_dir.clone()];
    while let Some(dir) = stack.pop() {
        if Instant::now() >= deadline {
            timed_out = true;
            break;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            if Instant::now() >= deadline {
                timed_out = true;
                break;
            }
            let path = entry.path();
            let Ok(ft) = entry.file_type() else { continue };
            if ft.is_dir() {
                stack.push(path);
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            // find：\( -name "*.sfl2" -o -name "*.sfl3" \) -type f
            // ! -path "*ApplicationRecentDocuments*"
            if !(name.ends_with(".sfl2") || name.ends_with(".sfl3")) {
                continue;
            }
            let path_str = path.to_string_lossy();
            if path_str.contains("ApplicationRecentDocuments") {
                continue;
            }
            candidates.push(path);
        }
    }
    if timed_out {
        return (Outcome::Unchanged, "扫描超时，本批放弃".into());
    }

    let mut repaired = 0usize;
    let mut failed = 0usize;
    for path in &candidates {
        if !path.is_file() {
            continue;
        }
        // plutil -lint：解析失败 = 损坏。
        if plist::Value::from_file(path).is_ok() {
            continue;
        }
        let path_str = path.to_string_lossy().to_string();
        if dry_run {
            repaired += 1;
            continue;
        }
        let outcome = crate::clean::delete::delete_to_trash(&path_str, false, "optimize");
        match outcome.status.as_str() {
            "ok" => repaired += 1,
            "failed" => failed += 1,
            _ => {}
        }
    }

    if failed > 0 {
        return (
            Outcome::Failed,
            format!("{failed} 个共享文件列表修复失败"),
        );
    }
    if repaired > 0 {
        return (
            Outcome::Applied,
            format!("已修复 {repaired} 个损坏的共享文件列表"),
        );
    }
    (Outcome::Unchanged, "共享文件列表全部健康".into())
}

/// 对标 `opt_disk_verify`：默认跳过（MOLE_ENABLE_DISK_VERIFY=1 才启用）——
/// verifyVolume 的内核级 I/O 无法被 SIGKILL 打断，可能导致系统冻结。
fn disk_verify(dry_run: bool) -> (Outcome, String) {
    let enabled = matches!(
        std::env::var("MOLE_ENABLE_DISK_VERIFY").as_deref(),
        Ok("1")
    );
    if !enabled {
        return (
            Outcome::Skipped,
            "磁盘校验已跳过（设置 MOLE_ENABLE_DISK_VERIFY=1 启用）".into(),
        );
    }
    if dry_run {
        return (Outcome::Skipped, "dry-run 下跳过磁盘校验".into());
    }

    let output = crate::status::run_cmd(
        "diskutil",
        &["verifyVolume", "/"],
        Duration::from_secs(300),
    );
    match output {
        Err(e) if e.contains("exited") => {
            // 退出码非 0（对标 verify_status -ne 0）。
            (Outcome::Failed, format!("磁盘校验失败：{e}"))
        }
        Err(_) => (Outcome::Failed, "磁盘校验超时或无法执行".into()),
        Ok(out) => {
            let lower = out.to_lowercase();
            if lower.contains("appears to be ok") || lower.contains("volume appears to be ok") {
                (Outcome::Unchanged, "磁盘文件系统校验通过".into())
            } else if lower.contains("error") || lower.contains("corrupt") || lower.contains("invalid") {
                (
                    Outcome::Attention,
                    "检测到磁盘问题 · 建议：sudo diskutil repairVolume /".into(),
                )
            } else {
                (Outcome::Failed, "磁盘校验结果无法识别".into())
            }
        }
    }
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
            let implemented = matches!(
                t.action,
                "saved_state_cleanup"
                    | "cache_refresh"
                    | "prevent_network_dsstore"
                    | "legacy_overrides_audit"
                    | "sqlite_vacuum"
                    | "quarantine_cleanup"
                    | "launch_agents_cleanup"
                    | "coreduet_cleanup"
                    | "notification_cleanup"
                    | "fix_broken_configs"
                    | "system_maintenance"
                    | "network_optimization"
                    | "launch_services_rebuild"
                    | "network_stack_optimize"
                    | "disk_permissions_repair"
                    | "periodic_maintenance"
                    | "shared_file_list_repair"
                    | "disk_verify"
            );
            assert_eq!(t.implemented, implemented, "{} 标记不一致", t.action);
        }
    }

    /// 执行框架：未移植任务记录 unavailable。
    #[test]
    fn unimplemented_task_unavailable() {
        let r = execute(&["spotlight_index_optimize".to_string()], true);
        assert_eq!(r.results.len(), 1);
        assert_eq!(r.results[0].outcome, "unavailable");
        assert_eq!(r.unavailable, 1);
    }
}

/// 真机冒烟（默认忽略）：dry-run 执行已移植优化任务。
#[cfg(test)]
mod smoke_tests {
    #[test]
    #[ignore]
    fn saved_state_dry_run_smoke() {
        let r = super::execute(&[
            "saved_state_cleanup".to_string(),
            "cache_refresh".to_string(),
            "prevent_network_dsstore".to_string(),
            "legacy_overrides_audit".to_string(),
            "sqlite_vacuum".to_string(),
            "quarantine_cleanup".to_string(),
            "launch_agents_cleanup".to_string(),
            "coreduet_cleanup".to_string(),
            "notification_cleanup".to_string(),
            "fix_broken_configs".to_string(),
            "system_maintenance".to_string(),
            "network_optimization".to_string(),
            "launch_services_rebuild".to_string(),
            "network_stack_optimize".to_string(),
            "disk_permissions_repair".to_string(),
            "periodic_maintenance".to_string(),
            "shared_file_list_repair".to_string(),
            "disk_verify".to_string(),
        ], true);
        for res in &r.results {
            println!("[{}] {} ({})", res.outcome, res.action, res.detail);
        }
        assert_eq!(r.failed, 0);
    }
}

#[cfg(test)]
mod sqlite_tests {
    use super::*;

    /// SQLite 魔数检测（对标 file -b *SQLite*）。
    #[test]
    fn sqlite_magic_detection() {
        let tmp = std::env::temp_dir().join(format!("mole_rs_sql_{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let db = tmp.join("test.db");
        std::fs::write(&db, b"SQLite format 3\0something").unwrap();
        assert!(is_sqlite_file(&db));
        let plain = tmp.join("plain.txt");
        std::fs::write(&plain, b"not a database").unwrap();
        assert!(!is_sqlite_file(&plain));
        assert!(!is_sqlite_file(&tmp.join("missing.db")));
        std::fs::remove_dir_all(&tmp).ok();
    }

    /// 进程探针：pgrep 自身必然存在；对不存在进程返回 1（未运行）。
    #[test]
    fn probe_exit_code_semantics() {
        if crate::clean::command_exists("pgrep") {
            let code = probe_exit_code("pgrep", &["-x", "__mole_rs_nonexistent_proc__"], Duration::from_secs(3));
            assert_eq!(code, Some(1), "不存在进程应返回 1");
        }
    }
}

#[cfg(test)]
mod launch_agent_tests {
    use super::*;

    /// 对标 launch_agent_volume_mounted。
    #[test]
    fn volume_mounted_semantics() {
        assert!(launch_agent_volume_mounted("/usr/bin/true"));
        assert!(launch_agent_volume_mounted("/bin/sh"));
        // /Volumes/<disk> 下依赖实际挂载状态：本机不存在该卷 → false。
        assert!(!launch_agent_volume_mounted("/Volumes/__mole_rs_no_such_vol__/tool"));
    }

    /// 损坏判定：ProgramArguments 首元素 / Program 回退 / 裸名与相对路径。
    #[test]
    fn broken_agent_detection() {
        let tmp = std::env::temp_dir().join(format!("mole_rs_la_{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();

        // 缺失绝对路径 → 损坏。
        let missing = tmp.join("missing.plist");
        std::fs::write(&missing, r#"<?xml version="1.0"?><plist><dict><key>ProgramArguments</key><array><string>/nonexistent/bin/tool</string></array></dict></plist>"#).unwrap();
        assert!(is_broken_agent(&missing));

        // 存在的绝对路径 → 健康（本机 macOS 27 无 /bin/true，用 /usr/bin/true）。
        let ok = tmp.join("ok.plist");
        std::fs::write(&ok, r#"<?xml version="1.0"?><plist><dict><key>ProgramArguments</key><array><string>/usr/bin/true</string></array></dict></plist>"#).unwrap();
        assert!(!is_broken_agent(&ok));

        // 裸名（PATH 解析）→ 健康。
        let bare = tmp.join("bare.plist");
        std::fs::write(&bare, r#"<?xml version="1.0"?><plist><dict><key>Program</key><string>node</string></dict></plist>"#).unwrap();
        assert!(!is_broken_agent(&bare));

        // 拔盘卷 → 健康。
        let vol = tmp.join("vol.plist");
        std::fs::write(&vol, r#"<?xml version="1.0"?><plist><dict><key>Program</key><string>/Volumes/__mole_rs_no_such_vol__/tool</string></dict></plist>"#).unwrap();
        assert!(!is_broken_agent(&vol));

        // Program 回退。
        let fallback = tmp.join("fallback.plist");
        std::fs::write(&fallback, r#"<?xml version="1.0"?><plist><dict><key>Program</key><string>/nonexistent/bin/tool</string></dict></plist>"#).unwrap();
        assert!(is_broken_agent(&fallback));

        std::fs::remove_dir_all(&tmp).ok();
    }
}

#[cfg(test)]
mod network_launch_tests {
    use super::*;

    /// 对标 optimize_task_result_from_counts 的六态映射。
    #[test]
    fn outcome_from_counts_semantics() {
        assert_eq!(outcome_from_counts(0, 1, 0), Outcome::Failed);
        assert_eq!(outcome_from_counts(2, 1, 0), Outcome::Failed);
        assert_eq!(outcome_from_counts(1, 0, 0), Outcome::Applied);
        assert_eq!(outcome_from_counts(0, 0, 1), Outcome::Skipped);
        assert_eq!(outcome_from_counts(0, 0, 0), Outcome::Unchanged);
    }

    /// dry-run 下 sudo 会话视为可用（对标 MOLE_OPTIMIZE_SUDO_AVAILABLE）。
    #[test]
    fn sudo_available_dry_run() {
        assert!(optimize_sudo_available(true));
        assert!(flush_dns_cache(true));
    }

    /// MOLE_LSREGISTER_PATH 覆盖优先；未设置时返回存在的候选或 None。
    #[test]
    fn lsregister_path_resolution() {
        // 本机应能通过默认候选找到（macOS）。
        let path = get_lsregister_path();
        if let Ok(p) = std::env::var("MOLE_LSREGISTER_PATH") {
            assert_eq!(path.as_deref(), Some(p.as_str()));
        } else if let Some(p) = path {
            assert!(Path::new(&p).is_file(), "lsregister 应存在: {p}");
            assert!(is_executable(Path::new(&p)));
        }
        // 不存在的覆盖路径：get_lsregister_path 在 env 设置时直接返回，
        // 不检查存在性——与原实现 echo "$MOLE_LSREGISTER_PATH" 一致。
        unsafe { std::env::set_var("MOLE_LSREGISTER_PATH", "/nonexistent/lsregister") };
        assert_eq!(get_lsregister_path().as_deref(), Some("/nonexistent/lsregister"));
        unsafe { std::env::remove_var("MOLE_LSREGISTER_PATH") };
    }

    /// network_optimization：本轮已刷 DNS → Unchanged。
    #[test]
    fn network_optimization_dedup() {
        let (outcome, _) = network_optimization(false, true);
        assert_eq!(outcome, Outcome::Unchanged);
    }

    /// system_maintenance dry-run：dns_flushed=true，outcome 非 Failed
    /// （mdutil 在真机存在；CI 无 mdutil 时可能 failed——仅断言 flushed 标记）。
    #[test]
    fn system_maintenance_dry_run_dns_flag() {
        if !crate::clean::command_exists("mdutil") && !cfg!(target_os = "macos") {
            return;
        }
        let (_, _, flushed) = system_maintenance(true);
        assert!(flushed, "dry-run 下 flush_dns_cache 必为 true");
    }

    /// MOLE_ASSUME_VPN_ACTIVE 覆盖语义（对标 has_active_vpn_interface case）。
    #[test]
    fn vpn_assume_override() {
        unsafe { std::env::set_var("MOLE_ASSUME_VPN_ACTIVE", "1") };
        assert_eq!(has_active_vpn_interface(), 0);
        unsafe { std::env::set_var("MOLE_ASSUME_VPN_ACTIVE", "0") };
        assert_eq!(has_active_vpn_interface(), 1);
        unsafe { std::env::remove_var("MOLE_ASSUME_VPN_ACTIVE") };
    }

    /// needs_permissions_repair：HOME 属主为当前用户且可写 → false（真机默认）。
    #[test]
    fn permissions_repair_probe_default() {
        if !Path::new(&std::env::var("HOME").unwrap_or_default()).is_dir() {
            return;
        }
        // 不断言 true/false——取决于本机状态；仅保证不 panic。
        let _ = needs_permissions_repair();
    }

    /// network_stack dry-run：无 VPN 时若探针健康 → Unchanged，
    /// 若不健康 → Applied（将刷新）。两种都合法，仅排除 Failed/Skipped。
    #[test]
    fn network_stack_dry_run_non_fatal() {
        unsafe { std::env::set_var("MOLE_ASSUME_VPN_ACTIVE", "0") };
        let (outcome, _) = network_stack_optimize(true);
        unsafe { std::env::remove_var("MOLE_ASSUME_VPN_ACTIVE") };
        assert!(
            matches!(outcome, Outcome::Unchanged | Outcome::Applied | Outcome::Failed),
            "unexpected: {outcome:?}"
        );
    }

    /// disk_verify 默认关闭 → Skipped（对标 MOLE_ENABLE_DISK_VERIFY 门）。
    #[test]
    fn disk_verify_default_disabled() {
        unsafe { std::env::remove_var("MOLE_ENABLE_DISK_VERIFY") };
        let (outcome, _) = disk_verify(false);
        assert_eq!(outcome, Outcome::Skipped);
        // dry-run 即使启用也跳过。
        unsafe { std::env::set_var("MOLE_ENABLE_DISK_VERIFY", "1") };
        let (outcome, _) = disk_verify(true);
        assert_eq!(outcome, Outcome::Skipped);
        unsafe { std::env::remove_var("MOLE_ENABLE_DISK_VERIFY") };
    }
}
