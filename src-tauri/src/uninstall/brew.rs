//! Homebrew Cask 卸载支持，对标 `lib/uninstall/brew.sh`。
//!
//! 四阶段检测（快→慢）：resolved path → Caskroom 搜索 → symlink →
//! brew list+info。卸载走 `brew uninstall --cask --zap`（NONINTERACTIVE），
//! 超时按应用大小 300/600/900s。

use std::path::{Path, PathBuf};
use std::time::Duration;

const PKG_LIST: Duration = Duration::from_secs(15);

fn homebrew_available() -> bool {
    super::super::clean::command_exists("brew")
}

/// 对标 resolve_path：canonicalize（跟随符号链接）。
fn resolve_path(p: &Path) -> Option<PathBuf> {
    if p.exists() {
        return p.canonicalize().ok();
    }
    None
}

/// 对标 _extract_cask_token_from_path：Caskroom 路径第一段为 token。
fn extract_cask_token(path: &str) -> Option<String> {
    let rest = path
        .strip_prefix("/opt/homebrew/Caskroom/")
        .or_else(|| path.strip_prefix("/usr/local/Caskroom/"))?;
    let token = rest.split('/').next()?;
    // 校验：小写字母数字 + 连字符。
    if token.is_empty() {
        return None;
    }
    let mut chars = token.chars();
    let first = chars.next()?;
    if !first.is_ascii_lowercase() && !first.is_ascii_digit() {
        return None;
    }
    if token
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        Some(token.to_string())
    } else {
        None
    }
}

/// brew list --cask 输出（失败返回 None）。
fn brew_list_casks() -> Option<String> {
    crate::status::run_cmd_with_env(
        "brew",
        &["list", "--cask"],
        &[("HOMEBREW_NO_ENV_HINTS", "1")],
        PKG_LIST,
    )
    .ok()
}

/// 对标 is_brew_cask_installed：0=installed，1=not，2=unknown。
fn cask_installed(cask: &str) -> Option<bool> {
    if cask.is_empty() || !homebrew_available() {
        return None;
    }
    let list = brew_list_casks()?;
    Some(list.lines().any(|l| l.trim() == cask))
}

/// 阶段 1：canonical 路径在 Caskroom 内。
fn detect_via_resolved_path(app_path: &Path) -> Option<String> {
    let resolved = resolve_path(app_path)?;
    // basename 必须一致（防止 symlink 指向别处同名）。
    if resolved.file_name()? != app_path.file_name()? {
        return None;
    }
    extract_cask_token(&resolved.to_string_lossy())
}

/// 阶段 2：Caskroom 按 .app 名搜索（maxdepth 3）；唯一 token 且 installed
/// 且 brew info 验证路径。
fn detect_via_caskroom_search(app_path: &Path) -> Option<String> {
    let bundle_name = app_path.file_name()?.to_string_lossy().to_string();
    let mut tokens: Vec<String> = Vec::new();
    for room in ["/opt/homebrew/Caskroom", "/usr/local/Caskroom"] {
        let room_path = Path::new(room);
        if !room_path.is_dir() {
            continue;
        }
        // 有界深度遍历（对标 find -maxdepth 3）。
        let Ok(top) = std::fs::read_dir(room_path) else {
            continue;
        };
        for token_dir in top.flatten() {
            let Ok(ft) = token_dir.file_type() else {
                continue;
            };
            if !ft.is_dir() {
                continue;
            }
            let Ok(versions) = std::fs::read_dir(token_dir.path()) else {
                continue;
            };
            for ver in versions.flatten() {
                let Ok(vft) = ver.file_type() else {
                    continue;
                };
                if !vft.is_dir() {
                    continue;
                }
                // 第三层：找 <bundle_name>。
                let app_in_ver = ver.path().join(&bundle_name);
                if app_in_ver.exists() || app_in_ver.is_symlink() {
                    if let Some(token) = extract_cask_token(&app_in_ver.to_string_lossy()) {
                        if !tokens.contains(&token) {
                            tokens.push(token);
                        }
                    }
                }
            }
        }
    }
    if tokens.len() != 1 {
        return None;
    }
    let token = &tokens[0];
    if cask_installed(token) != Some(true) {
        return None;
    }
    // brew info 验证路径归属。
    let info = crate::status::run_cmd_with_env(
        "brew",
        &["info", "--cask", token],
        &[("HOMEBREW_NO_ENV_HINTS", "1")],
        PKG_LIST,
    )
    .ok()?;
    let path_str = app_path.to_string_lossy().to_string();
    let bundle_name = app_path.file_name()?.to_string_lossy().to_string();
    if info.contains(&path_str)
        || (path_str == format!("/Applications/{bundle_name}") && info.contains(&bundle_name))
    {
        Some(token.clone())
    } else {
        None
    }
}

/// 阶段 3：app 为指向 Caskroom 的直接符号链接。
fn detect_via_symlink(app_path: &Path) -> Option<String> {
    let meta = std::fs::symlink_metadata(app_path).ok()?;
    if !meta.file_type().is_symlink() {
        return None;
    }
    let target = std::fs::read_link(app_path).ok()?;
    if target.file_name()? != app_path.file_name()? {
        return None;
    }
    extract_cask_token(&target.to_string_lossy())
}

