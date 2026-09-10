//! Chromium 系浏览器旧版本清理，对标 `lib/clean/user.sh` 的
//! `_clean_chromium_old_versions` 与 `clean_edge_updater_old_versions`。
//!
//! 语义（1:1）：
//! - Versions 目录下保留 `Current` 符号链接目标；
//! - 若存在 mtime 更新于 Current 的目录（staged auto-update），一并保留；
//! - 其余版本目录在进程守卫放行后走 Trash；
//! - EdgeUpdater 变体：有已安装 Edge 版本时保留 ≥ 安装版的 payload，
//!   否则按 sort -V 仅保留最新（#1216）。

use super::process::{self, ProcessState};
use super::protect;
use super::whitelist::Whitelist;
use super::ScanEntry;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

/// 浏览器族配置：(label, framework 名, 探针, 默认 app 路径)。
fn chromium_browsers() -> Vec<(&'static str, &'static str, fn() -> ProcessState, Vec<PathBuf>)> {
    let home = std::env::var("HOME").unwrap_or_default();
    let mk = |name: &str| -> Vec<PathBuf> {
        vec![
            PathBuf::from(format!("/Applications/{name}.app")),
            PathBuf::from(&home).join(format!("Applications/{name}.app")),
        ]
    };
    vec![
        (
            "Chrome",
            "Google Chrome Framework.framework",
            process::google_chrome_process_state,
            mk("Google Chrome"),
        ),
        (
            "Edge",
            "Microsoft Edge Framework.framework",
            process::microsoft_edge_process_state,
            mk("Microsoft Edge"),
        ),
        (
            "Brave",
            "Brave Browser Framework.framework",
            process::brave_process_state,
            mk("Brave Browser"),
        ),
    ]
}

/// 目录 mtime（秒）；失败返回 0。
fn dir_mtime_secs(path: &Path) -> u64 {
    std::fs::metadata(path)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// 对标 `_clean_chromium_old_versions` 的候选收集：返回旧版本目录列表。
fn collect_old_versions(app_path: &Path, framework: &str) -> Option<Vec<PathBuf>> {
    let versions_dir = app_path
        .join("Contents/Frameworks")
        .join(framework)
        .join("Versions");
    if !versions_dir.is_dir() {
        return None;
    }
    let current_link = versions_dir.join("Current");
    // Current 必须是符号链接。
    let meta = std::fs::symlink_metadata(&current_link).ok()?;
    if !meta.file_type().is_symlink() {
        return None;
    }
    let current_version = std::fs::read_link(&current_link)
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))?;
    if current_version.is_empty() {
        return None;
    }
    // Current 目标必须存在（损坏则跳过，避免误删活跃版本）。
    if !versions_dir.join(&current_version).is_dir() {
        return None;
    }

    let current_mtime = dir_mtime_secs(&versions_dir.join(&current_version));

    // 找 mtime 更新于 Current 的目录（staged auto-update）。
    let mut newest_version: Option<String> = None;
    let mut newest_mtime = current_mtime;
    let Ok(entries) = std::fs::read_dir(&versions_dir) else {
        return None;
    };
    for entry in entries.flatten() {
        let Ok(ft) = entry.file_type() else { continue };
        if !ft.is_dir() || ft.is_symlink() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if name == "Current" {
            continue;
        }
        let mtime = dir_mtime_secs(&entry.path());
        if mtime > newest_mtime {
            newest_mtime = mtime;
            newest_version = Some(name);
        }
    }

    // 收集旧版本（排除 Current、当前版本、staged 最新）。
    let mut old = Vec::new();
    let Ok(entries) = std::fs::read_dir(&versions_dir) else {
        return None;
    };
    for entry in entries.flatten() {
        let Ok(ft) = entry.file_type() else { continue };
        if !ft.is_dir() || ft.is_symlink() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if name == "Current" || name == current_version {
            continue;
        }
        if Some(name.as_str()) == newest_version.as_deref() {
            continue;
        }
        old.push(entry.path());
    }
    if old.is_empty() {
        return None;
    }
    Some(old)
}

