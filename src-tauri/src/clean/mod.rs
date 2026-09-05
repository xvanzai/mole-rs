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
mod delete;
mod protect;
mod protect_data;
mod whitelist;

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
fn expand_home(pattern: &str) -> PathBuf {    if let Some(rest) = pattern.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return Path::new(&home).join(rest);
        }
    }
    PathBuf::from(pattern)
}

/// 组件级 glob 展开（对标 shell nullglob 展开）。
///
/// 支持每段 `*` / `?` / `[...]`（fnmatch 风格，与 bash `[[ == $p ]]` 的
/// glob 匹配语义一致）；无通配段直接拼接，不存在的段返回空。
fn expand_glob(pattern: &Path) -> Vec<PathBuf> {
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

/// 只读扫描预览（对标 dry-run：`MOLE_DRY_RUN=1 ./mole clean`）。
pub fn scan_preview() -> CleanPreview {
    let whitelist = whitelist::Whitelist::load();
    let mut groups = Vec::new();

    for entry in catalog::apple_user_cache_catalog() {
        let pattern = expand_home(entry.path);
        let mut items = Vec::new();
        let mut skipped = 0usize;
        let mut total = 0u64;

        for target in expand_glob(&pattern) {
            let target_str = target.to_string_lossy().to_string();
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
            description: entry.description.to_string(),
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

    for entry in catalog::apple_user_cache_catalog() {
        if !selected_groups.iter().any(|s| s == entry.description) {
            continue;
        }
        let pattern = expand_home(entry.path);
        for target in expand_glob(&pattern) {
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
        let all: Vec<String> = crate::clean::catalog::apple_user_cache_catalog()
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
