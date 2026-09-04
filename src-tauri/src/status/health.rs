//! 健康评分，对标 `cmd/status/metrics_health.go`（权重与阈值 1:1）。

use super::types::{BatteryStatus, CpuStatus, DiskStatus, MemoryStatus, ThermalStatus};

// 对标常量。
const HEALTH_CPU_WEIGHT: f64 = 30.0;
const HEALTH_MEM_WEIGHT: f64 = 25.0;
const HEALTH_DISK_WEIGHT: f64 = 20.0;
const HEALTH_THERMAL_WEIGHT: f64 = 15.0;
const HEALTH_IO_WEIGHT: f64 = 10.0;

const CPU_NORMAL_THRESHOLD: f64 = 50.0;
const CPU_HIGH_THRESHOLD: f64 = 85.0;

const MEM_NORMAL_THRESHOLD: f64 = 70.0;
const MEM_HIGH_THRESHOLD: f64 = 88.0;
const MEM_PRESSURE_WARN_PENALTY: f64 = 5.0;
const MEM_PRESSURE_CRIT_PENALTY: f64 = 15.0;

const DISK_WARN_THRESHOLD: f64 = 80.0;
const DISK_CRIT_THRESHOLD: f64 = 93.0;

const THERMAL_NORMAL_THRESHOLD: f64 = 65.0;
const THERMAL_HIGH_THRESHOLD: f64 = 85.0;

const IO_NORMAL_THRESHOLD: f64 = 50.0;
const IO_HIGH_THRESHOLD: f64 = 150.0;

const BATTERY_CYCLE_WARN: i64 = 800;
const BATTERY_CYCLE_DANGER: i64 = 900;
const BATTERY_CAP_WARN: i64 = 80;
const BATTERY_CAP_DANGER: i64 = 60;

const UPTIME_WARN_SECS: u64 = 7 * 86400;
const UPTIME_DANGER_SECS: u64 = 14 * 86400;

const SCORE_EXCELLENT_THRESHOLD: i64 = 85;
const SCORE_GOOD_THRESHOLD: i64 = 65;
const SCORE_FAIR_THRESHOLD: i64 = 45;

/// 1:1 移植 `calculateHealthScore`。
/// diskIO 暂为占位（IO 计数器移植为后续子模块），传零值不影响其余规则。
pub fn calculate_health_score(
    cpu: &CpuStatus,
    mem: &MemoryStatus,
    disks: &[DiskStatus],
    disk_io: &[f64; 2], // [read_rate, write_rate] MB/s
    thermal: &ThermalStatus,
    uptime_secs: u64,
) -> (i64, String) {
    // 电池列表占位：battery penalty 依赖电池数据，full 快照传入。
    calculate_health_score_with_batteries(cpu, mem, disks, disk_io, thermal, &[], uptime_secs)
}

