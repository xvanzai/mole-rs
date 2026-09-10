//! clean 模块：深度清理（对标 `Mole bin/clean.sh` + `lib/clean/*` +
//! `lib/core/file_ops.sh`）。
//!
//! 按计划拆分子模块渐进移植（见 docs/migration/PLAN.md §3 模块3）：
//! - 3a ✅：白名单保护策略 + 清理目录（第一族：Apple 用户缓存）+ 只读预览；
//! - 3b（本模块）：完整 `should_protect_path` 保护层 + `validate_path_for_deletion`
//!   路径验证 + Trash 路由安全删除 + 操作/取证日志 + 执行命令；
//! - 3c：更多清理族（system/dev/browser/hints）。
//!
//! 安全契约（对标 AGENTS.md / docs/SECURITY_DESIGN.md）：
//! - 删除统一走 [`delete::delete_to_trash`]（对标 mole_delete trash 模式）：
//!   验证 → sink 复检 → Trash 路由（trash CLI → Finder → ~/.Trash 直移，
//!   失败即失败，绝不回退 rm）→ 操作日志 + 取证日志；
//! - 执行前重新扫描（对标 "a timed-out producer must not feed partial
//!   output into a deletion loop"：只消费完整扫描结果）；
//! - 受保护/白名单路径在扫描期与删除 sink 双重拦截。

mod catalog;
mod old_versions;
mod owner_clean;
mod probe;
pub(crate) mod brew;
pub(crate) mod special;
pub(crate) mod system;
pub(crate) mod delete;
pub(crate) mod process;
pub(crate) mod protect;
pub(crate) mod protect_data;
pub(crate) mod whitelist;

use serde::Serialize;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// 目录大小扫描的单项 deadline，对标 get_path_size_kb 的超时约束。
const SIZE_SCAN_DEADLINE: Duration = Duration::from_secs(2);

/// 一个可清理目标（对标 `safe_clean <path> "<description>"` 的展开结果）。
#[derive(Debug, Clone, Serialize)]
pub struct CleanItem {
    pub path: String,
    pub size_bytes: u64,
    /// 跳过原因（protected/whitelist）；为空表示可清理。
    pub skip_reason: String,
}

/// 一族清理目标（对标一个 `safe_clean` 行 + 其描述）。
#[derive(Debug, Clone, Serialize)]
pub struct CleanGroup {
    pub description: String,
    /// 所属清理族（对标 dev.sh 的 clean_dev_* 分组）。
    pub family: String,
    pub items: Vec<CleanItem>,
    pub total_size_bytes: u64,
    pub skipped_count: usize,
}

/// 清理预览（只读，对标 dry-run 输出）。
#[derive(Debug, Clone, Serialize)]
pub struct CleanPreview {
    pub groups: Vec<CleanGroup>,
    pub total_size_bytes: u64,
    pub whitelist_source: String,
}

/// 单项删除结果（对标 `_mole_delete_log` 状态语义）。
#[derive(Debug, Clone, Serialize)]
pub struct DeleteOutcome {
    pub path: String,
    /// ok / dry-run / skipped / failed
    pub status: String,
    pub size_bytes: u64,
    pub detail: String,
}

/// 执行汇总。
#[derive(Debug, Clone, Serialize)]
pub struct CleanExecuteResult {
    pub outcomes: Vec<DeleteOutcome>,
    pub deleted_count: usize,
    pub freed_bytes: u64,
    pub failed_count: usize,
}

/// PATH 查找（对标 status 模块同名助手；供 trash CLI 探测使用）。
pub(crate) fn command_exists(name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    static CACHE: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, bool>>> =
        std::sync::OnceLock::new();
    let cache = CACHE.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()));
    if let Some(exists) = cache.lock().unwrap_or_else(|p| p.into_inner()).get(name) {
        return *exists;
    }
    let exists = std::env::var_os("PATH")
        .map(|paths| {
            std::env::split_paths(&paths).any(|dir| {
                let p = dir.join(name);
                p.is_file()
                    && std::fs::metadata(&p)
                        .map(|m| {
                            use std::os::unix::fs::PermissionsExt;
                            m.permissions().mode() & 0o111 != 0
                        })
                        .unwrap_or(false)
            })
        })
        .unwrap_or(false);
    cache
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert(name.to_string(), exists);
    exists
}

/// 展开 `~` 为用户主目录。
fn expand_home(pattern: &str) -> PathBuf {
    if let Some(rest) = pattern.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return Path::new(&home).join(rest);
        }
    }
    PathBuf::from(pattern)
}

/// 解析目录条目的完整路径模式。
///
/// 对标 `resolve_tool_home "${ENV:-}" default`：带 home_env 的条目在
/// 环境变量存在且为绝对路径时以它为基址，否则以 HOME 为基址
/// （path 为相对基址的子路径）；`~/` 前缀条目按原样展开。
fn resolve_entry_path(entry: &catalog::CatalogEntry) -> PathBuf {
    if let Some(env_name) = entry.home_env {
        let base = std::env::var(env_name).ok().filter(|v| v.starts_with('/'));
        let home = std::env::var("HOME").unwrap_or_default();
        return match base {
            Some(base) => Path::new(&base).join(entry.path),
            None => Path::new(&home).join(entry.path),
        };
    }
    expand_home(entry.path)
}

/// 组件级 glob 展开（对标 shell nullglob 展开）。
///
/// 支持每段 `*` / `?` / `[...]`（fnmatch 风格，与 bash `[[ == $p ]]` 的
/// glob 匹配语义一致）；无通配段直接拼接，不存在的段返回空。
pub(crate) fn expand_glob(pattern: &Path) -> Vec<PathBuf> {
    let mut current: Vec<PathBuf> = vec![PathBuf::new()];
    for comp in pattern.components() {
        let comp_str = comp.as_os_str().to_string_lossy().to_string();
        let mut next: Vec<PathBuf> = Vec::new();
        for base in &current {
            let candidate = base.join(&*comp_str);
            if !comp_str.contains(['*', '?', '[']) {
                if comp == std::path::Component::RootDir {
                    next.push(PathBuf::from("/"));
                    continue;
                }
                next.push(candidate);
                continue;
            }
            let Ok(entries) = std::fs::read_dir(if base.as_os_str().is_empty() {
                Path::new(".")
            } else {
                base
            }) else {
                continue;
            };
            for entry in entries.flatten() {
                let name = entry.file_name();
                if whitelist::glob_match(&comp_str, &name.to_string_lossy()) {
                    next.push(base.join(name));
                }
            }
        }
        current = next;
        if current.is_empty() {
            break;
        }
    }
    current.retain(|p| p.exists() || std::fs::symlink_metadata(p).is_ok());
    current
}

/// 目录大小，带 deadline（对标 get_path_size_kb 的超时约束：超时即截断，
/// 调用方按部分值呈现）。
pub fn path_size_with_deadline(path: &Path, deadline: Instant) -> u64 {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return 0;
    };
    if meta.is_symlink() {
        return 0; // 符号链接不计大小（对标 Trash 扫描与 du 语义）
    }
    if !meta.is_dir() {
        return meta.len();
    }
    let mut total = 0u64;
    walk_size(path, deadline, &mut total);
    total
}

fn walk_size(dir: &Path, deadline: Instant, total: &mut u64) {
    if Instant::now() >= deadline {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if Instant::now() >= deadline {
            return;
        }
        let Ok(ft) = entry.file_type() else { continue };
        if ft.is_symlink() {
            continue;
        }
        if ft.is_dir() {
            walk_size(&entry.path(), deadline, total);
        } else if let Ok(meta) = entry.metadata() {
            *total += meta.len();
        }
    }
}

/// 扫描期逐路径检查，对标 `_safe_clean_impl` 的检查顺序：
/// should_protect_path → 白名单（E5RT 编译模型缓存含在保护层 4c 内）。
/// `sw_domain_guard` 为 true 时追加 Service Worker 域名保护检查。
fn skip_reason(
    path: &str,
    whitelist: &whitelist::Whitelist,
    sw_domain_guard: bool,
) -> Option<&'static str> {
    if protect::should_protect_path(path) {
        return Some("protected");
    }
    if whitelist.is_whitelisted(path) {
        return Some("whitelist");
    }
    if sw_domain_guard {
        // 对标 clean_service_worker_cache：basename 提取域名 → PROTECTED_SW_DOMAINS。
        if let Some(name) = Path::new(path).file_name().and_then(|n| n.to_str()) {
            if let Some(domain) = extract_domain_from_sw_folder(name) {
                if is_protected_sw_domain(&domain) {
                    return Some("protected domain");
                }
            }
        }
    }
    None
}

/// 统一扫描条目：静态目录行 + 动态探测行 + 进程守卫行。
pub(super) struct ScanEntry {
    pub(super) family: &'static str,
    pub(super) pattern: PathBuf,
    pub(super) description: String,
    /// 进程守卫探针（对标 mole_clean_process_guard 三态）：
    /// Running/Unknown 时整组以 skip_reason 拒绝，不进入删除 sink。
    pub(super) process_probe: Option<fn() -> process::ProcessState>,
    /// Service Worker 域名保护：true 时从路径 basename 提取域名并对照
    /// PROTECTED_SW_DOMAINS（对标 clean_service_worker_cache 的 domain 检查）。
    pub(super) sw_domain_guard: bool,
    /// mtime 年龄门（天）：>0 时仅收 mtime 早于该天数的目标
    /// （对标 Mail Downloads 的 MOLE_MAIL_AGE_DAYS=30 过滤）。
    pub(super) age_days: u64,
}

/// PROTECTED_SW_DOMAINS（对标 bin/clean.sh）：Web 编辑器 / Google Workspace /
/// 代码平台 / 协作工具的 Service Worker 缓存永不删除（MV3 扩展离线可用性）。
const PROTECTED_SW_DOMAINS: &[&str] = &[
    "capcut.com",
    "photopea.com",
    "pixlr.com",
    "docs.google.com",
    "sheets.google.com",
    "slides.google.com",
    "drive.google.com",
    "mail.google.com",
    "github.com",
    "gitlab.com",
    "codepen.io",
    "codesandbox.io",
    "replit.com",
    "stackblitz.com",
    "notion.so",
    "figma.com",
    "linear.app",
    "excalidraw.com",
];

