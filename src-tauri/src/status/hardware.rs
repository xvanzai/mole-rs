//! 硬件静态信息，对标 `cmd/status/metrics_hardware.go`。

use super::types::HardwareInfo;
use std::time::Duration;

/// 对标 `collectHardware`：型号/芯片来自 system_profiler，系统版本来自
/// sw_vers，刷新率来自 mini 级显示信息；RAM/磁盘由调用方传入。
pub fn collect_hardware(total_ram: u64, first_disk_total: Option<u64>) -> HardwareInfo {
    let mut model = String::new();
    let mut cpu_model = String::new();
    let mut os_version = String::new();
    let mut refresh_rate = String::new();

    if let Ok(out) = super::run_cmd("system_profiler", &["SPHardwareDataType"], Duration::from_secs(3))
    {
        for line in out.lines() {
            let lower = line.trim().to_lowercase();
            // 优先 "Model Name" 而非 "Model Identifier"。
            if lower.contains("model name:") {
                if let Some((_, v)) = line.split_once(':') {
                    model = v.trim().to_string();
                }
            }
            if lower.contains("chip:") {
                if let Some((_, v)) = line.split_once(':') {
                    cpu_model = v.trim().to_string();
                }
            }
            if lower.contains("processor name:") && cpu_model.is_empty() {
                if let Some((_, v)) = line.split_once(':') {
                    cpu_model = v.trim().to_string();
                }
            }
        }
    }

    if let Ok(out) = super::run_cmd("sw_vers", &["-productVersion"], Duration::from_secs(1)) {
        os_version = format!("macOS {}", out.trim());
    }

    // mini detail 档保持快速。
    if let Ok(out) = super::run_cmd(
        "system_profiler",
        &["-detailLevel", "mini", "SPDisplaysDataType"],
        Duration::from_secs(2),
    ) {
        refresh_rate = parse_refresh_rate(&out);
    }

    let disk_size = match first_disk_total {
        Some(t) => super::core_bytes_si(t as i64),
        None => "Unknown".into(),
    };

    HardwareInfo {
        model,
        cpu_model,
        total_ram: super::core_bytes_si(total_ram as i64),
        disk_size,
        os_version,
        refresh_rate,
    }
}

/// 对标 `parseRefreshRate`：取显示输出中最高的刷新率。
///
/// 硬件信息是静态卡片文本（Go 侧经 humanBytes 用 SI 单位），
/// 这里复用 core::units::bytes_si。
pub fn parse_refresh_rate(output: &str) -> String {
    let mut max_hz = 0i64;

    for line in output.lines() {
        let lower = line.to_lowercase();
        if !lower.contains("hz") {
            continue;
        }
        let fields: Vec<&str> = lower.split_whitespace().collect();
        for (i, field) in fields.iter().enumerate() {
            if *field == "hz" && i > 0 {
                let hz = parse_int(fields[i - 1]);
                if hz > max_hz && hz < 500 {
                    max_hz = hz;
                }
                continue;
            }
            if let Some(num_str) = field.strip_suffix("hz") {
                let mut num_str = num_str.to_string();
                if num_str.is_empty() && i > 0 {
                    num_str = fields[i - 1].to_string();
                }
                let hz = parse_int(&num_str);
                if hz > max_hz && hz < 500 {
                    max_hz = hz;
                }
            }
        }
    }

    if max_hz > 0 {
        format!("{max_hz}Hz")
    } else {
        String::new()
    }
}

/// 对标 `parseInt`：剥离两侧非数字填充（保留数字与小数点）后取整。
fn parse_int(s: &str) -> i64 {
    let cleaned: String = s
        .trim()
        .trim_matches(|r: char| (r < '0' || r > '9') && r != '.')
        .to_string();
    if cleaned.is_empty() {
        return 0;
    }
    // Go fmt.Sscanf("%d") 读取前缀整数（遇小数点停止）。
    let prefix: String = cleaned
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    prefix.parse().unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 对标 metrics_hardware_test.go / main_test.go 的 refresh rate 用例。
    #[test]
    fn refresh_rate_patterns() {
        assert_eq!(parse_refresh_rate("  Resolution: 3024x1964 Retina\n  UI Looks like: 1512x982 @ 120.00Hz\n"), "120Hz");
        assert_eq!(parse_refresh_rate("Refresh Rate: 60 Hz\n"), "60Hz");
        assert_eq!(parse_refresh_rate("@60Hz\n"), "60Hz");
        assert_eq!(parse_refresh_rate("no display info"), "");
        assert_eq!(parse_refresh_rate("@ 600Hz\n"), ""); // <500 之外忽略
        // 多显示器取最高。
        assert_eq!(parse_refresh_rate("@ 60Hz\n@ 120Hz\n"), "120Hz");
    }

    #[test]
    fn parse_int_prefix() {
        assert_eq!(parse_int("120.00"), 120);
        assert_eq!(parse_int("abc60"), 60);
        assert_eq!(parse_int("60"), 60);
        assert_eq!(parse_int(""), 0);
        assert_eq!(parse_int("abc"), 0);
    }
}
