//! analyze 磁盘扫描缓存，对标 `cmd/analyze/cache.go` 的核心子集。
//!
//! 对标语义：
//! - schema 版本拒绝陈旧条目（v3：硬链接去重 + 普通 Parallels 存储计入）；
//! - TTL 7 天（`analyzerCacheTTL` / `overviewCacheTTL`）；
//! - 条目预算：超 cap 后按 mtime 淘汰至 low-water mark（对标
//!   `overviewCacheMaxEntries=1000` / `keep=900`）；
//! - 相同大小且未临近过期时跳过写入（对标 overviewRefreshDivisor）。
//!
//! 与 Go 的差异：Go 分两层（overview sizes + 子树 gob 缓存）；GUI 按需
//! 下钻成本低，本片只做**路径 → ScanResult** 单层 JSON 缓存，覆盖重复
//! 进入同一目录的主要加速场景。Spotlight 预热与快照对比仍暂缓。

use super::ScanResult;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// 对标 cacheSchemaVersion=3。
const SCHEMA_VERSION: u32 = 3;
/// 对标 analyzerCacheTTL / overviewCacheTTL = 7 天。
const CACHE_TTL: Duration = Duration::from_secs(7 * 24 * 60 * 60);
/// 对标 overviewCacheMaxEntries。
const MAX_ENTRIES: usize = 1000;
/// 对标 overviewCacheKeepEntries。
const KEEP_ENTRIES: usize = 900;
/// 对标 overviewRefreshDivisor：相同结果在 TTL/8 内不重写。
const REFRESH_DIVISOR: u64 = 8;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CacheEntry {
    result: ScanResult,
    updated_unix: u64,
    schema_version: u32,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct CacheStore {
    entries: HashMap<String, CacheEntry>,
}

fn store() -> &'static Mutex<Option<CacheStore>> {
    static STORE: OnceLock<Mutex<Option<CacheStore>>> = OnceLock::new();
    STORE.get_or_init(|| Mutex::new(None))
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// 对标 getCacheDir：`~/.cache/mole/analyze`。
fn cache_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    PathBuf::from(home).join(".cache/mole/analyze")
}

fn store_path() -> PathBuf {
    cache_dir().join("scan_cache.json")
}

fn ensure_loaded() {
    let mut guard = match store().lock() {
        Ok(g) => g,
        Err(_) => return,
    };
    if guard.is_some() {
        return;
    }
    let mut cache = CacheStore::default();
    if let Ok(data) = std::fs::read(store_path()) {
        if let Ok(loaded) = serde_json::from_slice::<CacheStore>(&data) {
            let now = now_unix();
            cache.entries = loaded
                .entries
                .into_iter()
                .filter(|(_, e)| {
                    e.schema_version == SCHEMA_VERSION
                        && e.result.total_size > 0
                        && now.saturating_sub(e.updated_unix) < CACHE_TTL.as_secs()
                })
                .collect();
        }
    }
    *guard = Some(cache);
}

fn evict_locked(cache: &mut CacheStore) {
    if cache.entries.len() <= MAX_ENTRIES || KEEP_ENTRIES >= cache.entries.len() {
        return;
    }
    let mut aged: Vec<(String, u64)> = cache
        .entries
        .iter()
        .map(|(k, e)| (k.clone(), e.updated_unix))
        .collect();
    aged.sort_by_key(|(_, t)| *t);
    let drop_count = cache.entries.len() - KEEP_ENTRIES;
    for (key, _) in aged.into_iter().take(drop_count) {
        cache.entries.remove(&key);
    }
}

fn persist_locked(cache: &CacheStore) {
    let dir = cache_dir();
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let Ok(data) = serde_json::to_vec(cache) else {
        return;
    };
    // 唯一临时文件 + rename（对标 CreateTemp + Rename，避免并发半写）。
    let tmp = dir.join(format!("scan_cache.{}.tmp", std::process::id()));
    if std::fs::write(&tmp, &data).is_err() {
        return;
    }
    let _ = std::fs::rename(&tmp, store_path());
}

