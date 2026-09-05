//! 蓝牙设备，对标 `cmd/status/metrics_bluetooth.go`。

use super::types::BluetoothDevice;
use std::time::{Duration, Instant};

/// 对标常量。
const BLUETOOTH_CACHE_TTL: Duration = Duration::from_secs(30);
const BLUETOOTHCTL_TIMEOUT: Duration = Duration::from_millis(1500);
const SYSTEM_PROFILER_TIMEOUT: Duration = Duration::from_secs(4);

/// 30s 缓存（对标 lastBT/lastBTAt）。
#[derive(Default)]
pub struct BluetoothCache {
    pub devices: Vec<BluetoothDevice>,
    pub last_at: Option<Instant>,
}

/// 对标 `collectBluetooth`：system_profiler → bluetoothctl 回退 →
/// 30s 缓存与占位设备。
pub fn collect_bluetooth(cache: &mut BluetoothCache) -> Vec<BluetoothDevice> {
    if !cache.devices.is_empty() && cache.last_at.is_some_and(|t| t.elapsed() < BLUETOOTH_CACHE_TTL)
    {
        return cache.devices.clone();
    }

    if let Some(devs) = read_system_profiler_bluetooth() {
        if !devs.is_empty() {
            cache.devices = devs;
            cache.last_at = Some(Instant::now());
            return cache.devices.clone();
        }
    }

    if let Some(devs) = read_bluetoothctl_devices() {
        if !devs.is_empty() {
            cache.devices = devs;
            cache.last_at = Some(Instant::now());
            return cache.devices.clone();
        }
    }

    cache.last_at = Some(Instant::now());
    if cache.devices.is_empty() {
        cache.devices = vec![BluetoothDevice {
            name: "No Bluetooth info".into(),
            connected: false,
            battery: String::new(),
        }];
    }
    cache.devices.clone()
}

fn read_system_profiler_bluetooth() -> Option<Vec<BluetoothDevice>> {
    if !super::command_exists("system_profiler") {
        return None;
    }
    let out = super::run_cmd(
        "system_profiler",
        &["SPBluetoothDataType"],
        SYSTEM_PROFILER_TIMEOUT,
    )
    .ok()?;
    Some(parse_sp_bluetooth(&out))
}

fn read_bluetoothctl_devices() -> Option<Vec<BluetoothDevice>> {
    if !super::command_exists("bluetoothctl") {
        return None;
    }
    let out = super::run_cmd("bluetoothctl", &["info"], BLUETOOTHCTL_TIMEOUT).ok()?;
    Some(parse_bluetoothctl(&out))
}

/// 1:1 移植 `parseSPBluetooth`：基于缩进的层级解析——顶层节（无 4 空格
/// 前缀且以冒号结尾）重置状态；8 空格缩进的 "name:" 行开新设备。
pub fn parse_sp_bluetooth(raw: &str) -> Vec<BluetoothDevice> {
    let mut devices: Vec<BluetoothDevice> = Vec::new();
    let mut current_name = String::new();
    let mut connected = false;
    let mut battery = String::new();

    for line in raw.lines() {
        let trim = line.trim();
        if trim.is_empty() {
            continue;
        }
        if !line.starts_with("    ") && trim.ends_with(':') {
            // 顶层节重置。
            current_name = String::new();
            connected = false;
            battery = String::new();
            continue;
        }
        if line.starts_with("        ") && trim.ends_with(':') {
            if !current_name.is_empty() {
                devices.push(BluetoothDevice {
                    name: current_name.clone(),
                    connected,
                    battery: battery.clone(),
                });
            }
            current_name = trim.trim_end_matches(':').to_string();
            connected = false;
            battery = String::new();
            continue;
        }
        if trim.contains("Connected:") {
            connected = trim.contains("Yes");
        }
        if trim.contains("Battery Level:") {
            let after = trim.split_once("Battery Level:").map(|(_, a)| a).unwrap_or("");
            battery = after.trim().to_string();
        }
    }
    if !current_name.is_empty() {
        devices.push(BluetoothDevice {
            name: current_name,
            connected,
            battery,
        });
    }
    if devices.is_empty() {
        devices.push(BluetoothDevice {
            name: "No devices".into(),
            connected: false,
            battery: String::new(),
        });
    }
    devices
}

/// 1:1 移植 `parseBluetoothctl`（Linux 回退路径；macOS 上通常不可达，
/// 保留以对齐原实现的行为面）。
pub fn parse_bluetoothctl(raw: &str) -> Vec<BluetoothDevice> {
    let mut devices: Vec<BluetoothDevice> = Vec::new();
    let mut current = BluetoothDevice {
        name: String::new(),
        connected: false,
        battery: String::new(),
    };
    for line in raw.lines() {
        let trim = line.trim();
        if let Some(rest) = trim.strip_prefix("Device ") {
            if !current.name.is_empty() {
                devices.push(current.clone());
            }
            current = BluetoothDevice {
                name: rest.to_string(),
                connected: false,
                battery: String::new(),
            };
        }
        if let Some(after) = trim.strip_prefix("Name:") {
            current.name = after.trim().to_string();
        }
        if trim.starts_with("Connected:") {
            current.connected = trim.contains("yes");
        }
    }
    if !current.name.is_empty() {
        devices.push(current);
    }
    if devices.is_empty() {
        devices.push(BluetoothDevice {
            name: "No devices".into(),
            connected: false,
            battery: String::new(),
        });
    }
    devices
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 对标 parseSPBluetooth 用例（8 空格设备头 + 属性行 + Connected/Battery）。
    #[test]
    fn sp_bluetooth_parsing() {
        let raw = "Bluetooth:\n\n    Bluetooth Controller:\n        Address: AA\n\n    Connected:\n\n        AirPods Pro:\n            Address: BB\n            Connected: Yes\n            Battery Level: 80%\n\n        Magic Mouse:\n            Address: CC\n            Connected: No\n";
        let devices = parse_sp_bluetooth(raw);
        let named: Vec<_> = devices.iter().filter(|d| d.name == "AirPods Pro").collect();
        assert_eq!(named.len(), 1);
        assert!(named[0].connected);
        assert_eq!(named[0].battery, "80%");
        let mouse = devices.iter().find(|d| d.name == "Magic Mouse").unwrap();
        assert!(!mouse.connected);
        // 顶层节（无 4 空格前缀）不产生设备。
        assert!(!devices.iter().any(|d| d.name == "Bluetooth Controller"));
    }

    #[test]
    fn sp_bluetooth_empty_fallback() {
        let devices = parse_sp_bluetooth("Bluetooth:\n");
        assert_eq!(devices[0].name, "No devices");
    }

    /// 对标 parseBluetoothctl 用例。
    #[test]
    fn bluetoothctl_parsing() {
        let raw = "Device AA:BB DeviceAlias\n    Name: My Headset\n    Connected: yes\n";
        let devices = parse_bluetoothctl(raw);
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].name, "My Headset");
        assert!(devices[0].connected);
    }
}
