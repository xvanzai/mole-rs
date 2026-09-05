//! Tauri 命令层：薄封装，业务逻辑在 `status` / `core` 模块。

pub mod history {
    /// 操作历史（对标 mo history --json）。
    #[tauri::command]
    pub fn history_list(limit: Option<usize>) -> crate::history::HistoryData {
        crate::history::load_history(limit.unwrap_or(20))
    }
}

pub mod uninstall {
    /// 应用清单（只读，对标 mo uninstall 列表阶段）。
    #[tauri::command]
    pub fn uninstall_list_apps() -> Vec<crate::uninstall::AppInfo> {
        crate::uninstall::list_apps()
    }
}

pub mod app {
    /// 用户主目录（供前端默认扫描路径）。
    #[tauri::command]
    pub fn get_home_dir() -> String {
        std::env::var("HOME").unwrap_or_default()
    }
}

pub mod analyze {
    /// 扫描一个目录（对标 mo analyze 的单层浏览 + 按需下钻）。
    #[tauri::command]
    pub fn analyze_scan(path: String) -> Result<crate::analyze::ScanResult, String> {
        crate::analyze::scan_path(&path)
    }

    /// 从当前浏览层删除选中条目（仅直接子项，走 Trash 安全删除）。
    #[tauri::command]
    pub fn analyze_delete(root: String, selected: Vec<String>, dry_run: bool) -> crate::clean::CleanExecuteResult {
        crate::analyze::delete_entries(&root, &selected, dry_run)
    }
}

pub mod purge {
    use crate::purge::{PurgeScanResult};

    /// 项目产物只读扫描（对标 `MOLE_TEST_NO_AUTH=1 ./mole purge --dry-run`）。
    #[tauri::command]
    pub fn purge_scan() -> PurgeScanResult {
        crate::purge::scan()
    }

    /// 执行 purge：只接受本次扫描中出现的路径，sink 复检后走 Trash。
    #[tauri::command]
    pub fn purge_execute(selected_paths: Vec<String>, dry_run: bool) -> crate::clean::CleanExecuteResult {
        crate::purge::execute(&selected_paths, dry_run)
    }
}

pub mod clean {
    use crate::clean::{CleanExecuteResult, CleanPreview};

    /// 只读清理预览（对标 `MOLE_DRY_RUN=1 ./mole clean`）。
    #[tauri::command]
    pub fn clean_preview() -> CleanPreview {
        crate::clean::scan_preview()
    }

    /// 执行清理：按用户选择的组（对标 safe_clean 的组描述）删除到回收站。
    /// `dry_run=true` 时只产出结果不移动文件。
    /// 后端会重新扫描并在删除 sink 复检保护/白名单，不信任前端传来的路径。
    #[tauri::command]
    pub fn clean_execute(selected_groups: Vec<String>, dry_run: bool) -> CleanExecuteResult {
        crate::clean::execute_clean(&selected_groups, dry_run)
    }
}

pub mod status {
    use crate::status::{Collector, MetricsSnapshot};
    use std::sync::Mutex;

    /// Collector 的共享状态（对标 Go 的单例 Collector，保证速率类
    /// 指标跨采样准确）。
    pub struct CollectorState(pub Mutex<Collector>);

    impl Default for CollectorState {
        fn default() -> Self {
            Self(Mutex::new(Collector::new()))
        }
    }

    /// 对标 watch 模式：按 fast/process/full 节奏返回快照。
    /// 前端每秒调用一次即可，节奏由后端状态机维持（对标 watchState）。
    #[tauri::command]
    pub fn status_tick(state: tauri::State<CollectorState>) -> MetricsSnapshot {
        let mut collector = state
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        collector.tick()
    }
}
