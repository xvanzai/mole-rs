//! deep_system 系统级清理，对标 `lib/clean/system.sh` 的 `clean_deep_system`
//! 主体家族（sudo 路径）。
//!
//! GUI 约束（CHANGES.md 已记）：原 CLI 经 ensure_sudo_session 交互获取
//! 会话；GUI 仅接受 `sudo -n` 密码缓存，无缓存时整族 Skipped。
//!
//! 永不触碰（对标 AGENTS.md）：/Library/Updates、/macOS Install Data
//! （Software Update 所有，年龄/进程/ plist 探针无法证明扫描-删除窗口内
//! 保持空闲）。
//!
//! 首片范围：缓存/崩溃报告/系统日志/第三方日志四族。macOS 安装器清理
//! （14 天 + 运行中 + 当前版本身份门控）暂缓——身份链复杂，独立子片。

use super::protect;
use super::whitelist::Whitelist;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// 对标 MOLE_TEMP_FILE_AGE_DAYS / MOLE_LOG_AGE_DAYS / MOLE_CRASH_REPORT_AGE_DAYS。
const TEMP_AGE_DAYS: u64 = 7;
const LOG_AGE_DAYS: u64 = 7;
const CRASH_AGE_DAYS: u64 = 7;
/// 扫描预算（对标 system_cleanup_deadline 120s 的单族子集）。
const FAMILY_DEADLINE: Duration = Duration::from_secs(30);

/// 一个系统清理族。
pub struct SystemFamily {
    #[allow(dead_code)]
    pub id: &'static str,
    pub label: &'static str,
    pub root: &'static str,
    /// 文件名模式（对标 find -name；`*` 匹配任意）。
    pub patterns: &'static [&'static str],
    pub age_days: u64,
    pub max_depth: usize,
}

/// 对标 clean_deep_system 的主体四族（+ adobegc.log 行并入第三方日志族）。
pub fn system_families() -> Vec<SystemFamily> {
    vec![
        SystemFamily {
            id: "system_caches",
            label: "System caches",
            root: "/Library/Caches",
            patterns: &["*.cache", "*.tmp", "*.log"],
            age_days: TEMP_AGE_DAYS,
            max_depth: 5,
        },
        SystemFamily {
            id: "system_crash_reports",
            label: "System crash reports",
            root: "/Library/Logs/DiagnosticReports",
            patterns: &["*"],
            age_days: CRASH_AGE_DAYS,
            max_depth: 1,
        },
        SystemFamily {
            id: "system_logs",
            label: "System logs",
            root: "/private/var/log",
            patterns: &["*.log", "*.gz", "*.asl"],
            age_days: LOG_AGE_DAYS,
            max_depth: 3,
        },
        SystemFamily {
            id: "third_party_system_logs",
            label: "Third-party system logs",
            root: "/Library/Logs/Adobe",
            patterns: &["*"],
            age_days: LOG_AGE_DAYS,
            max_depth: 5,
        },
        SystemFamily {
            id: "third_party_system_logs_cc",
            label: "Third-party system logs (CreativeCloud)",
            root: "/Library/Logs/CreativeCloud",
            patterns: &["*"],
            age_days: LOG_AGE_DAYS,
            max_depth: 5,
        },
        SystemFamily {
            id: "adobegc_log",
            label: "Adobe GC log",
            root: "/Library/Logs",
            patterns: &["adobegc.log"],
            age_days: LOG_AGE_DAYS,
            max_depth: 1,
        },
        // Metal GPU 缓存（对标 system.sh 634-709 的 accessible rebuildable GPU caches）。
        SystemFamily {
            id: "metal_gpu_caches",
            label: "Metal GPU caches",
            root: "/private/var/folders",
            // 由 scan_metal_gpu_caches 特殊处理（C/ 容器 + 目录名匹配 + 陈旧）。
            patterns: &["com.apple.gpuarchiver", "com.apple.metal", "com.apple.metalfe"],
            age_days: 1, // MOLE_GPU_CACHE_AGE_DAYS
            max_depth: 8,
        },
        // macOS 安装器应用（对标 clean_deep_system 的 installer 分支，简化身份链）。
        SystemFamily {
            id: "macos_installers",
            label: "macOS installer apps",
            root: "/Applications",
            patterns: &["Install macOS*.app"],
            age_days: 14,
            max_depth: 1,
        },
    ]
}

/// sudo -n 是否可用（对标 optimize_sudo_available 的 GUI 语义）。
pub fn sudo_n_available() -> bool {
    crate::status::run_cmd("sudo", &["-n", "true"], Duration::from_secs(3)).is_ok()
}

fn mtime_secs(path: &Path) -> Option<u64> {
    std::fs::metadata(path)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
}

