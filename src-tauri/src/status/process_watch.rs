//! ProcessWatch 告警状态机，对标 `cmd/status/process_watch.go`。
//!
//! 语义（1:1）：
//! - 按 (pid, ppid, command) 三元组跟踪进程；
//! - CPU ≥ 阈值持续 ≥ window 才触发告警（防抖）；
//! - 进程消失或 CPU 回落到阈值下 → 清除跟踪；
//! - 快照按 active 优先、触发时间升序、CPU 降序、PID 升序排序。
//!
//! GUI 差异：CLI 长驻 watch 会话驱动；GUI 由 Collector 每次 full/process
//! 采集后调用 Update，告警随快照返回。

use super::types::ProcessInfo;
use serde::Serialize;
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// 对标 ProcessWatchOptions。
#[derive(Debug, Clone)]
pub struct ProcessWatchOptions {
    pub enabled: bool,
    pub cpu_threshold: f64,
    pub window: Duration,
}

impl Default for ProcessWatchOptions {
    fn default() -> Self {
        Self {
            enabled: false,
            cpu_threshold: 50.0,
            window: Duration::from_secs(60),
        }
    }
}

/// 对标 ProcessAlert。
#[derive(Debug, Clone, Serialize)]
pub struct ProcessAlert {
    pub pid: i64,
    pub name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub command: String,
    pub cpu: f64,
    pub threshold: f64,
    pub window: String,
    pub triggered_at: f64,
    pub status: String,
}

/// 进程身份键（对标 processIdentity）。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ProcessIdentity {
    pid: i64,
    ppid: i64,
    command: String,
}

#[derive(Debug, Clone)]
struct TrackedProcess {
    info: ProcessInfo,
    first_above: Option<Instant>,
    triggered_at: Option<Instant>,
    current_above: bool,
}

/// 对标 ProcessWatcher。
#[derive(Default)]
pub struct ProcessWatcher {
    options: ProcessWatchOptions,
    tracks: HashMap<ProcessIdentity, TrackedProcess>,
}

impl ProcessWatcher {
    #[allow(dead_code)] // 公开 API：命令层构造
    pub fn new(options: ProcessWatchOptions) -> Self {
        Self {
            options,
            tracks: HashMap::new(),
        }
    }

    #[allow(dead_code)] // 公开 API：UI/命令层配置阈值
    pub fn options_mut(&mut self) -> &mut ProcessWatchOptions {
        &mut self.options
    }

    /// 对标 Update：用本次进程列表刷新跟踪状态，返回当前活跃告警。
    pub fn update(&mut self, processes: &[ProcessInfo]) -> Vec<ProcessAlert> {
        if !self.options.enabled {
            return Vec::new();
        }
        let now = Instant::now();
        let mut seen: HashMap<ProcessIdentity, ()> = HashMap::new();

        for proc in processes {
            if proc.pid <= 0 {
                continue;
            }
            let key = ProcessIdentity {
                pid: proc.pid,
                ppid: proc.ppid,
                command: proc.command.clone(),
            };
            seen.insert(key.clone(), ());
            let entry = self
                .tracks
                .entry(key)
                .or_insert_with(|| TrackedProcess {
                    info: proc.clone(),
                    first_above: None,
                    triggered_at: None,
                    current_above: false,
                });
            entry.info = proc.clone();
            entry.current_above = proc.cpu >= self.options.cpu_threshold;

            if entry.current_above {
                if entry.first_above.is_none() {
                    entry.first_above = Some(now);
                }
                if let Some(first) = entry.first_above {
                    if now.duration_since(first) >= self.options.window && entry.triggered_at.is_none()
                    {
                        entry.triggered_at = Some(now);
                    }
                }
                continue;
            }
            // CPU 回落 → 清除。
            entry.first_above = None;
            entry.triggered_at = None;
        }

        // 消失的进程删除。
        self.tracks.retain(|k, _| seen.contains_key(k));

        self.snapshot()
    }

