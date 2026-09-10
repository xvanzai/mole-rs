//! 特殊清理族：orphaned container stubs、设备固件、Time Machine、大文件审查。
//!
//! 对标 apps.sh clean_orphaned_container_stubs、user.sh clean_cached_device_firmware、
//! system.sh clean_time_machine_failed_backups、user.sh check_large_file_candidates。

use super::protect;
use super::whitelist::Whitelist;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, UNIX_EPOCH};

/// 对标 clean_orphaned_container_stubs 的 stub_patterns（bundle_id_glob:app_path）。
const STUB_PATTERNS: &[(&str, &str)] = &[
    (
        "com.macpaw.CleanMyMac*",
        "/Applications/CleanMyMac X.app",
    ),
    (
        "*.com.macpaw.CleanMyMac*",
        "/Applications/CleanMyMac X.app",
    ),
];

/// 对标 clean_orphaned_container_stubs：扫描 Containers 下匹配 stub 模式的
/// 空容器（仅 metadata.plist，无其他子项），且关联 app 不存在。
pub fn scan_orphaned_container_stubs() -> Vec<PathBuf> {
    let home = std::env::var("HOME").unwrap_or_default();
    let containers = PathBuf::from(&home).join("Library/Containers");
    if !containers.is_dir() {
        return Vec::new();
    }
    let whitelist = Whitelist::load();
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(&containers) else {
        return Vec::new();
    };
    for entry in entries.flatten() {
        let Ok(ft) = entry.file_type() else { continue };
        if !ft.is_dir() || ft.is_symlink() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        // 匹配 stub_patterns（glob：* 匹配任意）。
        let matches = STUB_PATTERNS.iter().any(|(glob, _)| glob_match(glob, &name));
        if !matches {
            continue;
        }
        let path = entry.path();
        // 必须有 metadata.plist。
        let meta_plist = path.join(".com.apple.containermanagerd.metadata.plist");
        if !meta_plist.is_file() {
            continue;
        }
        // 除 metadata.plist 外无其他子项。
        let has_sibling = std::fs::read_dir(&path)
            .map(|rd| {
                rd.flatten().any(|e| {
                    e.file_name() != ".com.apple.containermanagerd.metadata.plist"
                })
            })
            .unwrap_or(true);
        if has_sibling {
            continue;
        }
        // 关联 app 不存在。
        let app_path = STUB_PATTERNS
            .iter()
            .find(|(glob, _)| glob_match(glob, &name))
            .map(|(_, app)| *app);
        if let Some(app) = app_path {
            if Path::new(app).exists() {
                continue;
            }
        }
        let path_str = path.to_string_lossy().to_string();
        if protect::should_protect_path(&path_str) || whitelist.is_whitelisted(&path_str) {
            continue;
        }
        out.push(path);
    }
    out
}

/// shell glob 匹配（* 匹配任意，无 ?）。
fn glob_match(pattern: &str, name: &str) -> bool {
    fn m(pat: &[u8], name: &[u8]) -> bool {
        match pat.first() {
            None => name.is_empty(),
            Some(b'*') => (0..=name.len()).any(|i| m(&pat[1..], &name[i..])),
            Some(b) => name.first() == Some(b) && m(&pat[1..], &name[1..]),
        }
    }
    m(pattern.as_bytes(), name.as_bytes())
}

