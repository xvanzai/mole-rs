//! 电池与热能/功耗指标，对标 `cmd/status/metrics_battery.go`。

use super::types::{BatteryStatus, ThermalStatus};
use std::time::{Duration, Instant};

/// 对标 `powerCacheTTL`：system_profiler 输出缓存 30 秒。
const POWER_CACHE_TTL: Duration = Duration::from_secs(30);

/// system_profiler 缓存，对标 Go 的 cachedPower/cachedPowerJSON。
#[derive(Default)]
pub struct PowerCache {
    pub text: Option<(String, Instant)>,
    pub json: Option<(String, Instant)>,
}

/// 对标 `collectBatteries`：pmset 实时百分比/状态 + 缓存的
/// system_profiler / ioreg 健康数据。
pub fn collect_batteries(cache: &mut PowerCache) -> Vec<BatteryStatus> {
    if super::command_exists("pmset") {
        if let Ok(out) = super::run_cmd("pmset", &["-g", "batt"], Duration::from_secs(10)) {
            let (health, cycles, capacity) = get_cached_power_data(cache);
            let batts = parse_pm_set(&out, &health, cycles, capacity);
            if !batts.is_empty() {
                return batts;
            }
        }
    }
    Vec::new()
}

/// 对标 `parsePMSet`。
pub fn parse_pm_set(raw: &str, health: &str, cycles: i64, capacity: i64) -> Vec<BatteryStatus> {
    let mut out = Vec::new();
    let mut time_left = String::new();

    for line in raw.lines() {
        // Time remaining。
        if line.contains("remaining") {
            let fields: Vec<&str> = line.split_whitespace().collect();
            for (i, p) in fields.iter().enumerate() {
                if *p == "remaining" && i > 0 {
                    time_left = fields[i - 1].to_string();
                }
            }
        }

        if !line.contains('%') {
            continue;
        }
        let fields: Vec<&str> = line.split_whitespace().collect();
        let mut percent = 0.0f64;
        let mut found = false;
        let mut status = "Unknown".to_string();
        for (i, f) in fields.iter().enumerate() {
            if f.contains('%') {
                let value = f.trim_end_matches(';').trim_end_matches('%');
                if let Ok(p) = value.parse::<f64>() {
                    percent = p;
                    found = true;
                    if i + 1 < fields.len() {
                        status = fields[i + 1].trim_end_matches(';').to_string();
                    }
                }
                break;
            }
        }
        if !found {
            continue;
        }
        out.push(BatteryStatus {
            percent,
            status,
            time_left: time_left.clone(),
            health: health.to_string(),
            cycle_count: cycles,
            capacity,
        });
    }
    out
}

/// 对标 `getCachedPowerData`：system_profiler（JSON→文本回退）+ ioreg
/// 的合并（ioreg 只补缺失值）。
fn get_cached_power_data(cache: &mut PowerCache) -> (String, i64, i64) {
    let (health, mut cycles, mut capacity) = get_cached_system_power_data(cache);
    let (ioreg_cycles, ioreg_capacity) = get_apple_smart_battery_health_data();
    if ioreg_cycles > 0 {
        cycles = ioreg_cycles;
    }
    // system_profiler 的 Maximum Capacity 与 macOS 显示一致；IORegistry
    // 比例只是估算，仅在缺失时补位。
    if capacity <= 0 && ioreg_capacity > 0 {
        capacity = ioreg_capacity;
    }
    (health, cycles, capacity)
}

/// 对标 `getCachedSystemPowerData`。
fn get_cached_system_power_data(cache: &mut PowerCache) -> (String, i64, i64) {
    let json = get_system_power_json_output(cache);
    if !json.is_empty() {
        if let Some((health, cycles, capacity)) = parse_system_power_json(&json) {
            return (health, cycles, capacity);
        }
    }
    let text = get_system_power_output(cache);
    if text.is_empty() {
        return (String::new(), 0, 0);
    }
    parse_system_power_text(&text)
}

/// 对标 `parseSystemPowerText`。
pub fn parse_system_power_text(out: &str) -> (String, i64, i64) {
    let mut health = String::new();
    let mut cycles = 0i64;
    let mut capacity = 0i64;
    for line in out.lines() {
        let lower = line.to_lowercase();
        if lower.contains("cycle count") {
            if let Some((_, after)) = line.split_once(':') {
                cycles = after.trim().parse().unwrap_or(0);
            }
        }
        if lower.contains("condition") {
            if let Some((_, after)) = line.split_once(':') {
                health = after.trim().to_string();
            }
        }
        if lower.contains("maximum capacity") {
            if let Some((_, after)) = line.split_once(':') {
                capacity = parse_percent_int(after);
            }
        }
    }
    (health, cycles, capacity)
}

