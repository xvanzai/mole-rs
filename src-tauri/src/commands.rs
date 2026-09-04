//! Tauri 命令层：薄封装，业务逻辑在 `status` / `core` 模块。

pub mod clean {
    use crate::clean::CleanPreview;

    /// 只读清理预览（对标 `MOLE_DRY_RUN=1 ./mole clean`）。
    /// 删除执行在 3b 子模块（完整保护层 + Trash 路由）落地后开放。
    #[tauri::command]
    pub fn clean_preview() -> CleanPreview {
        crate::clean::scan_preview()
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
