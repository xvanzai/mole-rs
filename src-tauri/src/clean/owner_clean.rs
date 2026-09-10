//! owner 命令删除汇，对标 `lib/clean/dev.sh` 的 `clean_tool_cache` 调用点。
//!
//! AGENTS.md 契约（必须满足）：
//! 1. **变更根可机器读出**：cache 根经 owner 命令探测并校验（绝对路径、
//!    无 `..`、非 / 非 $HOME）；
//! 2. **dry-run 与真实共享同一候选计划**：同一 resolve 路径；
//! 3. **部分失败可观察**：命令失败/超时记为 failed，不静默吞掉。
//!
//! 与路径删除不同：owner 命令自己管理缓存树，Mole 只触发并报告。

use super::process::{self, ProcessState};
use super::protect;
use super::probe;
use super::whitelist::Whitelist;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// owner 清理操作：一条 = 一个 owner 命令 + 其缓存根。
pub struct OwnerCleanOp {
    pub description: &'static str,
    /// 解析缓存根；None = 工具不可用/路径不安全，跳过。
    pub resolve_cache_path: fn() -> Option<PathBuf>,
    /// 构造 owner 命令 (bin, args, env)；None = 命令不可用。
    pub owner_command: fn(&Path) -> Option<OwnerCmd>,
    /// 可选进程守卫（对标 pnpm busy / GitHub CLI started）。
    pub process_probe: Option<fn() -> ProcessState>,
}

pub struct OwnerCmd {
    pub bin: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
}

/// 快速探测超时。
const QUICK: Duration = Duration::from_secs(3);
/// 包清理超时（对标 MOLE_TIMEOUT_PKG_CLEANUP_SEC 量级）。
const PKG_CLEANUP: Duration = Duration::from_secs(60);

/// npm 缓存根（对标 clean_dev_npm）。
fn npm_cache_path() -> Option<PathBuf> {
    let (path, _) = probe::npm_cache_path();
    let s = path.to_string_lossy();
    if s.starts_with('/') && s != "/" {
        Some(path)
    } else {
        None
    }
}

/// uv 缓存根（对标 clean_uv_cache）。
fn uv_cache_path() -> Option<PathBuf> {
    if !probe::tool_available("uv", &["--version"]) {
        return None;
    }
    // uv cache dir
    if let Ok(out) = crate::status::run_cmd("uv", &["cache", "dir"], QUICK) {
        let trimmed = out.trim().trim_end_matches('/').to_string();
        if trimmed.starts_with('/') && trimmed != "/" {
            return Some(PathBuf::from(trimmed));
        }
    }
    Some(probe::uv_default_cache_path())
}

/// corepack 缓存根（对标 clean_corepack_cache）。
fn corepack_cache_path() -> Option<PathBuf> {
    if !probe::tool_available("corepack", &["--version"]) {
        return None;
    }
    probe::corepack_cache_path()
}

/// pip 缓存根（对标 clean_dev_python）。
fn pip_cache_path() -> Option<PathBuf> {
    if !probe::tool_available("pip3", &["--version"]) {
        return None;
    }
    if let Ok(out) = crate::status::run_cmd("pip3", &["cache", "dir"], QUICK) {
        let trimmed = out.trim().trim_end_matches('/').to_string();
        if trimmed.starts_with('/') && trimmed != "/" {
            return Some(PathBuf::from(trimmed));
        }
    }
    let home = std::env::var("HOME").unwrap_or_default();
    Some(PathBuf::from(home).join("Library/Caches/pip"))
}

/// bun 缓存根（对标 clean_dev_npm 的 bun 分支）。
fn bun_cache_path() -> Option<PathBuf> {
    if !probe::tool_available("bun", &["--version"]) {
        return None;
    }
    let home = std::env::var("HOME").unwrap_or_default();
    let default = PathBuf::from(&home).join(".bun/install/cache");
    if let Ok(out) = crate::status::run_cmd("bun", &["pm", "cache"], QUICK) {
        let trimmed = out.trim().trim_end_matches('/').to_string();
        if trimmed.starts_with('/') && trimmed != "/" {
            return Some(PathBuf::from(trimmed));
        }
    }
    Some(default)
}

