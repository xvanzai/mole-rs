//! 安全删除，对标 `Mole lib/core/file_ops.sh`：
//! - `validate_path_for_deletion`：绝对路径、路径穿越、控制字符、符号链接
//!   目标与祖先链接重检、关键系统路径拒绝；
//! - `mole_delete` trash 模式：验证 → 尺寸捕获 → Trash 路由（trash CLI →
//!   Finder AppleScript → ~/.Trash 直移，fail-closed）→ 双日志；
//! - `log_operation`（operations.log）+ `_mole_delete_log`（deletions.log）。

use super::protect;
use std::io::Write;
use std::path::Path;
use std::time::Duration;

/// 子进程超时，对标 MOLE_TIMEOUT_DISK_VERIFY_SEC。
const TRASH_TIMEOUT: Duration = Duration::from_secs(10);

fn home() -> String {
    std::env::var("HOME").unwrap_or_default()
}

fn timestamp() -> String {
    // 对标 `date '+%Y-%m-%d %H:%M:%S'`（本地时间）。
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    format_epoch_local(now)
}

/// 本地时间格式化（不引入 chrono：用 libc localtime_r）。
fn format_epoch_local(epoch: i64) -> String {
    unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        let t = epoch as libc::time_t;
        if libc::localtime_r(&t, &mut tm).is_null() {
            return "unknown".into();
        }
        format!(
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
            tm.tm_year + 1900,
            tm.tm_mon + 1,
            tm.tm_mday,
            tm.tm_hour,
            tm.tm_min,
            tm.tm_sec
        )
    }
}

fn timestamp_rfc3339ish() -> String {
    // 对标 `date '+%Y-%m-%dT%H:%M:%S%z'`。
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        let t = now as libc::time_t;
        if libc::localtime_r(&t, &mut tm).is_null() {
            return "unknown".into();
        }
        let offset_min = tm.tm_gmtoff / 60;
        let (sign, abs) = if offset_min >= 0 { ('+', offset_min) } else { ('-', -offset_min) };
        format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}{}{:02}{:02}",
            tm.tm_year + 1900,
            tm.tm_mon + 1,
            tm.tm_mday,
            tm.tm_hour,
            tm.tm_min,
            tm.tm_sec,
            sign,
            abs / 60,
            abs % 60
        )
    }
}

fn append_line(file: &str, line: &str) {
    if let Some(parent) = Path::new(file).parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(file) {
        let _ = writeln!(f, "{line}");
    }
}

/// 对标 `log_operation`：`[YYYY-MM-DD HH:MM:SS] [clean] ACTION path (detail)`
/// 追加到 `~/Library/Logs/mole/operations.log`；`MO_NO_OPLOG=1` 禁用。
pub fn log_operation(command: &str, action: &str, path: &str, detail: &str) {
    if std::env::var("MO_NO_OPLOG").as_deref() == Ok("1") {
        return;
    }
    if path.is_empty() {
        return;
    }
    let file = format!("{}/Library/Logs/mole/operations.log", home());
    let mut line = format!("[{}] [{command}] {action} {path}", timestamp());
    if !detail.is_empty() {
        line.push_str(&format!(" ({detail})"));
    }
    append_line(&file, &line);
}

/// 对标 `log_operation_session_start`。
pub fn log_session_start(command: &str) {
    if std::env::var("MO_NO_OPLOG").as_deref() == Ok("1") {
        return;
    }
    let file = format!("{}/Library/Logs/mole/operations.log", home());
    append_line(
        &file,
        &format!(
            "\n# ========== {command} session started at {} ==========",
            timestamp()
        ),
    );
}

/// 对标 `log_operation_session_end`。
pub fn log_session_end(command: &str, items: usize, size_bytes: u64) {
    if std::env::var("MO_NO_OPLOG").as_deref() == Ok("1") {
        return;
    }
    let file = format!("{}/Library/Logs/mole/operations.log", home());
    append_line(
        &file,
        &format!(
            "# ========== {command} session ended at {}, {items} items, {} ==========",
            timestamp(),
            crate::core::units::bytes_bin(size_bytes)
        ),
    );
}

