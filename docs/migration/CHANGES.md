# 变更文档（对标 → 移植 → 差异记录）

> 用途：按模块记录「原实现（变更前）→ Rust 实现（变更后）→ 变更原因」。
> 原则：默认 1:1 移植原行为；只有编译问题 / 平台差异 / 终端 TUI 特有逻辑才允许变更，且必须留痕。

## 模块索引

| 模块 | 原代码 | Rust 代码 | 变更记录 |
|------|--------|----------|---------|
| core 工具层 | `internal/units/bytes.go` | `src-tauri/src/core/units.rs` | [§core 工具层](#core-工具层) |
| status 系统监控 | `cmd/status/*.go` | `src-tauri/src/status/*` | [§status](#status-系统监控) |

---

<a name="core-工具层"></a>
## core 工具层

### 对标记录

- 原实现：`Mole/internal/units/bytes.go`（87 行），供 analyze（磁盘容量，SI 1000 进制，对齐 Finder/diskutil）与 status（内存/实时计数，二进制 1024 进制，对齐活动监视器）共用。
- 测试向量：`Mole/internal/units/bytes_test.go`，共 4 组 45 个断言，全部翻译为 Rust 单元测试。

### 变更前后对照

| 项 | 原实现（Go） | Rust 实现 | 是否变更 | 原因 |
|----|------------|----------|---------|------|
| `BytesSI(int64)` 负数钳制为 `0 B` | `if size < 0` | `bytes_si(i64)` 相同分支 | 无变更 | 1:1 移植 |
| SI 前缀字符集 `kMGTPE`（小写 k） | `"kMGTPE"[exp]` | `['k','M','G','T','P','E']` | 无变更 | 保持与 Finder 一致 |
| `BytesBin(uint64)` 边界 `>`（恰好 1<<n 停留在小单位，如 1024→"1024 B"） | switch case | `if/else if` 相同比较符 | 无变更 | 1:1 移植 |
| `BytesBinShort` / `BytesBinCompact` 边界 `>=`（恰好 1<<n 晋升大单位） | switch case | 相同比较符 | 无变更 | 1:1 移植 |
| `%.1f` / `%.0f` 舍入 | Go strconv（精确十进制展开 + round-half-even） | Rust `{:.1}` / `{:.0}`（同算法） | 无变更 | 两者舍入语义一致，测试向量（1.5→"2K"）验证通过 |
| 暴露方式 | Go 包内函数 | Rust 模块函数 + Tauri command `format_bytes_si` / `format_bytes_bin`（供前端复用同一实现） | 新增接口 | GUI 前端需要与后端一致的格式化输出，避免前端重复实现第二套逻辑 |

### 编译/移植问题

- 无。Go `int64`/`uint64` 与 Rust `i64`/`u64` 语义一致，无溢出风险（SI 展开最大 exp=5，`div` 最大 10^18 < i64::MAX）。

---

<a name="status-系统监控"></a>
## status 系统监控

### 对标记录

- 原代码：`Mole/cmd/status/`（Go，非测试代码约 3,400 行）。数据源为 gopsutil + macOS 子进程（`sysctl`、`vm_stat`、`memory_pressure`、`diskutil`、`osascript`、`pmset`、`ioreg`、`system_profiler`、`sw_vers`、`ps`、`scutil`、`uptime`）。
- 快照结构 `MetricsSnapshot` 字段与 JSON 标签 1:1 保留（含 `omitempty`/`json:"-"` 语义）。
- 缓存分层节奏 1:1 移植：fast（1s）/process（1s）/full（30s）状态机（`watchState`）；硬件 10min、system_profiler 30s、接口 IP 10s、废纸篓 5s、网络速率窗口下限 100ms。
- 行为细节逐条移植：CPU 两次快照采样与 #1237 墙钟兜底、`kMGTPE`/`>=`/`>` 边界、噪声网卡过滤、Top3 网卡/Top3 磁盘截断、APFS 卷去重键、ps 严格解析与 `ps aux` 回退、僵尸进程父聚合（limit=3）、健康评分全部权重/阈值/文案、pmset/ioreg/system_profiler 三级电池数据合并、CPU 温度刻意不合成的安全约束、C locale 强制（#1267）。

### 变更前后对照

| 项 | 原实现（Go） | Rust 实现 | 是否变更 | 原因 |
|----|------------|----------|---------|------|
| CPU tick 读取 | gopsutil（mach `host_processor_info`） | 直连同一系统调用（libc ABI 声明），tick×0.01s 转换 | 无行为变更 | gopsutil 底层即该接口；秒单位换算后测试向量与 Go 完全一致 |
| 内存统计 | gopsutil `mem.VirtualMemory()` | 直连 `host_statistics64`；Available = free+inactive+purgeable | 无行为变更 | 同 gopsutil darwin 口径 |
| 磁盘枚举 | gopsutil `disk.Partitions/Usage`（getfsstat） | 直连 `getfsstat`，过滤/去重/排序规则 1:1 | 无行为变更 | 同一系统调用 |
| 网络计数 | gopsutil `net.IOCounters`（sysctl NET_RT_IFLIST2） | `getifaddrs` AF_LINK `if_data64`（ifi_ibytes/obytes） | 无行为变更 | 同源内核 64 位计数器 |
| load average | gopsutil（sysctl vm.loadavg） | `getloadavg`（内核同源算法） | 无行为变更 | 数值等价 |
| 硬件信息 | gopsutil `host.Info` | `sysctl`（hostname/boottime/osversion）+ 同样子进程解析 | 无行为变更 | — |
| `collected_at`/`process_collected_at` | RFC3339 字符串 | Unix 秒（f64） | **变更** | 前端 `new Date()` 直接消费；避免引入 chrono，JSON 消费方需按数值解析 |
| Top-N 进程 | 最小堆（processHeap） | 排序取前 N | 无行为变更 | `processRanksBefore` 为全序，两种实现结果等价 |
| 并发采集 | goroutine `collectConcurrently` | 顺序采集 | **变更（暂缓）** | 避免引入异步运行时；命令在独立线程执行，fast 路径 <100ms，full 路径最长数秒但仅每 30s 一次；后续可用 `std::thread` 并行化 |
| GPU 卡片 | `powermetrics`（需 root）+ `system_profiler` | **暂缓**：返回空数组 | **暂缓** | GPU 使用率需 sudo 授权（powermetrics），GUI 下交互路径不同，作为独立子模块跟进 |
| 蓝牙设备 | `system_profiler SPBluetoothDataType` | **暂缓**：返回空数组 | **暂缓** | 独立子模块跟进 |
| 磁盘 IO 速率 | gopsutil `disk.IOCounters`（IOKit） | **暂缓**：返回 0 | **暂缓** | 需 IOKit 绑定，独立子模块跟进；健康评分 IO 扣分项相应暂为 0 |
| APFS purgeable / diskutil SMART / Finder 容量修正 | 三级 fallback（osascript/diskutil） | **暂缓**：raw statfs | **暂缓** | 首版先保证结构与节奏对齐；修正为独立子模块（涉及 osascript 授权） |
| ProcessWatch 告警 | 进程出现/消失告警状态机 | **暂缓** | **暂缓** | 面向 CLI 长驻 watch 场景；GUI 实时视图本身可见进程，待设计 GUI 告警形态 |
| TUI 渲染（view.go，1200 行） | Bubble Tea 表格/动画 | Vue 组件（状态卡片 + SVG 迷你图） | **变更（平台差异）** | 终端 TUI → 图形界面为本项目动机本身 |

### 编译/移植问题

1. **`host_statistics64` 返回 KERN_INVALID_ARGUMENT**：初版 flavor 常量记错（26），且 count 必须与内核结构大小精确匹配。经 SDK 宏（`clang -dM`）确认 `HOST_VM_INFO64=4`；`vm_statistics64` 随 macOS 追加演进，count 由 `build.rs` 构建期编译探针计算（等价 gopsutil 的 cgo sizeof 展开）。
2. **`kern.proc.all` 计数**：返回 `kinfo_proc` 数组而非 PID 数组（首版按 4 字节/进程算出 108864 个进程）；libc crate 未提供该结构，同样由 build.rs 计算 `sizeof(kinfo_proc)`。
3. **mach tick 单位**：gopsutil `TimesStat` 为秒，原始 tick 需乘 0.01s（ClocksPerSec=100）；未换算前测试向量 20% 失败（得到 100%）。
4. Go `wg.Go`/`slices`/`strings.Lines` 等新标准库用法在 Rust 中以等价习语改写，无行为影响。

### 测试

- 46 个单元测试（`cargo test`）：Go 测试向量逐条翻译（parked core 20%、窗口加权总量、拓扑解析、pmset/ioreg/system_profiler 解析、僵尸聚合、健康评分扣分曲线、磁盘过滤与去重、噪声网卡、代理解析、RingBuffer 环绕序、SI/二进制格式化边界）。
- 真机冒烟测试（`#[ignore]`）：本机验证 fast/process/full 全链路——16GB 内存读数 82.1%、635 进程、电池 80%/AC/Good、10 核、Top 进程、utun 代理提示均正确。

---

<a name="clean-深度清理"></a>
## clean 深度清理（子模块 3a：白名单 + 只读预览）

### 对标记录

- 原代码：`bin/clean.sh`（safe_clean 编排）、`lib/core/app_protection.sh`（白名单）、`lib/clean/user.sh`（clean_app_caches 目录）。
- 本子模块完成：白名单加载/匹配 1:1、`_safe_clean_impl` 的逐路径检查顺序（保护 → 白名单）、Apple 用户缓存族目录（31 条 `safe_clean` 行）、glob 展开（nullglob 语义）、带 deadline 的目录测径、dry-run 预览。
- 原 `clean_app_caches` 的注释性排除项（Autosave Information、Calendar Cache、壁纸封面缩略图 #1118、E5RT 模型缓存）转化为测试锁定，防止回归。

### 变更前后对照

| 项 | 原实现（bash） | Rust 实现 | 是否变更 | 原因 |
|----|------------|----------|---------|------|
| 白名单匹配 | bash `[[ == $pattern ]]`（`*` 跨 `/`）+ 父目录保护 + 非 glob 条目子路径保护 | 自实现 fnmatch 匹配器，四条规则逐条移植 | 无行为变更 | 测试锁定：`*` 跨 `/`、`?`、`[a-z]`、`[!a]` |
| 白名单文件校验 | `//` 拒绝、系统路径拒绝、去重、`~` 展开、用户文件替换默认项 | 相同 | 无行为变更 | `/` 只匹配根路径本身（case 语义），不是全路径前缀 |
| `safe_clean` 检查顺序 | 存在性 → should_protect_path → whitelist → compiled model cache | 存在性 → 保护前缀 → 白名单 | **部分暂缓** | `should_protect_path` 完整数据（bundle ID 表）与 compiled model cache 检查在 3b 移植 |
| 删除执行 | `mole_delete`（Trash 路由 + 操作日志） | **未提供**（只有只读预览） | **暂缓** | fail-safe：完整保护层落地前不开放任何删除 |
| glob 展开 | shell nullglob | 组件级展开 + fnmatch | 无行为变更 | 语义一致 |
| 目录测径 | `du`-style + timeout | 递归 + 2s deadline，跳过符号链接 | 无行为变更 | 对标 timeout-bounded 约束 |

### 编译/移植问题

- 初版把白名单拒绝规则中的 `/` 实现为路径前缀，会把所有绝对路径误判为系统路径；原 case 语句中 `/` 仅精确匹配根路径。已修正并以测试锁定。

### 测试

- 白名单：glob 语义（跨 `/`、字符类、负类）、父/子方向保护、系统路径拒绝、`//` 与 `~` 处理。
- clean：glob 展开、测径（含符号链接跳过）、排除项锁定、保护前缀。
- 真机冒烟（`#[ignore]`）：本机扫描 30 组、83.96 MB 可释放，白名单 default。