/// 对标 is_safe_pnpm_store_path：绝对路径 + 无 .. / 控制字符。
fn is_safe_pnpm_store_path(path: &str) -> bool {
    if path.is_empty() || !path.starts_with('/') {
        return false;
    }
    if path.contains("/../") || path.ends_with("/..") || path == ".." {
        return false;
    }
    !path.chars().any(|c| c == '\n' || c == '\r')
}

/// 对标 list_installed_pnpm_binaries：PATH pnpm + mise 版本安装。
/// 每个元素是 (bin, store_path)。
fn list_pnpm_stores() -> Vec<(String, PathBuf)> {
    let mut pairs: Vec<(String, PathBuf)> = Vec::new();
    let mut seen_stores: Vec<PathBuf> = Vec::new();
    let mut bins: Vec<String> = Vec::new();

    // PATH pnpm。
    if probe::tool_available("pnpm", &["--version"]) {
        bins.push("pnpm".into());
    }
    // mise 安装。
    let home = std::env::var("HOME").unwrap_or_default();
    let mise_root = PathBuf::from(&home).join(".local/share/mise/installs/pnpm");
    if let Ok(entries) = std::fs::read_dir(&mise_root) {
        for e in entries.flatten() {
            let p = e.path().join("pnpm");
            if p.is_file() {
                use std::os::unix::fs::PermissionsExt;
                if std::fs::metadata(&p)
                    .map(|m| m.permissions().mode() & 0o111 != 0)
                    .unwrap_or(false)
                {
                    bins.push(p.to_string_lossy().to_string());
                }
            }
        }
    }

    for bin in bins {
        let bin_ref: &str = &bin;
        let out = crate::status::run_cmd_with_env(
            bin_ref,
            &["store", "path"],
            &[("COREPACK_ENABLE_DOWNLOAD_PROMPT", "0")],
            QUICK,
        );
        let Ok(o) = out else { continue };
        let trimmed = o.trim().trim_end_matches('/').to_string();
        if !is_safe_pnpm_store_path(&trimmed) {
            continue;
        }
        let store = PathBuf::from(&trimmed);
        if seen_stores.contains(&store) {
            continue; // 去重（#1370）
        }
        seen_stores.push(store.clone());
        pairs.push((bin, store));
    }
    pairs
}

/// pnpm store 路径（用于预览大小；取第一个唯一 store）。
fn pnpm_store_path() -> Option<PathBuf> {
    list_pnpm_stores().into_iter().next().map(|(_, p)| p)
}

/// pnpm 进程守卫（对标 pnpm_process_blocks_prune：Running/Unknown 均阻断）。
fn pnpm_process_blocks() -> ProcessState {
    process::pnpm_process_state()
}

/// Tart 缓存根（对标 clean_tart_caches）。
fn tart_cache_path() -> Option<PathBuf> {
    if !probe::tool_available("tart", &["--version"]) {
        return None;
    }
    let home = std::env::var("HOME").unwrap_or_default();
    let p = PathBuf::from(home).join(".tart/cache");
    if p.is_dir() {
        Some(p)
    } else {
        None
    }
}

/// Tart 进程守卫。
fn tart_process_blocks() -> ProcessState {
    process::tart_process_state()
}

fn npm_owner_cmd(_path: &Path) -> Option<OwnerCmd> {
    if !probe::tool_available("npm", &["--version"]) {
        return None;
    }
    Some(OwnerCmd {
        bin: "npm".into(),
        args: vec!["cache".into(), "clean".into(), "--force".into()],
        env: vec![],
    })
}

fn uv_owner_cmd(_path: &Path) -> Option<OwnerCmd> {
    Some(OwnerCmd {
        bin: "uv".into(),
        args: vec!["cache".into(), "prune".into()],
        env: vec![],
    })
}

