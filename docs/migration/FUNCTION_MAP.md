# mole-rs 功能对照表（原 Mole → Rust 迁移）

> 用途：逐模块比对原项目与迁移项目的功能覆盖，记录 1:1 迁移项与有意差异。
> 原则：默认 1:1；GUI 架构差异 / 平台限制 / 安全契约导致的差异必须记录原因。

## 1. 模块总览

| 模块 | 原实现 | Rust 落点 | 覆盖率 | 备注 |
|------|--------|----------|--------|------|
| core 工具层 | internal/units/bytes.go | core/units.rs | ✅ 100% | SI/二进制双进制 |
| status 系统监控 | cmd/status/*.go | status/* | ✅ ~95% | 并发采集简化为顺序 |
| clean 深度清理 | bin/clean.sh + lib/clean/* | clean/* | ✅ ~90% | 见 §2 |
| purge 项目清理 | bin/purge.sh + lib/clean/project.sh | purge/* | ✅ 100% | 扫描+dry-run+Trash |
| analyze 磁盘分析 | cmd/analyze/*.go | analyze/* | ✅ ~90% | live_scan 事件流未移植 |
| uninstall 应用卸载 | bin/uninstall.sh + lib/uninstall/* | uninstall/* | ✅ ~85% | 见 §3 |
| optimize 优化维护 | bin/optimize.sh + lib/optimize/* | optimize/* | ✅ 100% | 21/21 处理器 |
| history / manage | bin/history.sh + lib/manage/* | history.rs + manage.rs | ✅ ~70% | 见 §4 |
| installer / touchid | bin/installer.sh + bin/touchid.sh | — | ⚠️ 架构差异 | CLI 安装流；Tauri 用 .app 分发 |

## 2. clean 模块逐函数对照

### 2.1 已 1:1 迁移

| 原函数 | Rust 落点 | 说明 |
|--------|----------|------|
| safe_clean / _safe_clean_impl | clean/mod.rs execute_clean | 保护→白名单→Trash 路由 |
| clean_app_caches（静态行） | catalog app_cache_catalog | 30+ 应用族 |
| clean_browsers | catalog browser + guarded_browser_entries | 静态+进程守卫 |
| clean_cloud_storage / office | catalog cloud_office | Dropbox/GDrive/OneDrive 守卫 |
| clean_virtualization_tools | catalog virtualization + UTM 守卫 | VMware/Parallels/UTM/Tart |
| clean_apple_silicon_caches | catalog apple_silicon | Rosetta/media |
| clean_application_support_logs | mod app_support_regenerable_entries | 可再生缓存子树 |
| clean_user_essentials（日志/Recent/Mail） | catalog user_essentials + dynamic | Mail 30 天龄过滤 |
| clean_incomplete_downloads | mod incomplete_download_entries | lsof 开句柄三态 |
| clean_service_worker_cache | mod service_worker_entries | 18 保护域名 |
| clean_chromium_old_versions | old_versions.rs | Chrome/Edge/Brave + EdgeUpdater |
| clean_group_container_caches | mod Group Containers | contentdelivery |
| clean_homebrew | brew.rs | cleanup+autoremove 预览 |
| clean_finder_metadata | mod scan_ds_store_tree | maxdepth 5+排除表 |
| clean_trash | mod Trash execute | 直接清空 |
| clean_orphaned_container_stubs | special.rs | CleanMyMac glob |
| clean_cached_device_firmware | special.rs | *.ipsw |
| clean_time_machine_failed_backups | special.rs | 计数报告（只读） |
| check_large_file_candidates | special.rs | 13 路径审查（只读） |
| clean_external_volume_target | special.rs | /Volumes/* 自动发现 |
| show_user_launch_agent_hint_notice | special.rs | max 3 提示 |
| clean_tool_cache（owner 命令） | owner_clean.rs | npm/uv/corepack/pip/bun/pnpm/Tart |
| clean_dev_database / api_tools / jetbrains_logs / composer | catalog dev_database_api | 静态行 |
| clean_dev_jvm（Gradle 守卫） | mod gradle_guarded_entries | daemon 三态 |
| clean_xcode_documentation_cache | mod xcode_documentation_stale | keep-newest |
| clean_deep_system | system.rs | 四族+Metal GPU+macOS 安装器 |

### 2.2 有意简化 / 暂缓

| 原函数 | 状态 | 原因 |
|--------|------|------|
| clean_orphaned_app_data | 暂缓 | 需 scan_installed_apps + 身份快照（~700 行）；孤儿检测语义复杂 |
| clean_orphaned_system_services | 暂缓 | sudo 读取 LaunchDaemons + 大表保护模式（Sogou/ClashX/Docker 等） |
| brew 活跃链接恢复 | 简化 | 依赖 sudo -u 注入；GUI 无 root 调用场景 |
| clean_project_caches | 归 purge | 与 purge 共享 project.sh |
| hints project artifact | 归 purge | 与 purge 集成 |

## 3. uninstall 模块逐函数对照

### 3.1 已 1:1 迁移

| 原函数 | Rust 落点 |
|--------|----------|
| uninstall_list_apps | uninstall/mod.rs list_apps |
| should_protect_from_uninstall | uninstall/mod.rs |
| uninstall_app（本体+残留） | uninstall/mod.rs uninstall_app |
| name_variants / name_patterns | uninstall/mod.rs |
| bundle_id_residue_paths | uninstall/mod.rs |
| find_vendor_nested | uninstall/mod.rs |
| bundle leaf 推导 | uninstall/mod.rs bundle_leaf_variants |
| 兄弟守卫（normalize/strip/surviving） | uninstall/mod.rs |
| get_brew_cask_name（四阶段） | uninstall/brew.rs |
| brew_uninstall_cask（zap/nozap） | uninstall/brew.rs |
| uninstall_steam_launcher_appid | uninstall/steam.rs |
| find_app_system_files（LaunchAgents/Helpers/Receipts） | uninstall/mod.rs system_files_scan |

### 3.2 有意简化 / 暂缓

| 原函数 | 状态 | 原因 |
|--------|------|------|
| batch_uninstall_applications（完整批处理） | 简化 | GUI 逐个卸载；无交互预览/确认流程 |
| 指纹/身份绑定（_mole_snapshot_path_identity） | 简化 | sink 已复检存在性/保护 |
| Raycast 特例 | 暂缓 | 厂商专属扫描 |
| LaunchAgents 名称变体（非 bundle_id） | 部分 | 兄弟存在时抑制；无兄弟时仅 bundle_id 边界 |

## 4. manage / installer / touchid

| 原功能 | Rust 状态 | 原因 |
|--------|----------|------|
| manage/whitelist.sh | ✅ manage.rs | 白名单读写 |
| manage/purge_paths.sh | ✅ manage.rs | purge_paths 读写 |
| manage/update.sh（自更新） | ⚠️ 架构差异 | CLI 自更新 shell 脚本；Tauri 用 .app 分发 + 系统更新 |
| manage/remove.sh（自移除） | ⚠️ 架构差异 | CLI 自移除；Tauri 用系统卸载 |
| bin/installer.sh | ⚠️ 架构差异 | CLI 安装流；Tauri 产物是 .app |
| bin/touchid.sh | ⚠️ 架构差异 | CLI Touch ID sudo 配置；GUI 无 TTY |

## 5. status 模块

| 原功能 | Rust 状态 |
|--------|----------|
| CPU/内存/磁盘/网络/进程/电池/热能/传感器 | ✅ |
| GPU（system_profiler + powermetrics） | ✅ |
| 蓝牙（system_profiler + bluetoothctl） | ✅ |
| 磁盘 IO（ioreg 差分） | ✅ |
| APFS purgeable（Finder/diskutil 三级） | ✅ |
| ProcessWatch 告警 | ✅ |
| 并发采集（goroutine） | ⚠️ 顺序；GUI 命令已 spawn_blocking |

## 6. analyze 模块

| 原功能 | Rust 状态 |
|--------|----------|
| scanPathConcurrent（并行扫描） | ✅ |
| large_files 堆 + Spotlight | ✅ |
| cache.go（TTL/schema/预算） | ✅ |
| insights.go | ✅ |
| snapshots.go（tmutil 计数） | ✅ |
| live_scan.go（TUI 事件流） | ⚠️ GUI 用整段结果；无渐进事件流 |
| delete.go | ✅ |

## 7. 验证

- cargo test：205+ 通过
- npm run build：成功
- 真机冒烟：clean 350+ 组、optimize 21/21、uninstall 可用

## 8. 结论

**主体功能已 1:1 迁移。** 剩余项分三类：
1. **CLI 安装/更新流**（installer/touchid/update/remove）：Tauri .app 分发架构差异，非功能缺失；
2. **复杂孤儿检测**（orphaned_app_data/system_services）：需完整应用清单+身份绑定，独立子片；
3. **TUI 专用形态**（live_scan 事件流、purge 进度文件）：GUI 已用等价交互覆盖。