/// 收集三家浏览器的旧版本目录为 ScanEntry（带进程守卫）。
pub fn chromium_old_version_entries() -> Vec<ScanEntry> {
    let mut rows = Vec::new();
    let whitelist = Whitelist::load();
    for (label, framework, probe, app_paths) in chromium_browsers() {
        for app in &app_paths {
            if !app.is_dir() {
                continue;
            }
            let Some(old_versions) = collect_old_versions(app, framework) else {
                continue;
            };
            for dir in old_versions {
                let dir_str = dir.to_string_lossy().to_string();
                if protect::should_protect_path(&dir_str)
                    || whitelist.is_whitelisted(&dir_str)
                    || protect::holds_compiled_model_cache(&dir_str)
                {
                    continue;
                }
                let name = dir
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
                rows.push(ScanEntry {
                    family: "browser_old_versions",
                    pattern: dir,
                    description: format!("{label} old version {name}"),
                    process_probe: Some(probe),
                    sw_domain_guard: false,
                    age_days: 0,
                });
            }
        }
    }
    rows
}

/// 对标 `clean_edge_updater_old_versions`：EdgeUpdater staged payload。
///
/// 有已安装 Edge 版本时保留 ≥ 安装版（pending update）；否则 sort -V 仅留最新。
pub fn edge_updater_old_version_entries() -> Vec<ScanEntry> {
    let home = std::env::var("HOME").unwrap_or_default();
    let updater_dir = PathBuf::from(&home).join(
        "Library/Application Support/Microsoft/EdgeUpdater/apps/msedge-stable",
    );
    if !updater_dir.is_dir() {
        return Vec::new();
    }

    let mut version_dirs = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&updater_dir) {
        for entry in entries.flatten() {
            let Ok(ft) = entry.file_type() else { continue };
            if ft.is_dir() && !ft.is_symlink() {
                version_dirs.push(entry.path());
            }
        }
    }
    if version_dirs.is_empty() {
        return Vec::new();
    }

    // 已安装 Edge 版本。
    let installed = edge_installed_version();
    let mut latest_version: Option<String> = None;
    if installed.is_none() {
        if version_dirs.len() < 2 {
            return Vec::new();
        }
        // sort -V 取最新。
        let mut names: Vec<String> = version_dirs
            .iter()
            .filter_map(|d| d.file_name().map(|n| n.to_string_lossy().to_string()))
            .collect();
        names.sort_by(|a, b| version_cmp(a, b));
        latest_version = names.pop();
        if latest_version.is_none() {
            return Vec::new();
        }
    }

    let whitelist = Whitelist::load();
    let mut rows = Vec::new();
    for dir in &version_dirs {
        let name = dir
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        if let Some(installed) = &installed {
            // 保留 >= 安装版（pending update）。
            if &name == installed || version_cmp(&name, installed) != std::cmp::Ordering::Less {
                continue;
            }
        } else if Some(&name) == latest_version.as_ref() {
            continue;
        }
        let dir_str = dir.to_string_lossy().to_string();
        if protect::should_protect_path(&dir_str)
            || whitelist.is_whitelisted(&dir_str)
            || protect::holds_compiled_model_cache(&dir_str)
        {
            continue;
        }
        rows.push(ScanEntry {
            family: "browser_old_versions",
            pattern: dir.clone(),
            description: format!("Edge updater old version {name}"),
            process_probe: Some(process::microsoft_edge_process_state),
            sw_domain_guard: false,
            age_days: 0,
        });
    }
    rows
}

/// 已安装 Edge 的 CFBundleShortVersionString。
fn edge_installed_version() -> Option<String> {
    let home = std::env::var("HOME").unwrap_or_default();
    for app in [
        PathBuf::from("/Applications/Microsoft Edge.app"),
        PathBuf::from(&home).join("Applications/Microsoft Edge.app"),
    ] {
        let plist = app.join("Contents/Info.plist");
        if !plist.is_file() {
            continue;
        }
        if let Some(dict) = plist::Value::from_file(&plist)
            .ok()
            .and_then(|v| v.into_dictionary())
        {
            if let Some(v) = dict
                .get("CFBundleShortVersionString")
                .and_then(|v| v.as_string())
            {
                if !v.is_empty() {
                    return Some(v.to_string());
                }
            }
        }
    }
    None
}

