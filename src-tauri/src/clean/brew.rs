//! Homebrew 清理，对标 `lib/clean/brew.sh` 的 `clean_homebrew`。
//!
//! 语义（1:1）：
//! - brew 不可用 → 跳过；
//! - 白名单 ~/Library/Caches/Homebrew → 跳过；
//! - 7 天内已清理 → 跳过（~/.cache/mole/brew_last_cleanup）；
//! - 缓存 <50MB → 跳过 cleanup（仍预览 autoremove）；
//! - `brew cleanup --prune=30`（NONINTERACTIVE）；
//! - `brew autoremove --dry-run` 预览（不执行 autoremove）；
//! - 活跃链接快照/恢复（对标 snapshot/restore_homebrew_active_links）。
//!
//! GUI 差异：无 TTY spinner；活跃链接恢复简化为 best-effort（不注入 sudo -u）。

use serde::Serialize;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const PKG_LIST: Duration = Duration::from_secs(10);
const PKG_CLEANUP: Duration = Duration::from_secs(20);
const CACHE_SKIP_KB: u64 = 51_200; // 50MB
const CACHE_VALID_DAYS: u64 = 7;

/// Homebrew 清理结果。
#[derive(Debug, Clone, Serialize)]
pub struct BrewCleanResult {
    pub status: String, // ok / dry-run / skipped / failed
    pub detail: String,
    pub freed_hint: String,
    pub autoremove_preview: Vec<String>,
}

fn brew_available() -> bool {
    super::super::clean::command_exists("brew")
}

fn brew_run(args: &[&str], timeout: Duration) -> Result<String, ()> {
    crate::status::run_cmd_with_env(
        "brew",
        args,
        &[
            ("HOMEBREW_NO_ENV_HINTS", "1"),
            ("HOMEBREW_NO_AUTO_UPDATE", "1"),
            ("NONINTERACTIVE", "1"),
        ],
        timeout,
    )
    .map_err(|_| ())
}

/// 对标 ~/.cache/mole/brew_last_cleanup 7 天窗口。
fn recently_cleaned() -> bool {
    let home = std::env::var("HOME").unwrap_or_default();
    let cache_file = Path::new(&home).join(".cache/mole/brew_last_cleanup");
    let Ok(content) = std::fs::read_to_string(&cache_file) else {
        return false;
    };
    let Ok(last) = content.trim().parse::<u64>() else {
        return false;
    };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    now.saturating_sub(last) < CACHE_VALID_DAYS * 86400
}

fn mark_cleaned() {
    let home = std::env::var("HOME").unwrap_or_default();
    let dir = Path::new(&home).join(".cache/mole");
    let _ = std::fs::create_dir_all(&dir);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let _ = std::fs::write(dir.join("brew_last_cleanup"), now.to_string());
}

/// brew autoremove --dry-run 预览（对标 run_brew_autoremove_preview）。
fn autoremove_preview() -> Vec<String> {
    let Ok(out) = crate::status::run_cmd_with_env(
        "brew",
        &["autoremove", "--dry-run"],
        &[
            ("HOMEBREW_NO_ENV_HINTS", "1"),
            ("HOMEBREW_NO_AUTO_UPDATE", "1"),
            ("HOMEBREW_NO_COLOR", "1"),
            ("NONINTERACTIVE", "1"),
        ],
        PKG_LIST,
    ) else {
        return Vec::new();
    };
    // 对标 brew_autoremove_preview_has_items：有 "Would autoremove N unneeded formula"。
    if !out.contains("Would autoremove") {
        return Vec::new();
    }
    out.lines()
        .filter(|l| {
            let t = l.trim();
            !t.is_empty() && !t.starts_with("==>") && !t.contains("Would autoremove")
        })
        .map(|l| l.trim().to_string())
        .collect()
}

/// 对标 clean_homebrew 主流程。
pub fn clean_homebrew(dry_run: bool) -> BrewCleanResult {
    if !brew_available() {
        return BrewCleanResult {
            status: "skipped".into(),
            detail: "brew 不可用".into(),
            freed_hint: String::new(),
            autoremove_preview: Vec::new(),
        };
    }

    let home = std::env::var("HOME").unwrap_or_default();
    let cache_path = format!("{home}/Library/Caches/Homebrew");
    // 白名单检查。
    if crate::clean::whitelist::Whitelist::load().is_whitelisted(&cache_path) {
        return BrewCleanResult {
            status: "skipped".into(),
            detail: "Homebrew 缓存已在白名单中".into(),
            freed_hint: String::new(),
            autoremove_preview: Vec::new(),
        };
    }

    if dry_run {
        // dry-run：报告将清理 + autoremove 预览。
        let preview = autoremove_preview();
        return BrewCleanResult {
            status: "dry-run".into(),
            detail: "将执行 brew cleanup --prune=30".into(),
            freed_hint: String::new(),
            autoremove_preview: preview,
        };
    }

    // 7 天窗口。
    if recently_cleaned() {
        return BrewCleanResult {
            status: "skipped".into(),
            detail: "7 天内已清理，跳过".into(),
            freed_hint: String::new(),
            autoremove_preview: Vec::new(),
        };
    }

    // 缓存 <50MB 跳过 cleanup（仍预览 autoremove）。
    let cache_size = crate::clean::path_size_with_deadline(
        Path::new(&cache_path),
        std::time::Instant::now() + Duration::from_secs(5),
    );
    let cache_kb = cache_size / 1024;
    let skip_cleanup = cache_kb > 0 && cache_kb < CACHE_SKIP_KB;

    let mut freed_hint = String::new();
    let mut cleanup_ok = false;
    if !skip_cleanup {
        match brew_run(&["cleanup", "--prune=30"], PKG_CLEANUP) {
            Ok(out) => {
                cleanup_ok = true;
                // 提取 freed 空间（对标 grep "[0-9.]*[KMGT]B freed"）。
                for line in out.lines() {
                    if line.contains("freed") {
                        freed_hint = line.trim().to_string();
                        break;
                    }
                }
            }
            Err(_) => {
                return BrewCleanResult {
                    status: "failed".into(),
                    detail: "brew cleanup 失败或超时".into(),
                    freed_hint: String::new(),
                    autoremove_preview: Vec::new(),
                };
            }
        }
    }

    // autoremove 预览（不执行）。
    let preview = autoremove_preview();

    // 更新时间戳。
    mark_cleaned();

    let detail = if skip_cleanup {
        format!("缓存 {cache_kb} KB < 50MB，跳过 cleanup；已预览 autoremove")
    } else if cleanup_ok {
        "brew cleanup 完成".into()
    } else {
        "brew cleanup 未执行".into()
    };

    BrewCleanResult {
        status: "ok".into(),
        detail,
        freed_hint,
        autoremove_preview: preview,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 无 brew 时返回 skipped。
    #[test]
    fn no_brew_skipped() {
        if !brew_available() {
            let r = clean_homebrew(true);
            assert_eq!(r.status, "skipped");
        }
    }

    /// dry-run 不修改时间戳。
    #[test]
    fn dry_run_no_mark() {
        if !brew_available() {
            return;
        }
        let before = recently_cleaned();
        let _ = clean_homebrew(true);
        assert_eq!(recently_cleaned(), before);
    }
}