fn corepack_owner_cmd(_path: &Path) -> Option<OwnerCmd> {
    Some(OwnerCmd {
        bin: "corepack".into(),
        args: vec!["cache".into(), "clean".into()],
        env: vec![("COREPACK_ENABLE_DOWNLOAD_PROMPT".into(), "0".into())],
    })
}

fn pip_owner_cmd(_path: &Path) -> Option<OwnerCmd> {
    // 对标 bash -c 'pip3 cache purge || true'：失败不阻断（|| true）。
    // 但部分失败仍应可观察——记 Applied 但 detail 注明。
    Some(OwnerCmd {
        bin: "pip3".into(),
        args: vec!["cache".into(), "purge".into()],
        env: vec![],
    })
}

fn bun_owner_cmd(_path: &Path) -> Option<OwnerCmd> {
    Some(OwnerCmd {
        bin: "bun".into(),
        args: vec!["pm".into(), "cache".into(), "rm".into()],
        env: vec![],
    })
}

fn pnpm_owner_cmd(bin_path: &Path) -> Option<OwnerCmd> {
    // bin_path 实际是 store 根；命令仍用 pnpm（多二进制在 execute 中特殊处理）。
    let _ = bin_path;
    Some(OwnerCmd {
        bin: "pnpm".into(),
        args: vec!["store".into(), "prune".into()],
        env: vec![("COREPACK_ENABLE_DOWNLOAD_PROMPT".into(), "0".into())],
    })
}

/// 对标 clean_tart_caches 的 owner 命令：tart prune --entries caches --older-than 30。
fn tart_owner_cmd(_path: &Path) -> Option<OwnerCmd> {
    // MOLE_ORPHAN_AGE_DAYS 默认 30。
    Some(OwnerCmd {
        bin: "tart".into(),
        args: vec![
            "prune".into(),
            "--entries".into(),
            "caches".into(),
            "--older-than".into(),
            "30".into(),
        ],
        env: vec![],
    })
}

/// 全部 owner 命令操作表。
pub fn owner_clean_ops() -> Vec<OwnerCleanOp> {
    vec![
        OwnerCleanOp {
            description: "npm cache (owner command)",
            resolve_cache_path: npm_cache_path,
            owner_command: npm_owner_cmd,
            process_probe: None,
        },
        OwnerCleanOp {
            description: "uv cache (owner command)",
            resolve_cache_path: uv_cache_path,
            owner_command: uv_owner_cmd,
            process_probe: None,
        },
        OwnerCleanOp {
            description: "Corepack cache (owner command)",
            resolve_cache_path: corepack_cache_path,
            owner_command: corepack_owner_cmd,
            process_probe: None,
        },
        OwnerCleanOp {
            description: "pip cache (owner command)",
            resolve_cache_path: pip_cache_path,
            owner_command: pip_owner_cmd,
            process_probe: None,
        },
        OwnerCleanOp {
            description: "bun cache (owner command)",
            resolve_cache_path: bun_cache_path,
            owner_command: bun_owner_cmd,
            process_probe: None,
        },
        OwnerCleanOp {
            description: "pnpm cache (owner command)",
            resolve_cache_path: pnpm_store_path,
            owner_command: pnpm_owner_cmd,
            process_probe: Some(pnpm_process_blocks),
        },
        OwnerCleanOp {
            description: "Tart caches (owner command)",
            resolve_cache_path: tart_cache_path,
            owner_command: tart_owner_cmd,
            process_probe: Some(tart_process_blocks),
        },
    ]
}

