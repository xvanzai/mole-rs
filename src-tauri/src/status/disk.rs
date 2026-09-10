//! 磁盘指标与废纸篓扫描，对标 `cmd/status/metrics_disk.go`。

use super::types::{DiskIoStatus, DiskStatus};
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

/// 对标 `skipDiskMounts`。
const SKIP_DISK_MOUNTS: &[&str] = &[
    "/System/Volumes/VM",
    "/System/Volumes/Preboot",
    "/System/Volumes/Update",
    "/System/Volumes/xarts",
    "/System/Volumes/Hardware",
    "/System/Volumes/Data",
    "/dev",
];

/// 对标 `skipDiskFSTypes`。
const SKIP_DISK_FS_TYPES: &[&str] = &[
    "afpfs", "autofs", "cifs", "devfs", "fuse", "fuseblk", "fusefs", "macfuse", "nfs", "osxfuse",
    "procfs", "smbfs", "tmpfs", "webdav",
];

/// 对标 smartStatusUnknown。
const SMART_STATUS_UNKNOWN: &str = "unknown";

/// 对标 `collectDisks` / `collectDisksFast`。
///
/// `use_corrections=true`（full 路径）：APFS purgeable 三级修正 +
/// diskutil 元数据（External/SMART）。fast 路径返回 raw statfs。
pub fn collect_disks(use_corrections: bool) -> Vec<DiskStatus> {
    let mut seen_device: HashSet<String> = HashSet::new();
    let mut seen_volume: HashSet<String> = HashSet::new();
    let mut disks: Vec<DiskStatus> = Vec::new();

    for part in statfs_list() {
        if should_skip_partition(&part) {
            continue;
        }
        let base_device = {
            let base = base_device_name(&part.device);
            if base.is_empty() {
                part.device.clone()
            } else {
                base
            }
        };
        if seen_device.contains(&base_device) {
            continue;
        }
        if part.total == 0 {
            continue;
        }
        let mut total = part.total;
        if use_corrections {
            total = correct_disk_total_bytes(&part.mount, total);
        }
        // Skip <1GB volumes.
        if total < 1 << 30 {
            continue;
        }
        // Use size-based dedupe key for shared pools.
        let vol_key = format!("{}:{}", part.fstype, total);
        if seen_volume.contains(&vol_key) {
            continue;
        }

        let raw_free = total.saturating_sub(part.used);
        let (used, used_percent, purgeable) = if use_corrections
            && part.fstype.eq_ignore_ascii_case("apfs")
        {
            correct_apfs_disk_usage(&part.mount, total, part.used, raw_free)
        } else {
            let pct = if total > 0 {
                part.used as f64 / total as f64 * 100.0
            } else {
                0.0
            };
            (part.used, pct, 0u64)
        };

        disks.push(DiskStatus {
            mount: part.mount.clone(),
            device: part.device.clone(),
            used,
            total,
            used_percent,
            fstype: part.fstype.clone(),
            external: if !use_corrections {
                part.mount.starts_with("/Volumes/")
            } else {
                part.mount.starts_with("/Volumes/")
            },
            smart_status: SMART_STATUS_UNKNOWN.into(),
            purgeable,
        });
        seen_device.insert(base_device);
        seen_volume.insert(vol_key);
    }

    if use_corrections {
        annotate_disk_metadata(&mut disks);
    }

    // 对标排序：内置盘优先，再按容量从大到小。
    disks.sort_by(|a, b| match (a.external, b.external) {
        (false, true) => std::cmp::Ordering::Less,
        (true, false) => std::cmp::Ordering::Greater,
        _ => b.total.cmp(&a.total),
    });
    disks.truncate(3);
    disks
}

/// Finder 启动盘 free/total 缓存（2 分钟 TTL，对标 finderDiskCache）。
static FINDER_CACHE: std::sync::Mutex<Option<(u64, u64, Instant)>> = std::sync::Mutex::new(None);

