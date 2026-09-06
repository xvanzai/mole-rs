//! manage 模块：设置管理，对标 `Mole lib/manage/whitelist.sh` 与
//! `lib/clean/project.sh` 的 purge_paths 配置。
//!
//! 读写语义与清理模块共享同一校验（系统路径拒绝、`//` 拒绝、去重、
//! `~` 展开），写回采用临时文件 + rename 原子写。

use serde::Serialize;
use std::io::Write;
use std::path::{Path, PathBuf};

/// 配置文件位置（与清理/卸载模块读取路径一致）。
fn config_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    PathBuf::from(home).join(".config/mole")
}

fn whitelist_file() -> PathBuf {
    config_dir().join("whitelist")
}

fn purge_paths_file() -> PathBuf {
    config_dir().join("purge_paths")
}

/// 校验单个白名单行（对标 load_mole_whitelist 的拒绝规则）。
fn validate_whitelist_line(line: &str) -> Result<(), String> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return Ok(()); // 注释与空行原样保留
    }
    if line.contains("//") {
        return Err(format!("拒绝含连续斜杠的行: {line}"));
    }
    // 对标 is_rejected_system_path 的拒绝集合。
    let exact = ["/", "/System", "/bin", "/sbin", "/usr/bin", "/usr/sbin", "/etc", "/var/db"];
    if exact.contains(&line) {
        return Err(format!("拒绝系统保护路径: {line}"));
    }
    let parents = ["/System", "/bin", "/sbin", "/usr/bin", "/usr/sbin", "/etc", "/var/db"];
    if parents.iter().any(|p| line.starts_with(&format!("{p}/"))) {
        return Err(format!("拒绝系统保护路径: {line}"));
    }
    Ok(())
}

/// 校验 purge_paths 行（对标 mole_purge_read_paths_config 的读取语义 +
/// 保守写入校验：绝对路径）。
fn validate_purge_path_line(line: &str) -> Result<(), String> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return Ok(());
    }
    let expanded = line
        .strip_prefix('~')
        .map(|rest| format!("{}{rest}", std::env::var("HOME").unwrap_or_default()))
        .unwrap_or_else(|| line.to_string());
    if !expanded.starts_with('/') {
        return Err(format!("路径必须是绝对路径: {line}"));
    }
    if expanded.split('/').any(|c| c == "..") {
        return Err(format!("路径不允许 .. 组件: {line}"));
    }
    Ok(())
}

/// 读取配置文件原文（含注释），返回 (行, 是否是有效条目)。
fn read_config(file: &Path) -> Vec<String> {
    std::fs::read_to_string(file)
        .map(|content| content.lines().map(|l| l.to_string()).collect())
        .unwrap_or_default()
}

/// 原子写回（tmp + rename）。
fn write_config(file: &Path, lines: &[String]) -> Result<(), String> {
    let parent = file.parent().ok_or("无父目录")?;
    std::fs::create_dir_all(parent).map_err(|e| format!("创建目录失败: {e}"))?;
    let tmp = parent.join(format!(
        ".{}.tmp.{}",
        file.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
        std::process::id()
    ));
    {
        let mut f = std::fs::File::create(&tmp).map_err(|e| format!("创建临时文件失败: {e}"))?;
        for line in lines {
            writeln!(f, "{line}").map_err(|e| format!("写入失败: {e}"))?;
        }
    }
    std::fs::rename(&tmp, file).map_err(|e| format!("替换配置文件失败: {e}"))
}

/// 白名单配置视图。
#[derive(Debug, Clone, Serialize)]
pub struct WhitelistConfig {
    pub path: String,
    pub entries: Vec<String>,
}

/// 读取白名单（原文行，含注释；前端过滤展示）。
pub fn get_whitelist() -> WhitelistConfig {
    let file = whitelist_file();
    WhitelistConfig {
        path: file.to_string_lossy().to_string(),
        entries: read_config(&file),
    }
}

/// 全量写回白名单（逐行校验，拒绝非法行）。
pub fn set_whitelist(lines: &[String]) -> Result<(), String> {
    for line in lines {
        validate_whitelist_line(line)?;
    }
    write_config(&whitelist_file(), lines)
}

/// purge_paths 配置视图。
#[derive(Debug, Clone, Serialize)]
pub struct PurgePathsConfig {
    pub path: String,
    pub entries: Vec<String>,
}

pub fn get_purge_paths() -> PurgePathsConfig {
    let file = purge_paths_file();
    PurgePathsConfig {
        path: file.to_string_lossy().to_string(),
        entries: read_config(&file),
    }
}

pub fn set_purge_paths(lines: &[String]) -> Result<(), String> {
    for line in lines {
        validate_purge_path_line(line)?;
    }
    write_config(&purge_paths_file(), lines)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whitelist_line_validation() {
        assert!(validate_whitelist_line("~/Library/Caches/MyApp").is_ok());
        assert!(validate_whitelist_line("# 注释").is_ok());
        assert!(validate_whitelist_line("").is_ok());
        assert!(validate_whitelist_line("/System/Library").is_err());
        assert!(validate_whitelist_line("/etc/hosts").is_err());
        assert!(validate_whitelist_line("/Users/t//x").is_err());
    }

    #[test]
    fn purge_path_validation() {
        assert!(validate_purge_path_line("~/Projects").is_ok());
        assert!(validate_purge_path_line("/var/www").is_ok());
        assert!(validate_purge_path_line("relative/path").is_err());
        assert!(validate_purge_path_line("/a/../b").is_err());
    }

    /// 写回：临时目录内原子写 + 往返一致。
    #[test]
    fn config_roundtrip() {
        let tmp = std::env::temp_dir().join(format!("mole_rs_cfg_{}", std::process::id()));
        let file = tmp.join("whitelist");
        let lines = vec!["# 测试".to_string(), "~/Library/Caches/X".to_string()];
        write_config(&file, &lines).unwrap();
        assert_eq!(read_config(&file), lines);
        std::fs::remove_dir_all(&tmp).ok();
    }
}
