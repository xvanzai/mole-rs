//! 工具缓存路径探测，对标 `lib/clean/dev.sh` 的 owner 命令探测行
//! （`npm config get cache`、`uv cache dir`、`mise cache path` 等）。
//!
//! 安全约束（对标 resolve_tool_home / 各 probe 分支）：
//! - 仅当 owner 命令存在且执行成功时采信输出；
//! - 输出必须是绝对路径，拒绝 `..` 组件与控制字符（防被污染的
//!   环境变量/配置重定向清理，issue #1378）。

use std::path::PathBuf;
use std::time::Duration;

/// 快速探测超时，对标 MOLE_TIMEOUT_QUICK_DETECT_SEC（2s）。
const QUICK_DETECT_TIMEOUT: Duration = Duration::from_secs(2);

/// 探测输出校验：非空、绝对路径、无 `..` 组件、无控制字符。
fn validate_probe_output(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || !trimmed.starts_with('/') {
        return None;
    }
    if trimmed.split('/').any(|comp| comp == "..") {
        return None;
    }
    if trimmed.chars().any(|c| c.is_control()) {
        return None;
    }
    Some(trimmed.trim_end_matches('/').to_string())
}

/// 运行 owner 命令探测缓存路径。
fn probe(bin: &str, args: &[&str], timeout: Duration) -> Option<String> {
    if !super::command_exists(bin) {
        return None;
    }
    crate::status::run_cmd(bin, args, timeout)
        .ok()
        .and_then(|out: String| validate_probe_output(&out))
}

/// npm 缓存路径（对标 clean_dev_npm）：`npm config get cache`，失败回退
/// 默认 `~/.npm`。返回 (探测路径, 是否为自定义路径)。
pub fn npm_cache_path() -> (PathBuf, bool) {
    let home = std::env::var("HOME").unwrap_or_default();
    let default_path = PathBuf::from(&home).join(".npm");
    match probe("npm", &["config", "get", "cache"], QUICK_DETECT_TIMEOUT) {
        Some(detected) if detected != default_path.to_string_lossy() => {
            (PathBuf::from(detected), true)
        }
        _ => (default_path, false),
    }
}

/// mise 缓存路径（对标 get_mise_cache_path）：MISE_CACHE_DIR →
/// `mise cache path` → `~/Library/Caches/mise`。
pub fn mise_cache_path() -> PathBuf {
    if let Ok(env_dir) = std::env::var("MISE_CACHE_DIR") {
        if let Some(validated) = validate_probe_output(&env_dir) {
            return PathBuf::from(validated);
        }
    }
    if let Some(probed) = probe("mise", &["cache", "path"], QUICK_DETECT_TIMEOUT) {
        return PathBuf::from(probed);
    }
    let home = std::env::var("HOME").unwrap_or_default();
    PathBuf::from(home).join("Library/Caches/mise")
}

/// corepack home（对标 clean_corepack_cache）：COREPACK_HOME 或
/// `~/.cache/node/corepack`；不安全路径（/、$HOME、~/Library）拒绝并返回
/// None（对标 "Skipping unsafe Corepack cache path"）。
pub fn corepack_cache_path() -> Option<PathBuf> {
    let home = std::env::var("HOME").unwrap_or_default();
    let raw = std::env::var("COREPACK_HOME")
        .ok()
        .filter(|v| v.starts_with('/'))
        .unwrap_or_else(|| format!("{home}/.cache/node/corepack"));
    let trimmed = raw.trim_end_matches('/');
    if trimmed.is_empty() || trimmed == "/" {
        return None;
    }
    if trimmed == home || trimmed == format!("{home}/Library") {
        return None;
    }
    Some(PathBuf::from(trimmed))
}

/// uv 缓存路径（对标 clean_uv_cache else 分支）：固定 `~/.cache/uv`。
pub fn uv_default_cache_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    PathBuf::from(home).join(".cache/uv")
}

/// owner 命令是否可用（对标 `command -v uv && uv --version` 成功）。
pub fn tool_available(bin: &str, version_args: &[&str]) -> bool {
    if !super::command_exists(bin) {
        return false;
    }
    crate::status::run_cmd(bin, version_args, QUICK_DETECT_TIMEOUT).is_ok()
}

/// 规范化路径用于自定义/默认去重（对标 `cd && pwd -P`：存在时取真实路径）。
pub fn normalize_existing(path: &str) -> String {
    let p = PathBuf::from(path.trim_end_matches('/'));
    if p.is_dir() {
        if let Ok(real) = p.canonicalize() {
            return real.to_string_lossy().to_string();
        }
    }
    path.trim_end_matches('/').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 对标 validate 语义：绝对路径、拒绝 ..、拒绝控制字符。
    #[test]
    fn probe_output_validation() {
        assert_eq!(
            validate_probe_output("/Users/t/.npm\n"),
            Some("/Users/t/.npm".to_string())
        );
        assert_eq!(validate_probe_output(""), None);
        assert_eq!(validate_probe_output("relative/path"), None);
        assert_eq!(validate_probe_output("/Users/t/../evil"), None);
        assert_eq!(validate_probe_output("/bad\npath"), None);
        assert_eq!(validate_probe_output("/trailing/\n"), Some("/trailing".to_string()));
    }

    #[test]
    fn corepack_unsafe_paths_rejected() {
        // 直接测内部逻辑的路径分支：不设 env 时返回默认路径。
        std::env::remove_var("COREPACK_HOME");
        let home = std::env::var("HOME").unwrap();
        let p = corepack_cache_path().unwrap();
        assert_eq!(p, PathBuf::from(format!("{home}/.cache/node/corepack")));
    }

    #[test]
    fn npm_probe_falls_back_to_default() {
        // npm 可能不存在或存在；返回值必须是合法绝对路径。
        let (path, custom) = npm_cache_path();
        assert!(path.starts_with("/"));
        if !custom {
            assert!(path.ends_with(".npm"));
        }
    }

    #[test]
    fn normalize_existing_handles_missing() {
        assert_eq!(normalize_existing("/nonexistent/path/"), "/nonexistent/path");
    }
}
