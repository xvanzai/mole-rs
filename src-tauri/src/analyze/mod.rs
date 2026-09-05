//! analyze 模块：磁盘空间分析器，对标 `Mole cmd/analyze`（Go）。
//!
//! 数据模型 1:1（model.go）：单层目录浏览 + 按需下钻（dirEntry/
//! fileEntry/scanResult）。容量语义对齐 scanner.go：
//! - 文件计 `min(blocks*512, len)`（实际占用，稀疏文件不虚报）；
//! - 同一次扫描内硬链接（nlink>1，dev+ino）只计一次（countableFileSize）；
//! - 符号链接不计大小（跳过）；
//! - Top-20 大文件（对标 maxLargeFiles 的最小堆）。
//!
//! TUI 的 entriesHeap=30 限制不移植：GUI 列表可滚动，返回全部子项由
//! 前端排序（记入 CHANGES.md）。扫描 30s 预算（对标 duTimeout），
//! 超时标记 truncated——结果仅用于展示，删除判定不依赖大小。

use serde::Serialize;
use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant, UNIX_EPOCH};

/// 扫描总预算（对标 duTimeout = 30s）。
const SCAN_DEADLINE: Duration = Duration::from_secs(30);
/// 大文件保留数（对标 maxLargeFiles）。
const MAX_LARGE_FILES: usize = 20;
/// 并行遍历线程数上限（对标 Go calculateDirSizeConcurrent 的并发设计；
/// 文件系统遍历是 IO 密集，超过 8 收益递减）。
const MAX_WALKERS: usize = 8;

/// 目录条目（对标 dirEntry；last_access 为 Unix 毫秒）。
#[derive(Debug, Clone, Serialize)]
pub struct DirEntry {
    pub name: String,
    pub path: String,
    pub size: u64,
    pub is_dir: bool,
    pub last_access: u64,
}

/// 大文件条目（对标 fileEntry）。
#[derive(Debug, Clone, Serialize)]
pub struct FileEntry {
    pub name: String,
    pub path: String,
    pub size: u64,
}

/// 扫描结果（对标 scanResult + 进度计数）。
#[derive(Debug, Clone, Serialize)]
pub struct ScanResult {
    pub path: String,
    pub entries: Vec<DirEntry>,
    pub large_files: Vec<FileEntry>,
    pub total_size: u64,
    pub total_files: u64,
    pub total_dirs: u64,
    /// 扫描预算耗尽：结果为部分值（仅展示用）。
    pub truncated: bool,
}

/// 扫描内共享的容量记账状态（对标 countableFileSize 的 seen map 与 Top 堆；
/// 多 worker 共享，经互斥锁短暂持锁更新）。
struct ScanState {
    seen_inodes: HashSet<(u64, u64)>,
    large: std::collections::BinaryHeap<std::cmp::Reverse<(u64, PathBuf, String)>>,
}

impl ScanState {
    /// 记录文件大小：硬链接去重 + Top-20 大文件堆。返回计入的大小。
    fn count_file(&mut self, path: &Path, name: &str, meta: &std::fs::Metadata) -> u64 {
        use std::os::unix::fs::MetadataExt;
        let size = meta.len().min(meta.blocks() * 512);
        if meta.nlink() > 1 {
            let key = (meta.dev(), meta.ino());
            if !self.seen_inodes.insert(key) {
                return 0; // 本扫描已计过该 inode
            }
        }
        if self.large.len() < MAX_LARGE_FILES {
            self.large
                .push(std::cmp::Reverse((size, path.to_path_buf(), name.to_string())));
        } else if let Some(std::cmp::Reverse((min_size, _, _))) = self.large.peek() {
            if size > *min_size {
                self.large.pop();
                self.large
                    .push(std::cmp::Reverse((size, path.to_path_buf(), name.to_string())));
            }
        }
        size
    }
}

/// 并行目录遍历器（对标 calculateDirSizeConcurrent 的并发语义）：
/// 共享任务队列 + 有界 worker；队列元素携带其所属顶层条目的桶号，
/// 文件大小按桶归集，从而得到每个顶层条目的递归大小。
/// 终止条件：pending（已入队+在飞目录数）归零；deadline 触发 stop 后
/// worker 立即排空退出（结果标记 truncated，仅展示用）。
struct ParallelWalker {
    queue: Mutex<VecDeque<(PathBuf, usize)>>,
    idle: Condvar,
    pending: AtomicUsize,
    stop: AtomicBool,
    state: Mutex<ScanState>,
    total_files: AtomicU64,
    total_dirs: AtomicU64,
    total_size: AtomicU64,
    buckets: Vec<AtomicU64>,
    deadline: Instant,
}