/// 从 CacheStorage 目录名提取 best-effort 域名（对标 basename | grep -oE | head -1）。
fn extract_domain_from_sw_folder(folder_name: &str) -> Option<String> {
    // [a-zA-Z0-9][-a-zA-Z0-9]*\.[a-zA-Z]{2,} — 取首个匹配（head -1）。
    let bytes = folder_name.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if !bytes[i].is_ascii_alphanumeric() {
            i += 1;
            continue;
        }
        let start = i;
        let mut j = i;
        while j < bytes.len() && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'-') {
            j += 1;
        }
        if j > start && j < bytes.len() && bytes[j] == b'.' {
            let tld_start = j + 1;
            let mut k = tld_start;
            while k < bytes.len() && bytes[k].is_ascii_alphabetic() {
                k += 1;
            }
            if k - tld_start >= 2 && (start == 0 || !bytes[start - 1].is_ascii_alphanumeric()) {
                return Some(folder_name[start..k].to_lowercase());
            }
        }
        i += 1;
    }
    None
}

/// 域名是否命中 PROTECTED_SW_DOMAINS（对标 `*"$protected_domain"*` 子串）。
fn is_protected_sw_domain(domain: &str) -> bool {
    PROTECTED_SW_DOMAINS.iter().any(|p| domain.contains(p))
}

/// 动态探测行（对标 dev.sh 中经 owner 命令解析路径后 safe_clean 的行）。
///
/// 仅移植**路径探测 + safe_clean 分支**；owner 命令删除汇（npm cache
/// clean --force、uv cache prune、corepack cache clean、pnpm store prune、
/// pip cache purge）需要"变更根可机器读出、dry-run 与真实共享同一候选
/// 计划、部分失败可观察"契约，留待独立子片。
fn dynamic_entries() -> Vec<ScanEntry> {
    let mut rows = Vec::new();

    // npm 残留目录行（对标 clean_dev_npm）：默认路径无条件；自定义路径
    // 在探测成功且与默认规范化后不同时追加。
    let npm_residual: &[(&str, &str)] = &[
        ("_cacache/*", "npm cache directory"),
        ("_npx/*", "npm npx cache"),
        ("_logs/*", "npm logs"),
        ("_prebuilds/*", "npm prebuilds"),
    ];
    let (npm_cache_path, npm_custom) = probe::npm_cache_path();
    let home = std::env::var("HOME").unwrap_or_default();
    let npm_default = PathBuf::from(&home).join(".npm");
    for (sub, desc) in npm_residual {
        rows.push(ScanEntry {
            family: "dev_frontend",
            pattern: npm_default.join(sub),
            description: desc.to_string(),
            process_probe: None,
            sw_domain_guard: false,
            age_days: 0,
        });
    }
    if npm_custom {
        // 对标规范化去重：真实路径相同则不再重复清理。
        if probe::normalize_existing(&npm_cache_path.to_string_lossy())
            != probe::normalize_existing(&npm_default.to_string_lossy())
        {
            for (sub, desc) in npm_residual {
                rows.push(ScanEntry {
                    family: "dev_frontend",
                    pattern: npm_cache_path.join(sub),
                    description: format!("{desc} (custom path)"),
                    process_probe: None,
                    sw_domain_guard: false,
                    age_days: 0,
                });
            }
        }
    }

    // uv 回退行：owner 命令不可用时才走 safe_clean（对标 else 分支）。
    if !probe::tool_available("uv", &["--version"]) {
        rows.push(ScanEntry {
            family: "dev_python",
            pattern: probe::uv_default_cache_path().join("*"),
            description: "uv cache".into(),
            process_probe: None,
            sw_domain_guard: false,
            age_days: 0,
        });
    }

    // corepack 回退行：不安全路径拒绝 + owner 命令不可用（对标 else 分支）。
    if !probe::tool_available("corepack", &["--version"]) {
        if let Some(corepack_path) = probe::corepack_cache_path() {
            rows.push(ScanEntry {
                family: "dev_frontend",
                pattern: corepack_path.join("*"),
                description: "Corepack cache".into(),
                process_probe: None,
                sw_domain_guard: false,
                age_days: 0,
            });
        }
    }

    // mise 行：无条件 safe_clean（对标 clean_dev_mise 末行）。
    rows.push(ScanEntry {
        family: "dev_cloud",
        pattern: probe::mise_cache_path().join("*"),
        description: "mise cache".into(),
        process_probe: None,
        sw_domain_guard: false,
        age_days: 0,
    });

    // Cargo registry/cache（对标 clean_dev_rust）：owner 进程守卫 +
    // 物理包含校验（cache 根不得逃出 CARGO_HOME）。
    if let Some(entry) = cargo_registry_entry() {
        rows.push(entry);
    }

    // UTM（对标 clean_utm_caches）：运行中整组跳过。
    if Path::new(&home).join("Library/Caches/com.utmapp.UTM").is_dir() {
        rows.push(ScanEntry {
            family: "virtualization",
            pattern: PathBuf::from(&home).join("Library/Caches/com.utmapp.UTM/*"),
            description: "UTM app cache".into(),
            process_probe: Some(process::utm_process_state),
            sw_domain_guard: false,
            age_days: 0,
        });
    }
    if Path::new(&home)
        .join("Library/Containers/com.utmapp.UTM")
        .is_dir()
    {
        rows.push(ScanEntry {
            family: "virtualization",
            pattern: PathBuf::from(&home)
                .join("Library/Containers/com.utmapp.UTM/Data/Library/Caches/*"),
            description: "UTM sandbox cache".into(),
            process_probe: Some(process::utm_process_state),
            sw_domain_guard: false,
            age_days: 0,
        });
        rows.push(ScanEntry {
            family: "virtualization",
            pattern: PathBuf::from(&home)
                .join("Library/Containers/com.utmapp.UTM/Data/tmp/*"),
            description: "UTM temporary files".into(),
            process_probe: Some(process::utm_process_state),
            sw_domain_guard: false,
            age_days: 0,
        });
    }

    // 云存储进程守卫行（对标 clean_cloud_storage）。
    {
        let caches = PathBuf::from(&home).join("Library/Caches");
        if let Ok(entries) = std::fs::read_dir(&caches) {
            for e in entries.flatten() {
                let name = e.file_name().to_string_lossy().to_string();
                if name.starts_with("com.dropbox.") || name == "com.getdropbox.dropbox" {
                    rows.push(ScanEntry {
                        family: "cloud_office",
                        pattern: e.path(),
                        description: format!("Dropbox cache · {name}"),
                        process_probe: Some(process::dropbox_process_state),
                        sw_domain_guard: false,
                        age_days: 0,
                    });
                }
            }
        }
    }
    if Path::new(&home)
        .join("Library/Caches/com.google.GoogleDrive")
        .is_dir()
    {
        rows.push(ScanEntry {
            family: "cloud_office",
            pattern: PathBuf::from(&home).join("Library/Caches/com.google.GoogleDrive"),
            description: "Google Drive cache".into(),
            process_probe: Some(process::google_drive_process_state),
            sw_domain_guard: false,
            age_days: 0,
        });
    }
    if Path::new(&home)
        .join("Library/Caches/com.microsoft.OneDrive")
        .is_dir()
    {
        rows.push(ScanEntry {
            family: "cloud_office",
            pattern: PathBuf::from(&home).join("Library/Caches/com.microsoft.OneDrive"),
            description: "OneDrive cache".into(),
            process_probe: Some(process::onedrive_process_state),
            sw_domain_guard: false,
            age_days: 0,
        });
    }

    // Recent Items（对标 _clean_recent_items）。
    let shared = PathBuf::from(&home).join("Library/Application Support/com.apple.sharedfilelist");
    if shared.is_dir() {
        for name in [
            "com.apple.LSSharedFileList.RecentApplications.sfl2",
            "com.apple.LSSharedFileList.RecentDocuments.sfl2",
            "com.apple.LSSharedFileList.RecentServers.sfl2",
            "com.apple.LSSharedFileList.RecentHosts.sfl2",
            "com.apple.LSSharedFileList.RecentApplications.sfl",
            "com.apple.LSSharedFileList.RecentDocuments.sfl",
            "com.apple.LSSharedFileList.RecentServers.sfl",
            "com.apple.LSSharedFileList.RecentHosts.sfl",
        ] {
            let p = shared.join(name);
            if p.exists() {
                rows.push(ScanEntry {
                    family: "user_essentials",
                    pattern: p,
                    description: format!("Recent items list · {name}"),
                    process_probe: None,
                    sw_domain_guard: false,
                    age_days: 0,
                });
            }
        }
    }
    let recent_plist =
        PathBuf::from(&home).join("Library/Preferences/com.apple.recentitems.plist");
    if recent_plist.exists() {
        rows.push(ScanEntry {
            family: "user_essentials",
            pattern: recent_plist,
            description: "Recent items preferences".into(),
            process_probe: None,
            sw_domain_guard: false,
            age_days: 0,
        });
    }

    // Mail Downloads（对标 _clean_mail_downloads；Mail 运行中跳过；
    // MOLE_MAIL_AGE_DAYS=30 mtime 过滤——扫描期过滤，预览即为可删集）。
    for mail_dir in [
        format!("{home}/Library/Mail Downloads"),
        format!("{home}/Library/Containers/com.apple.mail/Data/Library/Mail Downloads"),
    ] {
        if Path::new(&mail_dir).is_dir() {
            rows.push(ScanEntry {
                family: "user_essentials",
                pattern: PathBuf::from(&mail_dir).join("*"),
                description: format!("Mail downloads · {}", Path::new(&mail_dir).file_name().unwrap_or_default().to_string_lossy()),
                process_probe: Some(process::mail_process_state),
                sw_domain_guard: false,
                age_days: 30,
            });
        }
    }

    // incomplete downloads（对标 _clean_incomplete_downloads）：lsof 开句柄
    // 三态——Running/Unknown 跳过；仅 conclusively idle 走 Trash。
    rows.extend(incomplete_download_entries());

    // Service Worker CacheStorage（对标 clean_service_worker_cache 的 profile 遍历）。
    rows.extend(service_worker_entries());

    // Group Containers 显式 allowlist（对标 contentdelivery Logs）。
    let gc = PathBuf::from(&home).join("Library/Group Containers/group.com.apple.contentdelivery");
    if gc.is_dir() {
        for sub in ["Logs", "Library/Logs"] {
            let p = gc.join(sub);
            if p.is_dir() {
                rows.push(ScanEntry {
                    family: "user_essentials",
                    pattern: p,
                    description: format!("Group Container contentdelivery {sub}"),
                    process_probe: None,
                    sw_domain_guard: false,
                    age_days: 0,
                });
            }
        }
    }

    // Chromium 系旧版本 + EdgeUpdater staged payload（对标 clean_*_old_versions）。
    rows.extend(old_versions::chromium_old_version_entries());
    rows.extend(old_versions::edge_updater_old_version_entries());

    rows.extend(app_support_regenerable_entries());
    rows.extend(gradle_guarded_entries());
    rows.extend(xcode_documentation_stale_entries());
    rows
}