/// 读缓存；过期/schema 不匹配返回 None。
pub fn get(path: &str) -> Option<ScanResult> {
    ensure_loaded();
    let guard = store().lock().ok()?;
    let cache = guard.as_ref()?;
    let entry = cache.entries.get(path)?;
    if entry.schema_version != SCHEMA_VERSION {
        return None;
    }
    let now = now_unix();
    if now.saturating_sub(entry.updated_unix) >= CACHE_TTL.as_secs() {
        return None;
    }
    Some(entry.result.clone())
}

/// 写缓存；相同大小且未临近过期时跳过（对标 overviewRefreshDivisor）。
pub fn put(path: &str, result: &ScanResult) {
    if path.is_empty() || result.total_size == 0 {
        return;
    }
    ensure_loaded();
    let mut guard = match store().lock() {
        Ok(g) => g,
        Err(_) => return,
    };
    let Some(cache) = guard.as_mut() else {
        return;
    };
    let now = now_unix();
    if let Some(existing) = cache.entries.get(path) {
        if existing.result.total_size == result.total_size
            && now.saturating_sub(existing.updated_unix) < CACHE_TTL.as_secs() / REFRESH_DIVISOR
        {
            return;
        }
    }
    cache.entries.insert(
        path.to_string(),
        CacheEntry {
            result: result.clone(),
            updated_unix: now,
            schema_version: SCHEMA_VERSION,
        },
    );
    evict_locked(cache);
    persist_locked(cache);
}

/// 清空缓存（设置页/测试用）。
#[allow(dead_code)]
pub fn clear() {
    let mut guard = match store().lock() {
        Ok(g) => g,
        Err(_) => return,
    };
    *guard = Some(CacheStore::default());
    let _ = std::fs::remove_file(store_path());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyze::{DirEntry, FileEntry, ScanResult};

    fn sample_result(total: u64) -> ScanResult {
        ScanResult {
            path: "/tmp".into(),
            entries: vec![DirEntry {
                name: "a".into(),
                path: "/tmp/a".into(),
                size: total,
                is_dir: true,
                last_access: 0,
            }],
            large_files: Vec::<FileEntry>::new(),
            total_size: total,
            total_files: 1,
            total_dirs: 1,
            truncated: false,
        }
    }

    /// 写后可读；schema/TTL 由 put/get 封装保证。
    #[test]
    fn put_get_roundtrip() {
        clear();
        let key = format!("/tmp/mole_cache_test_{}", std::process::id());
        put(&key, &sample_result(4096));
        let got = get(&key).expect("cache hit");
        assert_eq!(got.total_size, 4096);
        // 相同大小临近过期 → put 跳过但仍可读。
        put(&key, &sample_result(4096));
        assert!(get(&key).is_some());
        clear();
    }

    /// 空/零大小不入缓存。
    #[test]
    fn rejects_empty() {
        clear();
        put("", &sample_result(4096));
        put("/tmp/x", &sample_result(0));
        assert!(get("").is_none());
        assert!(get("/tmp/x").is_none());
        clear();
    }

    /// 淘汰：超 MAX_ENTRIES 后保留 KEEP_ENTRIES 条最新。
    #[test]
    fn eviction_keeps_recent() {
        clear();
        for i in 0..(MAX_ENTRIES + 10) {
            let key = format!("/dir-{i:04}");
            put(&key, &sample_result(1024));
        }
        let guard = store().lock().unwrap();
        let cache = guard.as_ref().unwrap();
        // put 会同步淘汰；条目数 ≤ MAX，且最新条目仍在。
        assert!(cache.entries.len() <= MAX_ENTRIES);
        assert!(cache.entries.contains_key(&format!("/dir-{:04}", MAX_ENTRIES + 9)));
        drop(guard);
        clear();
    }

    /// 持久化：put 后磁盘上有 JSON 文件。
    #[test]
    fn persists_to_disk() {
        clear();
        let key = format!("/tmp/mole_persist_{}", std::process::id());
        put(&key, &sample_result(8192));
        let path = store_path();
        assert!(path.is_file(), "store should exist at {:?}", path);
        let data = std::fs::read_to_string(&path).unwrap();
        assert!(data.contains(&key));
        clear();
    }
}