impl ParallelWalker {
    fn new(buckets: usize, deadline: Instant) -> Self {
        Self {
            queue: Mutex::new(VecDeque::new()),
            idle: Condvar::new(),
            pending: AtomicUsize::new(0),
            stop: AtomicBool::new(false),
            state: Mutex::new(ScanState {
                seen_inodes: HashSet::new(),
                large: std::collections::BinaryHeap::new(),
            }),
            total_files: AtomicU64::new(0),
            total_dirs: AtomicU64::new(0),
            total_size: AtomicU64::new(0),
            buckets: (0..buckets).map(|_| AtomicU64::new(0)).collect(),
            deadline,
        }
    }

    /// 目录入队（对标「发现即计数」：不可读目录同样计入 total_dirs）。
    fn push_dir(&self, path: PathBuf, bucket: usize) {
        self.total_dirs.fetch_add(1, Ordering::Relaxed);
        self.pending.fetch_add(1, Ordering::SeqCst);
        self.queue
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push_back((path, bucket));
        self.idle.notify_one();
    }

    /// 记录文件大小：硬链接去重 + Top-20 大文件堆。返回计入的大小。
    fn count_file(&self, path: &Path, name: &str, meta: &std::fs::Metadata, bucket: Option<usize>) -> u64 {
        let size = {
            let mut st = self.state.lock().unwrap_or_else(|p| p.into_inner());
            st.count_file(path, name, meta)
        };
        if size > 0 {
            if let Some(b) = bucket {
                self.buckets[b].fetch_add(size, Ordering::Relaxed);
            }
            self.total_size.fetch_add(size, Ordering::Relaxed);
        }
        self.total_files.fetch_add(1, Ordering::Relaxed);
        size
    }

    /// 遍历一个目录：文件计数，子目录入队（继承桶号）。
    fn walk_dir(&self, dir: &Path, bucket: usize) {
        if self.stop.load(Ordering::Relaxed) || Instant::now() >= self.deadline {
            self.stop.store(true, Ordering::Relaxed);
            return;
        }
        let Ok(entries) = std::fs::read_dir(dir) else {
            return; // 权限等错误：跳过（对标扫描对错误的容忍）
        };
        let mut subdirs: Vec<(PathBuf, usize)> = Vec::new();
        for entry in entries.flatten() {
            if Instant::now() >= self.deadline {
                self.stop.store(true, Ordering::Relaxed);
                break;
            }
            let Ok(ft) = entry.file_type() else { continue };
            if ft.is_symlink() {
                continue; // 符号链接不计大小
            }
            let path = entry.path();
            if ft.is_dir() {
                subdirs.push((path, bucket));
            } else if let Ok(meta) = entry.metadata() {
                self.count_file(
                    &path,
                    &entry.file_name().to_string_lossy(),
                    &meta,
                    Some(bucket),
                );
            }
        }
        for (d, b) in subdirs {
            if self.stop.load(Ordering::Relaxed) {
                break;
            }
            self.push_dir(d, b);
        }
    }

    /// worker 主循环：取目录 → 遍历 → pending 归零时全体退出。
    fn worker(&self) {
        let mut q = self.queue.lock().unwrap_or_else(|p| p.into_inner());
        loop {
            if self.stop.load(Ordering::Relaxed) {
                return;
            }
            if let Some((dir, bucket)) = q.pop_front() {
                drop(q);
                self.walk_dir(&dir, bucket);
                if self.pending.fetch_sub(1, Ordering::SeqCst) == 1 {
                    self.idle.notify_all();
                }
                q = self.queue.lock().unwrap_or_else(|p| p.into_inner());
            } else if self.pending.load(Ordering::SeqCst) == 0 {
                return; // 全部目录已处理完毕
            } else {
                // 队列暂时为空但仍有在飞目录：等待；超时兜底重查 stop。
                let (guard, _timeout) = self
                    .idle
                    .wait_timeout(q, Duration::from_millis(200))
                    .unwrap_or_else(|p| p.into_inner());
                q = guard;
            }
        }
    }

