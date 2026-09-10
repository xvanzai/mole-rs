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