/// 阶段 4：brew list 匹配小写名 + info 验证。
fn detect_via_brew_list(app_path: &Path) -> Option<String> {
    let bundle_name = app_path.file_name()?.to_string_lossy().to_string();
    let base = bundle_name
        .strip_suffix(".app")
        .or_else(|| bundle_name.strip_suffix(".App"))
        .unwrap_or(&bundle_name);
    let lower = base.to_lowercase();
    let list = brew_list_casks()?;
    let cask = list.lines().find(|l| l.trim() == lower)?.trim().to_string();
    let info = crate::status::run_cmd_with_env(
        "brew",
        &["info", "--cask", &cask],
        &[("HOMEBREW_NO_ENV_HINTS", "1")],
        PKG_LIST,
    )
    .ok()?;
    let path_str = app_path.to_string_lossy().to_string();
    if info.contains(&path_str)
        || (path_str == format!("/Applications/{bundle_name}") && info.contains(&bundle_name))
    {
        Some(cask)
    } else {
        None
    }
}

/// 对标 get_brew_cask_name：四阶段检测。
pub fn get_brew_cask_name(app_path: &str) -> Option<String> {
    let p = Path::new(app_path);
    if app_path.is_empty() || !(p.exists() || p.is_symlink()) {
        return None;
    }
    if !homebrew_available() {
        return None;
    }
    detect_via_resolved_path(p)
        .or_else(|| detect_via_caskroom_search(p))
        .or_else(|| detect_via_symlink(p))
        .or_else(|| detect_via_brew_list(p))
}

/// 超时秒数（对标按应用大小分级）。
fn uninstall_timeout(app_path: &str) -> u64 {
    let p = Path::new(app_path);
    if !p.is_dir() {
        return 300;
    }
    let size = crate::clean::path_size_with_deadline(p, std::time::Instant::now() + Duration::from_secs(5));
    let size_gb = size / (1024 * 1024 * 1024);
    if size_gb > 15 {
        900
    } else if size_gb > 5 {
        600
    } else {
        300
    }
}

/// 对标 brew_uninstall_cask。
/// `zap=false` 时用 nozap（共享 bundle id 兄弟守卫场景；首片默认 zap）。
/// 返回 (成功, 详情)。
pub fn brew_uninstall_cask(cask: &str, app_path: &str, zap: bool, dry_run: bool) -> (bool, String) {
    if cask.is_empty() {
        return (false, "cask 名为空".into());
    }
    if !homebrew_available() {
        return (false, "brew 不可用".into());
    }

    let mut args = vec!["uninstall".to_string(), "--cask".to_string()];
    if zap {
        args.push("--zap".to_string());
    }
    args.push(cask.to_string());

    if dry_run {
        return (true, format!("将执行 brew {}", args.join(" ")));
    }

    let timeout = uninstall_timeout(app_path);
    let env = [
        ("HOMEBREW_NO_ENV_HINTS", "1"),
        ("HOMEBREW_NO_AUTO_UPDATE", "1"),
        ("NONINTERACTIVE", "1"),
    ];
    let arg_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    match crate::status::run_cmd_with_env("brew", &arg_refs, &env, Duration::from_secs(timeout)) {
        Ok(_) => {
            // 验证移除。
            let cask_gone = cask_installed(cask) == Some(false);
            let app_gone = !Path::new(app_path).exists() && !Path::new(app_path).is_symlink();
            if cask_gone && app_gone {
                (true, format!("已通过 brew 卸载 {cask}"))
            } else {
                (
                    false,
                    format!("brew 返回成功但验证未通过（cask_gone={cask_gone} app_gone={app_gone}）"),
                )
            }
        }
        Err(e) if e.contains("timed out") => (false, format!("brew 卸载超时（{timeout}s）")),
        Err(e) => (false, format!("brew 卸载失败：{e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Caskroom token 提取与校验。
    #[test]
    fn cask_token_extraction() {
        assert_eq!(
            extract_cask_token("/opt/homebrew/Caskroom/visual-studio-code/1.85.0/Visual Studio Code.app"),
            Some("visual-studio-code".into())
        );
        assert_eq!(
            extract_cask_token("/usr/local/Caskroom/iterm2/3.4/iTerm.app"),
            Some("iterm2".into())
        );
        assert_eq!(extract_cask_token("/Applications/Foo.app"), None);
        assert_eq!(extract_cask_token("/opt/homebrew/Caskroom//x"), None);
        // 非法 token 字符。
        assert_eq!(extract_cask_token("/opt/homebrew/Caskroom/Bad_Token/1/x"), None);
    }

    /// 不存在的路径 → None。
    #[test]
    fn detect_missing_app() {
        assert_eq!(get_brew_cask_name("/nonexistent/App.app"), None);
        assert_eq!(get_brew_cask_name(""), None);
    }

    /// dry-run 卸载：有 brew 时返回 would 命令；无 brew 时失败详情非空。
    #[test]
    fn uninstall_dry_run() {
        let (ok, detail) = brew_uninstall_cask("foo-bar", "/nonexistent.app", true, true);
        if homebrew_available() {
            assert!(ok);
            assert!(detail.contains("brew"));
        } else {
            assert!(!ok);
        }
    }
}