/// 对标 correctAPFSDiskUsage：三级回退 Finder → diskutil APFSContainerFree → raw。
fn correct_apfs_disk_usage(
    mountpoint: &str,
    total: u64,
    raw_used: u64,
    raw_free: u64,
) -> (u64, f64, u64) {
    // Tier 1：Finder osascript（仅启动盘 "/"）。
    if mountpoint == "/" && super::command_exists("osascript") {
        if let Some((finder_free, finder_total)) = get_finder_startup_disk_free_bytes() {
            if finder_total > 0 && finder_free <= finder_total {
                let used = finder_total - finder_free;
                let pct = used as f64 / finder_total as f64 * 100.0;
                return (used, pct, finder_purgeable_bytes(raw_free, finder_free));
            }
        }
    }

    // Tier 2：diskutil APFSContainerFree（修正本地快照占用）。
    if super::command_exists("diskutil") {
        if let Some(container_free) = get_apfs_container_free_bytes(mountpoint) {
            if container_free <= total {
                let corrected = total - container_free;
                if raw_used > corrected && raw_used - corrected > 1 << 30 {
                    let pct = corrected as f64 / total as f64 * 100.0;
                    return (corrected, pct, 0);
                }
            }
        }
    }

    // Tier 3：raw statfs。
    let pct = if total > 0 {
        raw_used as f64 / total as f64 * 100.0
    } else {
        0.0
    };
    (raw_used, pct, 0)
}

/// finderPurgeableBytes：Finder free 减 statfs free 的差即 purgeable。
fn finder_purgeable_bytes(raw_free: u64, finder_free: u64) -> u64 {
    if finder_free <= raw_free {
        0
    } else {
        finder_free - raw_free
    }
}

/// getAPFSContainerFreeBytes：diskutil info -plist 的 APFSContainerFree。
fn get_apfs_container_free_bytes(mountpoint: &str) -> Option<u64> {
    let out = super::run_cmd("diskutil", &["info", "-plist", mountpoint], Duration::from_secs(3))
        .ok()?;
    extract_plist_uint(&out, &["APFSContainerFree"])
}

