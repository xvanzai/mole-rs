//! GPU 指标，对标 `cmd/status/metrics_gpu.go`。

use super::types::GpuStatus;
use std::time::{Duration, Instant};

/// 对标常量。
const SYSTEM_PROFILER_TIMEOUT: Duration = Duration::from_secs(4);
const MAC_GPU_INFO_TTL: Duration = Duration::from_secs(600);
const MAC_GPU_USAGE_TTL: Duration = Duration::from_secs(5);
const POWERMETRICS_TIMEOUT: Duration = Duration::from_secs(2);

/// GPU 静态信息 + 使用率的组合缓存（对标 Collector 的 cachedGPU/lastGPUAt/
/// cachedGPUUsage/lastGPUUsageAt）。
#[derive(Default)]
pub struct GpuCache {
    pub gpus: Vec<GpuStatus>,
    pub last_info_at: Option<Instant>,
    pub usage: f64,
    pub last_usage_at: Option<Instant>,
}

impl GpuCache {
    fn info_expired(&self) -> bool {
        self.gpus.is_empty()
            || self
                .last_info_at
                .is_none_or(|t| t.elapsed() >= MAC_GPU_INFO_TTL)
    }

    fn usage_fresh(&self) -> bool {
        self.last_usage_at
            .is_some_and(|t| t.elapsed() < MAC_GPU_USAGE_TTL)
    }
}

/// 对标 `collectGPU`（darwin 分支）：静态信息 10min 缓存 + 实时使用率
/// 5s 缓存，使用率应用到第一块 GPU（Apple Silicon）。
pub fn collect_gpu(cache: &mut GpuCache) -> Vec<GpuStatus> {
    if cache.info_expired() {
        if let Some(gpus) = read_mac_gpu_info() {
            if !gpus.is_empty() {
                cache.gpus = gpus;
                cache.last_info_at = Some(Instant::now());
            }
        }
    }

    if !cache.gpus.is_empty() {
        let usage = if cache.usage_fresh() {
            cache.usage
        } else {
            let usage = get_mac_gpu_usage();
            cache.usage = usage;
            cache.last_usage_at = Some(Instant::now());
            usage
        };
        let mut result = cache.gpus.clone();
        if let Some(first) = result.first_mut() {
            first.usage = usage;
        }
        return result;
    }

    Vec::new()
}

/// 对标 `readMacGPUInfo`：system_profiler JSON 解析。
fn read_mac_gpu_info() -> Option<Vec<GpuStatus>> {
    if !super::command_exists("system_profiler") {
        return None;
    }
    let out = super::run_cmd(
        "system_profiler",
        &["-json", "SPDisplaysDataType"],
        SYSTEM_PROFILER_TIMEOUT,
    )
    .ok()?;
    parse_mac_gpu_info(&out)
}

/// JSON 解析（独立函数便于测试；字段对标 Go struct 标签）。
fn parse_mac_gpu_info(out: &str) -> Option<Vec<GpuStatus>> {
    let payload: serde_json::Value = serde_json::from_str(out).ok()?;
    let displays = payload.get("SPDisplaysDataType")?.as_array()?;
    let mut gpus = Vec::new();
    for d in displays {
        let name = d.get("_name").and_then(|v| v.as_str()).unwrap_or("");
        if name.is_empty() {
            continue;
        }
        let vram = d.get("spdisplays_vram").and_then(|v| v.as_str()).unwrap_or("");
        let vendor = d.get("spdisplays_vendor").and_then(|v| v.as_str()).unwrap_or("");
        let metal = d.get("spdisplays_metal").and_then(|v| v.as_str()).unwrap_or("");
        let cores = d
            .get("sppci_cores")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<i64>().ok())
            .unwrap_or(0);

        let mut note_parts: Vec<String> = Vec::new();
        if !vram.is_empty() {
            // 对标 Go："VRAM "+vram 拼接为单段。
            note_parts.push(format!("VRAM {vram}"));
        }
        if !metal.is_empty() {
            note_parts.push(metal.to_string());
        }
        if !vendor.is_empty() {
            note_parts.push(vendor.to_string());
        }
        let note = note_parts.join(" · ");

        gpus.push(GpuStatus {
            name: name.to_string(),
            usage: -1.0, // 待实时数据更新
            core_count: cores,
            note,
            ..Default::default()
        });
    }
    if gpus.is_empty() {
        gpus.push(GpuStatus {
            name: "GPU info unavailable".into(),
            note: "Unable to parse system_profiler output".into(),
            ..Default::default()
        });
    }
    Some(gpus)
}

/// 对标 `getMacGPUUsage`：powermetrics GPU active residency。
/// powermetrics 通常需要 root；失败返回 -1（未知哨兵，与 Go 一致）。
fn get_mac_gpu_usage() -> f64 {
    let Ok(out) = super::run_cmd(
        "powermetrics",
        &["--samplers", "gpu_power", "-i", "500", "-n", "1"],
        POWERMETRICS_TIMEOUT,
    ) else {
        return -1.0;
    };
    parse_gpu_usage(&out)
}

/// 解析 "GPU HW active residency: X.XX%"，回退 idle residency 推导。
fn parse_gpu_usage(out: &str) -> f64 {
    for marker in ["GPU HW active residency:", "GPU idle residency:"] {
        if let Some(pos) = out.find(marker) {
            let rest = &out[pos + marker.len()..];
            let num: String = rest
                .trim_start()
                .chars()
                .take_while(|c| c.is_ascii_digit() || *c == '.')
                .collect();
            if let Ok(value) = num.parse::<f64>() {
                return if marker.contains("idle") {
                    100.0 - value
                } else {
                    value
                };
            }
        }
    }
    -1.0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 对标 JSON 结构（Apple Silicon 例）。
    #[test]
    fn parse_mac_gpu_info_json() {
        let raw = r#"{"SPDisplaysDataType":[{"_name":"Apple M2","spdisplays_vendor":"sApple","spdisplays_metal":"Metal3","spdisplays_vram":"8 GB","sppci_cores":"10"}]}"#;
        let gpus = parse_mac_gpu_info(raw).unwrap();
        assert_eq!(gpus.len(), 1);
        assert_eq!(gpus[0].name, "Apple M2");
        assert_eq!(gpus[0].core_count, 10);
        assert_eq!(gpus[0].usage, -1.0);
        assert_eq!(gpus[0].note, "VRAM 8 GB · Metal3 · sApple");
    }

    #[test]
    fn parse_gpu_usage_variants() {
        assert_eq!(parse_gpu_usage("GPU HW active residency: 42.55%\n"), 42.55);
        assert_eq!(parse_gpu_usage("GPU idle residency: 30%\n"), 70.0);
        assert_eq!(parse_gpu_usage("nothing here"), -1.0);
    }
}
