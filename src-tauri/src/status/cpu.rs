//! CPU 指标，对标 `cmd/status/metrics_cpu.go`。
//!
//! 原实现经 gopsutil 读取 mach `host_processor_info` 的 tick 分解；
//! 这里直接调用同一系统接口，因此 tick 语义（user/system/idle/nice）
//! 与 #1237 修复（denominator 以墙钟窗口兜底）可 1:1 移植。

use super::types::CpuStatus;
use std::time::Duration;

/// 对标 `cpuSampleInterval`。
const CPU_SAMPLE_INTERVAL: Duration = Duration::from_millis(100);

/// 对标 `hw.perflevel` 拓扑缓存 TTL。
const TOPOLOGY_TTL: Duration = Duration::from_secs(600);

/// `PROCESSOR_CPU_LOAD_INFO`（mach/host_info.h）。
const PROCESSOR_CPU_LOAD_INFO: i32 = 2;

/// tick 顺序：user, system, idle, nice（processor_cpu_load_info.cpu_ticks）。
const TICK_USER: usize = 0;
const TICK_SYSTEM: usize = 1;
const TICK_IDLE: usize = 2;
const TICK_NICE: usize = 3;

unsafe extern "C" {
    /// libsystem_kernel：mach_host_self()（真实函数）。
    fn mach_host_self() -> u32;
    /// libsystem_kernel：host_processor_info。
    fn host_processor_info(
        host: u32,
        flavor: i32,
        out_processor_count: *mut u32,
        out_processor_info: *mut *mut i32,
        out_processor_info_count: *mut u32,
    ) -> i32;
    /// libsystem_kernel：vm_deallocate（释放 mach 分配的缓冲区）。
    fn vm_deallocate(target_task: u32, address: usize, size: usize) -> i32;
}

/// mach_task_self 是全局变量 `mach_task_self_` 的宏，mach2 正确封装了它。
fn task_self() -> u32 {
    unsafe { mach2::traps::mach_task_self() }
}

/// mach tick 频率（gopsutil darwin ClocksPerSec = 100）。
const TICK_SECONDS: f64 = 0.01;

/// 读取当前 per-core tick 累计值（单位：秒，与 gopsutil `cpu.Times` 一致）。
fn read_cpu_ticks() -> Option<Vec<[f64; 4]>> {
    unsafe {
        let host = mach_host_self();
        let mut count: u32 = 0;
        let mut info: *mut i32 = std::ptr::null_mut();
        let mut info_count: u32 = 0;
        let kr = host_processor_info(
            host,
            PROCESSOR_CPU_LOAD_INFO,
            &mut count,
            &mut info,
            &mut info_count,
        );
        if kr != 0 || info.is_null() || count == 0 {
            return None;
        }
        // 每个 CPU 4 个 integer_t（user/system/idle/nice）。
        let words_per_cpu = 4usize;
        let mut cores = Vec::with_capacity(count as usize);
        for i in 0..count as usize {
            let base = i * words_per_cpu;
            let tick = |idx: usize| *info.add(base + idx) as f64 * TICK_SECONDS;
            cores.push([tick(TICK_USER), tick(TICK_SYSTEM), tick(TICK_IDLE), tick(TICK_NICE)]);
        }
        let addr = info as usize;
        let size = count as usize * words_per_cpu * std::mem::size_of::<i32>();
        vm_deallocate(task_self(), addr, size);
        Some(cores)
    }
}

/// busy 时间 = user + system + nice（对标 `cpuBusyTime`；
/// macOS 无 Irq/Softirq/Steal 计数，gopsutil darwin 亦为 0）。
fn cpu_busy_ticks(ticks: &[f64; 4]) -> f64 {
    ticks[TICK_USER] + ticks[TICK_SYSTEM] + ticks[TICK_NICE]
}

