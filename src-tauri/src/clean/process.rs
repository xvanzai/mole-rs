//! 进程状态三态探针，对标 `lib/core/base.sh` 的 `mole_pgrep_any` 与
//! `mole_clean_process_guard`。
//!
//! 契约（AGENTS.md）：`mole_clean_process_guard` 是探针三态的**唯一翻译层**
//! （0=运行中，1=未运行，2=无法判定）。状态 2 必须拒绝删除——折叠成
//! "未运行"会在进程实际活跃时删除其文件。本模块保持同一语义。

use std::time::Duration;

/// pgrep 探测超时（对标 MOLE_TIMEOUT_QUICK_DETECT_SEC 量级）。
const PGREP_TIMEOUT: Duration = Duration::from_secs(3);

/// 探针三态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessState {
    /// 0：至少一个模式命中。
    Running,
    /// 1：全部模式均成功执行且无命中。
    Idle,
    /// 2：无命中但至少一次探针无法完成（spawn 失败等）。
    Unknown,
}

/// 单个 pgrep 探测：返回 0=命中，1=无命中，其他=失败。
fn pgrep_once(selector: &str, pattern: &str) -> Option<i32> {
    // 对标 pgrep "$selector" "$pattern"；stdout/stderr 丢弃。
    let mut child = std::process::Command::new("pgrep")
        .arg(selector)
        .arg(pattern)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .stdin(std::process::Stdio::null())
        .spawn()
        .ok()?;
    let deadline = std::time::Instant::now() + PGREP_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Some(status.code().unwrap_or(-1)),
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(_) => return None,
        }
    }
}

/// 对标 `mole_pgrep_any`：任一模式命中 → Running；全部成功无命中 → Idle；
/// 至少一次探针失败且无命中 → Unknown。无 pgrep → Unknown。
///
/// `patterns` 为 (selector, pattern) 对，selector 为 `-x` / `-f`。
pub fn pgrep_any(patterns: &[(&str, &str)]) -> ProcessState {
    if patterns.is_empty() || !super::command_exists("pgrep") {
        return ProcessState::Unknown;
    }
    let mut any_probe_failed = false;
    for (selector, pattern) in patterns {
        match pgrep_once(selector, pattern) {
            Some(0) => return ProcessState::Running,
            Some(_) => {}
            None => any_probe_failed = true,
        }
    }
    if any_probe_failed {
        ProcessState::Unknown
    } else {
        ProcessState::Idle
    }
}

/// rust_build_process_state：cargo/rustc/rustdoc/clippy-driver/cargo-nextest。
pub fn rust_build_process_state() -> ProcessState {
    pgrep_any(&[
        ("-x", "cargo"),
        ("-x", "rustc"),
        ("-x", "rustdoc"),
        ("-x", "clippy-driver"),
        ("-x", "cargo-nextest"),
    ])
}

/// is_google_chrome_running：精确名 + Helper + bundle 路径模式。
pub fn google_chrome_process_state() -> ProcessState {
    pgrep_any(&[
        ("-x", "Google Chrome"),
        ("-x", "Google Chrome Helper"),
        ("-f", "/Google Chrome.app/"),
    ])
}

/// _firefox_process_state。
pub fn firefox_process_state() -> ProcessState {
    pgrep_any(&[("-x", "Firefox")])
}

/// Arc 浏览器（对标 pgrep -x "Arc"）。
pub fn arc_process_state() -> ProcessState {
    pgrep_any(&[("-x", "Arc")])
}

/// Brave Browser。
pub fn brave_process_state() -> ProcessState {
    pgrep_any(&[("-x", "Brave Browser")])
}

/// Microsoft Edge（精确名，不得匹配 Teams）。
pub fn microsoft_edge_process_state() -> ProcessState {
    pgrep_any(&[("-x", "Microsoft Edge")])
}

/// Dia 浏览器。
pub fn dia_process_state() -> ProcessState {
    pgrep_any(&[("-x", "Dia")])
}

/// Vivaldi。
pub fn vivaldi_process_state() -> ProcessState {
    pgrep_any(&[("-x", "Vivaldi")])
}

/// QQBrowser3。
pub fn qqbrowser3_process_state() -> ProcessState {
    pgrep_any(&[("-x", "QQBrowser3")])
}

/// UTM 虚拟机（对标 pgrep -x "UTM"）。
pub fn utm_process_state() -> ProcessState {
    pgrep_any(&[("-x", "UTM")])
}

/// Tart 虚拟机（对标 clean_tart_caches 的 mole_pgrep_any -x tart）。
pub fn tart_process_state() -> ProcessState {
    pgrep_any(&[("-x", "tart")])
}

/// pnpm（对标 pnpm_process_blocks_prune：Running/Unknown 均阻断 prune）。
/// 匹配调用程序而非 argv 子串（pnpm-lock.yaml 不得永久阻断，#1370）。
pub fn pnpm_process_state() -> ProcessState {
    pgrep_any(&[(
        "-f",
        "(^|/)pnpm(\\.cjs)?([[:space:]]|$)",
    )])
}

/// Dropbox。
pub fn dropbox_process_state() -> ProcessState {
    pgrep_any(&[("-x", "Dropbox")])
}

/// Google Drive。
pub fn google_drive_process_state() -> ProcessState {
    pgrep_any(&[("-x", "Google Drive")])
}

/// OneDrive。
pub fn onedrive_process_state() -> ProcessState {
    pgrep_any(&[("-x", "OneDrive")])
}

/// Mail（对标 _clean_mail_downloads 的 pgrep -x Mail）。
pub fn mail_process_state() -> ProcessState {
    pgrep_any(&[("-x", "Mail")])
}

/// 对标 `mole_clean_process_guard` 的翻译：仅 Idle 放行。
/// 返回 (是否放行, 拒绝原因)。
pub fn guard_allows(state: ProcessState) -> Result<(), &'static str> {
    match state {
        ProcessState::Idle => Ok(()),
        ProcessState::Running => Err("process running"),
        ProcessState::Unknown => Err("process state unknown"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 不存在的进程：Idle（pgrep 成功退出码 1）。
    #[test]
    fn pgrep_any_idle_on_missing_process() {
        if !super::super::command_exists("pgrep") {
            return;
        }
        assert_eq!(
            pgrep_any(&[("-x", "__mole_rs_no_such_proc__")]),
            ProcessState::Idle
        );
    }

    /// 自身进程名：Running（cargo test 进程含 "cargo"）。
    #[test]
    fn pgrep_any_running_on_self() {
        if !super::super::command_exists("pgrep") {
            return;
        }
        // pgrep -f 匹配完整命令行；用当前进程的可执行名。
        let self_name = std::env::current_exe()
            .ok()
            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_string()));
        if let Some(name) = self_name {
            // 短名可能过宽，仅断言不是 Unknown。
            let state = pgrep_any(&[("-f", &name)]);
            assert_ne!(state, ProcessState::Unknown);
        }
    }

    /// 空模式列表 → Unknown。
    #[test]
    fn pgrep_any_empty_is_unknown() {
        assert_eq!(pgrep_any(&[]), ProcessState::Unknown);
    }

    /// 翻译层：仅 Idle 放行。
    #[test]
    fn guard_translation() {
        assert!(guard_allows(ProcessState::Idle).is_ok());
        assert_eq!(
            guard_allows(ProcessState::Running),
            Err("process running")
        );
        assert_eq!(
            guard_allows(ProcessState::Unknown),
            Err("process state unknown")
        );
    }

    /// rust 探针不 panic。
    #[test]
    fn rust_probe_smoke() {
        let _ = rust_build_process_state();
    }
}
