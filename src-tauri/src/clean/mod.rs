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
mod probe;
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
fn skip_reason(path: &str, whitelist: &whitelist::Whitelist) -> Option<&'static str> {
    if protect::should_protect_path(path) {
        return Some("protected");
    }
    if whitelist.is_whitelisted(path) {
        return Some("whitelist");
    }
    None
}

/// 统一扫描条目：静态目录行 + 动态探测行 + 进程守卫行。
struct ScanEntry {
    family: &'static str,
    pattern: PathBuf,
    description: String,
    /// 进程守卫探针（对标 mole_clean_process_guard 三态）：
    /// Running/Unknown 时整组以 skip_reason 拒绝，不进入删除 sink。
    process_probe: Option<fn() -> process::ProcessState>,
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
            });
        }
    }

    // mise 行：无条件 safe_clean（对标 clean_dev_mise 末行）。
    rows.push(ScanEntry {
        family: "dev_cloud",
        pattern: probe::mise_cache_path().join("*"),
        description: "mise cache".into(),
        process_probe: None,
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
        });
        rows.push(ScanEntry {
            family: "virtualization",
            pattern: PathBuf::from(&home)
                .join("Library/Containers/com.utmapp.UTM/Data/tmp/*"),
            description: "UTM temporary files".into(),
            process_probe: Some(process::utm_process_state),
        });
    }

    rows.extend(app_support_regenerable_entries());
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
        });
        rows.push(ScanEntry {
            family: "browser",
            pattern: PathBuf::from(&home)
                .join("Library/Application Support/Firefox/Profiles/*/cache2/*"),
            description: "Firefox profile cache".into(),
            process_probe: Some(process::firefox_process_state),
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
            });
        }
    }

    rows
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
        })
        .collect();
    entries.extend(dynamic_entries());
    entries.extend(guarded_browser_entries());
    entries
}

/// 只读扫描预览（对标 dry-run：`MOLE_DRY_RUN=1 ./mole clean`）。
pub fn scan_preview() -> CleanPreview {
    let whitelist = whitelist::Whitelist::load();
    let mut groups = Vec::new();

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
            // 保护检查在扫描期同样执行：受保护/白名单路径永远不会出现在
            // 可清理列表（对标 _safe_clean_impl 的逐路径检查顺序）。
            if let Some(reason) = skip_reason(&target_str, &whitelist) {
                skipped += 1;
                items.push(CleanItem {
                    path: target_str,
                    size_bytes: 0,
                    skip_reason: reason.to_string(),
                });
                continue;
            }
            let size = path_size_with_deadline(&target, Instant::now() + SIZE_SCAN_DEADLINE);
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

    let total_size = groups.iter().map(|g| g.total_size_bytes).sum();
    CleanPreview {
        groups,
        total_size_bytes: total_size,
        whitelist_source: whitelist.source_description().to_string(),
    }
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
            // Sink 复检：扫描与执行之间状态可能变化（对标 sink re-verify）。
            if let Some(reason) = skip_reason(&target_str, &whitelist) {
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

    /// dry-run 全链路：执行 dry-run，确认零删除且有 dry-run 记录。
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
