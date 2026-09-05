//! uninstall 模块：应用卸载，对标 `bin/uninstall.sh` + `lib/uninstall/*`。
//!
//! 第一片（本模块）：应用清单（只读）+ 卸载模式保护分级。
//! 对标要点：
//! - 搜索目录：/Applications、~/Applications、/Library/Input Methods、
//!   ~/Library/Input Methods、/Volumes/*/Applications（与 /Applications
//!   同一目录时去重）；`find -maxdepth 3 -iname "*.app"`；
//! - bundle ID：Contents/Info.plist 的 CFBundleIdentifier；iOS 应用回退
//!   Wrapper/*.app/Info.plist（对标 batch.sh 的 Wrapper 分支）；
//! - `|`→`-`、控制字符清洗（对标 uninstall_resolve_bundle_id）；
//! - 后台专用应用（LSBackgroundOnly）仅在位于搜索根直接层时列出；
//! - 保护分级（对标 should_protect_from_uninstall）：先判
//!   APPLE_UNINSTALLABLE_APPS（可卸载放行），再判
//!   SYSTEM_CRITICAL_BUNDLES（系统关键保护）。
//!
//! 第二片（暂缓）：应用本体删除 + `find_app_files` 残留查找（共享
//! bundle ID 兄弟守卫、LaunchAgents、Containers、Cask zap 等）——该
//! 删除汇按 AGENTS.md 要求逐行复核后移植。

use serde::Serialize;
use std::path::{Path, PathBuf};

/// 应用清单条目。
#[derive(Debug, Clone, Serialize)]
pub struct AppInfo {
    pub path: String,
    pub name: String,
    pub bundle_id: String,
    pub version: String,
    pub size_bytes: u64,
    /// 卸载模式保护分级结果（系统关键组件不可卸载）。
    pub protected: bool,
    /// Apple 可卸载应用（App Store 类）。
    pub uninstallable: bool,
    /// 后台专用应用（LSBackgroundOnly）。
    pub background_only: bool,
    /// 位于搜索根直接层（非嵌套 .app）。
    pub in_search_root: bool,
}

/// 对标 uninstall_print_app_search_dirs。
fn search_dirs() -> Vec<PathBuf> {
    let home = std::env::var("HOME").unwrap_or_default();
    let mut dirs = vec![
        PathBuf::from("/Applications"),
        PathBuf::from(&home).join("Applications"),
        PathBuf::from("/Library/Input Methods"),
        PathBuf::from(&home).join("Library/Input Methods"),
    ];
    // /Volumes/*/Applications（跳过与 /Applications 同一目录）。
    if let Ok(entries) = std::fs::read_dir("/Volumes") {
        for entry in entries.flatten() {
            let vol_apps = entry.path().join("Applications");
            if vol_apps.is_dir() && vol_apps.is_file() == false {
                let same_as_main = std::fs::canonicalize(&vol_apps)
                    .map(|c| c == Path::new("/Applications"))
                    .unwrap_or(false);
                if !same_as_main {
                    dirs.push(vol_apps);
                }
            }
        }
    }
    dirs
}

/// 清洗 bundle ID（对标：`|`→`-`、剔除控制字符；"(null)" 视为空）。
fn sanitize_bundle_id(raw: &str) -> String {
    let cleaned: String = raw
        .replace('|', "-")
        .chars()
        .filter(|c| !c.is_control())
        .collect();
    let cleaned = cleaned.trim().to_string();
    if cleaned == "(null)" {
        String::new()
    } else {
        cleaned
    }
}

/// 从 Info.plist 读取字段。
fn read_info_plist(plist_path: &Path) -> Option<plist::Dictionary> {
    plist::Value::from_file(plist_path)
        .ok()?
        .into_dictionary()
}

fn plist_string(dict: &plist::Dictionary, key: &str) -> Option<String> {
    dict.get(key).and_then(|v| v.as_string()).map(|s| s.to_string())
}

/// 后台专用判定（对标 uninstall_app_is_background_only 的取值集合）。
fn ls_background_only(dict: Option<&plist::Dictionary>) -> bool {
    matches!(
        dict.and_then(|d| plist_string(d, "LSBackgroundOnly")).as_deref(),
        Some("1" | "YES" | "yes" | "TRUE" | "true")
    )
}

/// 从 Info.plist（单次读取）提取清单元数据；bundle ID 为空时回退
/// Wrapper/*.app/Info.plist（iOS 应用，对标 batch.sh）。
struct AppMeta {
    bundle_id: String,
    background_only: bool,
    version: String,
    name: String,
}

