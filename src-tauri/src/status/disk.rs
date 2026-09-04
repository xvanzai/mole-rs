//! 磁盘指标与废纸篓扫描，对标 `cmd/status/metrics_disk.go`。

use super::types::DiskStatus;
use std::collections::HashSet;
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
/// `use_corrections` 为 true 时对应 full 路径；APFS purgeable / diskutil
/// 元数据修正属于子进程密集操作，见 CHANGES.md §status（暂缓子项），
/// 本版两种路径都返回 raw statfs 数据。
pub fn collect_disks(_use_corrections: bool) -> Vec<DiskStatus> {
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
        // Skip <1GB volumes.
        if part.total < 1 << 30 {
            continue;
        }
        // Use size-based dedupe key for shared pools.
        let vol_key = format!("{}:{}", part.fstype, part.total);
        if seen_volume.contains(&vol_key) {
            continue;
        }

        disks.push(DiskStatus {
            mount: part.mount.clone(),
            device: part.device.clone(),
            used: part.used,
            total: part.total,
            used_percent: if part.total > 0 {
                part.used as f64 / part.total as f64 * 100.0
            } else {
                0.0
            },
            fstype: part.fstype.clone(),
            external: part.mount.starts_with("/Volumes/"),
            smart_status: SMART_STATUS_UNKNOWN.into(),
            purgeable: 0,
        });
        seen_device.insert(base_device);
        seen_volume.insert(vol_key);
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