/// 对标 `parseSystemPowerJSON`（serde_json Value 等价实现）。
pub fn parse_system_power_json(raw: &str) -> Option<(String, i64, i64)> {
    let payload: serde_json::Value = serde_json::from_str(raw).ok()?;
    let items = payload.get("SPPowerDataType")?.as_array()?;
    for item in items {
        let info = item.get("sppower_battery_health_info");
        let Some(info) = info else { continue };
        let health = info
            .get("sppower_battery_health")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let cycles = info
            .get("sppower_battery_cycle_count")
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        let max_capacity = info
            .get("sppower_battery_health_maximum_capacity")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let parsed_capacity = parse_percent_int(max_capacity);
        if !health.is_empty() || cycles > 0 || parsed_capacity > 0 {
            return Some((health, cycles, parsed_capacity));
        }
    }
    None
}

/// 对标 `parsePercentInt`。
fn parse_percent_int(raw: &str) -> i64 {
    let cleaned = raw.trim().trim_end_matches('%').trim();
    cleaned.parse().unwrap_or(0)
}

/// 对标 `getAppleSmartBatteryHealthData`。
fn get_apple_smart_battery_health_data() -> (i64, i64) {
    if !super::command_exists("ioreg") {
        return (0, 0);
    }
    let Ok(out) = super::run_cmd("ioreg", &["-rn", "AppleSmartBattery"], Duration::from_millis(500))
    else {
        return (0, 0);
    };
    parse_apple_smart_battery_health(&out)
}

/// 对标 `parseAppleSmartBatteryHealth`。
pub fn parse_apple_smart_battery_health(out: &str) -> (i64, i64) {
    let (mut design, mut nominal, mut raw_max) = (0i64, 0i64, 0i64);
    let mut cycles = 0i64;
    for line in out.lines() {
        let line = line.trim();
        if cycles == 0 {
            if let Some(raw) = ioreg_value_for_key(line, "CycleCount") {
                if let Ok(value) = raw.parse::<i64>() {
                    if value > 0 && value < 100000 {
                        cycles = value;
                    }
                }
            }
        }
        if design == 0 {
            if let Some(raw) = ioreg_value_for_key(line, "DesignCapacity") {
                design = raw.parse::<i64>().unwrap_or(0).max(0);
            }
        }
        if nominal == 0 {
            if let Some(raw) = ioreg_value_for_key(line, "NominalChargeCapacity") {
                nominal = raw.parse::<i64>().unwrap_or(0).max(0);
            }
        }
        if raw_max == 0 {
            if let Some(raw) = ioreg_value_for_key(line, "AppleRawMaxCapacity") {
                raw_max = raw.parse::<i64>().unwrap_or(0).max(0);
            }
        }
    }
    (cycles, battery_health_percent(design, nominal, raw_max))
}

/// 对标 `batteryHealthPercent`：优先 AppleRawMaxCapacity（Nominal 含缓冲
/// 会偏高）；MaxCapacity 在 Apple Silicon 上是当前电量分母（恒 100），
/// 不作健康度量。
fn battery_health_percent(design: i64, nominal: i64, raw_max: i64) -> i64 {
    if design <= 0 {
        return 0;
    }
    let capacity = if raw_max != 0 { raw_max } else { nominal };
    if capacity <= 0 {
        return 0;
    }
    let pct = (capacity as f64 * 100.0 / design as f64).round();
    pct.clamp(0.0, 100.0) as i64
}

/// 对标 `getSystemPowerJSONOutput`。
fn get_system_power_json_output(cache: &mut PowerCache) -> String {
    if let Some((text, at)) = &cache.json {
        if at.elapsed() < POWER_CACHE_TTL {
            return text.clone();
        }
    }
    match super::run_cmd(
        "system_profiler",
        &["SPPowerDataType", "-json"],
        Duration::from_secs(3),
    ) {
        Ok(out) => {
            cache.json = Some((out.clone(), Instant::now()));
            out
        }
        Err(_) => cache
            .json
            .as_ref()
            .map(|(t, _)| t.clone())
            .unwrap_or_default(),
    }
}