/// 执行单个 owner 命令清理。返回 (是否成功, 详情)。
/// dry_run=true 时只报告不执行。
pub fn execute_owner_clean(op: &OwnerCleanOp, dry_run: bool) -> (bool, String) {
    // 进程守卫。
    if let Some(probe) = op.process_probe {
        if let Err(reason) = process::guard_allows(probe()) {
            return (false, format!("跳过（{reason}）"));
        }
    }

    let Some(path) = (op.resolve_cache_path)() else {
        return (false, "工具不可用或路径不安全".into());
    };
    let path_str = path.to_string_lossy().to_string();

    // 路径安全复检（对标 validate_path_for_deletion 意图）。
    if !path_str.starts_with('/') || path_str == "/" {
        return (false, "缓存根不安全".into());
    }
    if protect::should_protect_path(&path_str) {
        return (false, "缓存根受保护".into());
    }
    let whitelist = Whitelist::load();
    if whitelist.is_whitelisted(&path_str) {
        return (true, "已在白名单中，跳过".into());
    }

    // 路径不存在：无事可做。
    if !path.exists() {
        return (true, "缓存根不存在".into());
    }

    let size = super::path_size_with_deadline(&path, std::time::Instant::now() + Duration::from_secs(5));

    // pnpm 多二进制：逐唯一 store 执行 prune（对标 list_installed_pnpm_binaries）。
    if op.description.contains("pnpm") {
        if dry_run {
            let stores = list_pnpm_stores();
            return (
                true,
                format!("将对 {} 个 pnpm store 执行 prune（合计约 {} KB）", stores.len(), size / 1024),
            );
        }
        let stores = list_pnpm_stores();
        if stores.is_empty() {
            return (false, "无可用 pnpm store".into());
        }
        let mut ok_count = 0usize;
        let mut fail_count = 0usize;
        for (bin, store) in &stores {
            let store_str = store.to_string_lossy().to_string();
            if protect::should_protect_path(&store_str) {
                fail_count += 1;
                continue;
            }
            let result = crate::status::run_cmd_with_env(
                bin,
                &["store", "prune"],
                &[("COREPACK_ENABLE_DOWNLOAD_PROMPT", "0")],
                PKG_CLEANUP,
            );
            if result.is_ok() {
                ok_count += 1;
            } else {
                fail_count += 1;
            }
        }
        if fail_count > 0 && ok_count == 0 {
            return (false, format!("全部 {fail_count} 个 store prune 失败"));
        }
        return (
            true,
            format!("已 prune {ok_count} 个 store{}（合计约 {} KB）",
                if fail_count > 0 { format!("，{fail_count} 失败") } else { String::new() },
                size / 1024),
        );
    }

    if dry_run {
        return (
            true,
            format!(
                "将执行 owner 命令清理 {}（{} KB）",
                path_str,
                size / 1024
            ),
        );
    }

    let Some(cmd) = (op.owner_command)(&path) else {
        return (false, "owner 命令不可用".into());
    };
    if !super::command_exists(&cmd.bin) {
        return (false, format!("{} 不可用", cmd.bin));
    }

    match crate::status::run_cmd_with_env(
        &cmd.bin,
        &cmd.args.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
        &cmd
            .env
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect::<Vec<_>>(),
        PKG_CLEANUP,
    ) {
        Ok(_) => (true, format!("owner 命令已完成（释放约 {} KB）", size / 1024)),
        Err(e) => (false, format!("owner 命令失败：{e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 契约：每条 op 的 description 唯一。
    #[test]
    fn owner_ops_descriptions_unique() {
        let mut seen = std::collections::HashSet::new();
        for op in owner_clean_ops() {
            assert!(seen.insert(op.description), "重复: {}", op.description);
        }
    }

    /// 契约：resolve 返回的路径必须绝对且非根（或 None）。
    #[test]
    fn resolve_paths_are_safe() {
        for op in owner_clean_ops() {
            if let Some(p) = (op.resolve_cache_path)() {
                let s = p.to_string_lossy();
                assert!(s.starts_with('/'), "{}: 非绝对路径 {s}", op.description);
                assert_ne!(s, "/", "{}: 根路径", op.description);
            }
        }
    }

    /// dry-run 对不可用工具返回 false + 详情（不 panic）。
    #[test]
    fn dry_run_no_panic() {
        for op in owner_clean_ops() {
            let (_ok, detail) = execute_owner_clean(&op, true);
            assert!(!detail.is_empty());
        }
    }
}
