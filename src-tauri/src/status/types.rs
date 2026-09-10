//! 快照数据结构，对标 `cmd/status/metrics.go` 中的 struct 定义，
//! JSON 字段名与 Go 标签保持一致。

use serde::Serialize;

/// 对标 `MetricsSnapshot`。
///
/// 差异：`collected_at` 与 `process_collected_at` 输出 Unix 秒（f64），
/// 原实现为 RFC3339 字符串——前端 `new Date()` 直接消费，见
/// `docs/migration/CHANGES.md` §status。
#[derive(Debug, Clone, Serialize)]
pub struct MetricsSnapshot {
    pub collected_at: f64,
    pub host: String,
    pub platform: String,
    pub uptime: String,
    pub uptime_seconds: u64,
    pub procs: u64,
    pub hardware: HardwareInfo,
    pub health_score: i64,
    pub health_score_msg: String,

    pub cpu: CpuStatus,
    pub gpu: Vec<GpuStatus>,
    pub memory: MemoryStatus,
    pub disks: Vec<DiskStatus>,
    pub trash_size: u64,
    pub trash_approx: bool,
    pub disk_io: DiskIoStatus,
    pub network: Vec<NetworkStatus>,
    pub network_history: NetworkHistory,
    pub proxy: ProxyStatus,
    pub batteries: Vec<BatteryStatus>,
    pub thermal: ThermalStatus,
    pub sensors: Vec<SensorReading>,
    pub bluetooth: Vec<BluetoothDevice>,

    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub top_processes: Vec<ProcessInfo>,
    pub process_collected_at: Option<f64>,
    pub process_stale: Option<bool>,
    pub zombie_count: Option<i64>,
    pub zombie_parents: Vec<ZombieParent>,
    pub zombie_parents_complete: Option<bool>,
    /// ProcessWatch 告警（对标 process_alerts；禁用时为空）。
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub process_alerts: Vec<super::process_watch::ProcessAlert>,
}

/// 对标 `HardwareInfo`。
#[derive(Debug, Clone, Default, Serialize)]
pub struct HardwareInfo {
    pub model: String,
    pub cpu_model: String,
    pub total_ram: String,
    pub disk_size: String,
    pub os_version: String,
    pub refresh_rate: String,
}

/// 对标 `DiskIOStatus`（MB/s）。
#[derive(Debug, Clone, Default, Serialize)]
pub struct DiskIoStatus {
    pub read_rate: f64,
    pub write_rate: f64,
}

/// 对标 `ProcessInfo`（State 为内部字段，JSON 不输出，对标 `json:"-"`）。
#[derive(Debug, Clone, Serialize)]
pub struct ProcessInfo {
    pub pid: i64,
    pub ppid: i64,
    #[serde(skip_serializing)]
    pub state: String,
    pub name: String,
    pub command: String,
    pub cpu: f64,
    pub memory: f64,
    #[serde(skip_serializing_if = "is_zero_u64")]
    pub memory_bytes: u64,
}

fn is_zero_u64(v: &u64) -> bool {
    *v == 0
}

/// 对标 `ZombieParent`。
#[derive(Debug, Clone, Serialize)]
pub struct ZombieParent {
    pub pid: i64,
    pub name: String,
    pub count: i64,
}

/// 对标 `CPUStatus`。
#[derive(Debug, Clone, Default, Serialize)]
pub struct CpuStatus {
    pub usage: f64,
    pub per_core: Vec<f64>,
    pub per_core_estimated: bool,
    pub load1: f64,
    pub load5: f64,
    pub load15: f64,
    pub core_count: i64,
    pub logical_cpu: i64,
    pub p_core_count: i64,
    pub e_core_count: i64,
}

/// 对标 `GPUStatus`。
#[derive(Debug, Clone, Default, Serialize)]
pub struct GpuStatus {
    pub name: String,
    pub usage: f64,
    pub memory_used: f64,
    pub memory_total: f64,
    pub core_count: i64,
    pub note: String,
}

/// 对标 `MemoryStatus`。
#[derive(Debug, Clone, Default, Serialize)]
pub struct MemoryStatus {
    pub used: u64,
    pub total: u64,
    pub available: u64,
    pub used_percent: f64,
    pub swap_used: u64,
    pub swap_total: u64,
    pub cached: u64,
    pub pressure: String,
}

/// 对标 `DiskStatus`。
#[derive(Debug, Clone, Serialize)]
pub struct DiskStatus {
    pub mount: String,
    pub device: String,
    pub used: u64,
    pub total: u64,
    pub used_percent: f64,
    pub fstype: String,
    pub external: bool,
    pub smart_status: String,
    #[serde(skip_serializing_if = "is_zero_u64")]
    pub purgeable: u64,
}

/// 对标 `NetworkStatus`（速率单位 MB/s）。
#[derive(Debug, Clone, Serialize)]
pub struct NetworkStatus {
    pub name: String,
    pub rx_rate_mbs: f64,
    pub tx_rate_mbs: f64,
    pub ip: String,
}

/// 对标 `NetworkHistory`。
#[derive(Debug, Clone, Serialize)]
pub struct NetworkHistory {
    pub rx_history: Vec<f64>,
    pub tx_history: Vec<f64>,
}

/// 对标 `ProxyStatus`（is_tunnel 为内部字段，对标 `json:"-"`）。
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct ProxyStatus {
    pub enabled: bool,
    #[serde(rename = "type")]
    pub kind: String,
    pub host: String,
    #[serde(skip_serializing)]
    #[allow(dead_code)]
    pub is_tunnel: bool,
}

/// 对标 `BatteryStatus`。
#[derive(Debug, Clone, Serialize)]
pub struct BatteryStatus {
    pub percent: f64,
    pub status: String,
    pub time_left: String,
    pub health: String,
    pub cycle_count: i64,
    pub capacity: i64,
}

/// 对标 `ThermalStatus`。
#[derive(Debug, Clone, Default, Serialize)]
pub struct ThermalStatus {
    pub cpu_temp: f64,
    pub gpu_temp: f64,
    pub battery_temp: f64,
    pub fan_speed: i64,
    pub fan_count: i64,
    pub system_power: f64,
    pub adapter_power: f64,
    pub battery_power: f64,
}

/// 对标 `SensorReading`（原实现已禁用传感器采集，保留结构以对齐 JSON）。
#[derive(Debug, Clone, Serialize)]
pub struct SensorReading {
    pub label: String,
    pub value: f64,
    pub unit: String,
    pub note: String,
}

/// 对标 `BluetoothDevice`。
#[derive(Debug, Clone, Serialize)]
pub struct BluetoothDevice {
    pub name: String,
    pub connected: bool,
    pub battery: String,
}