/// 对标 `_mole_delete_log`：TSV 取证日志
/// `ts \t mode \t size_kb \t status \t target` → `~/Library/Logs/mole/deletions.log`。
fn delete_log(mode: &str, size_kb: &str, status: &str, target: &str) {
    let file = format!("{}/Library/Logs/mole/deletions.log", home());
    append_line(
        &file,
        &format!(
            "{}\t{mode}\t{size_kb}\t{status}\t{target}",
            timestamp_rfc3339ish()
        ),
    );
}

/// 对标 `_mole_is_critical_deletion_path`：永不删除的关键系统路径。
/// 返回 true = 拒绝删除。
///
/// 语义 1:1：/usr/local 与 /opt/homebrew 下的具体条目可删（提前放行臂）；
/// 拒绝臂分"仅精确匹配"与"含子树"两类——/Users、/Library、/Applications
/// 等根本身拒绝，但用户家目录与其余子目录可删。
fn is_critical_deletion_path(path: &str) -> bool {
    // 提前放行（对标 case 首个 return 1 臂）。
    if path.starts_with("/usr/local/") || path.starts_with("/opt/homebrew/") {
        return false;
    }
    // 仅精确匹配的拒绝臂（case 中不带 `/*` 的臂）。
    const DENY_EXACT_ONLY: &[&str] = &[
        "/",
        "/Library",
        "/Library/Application Support",
        "/Applications",
        "/Volumes",
        "/opt",
        "/opt/homebrew",
        "/Users",
        "/Users/Shared",
    ];
    // 精确 + 子树的拒绝臂。
    const DENY_SUBTREE: &[&str] = &[
        "/bin",
        "/dev",
        "/sbin",
        "/usr",
        "/System",
        "/Library/Apple",
        "/Library/Extensions",
        "/Library/Keychains",
        "/Applications/Finder.app",
        "/Applications/Safari.app",
        "/Users/Guest",
    ];
    if DENY_EXACT_ONLY.contains(&path) {
        return true;
    }
    DENY_SUBTREE
        .iter()
        .any(|arm| path == *arm || path.starts_with(&format!("{arm}/")))
}

/// 对标 `validate_path_for_deletion`（Rust 侧覆盖原实现的主要分支）。
///
/// 返回 Err(reason) = 拒绝删除。
fn validate_path_for_deletion(path: &str) -> Result<(), String> {
    if path.is_empty() {
        return Err("empty path".into());
    }
    if !path.starts_with('/') {
        return Err("path must be absolute".into());
    }
    // 仅拒绝作为完整路径组件的 `..`（允许 "name..files" 这类目录名）。
    for comp in path.split('/') {
        if comp == ".." {
            return Err("path traversal not allowed".into());
        }
    }
    if path.chars().any(|c| c.is_control()) {
        return Err("contains control characters".into());
    }
    if is_critical_deletion_path(path) {
        return Err("critical system path".into());
    }
    // 符号链接：目标若指向关键路径则拒绝（对标 symlink 分支）。
    let p = Path::new(path);
    if p.is_symlink() {
        if let Ok(target) = std::fs::read_link(p) {
            let resolved = if target.is_absolute() {
                target
            } else {
                p.parent().unwrap_or(Path::new("/")).join(target)
            };
            let resolved_str = resolved.to_string_lossy().to_string();
            if is_critical_deletion_path(&resolved_str)
                || protect::should_protect_path(&resolved_str)
            {
                return Err("symlink points to protected path".into());
            }
        }
    }
    // 祖先符号链接重检（对标 ancestor-symlink guard）：
    // 父目录经真实解析后重新过关键路径与保护判定。
    let parent = p.parent().unwrap_or(Path::new("/"));
    if let Ok(resolved_parent) = parent.canonicalize() {
        let resolved_parent_str = resolved_parent.to_string_lossy().to_string();
        if resolved_parent_str != parent.to_string_lossy() {
            let leaf = p.file_name().map(|l| l.to_string_lossy().to_string()).unwrap_or_default();
            let resolved = format!("{resolved_parent_str}/{leaf}");
            if is_critical_deletion_path(&resolved) || protect::should_protect_path(&resolved) {
                return Err("resolves into a protected path".into());
            }
        }
    }
    Ok(())
}