    /// 对标 Snapshot。
    fn snapshot(&self) -> Vec<ProcessAlert> {
        if !self.options.enabled {
            return Vec::new();
        }
        // 先按绝对 Instant 排序，再映射为 elapsed 秒（避免 elapsed 调用时刻差异）。
        let mut tracks: Vec<&TrackedProcess> = self
            .tracks
            .values()
            .filter(|t| t.current_above && t.triggered_at.is_some())
            .collect();
        tracks.sort_by(|a, b| {
            a.triggered_at
                .cmp(&b.triggered_at)
                .then(
                    b.info
                        .cpu
                        .partial_cmp(&a.info.cpu)
                        .unwrap_or(std::cmp::Ordering::Equal),
                )
                .then(a.info.pid.cmp(&b.info.pid))
        });
        tracks
            .into_iter()
            .map(|t| ProcessAlert {
                pid: t.info.pid,
                name: t.info.name.clone(),
                command: t.info.command.clone(),
                cpu: t.info.cpu,
                threshold: self.options.cpu_threshold,
                window: format!("{}s", self.options.window.as_secs()),
                triggered_at: t
                    .triggered_at
                    .map(|i| i.elapsed().as_secs_f64())
                    .unwrap_or(0.0),
                status: "active".into(),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proc(pid: i64, name: &str, cpu: f64) -> ProcessInfo {
        ProcessInfo {
            pid,
            ppid: 1,
            state: String::new(),
            name: name.into(),
            command: format!("/usr/bin/{name}"),
            cpu,
            memory: 0.0,
            memory_bytes: 0,
        }
    }

    fn watcher(threshold: f64, window: Duration) -> ProcessWatcher {
        ProcessWatcher::new(ProcessWatchOptions {
            enabled: true,
            cpu_threshold: threshold,
            window,
        })
    }

    /// 低于阈值不触发。
    #[test]
    fn no_alert_below_threshold() {
        let mut w = watcher(50.0, Duration::from_secs(0));
        let alerts = w.update(&[proc(1, "foo", 10.0)]);
        assert!(alerts.is_empty());
    }

    /// 零窗口：首次超过阈值即触发（window=0 满足 ≥）。
    #[test]
    fn zero_window_triggers_immediately() {
        let mut w = watcher(50.0, Duration::from_secs(0));
        let alerts = w.update(&[proc(1, "foo", 60.0)]);
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].name, "foo");
        assert_eq!(alerts[0].status, "active");
    }

    /// 进程消失 → 告警清除。
    #[test]
    fn process_gone_clears_alert() {
        let mut w = watcher(50.0, Duration::from_secs(0));
        let _ = w.update(&[proc(1, "foo", 60.0)]);
        assert_eq!(w.snapshot().len(), 1);
        let alerts = w.update(&[]);
        assert!(alerts.is_empty());
    }

    /// CPU 回落 → 告警清除。
    #[test]
    fn cpu_drop_clears_alert() {
        let mut w = watcher(50.0, Duration::from_secs(0));
        let _ = w.update(&[proc(1, "foo", 60.0)]);
        assert_eq!(w.snapshot().len(), 1);
        let alerts = w.update(&[proc(1, "foo", 10.0)]);
        assert!(alerts.is_empty());
    }

    /// 禁用时不产生告警。
    #[test]
    fn disabled_watcher() {
        let mut w = ProcessWatcher::new(ProcessWatchOptions::default()); // enabled=false
        let alerts = w.update(&[proc(1, "foo", 99.0)]);
        assert!(alerts.is_empty());
    }

    /// 排序：CPU 降序（同 triggered_at）。
    #[test]
    fn alert_sorting() {
        let mut w = watcher(50.0, Duration::from_secs(0));
        let _ = w.update(&[
            proc(2, "low", 60.0),
            proc(1, "high", 90.0),
        ]);
        let alerts = w.snapshot();
        assert_eq!(alerts.len(), 2);
        assert_eq!(alerts[0].name, "high");
        assert_eq!(alerts[1].name, "low");
    }
}