/// 1:1 移植 `perCoreUsageFromTimes`：两次 tick 快照 → per-core 百分比与
/// tick 加权总使用率。每个核的分母以墙钟窗口兜底（#1237：被驻停的
/// Apple Silicon E-core 不再累计 idle tick，原始差值会把睡眠核读成
/// 约 100% busy）。
pub fn per_core_usage_from_times(
    before: &[[f64; 4]],
    after: &[[f64; 4]],
    elapsed: f64,
) -> Result<(Vec<f64>, f64), String> {
    if before.is_empty() || before.len() != after.len() {
        return Err("mismatched cpu times snapshots".into());
    }
    if elapsed <= 0.0 {
        return Err("non-positive sampling window".into());
    }

    let mut percents = Vec::with_capacity(before.len());
    let mut busy_sum = 0.0f64;
    let mut window_sum = 0.0f64;
    for (b, a) in before.iter().zip(after.iter()) {
        let mut busy = cpu_busy_ticks(a) as f64 - cpu_busy_ticks(b) as f64;
        if busy < 0.0 {
            busy = 0.0;
        }
        let total = busy + (a[TICK_IDLE] as f64 - b[TICK_IDLE] as f64);
        let window = total.max(elapsed);
        let usage = (busy / window * 100.0).clamp(0.0, 100.0);
        percents.push(usage);
        busy_sum += busy;
        window_sum += window;
    }
    if window_sum <= 0.0 {
        return Err("empty cpu sampling window".into());
    }
    let total = (busy_sum / window_sum * 100.0).clamp(0.0, 100.0);
    Ok((percents, total))
}

/// fast 路径（对标 `collectCPUFast` / gopsutil `cpu.Percent(0, true)`）：
/// 使用距上次调用的 tick 差值（分母为 busy+idle 原始差值，窗口即调用
/// 间隔，不需要墙钟兜底），总使用率取 per-core 均值；首次调用返回
/// 零值并预热缓存（gopsutil 语义）。
pub fn collect_cpu_fast(prev: &mut Option<Vec<[f64; 4]>>) -> CpuStatus {
    let logical = logical_cpu_count();
    let mut status = CpuStatus {
        core_count: physical_cpu_count(),
        logical_cpu: logical,
        ..Default::default()
    };

    let current = read_cpu_ticks();
    match (&current, prev.as_ref()) {
        (Some(current), Some(before)) if before.len() == current.len() => {
            let per: Vec<f64> = before
                .iter()
                .zip(current.iter())
                .map(|(b, a)| {
                    let busy = cpu_busy_ticks(a) - cpu_busy_ticks(b);
                    let idle = a[TICK_IDLE] - b[TICK_IDLE];
                    let denom = busy + idle;
                    if denom > 0.0 {
                        (busy / denom * 100.0).clamp(0.0, 100.0)
                    } else {
                        0.0
                    }
                })
                .collect();
            status.usage = per.iter().sum::<f64>() / per.len() as f64;
            status.per_core = per;
        }
        (Some(current), _) => {
            status.per_core_estimated = true;
            status.per_core = vec![0.0; current.len()];
        }
        (None, _) => {
            status.per_core_estimated = true;
            status.per_core = vec![0.0; (logical.max(1)) as usize];
        }
    }
    *prev = current;

    let load = load_avg();
    status.load1 = load.0;
    status.load5 = load.1;
    status.load15 = load.2;
    status
}

/// full 路径（对标 `collectCPU`）：显式两次快照采样 + 墙钟兜底 +
/// load 平均值回退 + P/E 拓扑。
pub fn collect_cpu_full(prev: &mut Option<Vec<[f64; 4]>>) -> CpuStatus {
    let logical = logical_cpu_count().max(1);
    let mut status = CpuStatus {
        core_count: physical_cpu_count(),
        logical_cpu: logical,
        ..Default::default()
    };

    if let Some(before) = read_cpu_ticks() {
        let start = std::time::Instant::now();
        std::thread::sleep(CPU_SAMPLE_INTERVAL);
        match read_cpu_ticks() {
            Some(after) => {
                let elapsed = start.elapsed().as_secs_f64();
                if let Ok((per, total)) = per_core_usage_from_times(&before, &after, elapsed) {
                    status.per_core = per;
                    status.usage = total;
                }
                *prev = Some(after);
            }
            None => {
                status.per_core_estimated = true;
                status.per_core = vec![0.0; logical as usize];
                *prev = Some(before);
            }
        }
    } else {
        status.per_core_estimated = true;
        status.per_core = vec![0.0; logical as usize];
    }

    let load = load_avg();
    if is_zero_load(&load) {
        // 对标 full 路径的 uptime 回退（fast 路径不做）。
        if let Some(fallback) = fallback_load_avg_from_uptime() {
            status.load1 = fallback.0;
            status.load5 = fallback.1;
            status.load15 = fallback.2;
        }
    } else {
        status.load1 = load.0;
        status.load5 = load.1;
        status.load15 = load.2;
    }

    let (p, e) = get_core_topology();
    status.p_core_count = p;
    status.e_core_count = e;
    status
}