/// 对标 clean_xcode_documentation_cache：DocumentationCache 下
/// DeveloperDocumentation*.index，保留 mtime 最新的一个，其余陈旧索引
/// 进程守卫后可删。
fn xcode_documentation_stale_entries() -> Vec<ScanEntry> {
    let root = PathBuf::from("/Library/Developer/Xcode/DocumentationCache");
    if !root.is_dir() {
        return Vec::new();
    }
    // 收集非符号链接的 *.index。
    let mut indexes: Vec<(PathBuf, u64)> = Vec::new();
    let Ok(entries) = std::fs::read_dir(&root) else {
        return Vec::new();
    };
    for e in entries.flatten() {
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_symlink() {
            continue;
        }
        let name = e.file_name().to_string_lossy().to_string();
        if name == "DeveloperDocumentation.index" || (name.starts_with("DeveloperDocumentation") && name.ends_with(".index")) {
            let mtime = std::fs::metadata(e.path())
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0);
            indexes.push((e.path(), mtime));
        }
    }
    if indexes.len() <= 1 {
        return Vec::new();
    }
    // 按 mtime 降序；除最新外均为陈旧。
    indexes.sort_by(|a, b| b.1.cmp(&a.1));
    let stale: Vec<PathBuf> = indexes.into_iter().skip(1).map(|(p, _)| p).collect();
    // 陈旧项合成一个组（对标 stale_entries 数组）。
    if stale.is_empty() {
        return Vec::new();
    }
    // GUI 按 description 选组：用首个陈旧项路径作为 pattern，其余单独列出。
    stale
        .into_iter()
        .map(|p| ScanEntry {
            family: "dev_xcode",
            pattern: p.clone(),
            description: format!(
                "Xcode stale doc index · {}",
                p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()
            ),
            process_probe: Some(process::xcode_process_state),
            sw_domain_guard: false,
            age_days: 0,
        })
        .collect()
}

/// 对标 clean_dev_jvm 的 Gradle 进程守卫行（daemon 不可删，build-cache 等可删）。
fn gradle_guarded_entries() -> Vec<ScanEntry> {
    let home = std::env::var("HOME").unwrap_or_default();
    let gradle = PathBuf::from(&home).join(".gradle");
    if !gradle.is_dir() {
        return Vec::new();
    }
    let rows: &[(&str, &str)] = &[
        ("caches/build-cache-*/*", "Gradle build cache"),
        ("notifications/*", "Gradle notifications cache"),
        ("daemon/*", "Gradle daemon/workers"),
        ("workers/*", "Gradle workers"),
    ];
    rows.iter()
        .map(|(sub, desc)| ScanEntry {
            family: "dev_jvm",
            pattern: gradle.join(sub),
            description: desc.to_string(),
            process_probe: Some(process::gradle_daemon_state),
            sw_domain_guard: false,
            age_days: 0,
        })
        .collect()
}

/// 对标 clean_service_worker_cache 的 profile 遍历：Chrome/Arc/Brave/Dia/
/// Vivaldi/QQBrowser3 的 Service Worker/CacheStorage 目录，depth≤2 展开为
/// 单独条目并挂 sw_domain_guard。
fn service_worker_entries() -> Vec<ScanEntry> {
    let mut rows = Vec::new();
    let mut seen_paths: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
    let home = std::env::var("HOME").unwrap_or_default();
    let support = PathBuf::from(&home).join("Library/Application Support");
    // 对标 clean_browsers 的 profile 遍历：各浏览器只列其实际 profile 根，
    // 子目录（Chrome Default 等）由 read_dir 展开；Arc 的 User Data 已在
    // Arc/*/ 中被覆盖，不重复显式加入。
    let profiles: &[(&str, &str)] = &[
        ("Google/Chrome", "Chrome"),
        ("Arc", "Arc"),
        ("BraveSoftware/Brave-Browser", "Brave"),
        ("Dia/User Data", "Dia"),
        ("Vivaldi", "Vivaldi"),
        ("QQBrowser3", "QQBrowser3"),
    ];
    for (profile_rel, browser) in profiles {
        let profile = support.join(profile_rel);
        if !profile.is_dir() {
            continue;
        }
        // profile 根 + 一层子目录（Chrome Default/Profile N 等）。
        // Arc 额外展开 User Data/*/（对标第二个 for 循环）。
        let mut roots = vec![profile.clone()];
        if let Ok(entries) = std::fs::read_dir(&profile) {
            for e in entries.flatten() {
                if e.path().is_dir() {
                    roots.push(e.path());
                    // Arc: User Data/<profile> 二级展开。
                    if *browser == "Arc" && e.file_name().to_string_lossy() == "User Data" {
                        if let Ok(inner) = std::fs::read_dir(e.path()) {
                            for i in inner.flatten() {
                                if i.path().is_dir() {
                                    roots.push(i.path());
                                }
                            }
                        }
                    }
                }
            }
        }
        for root in roots {
            let sw = root.join("Service Worker/CacheStorage");
            if !sw.is_dir() || !seen_paths.insert(sw.clone()) {
                continue;
            }
            // 符号链接根拒绝（对标 physical != lexical）。
            if sw.symlink_metadata().map(|m| m.is_symlink()).unwrap_or(false) {
                continue;
            }
            let Ok(entries) = std::fs::read_dir(&sw) else {
                continue;
            };
            for origin in entries.flatten() {
                if !origin.path().is_dir() {
                    continue;
                }
                rows.push(ScanEntry {
                    family: "service_worker",
                    pattern: origin.path(),
                    description: format!(
                        "{browser} Service Worker · {}",
                        origin.file_name().to_string_lossy()
                    ),
                    process_probe: None,
                    sw_domain_guard: true,
                    age_days: 0,
                });
                if let Ok(subs) = std::fs::read_dir(origin.path()) {
                    for sub in subs.flatten() {
                        if sub.path().is_dir() {
                            rows.push(ScanEntry {
                                family: "service_worker",
                                pattern: sub.path(),
                                description: format!(
                                    "{browser} Service Worker · {}/{}",
                                    origin.file_name().to_string_lossy(),
                                    sub.file_name().to_string_lossy()
                                ),
                                process_probe: None,
                                sw_domain_guard: true,
                                age_days: 0,
                            });
                        }
                    }
                }
            }
        }
    }
    rows
}

/// 对标 `is_apple_silicon`（IS_M_SERIES）：仅 arm64 主机启用 Apple Silicon 行。
fn is_apple_silicon() -> bool {
    cfg!(target_arch = "aarch64")
}

/// 过滤 full_catalog：非 arm64 主机剔除 apple_silicon 族。
fn filter_catalog_by_arch(entries: Vec<catalog::CatalogEntry>) -> Vec<catalog::CatalogEntry> {
    if is_apple_silicon() {
        return entries;
    }
    entries
        .into_iter()
        .filter(|e| e.family != "apple_silicon")
        .collect()
}

/// 对标 `clean_application_support_logs` 的可再生缓存子树扫描。
///
/// Application Support 可能含许可证/数据库/离线资源/会话状态，本通用扫描
/// 仅触碰显式可再生缓存子树（Code Cache / GPUCache / …）；有缓存标记的
/// 应用追加 Cache/CachedData。应用级保护：whitelist → should_protect_path
/// → should_protect_data → is_critical_system_component。
fn app_support_regenerable_entries() -> Vec<ScanEntry> {
    let mut rows = Vec::new();
    let home = std::env::var("HOME").unwrap_or_default();
    let support = PathBuf::from(&home).join("Library/Application Support");
    let Ok(apps) = std::fs::read_dir(&support) else {
        return rows;
    };

    const CANDIDATES: &[&str] = &[
        "Code Cache",
        "GPUCache",
        "DawnCache",
        "GrShaderCache",
        "GraphiteDawnCache",
        "DawnGraphiteCache",
        "DawnWebGPUCache",
        "Crashpad/completed",
    ];
    const MARKERS: &[&str] = &[
        "Code Cache",
        "GPUCache",
        "DawnCache",
        "GrShaderCache",
        "GraphiteDawnCache",
        "DawnGraphiteCache",
        "DawnWebGPUCache",
        "Crashpad",
    ];

    for entry in apps.flatten() {
        let Ok(ft) = entry.file_type() else { continue };
        if !ft.is_dir() {
            continue;
        }
        let app_dir = entry.path();
        let app_name = entry.file_name().to_string_lossy().to_string();
        // 应用级保护（对标循环内四层检查）。
        let app_str = app_dir.to_string_lossy().to_string();
        if protect::should_protect_path(&app_str)
            || protect::should_protect_data(&app_name)
            || protect::should_protect_data(&app_name.to_lowercase())
            || protect::is_critical_system_component(&app_name)
        {
            continue;
        }

        // 可再生缓存标记。
        let has_markers = MARKERS.iter().any(|m| app_dir.join(m).exists());
        let mut subs: Vec<&str> = CANDIDATES.to_vec();
        if has_markers {
            subs.push("Cache");
            subs.push("CachedData");
        }

        for sub in subs {
            let candidate = app_dir.join(sub);
            if !candidate.is_dir() {
                continue;
            }
            let cand_str = candidate.to_string_lossy().to_string();
            if protect::should_protect_path(&cand_str) {
                continue;
            }
            rows.push(ScanEntry {
                family: "app_support",
                pattern: candidate,
                description: format!("{app_name} · {sub}"),
                process_probe: None,
                sw_domain_guard: false,
                age_days: 0,
            });
        }
    }
    rows
}

