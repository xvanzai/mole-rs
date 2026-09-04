//! status 模块：系统监控指标采集。
//!
//! 对标 `Mole/cmd/status/*.go`。快照字段与 JSON 标签保持与原实现
//! `MetricsSnapshot` 一致，便于自动化消费方平滑迁移。
//!
//! 分层缓存节奏（对标 watch.go / main.go）：
//! - fast（1s）：CPU、内存、磁盘 statfs、网络速率；
//! - process（1s）：ps 进程采样；
//! - full（30s）：硬件、电池、热能、废纸篓、代理等慢变数据；
//! - 慢缓存：硬件 10min、system_profiler 30s、接口 IP 10s。

mod battery;
mod cpu;
mod disk;
mod hardware;
mod health;
mod memory;
mod network;
mod processes;
mod types;

pub use types::*;

/// 供各采集子模块复用模块1的 SI 单位格式化（对标 Go humanBytes）。
pub(crate) use crate::core::units::bytes_si as core_bytes_si;

use std::collections::HashMap;
use std::time::{Duration, Instant};

/// 网络历史环形缓冲大小，对标 `NetworkHistorySize`。
pub const NETWORK_HISTORY_SIZE: usize = 120;

/// 对标 `runCmd`：以 C locale 执行子进程（#1267：ps/uptime 会本地化小数点，
/// 导致解析失败），带超时，超时杀进程并返回错误。
pub fn run_cmd(name: &str, args: &[&str], timeout: Duration) -> Result<String, String> {
    let mut command = std::process::Command::new(name);
    command.args(args);
    // 对标 cLocaleEnv：过滤 LC_ALL / LANG / LC_* 后强制 LC_ALL=C。
    for key in ["LC_ALL", "LANG"] {
        command.env_remove(key);
    }
    for (key, _) in std::env::vars_os() {
        if let Some(key) = key.to_str() {
            if key.starts_with("LC_") {
                command.env_remove(key);
            }
        }
    }
    command.env("LC_ALL", "C");
    command.stdout(std::process::Stdio::piped());
    command.stderr(std::process::Stdio::null());
    command.stdin(std::process::Stdio::null());

    let mut child = command
        .spawn()
        .map_err(|e| format!("spawn {name}: {e}"))?;

    // 输出管道在独立线程读取，避免等待期间管道缓冲区写满死锁。
    let mut stdout = child.stdout.take().unwrap();
    let reader = std::thread::spawn(move || {
        use std::io::Read;
        let mut buf = String::new();
        let _ = stdout.read_to_string(&mut buf);
        buf
    });

    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let out = reader.join().unwrap_or_default();
                if status.success() {
                    return Ok(out);
                }
                return Err(format!("{name} exited with {status}"));
            }
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!("{name} timed out"));
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(e) => return Err(format!("{name}: {e}")),
        }
    }
}

fn command_exists(name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    // 对标 commandExists：PATH 查找并缓存结果。
    static CACHE: std::sync::OnceLock<Mutex2<HashMap<String, bool>>> = std::sync::OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex2::new(HashMap::new()));
    if let Some(exists) = cache.lock().get(name) {
        return *exists;
    }
    let exists = std::env::var_os("PATH")
        .map(|paths| {
            std::env::split_paths(&paths).any(|dir| {
                let p = dir.join(name);
                p.is_file() && is_executable(&p)
            })
        })
        .unwrap_or(false);
    cache.lock().insert(name.to_string(), exists);
    exists
}

fn is_executable(path: &std::path::Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path)
            .map(|m| m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        path.exists()
    }
}

/// 简化版互斥锁包装（避免为缓存引入跨平台锁细节）。
struct Mutex2<T>(std::sync::Mutex<T>);

impl<T> Mutex2<T> {
    fn new(v: T) -> Self {
        Self(std::sync::Mutex::new(v))
    }
    fn lock(&self) -> std::sync::MutexGuard<'_, T> {
        self.0.lock().unwrap_or_else(|p| p.into_inner())
    }
}

/// 对标 `formatUptime`：>1 天只显示天与小时，>1 小时显示小时与分钟。
pub fn format_uptime(secs: u64) -> String {
    let days = secs / 86400;
    let hours = (secs % 86400) / 3600;
    let mins = (secs % 3600) / 60;
    if days > 0 {
        format!("{days}d {hours}h")
    } else if hours > 0 {
        format!("{hours}h {mins}m")
    } else {
        format!("{mins}m")
    }
}

/// 采集节奏状态机，对标 `watchState` / `nextCollectionMode`：
/// 首次 fast 预热，随后每 30s 一次 full、每 1s 一次 process，其余 fast。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CollectionMode {
    Fast,
    Process,
    Full,
}