    fn run(&self) {
        let workers = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4)
            .min(MAX_WALKERS)
            .max(1);
        std::thread::scope(|scope| {
            for _ in 0..workers {
                scope.spawn(|| self.worker());
            }
        });
    }
}

/// 校验扫描目标（对标 analyze 接受的扫描目标形态：绝对路径、存在、目录）。
fn validate_scan_target(root: &str) -> Result<PathBuf, String> {
    if root.is_empty() {
        return Err("路径为空".into());
    }
    if !root.starts_with('/') {
        return Err("路径必须是绝对路径".into());
    }
    if root.split('/').any(|c| c == "..") {
        return Err("路径不允许 .. 组件".into());
    }
    let p = PathBuf::from(root);
    if !p.is_dir() {
        return Err("路径不存在或不是目录".into());
    }
    Ok(p)
}

/// 扫描一个目录：返回其子项（含递归大小）与 Top 大文件。
pub fn scan_path(root: &str) -> Result<ScanResult, String> {
    let root_path = validate_scan_target(root)?;
    let deadline = Instant::now() + SCAN_DEADLINE;

    // 顶层枚举：目录建桶（等待并行求和），文件直接计数。
    let Ok(children) = std::fs::read_dir(&root_path) else {
        return Err("无法读取目录".into());
    };
    enum Kind {
        Symlink,
        Dir,
        File(std::fs::Metadata),
    }
    struct TopEntry {
        name: String,
        path: PathBuf,
        last_access: u64,
        kind: Kind,
    }
    let mut tops: Vec<TopEntry> = Vec::new();
    let mut _dir_count = 0usize;
    for child in children.flatten() {
        if Instant::now() >= deadline {
            break;
        }
        let Ok(ft) = child.file_type() else { continue };
        let name = child.file_name().to_string_lossy().to_string();
        let path = child.path();
        let last_access = child
            .metadata()
            .ok()
            .and_then(|m| m.accessed().ok())
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let kind = if ft.is_symlink() {
            Kind::Symlink
        } else if ft.is_dir() {
            _dir_count += 1;
            Kind::Dir
        } else {
            match child.metadata() {
                Ok(meta) => Kind::File(meta),
                Err(_) => Kind::Symlink, // 不可读：不计大小
            }
        };
        tops.push(TopEntry {
            name,
            path,
            last_access,
            kind,
        });
    }

    let dir_count = tops
        .iter()
        .filter(|t| matches!(t.kind, Kind::Dir))
        .count();
    let walker = ParallelWalker::new(dir_count, deadline);
    let mut top_sizes: Vec<u64> = vec![0; tops.len()];
    let mut dir_bucket = 0usize;
    for (idx, t) in tops.iter().enumerate() {
        match &t.kind {
            Kind::Symlink => {}
            Kind::Dir => {
                walker.push_dir(t.path.clone(), dir_bucket);
                dir_bucket += 1;
            }
            Kind::File(meta) => {
                top_sizes[idx] = walker.count_file(&t.path, &t.name, meta, None);
            }
        }
    }

    walker.run();

    let truncated = walker.stop.load(Ordering::Relaxed);
    let total_files = walker.total_files.load(Ordering::Relaxed);
    let total_dirs = walker.total_dirs.load(Ordering::Relaxed);
    let total_size = walker.total_size.load(Ordering::Relaxed);

    // 大文件堆 → 大→小（对标堆导出顺序）。
    let mut large_files: Vec<FileEntry> = {
        let mut st = walker.state.lock().unwrap_or_else(|p| p.into_inner());
        std::mem::take(&mut st.large)
            .into_iter()
            .map(|std::cmp::Reverse((size, path, name))| FileEntry {
                name,
                path: path.to_string_lossy().to_string(),
                size,
            })
            .collect()
    };
    large_files.sort_by(|a, b| b.size.cmp(&a.size));

    let mut entries: Vec<DirEntry> = Vec::with_capacity(tops.len());
    let mut dir_bucket = 0usize;
    for (idx, t) in tops.into_iter().enumerate() {
        let (is_dir, size) = match t.kind {
            Kind::Symlink => (false, 0),
            Kind::Dir => {
                let size = walker.buckets[dir_bucket].load(Ordering::Relaxed);
                dir_bucket += 1;
                (true, size)
            }
            Kind::File(_) => (false, top_sizes[idx]),
        };
        entries.push(DirEntry {
            name: t.name,
            path: t.path.to_string_lossy().to_string(),
            size,
            is_dir,
            last_access: t.last_access,
        });
    }
    // 条目按大小降序（对标 TUI 的按大小排序展示）。
    entries.sort_by(|a, b| b.size.cmp(&a.size));

    Ok(ScanResult {
        path: root_path.to_string_lossy().to_string(),
        entries,
        large_files,
        total_size,
        total_files,
        total_dirs,
        truncated,
    })
}