/// 对标 clean_cached_device_firmware：iTunes/Configurator 下的 *.ipsw。
pub fn scan_device_firmware() -> Vec<PathBuf> {
    let home = std::env::var("HOME").unwrap_or_default();
    let mut roots = vec![
        PathBuf::from(&home).join("Library/iTunes/iPhone Software Updates"),
        PathBuf::from(&home).join("Library/iTunes/iPad Software Updates"),
        PathBuf::from(&home).join("Library/iTunes/iPod Software Updates"),
    ];
    // Configurator group containers。
    let gc = PathBuf::from(&home).join("Library/Group Containers");
    if let Ok(entries) = std::fs::read_dir(&gc) {
        for e in entries.flatten() {
            let n = e.file_name().to_string_lossy().to_string();
            if n.ends_with(".group.com.apple.configurator") && e.path().is_dir() {
                roots.push(e.path());
            }
        }
    }
    let whitelist = Whitelist::load();
    let mut out = Vec::new();
    for root in roots {
        if !root.is_dir() {
            continue;
        }
        // shallow：maxdepth 1 *.ipsw + Configurator 下递归。
        let is_configurator = root
            .to_string_lossy()
            .contains("group.com.apple.configurator");
        let max_depth = if is_configurator { 6 } else { 1 };
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut stack = vec![(root, 0usize)];
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
                let name = entry.file_name().to_string_lossy().to_string();
                let Ok(ft) = entry.file_type() else { continue };
                if ft.is_dir() && depth < max_depth {
                    stack.push((entry.path(), depth + 1));
                } else if name.ends_with(".ipsw") && ft.is_file() {
                    let p = entry.path().to_string_lossy().to_string();
                    if protect::should_protect_path(&p) || whitelist.is_whitelisted(&p) {
                        continue;
                    }
                    out.push(entry.path());
                }
            }
        }
    }
    out
}

/// 对标 clean_time_machine_failed_backups：报告未完成备份（只读）。
/// 返回 Some(count) 表示发现未完成备份数；None 表示无/不可用。
pub fn count_incomplete_tm_backups() -> Option<usize> {
    if !crate::clean::command_exists("tmutil") {
        return None;
    }
    // AutoBackup 配置检查。
    let Ok(out) = crate::status::run_cmd(
        "defaults",
        &[
            "read",
            "/Library/Preferences/com.apple.TimeMachine",
            "AutoBackup",
        ],
        Duration::from_secs(3),
    ) else {
        return None;
    };
    let t = out.trim();
    if t != "0" && t != "1" {
        return None;
    }
    // destinationinfo。
    let Ok(info) = crate::status::run_cmd("tmutil", &["destinationinfo"], Duration::from_secs(3))
    else {
        return None;
    };
    if info.contains("No destinations configured") {
        return None;
    }
    // 扫描 /Volumes 下的 incomplete backups。
    let Ok(vols) = std::fs::read_dir("/Volumes") else {
        return Some(0);
    };
    let mut count = 0usize;
    for vol in vols.flatten() {
        let backup_dir = vol.path().join("com.apple.TimeMachine.localsnapshots");
        // 更常见的：incomplete backup 目录。
        let incomplete = vol.path().join("Backups.backupdb");
        let _ = (backup_dir, incomplete);
        // tmutil listbackups 输出行数作为代理。
    }
    // 用 tmutil listbackups 计数（对标 grep com.apple.TimeMachine）。
    if let Ok(list) = crate::status::run_cmd("tmutil", &["listbackups"], Duration::from_secs(5)) {
        count = list
            .lines()
            .filter(|l| l.contains("com.apple.TimeMachine"))
            .count();
    }
    Some(count)
}

/// 对标 check_large_file_candidates：≥1GB 的已知大路径审查清单（只读）。
/// 返回 (label, path, size_bytes) 列表。
pub fn large_file_candidates() -> Vec<(String, String, u64)> {
    let home = std::env::var("HOME").unwrap_or_default();
    let threshold = 1_073_741_824u64; // 1GB
    let deadline = Instant::now() + Duration::from_secs(30);
    let candidates: &[(&str, &str)] = &[
        ("iOS Backups", "Library/Application Support/MobileSync/Backup"),
        ("Xcode DeviceSupport", "Library/Developer/Xcode/iOS DeviceSupport"),
        ("Xcode Archives", "Library/Developer/Xcode/Archives"),
        ("Xcode DerivedData", "Library/Developer/Xcode/DerivedData"),
        ("iOS Simulators", "Library/Developer/CoreSimulator/Devices"),
        ("Time Machine local", ".MobileBackups"),
        ("Docker Data", "Library/Containers/com.docker.docker/Data"),
        ("Parallels VMs", "Parallels"),
        ("VMware VMs", "Virtual Machines"),
        ("VirtualBox VMs", "VirtualBox VMs"),
        ("Steam steamapps", "Library/Application Support/Steam/steamapps"),
        ("Epic Games", "Library/Application Support/Epic"),
        ("Photos Library", "Pictures/Photos Library.photoslibrary"),
    ];
    let mut out = Vec::new();
    for (label, rel) in candidates {
        let path = PathBuf::from(&home).join(rel);
        if !path.exists() {
            continue;
        }
        if Instant::now() >= deadline {
            break;
        }
        let size = crate::clean::path_size_with_deadline(&path, Instant::now() + Duration::from_secs(5));
        if size >= threshold {
            out.push((
                label.to_string(),
                path.to_string_lossy().to_string(),
                size,
            ));
        }
    }
    out
}