/// 指标采集器，对标 Go `Collector`：持有多级缓存与上次网络计数，
/// 保证速率类指标跨采样准确。
pub struct Collector {
    // CPU fast 路径的上次 tick 快照（对标 gopsutil cpu.Percent(0) 内部缓存）。
    prev_cpu_ticks: Option<Vec<[f64; 4]>>,

    // 硬件静态缓存（10 分钟），对标 cachedHW/lastHWAt。
    cached_hw: Option<HardwareInfo>,
    last_hw_at: Option<Instant>,

    // 网络速率状态（对标 prevNet/lastNetAt/rxHistoryBuf/cachedNetIPs）。
    prev_net: HashMap<String, (u64, u64)>,
    last_net_at: Option<Instant>,
    rx_history: RingBuffer,
    tx_history: RingBuffer,
    cached_net_ips: HashMap<String, String>,
    last_net_ip_at: Option<Instant>,

    // system_profiler 缓存（30s），对标 cachedPower/cachedPowerJSON。
    power_cache: battery::PowerCache,

    // 废纸篓大小缓存（5s），对标 trashSizeCache。
    trash_cache: Option<(u64, bool, Instant)>,

    // 节奏状态（对标 watchState）。
    ready: bool,
    last_full_at: Option<Instant>,
    last_process_at: Option<Instant>,

    // 进程数据缓存（对标 processEnrichment）。
    process_enrichment: Option<ProcessEnrichment>,
}

#[derive(Clone)]
struct ProcessEnrichment {
    top_processes: Vec<ProcessInfo>,
    zombie_count: i64,
    zombie_parents: Vec<ZombieParent>,
    zombie_parents_complete: bool,
}

/// 定长环形缓冲，对标 Go `RingBuffer`（按时间序输出旧→新）。
pub struct RingBuffer {
    data: Vec<f64>,
    index: usize,
    size: usize,
    cap: usize,
}

impl RingBuffer {
    pub fn new(cap: usize) -> Self {
        Self {
            data: vec![0.0; cap],
            index: 0,
            size: 0,
            cap,
        }
    }

    pub fn add(&mut self, val: f64) {
        self.data[self.index] = val;
        self.index = (self.index + 1) % self.cap;
        if self.size < self.cap {
            self.size += 1;
        }
    }

    /// 按时间序返回（旧→新），对标 `Slice`。
    pub fn slice(&self) -> Vec<f64> {
        if self.size == 0 {
            return Vec::new();
        }
        if self.size < self.cap {
            return self.data[..self.size].to_vec();
        }
        let mut res = self.data[self.index..].to_vec();
        res.extend_from_slice(&self.data[..self.index]);
        res
    }
}

fn now_unix() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

impl Collector {
    pub fn new() -> Self {
        Self {
            prev_cpu_ticks: None,
            cached_hw: None,
            last_hw_at: None,
            prev_net: HashMap::new(),
            last_net_at: None,
            rx_history: RingBuffer::new(NETWORK_HISTORY_SIZE),
            tx_history: RingBuffer::new(NETWORK_HISTORY_SIZE),
            cached_net_ips: HashMap::new(),
            last_net_ip_at: None,
            power_cache: battery::PowerCache::default(),
            trash_cache: None,
            ready: false,
            last_full_at: None,
            last_process_at: None,
            process_enrichment: None,
        }
    }

    /// 对标 `nextCollectionMode`。
    fn next_mode(&self) -> CollectionMode {
        const SLOW_REFRESH: Duration = Duration::from_secs(30);
        const PROCESS_INTERVAL: Duration = Duration::from_secs(1);
        if !self.ready {
            return CollectionMode::Fast;
        }
        let now = Instant::now();
        if self.last_full_at.is_none_or(|t| now.duration_since(t) >= SLOW_REFRESH) {
            return CollectionMode::Full;
        }
        if self
            .last_process_at
            .is_none_or(|t| now.duration_since(t) >= PROCESS_INTERVAL)
        {
            return CollectionMode::Process;
        }
        CollectionMode::Fast
    }

    /// 对标 `watchState.collect`：按节奏选择 fast/process/full。
    pub fn tick(&mut self) -> MetricsSnapshot {
        let mode = self.next_mode();
        let snap = match mode {
            CollectionMode::Full => self.collect_full(),
            CollectionMode::Process => self.collect_with_processes(true),
            CollectionMode::Fast => self.collect_with_processes(false),
        };
        match mode {
            CollectionMode::Full => self.last_full_at = Some(Instant::now()),
            CollectionMode::Process => self.last_process_at = Some(Instant::now()),
            CollectionMode::Fast => {}
        }
        self.ready = true;
        snap
    }