/// 从当前浏览层删除选中条目：仅接受 root 的**直接子项**（对标 TUI 在当前
/// 层选择的删除语义），走统一的 Trash 安全删除 + 双日志。
pub fn delete_entries(root: &str, selected: &[String], dry_run: bool) -> crate::clean::CleanExecuteResult {
    let root_path = match validate_scan_target(root) {
        Ok(p) => p,
        Err(e) => {
            return crate::clean::CleanExecuteResult {
                outcomes: vec![crate::clean::DeleteOutcome {
                    path: root.into(),
                    status: "failed".into(),
                    size_bytes: 0,
                    detail: e,
                }],
                deleted_count: 0,
                freed_bytes: 0,
                failed_count: 1,
            };
        }
    };
    let canonical_root = root_path.canonicalize().unwrap_or(root_path);
    let mut outcomes = Vec::new();
    let mut deleted_count = 0usize;
    let mut freed_bytes = 0u64;
    let mut failed_count = 0usize;

    crate::clean::delete::log_session_start("analyze");

    for sel in selected {
        let p = PathBuf::from(sel);
        // 直接子项校验：父目录必须是当前扫描根（防注入任意路径）。
        let is_direct_child = p
            .parent()
            .and_then(|parent| parent.canonicalize().ok())
            .map(|parent| parent == canonical_root)
            .unwrap_or(false);
        if !is_direct_child {
            outcomes.push(crate::clean::DeleteOutcome {
                path: sel.clone(),
                status: "skipped".into(),
                size_bytes: 0,
                detail: "not-a-direct-child".into(),
            });
            continue;
        }
        let outcome = crate::clean::delete::delete_to_trash(sel, dry_run, "analyze");
        if outcome.status == "ok" {
            deleted_count += 1;
            freed_bytes += outcome.size_bytes;
        } else if outcome.status == "failed" {
            failed_count += 1;
        }
        outcomes.push(outcome);
    }

    crate::clean::delete::log_session_end("analyze", deleted_count, freed_bytes);

    crate::clean::CleanExecuteResult {
        outcomes,
        deleted_count,
        freed_bytes,
        failed_count,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("mole_rs_analyze_{tag}_{}", std::process::id()));
        std::fs::remove_dir_all(&p).ok();
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    /// 容量语义：递归求和、目录/文件计数、排序、条目路径。
    #[test]
    fn scan_lists_entries_with_recursive_sizes() {
        let root = tmp("scan");
        let big = root.join("bigdir");
        std::fs::create_dir_all(&big).unwrap();
        std::fs::write(big.join("a.bin"), vec![0u8; 4096]).unwrap();
        std::fs::create_dir_all(big.join("sub")).unwrap();
        std::fs::write(big.join("sub/b.bin"), vec![0u8; 2048]).unwrap();
        std::fs::write(root.join("c.txt"), vec![0u8; 100]).unwrap();

        let result = scan_path(&root.to_string_lossy()).unwrap();
        assert_eq!(result.entries.len(), 2); // bigdir + c.txt
        assert_eq!(result.entries[0].name, "bigdir"); // 按大小降序
        assert_eq!(result.entries[0].size, 6144);
        assert!(result.entries[0].is_dir);
        assert_eq!(result.total_size, 6244);
        assert_eq!(result.total_dirs, 2); // bigdir + sub
        assert_eq!(result.total_files, 3);
        assert!(!result.truncated);

        // 大文件 Top-N 含深层文件。
        let names: Vec<&str> = result.large_files.iter().map(|f| f.name.as_str()).collect();
        assert!(names.contains(&"a.bin"));
        assert!(names.contains(&"b.bin"));
        std::fs::remove_dir_all(&root).ok();
    }

    /// 大文件堆保留 Top-20。
    #[test]
    fn large_files_capped_at_20() {
        let root = tmp("heap");
        for i in 0..25 {
            std::fs::write(root.join(format!("f{i:02}.bin")), vec![0u8; 1000 + i]).unwrap();
        }
        let result = scan_path(&root.to_string_lossy()).unwrap();
        assert_eq!(result.large_files.len(), 20);
        // 最大的是 f24（1024+24）。
        assert_eq!(result.large_files[0].name, "f24.bin");
        std::fs::remove_dir_all(&root).ok();
    }

    /// 符号链接不计大小。
    #[test]
    fn symlinks_not_counted() {
        let root = tmp("symlink");
        let target = root.join("real.bin");
        std::fs::write(&target, vec![0u8; 512]).unwrap();
        std::os::unix::fs::symlink(&target, root.join("link.bin")).unwrap();
        let result = scan_path(&root.to_string_lossy()).unwrap();
        assert_eq!(result.total_size, 512);
        let link = result.entries.iter().find(|e| e.name == "link.bin").unwrap();
        assert_eq!(link.size, 0);
        std::fs::remove_dir_all(&root).ok();
    }

    /// 并行扫描下硬链接（nlink>1，同 dev+ino）仍只计一次。
    #[test]
    fn hardlinks_counted_once() {
        let root = tmp("hardlink");
        let dir = root.join("d");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.bin"), vec![0u8; 4096]).unwrap();
        std::fs::hard_link(dir.join("a.bin"), root.join("b.bin")).unwrap();
        let result = scan_path(&root.to_string_lossy()).unwrap();
        // 4096 只计一次：b.bin（顶层）与 d 内 a.bin 同 inode。
        // 哪个条目承载大小取决于遍历顺序（read_dir 顺序本就不保证，
        // 与原串行实现一致），确定性不变量是总量与"恰有一条非零"。
        assert_eq!(result.total_size, 4096);
        let b = result.entries.iter().find(|e| e.name == "b.bin").unwrap();
        let d = result.entries.iter().find(|e| e.name == "d").unwrap();
        assert_eq!(b.size + d.size, 4096);
        assert!(b.size == 4096 || d.size == 4096);
        std::fs::remove_dir_all(&root).ok();
    }

    /// 删除命令：仅直接子项放行。
    #[test]
    fn delete_rejects_non_direct_children() {
        let root = tmp("del");
        let sub = root.join("sub");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join("x.bin"), "x").unwrap();

        // 非 root 直接子项 → skipped。
        let r = delete_entries(
            &root.to_string_lossy(),
            &[sub.join("x.bin").to_string_lossy().to_string()],
            true,
        );
        assert_eq!(r.outcomes[0].status, "skipped");

        // root 直接子项 → dry-run。
        let r = delete_entries(&root.to_string_lossy(), &[sub.to_string_lossy().to_string()], true);
        assert_eq!(r.outcomes[0].status, "dry-run");
        assert!(sub.exists());

        // 注入路径（root 外）→ skipped。
        let r = delete_entries(
            &root.to_string_lossy(),
            &["/System/Library/Caches".to_string()],
            true,
        );
        assert_eq!(r.outcomes[0].status, "skipped");
        std::fs::remove_dir_all(&root).ok();
    }

    /// 扫描目标校验。
    #[test]
    fn scan_target_validation() {
        assert!(scan_path("").is_err());
        assert!(scan_path("relative").is_err());
        assert!(scan_path("/a/../b").is_err());
        assert!(scan_path("/nonexistent-path-xyz").is_err());
    }
}

/// 真机冒烟（默认忽略）：扫描 mole-rs 项目目录验证全链路。
#[cfg(test)]
mod smoke_tests {
    use super::*;

    #[test]
    #[ignore]
    fn analyze_scan_smoke() {
        let root = env!("CARGO_MANIFEST_DIR");
        let result = scan_path(root).expect("scan should succeed");
        println!(
            "path={} total={:.2} MB files={} dirs={} entries={} truncated={}",
            result.path,
            result.total_size as f64 / 1048576.0,
            result.total_files,
            result.total_dirs,
            result.entries.len(),
            result.truncated
        );
        for e in result.entries.iter().take(5) {
            println!("  {} {} {:.2} MB", if e.is_dir { "📁" } else { "📄" }, e.name, e.size as f64 / 1048576.0);
        }
        println!("large files: {}", result.large_files.len());
        for f in result.large_files.iter().take(3) {
            println!("  {} ({:.2} MB)", f.path, f.size as f64 / 1048576.0);
        }
        assert!(!result.entries.is_empty());
        assert!(result.total_size > 0);
    }
}