pub fn calculate_health_score_with_batteries(
    cpu: &CpuStatus,
    mem: &MemoryStatus,
    disks: &[DiskStatus],
    disk_io: &[f64; 2],
    thermal: &ThermalStatus,
    batteries: &[BatteryStatus],
    uptime_secs: u64,
) -> (i64, String) {
    let mut score = 100.0f64;
    let mut issues: Vec<&str> = Vec::new();

    // CPU penalty。
    if cpu.usage > CPU_NORMAL_THRESHOLD {
        let cpu_penalty = if cpu.usage > CPU_HIGH_THRESHOLD {
            // 在剩余区间内线性放大，保证使用率越分越高时扣分持续增长。
            HEALTH_CPU_WEIGHT * (cpu.usage - CPU_NORMAL_THRESHOLD) / (100.0 - CPU_NORMAL_THRESHOLD)
        } else {
            (HEALTH_CPU_WEIGHT / 2.0) * (cpu.usage - CPU_NORMAL_THRESHOLD)
                / (CPU_HIGH_THRESHOLD - CPU_NORMAL_THRESHOLD)
        };
        score -= cpu_penalty;
    }
    if cpu.usage > CPU_HIGH_THRESHOLD {
        issues.push("High CPU");
    }

    // Memory penalty。
    if mem.used_percent > MEM_NORMAL_THRESHOLD {
        let mem_penalty = if mem.used_percent > MEM_HIGH_THRESHOLD {
            HEALTH_MEM_WEIGHT * (mem.used_percent - MEM_NORMAL_THRESHOLD)
                / (100.0 - MEM_NORMAL_THRESHOLD)
        } else {
            (HEALTH_MEM_WEIGHT / 2.0) * (mem.used_percent - MEM_NORMAL_THRESHOLD)
                / (MEM_HIGH_THRESHOLD - MEM_NORMAL_THRESHOLD)
        };
        score -= mem_penalty;
    }
    if mem.used_percent > MEM_HIGH_THRESHOLD {
        issues.push("High Memory");
    }

    // Memory pressure penalty。
    match mem.pressure.as_str() {
        "warn" => {
            score -= MEM_PRESSURE_WARN_PENALTY;
            issues.push("Memory Pressure");
        }
        "critical" => {
            score -= MEM_PRESSURE_CRIT_PENALTY;
            issues.push("Critical Memory");
        }
        _ => {}
    }

    // Disk penalty。
    if !disks.is_empty() {
        let disk_usage = disks[0].used_percent;
        if disk_usage > DISK_WARN_THRESHOLD {
            let disk_penalty = if disk_usage > DISK_CRIT_THRESHOLD {
                HEALTH_DISK_WEIGHT * (disk_usage - DISK_WARN_THRESHOLD)
                    / (100.0 - DISK_WARN_THRESHOLD)
            } else {
                (HEALTH_DISK_WEIGHT / 2.0) * (disk_usage - DISK_WARN_THRESHOLD)
                    / (DISK_CRIT_THRESHOLD - DISK_WARN_THRESHOLD)
            };
            score -= disk_penalty;
        }
        if disk_usage > DISK_CRIT_THRESHOLD {
            issues.push("Disk Almost Full");
        }
    }
    for disk in disks {
        if disk.smart_status == "failing" {
            if score > 44.0 {
                score = 44.0;
            }
            issues.push("Disk SMART Failing");
            break;
        }
    }

    // Thermal penalty。
    if thermal.cpu_temp > 0.0 {
        if thermal.cpu_temp > THERMAL_NORMAL_THRESHOLD {
            if thermal.cpu_temp > THERMAL_HIGH_THRESHOLD {
                score -= HEALTH_THERMAL_WEIGHT;
                issues.push("Overheating");
            } else {
                score -= HEALTH_THERMAL_WEIGHT * (thermal.cpu_temp - THERMAL_NORMAL_THRESHOLD)
                    / (THERMAL_HIGH_THRESHOLD - THERMAL_NORMAL_THRESHOLD);
            }
        }
    }

    // Disk IO penalty。
    let total_io = disk_io[0] + disk_io[1];
    if total_io > IO_NORMAL_THRESHOLD {
        if total_io > IO_HIGH_THRESHOLD {
            score -= HEALTH_IO_WEIGHT;
            issues.push("Heavy Disk IO");
        } else {
            score -=
                HEALTH_IO_WEIGHT * (total_io - IO_NORMAL_THRESHOLD) / (IO_HIGH_THRESHOLD - IO_NORMAL_THRESHOLD);
        }
    }

    // Battery health penalty（仅存在电池时）。
    if let Some(b) = batteries.first() {
        let (_, sev) = battery_health_label(b.cycle_count, b.capacity);
        match sev {
            "danger" => {
                score -= 5.0;
                issues.push("Battery Service Soon");
            }
            "warn" => {
                score -= 2.0;
            }
            _ => {}
        }
    }

    // Uptime penalty。
    if uptime_secs > UPTIME_DANGER_SECS {
        score -= 3.0;
        issues.push("Restart Recommended");
    } else if uptime_secs > UPTIME_WARN_SECS {
        score -= 1.0;
    }

    // Clamp。
    let score = score.clamp(0.0, 100.0) as i64;

    let mut msg = if score >= SCORE_EXCELLENT_THRESHOLD {
        "Excellent".to_string()
    } else if score >= SCORE_GOOD_THRESHOLD {
        "Good".to_string()
    } else if score >= SCORE_FAIR_THRESHOLD {
        "Fair".to_string()
    } else {
        "Needs Attention".to_string()
    };

    if !issues.is_empty() {
        msg = format!("{msg}: {}", issues.join(", "));
    }

    (score, msg)
}

/// 1:1 移植 `batteryHealthLabel`。
pub fn battery_health_label(cycles: i64, capacity: i64) -> (&'static str, &'static str) {
    if cycles > BATTERY_CYCLE_DANGER || (capacity > 0 && capacity < BATTERY_CAP_DANGER) {
        ("Service Soon", "danger")
    } else if cycles > BATTERY_CYCLE_WARN || (capacity > 0 && capacity < BATTERY_CAP_WARN) {
        ("Fair", "warn")
    } else {
        ("Healthy", "ok")
    }
}

