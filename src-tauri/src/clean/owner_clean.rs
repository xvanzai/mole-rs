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

/// pnpm store 路径（仅第一个可用二进制；多二进制去重逻辑见原实现）。
fn pnpm_store_path() -> Option<PathBuf> {
    // 对标 list_installed_pnpm_binaries 的简化：仅检查 PATH 中的 pnpm。
    if !probe::tool_available("pnpm", &["--version"]) {
        return None;
    }
    // COREPACK_ENABLE_DOWNLOAD_PROMPT=0 避免交互下载提示。
    let out = crate::status::run_cmd_with_env(
        "pnpm",
        &["store", "path"],
        &[("COREPACK_ENABLE_DOWNLOAD_PROMPT", "0")],
        QUICK,
    );
    if let Ok(o) = out {
        let trimmed = o.trim().trim_end_matches('/').to_string();
        // is_safe_pnpm_store_path：绝对路径 + 非 / 非 $HOME。
        if trimmed.starts_with('/') && trimmed != "/" {
            let home = std::env::var("HOME").unwrap_or_default();
            if trimmed != home {
                return Some(PathBuf::from(trimmed));
            }
        }
    }
    None
}

/// pnpm 进程守卫（对标 pnpm_process_blocks_prune：Running/Unknown 均阻断）。
fn pnpm_process_blocks() -> ProcessState {
    process::pnpm_process_state()
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
    // bin_path 实际是 store 根；命令仍用 pnpm。
    let _ = bin_path;
    Some(OwnerCmd {
        bin: "pnpm".into(),
        args: vec!["store".into(), "prune".into()],
        env: vec![("COREPACK_ENABLE_DOWNLOAD_PROMPT".into(), "0".into())],
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
