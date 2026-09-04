//! 字节容量格式化。
//!
//! 对标 `Mole/internal/units/bytes.go`（1:1 移植，行为契约见
//! `docs/migration/CHANGES.md` §core 工具层）：
//! - 磁盘相关数字用 SI（1000 进制），与 Finder / diskutil 一致；
//! - 内存与实时计数用二进制（1024 进制），与 macOS 活动监视器一致。
//!
//! 下方单元测试向量逐条翻译自 `bytes_test.go`，用于锁定边界行为。

/// SI（1000 进制）格式化，匹配 Finder/diskutil。
///
/// 对标 `BytesSI`：负数输入钳制为 `"0 B"`；单位序列 `k M G T P E`（小写 k）。
pub fn bytes_si(size: i64) -> String {
    if size < 0 {
        return "0 B".to_string();
    }
    const UNIT: i64 = 1000;
    if size < UNIT {
        return format!("{size} B");
    }
    let mut div: i64 = UNIT;
    let mut exp: usize = 0;
    let mut n = size / UNIT;
    while n >= UNIT {
        div *= UNIT;
        exp += 1;
        n /= UNIT;
    }
    let value = size as f64 / div as f64;
    const PREFIX: [char; 6] = ['k', 'M', 'G', 'T', 'P', 'E'];
    format!("{value:.1} {}B", PREFIX[exp])
}

/// 二进制（1024 进制）格式化，一位小数，带单位（如 `"1.0 GB"`）。
///
/// 对标 `BytesBin`：边界用 `>`，恰好 `1<<n` 停留在小单位（如 1024 → `"1024 B"`）。
pub fn bytes_bin(v: u64) -> String {
    const KB: u64 = 1 << 10;
    const MB: u64 = 1 << 20;
    const GB: u64 = 1 << 30;
    const TB: u64 = 1 << 40;
    if v > TB {
        format!("{:.1} TB", v as f64 / TB as f64)
    } else if v > GB {
        format!("{:.1} GB", v as f64 / GB as f64)
    } else if v > MB {
        format!("{:.1} MB", v as f64 / MB as f64)
    } else if v > KB {
        format!("{:.1} KB", v as f64 / KB as f64)
    } else {
        format!("{v} B")
    }
}

/// 二进制格式化，无小数、单字母后缀、无空格（如 `"100G"`）。
///
/// 对标 `BytesBinShort`：边界用 `>=`，恰好 `1<<n` 晋升大单位。
pub fn bytes_bin_short(v: u64) -> String {
    const KB: u64 = 1 << 10;
    const MB: u64 = 1 << 20;
    const GB: u64 = 1 << 30;
    const TB: u64 = 1 << 40;
    if v >= TB {
        format!("{:.0}T", v as f64 / TB as f64)
    } else if v >= GB {
        format!("{:.0}G", v as f64 / GB as f64)
    } else if v >= MB {
        format!("{:.0}M", v as f64 / MB as f64)
    } else if v >= KB {
        format!("{:.0}K", v as f64 / KB as f64)
    } else {
        v.to_string()
    }
}

/// 二进制格式化，一位小数、单字母后缀、无空格（如 `"1.5G"`）。
///
/// 对标 `BytesBinCompact`：边界与 [`bytes_bin_short`] 一致用 `>=`。
pub fn bytes_bin_compact(v: u64) -> String {
    const KB: u64 = 1 << 10;
    const MB: u64 = 1 << 20;
    const GB: u64 = 1 << 30;
    const TB: u64 = 1 << 40;
    if v >= TB {
        format!("{:.1}T", v as f64 / TB as f64)
    } else if v >= GB {
        format!("{:.1}G", v as f64 / GB as f64)
    } else if v >= MB {
        format!("{:.1}M", v as f64 / MB as f64)
    } else if v >= KB {
        format!("{:.1}K", v as f64 / KB as f64)
    } else {
        v.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 翻译自 bytes_test.go TestBytesSI
    #[test]
    fn test_bytes_si() {
        let cases: &[(i64, &str)] = &[
            (-100, "0 B"),
            (0, "0 B"),
            (512, "512 B"),
            (999, "999 B"),
            (1000, "1.0 kB"),
            (1500, "1.5 kB"),
            (10000, "10.0 kB"),
            (1000000, "1.0 MB"),
            (1500000, "1.5 MB"),
            (1000000000, "1.0 GB"),
            (1000000000000, "1.0 TB"),
            (1000000000000000, "1.0 PB"),
        ];
        for &(input, want) in cases {
            assert_eq!(bytes_si(input), want, "bytes_si({input})");
        }
    }

    /// 翻译自 bytes_test.go TestBytesBin
    #[test]
    fn test_bytes_bin() {
        let cases: &[(u64, &str)] = &[
            (0, "0 B"),
            (1, "1 B"),
            (1023, "1023 B"),
            (1 << 10, "1024 B"),
            ((1 << 10) + 1, "1.0 KB"),
            (1536, "1.5 KB"),
            (1 << 20, "1024.0 KB"),
            ((1 << 20) + 1, "1.0 MB"),
            (500 << 20, "500.0 MB"),
            (1 << 30, "1024.0 MB"),
            ((1 << 30) + 1, "1.0 GB"),
            (100 << 30, "100.0 GB"),
            (1 << 40, "1024.0 GB"),
            ((1 << 40) + 1, "1.0 TB"),
            (2 << 40, "2.0 TB"),
        ];
        for &(input, want) in cases {
            assert_eq!(bytes_bin(input), want, "bytes_bin({input})");
        }
    }

    /// 翻译自 bytes_test.go TestBytesBinShort
    #[test]
    fn test_bytes_bin_short() {
        let cases: &[(u64, &str)] = &[
            (0, "0"),
            (1, "1"),
            (999, "999"),
            (1 << 10, "1K"),
            ((1 << 10) - 1, "1023"),
            (1536, "2K"),
            (999 << 10, "999K"),
            (1 << 20, "1M"),
            ((1 << 20) - 1, "1024K"),
            (500 << 20, "500M"),
            (1 << 30, "1G"),
            ((1 << 30) - 1, "1024M"),
            (100 << 30, "100G"),
            (1 << 40, "1T"),
            ((1 << 40) - 1, "1024G"),
            (2 << 40, "2T"),
        ];
        for &(input, want) in cases {
            assert_eq!(bytes_bin_short(input), want, "bytes_bin_short({input})");
        }
    }

    /// 翻译自 bytes_test.go TestBytesBinCompact
    #[test]
    fn test_bytes_bin_compact() {
        let cases: &[(u64, &str)] = &[
            (0, "0"),
            (1, "1"),
            (1023, "1023"),
            (1 << 10, "1.0K"),
            (1536, "1.5K"),
            (1 << 20, "1.0M"),
            (500 << 20, "500.0M"),
            (1 << 30, "1.0G"),
            (100 << 30, "100.0G"),
            (1 << 40, "1.0T"),
            (2 << 40, "2.0T"),
        ];
        for &(input, want) in cases {
            assert_eq!(bytes_bin_compact(input), want, "bytes_bin_compact({input})");
        }
    }
}