/// 对标 `sort -V`：数字感知版本比较（逐段数字/字母）。
/// 注意：不可用 take_while——它会吞掉第一个不匹配字符，破坏 Peekable 状态。
fn version_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    let mut ai = a.chars().peekable();
    let mut bi = b.chars().peekable();
    loop {
        match (ai.peek().copied(), bi.peek().copied()) {
            (None, None) => return std::cmp::Ordering::Equal,
            (None, Some(_)) => return std::cmp::Ordering::Less,
            (Some(_), None) => return std::cmp::Ordering::Greater,
            (Some(ca), Some(cb)) => {
                if ca.is_ascii_digit() && cb.is_ascii_digit() {
                    // 手动消费数字段（peek 后 next，不吞后续分隔符）。
                    let mut na = String::new();
                    while let Some(c) = ai.peek().copied() {
                        if c.is_ascii_digit() {
                            na.push(c);
                            ai.next();
                        } else {
                            break;
                        }
                    }
                    let mut nb = String::new();
                    while let Some(c) = bi.peek().copied() {
                        if c.is_ascii_digit() {
                            nb.push(c);
                            bi.next();
                        } else {
                            break;
                        }
                    }
                    // 去前导零后按数值比较。
                    let va = na.trim_start_matches('0');
                    let vb = nb.trim_start_matches('0');
                    match va.len().cmp(&vb.len()).then_with(|| va.cmp(vb)) {
                        std::cmp::Ordering::Equal => {}
                        other => return other,
                    }
                } else if ca.is_ascii_digit() {
                    // 数字 < 字母（sort -V 常见语义）。
                    return std::cmp::Ordering::Less;
                } else if cb.is_ascii_digit() {
                    return std::cmp::Ordering::Greater;
                } else {
                    match ca.cmp(&cb) {
                        std::cmp::Ordering::Equal => {
                            ai.next();
                            bi.next();
                        }
                        other => return other,
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// sort -V 语义：数字逐段比较。
    #[test]
    fn version_ordering() {
        assert_eq!(version_cmp("1.0", "1.0"), std::cmp::Ordering::Equal);
        assert_eq!(version_cmp("1.0", "1.1"), std::cmp::Ordering::Less);
        assert_eq!(version_cmp("1.10", "1.9"), std::cmp::Ordering::Greater);
        assert_eq!(version_cmp("2.0", "10.0"), std::cmp::Ordering::Less);
        assert_eq!(version_cmp("1.0a", "1.0"), std::cmp::Ordering::Greater);
        assert_eq!(
            version_cmp("126.0.6478.127", "126.0.6478.57"),
            std::cmp::Ordering::Greater
        );
    }

    /// 本机无 Chrome Versions 时返回空（不 panic）。
    #[test]
    fn chromium_entries_no_panic() {
        let _ = chromium_old_version_entries();
        let _ = edge_updater_old_version_entries();
    }

    /// fixture：构造 Versions 目录验证候选收集。
    #[test]
    fn collect_old_versions_fixture() {
        let tmp = std::env::temp_dir().join(format!("mole_rs_ov_{}", std::process::id()));
        let versions = tmp.join("Contents/Frameworks/Test.framework/Versions");
        std::fs::create_dir_all(versions.join("100.0")).unwrap();
        std::fs::create_dir_all(versions.join("101.0")).unwrap();
        std::fs::create_dir_all(versions.join("102.0")).unwrap();
        // Current → 101.0
        std::os::unix::fs::symlink(versions.join("101.0"), versions.join("Current")).unwrap();

        let old = collect_old_versions(&tmp, "Test.framework").unwrap();
        let names: Vec<String> = old
            .iter()
            .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
            .collect();
        // 101 是 Current；102 若 mtime 不更新则也是旧版本（fixture 同时创建，
        // mtime 相同或 102 更新——按实现：仅 mtime 严格更新才保留 staged）。
        assert!(names.contains(&"100.0".to_string()), "100 应为旧版本: {names:?}");
        // 损坏 Current：跳过。
        let broken = tmp.join("Broken");
        std::fs::create_dir_all(broken.join("Versions/1.0")).unwrap();
        assert!(collect_old_versions(&broken, "Test.framework").is_none());

        std::fs::remove_dir_all(&tmp).ok();
    }
}