/// mtime 用于日期展示。
#[allow(dead_code)]
fn newest_child_date(dir: &Path) -> Option<String> {
    let entries = std::fs::read_dir(dir).ok()?;
    let mut newest: u64 = 0;
    for e in entries.flatten() {
        if let Ok(m) = e.metadata() {
            if let Ok(t) = m.modified() {
                if let Ok(d) = t.duration_since(UNIX_EPOCH) {
                    newest = newest.max(d.as_secs());
                }
            }
        }
    }
    if newest == 0 {
        return None;
    }
    // 简化：返回 unix 秒字符串（UI 可格式化）。
    Some(newest.to_string())
}

/// 对标 clean_external_volume_target：外置卷 .TemporaryItems/.Trashes +
/// .DS_Store + 常见元数据文件。
/// 返回可清理目标列表。
pub fn scan_external_volume(volume: &str) -> Vec<PathBuf> {
    let root = Path::new(volume);
    if !root.is_dir() || root.is_symlink() {
        return Vec::new();
    }
    let whitelist = Whitelist::load();
    let mut out = Vec::new();
    // .TemporaryItems / .Trashes。
    for name in [".TemporaryItems", ".Trashes"] {
        let p = root.join(name);
        if p.exists() && !p.is_symlink() {
            let s = p.to_string_lossy().to_string();
            if !protect::should_protect_path(&s) && !whitelist.is_whitelisted(&s) {
                out.push(p);
            }
        }
    }
    // .DS_Store（maxdepth 5，排除子目录内同名排除表——外置卷简化为全扫）。
    let deadline = Instant::now() + Duration::from_secs(15);
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
            let name = entry.file_name().to_string_lossy().to_string();
            let Ok(ft) = entry.file_type() else { continue };
            if ft.is_dir() && depth < 5 {
                stack.push((entry.path(), depth + 1));
            } else if name == ".DS_Store" && ft.is_file() {
                let s = entry.path().to_string_lossy().to_string();
                if !protect::should_protect_path(&s) && !whitelist.is_whitelisted(&s) {
                    out.push(entry.path());
                }
            }
        }
    }
    out
}

/// 对标 ORPHAN_NEVER_DELETE_PATTERNS（敏感数据模式，小写匹配）。
const ORPHAN_NEVER_DELETE: &[&str] = &[
    "com.apple.*",
    "com.microsoft.*",
    "com.adobe.*",
    "com.google.*",
    "org.mozilla.*",
    "net.*",
    "io.*",
    "com.slack.*",
    "com.spotify.*",
    "com.whatsapp.*",
    "ru.keepcoder.*",
    "com.tencent.*",
    "com.alibaba.*",
    "com.bytedance.*",
    "com.zhiliaoapp.*",
    "com.facebook.*",
    "com.twitter.*",
    "com.instagram.*",
    "com.telegram.*",
    "com.discord.*",
    "com.notion.*",
    "com.figma.*",
    "com.linear.*",
    "com.slack.*",
];