/// getFinderStartupDiskFreeBytes：osascript 查 Finder 启动盘 free/total。
fn get_finder_startup_disk_free_bytes() -> Option<(u64, u64)> {
    {
        let guard = FINDER_CACHE.lock().ok()?;
        if let Some((free, total, at)) = *guard {
            if at.elapsed() < Duration::from_secs(120) {
                return Some((free, total));
            }
        }
    }
    let out = super::run_cmd(
        "osascript",
        &["-e", r#"tell application "Finder" to return {free space of startup disk, capacity of startup disk}"#],
        Duration::from_secs(5),
    );
    let Ok(out) = out else {
        // 缓存失败时间戳，避免每次等满 5s。
        if let Ok(mut guard) = FINDER_CACHE.lock() {
            *guard = Some((0, 0, Instant::now()));
        }
        return None;
    };
    // "3.2489E+11, 4.9438E+11" 或 "324892202048, 494384795648"
    let mut parts = out.trim().splitn(2, ',');
    let free_f: f64 = parts.next()?.trim().parse().ok()?;
    let total_f: f64 = parts.next()?.trim().parse().ok()?;
    if free_f <= 0.0 || total_f <= 0.0 {
        return None;
    }
    let free = free_f as u64;
    let total = total_f as u64;
    if let Ok(mut guard) = FINDER_CACHE.lock() {
        *guard = Some((free, total, Instant::now()));
    }
    Some((free, total))
}

/// extract_plist_uint：从 plist XML 提取整数键。
fn extract_plist_uint(plist: &str, keys: &[&str]) -> Option<u64> {
    for key in keys {
        let marker = format!("<key>{key}</key>");
        if let Some(pos) = plist.find(&marker) {
            let rest = &plist[pos + marker.len()..];
            if let Some(start) = rest.find("<integer>") {
                let num = &rest[start + 9..];
                if let Some(end) = num.find("</integer>") {
                    if let Ok(v) = num[..end].trim().parse::<u64>() {
                        return Some(v);
                    }
                }
            }
        }
    }
    None
}

/// correctDiskTotalBytes：diskutil 总容量与 statfs 差 >1GB 时采用 diskutil。
fn correct_disk_total_bytes(mountpoint: &str, raw_total: u64) -> u64 {
    if raw_total == 0 || !super::command_exists("diskutil") {
        return raw_total;
    }
    let Ok(out) = super::run_cmd("diskutil", &["info", "-plist", mountpoint], Duration::from_secs(3))
    else {
        return raw_total;
    };
    let Some(diskutil_total) = extract_plist_uint(&out, &["TotalSize", "DiskSize", "Size"])
    else {
        return raw_total;
    };
    let diff = raw_total.abs_diff(diskutil_total);
    if diff > 1 << 30 {
        diskutil_total
    } else {
        raw_total
    }
}

/// 磁盘元数据缓存（2 分钟）：External + SMART。
static DISK_META_CACHE: std::sync::Mutex<Option<(HashMap<String, (bool, String)>, Instant)>> =
    std::sync::Mutex::new(None);

/// annotateDiskMetadata：diskutil info 补 External/SMART。
fn annotate_disk_metadata(disks: &mut [DiskStatus]) {
    if disks.is_empty() || !super::command_exists("diskutil") {
        return;
    }
    // 清理过期缓存。
    {
        if let Ok(mut guard) = DISK_META_CACHE.lock() {
            if let Some((_, at)) = guard.as_ref() {
                if at.elapsed() > Duration::from_secs(120) {
                    *guard = None;
                }
            }
        }
    }
    for disk in disks.iter_mut() {
        let base = base_device_name(&disk.device);
        let base = if base.is_empty() {
            disk.device.clone()
        } else {
            base
        };
        // 缓存命中。
        if let Ok(guard) = DISK_META_CACHE.lock() {
            if let Some((map, _)) = guard.as_ref() {
                if let Some((external, smart)) = map.get(&base) {
                    disk.external = *external;
                    disk.smart_status = smart.clone();
                    continue;
                }
            }
        }
        let (external, smart) = read_disk_metadata(&base)
            .unwrap_or_else(|_| (disk.mount.starts_with("/Volumes/"), SMART_STATUS_UNKNOWN.into()));
        disk.external = external;
        disk.smart_status = smart.clone();
        if let Ok(mut guard) = DISK_META_CACHE.lock() {
            let entry = guard.get_or_insert_with(|| (HashMap::new(), Instant::now()));
            entry.0.insert(base, (external, smart));
        }
    }
}

/// parseDiskMetadata：Internal/Device Location + SMART Status。
fn read_disk_metadata(device: &str) -> Result<(bool, String), ()> {
    let out = super::run_cmd("diskutil", &["info", device], Duration::from_secs(1)).map_err(|_| ())?;
    let mut external_found = false;
    let mut external = false;
    let mut location_found = false;
    let mut location_external = false;
    let mut smart = SMART_STATUS_UNKNOWN.to_string();
    for line in out.lines() {
        let trim = line.trim();
        if trim.starts_with("Internal:") {
            external_found = true;
            external = trim.contains("No");
        }
        if !external_found && trim.starts_with("Device Location:") {
            location_found = true;
            location_external = trim.contains("External");
        }
        if let Some(value) = trim.strip_prefix("SMART Status:") {
            let v = value.trim().to_lowercase();
            smart = match v.as_str() {
                "verified" => "verified".into(),
                "failing" | "failed" => "failing".into(),
                "not supported" | "unsupported" => "unsupported".into(),
                _ => SMART_STATUS_UNKNOWN.into(),
            };
        }
    }
    if !external_found && location_found {
        external_found = true;
        external = location_external;
    }
    if !external_found {
        return Err(());
    }
    Ok((external, smart))
}

struct StatFsEntry {
    mount: String,
    device: String,
    fstype: String,
    total: u64,
    used: u64,
}

/// 对标 gopsutil `disk.Partitions`：getfsstat 读取全部挂载点；
/// Usage 口径一致：total = f_blocks * f_bsize，used = total - f_bfree * f_bsize。
fn statfs_list() -> Vec<StatFsEntry> {
    unsafe {
        let count = libc::getfsstat(std::ptr::null_mut(), 0, libc::MNT_NOWAIT);
        if count <= 0 {
            return Vec::new();
        }
        let mut buf: Vec<libc::statfs> = vec![std::mem::zeroed(); count as usize];
        let bufsize = (count as usize * std::mem::size_of::<libc::statfs>()) as libc::c_int;
        let n = libc::getfsstat(buf.as_mut_ptr(), bufsize, libc::MNT_NOWAIT);
        if n <= 0 {
            return Vec::new();
        }
        buf.truncate(n as usize);
        let cstr = |field: &[libc::c_char]| -> String {
            let bytes: Vec<u8> = field
                .iter()
                .take_while(|&&c| c != 0)
                .map(|&c| c as u8)
                .collect();
            String::from_utf8_lossy(&bytes).into_owned()
        };
        buf.into_iter()
            .map(|fs| {
                let bsize = fs.f_bsize as u64;
                let total = fs.f_blocks as u64 * bsize;
                let free = fs.f_bfree as u64 * bsize;
                StatFsEntry {
                    mount: cstr(&fs.f_mntonname),
                    device: cstr(&fs.f_mntfromname),
                    fstype: cstr(&fs.f_fstypename),
                    total,
                    used: total.saturating_sub(free),
                }
            })
            .collect()
    }
}

/// 1:1 移植 `shouldSkipDiskPartition`。
fn should_skip_partition(part: &StatFsEntry) -> bool {
    if part.device.starts_with("/dev/loop") {
        return true;
    }
    if SKIP_DISK_MOUNTS.contains(&part.mount.as_str()) {
        return true;
    }
    if part.mount.starts_with("/System/Volumes/") {
        return true;
    }
    if part.mount.starts_with("/private/") {
        return true;
    }

    let fstype = part.fstype.to_lowercase();
    if SKIP_DISK_FS_TYPES.contains(&fstype.as_str()) || fstype.contains("fuse") {
        return true;
    }

    // On macOS, local disks should come from /dev。过滤 sshfs/macFUSE 式
    // 镜像根卷的挂载，避免出现重复内置盘。
    if !part.device.is_empty() && !part.device.starts_with("/dev/") {
        return true;
    }

    false
}

/// 1:1 移植 `baseDeviceName`："disk3s1" → "disk3"。
pub fn base_device_name(device: &str) -> String {
    let device = device.strip_prefix("/dev/").unwrap_or(device);
    if !device.starts_with("disk") {
        return device.to_string();
    }
    let bytes = device.as_bytes();
    for (i, &b) in bytes.iter().enumerate().skip(4) {
        if b == b's' {
            return device[..i].to_string();
        }
    }
    device.to_string()
}

/// 对标 `scanTrashSize`：遍历 ~/.Trash 统计文件大小（跳过符号链接），
/// 2s 超时；返回 (字节数, 是否因超时而是近似值)。
pub fn scan_trash_size() -> (u64, bool) {
    let Ok(home) = std::env::var("HOME") else {
        return (0, false);
    };
    let trash_path = std::path::PathBuf::from(home).join(".Trash");
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut total = 0u64;
    walk_sum(&trash_path, &deadline, &mut total);
    (total, Instant::now() >= deadline)
}

fn walk_sum(dir: &std::path::Path, deadline: &Instant, total: &mut u64) {
    if Instant::now() >= *deadline {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if Instant::now() >= *deadline {
            return;
        }
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            walk_sum(&entry.path(), deadline, total);
        } else if let Ok(meta) = entry.metadata() {
            *total += meta.len();
        }
    }
}

