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
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, UNIX_EPOCH};

/// 扫描总预算（对标 duTimeout = 30s）。
const SCAN_DEADLINE: Duration = Duration::from_secs(30);
/// 大文件保留数（对标 maxLargeFiles）。
const MAX_LARGE_FILES: usize = 20;

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

/// 扫描内共享的容量记账状态（对标 countableFileSize 的 seen map 与计数器）。
struct ScanState {
    seen_inodes: HashSet<(u64, u64)>,
    large: std::collections::BinaryHeap<std::cmp::Reverse<(u64, PathBuf, String)>>,
    deadline: Instant,
    truncated: bool,
}

impl ScanState {
    /// 记录文件大小：硬链接去重 + Top-20 大文件堆。
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

/// 递归计算目录大小（对标 calculateDirSizeConcurrent 的容量语义）。
fn dir_size(dir: &Path, state: &mut ScanState, total_files: &mut u64, total_dirs: &mut u64) -> u64 {
    if Instant::now() >= state.deadline {
        state.truncated = true;
        return 0;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0; // 权限等错误：跳过（对标扫描对错误的容忍）
    };
    let mut total = 0u64;
    for entry in entries.flatten() {
        if Instant::now() >= state.deadline {
            state.truncated = true;
            return total;
        }
        let Ok(ft) = entry.file_type() else { continue };
        if ft.is_symlink() {
            continue; // 符号链接不计大小
        }
        let path = entry.path();
        if ft.is_dir() {
            *total_dirs += 1;
            total += dir_size(&path, state, total_files, total_dirs);
        } else if let Ok(meta) = entry.metadata() {
            *total_files += 1;
            total += state.count_file(&path, &entry.file_name().to_string_lossy(), &meta);
        }
    }
    total
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
    let state_deadline = Instant::now() + SCAN_DEADLINE;
    let mut state = ScanState {
        seen_inodes: HashSet::new(),
        large: std::collections::BinaryHeap::new(),
        deadline: state_deadline,
        truncated: false,
    };

    let mut entries: Vec<DirEntry> = Vec::new();
    let mut total_size = 0u64;
    let mut total_files = 0u64;
    let mut total_dirs = 0u64;

    let Ok(children) = std::fs::read_dir(&root_path) else {
        return Err("无法读取目录".into());
    };
    for child in children.flatten() {
        if Instant::now() >= state.deadline {
            state.truncated = true;
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

        let (is_dir, size) = if ft.is_symlink() {
            (false, 0)
        } else if ft.is_dir() {
            total_dirs += 1;
            (true, dir_size(&path, &mut state, &mut total_files, &mut total_dirs))
        } else {
            match child.metadata() {
                Ok(meta) => {
                    total_files += 1;
                    (false, state.count_file(&path, &name, &meta))
                }
                Err(_) => (false, 0),
            }
        };

        total_size += size;
        entries.push(DirEntry {
            name,
            path: path.to_string_lossy().to_string(),
            size,
            is_dir,
            last_access,
        });
    }

    // 大文件堆 → 时间序倒序列出（大→小，对标堆导出顺序）。
    let mut large_files: Vec<FileEntry> = state
        .large
        .into_iter()
        .map(|std::cmp::Reverse((size, path, name))| FileEntry {
            name,
            path: path.to_string_lossy().to_string(),
            size,
        })
        .collect();
    large_files.sort_by(|a, b| b.size.cmp(&a.size));

    // 条目按大小降序（对标 TUI 的按大小排序展示）。
    entries.sort_by(|a, b| b.size.cmp(&a.size));

    Ok(ScanResult {
        path: root_path.to_string_lossy().to_string(),
        entries,
        large_files,
        total_size,
        total_files,
        total_dirs,
        truncated: state.truncated,
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