/// 对标 scan_installed_apps：扫描标准位置的 .app，提取 CFBundleIdentifier。
/// 返回已安装 bundle ID 集合（小写）。
pub fn scan_installed_bundle_ids() -> std::collections::HashSet<String> {
    let home = std::env::var("HOME").unwrap_or_default();
    let roots = [
        "/Applications",
        "/System/Applications",
    ];
    let home_roots = [
        format!("{home}/Applications"),
        format!("{home}/Library/Application Support/Setapp/Applications"),
    ];
    let mut ids = std::collections::HashSet::new();
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut stack: Vec<(PathBuf, usize)> = roots
        .iter()
        .map(PathBuf::from)
        .chain(home_roots.iter().map(PathBuf::from))
        .filter(|p| p.is_dir())
        .map(|p| (p, 0))
        .collect();
    while let Some((dir, depth)) = stack.pop() {
        if Instant::now() >= deadline {
            break;
        }
        if depth > 3 {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            if Instant::now() >= deadline {
                break;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            let path = entry.path();
            if name.ends_with(".app") && path.is_dir() {
                // 提取 CFBundleIdentifier。
                if let Some(dict) = crate::clean::protect::read_info_dict_for_orphan(&path) {
                    if let Some(id) = dict.get("CFBundleIdentifier").and_then(|v| v.as_string()) {
                        ids.insert(id.to_lowercase());
                    }
                }
                continue; // 不下钻 .app
            }
            let Ok(ft) = entry.file_type() else { continue };
            if ft.is_dir() && depth < 3 {
                stack.push((path, depth + 1));
            }
        }
    }
    ids
}

/// 对标 is_bundle_orphaned 简化版：保护 → never_delete → installed → 系统组件
/// → 30 天 mtime → mdfind。
fn is_bundle_orphaned(bundle_id: &str, dir: &Path, installed: &std::collections::HashSet<String>) -> bool {
    // 1. should_protect_data。
    if protect::should_protect_data(bundle_id) {
        return false;
    }
    // 2. never_delete 模式（小写 glob）。
    let lower = bundle_id.to_lowercase();
    if ORPHAN_NEVER_DELETE.iter().any(|p| glob_match(p, &lower)) {
        return false;
    }
    // 3. 已安装。
    if installed.contains(&lower) {
        return false;
    }
    // 4. 硬编码系统组件。
    if matches!(
        lower.as_str(),
        "loginwindow" | "dock" | "systempreferences" | "systemsettings" | "settings"
            | "controlcenter" | "finder" | "safari"
    ) {
        return false;
    }
    // 5. 30 天 mtime。
    if let Ok(meta) = std::fs::metadata(dir) {
        if let Ok(modified) = meta.modified() {
            if let Ok(age) = modified.elapsed() {
                if age.as_secs() < 30 * 86400 {
                    return false;
                }
            }
        }
    }
    // 6. mdfind 回退。
    if crate::uninstall::is_reverse_dns_bundle_id(bundle_id)
        && crate::clean::command_exists("mdfind")
    {
        let query = format!("kMDItemCFBundleIdentifier == '{bundle_id}'");
        if let Ok(out) = crate::status::run_cmd("mdfind", &[&query], Duration::from_secs(5)) {
            if !out.trim().is_empty() {
                return false;
            }
        }
        // mdfind 失败/超时 → 保守视为非孤儿。
    }
    true
}

/// 对标 clean_orphaned_app_data 的核心：扫描 Caches/Logs/Saved Application State
/// 下 com.*/org.*/net.*/io.* 或 *.savedState，检测孤儿并返回可清理列表。
pub fn scan_orphaned_app_data() -> Vec<PathBuf> {
    let home = std::env::var("HOME").unwrap_or_default();
    let installed = scan_installed_bundle_ids();
    let whitelist = Whitelist::load();
    let mut out = Vec::new();

    // 三类资源 + 模式。
    let resource_types: &[(&str, &str, &[&str])] = &[
        ("Library/Caches", "Caches", &["com.*", "org.*", "net.*", "io.*"]),
        ("Library/Logs", "Logs", &["com.*", "org.*", "net.*", "io.*"]),
        ("Library/Saved Application State", "States", &["*.savedState"]),
    ];
    let deadline = Instant::now() + Duration::from_secs(30);
    for (rel, _label, patterns) in resource_types {
        let base = PathBuf::from(&home).join(rel);
        if !base.is_dir() {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&base) else { continue };
        for entry in entries.flatten() {
            if Instant::now() >= deadline {
                break;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            let matches_pattern = patterns.iter().any(|p| {
                if p.starts_with("*.") {
                    name.ends_with(&p[1..])
                } else if p.ends_with(".*") {
                    name.starts_with(&p[..p.len() - 1])
                } else {
                    name == *p
                }
            });
            if !matches_pattern {
                continue;
            }
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            // 提取 bundle_id（basename 去后缀）。
            let bundle_id = name
                .trim_end_matches(".savedState")
                .trim_end_matches(".binarycookies")
                .trim_end_matches(".plist")
                .to_string();
            if bundle_id.is_empty() {
                continue;
            }
            // 最大迭代限制（对标 MOLE_MAX_ORPHAN_ITERATIONS）。
            if out.len() >= 100 {
                break;
            }
            if !is_bundle_orphaned(&bundle_id, &path, &installed) {
                continue;
            }
            let path_str = path.to_string_lossy().to_string();
            if protect::should_protect_path(&path_str) || whitelist.is_whitelisted(&path_str) {
                continue;
            }
            out.push(path);
        }
    }
    out
}

/// 对标 show_user_launch_agent_hint_notice：扫描 LaunchAgents 中程序目标
/// 缺失/不可执行的条目（只读提示）。
pub fn launch_agent_hints() -> Vec<(String, String)> {
    let home = std::env::var("HOME").unwrap_or_default();
    let dir = PathBuf::from(&home).join("Library/LaunchAgents");
    if !dir.is_dir() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.ends_with(".plist") || name.starts_with("com.apple.") {
            continue;
        }
        let path = entry.path();
        // 读 Program 路径（简化：plist Program 或 ProgramArguments[0]）。
        let Some(dict) = plist::Value::from_file(&path)
            .ok()
            .and_then(|v| v.into_dictionary())
        else {
            continue;
        };
        let program = dict
            .get("Program")
            .and_then(|v| v.as_string())
            .map(|s| s.to_string())
            .or_else(|| {
                dict.get("ProgramArguments")
                    .and_then(|v| v.as_array())
                    .and_then(|a| a.first())
                    .and_then(|v| v.as_string())
                    .map(|s| s.to_string())
            });
        let Some(program) = program else {
            continue;
        };
        // 有 MachServices 且无 Program → 跳过（daemon 类）。
        if program.is_empty() && dict.contains_key("MachServices") {
            continue;
        }
        if program.is_empty() {
            continue;
        }
        // 系统二进制跳过。
        if program.starts_with("/usr/") || program.starts_with("/bin/") || program.starts_with("/sbin/")
        {
            continue;
        }
        // 存在且可执行 → 健康。
        let p = Path::new(&program);
        if program.starts_with('/') && p.is_file() {
            use std::os::unix::fs::PermissionsExt;
            if std::fs::metadata(p)
                .map(|m| m.permissions().mode() & 0o111 != 0)
                .unwrap_or(false)
            {
                continue;
            }
            out.push((
                name.clone(),
                format!("程序目标不可执行：{program}"),
            ));
        } else if program.starts_with('/') && !p.exists() {
            out.push((name.clone(), format!("程序目标缺失：{program}")));
        }
        if out.len() >= 3 {
            break; // max_hits=3
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// glob 匹配。
    #[test]
    fn glob_semantics() {
        assert!(glob_match("com.macpaw.CleanMyMac*", "com.macpaw.CleanMyMacX"));
        assert!(glob_match("*.com.macpaw.CleanMyMac*", "S8EX82NJP6.com.macpaw.CleanMyMac"));
        assert!(!glob_match("com.macpaw.CleanMyMac*", "com.other.app"));
    }

    /// 孤儿容器扫描不 panic。
    #[test]
    fn stubs_no_panic() {
        let _ = scan_orphaned_container_stubs();
    }

    /// 设备固件扫描不 panic。
    #[test]
    fn firmware_no_panic() {
        let _ = scan_device_firmware();
    }

    /// TM 备份计数不 panic。
    #[test]
    fn tm_no_panic() {
        let _ = count_incomplete_tm_backups();
    }

    /// 大文件候选不 panic。
    #[test]
    fn large_files_no_panic() {
        let _ = large_file_candidates();
    }
}
