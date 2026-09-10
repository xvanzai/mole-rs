//! analyze 本地快照计数，对标 `cmd/analyze/snapshots.go`。
//!
//! `tmutil listlocalsnapshotdates /` 输出中仅 YYYY-MM-DD-HHMMSS 行计数；
//! 快照大小不推断（tmutil 不暴露保留字节）。

use std::time::Duration;

const PROBE_TIMEOUT: Duration = Duration::from_secs(3);

/// 对标 parseLocalSnapshotCount：解析 tmutil 输出中的日期行数。
pub fn parse_local_snapshot_count(data: &str) -> usize {
    data.lines()
        .filter(|line| is_snapshot_date_line(line.trim()))
        .count()
}

/// 对标 time.Parse("2006-01-02-150405")：YYYY-MM-DD-HHMMSS（17 字符）。
fn is_snapshot_date_line(s: &str) -> bool {
    if s.len() != 17 {
        return false;
    }
    let b = s.as_bytes();
    let digit = |i: usize| b[i].is_ascii_digit();
    let dash = |i: usize| b[i] == b'-';
    // YYYY-MM-DD-HHMMSS
    (0..4).all(digit)
        && dash(4)
        && (5..7).all(digit)
        && dash(7)
        && (8..10).all(digit)
        && dash(10)
        && (11..17).all(digit)
}

/// 探测本地快照数量；tmutil 不可用返回 None。
pub fn count_local_snapshots() -> Option<usize> {
    if !crate::clean::command_exists("tmutil") {
        return None;
    }
    let out = crate::status::run_cmd("tmutil", &["listlocalsnapshotdates", "/"], PROBE_TIMEOUT).ok()?;
    Some(parse_local_snapshot_count(&out))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 日期行计数：忽略标题与杂行。
    #[test]
    fn snapshot_count_parsing() {
        let raw = "Hostname: Mac\n2024-01-15-104530\n2024-02-20-081200\n\nNot a date\n";
        assert_eq!(parse_local_snapshot_count(raw), 2);
        assert_eq!(parse_local_snapshot_count(""), 0);
        assert_eq!(parse_local_snapshot_count("2024-1-5-104530\n"), 0); // 零填充要求
    }

    /// 真机 tmutil 探测不 panic。
    #[test]
    fn snapshot_probe_no_panic() {
        let _ = count_local_snapshots();
    }
}
