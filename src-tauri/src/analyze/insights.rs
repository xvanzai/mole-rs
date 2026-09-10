//! analyze 隐藏空间洞察条目，对标 `cmd/analyze/insights.go` 的
//! `createInsightEntries` 与大小测量。
//!
//! 洞察条目：iOS 备份、旧下载（90 天+）、以及 mo clean 可清理的已知路径。
//! Size 初始为 -1（未知），由 measure_insight_size 按需测量。

use super::DirEntry;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// 对标 createInsightEntries：返回存在的洞察目录条目（size=-1 待测）。
pub fn create_insight_entries() -> Vec<DirEntry> {
    let Ok(home) = std::env::var("HOME") else {
        return Vec::new();
    };
    let mut entries = Vec::new();
    let mut push_if_dir = |name: &str, path: PathBuf| {
        if path.is_dir() {
            entries.push(DirEntry {
                name: name.into(),
                path: path.to_string_lossy().to_string(),
                size: 0, // 占位；UI 用 size=-1 语义时由 measure 填充
                is_dir: true,
                last_access: 0,
            });
        }
    };

    // iOS Backups。
    push_if_dir(
        "iOS Backups",
        PathBuf::from(&home).join("Library/Application Support/MobileSync/Backup"),
    );
    // Old Downloads（文件级 90 天过滤由 measure 处理）。
    push_if_dir("Old Downloads (90d+)", PathBuf::from(&home).join("Downloads"));

    // Cleanable paths（对标 cleanablePaths）。
    let cleanable: &[(&str, &str)] = &[
        ("System Logs", "Library/Logs"),
        ("Homebrew Cache", "Library/Caches/Homebrew"),
        ("Xcode DerivedData", "Library/Developer/Xcode/DerivedData"),
        ("Xcode Simulators", "Library/Developer/CoreSimulator/Devices"),
        ("Xcode Archives", "Library/Developer/Xcode/Archives"),
        ("Spotify Cache", "Library/Application Support/Spotify/PersistentCache"),
        ("JetBrains Cache", "Library/Caches/JetBrains"),
        ("Docker Data", "Library/Containers/com.docker.docker/Data"),
        ("pip Cache", "Library/Caches/pip"),
        ("uv Cache", ".cache/uv"),
        ("Gradle Cache", ".gradle/caches"),
        ("CocoaPods Cache", "Library/Caches/CocoaPods"),
    ];
    for (name, rel) in cleanable {
        push_if_dir(name, PathBuf::from(&home).join(rel));
    }

    // OrbStack：Group Containers/*dev.orbstack/data。
    let gc = PathBuf::from(&home).join("Library/Group Containers");
    if let Ok(entries_dir) = std::fs::read_dir(&gc) {
        for e in entries_dir.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if name.contains("dev.orbstack") {
                let data = e.path().join("data");
                if data.is_dir() {
                    push_if_dir("OrbStack Data", data);
                }
                break;
            }
        }
    }

    // 尺寸占位：DirEntry.size 用 0，UI 调用 measure_insight_size。
    for e in &mut entries {
        e.size = 0;
    }
    entries
}

/// 对标 measureInsightSize：Downloads 特判 90 天过滤；其余整树。
pub fn measure_insight_size(path: &str) -> u64 {
    let home = std::env::var("HOME").unwrap_or_default();
    let downloads = format!("{home}/Downloads");
    if path == downloads {
        return measure_old_downloads(path, 90);
    }
    let deadline = Instant::now() + Duration::from_secs(10);
    crate::clean::path_size_with_deadline(Path::new(path), deadline)
}

/// measureOldDownloads：仅计 mtime 早于 daysOld 的非隐藏项。
fn measure_old_downloads(dir: &str, days_old: u64) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let cutoff = SystemTime::now()
        .checked_sub(Duration::from_secs(days_old * 86400))
        .unwrap_or(UNIX_EPOCH);
    let mut total = 0u64;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        let Ok(modified) = meta.modified() else {
            continue;
        };
        if modified >= cutoff {
            continue;
        }
        if meta.is_dir() {
            let deadline = Instant::now() + Duration::from_secs(10);
            total += crate::clean::path_size_with_deadline(&entry.path(), deadline);
        } else {
            total += meta.len();
        }
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 洞察条目：已存在的 cleanable 路径至少包含 Logs（本机必有）。
    #[test]
    fn insights_include_logs_when_present() {
        let entries = create_insight_entries();
        let home = std::env::var("HOME").unwrap();
        let logs = format!("{home}/Library/Logs");
        if Path::new(&logs).is_dir() {
            assert!(
                entries.iter().any(|e| e.name == "System Logs"),
                "应包含 System Logs: {entries:?}"
            );
        }
        // 不 panic 即可；条目名唯一。
        let mut names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), entries.len(), "洞察名应唯一");
    }

    /// Old Downloads：90 天过滤不 panic。
    #[test]
    fn old_downloads_measure() {
        let home = std::env::var("HOME").unwrap();
        let dl = format!("{home}/Downloads");
        if Path::new(&dl).is_dir() {
            let _ = measure_old_downloads(&dl, 90);
        }
    }

    /// measure_insight_size 对不存在路径返回 0。
    #[test]
    fn measure_missing() {
        assert_eq!(measure_insight_size("/nonexistent/insight/path"), 0);
    }
}