/// Trash 路由（对标 `_mole_move_to_trash` 用户路径分支，fail-closed）：
/// 0. `MOLE_TEST_TRASH_DIR`（测试缝，直移到指定目录）；
/// 1. `trash` CLI（若安装）；2. Finder AppleScript；3. ~/.Trash 直移。
/// 任一步失败即失败——绝不回退到永久删除。
fn move_to_trash(path: &str) -> Result<(), String> {
    // 0. 测试缝（对标 MOLE_TEST_TRASH_DIR）。
    if let Ok(test_trash) = std::env::var("MOLE_TEST_TRASH_DIR") {
        std::fs::create_dir_all(&test_trash).map_err(|e| format!("test trash: {e}"))?;
        let name = Path::new(path)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let dest = Path::new(&test_trash).join(format!(
            "{name}.{}.{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0)
        ));
        return std::fs::rename(path, &dest).map_err(|e| format!("test trash move: {e}"));
    }

    // 1. trash CLI（Homebrew）。
    if super::command_exists("trash") {
        let status = std::process::Command::new("trash").arg(path).status();
        if matches!(status, Ok(s) if s.success()) {
            return Ok(());
        }
    }

    // 2. Finder AppleScript：路径经 argv 传入，特殊字符无法逃逸（对标）。
    let script = "on run argv\n    set p to POSIX file (item 1 of argv)\n    tell application \"Finder\"\n        delete p\n    end tell\nend run";
    let spawned = std::process::Command::new("osascript")
        .arg("-")
        .arg(path)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    if let Ok(mut child) = spawned {
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(script.as_bytes());
        }
        let deadline = std::time::Instant::now() + TRASH_TIMEOUT;
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    if status.success() {
                        return Ok(());
                    }
                    break;
                }
                Ok(None) => {
                    if std::time::Instant::now() >= deadline {
                        let _ = child.kill();
                        return Err("osascript timed out".into());
                    }
                    std::thread::sleep(std::time::Duration::from_millis(25));
                }
                Err(_) => break,
            }
        }
    }

    // 3. ~/.Trash 直移（对标 MOLE_TEST_TRASH_DIR 直移分支 + 用户 Trash 契约）。
    // 目标名带 pid+epoch 后缀避免冲突；不可写则失败（不回退 rm）。
    let trash = Path::new(&home()).join(".Trash");
    let name = Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    if name.is_empty() {
        return Err("no basename".into());
    }
    let dest = trash.join(format!(
        "{name}.{}.{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&trash).map_err(|e| format!("trash unavailable: {e}"))?;
    std::fs::rename(path, &dest).map_err(|e| format!("trash move failed: {e}"))?;
    Ok(())
}

/// 对标 `mole_delete`（trash 模式、用户级路径）：验证 → sink 复检 →
/// 尺寸捕获 → Trash 路由 → 双日志。
///
/// `command_name` 写入操作日志（对标 MOLE_CURRENT_COMMAND）。
pub fn delete_to_trash(path: &str, dry_run: bool, command_name: &str) -> super::DeleteOutcome {
    let size_bytes = super::path_size_with_deadline(
        Path::new(path),
        std::time::Instant::now() + std::time::Duration::from_secs(2),
    );
    let size_kb = if size_bytes > 0 {
        (size_bytes / 1024).max(1).to_string()
    } else {
        "0".to_string()
    };

    if let Err(reason) = validate_path_for_deletion(path) {
        delete_log("trash", &size_kb, "rejected", path);
        log_operation(command_name, "SKIPPED", path, &reason);
        return super::DeleteOutcome {
            path: path.into(),
            status: "skipped".into(),
            size_bytes: 0,
            detail: format!("validation: {reason}"),
        };
    }

    // Sink 复检（对标 safe_clean 在删除点的复查语义；E5RT 含在保护层内）。
    if protect::should_protect_path(path) {
        delete_log("trash", &size_kb, "rejected", path);
        log_operation(command_name, "SKIPPED", path, "protected");
        return super::DeleteOutcome {
            path: path.into(),
            status: "skipped".into(),
            size_bytes: 0,
            detail: "protected".into(),
        };
    }

    if dry_run {
        delete_log("trash", &size_kb, "dry-run", path);
        return super::DeleteOutcome {
            path: path.into(),
            status: "dry-run".into(),
            size_bytes,
            detail: String::new(),
        };
    }

    match move_to_trash(path) {
        Ok(()) => {
            delete_log("trash", &size_kb, "ok", path);
            log_operation(command_name, "TRASHED", path, &format!("{size_kb}KB"));
            super::DeleteOutcome {
                path: path.into(),
                status: "ok".into(),
                size_bytes,
                detail: String::new(),
            }
        }
        Err(e) => {
            // Trash 不可用时 fail-closed：拒绝永久删除（对标）。
            delete_log("trash", &size_kb, "trash-failed", path);
            log_operation(command_name, "SKIPPED", path, "trash-failed");
            super::DeleteOutcome {
                path: path.into(),
                status: "failed".into(),
                size_bytes: 0,
                detail: e,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 对标 validate_path_for_deletion 的拒绝分支。
    #[test]
    fn validation_rejects_dangerous_paths() {
        assert!(validate_path_for_deletion("").is_err());
        assert!(validate_path_for_deletion("relative/path").is_err());
        assert!(validate_path_for_deletion("/Users/t/../System/x").is_err());
        assert!(validate_path_for_deletion("/bad\npath").is_err());
        assert!(validate_path_for_deletion("/System").is_err());
        assert!(validate_path_for_deletion("/System/Library").is_err());
        assert!(validate_path_for_deletion("/Library/Keychains/x").is_err());
        assert!(validate_path_for_deletion("/Applications").is_err());
        assert!(validate_path_for_deletion("/Users").is_err());
        // 合法目录名含 ..（Firefox "name..files"）。
        assert!(validate_path_for_deletion("/Users/t/Library/Caches/name..files").is_ok());
        // Homebrew 根不可删，但根下条目可删。
        assert!(validate_path_for_deletion("/opt/homebrew").is_err());
        assert!(validate_path_for_deletion("/opt/homebrew/Cellar").is_ok());
        assert!(validate_path_for_deletion("/usr/local").is_err());
        assert!(validate_path_for_deletion("/usr/local/bin/tool").is_ok());
    }

    /// 对标 TSV 取证日志格式。
    #[test]
    fn forensic_log_line_format() {
        // 时间戳格式校验。
        let ts = timestamp_rfc3339ish();
        assert!(ts.len() >= 19 && ts.contains('T'), "got {ts}");
        let ts2 = timestamp();
        assert_eq!(ts2.len(), 19, "got {ts2}");
    }

    /// Trash 直移 + 日志 + dry-run 语义（临时目录内完成，不触真实数据）。
    #[test]
    fn delete_to_trash_dry_run_and_real() {
        let tmp = std::env::temp_dir().join(format!("mole_rs_del_{}", std::process::id()));
        let target_dir = tmp.join("cache-target");
        std::fs::create_dir_all(&target_dir).unwrap();
        std::fs::write(target_dir.join("f.bin"), vec![0u8; 2048]).unwrap();

        // 保护路径在 sink 被拒（对 /System 的删除即使 dry-run 也拒绝）。
        let blocked = delete_to_trash("/System/Library/Caches/x", true, "clean");
        assert_eq!(blocked.status, "skipped");

        // dry-run：文件保留。
        let outcome = delete_to_trash(&target_dir.to_string_lossy(), true, "clean");
        assert_eq!(outcome.status, "dry-run");
        assert_eq!(outcome.size_bytes, 2048);
        assert!(target_dir.exists(), "dry-run 不得删除文件");

        // 真实执行：文件被移入临时 Trash（MOLE_TEST_TRASH_DIR 对标：
        // 通过环境变量注入测试 Trash）。
        let test_trash = tmp.join("TestTrash");
        std::env::set_var("MOLE_TEST_TRASH_DIR", &test_trash);
        let outcome = delete_to_trash(&target_dir.to_string_lossy(), false, "clean");
        assert_eq!(outcome.status, "ok", "detail={}", outcome.detail);
        assert_eq!(outcome.size_bytes, 2048);
        assert!(!target_dir.exists());
        assert!(test_trash.exists());
        std::env::remove_var("MOLE_TEST_TRASH_DIR");
        std::fs::remove_dir_all(&tmp).ok();
    }
}