fn read_app_meta(app: &Path) -> AppMeta {
    let dict = read_info_plist(&app.join("Contents/Info.plist"));
    let dir_name = app
        .file_stem()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let bundle_id = dict
        .as_ref()
        .and_then(|d| plist_string(d, "CFBundleIdentifier"))
        .map(|s| sanitize_bundle_id(&s))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| resolve_wrapper_bundle_id(app));
    let version = dict
        .as_ref()
        .and_then(|d| {
            plist_string(d, "CFBundleShortVersionString")
                .or_else(|| plist_string(d, "CFBundleVersion"))
        })
        .unwrap_or_default();
    let name = dict
        .as_ref()
        .and_then(|d| {
            plist_string(d, "CFBundleDisplayName").or_else(|| plist_string(d, "CFBundleName"))
        })
        .filter(|s| !s.is_empty())
        .unwrap_or(dir_name);
    AppMeta {
        bundle_id,
        background_only: ls_background_only(dict.as_ref()),
        version,
        name,
    }
}

/// Wrapper 回退：iOS 应用真实 plist 在 Wrapper/<name>.app/Info.plist。
fn resolve_wrapper_bundle_id(app: &Path) -> String {
    if let Ok(wrappers) = std::fs::read_dir(app.join("Wrapper")) {
        for wrapper in wrappers.flatten() {
            let plist_path = wrapper.path().join("Info.plist");
            if let Some(dict) = read_info_plist(&plist_path) {
                if let Some(id) = plist_string(&dict, "CFBundleIdentifier") {
                    let id = sanitize_bundle_id(&id);
                    if !id.is_empty() {
                        return id;
                    }
                }
            }
        }
    }
    "unknown".into()
}

/// 整串锚定的 bundle 模式匹配（对标 build_regex_var：点转义、`*`→任意、
/// ^$ 锚定；与 bash `[[ == $p ]]` 整串 glob 等价）。
fn anchored_bundle_match(value: &str, pattern: &str) -> bool {
    crate::clean::whitelist::glob_match(pattern, value)
}

/// 对标 should_protect_from_uninstall：系统关键 → 保护；Apple 可卸载 → 放行。
pub fn should_protect_from_uninstall(bundle_id: &str) -> bool {
    use crate::clean::protect_data::{APPLE_UNINSTALLABLE_APPS, SYSTEM_CRITICAL_BUNDLES};
    for pattern in APPLE_UNINSTALLABLE_APPS {
        if anchored_bundle_match(bundle_id, pattern) {
            return false;
        }
    }
    for pattern in SYSTEM_CRITICAL_BUNDLES {
        if anchored_bundle_match(bundle_id, pattern) {
            return true;
        }
    }
    false
}

fn is_app_bundle(path: &Path) -> bool {
    let Some(name) = path.file_name().map(|n| n.to_string_lossy().to_lowercase()) else {
        return false;
    };
    name.ends_with(".app")
}

/// 并行测径线程数上限（IO 密集，8 以上收益递减）。
const SIZE_WORKERS: usize = 8;