    /// fast/process 路径（对标 CollectFast / CollectProcesses）：
    /// 快变指标实时采集，慢变字段从缓存继承。
    fn collect_with_processes(&mut self, include_processes: bool) -> MetricsSnapshot {
        let mut snap = self.collect_fast();
        if include_processes {
            let procs = processes::collect_processes();
            self.apply_process_data(&mut snap, &procs, false);
            self.cache_process_data(&snap);
        } else if let Some(cache) = &self.process_enrichment {
            // 对标 processEnrichment.apply：继承上次进程数据并标记 stale。
            snap.top_processes = cache.top_processes.clone();
            snap.process_collected_at = Some(now_unix());
            snap.process_stale = Some(true);
            snap.zombie_count = Some(cache.zombie_count);
            snap.zombie_parents = cache.zombie_parents.clone();
            snap.zombie_parents_complete = Some(cache.zombie_parents_complete);
        }
        snap
    }

    fn collect_fast(&mut self) -> MetricsSnapshot {
        let cpu = cpu::collect_cpu_fast(&mut self.prev_cpu_ticks);
        let memory = memory::collect_memory(false);
        let disks = disk::collect_disks(false);
        let network = self.collect_network();
        let uptime_secs = boot_uptime_secs();
        let (health_score, health_score_msg) = health::calculate_health_score(
            &cpu,
            &memory,
            &disks,
            &[0.0, 0.0],
            &ThermalStatus::default(),
            uptime_secs,
        );

        MetricsSnapshot {
            collected_at: now_unix(),
            host: host_name(),
            platform: platform_name(),
            uptime: format_uptime(uptime_secs),
            uptime_seconds: uptime_secs,
            procs: process_count(),
            hardware: self.cached_hw.clone().unwrap_or_default(),
            health_score,
            health_score_msg,
            cpu,
            gpu: Vec::new(),
            memory,
            disks,
            trash_size: 0,
            trash_approx: false,
            disk_io: DiskIoStatus::default(),
            network,
            network_history: NetworkHistory {
                rx_history: self.rx_history.slice(),
                tx_history: self.tx_history.slice(),
            },
            proxy: ProxyStatus::default(),
            batteries: Vec::new(),
            thermal: ThermalStatus::default(),
            sensors: Vec::new(),
            bluetooth: Vec::new(),
            top_processes: Vec::new(),
            process_collected_at: None,
            process_stale: None,
            zombie_count: None,
            zombie_parents: Vec::new(),
            zombie_parents_complete: None,
        }
    }

    /// full 路径（对标 Collect / collectFull）：补充硬件、电池、热能、
    /// 废纸篓、代理与进程等慢变数据，并刷新缓存供 fast 路径继承。
    fn collect_full(&mut self) -> MetricsSnapshot {
        let cpu = cpu::collect_cpu_full(&mut self.prev_cpu_ticks);
        let memory = memory::collect_memory(true);
        let disks = disk::collect_disks(true);
        let network = self.collect_network();
        let trash = self.collect_trash();
        let batteries = battery::collect_batteries(&mut self.power_cache);
        let thermal = battery::collect_thermal(&mut self.power_cache);
        let proxy = network::collect_proxy();

        // 硬件信息缓存 10 分钟（对标 snapshotFromMetrics 的 refreshHardware）。
        let hw_expired = self
            .last_hw_at
            .is_none_or(|t| t.elapsed() > Duration::from_secs(600));
        if hw_expired {
            self.cached_hw = Some(hardware::collect_hardware(
                memory.total,
                disks.first().map(|d| d.total),
            ));
            self.last_hw_at = Some(Instant::now());
        }

        let uptime_secs = boot_uptime_secs();
        let (health_score, health_score_msg) = health::calculate_health_score_with_batteries(
            &cpu,
            &memory,
            &disks,
            &[0.0, 0.0],
            &thermal,
            &batteries,
            uptime_secs,
        );

        let mut snap = MetricsSnapshot {
            collected_at: now_unix(),
            host: host_name(),
            platform: platform_name(),
            uptime: format_uptime(uptime_secs),
            uptime_seconds: uptime_secs,
            procs: process_count(),
            hardware: self.cached_hw.clone().unwrap_or_default(),
            health_score,
            health_score_msg,
            cpu,
            gpu: Vec::new(),
            memory,
            disks,
            trash_size: trash.0,
            trash_approx: trash.1,
            disk_io: DiskIoStatus::default(),
            network,
            network_history: NetworkHistory {
                rx_history: self.rx_history.slice(),
                tx_history: self.tx_history.slice(),
            },
            proxy,
            batteries,
            thermal,
            sensors: Vec::new(),
            bluetooth: Vec::new(),
            top_processes: Vec::new(),
            process_collected_at: None,
            process_stale: None,
            zombie_count: None,
            zombie_parents: Vec::new(),
            zombie_parents_complete: None,
        };

        let procs = processes::collect_processes();
        self.apply_process_data(&mut snap, &procs, true);
        self.cache_process_data(&snap);
        snap
    }