/// 1:1 移植 `uptimeSeverity`（保留 API 完整性，view 层使用）。
#[allow(dead_code)]
pub fn uptime_severity(secs: u64) -> &'static str {
    if secs > UPTIME_DANGER_SECS {
        "danger"
    } else if secs > UPTIME_WARN_SECS {
        "warn"
    } else {
        "ok"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_inputs() -> (CpuStatus, MemoryStatus, Vec<DiskStatus>, ThermalStatus) {
        (
            CpuStatus::default(),
            MemoryStatus::default(),
            Vec::new(),
            ThermalStatus::default(),
        )
    }

    /// 对标 metrics_health_test.go：全指标正常 → 100 分 Excellent。
    #[test]
    fn all_normal() {
        let (cpu, mem, disks, thermal) = base_inputs();
        let (score, msg) = calculate_health_score(&cpu, &mem, &disks, &[0.0, 0.0], &thermal, 3600);
        assert_eq!(score, 100);
        assert_eq!(msg, "Excellent");
    }

    /// 对标：高 CPU 线性扣分（>85% 区间持续放大，不回落）。
    #[test]
    fn cpu_penalty_scales() {
        let (mut cpu, mem, disks, thermal) = base_inputs();
        cpu.usage = 95.0;
        let (score, msg) = calculate_health_score(&cpu, &mem, &disks, &[0.0, 0.0], &thermal, 3600);
        // 30 * (95-50) / 50 = 27 → 73
        assert_eq!(score, 73);
        assert!(msg.contains("High CPU"));

        cpu.usage = 60.0;
        let (score, _) = calculate_health_score(&cpu, &mem, &disks, &[0.0, 0.0], &thermal, 3600);
        // 15 * (60-50) / 35 ≈ 4.29 → 95.71 → 95
        assert_eq!(score, 95);
    }

    /// 对标：内存压力惩罚。
    #[test]
    fn memory_pressure_penalty() {
        let (cpu, mut mem, disks, thermal) = base_inputs();
        mem.pressure = "warn".into();
        let (score, msg) = calculate_health_score(&cpu, &mem, &disks, &[0.0, 0.0], &thermal, 3600);
        assert_eq!(score, 95);
        assert!(msg.contains("Memory Pressure"));

        mem.pressure = "critical".into();
        let (score, msg) = calculate_health_score(&cpu, &mem, &disks, &[0.0, 0.0], &thermal, 3600);
        assert_eq!(score, 85);
        assert!(msg.contains("Critical Memory"));
    }

    /// 对标：磁盘几乎满（>93%）与 SMART 失败封顶 44。
    #[test]
    fn disk_penalties() {
        let (cpu, mem, disks, thermal) = base_inputs();
        let mut disks = disks;
        disks.push(DiskStatus {
            mount: "/".into(),
            device: "/dev/disk3".into(),
            used: 95,
            total: 100,
            used_percent: 95.0,
            fstype: "apfs".into(),
            external: false,
            smart_status: "unknown".into(),
            purgeable: 0,
        });
        let (score, msg) = calculate_health_score(&cpu, &mem, &disks, &[0.0, 0.0], &thermal, 3600);
        // 20 * (95-80) / 20 = 15 → 85
        assert_eq!(score, 85);
        assert!(msg.contains("Disk Almost Full"));

        disks[0].smart_status = "failing".into();
        let (score, msg) = calculate_health_score(&cpu, &mem, &disks, &[0.0, 0.0], &thermal, 3600);
        assert_eq!(score, 44);
        assert!(msg.contains("Disk SMART Failing"));
    }

    /// 对标：电池健康与 uptime 惩罚。
    #[test]
    fn battery_and_uptime_penalties() {
        let (cpu, mem, disks, thermal) = base_inputs();
        let old = BatteryStatus {
            percent: 80.0,
            status: "discharging".into(),
            time_left: String::new(),
            health: "Normal".into(),
            cycle_count: 950,
            capacity: 0,
        };
        let (score, msg) = calculate_health_score_with_batteries(
            &cpu, &mem, &disks, &[0.0, 0.0], &thermal, &[old.clone()], 3600,
        );
        assert_eq!(score, 95);
        assert!(msg.contains("Battery Service Soon"));

        // 电池危险 -5 + uptime 14 天以上 -3 → 92。
        let (score, msg) = calculate_health_score_with_batteries(
            &cpu, &mem, &disks, &[0.0, 0.0], &thermal, &[old], 15 * 86400,
        );
        assert_eq!(score, 92);
        assert!(msg.contains("Restart Recommended"));

        // 7-14 天只扣 1 分且无 issue。
        let (score, msg) =
            calculate_health_score(&cpu, &mem, &disks, &[0.0, 0.0], &thermal, 8 * 86400);
        assert_eq!(score, 99);
        assert_eq!(msg, "Excellent");
    }

    #[test]
    fn battery_label_bands() {
        assert_eq!(battery_health_label(100, 90), ("Healthy", "ok"));
        assert_eq!(battery_health_label(850, 90), ("Fair", "warn"));
        assert_eq!(battery_health_label(950, 90), ("Service Soon", "danger"));
        assert_eq!(battery_health_label(100, 50), ("Service Soon", "danger"));
    }

    #[test]
    fn uptime_severity_bands() {
        assert_eq!(uptime_severity(3600), "ok");
        assert_eq!(uptime_severity(8 * 86400), "warn");
        assert_eq!(uptime_severity(15 * 86400), "danger");
    }
}