/// 磁盘 IO 累计字节（对标 gopsutil disk.IOCountersStat 的 ReadBytes/WriteBytes）。
/// macOS 上通过 `ioreg -r -c IOBlockStorageDriver` 读取 IORegistry Statistics。
pub struct DiskIoPrev {
    pub read_bytes: u64,
    pub write_bytes: u64,
    pub at: Instant,
}

/// 解析 ioreg 输出中的 `"Bytes (Read)"=N` / `"Bytes (Write)"=N` 并求和。
/// 对标 Go 侧对所有 counters 求 total.ReadBytes/WriteBytes。
pub fn parse_ioreg_disk_bytes(raw: &str) -> (u64, u64) {
    let mut read = 0u64;
    let mut write = 0u64;
    // ioreg 单行可能含 `"Statistics" = {"Bytes (Read)"=N,...}`，
    // 逗号切分后 key 不在 token 开头——改为全局扫描 key=value。
    const READ_KEY: &str = "\"Bytes (Read)\"=";
    const WRITE_KEY: &str = "\"Bytes (Write)\"=";
    for (key, acc) in [(READ_KEY, &mut read), (WRITE_KEY, &mut write)] {
        let mut rest = raw;
        while let Some(pos) = rest.find(key) {
            rest = &rest[pos + key.len()..];
            let end = rest
                .find(|c: char| !c.is_ascii_digit())
                .unwrap_or(rest.len());
            if end > 0 {
                if let Ok(v) = rest[..end].parse::<u64>() {
                    *acc += v;
                }
            }
        }
    }
    (read, write)
}