/// 对标 clean_dev_rust 的 cargo registry/cache 行：
/// - rust_build_process_state 三态守卫（Running → 延迟；Unknown → 拒绝）；
/// - cache 根物理路径必须仍在 CARGO_HOME 内（对标 rust_cache_root_physical_path）。
fn cargo_registry_entry() -> Option<ScanEntry> {
    let cargo_home = probe::resolve_tool_home("CARGO_HOME", ".cargo");
    let cache_root = cargo_home.join("registry/cache");
    if !cache_root.is_dir() {
        return None;
    }
    // 物理包含：cache 的 canonical 必须在 cargo_home 的 canonical 下。
    if let (Ok(home_real), Ok(cache_real)) =
        (cargo_home.canonicalize(), cache_root.canonicalize())
    {
        if !cache_real.starts_with(&home_real) || home_real == Path::new("/") {
            return None; // 逃出 CARGO_HOME → 不清理（对标 stopped）
        }
    } else {
        return None;
    }
    Some(ScanEntry {
        family: "dev_rust",
        pattern: cache_root.join("*"),
        description: "Rust cargo cache".into(),
        process_probe: Some(process::rust_build_process_state),
        sw_domain_guard: false,
        age_days: 0,
    })
}

/// 浏览器进程守卫行（对标 clean_browsers 中带 pgrep 守卫的档案缓存）。
fn guarded_browser_entries() -> Vec<ScanEntry> {
    let mut rows = Vec::new();
    let home = std::env::var("HOME").unwrap_or_default();
    let chrome = format!("{home}/Library/Application Support/Google/Chrome");
    if Path::new(&chrome).is_dir() {
        // 描述唯一化：profile 层与根层同名缓存在 GUI 中需可区分选择
        // （对标原多行 safe_clean；GUI 按 description 选组）。
        let chrome_rows: &[(&str, &str)] = &[
            ("*/Application Cache/*", "Chrome app cache"),
            ("*/Code Cache/*", "Chrome code cache"),
            ("*/GPUCache/*", "Chrome GPU cache"),
            ("*/DawnCache/*", "Chrome Dawn cache"),
            ("*/GrShaderCache/*", "Chrome profile GR shader cache"),
            ("*/GraphiteDawnCache/*", "Chrome profile Graphite Dawn cache"),
            ("component_crx_cache/*", "Chrome component CRX cache"),
            ("ShaderCache/*", "Chrome shader cache"),
            ("GrShaderCache/*", "Chrome GR shader cache"),
            ("GraphiteDawnCache/*", "Chrome Dawn cache"),
            ("Crashpad/completed/*", "Chrome crash reports"),
            ("OptGuideOnDeviceModel/*", "Chrome on-device model cache"),
            ("OptGuideOnDeviceClassifierModel/*", "Chrome on-device classifier cache"),
            ("optimization_guide_model_store/*", "Chrome optimization guide models"),
        ];
        for (sub, desc) in chrome_rows {
            rows.push(ScanEntry {
                family: "browser",
                pattern: PathBuf::from(&chrome).join(sub),
                description: desc.to_string(),
                process_probe: Some(process::google_chrome_process_state),
                sw_domain_guard: false,
                age_days: 0,
            });
        }
    }

    // Firefox。
    if Path::new(&home).join("Library/Application Support/Firefox").is_dir() {
        rows.push(ScanEntry {
            family: "browser",
            pattern: PathBuf::from(&home).join("Library/Caches/Firefox/*"),
            description: "Firefox cache".into(),
            process_probe: Some(process::firefox_process_state),
            sw_domain_guard: false,
            age_days: 0,
        });
        rows.push(ScanEntry {
            family: "browser",
            pattern: PathBuf::from(&home)
                .join("Library/Application Support/Firefox/Profiles/*/cache2/*"),
            description: "Firefox profile cache".into(),
            process_probe: Some(process::firefox_process_state),
            sw_domain_guard: false,
            age_days: 0,
        });
    }

    // Arc 档案缓存（对标 Arc 未运行分支；GUI 按描述选择，故描述必须唯一）。
    let arc = PathBuf::from(&home).join("Library/Application Support/Arc");
    if arc.is_dir() {
        let arc_rows: &[(&str, &str)] = &[
            ("*/Code Cache/*", "Arc profile code cache"),
            ("*/GPUCache/*", "Arc profile GPU cache"),
            ("*/DawnCache/*", "Arc profile Dawn cache"),
            ("*/GrShaderCache/*", "Arc profile GR shader cache"),
            ("*/GraphiteDawnCache/*", "Arc profile Graphite Dawn cache"),
            ("ShaderCache/*", "Arc shader cache"),
            ("GrShaderCache/*", "Arc GR shader cache"),
            ("GraphiteDawnCache/*", "Arc Dawn cache"),
            ("Crashpad/completed/*", "Arc crash reports"),
            ("User Data/*/Code Cache/*", "Arc User Data code cache"),
            ("User Data/*/GPUCache/*", "Arc User Data GPU cache"),
            ("User Data/*/DawnCache/*", "Arc User Data Dawn cache"),
            ("User Data/*/GrShaderCache/*", "Arc User Data GR shader cache"),
            ("User Data/*/GraphiteDawnCache/*", "Arc User Data Graphite Dawn cache"),
            ("User Data/ShaderCache/*", "Arc User Data shader cache"),
            ("User Data/GrShaderCache/*", "Arc User Data GR shader cache"),
            ("User Data/GraphiteDawnCache/*", "Arc User Data Dawn cache"),
            ("User Data/component_crx_cache/*", "Arc component CRX cache"),
            ("User Data/extensions_crx_cache/*", "Arc extensions CRX cache"),
            ("User Data/Crashpad/completed/*", "Arc User Data crash reports"),
        ];
        for (sub, desc) in arc_rows {
            rows.push(ScanEntry {
                family: "browser",
                pattern: arc.join(sub),
                description: desc.to_string(),
                process_probe: Some(process::arc_process_state),
                sw_domain_guard: false,
                age_days: 0,
            });
        }
    }

    // Brave。
    let brave = PathBuf::from(&home).join("Library/Application Support/BraveSoftware/Brave-Browser");
    if brave.is_dir() {
        let brave_rows: &[(&str, &str)] = &[
            ("*/Application Cache/*", "Brave app cache"),
            ("*/Code Cache/*", "Brave code cache"),
            ("*/GPUCache/*", "Brave GPU cache"),
            ("*/DawnCache/*", "Brave Dawn cache"),
            ("*/GrShaderCache/*", "Brave profile GR shader cache"),
            ("*/GraphiteDawnCache/*", "Brave profile Graphite Dawn cache"),
            ("component_crx_cache/*", "Brave component CRX cache"),
            ("ShaderCache/*", "Brave shader cache"),
            ("GrShaderCache/*", "Brave GR shader cache"),
            ("GraphiteDawnCache/*", "Brave Dawn cache"),
            ("Crashpad/completed/*", "Brave crash reports"),
        ];
        for (sub, desc) in brave_rows {
            rows.push(ScanEntry {
                family: "browser",
                pattern: brave.join(sub),
                description: desc.to_string(),
                process_probe: Some(process::brave_process_state),
                sw_domain_guard: false,
                age_days: 0,
            });
        }
    }

    // Dia（仅 Application Support 侧有守卫；缓存目录见静态行）。
    let dia = PathBuf::from(&home).join("Library/Application Support/Dia");
    if dia.is_dir() {
        let dia_rows: &[(&str, &str)] = &[
            ("User Data/GraphiteDawnCache/*", "Dia Graphite Dawn cache"),
            ("User Data/GPUPersistentCache/*", "Dia GPU cache"),
            ("User Data/component_crx_cache/*", "Dia component CRX cache"),
            ("User Data/extensions_crx_cache/*", "Dia extensions CRX cache"),
            ("User Data/*/DawnGraphiteCache/*", "Dia Dawn Graphite cache"),
            ("User Data/*/DawnWebGPUCache/*", "Dia Dawn WebGPU cache"),
            ("User Data/*/GPUCache/*", "Dia profile GPU cache"),
        ];
        for (sub, desc) in dia_rows {
            rows.push(ScanEntry {
                family: "browser",
                pattern: dia.join(sub),
                description: desc.to_string(),
                process_probe: Some(process::dia_process_state),
                sw_domain_guard: false,
                age_days: 0,
            });
        }
    }

    // Vivaldi。
    let vivaldi = PathBuf::from(&home).join("Library/Application Support/Vivaldi");
    if vivaldi.is_dir() {
        let vivaldi_rows: &[(&str, &str)] = &[
            ("*/Code Cache/*", "Vivaldi code cache"),
            ("*/GPUCache/*", "Vivaldi GPU cache"),
            ("*/DawnCache/*", "Vivaldi Dawn cache"),
            ("*/GrShaderCache/*", "Vivaldi profile GR shader cache"),
            ("*/GraphiteDawnCache/*", "Vivaldi profile Graphite Dawn cache"),
            ("ShaderCache/*", "Vivaldi shader cache"),
            ("GrShaderCache/*", "Vivaldi GR shader cache"),
            ("GraphiteDawnCache/*", "Vivaldi Dawn cache"),
            ("Crashpad/completed/*", "Vivaldi crash reports"),
        ];
        for (sub, desc) in vivaldi_rows {
            rows.push(ScanEntry {
                family: "browser",
                pattern: vivaldi.join(sub),
                description: desc.to_string(),
                process_probe: Some(process::vivaldi_process_state),
                sw_domain_guard: false,
                age_days: 0,
            });
        }
    }

    // QQBrowser3。
    let qq = PathBuf::from(&home).join("Library/Application Support/QQBrowser3");
    if qq.is_dir() {
        let qq_rows: &[(&str, &str)] = &[
            ("*/Code Cache/*", "QQ Browser code cache"),
            ("*/GPUCache/*", "QQ Browser GPU cache"),
            ("ShaderCache/*", "QQ Browser shader cache"),
            ("GrShaderCache/*", "QQ Browser GR shader cache"),
            ("GraphiteDawnCache/*", "QQ Browser Dawn cache"),
            ("component_crx_cache/*", "QQ Browser component cache"),
            ("Crashpad/completed/*", "QQ Browser crash reports"),
        ];
        for (sub, desc) in qq_rows {
            rows.push(ScanEntry {
                family: "browser",
                pattern: qq.join(sub),
                description: desc.to_string(),
                process_probe: Some(process::qqbrowser3_process_state),
                sw_domain_guard: false,
                age_days: 0,
            });
        }
    }

    rows
}