/// 对标 `getSystemPowerOutput`。
pub fn get_system_power_output(cache: &mut PowerCache) -> String {
    if let Some((text, at)) = &cache.text {
        if at.elapsed() < POWER_CACHE_TTL {
            return text.clone();
        }
    }
    match super::run_cmd("system_profiler", &["SPPowerDataType"], Duration::from_secs(3)) {
        Ok(out) => {
            cache.text = Some((out.clone(), Instant::now()));
            out
        }
        Err(_) => cache
            .text
            .as_ref()
            .map(|(t, _)| t.clone())
            .unwrap_or_default(),
    }
}

/// 对标 `collectThermal`：风扇转速来自缓存的 system_profiler 文本，
/// 功耗来自 ioreg（实时）。CPU 温度刻意不合成——电池传感器与
/// cpu_thermal_level 都不是 CPU 封装温度，合成会产生虚假过热数据。
pub fn collect_thermal(cache: &mut PowerCache) -> ThermalStatus {
    let mut thermal = ThermalStatus::default();

    let out = get_system_power_output(cache);
    if !out.is_empty() {
        for line in out.lines() {
            let lower = line.to_lowercase();
            if lower.contains("fan") && lower.contains("speed") {
                if let Some((_, after)) = line.split_once(':') {
                    let num = after.trim().split(' ').next().unwrap_or("");
                    thermal.fan_speed = num.parse().unwrap_or(0);
                }
            }
        }
    }

    if let Ok(out) = super::run_cmd("ioreg", &["-rn", "AppleSmartBattery"], Duration::from_millis(500))
    {
        let parsed = parse_apple_smart_battery_thermal(&out);
        thermal.battery_temp = parsed.battery_temp;
        thermal.system_power = parsed.system_power;
        thermal.adapter_power = parsed.adapter_power;
        thermal.battery_power = parsed.battery_power;
    }

    thermal
}

/// 对标 `parseAppleSmartBatteryThermal`。
pub fn parse_apple_smart_battery_thermal(out: &str) -> ThermalStatus {
    let mut thermal = ThermalStatus::default();
    let mut voltage_mv = 0.0f64;
    let mut amperage_ma = 0.0f64;

    for line in out.lines() {
        let line = line.trim();

        // AppleSmartBattery 的 Temperature 单位是百分之一摄氏度。
        if let Some(temp_raw) = parse_ioreg_float_value(line, "Temperature") {
            if temp_raw > 0.0 {
                if temp_raw < 1000.0 {
                    // 部分平台/夹具直接输出摄氏度。
                    thermal.battery_temp = temp_raw;
                } else {
                    thermal.battery_temp = temp_raw / 100.0;
                }
            }
        }

        // 适配器功率：忽略 AppleRawAdapterDetails（原始条目可能先于
        // 规范化条目出现）。
        if line.contains("\"AdapterDetails\"")
            && !line.contains("AppleRaw")
            && thermal.adapter_power == 0.0
        {
            if let Some(watts) = parse_ioreg_float_value(line, "Watts") {
                if watts > 0.0 {
                    thermal.adapter_power = watts;
                }
            }
        }

        // 系统功耗（mW → W）。
        if let Some(power_mw) = parse_ioreg_float_value(line, "SystemPowerIn") {
            set_system_power_mw(&mut thermal, power_mw);
        }
        if thermal.system_power == 0.0 {
            if let Some(power_mw) = parse_ioreg_float_value(line, "SystemPower") {
                set_system_power_mw(&mut thermal, power_mw);
            }
        }

        // 电池功率（mW → W，正 = 放电）。
        if let Some(power_mw) = parse_ioreg_signed_number(line, "BatteryPower") {
            set_battery_power_mw(&mut thermal, power_mw);
        }

        if let Some(voltage) = parse_ioreg_float_value(line, "Voltage") {
            if voltage > 0.0 {
                voltage_mv = voltage;
            }
        }
        if let Some(voltage) = parse_ioreg_float_value(line, "AppleRawBatteryVoltage") {
            if voltage > 0.0 {
                voltage_mv = voltage;
            }
        }
        if let Some(amperage) = parse_ioreg_signed_number(line, "InstantAmperage") {
            if amperage != 0.0 {
                amperage_ma = amperage;
            }
        }
        if let Some(amperage) = parse_ioreg_signed_number(line, "Amperage") {
            if amperage != 0.0 && amperage_ma == 0.0 {
                amperage_ma = amperage;
            }
        }
    }

    if thermal.battery_power == 0.0 && voltage_mv > 0.0 && amperage_ma != 0.0 {
        // AppleSmartBattery 的 amperage 为带符号 mA，负电流 = 放电；
        // BatteryPower 保持放电为正。
        let battery_power_w = -(voltage_mv * amperage_ma) / 1_000_000.0;
        if battery_power_w > -200.0 && battery_power_w < 200.0 {
            thermal.battery_power = battery_power_w;
        }
    }
    thermal
}