    fn apply_process_data(
        &mut self,
        snap: &mut MetricsSnapshot,
        procs: &[ProcessInfo],
        _full: bool,
    ) {
        let parents_available = procs.iter().any(|p| p.ppid > 0);
        snap.top_processes = processes::top_processes(procs, 5);
        snap.process_collected_at = Some(now_unix());
        snap.process_stale = Some(false);
        let (count, parents, complete) =
            processes::summarize_zombies(procs, processes::ZOMBIE_PARENT_LIMIT, parents_available);
        snap.zombie_count = Some(count);
        snap.zombie_parents = parents;
        snap.zombie_parents_complete = Some(complete);
    }

    fn cache_process_data(&mut self, snap: &MetricsSnapshot) {
        let Some(count) = snap.zombie_count else {
            return;
        };
        self.process_enrichment = Some(ProcessEnrichment {
            top_processes: snap.top_processes.clone(),
            zombie_count: count,
            zombie_parents: snap.zombie_parents.clone(),
            zombie_parents_complete: snap.zombie_parents_complete.unwrap_or(false),
        });
    }

    /// 废纸篓大小（5s 缓存，对标 collectTrashSize）。
    fn collect_trash(&mut self) -> (u64, bool) {
        if let Some((value, approx, at)) = self.trash_cache {
            if at.elapsed() < Duration::from_secs(5) {
                return (value, approx);
            }
        }
        let result = disk::scan_trash_size();
        self.trash_cache = Some((result.0, result.1, Instant::now()));
        result
    }

