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
            );
            assert_eq!(t.implemented, implemented, "{} 标记不一致", t.action);
        }
    }

    /// 执行框架：未移植任务记录 unavailable。
    #[test]
    fn unimplemented_task_unavailable() {
        let r = execute(&["fix_broken_configs".to_string()], true);
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