fn matches_pattern(name: &str, pattern: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    // 对标 find -name 的 shell glob：* 匹配任意序列（含空）。
    fn glob_match(pat: &[u8], name: &[u8]) -> bool {
        match pat.first() {
            None => name.is_empty(),
            Some(b'*') => {
                // 贪婪回溯。
                for i in 0..=name.len() {
                    if glob_match(&pat[1..], &name[i..]) {
                        return true;
                    }
                }
                false
            }
            Some(b) => name.first() == Some(b) && glob_match(&pat[1..], &name[1..]),
        }
    }
    glob_match(pattern.as_bytes(), name.as_bytes())
}

/// 是否 Software Update 所有、永不删除。
fn is_never_delete(path: &str) -> bool {
    path == "/Library/Updates"
        || path.starts_with("/Library/Updates/")
        || path == "/macOS Install Data"
        || path.starts_with("/macOS Install Data/")
}

/// 扫描族内过期候选（文件；depth ≤ max_depth；mtime 早于 age_days）。
/// Metal GPU / macOS 安装器走各自特扫。
pub fn scan_family(family: &SystemFamily) -> Vec<PathBuf> {
    match family.id {
        "metal_gpu_caches" => return scan_metal_gpu_caches(),
        "macos_installers" => return scan_macos_installers(),
        _ => {}
    }
    let root = Path::new(family.root);
    if !root.is_dir() {
        return Vec::new();
    }
    let cutoff = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
        .saturating_sub(family.age_days * 86400);
    let deadline = Instant::now() + FAMILY_DEADLINE;
    let whitelist = Whitelist::load();
    let mut out = Vec::new();
    let mut stack = vec![(root.to_path_buf(), 0usize)];
    while let Some((dir, depth)) = stack.pop() {
        if Instant::now() >= deadline {
            break;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            if Instant::now() >= deadline {
                break;
            }
            let path = entry.path();
            let path_str = path.to_string_lossy().to_string();
            if is_never_delete(&path_str) {
                continue;
            }
            let Ok(ft) = entry.file_type() else {
                continue;
            };
            if ft.is_dir() && depth < family.max_depth {
                stack.push((path, depth + 1));
                continue;
            }
            if !ft.is_file() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            if !family
                .patterns
                .iter()
                .any(|p| matches_pattern(&name, p))
            {
                continue;
            }
            // mtime 年龄门。
            let Some(mt) = mtime_secs(&path) else {
                continue;
            };
            if mt > cutoff {
                continue;
            }
            // 保护/白名单。
            if protect::should_protect_path(&path_str) || whitelist.is_whitelisted(&path_str) {
                continue;
            }
            out.push(path);
        }
    }
    out
}

/// 对标 is_rebuildable_gpu_cache_dir：仅 /var/folders/**/C/**/ 下的
/// com.apple.gpuarchiver|metal|metalfe。
fn is_rebuildable_gpu_cache_dir(path: &str) -> bool {
    let p = path.trim_start_matches("/private");
    // 必须在 /var/folders/ 下且含 /C/ 段。
    if !p.starts_with("/var/folders/") || !p.contains("/C/") {
        return false;
    }
    let name = Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    matches!(
        name.as_str(),
        "com.apple.gpuarchiver" | "com.apple.metal" | "com.apple.metalfe"
    )
}

/// 对标 gpu_cache_dir_is_stale：目录内无 age_days 内修改的文件 → 陈旧。
fn gpu_cache_dir_is_stale(dir: &Path, age_days: u64) -> bool {
    if !dir.is_dir() || dir.is_symlink() {
        return false;
    }
    let cutoff = SystemTime::now()
        .checked_sub(Duration::from_secs(age_days * 86400))
        .unwrap_or(UNIX_EPOCH);
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        if Instant::now() >= deadline {
            return false; // 超时视为非陈旧（fail-closed）
        }
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for entry in entries.flatten() {
            if Instant::now() >= deadline {
                return false;
            }
            let Ok(ft) = entry.file_type() else {
                continue;
            };
            if ft.is_dir() {
                stack.push(entry.path());
            } else if let Ok(meta) = entry.metadata() {
                if let Ok(m) = meta.modified() {
                    if m >= cutoff {
                        return false; // 有近期文件 → 活跃
                    }
                }
            }
        }
    }
    true // 无近期文件 → 陈旧
}