/// 对标 `getCoreTopology`：sysctl 读取 P/E 核数，带 10 分钟缓存。
fn get_core_topology() -> (i64, i64) {
    static CACHE: std::sync::OnceLock<std::sync::Mutex<Option<(i64, i64, std::time::Instant)>>> =
        std::sync::OnceLock::new();
    let cache = CACHE.get_or_init(|| std::sync::Mutex::new(None));
    let mut guard = cache.lock().unwrap_or_else(|p| p.into_inner());
    if let Some((p, e, at)) = *guard {
        if (p > 0 || e > 0) && at.elapsed() < TOPOLOGY_TTL {
            return (p, e);
        }
    }

    let Ok(out) = super::run_cmd(
        "sysctl",
        &["-n", "hw.perflevel0.logicalcpu", "hw.perflevel1.logicalcpu"],
        Duration::from_millis(500),
    ) else {
        *guard = None;
        return (0, 0);
    };
    let (p, e) = parse_core_topology(&out);
    if p == 0 && e == 0 {
        return (0, 0);
    }
    *guard = Some((p, e, std::time::Instant::now()));
    (p, e)
}

/// 1:1 移植 `parseCoreTopology`：按行读取 perflevel0（最高性能档）与
/// perflevel1。档位顺序决定归类，而非档位名称（M5 的两档叫
/// "Super"/"Performance"，没有 "Efficiency"，按名称匹配会把 12 个
/// 高性能核误判为能效核）。
fn parse_core_topology(out: &str) -> (i64, i64) {
    let lines: Vec<&str> = out.trim().lines().collect();
    if lines.len() < 2 {
        return (0, 0);
    }
    let p: Option<i64> = lines[0].trim().parse().ok();
    let e: Option<i64> = lines[1].trim().parse().ok();
    match (p, e) {
        (Some(p), Some(e)) if p > 0 && e > 0 => (p, e),
        _ => (0, 0),
    }
}

/// 对标 `fallbackLoadAvgFromUptime`：从 uptime 输出解析 load averages。
fn fallback_load_avg_from_uptime() -> Option<(f64, f64, f64)> {
    if !super::command_exists("uptime") {
        return None;
    }
    let out = super::run_cmd("uptime", &[], Duration::from_millis(500)).ok()?;
    let mut idx = None;
    for marker in ["load averages:", "load average:"] {
        if let Some(pos) = out.rfind(marker) {
            idx = Some(pos + marker.len());
            break;
        }
    }
    let idx = idx?;
    let segment = out[idx..].trim();
    let mut values = Vec::new();
    for field in segment.split_whitespace() {
        let cleaned = field.trim_matches(|c| c == ',' || c == ';');
        if cleaned.is_empty() {
            continue;
        }
        if let Ok(v) = cleaned.parse::<f64>() {
            values.push(v);
            if values.len() == 3 {
                break;
            }
        }
    }
    if values.len() < 3 {
        return None;
    }
    Some((values[0], values[1], values[2]))
}

fn is_zero_load(load: &(f64, f64, f64)) -> bool {
    load.0 == 0.0 && load.1 == 0.0 && load.2 == 0.0
}

