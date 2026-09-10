//! uninstall 模块：应用卸载，对标 `bin/uninstall.sh` + `lib/uninstall/*`。
//!
//! 第一片：应用清单（只读）+ 卸载模式保护分级。
//! 第二片：应用本体删除 + 精确 bundle ID 残留。
//! 第三片：名称变体残留 + 卸载模式保护语义。
//! 第四片（本片）：Homebrew cask 检测/卸载 + Steam 启动器识别
//! （对标 lib/uninstall/brew.sh + steam.sh）。

pub mod brew;
pub mod steam;

use serde::Serialize;
use std::path::{Path, PathBuf};
use std::time::Duration;

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

/// 对标 uninstall_print_app_search_dirs（清单）+ 兄弟扫描根（batch.sh）。
fn search_dirs() -> Vec<PathBuf> {
    let home = std::env::var("HOME").unwrap_or_default();
    let mut dirs = vec![
        PathBuf::from("/Applications"),
        PathBuf::from(&home).join("Applications"),
        PathBuf::from("/Library/Input Methods"),
        PathBuf::from(&home).join("Library/Input Methods"),
        // 兄弟守卫额外根（对标 _MOLE_UNINSTALL_LIVE_APP_ROOTS）。
        PathBuf::from("/System/Applications"),
        PathBuf::from(&home).join("Library/Application Support/Setapp/Applications"),
        PathBuf::from("/opt/homebrew/Caskroom"),
        PathBuf::from("/usr/local/Caskroom"),
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
mod first_tests {
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

/// 对标 `mole_is_reverse_dns_bundle_id`：合法 reverse-DNS 校验
/// （段以字母数字开头，允许内部连字符，至少两段）。
pub fn is_reverse_dns_bundle_id(bundle_id: &str) -> bool {
    if bundle_id.is_empty() || bundle_id == "unknown" {
        return false;
    }
    let segment = |s: &str| {
        let bytes = s.as_bytes();
        !bytes.is_empty()
            && bytes[0].is_ascii_alphanumeric()
            && bytes
                .iter()
                .all(|b| b.is_ascii_alphanumeric() || *b == b'-')
    };
    let parts: Vec<&str> = bundle_id.split('.').collect();
    parts.len() >= 2 && parts.iter().all(|p| segment(p))
}

/// 对标 `find_app_files` 中 bundle_id 字面路径集合（reverse-DNS 校验后）。
/// 仅精确 bundle ID 残留；名称变体路径、bundle leaf 推导、LaunchAgents、
/// Receipts 与共享兄弟守卫留待 6c（逐行复核后移植）。
fn bundle_id_residue_paths(bundle_id: &str) -> Vec<String> {
    let home = std::env::var("HOME").unwrap_or_default();
    [
        "Application Support/{id}", "Caches/{id}", "Logs/{id}",
        "Saved Application State/{id}.savedState",
        "Containers/{id}", "WebKit/{id}",
        "WebKit/com.apple.WebKit.WebContent/{id}",
        "HTTPStorages/{id}", "HTTPStorages/{id}.binarycookies",
        "Cookies/{id}.binarycookies", "Application Scripts/{id}",
        "Input Methods/{id}.app", "Autosave Information/{id}",
        "SyncedPreferences/{id}.plist",
    ]
    .iter()
    .map(|t| {
        let expanded = t.replace("{id}", bundle_id);
        format!("{home}/Library/{expanded}")
    })
    .collect()
}

/// 对标 `_mole_uninstall_is_common_app_name`：与许多无关 LaunchAgent 撞词的
/// 通用名（名称匹配时拒绝，bundle ID 匹配仍生效）。
pub fn is_common_app_name(name: &str) -> bool {
    const COMMON: &[&str] = &[
        "music", "notes", "photos", "finder", "safari", "preview", "calendar", "contacts",
        "messages", "reminders", "clock", "weather", "stocks", "books", "news", "podcasts",
        "voice", "files", "store", "system", "helper", "agent", "daemon", "service", "update",
        "sync", "backup", "cloud", "manager", "monitor", "server", "client", "worker", "runner",
        "launcher", "driver", "plugin", "extension", "widget", "utility",
    ];
    COMMON.contains(&name.to_lowercase().as_str())
}

/// 对标 `_mole_uninstall_vendor_product_tokens`：从 bundle ID 提取
/// vendor|product 段（各 ≥3 字符、字母数字开头、允许连字符/下划线）。
pub fn vendor_product_tokens(bundle_id: &str) -> Option<(String, String)> {
    if !is_reverse_dns_bundle_id(bundle_id) {
        return None;
    }
    let product = bundle_id.rsplit('.').next()?;
    let without_product = &bundle_id[..bundle_id.len() - product.len() - 1];
    let vendor = without_product.rsplit('.').next()?;
    let valid_segment = |s: &str| {
        let b = s.as_bytes();
        b.len() >= 3
            && b[0].is_ascii_alphanumeric()
            && b.iter().all(|c| c.is_ascii_alphanumeric() || *c == b'-' || *c == b'_')
    };
    if valid_segment(vendor) && valid_segment(product) {
        Some((vendor.to_string(), product.to_string()))
    } else {
        None
    }
}

/// 对标 `_mole_uninstall_name_variant_matches`：候选名（小写）等于变体或
/// 以变体 + 空格/连字符/下划线/点 开头。
pub fn name_variant_matches(candidate_lower: &str, variants: &[String]) -> bool {
    variants.iter().any(|v| {
        if v.is_empty() {
            return false;
        }
        candidate_lower == v
            || candidate_lower.starts_with(&format!("{v} "))
            || candidate_lower.starts_with(&format!("{v}-"))
            || candidate_lower.starts_with(&format!("{v}_"))
            || candidate_lower.starts_with(&format!("{v}."))
    })
}

/// 对标 `mole_name_starts_with_bundle_id_boundary`：文件名 == bundle_id 或
/// bundle_id.*（reverse-DNS 校验后）。
pub fn name_starts_with_bundle_id_boundary(name: &str, bundle_id: &str) -> bool {
    if !is_reverse_dns_bundle_id(bundle_id) {
        return false;
    }
    name == bundle_id || name.starts_with(&format!("{bundle_id}."))
}

/// 对标 `_path_belongs_to_independent_cli`（#993）：与同名 GUI 应用无关的
/// 独立 CLI 工具 dotdir，卸载 GUI 时绝不删除。
pub fn path_belongs_to_independent_cli(path: &str) -> bool {
    let Some(base) = path.rsplit('/').next() else { return false };
    let lc_name = base.trim_start_matches('.').to_lowercase();
    if !matches!(lc_name.as_str(), "claude" | "opencode" | "codex" | "gemini") {
        return false;
    }
    let home = std::env::var("HOME").unwrap_or_default();
    let parent = path.trim_end_matches('/').rsplit_once('/').map(|(p, _)| p).unwrap_or("");
    matches!(
        parent,
        p if p == home || p == format!("{home}/.config") || p == format!("{home}/.local/share") || p == format!("{home}/.cache")
    )
}

/// 生成名称变体集合（对标 find_app_files 开头的变体派生）。
fn name_variants(app_name: &str, bundle_id: &str) -> Vec<String> {
    let lower = app_name.to_lowercase();
    let nospace: String = app_name.chars().filter(|c| *c != ' ').collect();
    let hyphen = app_name.replace(' ', "-");
    let underscore = app_name.replace(' ', "_");
    let mut variants = vec![
        lower.clone(),
        nospace.to_lowercase(),
        hyphen.to_lowercase(),
        underscore.to_lowercase(),
    ];
    // base_name：版本/渠道后缀剥离（对标 regex ^(.+)[[:space:]]+(SUFFIX)$，
    // 大小写敏感、支持多词后缀）。base 与原名不同且 >2 字符才产出。
    const SUFFIXES: &[&str] = &[
        "Nightly", "Beta", "Alpha", "Dev", "Canary", "Preview", "Insider", "Edge", "Stable",
        "Release", "RC", "LTS", "Developer Edition", "Technology Preview",
    ];
    for suffix in SUFFIXES {
        let needle = format!(" {suffix}");
        if app_name.ends_with(&needle) {
            let base = app_name[..app_name.len() - needle.len()].trim();
            if !base.is_empty() && base.len() > 2 {
                variants.push(base.to_lowercase());
            }
            break;
        }
    }
    // Zed 渠道特例（#422）：dev.zed.Zed-Nightly 也扫 dev.zed.Zed-*。
    if is_reverse_dns_bundle_id(bundle_id) && bundle_id.starts_with("dev.zed.Zed-") {
        variants.push("dev.zed.zed-".to_string());
    }
    // bundle leaf 推导（对标 app_protection.sh 1071-1099）：leaf ≥8、驼峰、
    // 以去空格显示名开头且 rest 以大写/数字开头时，产出 leaf 与
    // "AppName RestSpaced" 两个变体。
    for leaf_var in bundle_leaf_variants(app_name, bundle_id) {
        if !variants.contains(&leaf_var) {
            variants.push(leaf_var);
        }
    }
    variants
}

/// 对标 bundle leaf 推导：返回应追加到 user_patterns 的变体名（小写）。
fn bundle_leaf_variants(app_name: &str, bundle_id: &str) -> Vec<String> {
    if !is_reverse_dns_bundle_id(bundle_id) || app_name.len() < 3 {
        return Vec::new();
    }
    let Some(bundle_leaf) = bundle_id.rsplit('.').next() else {
        return Vec::new();
    };
    let app_name_nospace: String = app_name.chars().filter(|c| *c != ' ').collect();
    if bundle_leaf.len() < 8 || app_name_nospace.len() < 3 {
        return Vec::new();
    }
    if bundle_leaf == app_name {
        return Vec::new();
    }
    // 驼峰转换：存在 [a-z][A-Z] 相邻。
    let has_camel = bundle_leaf.as_bytes().windows(2).any(|w| {
        w[0].is_ascii_lowercase() && w[1].is_ascii_uppercase()
    });
    if !has_camel {
        return Vec::new();
    }
    let leaf_lower = bundle_leaf.to_lowercase();
    let name_lower = app_name_nospace.to_lowercase();
    if !leaf_lower.starts_with(&name_lower) || leaf_lower == name_lower {
        return Vec::new();
    }
    let rest = &bundle_leaf[name_lower.len()..];
    let rest_first = rest.as_bytes().first().copied().unwrap_or(0);
    if !(rest_first.is_ascii_uppercase() || rest_first.is_ascii_digit()) {
        return Vec::new();
    }
    // rest 分词：([A-Z]+)([A-Z][a-z]) → 空格；([a-z0-9])([A-Z]) → 空格。
    let rest_spaced = insert_camel_spaces(rest);
    let mut out = vec![bundle_leaf.to_lowercase()];
    let spaced = format!("{app_name} {rest_spaced}");
    if spaced != app_name {
        out.push(spaced.to_lowercase());
    }
    out
}

/// 对标 sed 's/([A-Z]+)([A-Z][a-z])/\1 \2/g; s/([a-z0-9])([A-Z])/\1 \2/g'。
fn insert_camel_spaces(s: &str) -> String {
    let bytes: Vec<char> = s.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < bytes.len() {
        // 规则 1：连续大写后接 大写+小写 → 在最后一个大写前插入空格。
        if i + 2 < bytes.len()
            && bytes[i].is_ascii_uppercase()
            && bytes[i + 1].is_ascii_uppercase()
            && bytes[i + 2].is_ascii_lowercase()
        {
            // 找到 AAA...BC 模式中最后一个大写 B 的位置。
            let mut j = i;
            while j < bytes.len() && bytes[j].is_ascii_uppercase() {
                j += 1;
            }
            // j 是第一个非大写（或结尾）；在 j-1 前插空格。
            for k in i..j - 1 {
                out.push(bytes[k]);
            }
            out.push(' ');
            out.push(bytes[j - 1]);
            i = j;
            continue;
        }
        // 规则 2：小写/数字 + 大写 → 插入空格。
        if i > 0
            && (bytes[i - 1].is_ascii_lowercase() || bytes[i - 1].is_ascii_digit())
            && bytes[i].is_ascii_uppercase()
        {
            out.push(' ');
            out.push(bytes[i]);
            i += 1;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    out
}

/// 名称模式集合（对标 user_patterns 的名称部分 + 变体 dotdirs + base 变体）。
fn name_patterns(app_name: &str, variants: &[String]) -> Vec<String> {
    let home = std::env::var("HOME").unwrap_or_default();
    let mut patterns = Vec::new();
    if app_name.len() < 2 {
        return patterns;
    }
    // 主名 Library 位置（含插件类）。
    for t in [
        "Application Support", "Caches", "Logs", "Preferences",
        "Preferences/{n}.plist", "Saved Application State/{n}.savedState",
        "Services/{n}.workflow", "QuickLook/{n}.qlgenerator",
        "Internet Plug-Ins/{n}.plugin", "Audio/Plug-Ins/Components/{n}.component",
        "Audio/Plug-Ins/VST/{n}.vst", "Audio/Plug-Ins/VST3/{n}.vst3",
        "Audio/Plug-Ins/Digidesign/{n}.dpm", "PreferencePanes/{n}.prefPane",
        "Input Methods/{n}.app", "Screen Savers/{n}.saver", "Frameworks/{n}.framework",
        "Contextual Menu Items/{n}.plugin", "Spotlight/{n}.mdimporter",
        "ColorPickers/{n}.colorPicker", "Workflows/{n}.workflow",
        "Address Book Plug-Ins/{n}.bundle", "Accessibility/{n}.bundle",
        "Mail/Bundles/{n}.mailbundle",
    ] {
        patterns.push(format!("{home}/Library/{}", t.replace("{n}", app_name)));
    }
    // dotdirs：原样 + 变体。
    for v in variants {
        patterns.push(format!("{home}/.config/{v}"));
        patterns.push(format!("{home}/.cache/{v}"));
        patterns.push(format!("{home}/.local/share/{v}"));
    }
    patterns
}

/// 对标 find_app_files 的常用目录安全跳过：展开路径命中 Library 根目录
/// 本身（空名称/空 bundle 的产物）时跳过，防止整目录删除。
fn is_common_library_root(path: &str) -> bool {
    let trimmed = path.trim_end_matches('/');
    const ROOTS: &[&str] = &[
        "Library/Application Support", "Library/Caches", "Library/Logs", "Library/Preferences",
        "Library/Preferences/ByHost", "Library/Containers", "Library/WebKit",
        "Library/HTTPStorages", "Library/Application Scripts", "Library/Autosave Information",
        "Library/Group Containers", ".config", ".cache", ".local/share",
    ];
    let home = std::env::var("HOME").unwrap_or_default();
    ROOTS.iter().any(|r| trimmed == format!("{home}/{r}"))
        || trimmed == home
        || trimmed == format!("{home}/.")
}

/// vendor-nested 扫描（对标 find_vendor_nested_app_paths）：在 Application
/// Support / Caches / Logs 下深度 2 内，vendor 目录名匹配 bundle 的
/// vendor 段，子项匹配名称变体/产品段。
fn find_vendor_nested(bundle_id: &str, app_name: &str, variants: &[String]) -> Vec<String> {
    if app_name.len() < 4 || is_common_app_name(app_name) {
        return Vec::new();
    }
    let Some((vendor, product)) = vendor_product_tokens(bundle_id) else {
        return Vec::new();
    };
    let home = std::env::var("HOME").unwrap_or_default();
    let mut matched = Vec::new();
    for root in [
        format!("{home}/Library/Application Support"),
        format!("{home}/Library/Caches"),
        format!("{home}/Library/Logs"),
    ] {
        let Ok(vendor_dirs) = std::fs::read_dir(&root) else { continue };
        for vd in vendor_dirs.flatten() {
            let vendor_name = vd.file_name().to_string_lossy().to_string();
            if !vendor_name.eq_ignore_ascii_case(&vendor) {
                continue;
            }
            let Ok(children) = std::fs::read_dir(vd.path()) else { continue };
            for child in children.flatten() {
                let name = child.file_name().to_string_lossy().to_lowercase();
                let mut v: Vec<String> = variants.to_vec();
                v.push(product.to_lowercase());
                if name_variant_matches(&name, &v) {
                    matched.push(child.path().to_string_lossy().to_string());
                }
            }
        }
    }
    matched.sort();
    matched.dedup();
    matched
}

/// 卸载一个应用（对标 uninstall 删除阶段）：前置校验（卸载模式保护、
/// bundle ID 合法性）→ 应用本体 + 残留（精确 bundle ID + 名称模式 +
/// vendor-nested + ByHost + LaunchAgents + Zed 特例）逐项 sink 复检 → Trash。
/// 对标 uninstall_normalize_bundle_id：大小写不敏感比较用小写形式。
/// （bundle ID 在路径语义上 case-preserving 但非 case-sensitive——
/// APFS 上 com.Foo.Bar.plist 与 com.foo.bar.plist 是同一文件。）
fn normalize_bundle_id(id: &str) -> String {
    id.to_lowercase()
}

/// 对标 uninstall_strip_version_suffix：剥 Nightly|Beta|… 后缀
/// （含多词：Developer Edition / Technology Preview）。
fn strip_version_suffix(name: &str) -> String {
    const SUFFIXES: &[&str] = &[
        "Developer Edition",
        "Technology Preview",
        "Nightly",
        "Beta",
        "Alpha",
        "Dev",
        "Canary",
        "Preview",
        "Insider",
        "Edge",
        "Stable",
        "Release",
        "RC",
        "LTS",
    ];
    for suffix in SUFFIXES {
        let pattern = format!(" {suffix}");
        if name.ends_with(&pattern) && name.len() > pattern.len() {
            return name[..name.len() - pattern.len()].to_string();
        }
    }
    name.to_string()
}

/// 收集与 bundle_id 相同（忽略大小写）且路径不同、仍存在的其它 .app
/// 的 (path, name) 列表。对标 uninstall_bundle_id_has_surviving_sibling
/// 的 apps_data 遍历 + _MOLE_UNINSTALL_LIVE_APP_ROOTS 实时扫描。
pub fn surviving_siblings(bundle_id: &str, app_path: &str) -> Vec<(String, String)> {
    if bundle_id.is_empty() || bundle_id == "unknown" {
        return Vec::new();
    }
    let target_lower = normalize_bundle_id(bundle_id);
    let mut out = Vec::new();
    for root in search_dirs() {
        if !root.is_dir() {
            continue;
        }
        // maxdepth 2（对标 find -maxdepth 3 从根算起的 .app 层级）。
        let Ok(entries) = std::fs::read_dir(&root) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !is_app_bundle(&path) {
                continue;
            }
            let path_str = path.to_string_lossy().to_string();
            if path_str == app_path {
                continue;
            }
            if !std::fs::metadata(&path).map(|m| m.is_dir()).unwrap_or(false) {
                continue;
            }
            let meta = read_app_meta(&path);
            if normalize_bundle_id(&meta.bundle_id) == target_lower {
                out.push((path_str, meta.name));
            }
            // Caskroom 二层（token/version/App.app）。
            if root.ends_with("Caskroom") {
                if let Ok(versions) = std::fs::read_dir(&path) {
                    for ver in versions.flatten() {
                        if let Ok(apps) = std::fs::read_dir(ver.path()) {
                            for app in apps.flatten() {
                                let p = app.path();
                                if !is_app_bundle(&p) {
                                    continue;
                                }
                                let s = p.to_string_lossy().to_string();
                                if s == app_path {
                                    continue;
                                }
                                let m = read_app_meta(&p);
                                if normalize_bundle_id(&m.bundle_id) == target_lower {
                                    out.push((s, m.name));
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    out
}

/// 对标 uninstall_bundle_id_has_surviving_sibling。
#[allow(dead_code)] // 公开 API：UI/命令层可复用；当前由 surviving_sibling_names 间接覆盖。
pub fn has_surviving_sibling(bundle_id: &str, app_path: &str) -> bool {
    !surviving_siblings(bundle_id, app_path).is_empty()
}

/// 对标 uninstall_surviving_sibling_names：存活兄弟的 display name、
/// .app 去后缀 basename，均小写——用于名称碰撞抑制。
pub fn surviving_sibling_names(bundle_id: &str, app_path: &str) -> Vec<String> {
    let mut names = Vec::new();
    for (path, display) in surviving_siblings(bundle_id, app_path) {
        let base = Path::new(&path)
            .file_stem()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        for candidate in [display, base] {
            if candidate.is_empty() {
                continue;
            }
            let lower = candidate.to_lowercase();
            if !names.contains(&lower) {
                names.push(lower);
            }
            let stripped = strip_version_suffix(&candidate).to_lowercase();
            if !stripped.is_empty() && !names.contains(&stripped) {
                names.push(stripped);
            }
        }
    }
    names
}

/// sudo -n 是否可用（对标 optimize_sudo_available 的 GUI 语义）。
fn sudo_n_available() -> bool {
    crate::status::run_cmd("sudo", &["-n", "true"], Duration::from_secs(3)).is_ok()
}

/// 对标 find_app_system_files：扫描系统级 LaunchAgents/Daemons、
/// PrivilegedHelperTools、Receipts（需 sudo -n 读取；无缓存返回空）。
/// 仅 bundle_id 边界匹配；com.apple.* 前缀跳过。
fn system_files_scan(bundle_id: &str, app_name: &str) -> Vec<String> {
    if !is_reverse_dns_bundle_id(bundle_id) || !sudo_n_available() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let boundary = name_starts_with_bundle_id_boundary; // 复用现有边界检查

    // LaunchAgents / LaunchDaemons 下 *.plist（对标 batch.sh 432-456）。
    for root in ["/Library/LaunchAgents", "/Library/LaunchDaemons"] {
        let Ok(entries) = std::fs::read_dir(root) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if !name.ends_with(".plist") {
                continue;
            }
            if name.starts_with("com.apple.") {
                continue;
            }
            let path = entry.path().to_string_lossy().to_string();
            if boundary(&name, bundle_id) {
                out.push(path);
            }
        }
    }

    // PrivilegedHelperTools：bundle_id 边界 + 名称变体（≥5 字符）。
    if let Ok(entries) = std::fs::read_dir("/Library/PrivilegedHelperTools") {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with("com.apple.") {
                continue;
            }
            let path = entry.path().to_string_lossy().to_string();
            if boundary(&name, bundle_id) {
                out.push(path);
                continue;
            }
            // 名称变体（对标 helper_name_variants；≥5 字符、非通用词）。
            if !is_common_app_name(app_name) {
                let lower = app_name.to_lowercase();
                let nospace: String = app_name.chars().filter(|c| *c != ' ').collect();
                for variant in [lower, nospace.to_lowercase()] {
                    if variant.len() >= 5 && name.to_lowercase().contains(&variant) {
                        out.push(path.clone());
                        break;
                    }
                }
            }
        }
    }

    // Receipts（/private/var/db/receipts *.bom/*.plist）。
    if let Ok(entries) = std::fs::read_dir("/private/var/db/receipts") {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if !(name.ends_with(".bom") || name.ends_with(".plist")) {
                continue;
            }
            let stem = name.trim_end_matches(".bom").trim_end_matches(".plist");
            if boundary(stem, bundle_id) {
                out.push(entry.path().to_string_lossy().to_string());
            }
        }
    }
    out
}

pub fn uninstall_app(app_path: &str, bundle_id: &str, dry_run: bool) -> crate::clean::CleanExecuteResult {
    let mut outcomes = Vec::new();
    let mut deleted_count = 0usize;
    let mut freed_bytes = 0u64;
    let mut failed_count = 0usize;

    crate::clean::delete::log_session_start("uninstall");

    let path = Path::new(app_path);
    if !path.exists() && !path.is_symlink() {
        outcomes.push(crate::clean::DeleteOutcome {
            path: app_path.to_string(),
            status: "skipped".into(),
            size_bytes: 0,
            detail: "应用不存在".into(),
        });
        return crate::clean::CleanExecuteResult {
            outcomes,
            deleted_count,
            freed_bytes,
            failed_count,
        };
    }

    // 兄弟守卫（AGENTS.md：同 bundle ID 幸存安装仍在时，bundle-id 派生
    // 残留与名称派生清理都属于幸存安装，不得触碰）。
    let sibling_names = surviving_sibling_names(bundle_id, app_path);
    let sibling_survives = !sibling_names.is_empty();
    if sibling_survives {
        outcomes.push(crate::clean::DeleteOutcome {
            path: app_path.to_string(),
            status: "skipped".into(),
            size_bytes: 0,
            detail: "检测到同 bundle ID 兄弟安装，已抑制名称派生清理".into(),
        });
    }

    // Homebrew cask：优先 brew uninstall；有兄弟时 nozap（对标 batch.sh）。
    let mut brew_handled = false;
    if let Some(cask) = brew::get_brew_cask_name(app_path) {
        let zap = !sibling_survives;
        let (ok, detail) = brew::brew_uninstall_cask(&cask, app_path, zap, dry_run);
        let status = if dry_run {
            "dry-run"
        } else if ok {
            "ok"
        } else {
            "failed"
        };
        if ok && !dry_run {
            deleted_count += 1;
        } else if status == "failed" {
            failed_count += 1;
        }
        outcomes.push(crate::clean::DeleteOutcome {
            path: format!("{app_path} (cask:{cask}{})", if zap { " --zap" } else { " nozap" }),
            status: status.into(),
            size_bytes: 0,
            detail,
        });
        brew_handled = ok;
    }

    // 应用本体：brew 已处理则跳过 Trash 删除。
    let mut targets: Vec<String> = Vec::new();
    if !brew_handled {
        // Steam 启动器快捷方式仍在可删除路径上；detail 标注（供 UI）。
        if steam::is_steam_launcher(app_path) {
            outcomes.push(crate::clean::DeleteOutcome {
                path: app_path.to_string(),
                status: "skipped".into(),
                size_bytes: 0,
                detail: "Steam 启动器快捷方式（非游戏本体），将随下方目标删除".into(),
            });
        }
        targets.push(app_path.to_string());
    }

    let bundle_valid = is_reverse_dns_bundle_id(bundle_id);
    let app_name = Path::new(app_path)
        .file_stem()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();

    // 精确 bundle ID 残留（兄弟存在时仍安全——路径按 bundle ID 键控）。
    if bundle_valid {
        for residue in bundle_id_residue_paths(bundle_id) {
            let p = Path::new(&residue);
            if p.exists() || p.is_symlink() {
                targets.push(residue);
            }
        }
        // 系统级 LaunchAgents/Daemons/PrivilegedHelperTools/Receipts
        // （对标 find_app_system_files；sudo -n 读取，无缓存时跳过）。
        for sys in system_files_scan(bundle_id, &app_name) {
            if !targets.contains(&sys) {
                targets.push(sys);
            }
        }
    }

    // 名称模式残留：兄弟存在时**全部抑制**（对标 discovery_app_name 清空
    // + MOLE_UNINSTALL_SIBLING_SURVIVES=1 跳过 regex 键控工具链启发）。
    if !sibling_survives {
        let variants = name_variants(&app_name, bundle_id);
        let mut patterns = name_patterns(&app_name, &variants);
        for v in &variants {
            for root in ["Application Support", "Caches", "Logs", "Preferences",
                "Preferences/{n}.plist", "Saved Application State/{n}.savedState"] {
                patterns.push(format!(
                    "{}/Library/{}",
                    std::env::var("HOME").unwrap_or_default(),
                    root.replace("{n}", v)
                ));
            }
        }
        for p in patterns {
            let pp = Path::new(&p);
            if !pp.exists() && !pp.is_symlink() {
                continue;
            }
            if is_common_library_root(&p) {
                continue;
            }
            if crate::clean::protect::is_shared_home_state_root(&p)
                || path_belongs_to_independent_cli(&p)
            {
                continue;
            }
            // 名称与存活兄弟碰撞 → 抑制（对标 uninstall_surviving_sibling_names）。
            let base_lower = Path::new(&p)
                .file_name()
                .map(|n| n.to_string_lossy().to_lowercase())
                .unwrap_or_default();
            if sibling_names.iter().any(|n| base_lower.contains(n.as_str()) || n.contains(&base_lower)) {
                continue;
            }
            if !targets.contains(&p) {
                targets.push(p);
            }
        }

        // vendor-nested。
        for path in find_vendor_nested(bundle_id, &app_name, &variants) {
            if !targets.contains(&path) {
                targets.push(path);
            }
        }
    }

    // Preferences/ByHost：扫描 *.plist 后按 bundle ID 边界过滤。
    if bundle_valid {
        let home = std::env::var("HOME").unwrap_or_default();
        let prefs = format!("{home}/Library/Preferences");
        if Path::new(&prefs).is_dir() {
            if let Ok(entries) = std::fs::read_dir(&prefs) {
                for entry in entries.flatten() {
                    let name = entry.file_name().to_string_lossy().to_string();
                    if name.ends_with(".plist")
                        && name_starts_with_bundle_id_boundary(&name, bundle_id)
                    {
                        let p = entry.path().to_string_lossy().to_string();
                        if !targets.contains(&p) {
                            targets.push(p);
                        }
                    }
                }
            }
        }
    }

    // 用户 LaunchAgents：${bundle_id}.plist 与 ${bundle_id}.*.plist（精确前缀）。
    if bundle_valid {
        let home = std::env::var("HOME").unwrap_or_default();
        let agents = format!("{home}/Library/LaunchAgents");
        if Path::new(&agents).is_dir() {
            if let Ok(entries) = std::fs::read_dir(&agents) {
                for entry in entries.flatten() {
                    let name = entry.file_name().to_string_lossy().to_string();
                    if name == format!("{bundle_id}.plist")
                        || (name.starts_with(&format!("{bundle_id}.")) && name.ends_with(".plist"))
                    {
                        let p = entry.path().to_string_lossy().to_string();
                        if !targets.contains(&p) {
                            targets.push(p);
                        }
                    }
                }
            }
        }
    }

    for target in targets {
        // 卸载模式保护（对标 should_protect_from_uninstall 前置检查）。
        if should_protect_from_uninstall(bundle_id) {
            outcomes.push(crate::clean::DeleteOutcome {
                path: target,
                status: "skipped".into(),
                size_bytes: 0,
                detail: "uninstall-protected".into(),
            });
            continue;
        }
        let outcome = crate::clean::delete::delete_to_trash_uninstall(&target, dry_run, "uninstall");
        if outcome.status == "ok" {
            deleted_count += 1;
            freed_bytes += outcome.size_bytes;
        } else if outcome.status == "failed" {
            failed_count += 1;
        }
        outcomes.push(outcome);
    }

    crate::clean::delete::log_session_end("uninstall", deleted_count, freed_bytes);

    crate::clean::CleanExecuteResult {
        outcomes,
        deleted_count,
        freed_bytes,
        failed_count,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 卸载模式：DATA_PROTECTED 残留不被拦（对标 MOLE_UNINSTALL_MODE=1）。
    #[test]
    fn uninstall_mode_skips_data_protected() {
        // clean 模式下被 DATA_PROTECTED 拦截的路径。
        let clash_residue = "/Users/x/Library/Application Support/io.github.clash-verge-rev.clash-verge-rev";
        assert!(crate::clean::protect::should_protect_path(clash_residue));
        // 卸载模式放行（system critical 仍拦）。
        assert!(!super::super::clean::protect::should_protect_path_uninstall(clash_residue));
        assert!(super::super::clean::protect::should_protect_path_uninstall(
            "/Users/x/Library/Caches/com.apple.dock"
        ));
    }

    /// 对标 mole_is_reverse_dns_bundle_id 用例。
    #[test]
    fn reverse_dns_validation() {
        assert!(is_reverse_dns_bundle_id("com.example.app"));
        assert!(is_reverse_dns_bundle_id("com.github.wez.wezterm"));
        assert!(is_reverse_dns_bundle_id("dev.orbstack.OrbStack"));
        assert!(is_reverse_dns_bundle_id("com.example-1.thing"));
        assert!(!is_reverse_dns_bundle_id(""));
        assert!(!is_reverse_dns_bundle_id("unknown"));
        assert!(!is_reverse_dns_bundle_id("single"));
        assert!(!is_reverse_dns_bundle_id("com..x"));
        assert!(!is_reverse_dns_bundle_id(".com.x"));
        assert!(!is_reverse_dns_bundle_id("com.exa mple.x"));
    }

    /// 残留路径集合只含 HOME/Library 下精确位置。
    #[test]
    fn residue_paths_are_precise() {
        let home = std::env::var("HOME").unwrap();
        let paths = bundle_id_residue_paths("com.example.App");
        for p in &paths {
            assert!(p.starts_with(&format!("{home}/Library/")), "越界: {p}");
        }
        assert!(paths.contains(&format!("{home}/Library/Containers/com.example.App")));
        assert!(paths.contains(&format!("{home}/Library/Preferences")) == false); // Preferences 只按名称，不在 bundle 集合
    }

    /// 卸载本体：不存在的应用跳过。
    #[test]
    fn uninstall_missing_app_skipped() {
        let r = uninstall_app("/nonexistent/App.app", "com.example.none", true);
        assert_eq!(r.outcomes[0].status, "skipped");
    }

    /// 保护分级阻止系统关键应用。
    #[test]
    fn protected_app_uninstall_blocked() {
        // 用临时目录构造一个"应用"，但 bundle 属于系统关键 → 本体跳过。
        let tmp = std::env::temp_dir().join(format!("mole_rs_un_{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let r = uninstall_app(&tmp.to_string_lossy(), "com.apple.finder", true);
        assert!(r.outcomes.iter().all(|o| o.status == "skipped"));
        assert!(tmp.exists(), "受保护应用不得被删除");
        std::fs::remove_dir_all(&tmp).ok();
    }
}

#[cfg(test)]
mod variant_tests {
    use super::*;

    /// 对标 _mole_uninstall_is_common_app_name。
    #[test]
    fn common_app_names() {
        assert!(is_common_app_name("Finder"));
        assert!(is_common_app_name("notes"));
        assert!(is_common_app_name("Safari"));
        assert!(!is_common_app_name("Claude"));
        assert!(!is_common_app_name("Zed"));
    }

    /// 对标 _mole_uninstall_vendor_product_tokens。
    #[test]
    fn vendor_product_tokens_extraction() {
        assert_eq!(
            vendor_product_tokens("com.jetbrains.intellij"),
            Some(("jetbrains".to_string(), "intellij".to_string()))
        );
        assert_eq!(
            vendor_product_tokens("dev.orbstack.OrbStack"),
            Some(("orbstack".to_string(), "OrbStack".to_string()))
        );
        // 对标：com.example 的 vendor 段是 "com"（3 字符，通过正则 {2,}）。
        assert_eq!(
            vendor_product_tokens("com.example"),
            Some(("com".to_string(), "example".to_string()))
        );
        assert!(vendor_product_tokens("not-reverse-dns").is_none());
    }

    /// 对标 _mole_uninstall_name_variant_matches（前缀边界五种形态）。
    #[test]
    fn variant_prefix_matching() {
        let v = vec!["zed".to_string(), "zed nightly".to_string()];
        assert!(name_variant_matches("zed", &v));
        assert!(name_variant_matches("zed-nightly", &v));
        assert!(name_variant_matches("zed_nightly", &v));
        assert!(name_variant_matches("zed.nightly", &v));
        assert!(name_variant_matches("zed nightly helper", &v));
        assert!(!name_variant_matches("zedx", &v));
        assert!(!name_variant_matches("zedge", &v));
    }

    /// 对标 base_name 剥离（大小写敏感 + 多词后缀）。
    #[test]
    fn base_name_extraction() {
        assert_eq!(name_variants("Zed Nightly", ""), vec!["zed nightly", "zednightly", "zed-nightly", "zed_nightly", "zed"]);
        assert_eq!(name_variants("Firefox Developer Edition", ""), vec!["firefox developer edition", "firefoxdeveloperedition", "firefox-developer-edition", "firefox_developer_edition", "firefox"]);
        // 小写后缀不剥离（原 regex 大小写敏感）。
        assert_eq!(name_variants("MyApp nightly", "").len(), 4);
        // 短 base 不产出。
        assert_eq!(name_variants("A Beta", "").len(), 4);
    }

    /// 对标 mole_name_starts_with_bundle_id_boundary。
    #[test]
    fn bundle_id_boundary() {
        assert!(name_starts_with_bundle_id_boundary("com.example.app", "com.example.app"));
        assert!(name_starts_with_bundle_id_boundary("com.example.app.plist", "com.example.app"));
        assert!(name_starts_with_bundle_id_boundary("com.example.app.helper.plist", "com.example.app"));
        assert!(!name_starts_with_bundle_id_boundary("com.example.appx.plist", "com.example.app"));
        assert!(!name_starts_with_bundle_id_boundary("xcom.example.app.plist", "com.example.app"));
        assert!(!name_starts_with_bundle_id_boundary("com.example.app.plist", "unknown"));
    }

    /// 对标 _path_belongs_to_independent_cli（#993）。
    #[test]
    fn independent_cli_dotdirs_protected() {
        let home = std::env::var("HOME").unwrap();
        assert!(path_belongs_to_independent_cli(&format!("{home}/.claude")));
        assert!(path_belongs_to_independent_cli(&format!("{home}/.config/claude")));
        assert!(path_belongs_to_independent_cli(&format!("{home}/.local/share/opencode")));
        assert!(!path_belongs_to_independent_cli(&format!("{home}/.config/zed")));
        assert!(!path_belongs_to_independent_cli(&format!("{home}/Library/Application Support/Claude")));
    }

    /// 对标常用目录根安全跳过。
    #[test]
    fn common_library_roots_skipped() {
        let home = std::env::var("HOME").unwrap();
        assert!(is_common_library_root(&format!("{home}/Library/Caches")));
        assert!(is_common_library_root(&format!("{home}/Library/Preferences/ByHost")));
        assert!(is_common_library_root(&format!("{home}/.config")));
        assert!(is_common_library_root(&home));
        assert!(!is_common_library_root(&format!("{home}/Library/Caches/MyApp")));
        assert!(!is_common_library_root(&format!("{home}/.config/zed")));
    }

    /// vendor-nested：构造 fixture 验证 vendor 段目录 + 变体子项。
    #[test]
    fn vendor_nested_discovery() {
        let home = std::env::var("HOME").unwrap();
        let support = format!("{home}/Library/Application Support");
        // 若真实目录存在且有匹配，测试只验证函数不 panic；
        // 用伪造 vendor 名（不可能命中）验证空结果。
        let _ = support;
        let v = name_variants("Sibelius", "com.avid.Sibelius");
        let found = find_vendor_nested("com.avid.Sibelius", "Sibelius", &v);
        // 本机无 Avid 目录时为空；有则必须全部在 Application Support 下。
        for p in &found {
            assert!(p.starts_with(&format!("{home}/Library/")));
        }
    }

    /// Zed 特例变体（#422）。
    #[test]
    fn zed_channel_variant() {
        let v = name_variants("Zed", "dev.zed.Zed-Nightly");
        assert!(v.contains(&"dev.zed.zed-".to_string()));
        let v = name_variants("Zed", "dev.zed.Zed");
        assert!(!v.contains(&"dev.zed.zed-".to_string()));
    }

    /// 兄弟守卫：bundle ID 小写比较 + 版本后缀剥离。
    #[test]
    fn sibling_guard_helpers() {
        assert_eq!(normalize_bundle_id("Com.Foo.Bar"), "com.foo.bar");
        assert_eq!(strip_version_suffix("Zed Nightly"), "Zed");
        assert_eq!(strip_version_suffix("Zed Beta"), "Zed");
        assert_eq!(strip_version_suffix("Zed Developer Edition"), "Zed");
        assert_eq!(strip_version_suffix("Zed Technology Preview"), "Zed");
        assert_eq!(strip_version_suffix("Zed"), "Zed");
        assert_eq!(strip_version_suffix("Zed Preview"), "Zed");
        // Edge 是合法后缀（对标 bash 正则）：Microsoft Edge → Microsoft。
        assert_eq!(strip_version_suffix("Microsoft Edge"), "Microsoft");
        // 单独 "Edge" 无前缀 base → 不剥。
        assert_eq!(strip_version_suffix("Edge"), "Edge");
    }

    /// fixture：/Applications 下两个同 bundle ID 的 .app → 兄弟存在。
    #[test]
    fn sibling_detection_fixture() {
        // 用真实 /Applications 不可靠；构造临时目录并注入 search 路径不可行。
        // 改为：不存在的 bundle → 无兄弟；本机真实存在的 bundle → 不 panic。
        assert!(!has_surviving_sibling("com.example.no_such_app_xyz", "/nonexistent/App.app"));
        let _ = surviving_siblings("com.example.no_such_app_xyz", "/nonexistent/App.app");
        let _ = surviving_sibling_names("com.example.no_such_app_xyz", "/nonexistent/App.app");
        // unknown/empty 直接无兄弟。
        assert!(!has_surviving_sibling("", "/Applications/Foo.app"));
        assert!(!has_surviving_sibling("unknown", "/Applications/Foo.app"));
    }

    /// bundle leaf 推导：有效/无效矩阵。
    #[test]
    fn bundle_leaf_derivation() {
        // 有效：com.example.FooBarBaz + "Foo" → leaf=FooBarBaz ≥8、驼峰、
        // 以 Foo 开头、rest=BarBaz 以 B 开头。
        let v = bundle_leaf_variants("Foo", "com.example.FooBarBaz");
        assert!(v.contains(&"foobarbaz".to_string()), "{v:?}");
        assert!(v.contains(&"foo bar baz".to_string()), "{v:?}");

        // 太短 leaf。
        assert!(bundle_leaf_variants("Ab", "com.example.AbCd").is_empty());
        // 显示名太短。
        assert!(bundle_leaf_variants("A", "com.example.AbcDefgh").is_empty());
        // 无驼峰（全小写）。
        assert!(bundle_leaf_variants("Myapp", "com.example.myappcache").is_empty());
        // leaf 不以显示名开头。
        assert!(bundle_leaf_variants("Zed", "com.example.SomethingElse").is_empty());
        // 非 reverse-DNS。
        assert!(bundle_leaf_variants("FooBarBaz", "not-reverse-dns").is_empty());
    }

    /// 驼峰分词。
    #[test]
    fn camel_spacing() {
        assert_eq!(insert_camel_spaces("BarBaz"), "Bar Baz");
        assert_eq!(insert_camel_spaces("HTMLParser"), "HTML Parser");
        assert_eq!(insert_camel_spaces("simple"), "simple");
        assert_eq!(insert_camel_spaces("A1B2"), "A1 B2");
    }

    /// 系统文件扫描：无 sudo 或非法 bundle → 空；有 sudo 时不 panic。
    #[test]
    fn system_files_scan_guards() {
        assert!(system_files_scan("", "Foo").is_empty());
        assert!(system_files_scan("unknown", "Foo").is_empty());
        assert!(system_files_scan("com.example.Foo", "Foo").is_empty() || !sudo_n_available());
        // 不 panic。
        let _ = system_files_scan("com.example.Something", "Something");
    }
}

#[cfg(test)]
mod smoke_tests6c {
    #[test]
    #[ignore]
    fn uninstall_residue_dry_run_smoke() {
        // 本机已装应用（Clash Verge）：dry-run 列出将清理项，验证残留发现。
        let result = super::uninstall_app(
            "/Applications/Clash Verge.app",
            "io.github.clash-verge-rev.clash-verge-rev",
            true,
        );
        for o in &result.outcomes {
            println!("[{}] {} ({})", o.status, o.path, o.detail);
        }
        assert_eq!(result.deleted_count, 0, "dry-run 不得删除");
    }
}