/// 对标 `setSystemPowerMW`：拒绝非法值（0–1000W）。
fn set_system_power_mw(thermal: &mut ThermalStatus, power_mw: f64) {
    if (0.0..1_000_000.0).contains(&power_mw) {
        thermal.system_power = power_mw / 1000.0;
    }
}

/// 对标 `setBatteryPowerMW`：±200W 合理区间。
fn set_battery_power_mw(thermal: &mut ThermalStatus, power_mw: f64) {
    if power_mw > -200_000.0 && power_mw < 200_000.0 {
        thermal.battery_power = power_mw / 1000.0;
    }
}

/// 对标 `parseIORegFloatValue`。
fn parse_ioreg_float_value(line: &str, key: &str) -> Option<f64> {
    let raw = ioreg_value_for_key(line, key)?;
    raw.parse::<f64>().ok()
}

/// 对标 `parseIORegSignedNumber`。
fn parse_ioreg_signed_number(line: &str, key: &str) -> Option<f64> {
    let raw = ioreg_value_for_key(line, key)?;
    let val = parse_ioreg_signed_integer(&raw)?;
    Some(val as f64)
}

/// 对标 `parseIORegSignedInteger`：ioreg 偶尔把负 int64 打印成
/// uint64 二补码，需要还原。
fn parse_ioreg_signed_integer(raw: &str) -> Option<i64> {
    if let Ok(val) = raw.parse::<i64>() {
        return Some(val);
    }
    let val_uint = raw.parse::<u64>().ok()?;
    // u64 二补码 → i64。
    Some(val_uint as i64)
}