/// load average（对标 gopsutil `load.Avg`，darwin 经 getloadavg）。
fn load_avg() -> (f64, f64, f64) {
    unsafe {
        let mut values = [0.0f64; 3];
        let n = libc::getloadavg(values.as_mut_ptr(), 3);
        if n == 3 {
            (values[0], values[1], values[2])
        } else {
            (0.0, 0.0, 0.0)
        }
    }
}

fn logical_cpu_count() -> i64 {
    super::memory::sysctl_u64("hw.logicalcpu").unwrap_or(0) as i64
}

fn physical_cpu_count() -> i64 {
    super::memory::sysctl_u64("hw.physicalcpu").unwrap_or(0) as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 翻译自 metrics_cpu_test.go TestPerCoreUsageParkedCoreIsNotInflated：
    /// 驻停核在 0.1s 窗口只醒 0.02s 且全在忙，读数必须是 20% 而非 ~100%。
    #[test]
    fn parked_core_is_not_inflated() {
        let before = vec![[10.0, 5.0, 100.0, 0.0]];
        let after = vec![[10.01, 5.01, 100.0, 0.0]];
        let (percents, total) = per_core_usage_from_times(&before, &after, 0.1).unwrap();
        assert!((percents[0] - 20.0).abs() < 0.01, "got {}", percents[0]);
        assert!((total - 20.0).abs() < 0.01, "got {total}");
    }

    /// 翻译自 TestPerCoreUsageFullyCoveredWindowUnchanged：tick 差值覆盖
    /// 整个窗口时，墙钟兜底不改变经典 busy/(busy+idle) 结果。
    #[test]
    fn fully_covered_window_unchanged() {
        let before = vec![[1.0, 1.0, 10.0, 0.0]];
        let after = vec![[1.03, 1.02, 10.05, 0.0]];
        let (percents, total) = per_core_usage_from_times(&before, &after, 0.1).unwrap();
        assert!((percents[0] - 50.0).abs() < 0.01, "got {}", percents[0]);
        assert!((total - 50.0).abs() < 0.01, "got {total}");
    }

    /// 翻译自 TestPerCoreUsageTotalIsWindowWeighted：总使用率是
    /// busy-over-window 加权，不是 per-core 百分比的均值。
    #[test]
    fn total_is_window_weighted() {
        // core0 驻停：busy 增长 0.02s 但无 idle tick → 墙钟兜底 0.1s → 20%；
        // core1 空闲但 tick 覆盖更长窗口（busy 0.3 + idle 0.7 = 1.0s）→ 30%。
        let before = vec![[10.0, 0.0, 100.0, 0.0], [10.0, 0.0, 100.0, 0.0]];
        let after = vec![[10.02, 0.0, 100.0, 0.0], [10.3, 0.0, 100.7, 0.0]];
        let (percents, total) = per_core_usage_from_times(&before, &after, 0.1).unwrap();
        assert!((percents[0] - 20.0).abs() < 0.01, "got {}", percents[0]);
        assert!((percents[1] - 30.0).abs() < 0.01, "got {}", percents[1]);
        // busy_sum 0.32 / window_sum (0.1 + 1.0) ≈ 29.09%，
        // 而 per-core 均值 (20+30)/2 = 25%——总量是加权值而非均值。
        assert!((total - 29.09).abs() < 0.01, "got {total}");
    }

    /// 翻译自 metrics_cpu_test.go 的 parseCoreTopology 用例。
    #[test]
    fn core_topology_by_level_order() {
        assert_eq!(parse_core_topology("12\n4\n"), (12, 4));
        assert_eq!(parse_core_topology(""), (0, 0));
        assert_eq!(parse_core_topology("12\n"), (0, 0));
        assert_eq!(parse_core_topology("0\n4\n"), (0, 0));
    }

    /// 翻译自 metrics_cpu_test.go：M5 命名场景——档位名不含 "Efficiency"，
    /// 必须仍按 perflevel 顺序正确归类（这里通过顺序输入验证）。
    #[test]
    fn topology_survives_level_rename() {
        // 18 核 M5 Pro：12 高性能 + 4 高效率（名称无关，顺序决定）。
        assert_eq!(parse_core_topology("12\n4\n"), (12, 4));
    }
}