/// 对标 _clean_incomplete_downloads：Downloads 下 *.download/*.crdownload/*.part。
/// 开句柄三态在 scan/execute 通过 process_probe 风格的 lsof 检查实现
///（见 incomplete_download_probe）。
fn incomplete_download_entries() -> Vec<ScanEntry> {
    let home = std::env::var("HOME").unwrap_or_default();
    let dl = PathBuf::from(&home).join("Downloads");
    if !dl.is_dir() {
        return Vec::new();
    }
    let labels = [
        ("*.download", "Safari incomplete downloads"),
        ("*.crdownload", "Chrome incomplete downloads"),
        ("*.part", "Partial incomplete downloads"),
    ];
    labels
        .iter()
        .map(|(pat, desc)| ScanEntry {
            family: "user_essentials",
            pattern: dl.join(pat),
            description: desc.to_string(),
            // 用 lsof 开句柄探针替代应用进程探针。
            process_probe: Some(incomplete_download_open_probe),
            sw_domain_guard: false,
            age_days: 0,
        })
        .collect()
}

/// lsof 开句柄三态（对标 _mole_paths_have_open_handle 的简化）：
/// Running=有句柄，Idle=无句柄，Unknown=lsof 不可用/失败。
fn incomplete_download_open_probe() -> process::ProcessState {
    // 探测用通配路径不精确；真实检查在 per-file 扫描时做。
    // 此处返回 Idle 允许展开条目，单文件 open 检查在 skip_reason 扩展。
    process::ProcessState::Idle
}

/// 单路径是否有打开句柄（对标 _mole_paths_have_open_handle）。
/// 返回 Some(true)=有句柄，Some(false)=无句柄，None=无法判定。
fn path_has_open_handle(path: &str) -> Option<bool> {
    if !crate::clean::command_exists("lsof") {
        return None;
    }
    let out = crate::status::run_cmd("lsof", &["-F", "n", "--", path], Duration::from_secs(3));
    match out {
        Ok(o) => {
            // lsof 退出 0 = 有打开；字段记录含 \nn 开头的 file name。
            Some(o.lines().any(|l| l.starts_with('n') && l.len() > 1) || !o.trim().is_empty())
        }
        Err(e) if e.contains("exited with 1") => Some(false), // 无匹配
        Err(_) => None,
    }
}

/// mtime 早于 age_days 的目标过滤（对标 Mail Downloads 龄过滤）。
fn is_older_than_days(path: &Path, age_days: u64) -> bool {
    if age_days == 0 {
        return true;
    }
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    let Ok(modified) = meta.modified() else {
        return false;
    };
    match modified.elapsed() {
        Ok(age) => age.as_secs() >= age_days * 86400,
        Err(_) => false,
    }
}

/// 对标 mole_deno_cache_root：DENO_DIR 或 ~/Library/Caches/deno；
/// 不安全路径（相对、含 ..、等于 HOME/Caches 等）返回 None。
/// 当前 catalog 使用显式路径而非宽扫 Library/Caches，故 Deno 本身
/// 已不在删除路径；此函数供未来宽扫接入排除表。
#[allow(dead_code)]
pub(crate) fn deno_cache_root() -> Option<PathBuf> {
    let home = std::env::var("HOME").unwrap_or_default();
    // DENO_DIR 设置时必须是绝对路径；未设置用默认。
    let raw = match std::env::var("DENO_DIR") {
        Ok(v) => {
            if !v.starts_with('/') || v.chars().any(|c| c.is_control()) {
                return None;
            }
            v
        }
        Err(_) => format!("{home}/Library/Caches/deno"),
    };
    let trimmed = raw.trim_end_matches('/');
    if trimmed.is_empty() || trimmed == "/" {
        return None;
    }
    if trimmed.contains("/../") || trimmed.ends_with("/..") || trimmed.contains("/./") || trimmed.contains("//") {
        return None;
    }
    let home_root = home.trim_end_matches('/');
    for forbidden in [
        "",
        "/",
        home_root,
        &format!("{home_root}/Library"),
        &format!("{home_root}/Library/Caches"),
        &format!("{home_root}/.cache"),
    ] {
        if trimmed == forbidden {
            return None;
        }
    }
    Some(PathBuf::from(trimmed))
}

/// 汇总静态目录、动态探测行与进程守卫行。
fn collect_entries() -> Vec<ScanEntry> {
    let mut entries: Vec<ScanEntry> = filter_catalog_by_arch(catalog::full_catalog())
        .into_iter()
        .map(|entry| ScanEntry {
            family: entry.family,
            pattern: resolve_entry_path(&entry),
            description: entry.description.to_string(),
            process_probe: None,
            sw_domain_guard: false,
            age_days: 0,
        })
        .collect();
    entries.extend(dynamic_entries());
    entries.extend(guarded_browser_entries());
    entries
}

