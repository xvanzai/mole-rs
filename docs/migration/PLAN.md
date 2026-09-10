# mole-rs 迁移计划（Mole CLI → Tauri GUI）

## 1. 背景与目标

- 原项目 [Mole](../..)（`Mole/`）是 macOS 终端清理/维护工具，由 **shell（业务逻辑）+ Go（status / analyze）** 组成，对普通用户不友好。
- 新项目 `mole-rs` 使用 **Tauri 2 + Vue 3 + TypeScript（前端）+ Rust（后端）** 重写：
  - 图形化界面降低使用门槛；
  - Rust 重写业务核心，提升运行效率并保障内存安全；
  - 遵循原项目的安全契约（dry-run 预览、Trash 可恢复删除、白名单保护、操作日志）。

## 2. 强制工作流（每个模块必须遵守）

1. **对标先行**：精读原项目对应代码（含测试），在模块的移植说明中记录功能清单与行为契约，再动手写 Rust。
2. **行为对齐**：默认 1:1 移植原行为（含格式化、边界值、单位约定）。只有出现编译问题 / Rust 平台不适用 / 原实现依赖终端 TUI 时才允许变更，且必须在 `CHANGES.md` 写明 **变更前（原实现）→ 变更后（Rust 实现）→ 变更原因**。
3. **测试对齐**：原项目的测试向量（Go test / bats 用例）翻译为 `cargo test` 单元测试，逐条锁定行为。
4. **一次提交**：每个小模块完成（Rust 核心 + 测试 + UI + 编译验证通过）即提交一次 git，提交信息注明对标来源。
5. **目录约束**：所有命令仅在 `mole-rust/` 目录内执行，不读写项目外的文件。

## 3. 模块划分与顺序（循序渐进）

| # | 模块 | 原代码位置 | Rust 落点 | UI | 状态 |
|---|------|-----------|----------|-----|------|
| 1 | core 工具层（字节单位等） | `internal/units/bytes.go` | `src-tauri/src/core/units.rs` | — | ✅ 已完成 |
| 2 | status 系统监控 | `cmd/status/*.go` | `src-tauri/src/status/` + `src/pages/StatusPage.vue` | 监控仪表盘 | ✅ 已完成（GPU/蓝牙/磁盘IO/APFS修正为后续子项） |
| 3 | clean 深度清理 | `bin/clean.sh`、`lib/clean/*`、`lib/core/app_protection*.sh` | `src-tauri/src/clean/` + `src/pages/CleanPage.vue` | 清理页（预览→执行） | 🟢 3a+3b+3c(第一片) 完成（白名单、完整保护层、Trash 安全删除、双日志、执行 UI、开发工具链族 46 行）；3c 其余=需要探测/进程守卫的行 + system/browser 族 |
| 4 | purge 项目构建产物清理 | `bin/purge.sh`、`lib/clean/project.sh` | `src-tauri/src/purge/` + `src/pages/PurgePage.vue` | 项目清理页 | ✅ 扫描 + dry-run + Trash 执行 |
| 5 | analyze 磁盘分析 | `cmd/analyze/*.go` | `src-tauri/src/analyze/` + `src/pages/AnalyzePage.vue` | 磁盘浏览页 | ✅ 第一片完成（扫描器容量语义+浏览+删除）；缓存层/Spotlight预热/快照对比暂缓 |
| 6 | uninstall 应用卸载 | `bin/uninstall.sh` + `lib/uninstall/*` | `src-tauri/src/uninstall/` + `src/pages/UninstallPage.vue` | 卸载页 | 🟢 6a+6b+6c 完成（清单+保护分级+本体+精确残留删除+名称变体） |
| 7 | optimize 优化维护 | `bin/optimize.sh` + `lib/optimize/*` | `src-tauri/src/optimize/` + `src/pages/OptimizePage.vue` | 优化页 | 🟡 20/21 处理器完成；剩余：login_items_audit |
| 8 | history / manage（更新、白名单、自移除） | `bin/history.sh` + `lib/core/history.sh` + `lib/manage/*` | `src-tauri/src/history.rs` + `src/pages/HistoryPage.vue` | 历史页 / 设置页 | ✅ 8a 历史 + 8b 设置页（白名单/purge_paths 管理） |

> 顺序理由：core 是公共底座；status 只读、风险最低，先打通「Rust 命令 + 实时 UI」管线；clean 是旗舰功能；purge 与 clean 共享 project.sh 逻辑；analyze/uninstall/optimize 依次跟进；history/manage 依赖前序模块产生的数据。

## 4. 安全契约移植清单（贯穿所有模块）

来自 `Mole/AGENTS.md`（原项目安全规则），Rust 侧必须等价落实：

1. 所有删除走统一安全删除助手（对标 `mole_delete`）：Trash 路由 + 操作日志 + dry-run + 路径保护检查；禁止裸 `rm -rf` 语义。
2. 删除前必须经过路径保护检查（对标 `should_protect_path`，`/System`、`/Library/Apple`、`com.apple.*` 等永不动）。
3. 破坏性操作默认 dry-run 预览，UI 先展示「将删除什么、可释放多少」，确认后才执行。
4. 单位约定（对标 `internal/units`）：磁盘容量用 SI(1000)（与 Finder/diskutil 一致），内存/实时计数用二进制(1024)（与活动监视器一致）。
5. 不做后台常驻、不自动清理；每次操作可解释、可复核。

## 5. 验证方式

- Rust：`cargo test`（行为对齐测试）+ `cargo check`。
- 前端：`npm run build`（vue-tsc 类型检查 + vite 构建）。
- 集成：`npm run tauri dev` 人工验收。
- 每个模块在 `CHANGES.md` 追加「对标记录 + 变更前后对照」，提交 git。