/// 列出全部应用（对标应用清单阶段；只读）。
///
/// 性能：应用元数据每只 .app 只读一次 Info.plist；测径（每只上限 2s，
/// 1:1 对标）改为跨应用并行——44 只应用串行最坏 88s，并行后 ≈ 单只耗时。
pub fn list_apps() -> Vec<AppInfo> {
    let roots = search_dirs();

    // 阶段一：目录遍历 + 元数据分类（单线程，plist 读取很快）。
    let mut apps: Vec<AppInfo> = Vec::new();
    for dir in &roots {
        // maxdepth 3 探测 *.app。
        let mut stack: Vec<(PathBuf, usize)> = vec![(dir.clone(), 0)];
        while let Some((cur, depth)) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&cur) else { continue };
            for entry in entries.flatten() {
                let path = entry.path();
                // 对标 `find -maxdepth 3 -iname "*.app"` + `-d` 检查：按名
                // 匹配 .app，目录判定用 metadata（跟踪符号链接）——
                // /Applications/Safari.app 是指向 Cryptex 的符号链接，
                // entry.file_type() 不跟踪链接，会把它整只漏掉。
                if is_app_bundle(&path) {
                    if !std::fs::metadata(&path)
                        .map(|m| m.is_dir())
                        .unwrap_or(false)
                    {
                        continue;
                    }
                    let meta = read_app_meta(&path);
                    let directly_in_root = path.parent().map(|p| p == dir).unwrap_or(false);
                    // 对标：后台专用应用仅当直接位于搜索根时列出。
                    if meta.background_only && !directly_in_root {
                        continue;
                    }
                    let protected = should_protect_from_uninstall(&meta.bundle_id);
                    let uninstallable = crate::clean::protect_data::APPLE_UNINSTALLABLE_APPS
                        .iter()
                        .any(|p| anchored_bundle_match(&meta.bundle_id, p));
                    apps.push(AppInfo {
                        path: path.to_string_lossy().to_string(),
                        name: meta.name,
                        bundle_id: meta.bundle_id,
                        version: meta.version,
                        size_bytes: 0,
                        protected,
                        uninstallable,
                        background_only: meta.background_only,
                        in_search_root: directly_in_root,
                    });
                    continue; // .app 内部不再下钻
                }
                // 非 .app：仅下钻真实目录（find 不进入符号链接目录）。
                let Ok(ft) = entry.file_type() else { continue };
                if ft.is_dir() && depth < 3 {
                    stack.push((path, depth + 1));
                }
            }
        }
    }

    // 阶段二：并行测径（对标 du -sk 的语义，每只 2s 上限）。
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
    let cursor = AtomicUsize::new(0);
    let sizes: Vec<AtomicU64> = (0..apps.len()).map(|_| AtomicU64::new(0)).collect();
    let workers = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .min(SIZE_WORKERS)
        .max(1);
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                loop {
                    let i = cursor.fetch_add(1, Ordering::SeqCst);
                    if i >= apps.len() {
                        return;
                    }
                    let deadline =
                        std::time::Instant::now() + std::time::Duration::from_secs(2);
                    let size = crate::clean::path_size_with_deadline(
                        Path::new(&apps[i].path),
                        deadline,
                    );
                    sizes[i].store(size, Ordering::Relaxed);
                }
            });
        }
    });
    for (a, size) in apps.iter_mut().zip(&sizes) {
        a.size_bytes = size.load(Ordering::Relaxed);
    }

    // 去重 + 按名称排序。
    apps.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    apps.dedup_by(|a, b| a.path == b.path);
    apps
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 对标 uninstall_resolve_bundle_id 的清洗语义。
    #[test]
    fn bundle_id_sanitization() {
        assert_eq!(sanitize_bundle_id("com.example|app"), "com.example-app");
        assert_eq!(sanitize_bundle_id("com.example\napp"), "com.exampleapp");
        assert_eq!(sanitize_bundle_id("(null)"), "");
        assert_eq!(sanitize_bundle_id("  com.example  "), "com.example");
    }

    /// 对标 should_protect_from_uninstall 用例。
    #[test]
    fn uninstall_protection_classification() {
        // 系统关键 → 保护。
        assert!(should_protect_from_uninstall("com.apple.finder"));
        assert!(should_protect_from_uninstall("com.apple.dock"));
        assert!(should_protect_from_uninstall("com.apple.SecurityAgent"));
        // Apple 可卸载 → 放行（先判）。
        assert!(!should_protect_from_uninstall("com.apple.dt.Xcode"));
        assert!(!should_protect_from_uninstall("com.apple.iWork.Pages"));
        assert!(!should_protect_from_uninstall("com.apple.garageband10"));
        // 第三方 → 不保护（卸载允许）。
        assert!(!should_protect_from_uninstall("com.tencent.xinWeChat"));
        assert!(!should_protect_from_uninstall("unknown"));
        // 锚定语义：com.apple.dt.* 是整串匹配，不匹配中缀——伪造前缀
        // 不命中任何模式 → 不保护（作为普通第三方应用可卸载）。
        assert!(!should_protect_from_uninstall("net.fake.com.apple.dt.Xcode"));
    }

    /// 搜索目录包含标准位置。
    #[test]
    fn search_dirs_cover_standard_locations() {
        let dirs = search_dirs();
        assert!(dirs.contains(&PathBuf::from("/Applications")));
        let home = std::env::var("HOME").unwrap();
        assert!(dirs.contains(&PathBuf::from(format!("{home}/Applications"))));
    }
}

/// 真机冒烟（默认忽略）。
#[cfg(test)]
mod smoke_tests {
    use super::*;

    #[test]
    #[ignore]
    fn uninstall_list_smoke() {
        let apps = list_apps();
        println!("apps={} (protected={})", apps.len(), apps.iter().filter(|a| a.protected).count());
        for a in apps.iter().take(10) {
            println!(
                "  {} {} {} [{:.1} MB]{}{}",
                a.name,
                a.bundle_id,
                a.version,
                a.size_bytes as f64 / 1048576.0,
                if a.protected { " 🛡" } else { "" },
                if a.background_only { " (bg)" } else { "" },
            );
        }
        assert!(!apps.is_empty());
    }
}
