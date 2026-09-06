//! Tauri 命令层：薄封装，业务逻辑在 `status` / `clean` 等业务模块。
//!
//! 重要约束（修复主线程冻结）：Tauri 2 的**同步命令**经 WKScriptMessage
//! 在主线程执行，任何超过几十毫秒的工作都会卡死整个窗口（事件循环停转，
//! 无法渲染/点击）。因此所有涉及扫描、子进程、文件系统遍历的命令一律
//! 声明为 `async` 并用 `spawn_blocking` 交给阻塞线程池（对标 Go 版在
//! 独立 goroutine 中采集、不阻塞 TUI 的模型）。轻量命令（env/格式化）
//! 保持同步，无冻结风险。

use tauri::async_runtime::spawn_blocking;

/// spawn_blocking 的 JoinError → String（命令层统一错误通道）。
async fn blocking<T, F>(f: F) -> Result<T, String>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    spawn_blocking(f)
        .await
        .map_err(|e| format!("后台任务失败: {e}"))
}

/// 同上，用于闭包本身已返回 `Result<T, String>` 的场景（拍平双层）。
async fn blocking_result<T, F>(f: F) -> Result<T, String>
where
    F: FnOnce() -> Result<T, String> + Send + 'static,
    T: Send + 'static,
{
    match spawn_blocking(f).await {
        Ok(inner) => inner,
        Err(e) => Err(format!("后台任务失败: {e}")),
    }
}

pub mod optimize {
    /// 任务目录（对标 catalog.sh 21 项）。
    #[tauri::command]
    pub async fn optimize_tasks() -> Result<Vec<crate::optimize::OptimizeTask>, String> {
        super::blocking(crate::optimize::task_catalog).await
    }

    /// 执行选中的优化任务。
    #[tauri::command]
    pub async fn optimize_execute(
        selected: Vec<String>,
        dry_run: bool,
    ) -> Result<crate::optimize::OptimizeResult, String> {
        super::blocking(move || crate::optimize::execute(&selected, dry_run)).await
    }
}

pub mod manage {
    use crate::manage::{PurgePathsConfig, WhitelistConfig};

    #[tauri::command]
    pub fn get_whitelist() -> WhitelistConfig { crate::manage::get_whitelist() }
    #[tauri::command]
    pub fn set_whitelist(lines: Vec<String>) -> Result<(), String> { crate::manage::set_whitelist(&lines) }
    #[tauri::command]
    pub fn get_purge_paths() -> PurgePathsConfig { crate::manage::get_purge_paths() }
    #[tauri::command]
    pub fn set_purge_paths(lines: Vec<String>) -> Result<(), String> { crate::manage::set_purge_paths(&lines) }
}

pub mod history {
    /// 操作历史（对标 mo history --json）。
    #[tauri::command]
    pub async fn history_list(limit: Option<usize>) -> Result<crate::history::HistoryData, String> {
        super::blocking(move || {
            crate::history::load_history(limit.unwrap_or(crate::history::DEFAULT_LIMIT))
        })
        .await
    }
}

pub mod uninstall {
    /// 应用清单（只读，对标 mo uninstall 列表阶段）。
    #[tauri::command]
    pub async fn uninstall_list_apps() -> Result<Vec<crate::uninstall::AppInfo>, String> {
        super::blocking(crate::uninstall::list_apps).await
    }
}

pub mod app {
    /// 用户主目录（供前端默认扫描路径）。纯 env 读取，保持同步。
    #[tauri::command]
    pub fn get_home_dir() -> String {
        std::env::var("HOME").unwrap_or_default()
    }
}

pub mod analyze {
    /// 扫描一个目录（对标 mo analyze 的单层浏览 + 按需下钻）。
    #[tauri::command]
    pub async fn analyze_scan(path: String) -> Result<crate::analyze::ScanResult, String> {
        super::blocking_result(move || crate::analyze::scan_path(&path)).await
    }

    /// 从当前浏览层删除选中条目（仅直接子项，走 Trash 安全删除）。
    #[tauri::command]
    pub async fn analyze_delete(
        root: String,
        selected: Vec<String>,
        dry_run: bool,
    ) -> Result<crate::clean::CleanExecuteResult, String> {
        super::blocking(move || crate::analyze::delete_entries(&root, &selected, dry_run)).await
    }
}

pub mod purge {
    use crate::purge::PurgeScanResult;

    /// 项目产物只读扫描（对标 `MOLE_TEST_NO_AUTH=1 ./mole purge --dry-run`）。
    #[tauri::command]
    pub async fn purge_scan() -> Result<PurgeScanResult, String> {
        super::blocking(crate::purge::scan).await
    }

    /// 执行 purge：只接受本次扫描中出现的路径，sink 复检后走 Trash。
    #[tauri::command]
    pub async fn purge_execute(
        selected_paths: Vec<String>,
        dry_run: bool,
    ) -> Result<crate::clean::CleanExecuteResult, String> {
        super::blocking(move || crate::purge::execute(&selected_paths, dry_run)).await
    }
}

pub mod clean {
    use crate::clean::{CleanExecuteResult, CleanPreview};

    /// 只读清理预览（对标 `MOLE_DRY_RUN=1 ./mole clean`）。
    #[tauri::command]
    pub async fn clean_preview() -> Result<CleanPreview, String> {
        super::blocking(crate::clean::scan_preview).await
    }

    /// 执行清理：按用户选择的组（对标 safe_clean 的组描述）删除到回收站。
    /// `dry_run=true` 时只产出结果不移动文件。
    /// 后端会重新扫描并在删除 sink 复检保护/白名单，不信任前端传来的路径。
    #[tauri::command]
    pub async fn clean_execute(
        selected_groups: Vec<String>,
        dry_run: bool,
    ) -> Result<CleanExecuteResult, String> {
        super::blocking(move || crate::clean::execute_clean(&selected_groups, dry_run)).await
    }
}

pub mod status {
    use crate::status::{Collector, MetricsSnapshot};
    use std::sync::{Arc, Mutex};
    use tauri::async_runtime::spawn_blocking;

    /// Collector 的共享状态（对标 Go 的单例 Collector，保证速率类
    /// 指标跨采样准确）。Arc 以便把锁移入阻塞线程池。
    pub struct CollectorState(pub Arc<Mutex<Collector>>);

    impl Default for CollectorState {
        fn default() -> Self {
            Self(Arc::new(Mutex::new(Collector::new())))
        }
    }

    /// 对标 watch 模式：按 fast/process/full 节奏返回快照。
    /// 前端每秒调用一次即可，节奏由后端状态机维持（对标 watchState）。
    /// 采集含 ps 等子进程调用，必须离开主线程（async 命令约束）。
    #[tauri::command]
    pub async fn status_tick(
        state: tauri::State<'_, CollectorState>,
    ) -> Result<MetricsSnapshot, String> {
        let state = Arc::clone(&state.0);
        spawn_blocking(move || {
            let mut collector = state.lock().unwrap_or_else(|p| p.into_inner());
            collector.tick()
        })
        .await
        .map_err(|e| format!("status_tick 失败: {e}"))
    }
}