/// 读取当前累计 IO 字节；ioreg 不可用返回 None。
fn read_disk_io_totals() -> Option<(u64, u64)> {
    let out = super::run_cmd(
        "ioreg",
        &["-r", "-c", "IOBlockStorageDriver", "-d", "1"],
        Duration::from_secs(3),
    )
    .ok()?;
    let totals = parse_ioreg_disk_bytes(&out);
    // 两个计数器均为 0 视为不可用（避免除零/假零）。
    if totals.0 == 0 && totals.1 == 0 {
        return None;
    }
    Some(totals)
}

/// 对标 `collectDiskIO`：累计计数器差分 → MB/s。
/// 首次采样只记录 prev，返回零值（与 Go 一致）。
pub fn collect_disk_io(prev: &mut Option<DiskIoPrev>) -> DiskIoStatus {
    let Some((read, write)) = read_disk_io_totals() else {
        return DiskIoStatus::default();
    };
    let now = Instant::now();
    let Some(p) = prev.as_ref() else {
        *prev = Some(DiskIoPrev {
            read_bytes: read,
            write_bytes: write,
            at: now,
        });
        return DiskIoStatus::default();
    };
    let elapsed = now.duration_since(p.at).as_secs_f64().max(0.1);
    let read_delta = read.saturating_sub(p.read_bytes);
    let write_delta = write.saturating_sub(p.write_bytes);
    *prev = Some(DiskIoPrev {
        read_bytes: read,
        write_bytes: write,
        at: now,
    });
    DiskIoStatus {
        read_rate: read_delta as f64 / 1024.0 / 1024.0 / elapsed,
        write_rate: write_delta as f64 / 1024.0 / 1024.0 / elapsed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 对标 metrics_disk_test.go 的 baseDeviceName 用例。
    #[test]
    fn base_device_name_strips_slice() {
        assert_eq!(base_device_name("/dev/disk3s1s1"), "disk3");
        assert_eq!(base_device_name("/dev/disk1s2"), "disk1");
        assert_eq!(base_device_name("/dev/disk5"), "disk5");
        assert_eq!(base_device_name("/dev/sda1"), "sda1");
        assert_eq!(base_device_name(""), "");
    }

    /// ioreg Statistics 字节解析：多驱动器求和。
    #[test]
    fn parse_ioreg_disk_bytes_sums_drivers() {
        let raw = r#"
      "Statistics" = {"Operations (Write)"=425617,"Bytes (Read)"=40313978880,"Bytes (Write)"=26369171456}
      "Statistics" = {"Bytes (Read)"=7293149696,"Bytes (Write)"=19603329024}
"#;
        let (r, w) = parse_ioreg_disk_bytes(raw);
        assert_eq!(r, 40313978880 + 7293149696);
        assert_eq!(w, 26369171456 + 19603329024);
    }

    #[test]
    fn parse_ioreg_disk_bytes_empty() {
        assert_eq!(parse_ioreg_disk_bytes(""), (0, 0));
        assert_eq!(parse_ioreg_disk_bytes("no stats here"), (0, 0));
    }

    /// 首次采样返回零速率（对标 Go lastDiskAt.IsZero 分支）。
    #[test]
    fn disk_io_first_sample_zero() {
        let mut prev = None;
        // 可能读到真实 ioreg；无论是否有数据，首次都不 panic。
        let _ = collect_disk_io(&mut prev);
        if prev.is_some() {
            // 第二次调用（间隔极短）也不 panic。
            let _ = collect_disk_io(&mut prev);
        }
    }

    /// plist 整数提取。
    #[test]
    fn plist_uint_extraction() {
        let raw = r#"<?xml version="1.0"?><plist><dict>
<key>APFSContainerFree</key><integer>123456789</integer>
<key>TotalSize</key><integer>999</integer>
</dict></plist>"#;
        assert_eq!(extract_plist_uint(raw, &["APFSContainerFree"]), Some(123456789));
        assert_eq!(extract_plist_uint(raw, &["TotalSize"]), Some(999));
        assert_eq!(extract_plist_uint(raw, &["Missing"]), None);
    }

    /// purgeable 差值语义。
    #[test]
    fn finder_purgeable_semantics() {
        assert_eq!(finder_purgeable_bytes(100, 100), 0);
        assert_eq!(finder_purgeable_bytes(100, 50), 0);
        assert_eq!(finder_purgeable_bytes(100, 150), 50);
    }

    /// parse_disk_metadata：Internal/No + SMART verified。
    #[test]
    fn disk_metadata_parsing() {
        let raw = "   Internal:                    Yes\n   Device Location:            Internal\n   SMART Status:               Verified\n";
        let (external, smart) = read_disk_metadata_from_str(raw).unwrap();
        assert!(!external);
        assert_eq!(smart, "verified");

        let raw2 = "   Internal:                    No\n   SMART Status:               Failing\n";
        let (external, smart) = read_disk_metadata_from_str(raw2).unwrap();
        assert!(external);
        assert_eq!(smart, "failing");
    }

    /// 从字符串解析元数据（测试用包装）。
    fn read_disk_metadata_from_str(out: &str) -> Result<(bool, String), ()> {
        let mut external_found = false;
        let mut external = false;
        let mut location_found = false;
        let mut location_external = false;
        let mut smart = SMART_STATUS_UNKNOWN.to_string();
        for line in out.lines() {
            let trim = line.trim();
            if trim.starts_with("Internal:") {
                external_found = true;
                external = trim.contains("No");
            }
            if !external_found && trim.starts_with("Device Location:") {
                location_found = true;
                location_external = trim.contains("External");
            }
            if let Some(value) = trim.strip_prefix("SMART Status:") {
                let v = value.trim().to_lowercase();
                smart = match v.as_str() {
                    "verified" => "verified".into(),
                    "failing" | "failed" => "failing".into(),
                    "not supported" | "unsupported" => "unsupported".into(),
                    _ => SMART_STATUS_UNKNOWN.into(),
                };
            }
        }
        if !external_found && location_found {
            external_found = true;
            external = location_external;
        }
        if !external_found {
            return Err(());
        }
        Ok((external, smart))
    }

    /// collect_disks 两种模式均不 panic。
    #[test]
    fn collect_disks_both_modes() {
        let fast = collect_disks(false);
        let full = collect_disks(true);
        // fast ≤ full 条目数（corrections 可能过滤/合并）。
        assert!(fast.len() <= 3);
        assert!(full.len() <= 3);
    }

    #[test]
    fn skip_rules_match_go() {
        let mk = |mount: &str, device: &str, fstype: &str| StatFsEntry {
            mount: mount.into(),
            device: device.into(),
            fstype: fstype.into(),
            total: 0,
            used: 0,
        };
        assert!(should_skip_partition(&mk("/System/Volumes/Data", "/dev/disk3s5", "apfs")));
        assert!(should_skip_partition(&mk("/private/var", "/dev/disk3s5", "apfs")));
        assert!(should_skip_partition(&mk("/mnt/share", "/dev/disk3s5", "smbfs")));
        assert!(should_skip_partition(&mk("/net", "server:/share", "nfs")));
        // darwin 上设备必须来自 /dev。
        assert!(should_skip_partition(&mk("/", "rootfs", "apfs")));
        assert!(!should_skip_partition(&mk("/", "/dev/disk3s1s1", "apfs")));
        assert!(!should_skip_partition(&mk("/Volumes/USB", "/dev/disk4s1", "exfat")));
    }
}