/// 扫描陈旧 Metal GPU 缓存目录（对标 find /private/var/folders maxdepth 8）。
fn scan_metal_gpu_caches() -> Vec<PathBuf> {
    let root = Path::new("/private/var/folders");
    if !root.is_dir() {
        return Vec::new();
    }
    let deadline = Instant::now() + Duration::from_secs(8);
    let whitelist = Whitelist::load();
    let mut out = Vec::new();
    let mut stack = vec![(root.to_path_buf(), 0usize)];
    while let Some((dir, depth)) = stack.pop() {
        if Instant::now() >= deadline {
            break;
        }
        // -prune：depth 3 且非 C 的目录不再下钻（对标 find -depth 3 ! -name C -prune）。
        if depth >= 3 {
            let name = dir
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            if name != "C" {
                continue;
            }
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            if Instant::now() >= deadline {
                break;
            }
            let path = entry.path();
            let path_str = path.to_string_lossy().to_string();
            let Ok(ft) = entry.file_type() else {
                continue;
            };
            if !ft.is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            // 目标名匹配 + 路径必须含 /C/。
            if matches!(
                name.as_str(),
                "com.apple.gpuarchiver" | "com.apple.metal" | "com.apple.metalfe"
            ) && path_str.contains("/C/")
                && is_rebuildable_gpu_cache_dir(&path_str)
            {
                // 端点安全缓存跳过（对标 is_endpoint_security_cache_path）。
                if protect::is_endpoint_security_cache_path(&path_str) {
                    continue;
                }
                if protect::should_protect_path(&path_str) || whitelist.is_whitelisted(&path_str) {
                    continue;
                }
                if gpu_cache_dir_is_stale(&path, 1) {
                    out.push(path);
                }
                continue; // 不再下钻目标目录内部
            }
            if depth < 8 {
                stack.push((path, depth + 1));
            }
        }
    }
    out
}

/// 对标 macos_installer_candidate_still_eligible 的简化：≥14 天、非符号链接、
/// 软件更新未挂起、进程空闲、版本非当前。
fn scan_macos_installers() -> Vec<PathBuf> {
    let apps = Path::new("/Applications");
    if !apps.is_dir() {
        return Vec::new();
    }
    // 当前 macOS 大版本（sw_vers -productVersion → 第一段）。
    let current_major = crate::status::run_cmd("sw_vers", &["-productVersion"], Duration::from_secs(3))
        .ok()
        .map(|v| v.trim().split('.').next().unwrap_or("").to_string())
        .unwrap_or_default();

    // 软件更新挂起检查（fail-closed）。
    if software_update_pending_or_unknown() {
        return Vec::new();
    }

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let whitelist = Whitelist::load();
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(apps) else {
        return Vec::new();
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        // Install macOS*.app
        if !(name.starts_with("Install macOS") && name.ends_with(".app")) {
            continue;
        }
        let path = entry.path();
        let Ok(ft) = entry.file_type() else {
            continue;
        };
        if ft.is_symlink() {
            continue;
        }
        let path_str = path.to_string_lossy().to_string();
        if protect::should_protect_path(&path_str) || whitelist.is_whitelisted(&path_str) {
            continue;
        }
        // ≥14 天。
        let Some(mt) = mtime_secs(&path) else {
            continue;
        };
        if now.saturating_sub(mt) < 14 * 86400 {
            continue;
        }
        // 进程空闲。
        if installer_process_running(&path_str) {
            continue;
        }
        // 版本非当前（DTPlatformVersion 大版本）。
        if !current_major.is_empty() {
            let installer_major = read_plat_version(&path)
                .map(|v| v.split('.').next().unwrap_or("").to_string())
                .unwrap_or_default();
            if installer_major.is_empty() || installer_major == current_major {
                continue;
            }
        }
        out.push(path);
    }
    out
}

/// 对标 software_update_pending_or_unknown：RecommendedUpdates 非显式空数组
/// 或不可读 → 视为挂起（fail-closed）。
fn software_update_pending_or_unknown() -> bool {
    let plist = "/Library/Preferences/com.apple.SoftwareUpdate.plist";
    if !Path::new(plist).is_file() {
        return true; // 无文件视为未知 → 阻止
    }
    // 用 plutil 提取 JSON。
    let Ok(out) = crate::status::run_cmd(
        "plutil",
        &["-extract", "RecommendedUpdates", "json", "-o", "-", plist],
        Duration::from_secs(3),
    ) else {
        return true; // 不可读 → fail-closed
    };
    // 只有显式 [] 才算无挂起。
    out.trim() != "[]"
}

/// installer 进程是否在运行（对标 pgrep -f path）。
fn installer_process_running(path: &str) -> bool {
    crate::status::run_cmd("pgrep", &["-f", path], Duration::from_secs(3)).is_ok()
}

/// 读取 DTPlatformVersion（对标 PlistBuddy）。
fn read_plat_version(app: &Path) -> Option<String> {
    let plist = app.join("Contents/Info.plist");
    let dict = plist::Value::from_file(plist).ok()?.into_dictionary()?;
    dict.get("DTPlatformVersion")
        .and_then(|v| v.as_string())
        .map(|s| s.to_string())
}