    /// 网络速率（对标 collectNetwork：噪声过滤、速率窗口下限 100ms、
    /// Top3 排序、历史缓冲、IP 10s 缓存）。
    fn collect_network(&mut self) -> Vec<NetworkStatus> {
        let now = Instant::now();
        let counters = network::interface_counters();
        let ips = if self
            .last_net_ip_at
            .is_none_or(|t| now.duration_since(t) >= Duration::from_secs(10))
        {
            self.cached_net_ips = network::interface_ips();
            self.last_net_ip_at = Some(now);
            self.cached_net_ips.clone()
        } else {
            self.cached_net_ips.clone()
        };

        let Some(counters) = counters else {
            self.rx_history.add(0.0);
            self.tx_history.add(0.0);
            return Vec::new();
        };

        let Some(last_net_at) = self.last_net_at else {
            self.last_net_at = Some(now);
            self.prev_net = counters;
            return Vec::new();
        };

        let elapsed = now.duration_since(last_net_at).as_secs_f64().max(0.1);
        let mut result = Vec::new();
        for (name, (rx, tx)) in &counters {
            if network::is_noise_interface(name) {
                continue;
            }
            let Some((prev_rx, prev_tx)) = self.prev_net.get(name) else {
                continue;
            };
            result.push(NetworkStatus {
                name: name.clone(),
                rx_rate_mbs: network::counter_delta(*rx, *prev_rx) as f64 / 1024.0 / 1024.0 / elapsed,
                tx_rate_mbs: network::counter_delta(*tx, *prev_tx) as f64 / 1024.0 / 1024.0 / elapsed,
                ip: ips.get(name).cloned().unwrap_or_default(),
            });
        }

        self.last_net_at = Some(now);
        self.prev_net = counters;

        result.sort_by(|a, b| {
            (b.rx_rate_mbs + b.tx_rate_mbs)
                .partial_cmp(&(a.rx_rate_mbs + a.tx_rate_mbs))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        result.truncate(3);

        let (total_rx, total_tx) = result
            .iter()
            .fold((0.0, 0.0), |(rx, tx), r| (rx + r.rx_rate_mbs, tx + r.tx_rate_mbs));
        self.rx_history.add(total_rx);
        self.tx_history.add(total_tx);

        result
    }
}

fn boot_uptime_secs() -> u64 {
    // 对标 gopsutil host.Info 的 Uptime：kern.boottime 距今秒数。
    match crate::status::memory::sysctl_boottime_secs() {
        Some(boot) => {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            now.saturating_sub(boot)
        }
        None => 0,
    }
}

fn host_name() -> String {
    // 对标 hostInfo.Hostname。
    let mut buf = [0u8; 256];
    unsafe {
        if libc::gethostname(buf.as_mut_ptr() as *mut libc::c_char, buf.len()) == 0 {
            let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
            return String::from_utf8_lossy(&buf[..end]).into_owned();
        }
    }
    String::new()
}

fn platform_name() -> String {
    // 对标 fmt.Sprintf("%s %s", Platform, PlatformVersion)，如 "darwin 26.0"。
    let version = memory::sysctl_string("kern.osproductversion").unwrap_or_default();
    format!("darwin {version}")
}

fn process_count() -> u64 {
    // 对标 hostInfo.Procs：kern.proc.all 返回 kinfo_proc 数组。
    unsafe {
        let mut size: libc::size_t = 0;
        let name = b"kern.proc.all\0";
        if libc::sysctlbyname(
            name.as_ptr() as *const libc::c_char,
            std::ptr::null_mut(),
            &mut size,
            std::ptr::null_mut(),
            0,
        ) == 0
            && size > 0
        {
            (size as u64) / (kinfo_proc_bytes() as u64)
        } else {
            0
        }
    }
}

/// sizeof(kinfo_proc)（libc crate 未提供，由 build.rs 构建期计算）。
fn kinfo_proc_bytes() -> usize {
    match option_env!("KINFO_PROC_BYTES") {
        Some(v) => v.parse().unwrap_or(648),
        None => 648,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_buffer_orders_chronologically() {
        // 对标 Go TestRingBuffer 语义：部分填充按序返回，填满后环绕拼接。
        let mut rb = RingBuffer::new(5);
        assert!(rb.slice().is_empty());
        for v in [1.0, 2.0, 3.0] {
            rb.add(v);
        }
        assert_eq!(rb.slice(), vec![1.0, 2.0, 3.0]);
        for v in [4.0, 5.0, 6.0, 7.0] {
            rb.add(v);
        }
        // data: [6,7,3,4,5] index=2 → 期望 [3,4,5,6,7]
        assert_eq!(rb.slice(), vec![3.0, 4.0, 5.0, 6.0, 7.0]);
    }

    #[test]
    fn format_uptime_matches_go() {
        assert_eq!(format_uptime(59), "0m");
        assert_eq!(format_uptime(60), "1m");
        assert_eq!(format_uptime(3_700), "1h 1m");
        assert_eq!(format_uptime(90_060), "1d 1h");
    }
}

/// 真机冒烟测试（默认忽略）：验证各采集路径在本机可用。
/// 运行：`cargo test -- --ignored --nocapture`
#[cfg(test)]
mod smoke_tests {
    use super::*;

    #[test]
    #[ignore]
    fn real_collection_smoke() {
        let mut collector = Collector::new();
        let snap = collector.tick();
        println!("host={} platform={}", snap.host, snap.platform);
        println!(
            "cpu: usage={:.1} cores={} logical={} P={}+E={}",
            snap.cpu.usage, snap.cpu.core_count, snap.cpu.logical_cpu, snap.cpu.p_core_count, snap.cpu.e_core_count
        );
        println!(
            "mem: {:.1}% used ({}/{})",
            snap.memory.used_percent,
            crate::core::units::bytes_bin(snap.memory.used),
            crate::core::units::bytes_bin(snap.memory.total)
        );
        for d in &snap.disks {
            println!("disk {} ({}): {:.1}% used", d.mount, d.fstype, d.used_percent);
        }
        for n in &snap.network {
            println!("net {}: rx {:.2} MB/s tx {:.2} MB/s ip {}", n.name, n.rx_rate_mbs, n.tx_rate_mbs, n.ip);
        }
        let snap2 = collector.tick();
        println!(
            "health={} ({}) procs={} top1={:?} battery={:?} proxy={:?} trash={}",
            snap2.health_score,
            snap2.health_score_msg,
            snap2.procs,
            snap2.top_processes.first().map(|p| (p.name.clone(), p.cpu)),
            snap2.batteries.first().map(|b| (b.percent, b.status.clone(), b.health.clone())),
            (snap2.proxy.enabled, snap2.proxy.kind.clone(), snap2.proxy.host.clone()),
            crate::core::units::bytes_bin(snap2.trash_size),
        );
        assert!(snap.cpu.core_count > 0);
        assert!(snap.memory.total > 0);
        assert!(!snap.disks.is_empty());
    }
}