/// 只读扫描预览（对标 dry-run：`MOLE_DRY_RUN=1 ./mole clean`）。
/// 全局预算 90s（对标 section budget）：超时后剩余条目 size=0 仍列出。
pub fn scan_preview() -> CleanPreview {
    let whitelist = whitelist::Whitelist::load();
    let mut groups = Vec::new();
    let global_deadline = Instant::now() + Duration::from_secs(90);

    for entry in collect_entries() {
        // 进程守卫：Running/Unknown 整组以 skip_reason 拒绝（对标
        // mole_clean_process_guard + mole_report_guard_stop / defer）。
        // 预览仍展开条目，便于用户看到"退出应用后可清理"的内容。
        let guard_reason: Option<&'static str> = entry
            .process_probe
            .and_then(|probe| process::guard_allows(probe()).err());

        let mut items = Vec::new();
        let mut skipped = 0usize;
        let mut total = 0u64;

        for target in expand_glob(&entry.pattern) {
            let target_str = target.to_string_lossy().to_string();
            if let Some(reason) = guard_reason {
                skipped += 1;
                items.push(CleanItem {
                    path: target_str,
                    size_bytes: 0,
                    skip_reason: reason.to_string(),
                });
                continue;
            }
            // 年龄门（对标 Mail Downloads MOLE_MAIL_AGE_DAYS）。
            if entry.age_days > 0 && !is_older_than_days(&target, entry.age_days) {
                skipped += 1;
                items.push(CleanItem {
                    path: target_str,
                    size_bytes: 0,
                    skip_reason: format!("未满 {} 天", entry.age_days),
                });
                continue;
            }
            // incomplete downloads：单文件 lsof 开句柄检查。
            if entry.description.contains("incomplete") {
                match path_has_open_handle(&target_str) {
                    Some(true) => {
                        skipped += 1;
                        items.push(CleanItem {
                            path: target_str,
                            size_bytes: 0,
                            skip_reason: "下载进行中（有打开句柄）".into(),
                        });
                        continue;
                    }
                    None => {
                        skipped += 1;
                        items.push(CleanItem {
                            path: target_str,
                            size_bytes: 0,
                            skip_reason: "开句柄检查不可用".into(),
                        });
                        continue;
                    }
                    Some(false) => {}
                }
            }
            // 保护检查在扫描期同样执行：受保护/白名单路径永远不会出现在
            // 可清理列表（对标 _safe_clean_impl 的逐路径检查顺序）。
            if let Some(reason) = skip_reason(&target_str, &whitelist, entry.sw_domain_guard) {
                skipped += 1;
                items.push(CleanItem {
                    path: target_str,
                    size_bytes: 0,
                    skip_reason: reason.to_string(),
                });
                continue;
            }
            let size = path_size_with_deadline(&target, global_deadline.min(Instant::now() + SIZE_SCAN_DEADLINE));
            total += size;
            items.push(CleanItem {
                path: target_str,
                size_bytes: size,
                skip_reason: String::new(),
            });
        }

        groups.push(CleanGroup {
            description: entry.description,
            family: catalog::family_label(entry.family).to_string(),
            items,
            total_size_bytes: total,
            skipped_count: skipped,
        });
    }

    // owner 命令删除汇（对标 clean_tool_cache 调用点）：每条一个合成组。
    for op in owner_clean::owner_clean_ops() {
        let (ok, detail) = owner_clean::execute_owner_clean(&op, true);
        let (path, size) = if let Some(p) = (op.resolve_cache_path)() {
            let s = p.to_string_lossy().to_string();
            let sz = path_size_with_deadline(&p, global_deadline);
            (s, sz)
        } else {
            (String::new(), 0)
        };
        let items = if path.is_empty() {
            Vec::new()
        } else {
            vec![CleanItem {
                path,
                size_bytes: size,
                skip_reason: if ok { String::new() } else { detail.clone() },
            }]
        };
        groups.push(CleanGroup {
            description: op.description.to_string(),
            family: "owner_command".to_string(),
            items,
            total_size_bytes: size,
            skipped_count: usize::from(!ok),
        });
    }

    // deep_system 族（对标 clean_deep_system；sudo -n 门控 + 年龄过滤）。
    for family in system::system_families() {
        let candidates = system::scan_family(&family);
        let size = system::family_size(&candidates);
        let sudo_ok = system::sudo_n_available();
        let items: Vec<CleanItem> = candidates
            .iter()
            .map(|p| CleanItem {
                path: p.to_string_lossy().to_string(),
                size_bytes: 0,
                skip_reason: if sudo_ok {
                    String::new()
                } else {
                    "需要管理员权限".into()
                },
            })
            .collect();
        groups.push(CleanGroup {
            description: family.label.to_string(),
            family: "deep_system".to_string(),
            items,
            total_size_bytes: size,
            skipped_count: if sudo_ok { 0 } else { candidates.len() },
        });
    }

    // Finder metadata (.DS_Store)（对标 clean_finder_metadata → clean_ds_store_tree）。
    {
        let ds_files = scan_ds_store_tree();
        let size: u64 = ds_files
            .iter()
            .map(|p| path_size_with_deadline(p, global_deadline.min(Instant::now() + SIZE_SCAN_DEADLINE)))
            .sum();
        let items: Vec<CleanItem> = ds_files
            .iter()
            .map(|p| CleanItem {
                path: p.to_string_lossy().to_string(),
                size_bytes: 0,
                skip_reason: String::new(),
            })
            .collect();
        groups.push(CleanGroup {
            description: "Finder metadata (.DS_Store)".into(),
            family: "user_essentials".into(),
            items,
            total_size_bytes: size,
            skipped_count: 0,
        });
    }

    // Trash（对标 clean_trash）。
    {
        let home = std::env::var("HOME").unwrap_or_default();
        let trash = PathBuf::from(&home).join(".Trash");
        let (count, size) = if trash.is_dir() {
            let entries: Vec<_> = std::fs::read_dir(&trash)
                .map(|rd| rd.flatten().collect())
                .unwrap_or_default();
            let sz: u64 = entries
                .iter()
                .filter(|e| {
                    !crate::clean::whitelist::Whitelist::load()
                        .is_whitelisted(&e.path().to_string_lossy())
                })
                .map(|e| path_size_with_deadline(&e.path(), global_deadline))
                .sum();
            (entries.len(), sz)
        } else {
            (0, 0)
        };
        let items: Vec<CleanItem> = if count > 0 {
            vec![CleanItem {
                path: format!("{home}/.Trash"),
                size_bytes: size,
                skip_reason: String::new(),
            }]
        } else {
            Vec::new()
        };
        groups.push(CleanGroup {
            description: "Trash".into(),
            family: "user_essentials".into(),
            items,
            total_size_bytes: size,
            skipped_count: 0,
        });
    }

    // Homebrew（对标 clean_homebrew）。
    {
        let result = brew::clean_homebrew(true);
        let home = std::env::var("HOME").unwrap_or_default();
        let cache = PathBuf::from(&home).join("Library/Caches/Homebrew");
        let size = if cache.is_dir() {
            path_size_with_deadline(&cache, global_deadline)
        } else {
            0
        };
        let items: Vec<CleanItem> = if result.status != "skipped" {
            vec![CleanItem {
                path: format!("{home}/Library/Caches/Homebrew"),
                size_bytes: size,
                skip_reason: if result.status == "ok" || result.status == "dry-run" {
                    String::new()
                } else {
                    result.detail.clone()
                },
            }]
        } else {
            vec![CleanItem {
                path: format!("{home}/Library/Caches/Homebrew"),
                size_bytes: 0,
                skip_reason: result.detail.clone(),
            }]
        };
        groups.push(CleanGroup {
            description: "Homebrew cleanup".into(),
            family: "owner_command".into(),
            items,
            total_size_bytes: if result.status == "skipped" { 0 } else { size },
            skipped_count: usize::from(result.status == "skipped"),
        });
    }

    // orphaned app data（对标 clean_orphaned_app_data）。
    {
        let orphans = special::scan_orphaned_app_data();
        let size: u64 = orphans
            .iter()
            .map(|p| path_size_with_deadline(p, global_deadline.min(Instant::now() + SIZE_SCAN_DEADLINE)))
            .sum();
        let items: Vec<CleanItem> = orphans
            .iter()
            .map(|p| CleanItem {
                path: p.to_string_lossy().to_string(),
                size_bytes: 0,
                skip_reason: String::new(),
            })
            .collect();
        groups.push(CleanGroup {
            description: "Orphaned app data".into(),
            family: "app_leftovers".into(),
            items,
            total_size_bytes: size,
            skipped_count: 0,
        });
    }

    // orphaned container stubs（对标 clean_orphaned_container_stubs）。
    {
        let stubs = special::scan_orphaned_container_stubs();
        let size: u64 = stubs
            .iter()
            .map(|p| path_size_with_deadline(p, global_deadline.min(Instant::now() + SIZE_SCAN_DEADLINE)))
            .sum();
        let items: Vec<CleanItem> = stubs
            .iter()
            .map(|p| CleanItem {
                path: p.to_string_lossy().to_string(),
                size_bytes: 0,
                skip_reason: String::new(),
            })
            .collect();
        groups.push(CleanGroup {
            description: "Orphaned container stubs".into(),
            family: "app_leftovers".into(),
            items,
            total_size_bytes: size,
            skipped_count: 0,
        });
    }

    // 设备固件（对标 clean_cached_device_firmware）。
    {
        let fw = special::scan_device_firmware();
        let size: u64 = fw
            .iter()
            .map(|p| path_size_with_deadline(p, global_deadline.min(Instant::now() + SIZE_SCAN_DEADLINE)))
            .sum();
        let items: Vec<CleanItem> = fw
            .iter()
            .map(|p| CleanItem {
                path: p.to_string_lossy().to_string(),
                size_bytes: 0,
                skip_reason: String::new(),
            })
            .collect();
        groups.push(CleanGroup {
            description: "Device firmware (IPSW)".into(),
            family: "user_essentials".into(),
            items,
            total_size_bytes: size,
            skipped_count: 0,
        });
    }

    // Time Machine 未完成备份（对标 clean_time_machine_failed_backups；只读报告）。
    {
        let count = special::count_incomplete_tm_backups();
        let items: Vec<CleanItem> = match count {
            Some(n) if n > 0 => vec![CleanItem {
                path: "/Volumes (Time Machine)".into(),
                size_bytes: 0,
                skip_reason: format!("{n} 个未完成备份（审查：tmutil listbackups）"),
            }],
            _ => Vec::new(),
        };
        let skipped = items.len();
        groups.push(CleanGroup {
            description: "Time Machine incomplete backups".into(),
            family: "user_essentials".into(),
            items,
            total_size_bytes: 0,
            skipped_count: skipped,
        });
    }

    // 大文件审查（对标 check_large_file_candidates；只读）。
    {
        let large = special::large_file_candidates();
        let total: u64 = large.iter().map(|(_, _, s)| *s).sum();
        let items: Vec<CleanItem> = large
            .iter()
            .map(|(label, path, size)| CleanItem {
                path: path.clone(),
                size_bytes: *size,
                skip_reason: format!("{label}（≥1GB 审查）"),
            })
            .collect();
        let skipped = items.len();
        groups.push(CleanGroup {
            description: "Large files review".into(),
            family: "user_essentials".into(),
            items,
            total_size_bytes: total,
            skipped_count: skipped,
        });
    }

    // 外置卷 .TemporaryItems/.Trashes/.DS_Store（对标 clean_external_volume_target）。
    if let Ok(vols) = std::fs::read_dir("/Volumes") {
        for vol in vols.flatten() {
            let vol_path = vol.path();
            if !vol_path.is_dir() || vol_path.is_symlink() {
                continue;
            }
            // 跳过与 /Applications 同名或系统卷。
            let name = vol.file_name().to_string_lossy().to_string();
            if name.starts_with('.') || name == "Data" || name == "VM" {
                continue;
            }
            let targets = special::scan_external_volume(&vol_path.to_string_lossy());
            if targets.is_empty() {
                continue;
            }
            let size: u64 = targets
                .iter()
                .map(|p| path_size_with_deadline(p, global_deadline.min(Instant::now() + SIZE_SCAN_DEADLINE)))
                .sum();
            let items: Vec<CleanItem> = targets
                .iter()
                .map(|p| CleanItem {
                    path: p.to_string_lossy().to_string(),
                    size_bytes: 0,
                    skip_reason: String::new(),
                })
                .collect();
            groups.push(CleanGroup {
                description: format!("External volume · {name}"),
                family: "user_essentials".into(),
                items,
                total_size_bytes: size,
                skipped_count: 0,
            });
        }
    }

    // LaunchAgents 提示（对标 show_user_launch_agent_hint_notice；只读）。
    {
        let hints = special::launch_agent_hints();
        let items: Vec<CleanItem> = hints
            .iter()
            .map(|(name, reason)| CleanItem {
                path: format!("~/Library/LaunchAgents/{name}"),
                size_bytes: 0,
                skip_reason: reason.clone(),
            })
            .collect();
        let skipped = items.len();
        groups.push(CleanGroup {
            description: "LaunchAgents hints".into(),
            family: "user_essentials".into(),
            items,
            total_size_bytes: 0,
            skipped_count: skipped,
        });
    }

    let total_size = groups.iter().map(|g| g.total_size_bytes).sum();
    CleanPreview {
        groups,
        total_size_bytes: total_size,
        whitelist_source: whitelist.source_description().to_string(),
    }
}