/// 1:1 移植 `ioRegValueForKey`：在 `"key" = value` 行内截取 value，
/// 遇 `, } ) 空白` 结束，剥去引号。
pub fn ioreg_value_for_key(line: &str, key: &str) -> Option<String> {
    let marker = format!("\"{key}\"");
    let mut rest = line.split_once(&marker)?.1;
    rest = rest.trim_start_matches([' ', '\t']);
    if !rest.starts_with('=') {
        return None;
    }
    rest = rest[1..].trim_start_matches([' ', '\t']);
    if rest.is_empty() || rest.starts_with(',') {
        return None;
    }
    let end = rest
        .char_indices()
        .find(|(_, r)| matches!(r, ',' | '}' | ')' | ' ' | '\t' | '\n' | '\r'))
        .map(|(i, _)| i)
        .unwrap_or(rest.len());
    let value = rest[..end].trim_matches('"');
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 对标 metrics_battery_test.go 的 parsePMSet 用例。
    #[test]
    fn parse_pm_set_ac_power() {
        let raw = "Now drawing from 'AC Power'\n -InternalBattery-0 (id=12345678)\t100%; charged; 0:00 remaining present: true\n";
        let batts = parse_pm_set(raw, "Normal", 120, 92);
        assert_eq!(batts.len(), 1);
        assert_eq!(batts[0].percent, 100.0);
        assert_eq!(batts[0].status, "charged");
        assert_eq!(batts[0].time_left, "0:00");
        assert_eq!(batts[0].cycle_count, 120);
        assert_eq!(batts[0].capacity, 92);
        assert_eq!(batts[0].health, "Normal");
    }

    #[test]
    fn parse_pm_set_battery_with_time() {
        let raw = "Now drawing from 'Battery Power'\n -InternalBattery-0 (id=1)\t85%; discharging; 4:20 remaining present: true\n";
        let batts = parse_pm_set(raw, "", 0, 0);
        assert_eq!(batts.len(), 1);
        assert_eq!(batts[0].percent, 85.0);
        assert_eq!(batts[0].status, "discharging");
        assert_eq!(batts[0].time_left, "4:20");
    }

    /// 对标 parseAppleSmartBatteryHealth 用例语义。
    #[test]
    fn smart_battery_health() {
        let out = "  \"CycleCount\" = 187\n  \"DesignCapacity\" = 6279\n  \"NominalChargeCapacity\" = 5200\n  \"AppleRawMaxCapacity\" = 5237\n  \"MaxCapacity\" = 100\n";
        let (cycles, capacity) = parse_apple_smart_battery_health(out);
        assert_eq!(cycles, 187);
        // 优先 AppleRawMaxCapacity：5237/6279*100 ≈ 83。
        assert_eq!(capacity, ((5237f64 * 100.0 / 6279.0).round()) as i64);
    }

    #[test]
    fn smart_battery_health_falls_back_to_nominal() {
        let out = "  \"CycleCount\" = 50\n  \"DesignCapacity\" = 1000\n  \"NominalChargeCapacity\" = 850\n";
        let (_, capacity) = parse_apple_smart_battery_health(out);
        assert_eq!(capacity, 85);
    }

    /// 对标 ioRegValueForKey 用例（含逗号、引号、嵌套）。
    #[test]
    fn ioreg_value_parsing() {
        assert_eq!(
            ioreg_value_for_key("  \"Voltage\" = 11660\n", "Voltage"),
            Some("11660".to_string())
        );
        assert_eq!(
            ioreg_value_for_key("  \"BatterySerialNumber\" = \"F7Y1234ABC\"\n", "BatterySerialNumber"),
            Some("F7Y1234ABC".to_string())
        );
        assert_eq!(
            ioreg_value_for_key("  \"Temperature\" = 2987, \"x\" = 1\n", "Temperature"),
            Some("2987".to_string())
        );
        assert_eq!(ioreg_value_for_key("nope", "Voltage"), None);
    }

    #[test]
    fn signed_integer_two_complement() {
        assert_eq!(parse_ioreg_signed_integer("-1500"), Some(-1500));
        // u64 二补码：-1500 = 2^64 - 1500。
        let raw = (u64::MAX - 1499).to_string();
        assert_eq!(parse_ioreg_signed_integer(&raw), Some(-1500));
    }

    /// 对标 parseAppleSmartBatteryThermal 用例。
    #[test]
    fn battery_thermal_from_ioreg() {
        let out = "  \"Temperature\" = 2987\n  \"Voltage\" = 11660\n  \"InstantAmperage\" = -1500\n  \"SystemPower\" = 12500\n";
        let thermal = parse_apple_smart_battery_thermal(out);
        assert!((thermal.battery_temp - 29.87).abs() < 0.001);
        assert!((thermal.system_power - 12.5).abs() < 0.001);
        // -(11660 * -1500) / 1e6 = 17.49W（放电为正）。
        assert!((thermal.battery_power - 17.49).abs() < 0.01);
        assert_eq!(thermal.adapter_power, 0.0);
    }

    #[test]
    fn adapter_details_skip_raw() {
        let out = "  \"AppleRawAdapterDetails\" = {\"Watts\" = 140}\n  \"AdapterDetails\" = {\"Watts\" = 96, \"Amperage\" = 4900}\n";
        let thermal = parse_apple_smart_battery_thermal(out);
        assert!((thermal.adapter_power - 96.0).abs() < 0.001);
    }

    /// 对标 parseSystemPowerText 用例。
    #[test]
    fn system_power_text() {
        let out = "    Battery Health Information:\n      Cycle Count: 42\n      Condition: Normal\n      Maximum Capacity: 98%\n";
        let (health, cycles, capacity) = parse_system_power_text(out);
        assert_eq!(health, "Normal");
        assert_eq!(cycles, 42);
        assert_eq!(capacity, 98);
    }

    /// 对标 parseSystemPowerJSON 用例。
    #[test]
    fn system_power_json() {
        let raw = r#"{"SPPowerDataType":[{"sppower_battery_health_info":{"sppower_battery_cycle_count":150,"sppower_battery_health":"Normal","sppower_battery_health_maximum_capacity":"91%"}}]}"#;
        let (health, cycles, capacity) = parse_system_power_json(raw).unwrap();
        assert_eq!(health, "Normal");
        assert_eq!(cycles, 150);
        assert_eq!(capacity, 91);
        assert!(parse_system_power_json("not json").is_none());
    }
}
