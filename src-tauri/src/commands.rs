//! Tauri 命令层：薄封装，业务逻辑在 `status` / `core` 模块。

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