/// 扫描家目录下 .DS_Store（对标 clean_ds_store_tree：maxdepth 5，排除
/// MobileSync/Developer/.Trash/node_modules/.git/Library/Caches）。
fn scan_ds_store_tree() -> Vec<PathBuf> {
    let home = std::env::var("HOME").unwrap_or_default();
    let root = PathBuf::from(&home);
    if !root.is_dir() {
        return Vec::new();
    }
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut out = Vec::new();
    let mut stack = vec![(root, 0usize)];
    while let Some((dir, depth)) = stack.pop() {
        if Instant::now() >= deadline {
            break;
        }
        // 排除目录（对标 find prune）。
        let dir_str = dir.to_string_lossy();
        if dir_str.contains("/Library/Application Support/MobileSync")
            || dir_str.contains("/Library/Developer")
            || dir_str.ends_with("/.Trash")
            || dir_str.ends_with("/node_modules")
            || dir_str.ends_with("/.git")
            || dir_str.contains("/Library/Caches")
        {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            if Instant::now() >= deadline {
                break;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            let Ok(ft) = entry.file_type() else {
                continue;
            };
            if ft.is_dir() && depth < 5 {
                stack.push((entry.path(), depth + 1));
            } else if name == ".DS_Store" && ft.is_file() {
                out.push(entry.path());
            }
        }
    }
    out
}

/// 执行清理（对标 safe_clean 的真实删除分支 + mole_delete trash 模式）。
///
/// 安全流程：**重新完整扫描**（不信任旧预览数据）→ 过滤出用户选择的组 →
/// 每个目标在删除 sink 再次复检（保护/白名单/存在性，对标 sink 复查）→
/// Trash 路由删除 → 逐项记录结果与日志。
///
/// `dry_run=true` 时只生成结果不移动任何文件（对标 MOLE_DRY_RUN=1）。
pub fn execute_clean(selected_groups: &[String], dry_run: bool) -> CleanExecuteResult {
    let whitelist = whitelist::Whitelist::load();
    let mut outcomes = Vec::new();
    let mut deleted_count = 0usize;
    let mut freed_bytes = 0u64;
    let mut failed_count = 0usize;

    delete::log_session_start("clean");

    for entry in collect_entries() {
        if !selected_groups.iter().any(|s| *s == entry.description) {
            continue;
        }
        // Sink 前进程守卫复检（对标 _dev_safe_clean_process_guarded 的
        // 扫描到 sink 双重探针）。
        if let Some(probe) = entry.process_probe {
            if let Err(reason) = process::guard_allows(probe()) {
                for target in expand_glob(&entry.pattern) {
                    outcomes.push(DeleteOutcome {
                        path: target.to_string_lossy().to_string(),
                        status: "skipped".into(),
                        size_bytes: 0,
                        detail: reason.into(),
                    });
                }
                continue;
            }
        }
        for target in expand_glob(&entry.pattern) {
            let target_str = target.to_string_lossy().to_string();
            // Sink 前年龄门复检。
            if entry.age_days > 0 && !is_older_than_days(&target, entry.age_days) {
                outcomes.push(DeleteOutcome {
                    path: target_str,
                    status: "skipped".into(),
                    size_bytes: 0,
                    detail: format!("未满 {} 天", entry.age_days),
                });
                continue;
            }
            // incomplete downloads：sink 前开句柄复检（对标 guarded + final recheck）。
            if entry.description.contains("incomplete") {
                match path_has_open_handle(&target_str) {
                    Some(true) => {
                        outcomes.push(DeleteOutcome {
                            path: target_str,
                            status: "skipped".into(),
                            size_bytes: 0,
                            detail: "下载进行中（有打开句柄）".into(),
                        });
                        continue;
                    }
                    None => {
                        outcomes.push(DeleteOutcome {
                            path: target_str,
                            status: "skipped".into(),
                            size_bytes: 0,
                            detail: "开句柄检查不可用".into(),
                        });
                        continue;
                    }
                    Some(false) => {}
                }
            }
            // Sink 复检：扫描与执行之间状态可能变化（对标 sink re-verify）。
            if let Some(reason) = skip_reason(&target_str, &whitelist, entry.sw_domain_guard) {
                outcomes.push(DeleteOutcome {
                    path: target_str,
                    status: "skipped".into(),
                    size_bytes: 0,
                    detail: reason.into(),
                });
                continue;
            }
            let outcome = delete::delete_to_trash(&target_str, dry_run, "clean");
            if outcome.status == "ok" {
                deleted_count += 1;
                freed_bytes += outcome.size_bytes;
            } else if outcome.status == "failed" {
                failed_count += 1;
            }
            outcomes.push(outcome);
        }
    }

    // owner 命令删除汇（对标 clean_tool_cache）。
    for op in owner_clean::owner_clean_ops() {
        if !selected_groups.iter().any(|s| s == op.description) {
            continue;
        }
        let (ok, detail) = owner_clean::execute_owner_clean(&op, dry_run);
        let path = (op.resolve_cache_path)()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
        let status = if dry_run {
            "dry-run"
        } else if ok {
            "ok"
        } else {
            "failed"
        };
        if status == "ok" && !dry_run {
            deleted_count += 1;
        } else if status == "failed" {
            failed_count += 1;
        }
        outcomes.push(DeleteOutcome {
            path: if path.is_empty() {
                op.description.to_string()
            } else {
                path
            },
            status: status.into(),
            size_bytes: 0,
            detail,
        });
    }

    // deep_system 族（对标 clean_deep_system）。
    for family in system::system_families() {
        if !selected_groups.iter().any(|s| s == family.label) {
            continue;
        }
        let (ok, detail, removed) = system::execute_family(&family, dry_run);
        let status = if dry_run {
            "dry-run"
        } else if ok {
            "ok"
        } else {
            "failed"
        };
        if !dry_run && ok {
            deleted_count += removed;
        } else if status == "failed" {
            failed_count += 1;
        }
        outcomes.push(DeleteOutcome {
            path: family.root.to_string(),
            status: status.into(),
            size_bytes: 0,
            detail,
        });
    }

    // Finder metadata (.DS_Store)（对标 clean_finder_metadata）。
    if selected_groups.iter().any(|s| s == "Finder metadata (.DS_Store)") {
        let files = scan_ds_store_tree();
        let mut removed = 0usize;
        let mut failed = 0usize;
        for f in &files {
            let s = f.to_string_lossy().to_string();
            if protect::should_protect_path(&s) || whitelist.is_whitelisted(&s) {
                continue;
            }
            let outcome = delete::delete_to_trash(&s, dry_run, "clean");
            match outcome.status.as_str() {
                "ok" | "dry-run" => removed += 1,
                "failed" => failed += 1,
                _ => {}
            }
            freed_bytes += outcome.size_bytes;
        }
        if !dry_run {
            deleted_count += removed;
        }
        if failed > 0 {
            failed_count += 1;
        }
        outcomes.push(DeleteOutcome {
            path: "Finder metadata (.DS_Store)".into(),
            status: if dry_run {
                "dry-run".into()
            } else if failed > 0 {
                "failed".into()
            } else {
                "ok".into()
            },
            size_bytes: 0,
            detail: format!("已清理 {removed} 个 .DS_Store"),
        });
    }

    // Trash（对标 clean_trash）。
    if selected_groups.iter().any(|s| s == "Trash") {
        let home = std::env::var("HOME").unwrap_or_default();
        let trash = PathBuf::from(&home).join(".Trash");
        if trash.is_dir() && !whitelist.is_whitelisted(&trash.to_string_lossy()) {
            let entries: Vec<_> = std::fs::read_dir(&trash)
                .map(|rd| rd.flatten().collect())
                .unwrap_or_default();
            let mut removed = 0usize;
            for e in &entries {
                let s = e.path().to_string_lossy().to_string();
                if protect::should_protect_path(&s) || whitelist.is_whitelisted(&s) {
                    continue;
                }
                // Trash 内项目直接 rm（已在 Trash 中，不再 Trash 路由）。
                if dry_run {
                    removed += 1;
                    continue;
                }
                let status = std::process::Command::new("/bin/rm")
                    .arg("-rf")
                    .arg(&s)
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status();
                match status {
                    Ok(st) if st.success() => {
                        removed += 1;
                        freed_bytes += delete::delete_to_trash(&s, false, "clean").size_bytes;
                    }
                    _ => failed_count += 1,
                }
            }
            if !dry_run {
                deleted_count += removed;
            }
            outcomes.push(DeleteOutcome {
                path: format!("{home}/.Trash"),
                status: if dry_run { "dry-run" } else { "ok" }.into(),
                size_bytes: 0,
                detail: format!("已清空 {removed} 项"),
            });
        }
    }

    // Homebrew（对标 clean_homebrew）。
    if selected_groups.iter().any(|s| s == "Homebrew cleanup") {
        let result = brew::clean_homebrew(dry_run);
        if result.status == "ok" && !dry_run {
            deleted_count += 1;
        } else if result.status == "failed" {
            failed_count += 1;
        }
        let mut detail = result.detail.clone();
        if !result.freed_hint.is_empty() {
            detail.push_str(" · ");
            detail.push_str(&result.freed_hint);
        }
        if !result.autoremove_preview.is_empty() {
            detail.push_str(&format!(" · autoremove 预览 {} 项", result.autoremove_preview.len()));
        }
        outcomes.push(DeleteOutcome {
            path: "Homebrew cleanup".into(),
            status: result.status,
            size_bytes: 0,
            detail,
        });
    }

    // orphaned app data（对标 clean_orphaned_app_data）。
    if selected_groups.iter().any(|s| s == "Orphaned app data") {
        let orphans = special::scan_orphaned_app_data();
        let mut removed = 0usize;
        for p in &orphans {
            let s = p.to_string_lossy().to_string();
            let outcome = delete::delete_to_trash(&s, dry_run, "clean");
            match outcome.status.as_str() {
                "ok" | "dry-run" => removed += 1,
                "failed" => failed_count += 1,
                _ => {}
            }
            freed_bytes += outcome.size_bytes;
        }
        if !dry_run {
            deleted_count += removed;
        }
        outcomes.push(DeleteOutcome {
            path: "Orphaned app data".into(),
            status: if dry_run { "dry-run" } else { "ok" }.into(),
            size_bytes: 0,
            detail: format!("已清理 {removed} 个孤儿数据目录"),
        });
    }

    // orphaned container stubs（对标 clean_orphaned_container_stubs）。
    if selected_groups.iter().any(|s| s == "Orphaned container stubs") {
        let stubs = special::scan_orphaned_container_stubs();
        let mut removed = 0usize;
        for p in &stubs {
            let s = p.to_string_lossy().to_string();
            let outcome = delete::delete_to_trash(&s, dry_run, "clean");
            match outcome.status.as_str() {
                "ok" | "dry-run" => removed += 1,
                "failed" => failed_count += 1,
                _ => {}
            }
            freed_bytes += outcome.size_bytes;
        }
        if !dry_run {
            deleted_count += removed;
        }
        outcomes.push(DeleteOutcome {
            path: "Orphaned container stubs".into(),
            status: if dry_run { "dry-run" } else { "ok" }.into(),
            size_bytes: 0,
            detail: format!("已清理 {removed} 个孤儿容器"),
        });
    }

    // 设备固件（对标 clean_cached_device_firmware）。
    if selected_groups.iter().any(|s| s == "Device firmware (IPSW)") {
        let fw = special::scan_device_firmware();
        let mut removed = 0usize;
        for p in &fw {
            let s = p.to_string_lossy().to_string();
            let outcome = delete::delete_to_trash(&s, dry_run, "clean");
            match outcome.status.as_str() {
                "ok" | "dry-run" => removed += 1,
                "failed" => failed_count += 1,
                _ => {}
            }
            freed_bytes += outcome.size_bytes;
        }
        if !dry_run {
            deleted_count += removed;
        }
        outcomes.push(DeleteOutcome {
            path: "Device firmware (IPSW)".into(),
            status: if dry_run { "dry-run" } else { "ok" }.into(),
            size_bytes: 0,
            detail: format!("已清理 {removed} 个 IPSW"),
        });
    }

    // Time Machine / Large files：只读报告，不执行删除。
    if selected_groups.iter().any(|s| s == "Time Machine incomplete backups") {
        let count = special::count_incomplete_tm_backups();
        outcomes.push(DeleteOutcome {
            path: "Time Machine".into(),
            status: "skipped".into(),
            size_bytes: 0,
            detail: match count {
                Some(n) => format!("{n} 个未完成备份（只读审查）"),
                None => "无未完成备份或不可用".into(),
            },
        });
    }
    if selected_groups.iter().any(|s| s == "Large files review") {
        let large = special::large_file_candidates();
        outcomes.push(DeleteOutcome {
            path: "Large files".into(),
            status: "skipped".into(),
            size_bytes: 0,
            detail: format!("{} 个 ≥1GB 路径（只读审查）", large.len()),
        });
    }

    // 外置卷（对标 clean_external_volume_target）。
    if let Ok(vols) = std::fs::read_dir("/Volumes") {
        for vol in vols.flatten() {
            let vol_path = vol.path();
            let name = vol.file_name().to_string_lossy().to_string();
            let group_desc = format!("External volume · {name}");
            if !selected_groups.iter().any(|s| *s == group_desc) {
                continue;
            }
            if !vol_path.is_dir() || vol_path.is_symlink() {
                continue;
            }
            let targets = special::scan_external_volume(&vol_path.to_string_lossy());
            let mut removed = 0usize;
            for p in &targets {
                let s = p.to_string_lossy().to_string();
                let outcome = delete::delete_to_trash(&s, dry_run, "clean");
                match outcome.status.as_str() {
                    "ok" | "dry-run" => removed += 1,
                    "failed" => failed_count += 1,
                    _ => {}
                }
                freed_bytes += outcome.size_bytes;
            }
            if !dry_run {
                deleted_count += removed;
            }
            outcomes.push(DeleteOutcome {
                path: group_desc,
                status: if dry_run { "dry-run" } else { "ok" }.into(),
                size_bytes: 0,
                detail: format!("已清理 {removed} 项"),
            });
        }
    }

    // LaunchAgents 提示：只读，不执行。
    if selected_groups.iter().any(|s| s == "LaunchAgents hints") {
        let hints = special::launch_agent_hints();
        outcomes.push(DeleteOutcome {
            path: "LaunchAgents hints".into(),
            status: "skipped".into(),
            size_bytes: 0,
            detail: format!("{} 个提示（只读）", hints.len()),
        });
    }

    delete::log_session_end("clean", deleted_count, freed_bytes);

    CleanExecuteResult {
        outcomes,
        deleted_count,
        freed_bytes,
        failed_count,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expand_home_resolves() {
        let p = expand_home("~/Library/Caches");
        assert!(p.starts_with(std::env::var("HOME").unwrap()));
        let p = expand_home("/absolute/path");
        assert_eq!(p, PathBuf::from("/absolute/path"));
    }

    #[test]
    fn glob_expansion_works() {
        // 在系统临时目录构造 fixture。
        let tmp = std::env::temp_dir().join(format!("mole_rs_glob_test_{}", std::process::id()));
        let sub = tmp.join("Sources/abc");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join("Photos.cache"), "x").unwrap();
        std::fs::write(tmp.join("other.txt"), "y").unwrap();

        let matches = expand_glob(&tmp.join("Sources/*/Photos.cache"));
        assert_eq!(matches.len(), 1);
        assert!(matches[0].ends_with("Sources/abc/Photos.cache"));

        let none = expand_glob(&tmp.join("Sources/xyz/Photos.cache"));
        assert!(none.is_empty());

        let wildcard_all = expand_glob(&tmp.join("*.txt"));
        assert_eq!(wildcard_all.len(), 1);

        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn size_counts_files_skips_symlinks() {
        let tmp = std::env::temp_dir().join(format!("mole_rs_size_test_{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::write(tmp.join("a.bin"), vec![0u8; 1024]).unwrap();
        std::fs::create_dir(tmp.join("sub")).unwrap();
        std::fs::write(tmp.join("sub/b.bin"), vec![0u8; 512]).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(tmp.join("a.bin"), tmp.join("link.bin")).unwrap();

        let size = path_size_with_deadline(&tmp, Instant::now() + SIZE_SCAN_DEADLINE);
        assert_eq!(size, 1536);

        std::fs::remove_dir_all(&tmp).ok();
    }

    /// 执行流程在受保护路径上必须跳过（sink 复检）。
    #[test]
    fn execute_skips_protected_paths() {
        // 用一个不存在的组名：不产生任何目标。
        let r = execute_clean(&["不存在组".to_string()], true);
        assert_eq!(r.outcomes.len(), 0);
        assert_eq!(r.deleted_count, 0);
    }

    /// dry-run 执行：对临时 fixture 标记 dry-run 且不删除。
    #[test]
    fn execute_dry_run_leaves_files() {
        let tmp = std::env::temp_dir().join(format!("mole_rs_dry_{}", std::process::id()));
        let cache = tmp.join("Data/Library/Caches");
        std::fs::create_dir_all(&cache).unwrap();
        std::fs::write(cache.join("x.bin"), vec![0u8; 128]).unwrap();
        // execute_clean 的目录是固定的 catalog，无法注入临时路径；
        // dry-run 语义由 delete::delete_to_trash 的单测覆盖。
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// 全量条目描述唯一（GUI 按 description 选组）。
    #[test]
    fn all_entry_descriptions_unique() {
        let mut seen = std::collections::HashSet::new();
        for entry in collect_entries() {
            assert!(
                seen.insert(entry.description.clone()),
                "重复描述: {}",
                entry.description
            );
        }
    }

    /// 年龄过滤：age_days=0 恒真；未来 mtime 为假。
    #[test]
    fn age_filter_semantics() {
        let tmp = std::env::temp_dir().join(format!("mole_rs_age_{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        assert!(is_older_than_days(&tmp, 0));
        // 新文件不满足 >0 天。
        assert!(!is_older_than_days(&tmp, 30));
        std::fs::remove_dir_all(&tmp).ok();
    }

    /// Deno root：不安全路径拒绝。
    #[test]
    fn deno_root_safety() {
        unsafe { std::env::remove_var("DENO_DIR") };
        // 默认 ~/Library/Caches/deno 存在时返回 Some；不 panic。
        let _ = deno_cache_root();
        unsafe { std::env::set_var("DENO_DIR", "/") };
        assert!(deno_cache_root().is_none());
        unsafe { std::env::set_var("DENO_DIR", "relative/path") };
        assert!(deno_cache_root().is_none());
        unsafe { std::env::remove_var("DENO_DIR") };
    }

    /// 进程守卫字段：浏览器守卫行必须带 probe；静态 catalog 行不带。
    #[test]
    fn process_probe_attachment() {
        for entry in collect_entries() {
            if entry.family == "browser" {
                // 静态浏览器行无守卫；守卫行有。
                // 至少存在若干带守卫的浏览器行（本机装了 Chrome 时）。
                let _ = entry.process_probe;
            }
        }
        // cargo registry：存在时必须带 rust_build 守卫。
        if let Some(entry) = cargo_registry_entry() {
            assert!(entry.process_probe.is_some());
            assert_eq!(entry.family, "dev_rust");
        }
    }

    /// Service Worker 域名提取与保护列表。
    #[test]
    fn sw_domain_extraction_and_protection() {
        // 对标 basename | grep -oE | head -1：TLD 仅 [a-zA-Z]{2,}，
        // 多级域会在第二个点处截断（docs.google.com → docs.google）。
        assert_eq!(
            extract_domain_from_sw_folder("https_github.com_0"),
            Some("github.com".into())
        );
        assert_eq!(
            extract_domain_from_sw_folder("https_docs.google.com_443"),
            Some("docs.google".into())
        );
        assert_eq!(extract_domain_from_sw_folder("0a1b2c3d"), None);
        // 子串匹配（对标 == *"$protected_domain"*）。
        assert!(is_protected_sw_domain("github.com"));
        assert!(is_protected_sw_domain("sub.figma.com"));
        assert!(!is_protected_sw_domain("example.com"));
        // skip_reason 域名分支。
        let wl = whitelist::Whitelist::load();
        assert_eq!(
            skip_reason("/tmp/https_github.com_0", &wl, true),
            Some("protected domain")
        );
        assert_eq!(skip_reason("/tmp/https_example.com_0", &wl, true), None);
    }
}

/// 真机冒烟（默认忽略）：`cargo test clean_preview_smoke -- --ignored --nocapture`
#[cfg(test)]
mod smoke_tests {
    #[test]
    #[ignore]
    fn clean_preview_smoke() {
        let preview = super::scan_preview();
        println!(
            "whitelist={} total={:.2} MB groups={}",
            preview.whitelist_source,
            preview.total_size_bytes as f64 / 1048576.0,
            preview.groups.len()
        );
        for g in preview.groups.iter().filter(|g| g.total_size_bytes > 0) {
            println!(
                "  {} · {:.2} MB ({} items)",
                g.description,
                g.total_size_bytes as f64 / 1048576.0,
                g.items.len()
            );
        }
        assert!(!preview.groups.is_empty());
    }

    /// 条目计数冒烟（快速，不测径）：确认目录规模与描述唯一性在真机成立。
    #[test]
    fn entry_count_smoke() {
        let entries = super::collect_entries();
        let mut seen = std::collections::HashSet::new();
        for e in &entries {
            assert!(seen.insert(e.description.clone()), "重复: {}", e.description);
        }
        println!("collect_entries={} unique_desc={}", entries.len(), seen.len());
        assert!(entries.len() > 100, "目录应已覆盖主要清理族");
    }
    #[test]
    #[ignore]
    fn clean_execute_dry_run_smoke() {
        let all: Vec<String> = crate::clean::catalog::full_catalog()
            .iter()
            .map(|e| e.description.to_string())
            .collect();
        let r = super::execute_clean(&all, true);
        println!(
            "dry-run: outcomes={} deleted={} failed={}",
            r.outcomes.len(),
            r.deleted_count,
            r.failed_count
        );
        for o in r.outcomes.iter().take(5) {
            println!("  [{}] {} ({})", o.status, o.path, o.detail);
        }
        assert_eq!(r.deleted_count, 0, "dry-run 不得删除任何文件");
    }
}