/// 族大小合计。
pub fn family_size(paths: &[PathBuf]) -> u64 {
    let deadline = Instant::now() + Duration::from_secs(10);
    paths
        .iter()
        .map(|p| crate::clean::path_size_with_deadline(p, deadline.min(Instant::now() + Duration::from_secs(1))))
        .sum()
}

/// 执行族清理。dry_run 只报告；真实需要 sudo -n。
/// 返回 (是否成功, 详情, 删除数)。
pub fn execute_family(family: &SystemFamily, dry_run: bool) -> (bool, String, usize) {
    if !Path::new(family.root).is_dir() {
        return (true, "目录不存在，跳过".into(), 0);
    }
    if !dry_run && !sudo_n_available() {
        return (
            false,
            "需要管理员权限（sudo -n 不可用）".into(),
            0,
        );
    }

    let candidates = scan_family(family);
    if candidates.is_empty() {
        return (true, "无过期目标".into(), 0);
    }
    let size = family_size(&candidates);

    if dry_run {
        return (
            true,
            format!(
                "将清理 {} 项（{} KB，≥{} 天）",
                candidates.len(),
                size / 1024,
                family.age_days
            ),
            0,
        );
    }

    // 真实删除：sudo -n 逐项 rm（对标 safe_sudo_remove 的特权删除；
    // GUI 无 Trash 暂存基建——见 CHANGES.md 简化差异）。
    let mut removed = 0usize;
    let mut failed = 0usize;
    for path in &candidates {
        let s = path.to_string_lossy().to_string();
        if protect::should_protect_path(&s) || is_never_delete(&s) {
            continue;
        }
        let status = std::process::Command::new("sudo")
            .args(["-n", "/bin/rm", "-rf"])
            .arg(path)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        match status {
            Ok(st) if st.success() => removed += 1,
            _ => failed += 1,
        }
    }

    if failed > 0 && removed == 0 {
        (false, format!("全部 {failed} 项删除失败"), 0)
    } else if failed > 0 {
        (true, format!("已清理 {removed} 项，{failed} 失败"), removed)
    } else {
        (
            true,
            format!("已清理 {removed} 项（约 {} KB）", size / 1024),
            removed,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 永不删除路径。
    #[test]
    fn never_delete_paths() {
        assert!(is_never_delete("/Library/Updates"));
        assert!(is_never_delete("/Library/Updates/apple"));
        assert!(is_never_delete("/macOS Install Data"));
        assert!(!is_never_delete("/Library/Caches"));
        assert!(!is_never_delete("/Library/Caches/foo.cache"));
    }

    /// find -name 风格 glob。
    #[test]
    fn glob_semantics() {
        assert!(matches_pattern("foo.cache", "*.cache"));
        assert!(matches_pattern("a.tmp", "*.tmp"));
        assert!(matches_pattern("adobegc.log", "adobegc.log"));
        assert!(matches_pattern("anything", "*"));
        assert!(!matches_pattern("foo.txt", "*.cache"));
        assert!(!matches_pattern("adobegc.logx", "adobegc.log"));
    }

    /// 族目录存在时扫描不 panic；无 sudo 时真实执行返回失败详情。
    #[test]
    fn families_no_panic() {
        for f in system_families() {
            let _ = scan_family(&f);
            let (ok, detail, n) = execute_family(&f, true);
            assert!(!detail.is_empty());
            let _ = (ok, n);
        }
    }

    /// 真实执行（非 dry-run）：无 sudo 缓存时返回失败 + 详情。
    #[test]
    fn execute_requires_sudo() {
        let f = &system_families()[0];
        if !sudo_n_available() {
            let (ok, detail, _) = execute_family(f, false);
            assert!(!ok);
            assert!(detail.contains("sudo") || detail.contains("管理员"));
        }
    }

    /// Metal GPU 缓存目录匹配。
    #[test]
    fn metal_gpu_dir_matching() {
        assert!(is_rebuildable_gpu_cache_dir(
            "/private/var/folders/ab/cd/ef/C/xyz/com.apple.metal"
        ));
        assert!(is_rebuildable_gpu_cache_dir(
            "/var/folders/ab/cd/EF/C/xyz/com.apple.gpuarchiver"
        ));
        assert!(!is_rebuildable_gpu_cache_dir(
            "/private/var/folders/ab/T/xyz/com.apple.metal"
        )); // 非 C/
        assert!(!is_rebuildable_gpu_cache_dir("/Library/Caches/com.apple.metal"));
        assert!(!is_rebuildable_gpu_cache_dir(
            "/private/var/folders/ab/C/xyz/com.apple.other"
        ));
    }

    /// 族含 Metal 与安装器。
    #[test]
    fn families_include_metal_and_installers() {
        let ids: Vec<&str> = system_families().iter().map(|f| f.id).collect();
        assert!(ids.contains(&"metal_gpu_caches"));
        assert!(ids.contains(&"macos_installers"));
    }
}
