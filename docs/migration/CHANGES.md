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

---

<a name="clean-深度清理3b"></a>
## clean 深度清理（子模块 3b：完整保护层 + Trash 安全删除 + 双日志）

### 对标记录

- `should_protect_path`（app_protection.sh:357，7 层检查）1:1 移植：共享 home 状态根 → Codex 可重建缓存叶 → OrbStack → 关键词层（大小写变体）→ 系统 UI 关键缓存 → 容器 bundle ID 提取（Caches/tmp 放行）→ EDR 代理（nocasematch，锚定 /private/var/folders）→ E5RT 编译模型缓存 → 偏好/用户数据/高风险 denylist（逐条转译）→ 全路径 bundle 模式匹配 → 文件名级 `should_protect_data`。
- 数据表（app_protection_data.sh）1:1 搬运：SYSTEM_CRITICAL_BUNDLES（118 项）、DATA_PROTECTED_BUNDLES（约 280 项）、ENDPOINT_SECURITY_BUNDLE_PREFIXES（9 项）。
- `validate_path_for_deletion` + `_mole_is_critical_deletion_path`：绝对路径、`..` 组件、控制字符、符号链接目标与祖先链接重检、关键系统路径拒绝（拒绝臂分"仅精确"与"含子树"两类）。
- `mole_delete` trash 模式：验证 → sink 复检 → 尺寸捕获 → Trash 路由（`trash` CLI → Finder AppleScript（路径经 argv 防逃逸）→ `~/.Trash` 直移）→ fail-closed（Trash 不可用绝不回退永久删除）。
- 双日志 1:1：`operations.log`（`[ts] [clean] TRASHED path (KB)`，`MO_NO_OPLOG=1` 可禁用）+ `deletions.log`（TSV 取证日志）。

### 变更前后对照

| 项 | 原实现（bash） | Rust 实现 | 是否变更 | 原因 |
|----|------------|----------|---------|------|
| `should_protect_path` 分层 | 7 层 case/regex | 同顺序 7 层（glob/前缀/大小写等价转译） | 无行为变更 | 测试锁定各层语义 |
| 卸载模式分支（MOLE_UNINSTALL_MODE=1） | APPLE_UNINSTALLABLE_APPS 先放行 | 未实现 | **暂缓** | clean 不需要；uninstall 模块移植时补齐 |
| Trash 路由 | trash CLI → Finder → sudo staging | trash CLI → Finder → ~/.Trash 直移 | **简化（仅用户级路径）** | 3b 范围为用户级缓存（无 sudo 需求）；特权路径 staging 属 uninstall/system 清理族，届时移植 |
| MOLE_TEST_TRASH_DIR 测试缝 | 直移到指定目录 | 相同 | 无行为变更 | 单测据此验证删除语义 |
| dry-run | 记录预览不删除 | 相同（status="dry-run"） | 无行为变更 | — |
| 执行流程 | safe_clean 边扫边删 | **重扫后执行**：前端只传组描述（非路径），后端重新扫描 + sink 复检 | 加固 | 对标 "materialize only completed scans"：不消费陈旧预览；前端无法注入任意路径 |
| 保护层对扫描的反馈 | 同一函数 | 相同（scan 与 execute 共用 skip_reason） | 无行为变更 | 3a 时 akd 组曾显示可清理；3b 完整层正确拦截（与原实现 step 7 行为一致） |

### 编译/移植问题

1. 关键路径拒绝臂语义：`/Users`、`/Library`、`/Applications` 是**仅精确匹配**（用户家目录可删），`/System`、`/bin` 等是**含子树**——首版混用导致合法路径被拒，已按原 case 语句逐臂区分。
2. bash glob 锚定语义两处反例进入测试期望（`ClashX*` 不匹配中缀；`com.crowdstrike.*` 经文件名级检查保护任意位置的 EDR 缓存），按原行为修正测试。

### 测试

- 71 个测试通过：保护层各层（关键词/容器/EDR/E5RT/共享根/Codex 叶）、`should_protect_data` case 组、路径验证拒绝矩阵（含 `name..files` 合例外、Homebrew 子条目放行）、trash 直移 + 双日志 + dry-run 语义（MOLE_TEST_TRASH_DIR 缝）。
- 真机 dry-run 全链路冒烟：30 项、0 删除、保护层正确拦截。

---

<a name="clean-深度清理3c"></a>
## clean 深度清理（子模块 3c 第一片：开发工具链清理族）

### 对标记录

- 原代码：`lib/clean/dev.sh`（5,554 行）的普通 `safe_clean` 行，按原函数分组为 7 个清理族：前端构建（clean_dev_frontend + yarn/tnpm 行）、Python（clean_dev_python 普通行）、Rust（rustup downloads 行）、Ruby/Perl（clean_dev_ruby / clean_dev_perl）、云 CLI 与容器、CI 与 DevOps。共 46 条显式行，目录总量 75 组。
- 环境基址语义：`resolve_tool_home "${RUSTUP_HOME:-}" ~/.rustup` 移植为 CatalogEntry.home_env。

### 变更前后对照

| 项 | 原实现（bash） | Rust 实现 | 是否变更 | 原因 |
|----|------------|----------|---------|------|
| 普通 safe_clean 行 | 逐行调用 | 目录表 1:1（含描述原文） | 无行为变更 | — |
| RUSTUP_HOME/CARGO_HOME 解析 | resolve_tool_home | home_env 字段（env 绝对路径优先，否则 HOME） | 无行为变更 | — |
| npm/bun/corepack/uv/mise/pip 缓存目录解析 | 探测 owner 命令/自定义路径 | **暂缓** | **暂缓** | 需 owner 命令探测逻辑，作为独立子片移植 |
| cargo registry/cache | owner-process guard（tri-state） | **暂缓** | **暂缓** | 需进程状态三态判定，作为独立子片移植 |
| 混合状态/模型/会话存储 | 不在目录（AGENTS.md 恢复契约分级） | 测试锁定不入目录 | 无行为变更 | registry/src、Cargo git、HuggingFace/torch/tensorflow/wandb、pypoetry/virtualenvs、.cpan/sources、.m2、AI CLI 缓存等 |
| Android/JetBrains/浏览器/数据库/抓包工具族 | dev.sh 其余函数 | **暂缓** | **暂缓** | 涉及版本管理与泄漏配置扫描（check_multiple_versions、JetBrains Toolbox、泄漏 profile），后续子片 |

### 测试

- 74 个测试通过；新增：目录排除项锁定（混合状态存储/AI 会话）、描述唯一性、族标签覆盖、home_env 解析。
- 真机冒烟：75 组扫描正常。

---

<a name="clean-深度清理3c-二"></a>
## clean 深度清理（子模块 3c 第二片：owner 命令探测行）

### 对标记录

- 原代码：`lib/clean/dev.sh` 的 `clean_dev_npm`（npm config get cache + 自定义路径规范化去重）、`clean_uv_cache`（uv cache dir / else 回退）、`clean_corepack_cache`（COREPACK_HOME + 不安全路径拒绝 / else 回退）、`get_mise_cache_path`（MISE_CACHE_DIR → mise cache path → 默认）、`resolve_tool_home`（#1378 防污染校验）。
- 实现：`clean/probe.rs` —— 探测输出校验（绝对路径、拒绝 `..`、控制字符、尾斜杠归一）+ 各工具路径解析 1:1。

### 变更前后对照

| 项 | 原实现（bash） | Rust 实现 | 是否变更 | 原因 |
|----|------------|----------|---------|------|
| npm 残留目录行（默认路径） | 无条件 safe_clean | 相同（无条件） | 无行为变更 | — |
| npm 自定义路径行 | 探测 + realpath 规范化去重后追加 "(custom path)" 行 | 相同 | 无行为变更 | canonicalize 对标 `cd && pwd -P` |
| uv/corepack 回退行 | owner 命令不可用时 safe_clean | 相同（tool_available 判定） | 无行为变更 | — |
| corepack 不安全路径拒绝 | `/`、`$HOME`、`~/Library` 拒绝 | 相同 | 无行为变更 | — |
| mise 行 | 无条件 safe_clean（路径三级解析） | 相同 | 无行为变更 | — |
| **owner 命令删除汇** | `npm cache clean --force`、`uv cache prune`、`corepack cache clean`、`pnpm store prune`、`pip cache purge`、`bun pm cache rm` | **未实现** | **暂缓** | AGENTS.md 契约：owner 命令作为删除汇必须"变更根可机器读出、dry-run 与真实共享同一候选计划、部分失败可观察"；需独立设计与子片。因此本机上若安装了这些工具，批量清空类动作（如 npm 缓存整体清空）暂由残留目录行以 Trash 方式覆盖 |
| pnpm store 多二进制探测 | 逐 pnpm 二进制 store path + prune | 未实现 | **暂缓** | 同 owner 命令删除汇契约 |

### 测试

- 78 个测试通过：探测输出校验（绝对路径/`..`/控制字符/尾斜杠）、corepack 默认路径、npm 探测回退、路径规范化；保护层/删除/白名单既有测试无回归。
- 真机冒烟：82 组（新增 npm 残留 4 行探测生效，npm cache directory 164.93 MB）。

---

<a name="status-系统监控-gpu-bt"></a>
## status 系统监控（增量子片：GPU + 蓝牙）

### 对标记录

- `metrics_gpu.go`（196 行）1:1：静态 GPU 信息经 `system_profiler -json SPDisplaysDataType`（10min 缓存，note 拼接 "VRAM x · Metal · Vendor"），实时使用率经 `powermetrics --samplers gpu_power`（5s 缓存；解析 "GPU HW active residency" 并以 idle residency 回退推导；powermetrics 通常需 root，失败返回 -1 哨兵，与 Go 一致），使用率只应用到第一块 GPU。
- `metrics_bluetooth.go`（138 行）1:1：`system_profiler SPBluetoothDataType` 缩进层级解析（顶层节重置、8 空格设备头、Connected/Battery Level）→ `bluetoothctl info` 回退 → 30s 缓存 + "No Bluetooth info" 占位。

### 变更前后对照

| 项 | 原实现（Go） | Rust 实现 | 是否变更 | 原因 |
|----|------------|----------|---------|------|
| GPU JSON 解析 | struct unmarshal | serde_json Value 字段访问 | 无行为变更 | 字段名一致（_name/spdisplays_vram/spdisplays_vendor/spdisplays_metal/sppci_cores） |
| GPU usage 正则 | regexp 两个模式 | 字符串定位 + 数字前缀截取 | 无行为变更 | 等价（固定标记 + 浮点） |
| nvidia-smi 分支 | 非 darwin 平台 | 未移植 | **平台差异** | 本应用仅支持 macOS，darwin 分支恒可达 |
| UI | TUI GPU/蓝牙行 | Vue GPU 卡（usage=-1 显示"需 root"）+ 蓝牙设备列表 | 平台差异 | TUI → GUI |

### 测试

- 83 个测试通过（新增 JSON 解析、usage 正则等价、蓝牙缩进解析含顶层节重置与空回退、bluetoothctl 解析）。

---

<a name="purge-项目清理"></a>
## purge 项目清理（模块4，第一片：扫描 + dry-run + Trash 执行）

### 对标记录

- 原代码：`bin/purge.sh`（编排）、`lib/clean/project.sh`（2,549 行扫描管线）、`lib/clean/purge_shared.sh`（配置数据）。
- 数据 1:1：PURGE_TARGETS（33 项）、PROJECT_INDICATORS/MONOREPO_INDICATORS、DEFAULT_PURGE_SEARCH_PATHS（含 `.codex/worktrees`、`.claude/worktrees` 显式容器）、CACHEDIR.TAG 签名、`~/.config/mole/purge_paths` 配置。
- 管线 1:1：容器发现（默认路径 ∪ HOME 一级探针 ∪ 配置）→ 有界遍历（深度 1–6，目标命名即 prune，.git/Library/.Trash/Applications 不下钻，CACHEDIR.TAG 有效签名目录同目标）→ 每根独立过滤后才并入 → 嵌套折叠（字节序排序 + 前缀丢弃，对标 awk 管线）→ 物理包含校验（canonicalize 双侧）→ 保护谓词（bin/.NET、vendor/Composer、项目内 DerivedData）→ 活动分级（7 天，find -mtime -7 语义，fail-closed）→ 按项目父目录分组。

### 变更前后对照

| 项 | 原实现（bash） | Rust 实现 | 是否变更 | 原因 |
|----|------------|----------|---------|------|
| 大小写去重 | /bin/pwd 取磁盘真实名（#1416） | canonicalize | 无行为变更 | getcwd 语义等价 |
| fd/find 双引擎 | fd 优先，MO_USE_FIND 回退 | 单一有界遍历 | **实现差异（无行为差异）** | 遍历语义与 fd --prune 对齐（目标目录不下钻）；原生实现避免外部引擎差异 |
| 扫描进度文件 purge_scanning | XDG 缓存目录状态文件 | 未实现 | **暂缓** | 供 TUI 实时进度条；GUI 用整段扫描结果渲染 |
| 活动预算 _PURGE_ACTIVITY_DEADLINE_EPOCH | 整个分类 pass 共享 deadline | 每项 5s 预算 | **简化** | 原语义"预算耗尽不返回 old"已保留（fail-closed）；整体 deadline 后续并入 |
| `mo purge --paths` 交互配置编辑 | 菜单写入配置文件 | 配置文件只读消费 | **暂缓** | 编辑 UI 后续补 |
| 执行入口 | purge_target_activity_still_safe + safe_remove | execute() 内 sink 复检（包含性 + 保护 + 活动 old）+ Trash 删除 | 无行为变更（加强） | 复检项对齐 + 复用 clean 的 Trash/双日志 |

### 测试

- 90 个测试通过（新增：#1459 容器守卫、嵌套折叠、vendor/bin/DerivedData 分级、深度规则与单项目模式、CACHEDIR.TAG 签名、walk prune/排除、活动分级、路径消失=old）。
- 真机冒烟：发现 1 个搜索根、0 产物（本机项目在 HOME 外，符合预期；自定义路径经配置文件支持）。

---

<a name="analyze-磁盘分析"></a>
## analyze 磁盘分析（模块5，第一片：扫描器 + 浏览 + Trash 删除）

### 对标记录

- 原代码：`cmd/analyze` 的 `model.go`（数据结构）、`scanner.go`（容量语义）、`delete.go`（删除入口）；TUI（update.go/view.go，约 1,800 行）由 Vue 页替代。
- 数据模型 1:1：单层目录浏览 + 按需下钻（dirEntry/fileEntry/scanResult）。
- 容量语义 1:1：文件计 `min(blocks*512, len)`（实际占用，对标 getActualFileSize）；同一次扫描内硬链接（nlink>1，dev+ino）只计一次（对标 countableFileSize 的 seen map）；符号链接不计大小；Top-20 大文件（对标 maxLargeFiles 最小堆，导出按大小降序）。

### 变更前后对照

| 项 | 原实现（Go） | Rust 实现 | 是否变更 | 原因 |
|----|------------|----------|---------|------|
| 并发扫描 | goroutine + channel + 信号量 | 并行遍历器（≤8 worker 共享有界任务队列；初版曾为串行，后按原版并发设计补齐，见 §交互与性能改进） | 无行为差异 | 对标 calculateDirSizeConcurrent |
| 大文件发现 | mdfind（Spotlight）预热 + 扫描堆 | 纯扫描堆（无下限阈值） | **简化** | Top-20 结果集合等价；Spotlight 快速预热属性能优化，后续子片可加 |
| entriesHeap=30 | TUI 每层仅展示 Top30 | 返回全部子项 | **变更** | GUI 列表可滚动，TUI 堆是终端展示约束 |
| 缓存层 cache.go（820 行） | 磁盘缓存（TTL 7 天、schema 版本、准入预算） | 未实现 | **暂缓** | GUI 按需下钻成本低；重复扫描预算 30s 封顶，缓存层作为后续子片 |
| 快照对比 snapshots.go / 概览 overview | 概览缓存与对比 | 未实现 | **暂缓** | 独立子片 |
| 删除入口 | delete.go 确认后 mole_delete | 仅当前层直接子项 → clean::delete 统一 Trash | 无行为变更（加强） | 直接子项校验防路径注入 |

### 测试

- 95 个测试通过（新增：递归求和与计数、Top-20 截断、符号链接不计、非直接子项拒绝/dry-run/注入路径拒绝、扫描目标校验）。
- 真机冒烟：扫描 src-tauri 3,235.5 MB / 19,721 文件 / 980 目录，target/ 正确定位为最大子项，未截断。

---

<a name="uninstall-应用卸载"></a>
## uninstall 应用卸载（模块6第一片：只读应用清单 + 保护分级）

### 对标记录

- 原代码：`bin/uninstall.sh`（搜索目录/mtimes 清单/bundle ID 解析/后台应用过滤）、`lib/uninstall/batch.sh`（Wrapper 回退）、`lib/core/app_protection.sh` 的 `should_protect_from_uninstall`。
- 搜索目录 1:1：/Applications、~/Applications、/Library/Input Methods、~/Library/Input Methods、/Volumes/*/Applications（与 /Applications 同卷去重）。
- bundle ID 1:1：`Contents/Info.plist` CFBundleIdentifier（`|`→`-`、控制字符剔除、`(null)` 判空）+ iOS 应用 Wrapper/*.app/Info.plist 回退。
- 后台专用应用（LSBackgroundOnly）仅当直接位于搜索根时列出（对标）。
- 保护分级 1:1：先判 APPLE_UNINSTALLABLE_APPS（放行）→ 再判 SYSTEM_CRITICAL_BUNDLES（保护）；build_regex_var 的 `^pattern$`（点转义、`*`→任意）与整串 glob 语义等价，用 glob_match 实现。
- plist 读取改用纯 Rust `plist` crate（对标 plutil -extract 输出；二进制/XML 两种格式均支持）。

### 变更前后对照

| 项 | 原实现（bash） | Rust 实现 | 是否变更 | 原因 |
|----|------------|----------|---------|------|
| plutil 子进程 | 每应用 1-2 次 plutil | plist crate 直读 | 实现差异 | 输出等价，免去每应用 fork |
| 应用内嵌 .app 下钻 | maxdepth 3 | 相同（深度 0-3） | 无行为变更 | — |
| 大小计量 | du -sk | path_size_with_deadline（2s 截断） | 无行为变更 | 与 clean 一致 |
| **应用删除** | 卸载流程（登录项/LaunchServices/残留/zap） | **未实现** | **暂缓** | 删除汇按 AGENTS.md "逐行复核" 要求移植：find_app_files（约 700 行，含共享 bundle ID 兄弟守卫）、remove_file_list、Cask zap 等为第二片 |
| brew/steam 卸载 | lib/uninstall/brew.sh、steam.sh | 未实现 | **暂缓** | 第二片 |

### 测试

- 98 个测试通过（新增：bundle ID 清洗、保护分级含锚定反例、搜索目录覆盖）。
- 真机冒烟：42 个应用、bundle ID 与保护标记正确（GarageBand 归为 Apple 可卸载、系统 App 标 🛡）。

---

<a name="history-历史记录"></a>
## history 历史记录（模块8第一片）

### 对标记录

- 原代码：`bin/history.sh` + `lib/core/history.sh`（564 行）。
- operations.log 解析 1:1：会话开始/结束标记行、`[ts] [cmd] ACTION path (detail)` 操作行按 action 聚合（removed/trashed/skipped/failed/rebuilt/other）；未终止会话读取时闭合（对标 history_finish_session）；limit 1-200 取最近 N 条并倒序输出（对标 history_normalize_limit + 渲染顺序）。
- deletions.log TSV 解析 1:1（五列，空行与短行跳过）。

### 变更前后对照

| 项 | 原实现（bash） | Rust 实现 | 是否变更 | 原因 |
|----|------------|----------|---------|------|
| 会话归属 | 全局状态机逐行解析 | 最近同名未闭合会话归属 | 无行为变更 | shell 日志为单写者追加，语义一致 |
| `mo history` 文本渲染 | 终端表格 | Vue 会话卡片 + 取证表 | 平台差异 | TUI → GUI |
| 无效行处理 | 静默跳过 | 相同 | 无行为变更 | — |

### 测试

- 101 个测试通过（新增：三种行格式解析、items/size 提取、无效行、limit 边界）。

---

<a name="ux-性能"></a>
## 交互与性能改进：页面缓存、显式触发、并行扫描/测径

### 问题（用户反馈）

1. 磁盘分析扫描中切换标签页，返回后扫描状态丢失、从头重扫——页面由
   `v-if` 卸载，组件状态被销毁。
2. 打开标签页即自动开始扫描，不符合「用户显式触发」的交互预期（对标
   CLI 中 `mo analyze` 等也是手动运行的）。
3. 磁盘分析、应用卸载反应非常慢。

### 变更前后对照

| 项 | 变更前 | 变更后 | 原因 |
|----|-------|-------|------|
| 页面切换 | `v-if`/`v-else-if` 链，切页即卸载 | `<KeepAlive>` + 动态组件 | 切走不丢状态；后端命令本就在后台线程继续执行，回到页面即见结果 |
| 重操作触发 | 页面挂载即自动扫描（analyze/clean/purge/uninstall） | 空闲引导态 + 「开始扫描」按钮，结果页保留「重新扫描」 | 对标 CLI 手动运行子命令的语义；扫描是用户应知情的动作 |
| 监控页采集 | KeepAlive 下仍每秒轮询 | `onActivated/onDeactivated` 暂停/恢复 | 后台标签页不占用采集线程；节奏与 watch 模式一致 |
| analyze 扫描 | 单线程递归（初版把 Go 并发简化为串行） | 并行遍历器：共享有界任务队列 + `pending` 计数 + ≤8 worker（对标 `calculateDirSizeConcurrent`），桶号归集各顶层条目大小；deadline 触发全局 stop 快速收尾 | 1:1 补齐原版并发设计。真机：$HOME 18.7 GB/42.3 万文件由「30s 预算耗尽截断」→ **3.4s 完整完成** |
| uninstall 测径 | 串行逐应用（每只 ≤2s，44 只最坏 ~88s），每应用读 Info.plist 4 次 | 两阶段：清单（每应用读 plist 1 次）→ 跨应用并行测径（≤8 worker，单只 2s 上限不变） | 语义 1:1（du 语义、2s 上限、保护分级不变），真机 5.8s → 2.1s |

### 语义说明

- analyze：硬链接去重后「哪个条目名承载大小」取决于遍历顺序（read_dir
  顺序本就不保证，与原串行实现一致）；确定性不变量（总量计一次、
  Top-20、截断标记）有测试锁定（新增 `hardlinks_counted_once`）。
- truncated 时顶层枚举同步检查 deadline，行为与原「枚举中断」一致。

### 测试

- `cargo test` 105 通过（新增硬链接并行去重）；真机冒烟：analyze
  0.19s（4 GB/2.9 万文件完整）、uninstall 2.07s（44 应用、protected=1）。
- 前端 `vue-tsc` + `vite build` 通过。

---

<a name="runtime-命令层"></a>
## 运行时修复：命令层主线程冻结（全局）+ uninstall 符号链接漏列

### 问题（真机实测）

1. **全部重命令冻结 UI**：`sample` 显示主线程过半时间阻塞在
   `run_invoke_handler → analyze_scan → dir_size`（status_tick / clean_preview /
   purge_scan / uninstall_list_apps 同理）。原因：Tauri 2 的**同步命令**经
   WKScriptMessage 在主线程执行，扫描/子进程期间事件循环停转，窗口假死、
   无法点击，打开任一重页面（含默认监控页的 30s full 采集）均卡死。
2. **卸载清单漏掉符号链接应用**：本机 `/Applications/Safari.app` 是指向
   `../System/Cryptex/App/System/Applications/Safari.app` 的符号链接，
   Rust `entry.file_type()` 不跟踪链接 → `is_dir()==false` → 整只跳过
   （42 个应用、protected=0）；原实现 `find -iname "*.app"` + `-d` 检查
   （`-d` 跟踪链接）会列出 Safari 且标记 🛡。
3. **所有删除操作永远无法确认执行**：Clean/Purge/Analyze 三页的执行
   按钮依赖 `window.confirm`，而 Tauri 的 WKWebView（wry 0.55.1）未实现
   WKUIDelegate 的 JS 对话框方法（源码无
   `runJavaScriptConfirmPanelWithMessage`），WebKit 对未实现的对话框一律
   按"用户取消"处理 → `confirm()` 恒返回 false → 执行函数静默返回。

### 变更前后对照（3：确认对话框）

| 项 | 变更前 | 变更后 | 原因 |
|----|-------|-------|------|
| 删除前确认 | `window.confirm(...)`（恒 false，删除永不可达） | 应用内模态确认框 `composables/confirm.ts` + `ConfirmDialog.vue`（Promise 风格 `await confirm(msg)`） | 对标终端 `mo` 删除前 y/N 确认的契约；wry 未实现原生对话框，属平台差异 |

### 变更前后对照

| 项 | 变更前（Rust） | 变更后（Rust） | 原因 |
|----|------------|------------|------|
| 命令执行模型 | 重命令为同步 `fn` → 主线程执行 → 冻结 | `async fn` + `tauri::async_runtime::spawn_blocking` → 阻塞线程池 | Tauri 2 async 命令离开主线程；对标 Go 版采集在独立 goroutine、不阻塞 TUI 的模型 |
| `CollectorState` | `Mutex<Collector>` | `Arc<Mutex<Collector>>` | 锁需移入 `spawn_blocking` 闭包 |
| 命令返回类型 | `T` / `Result<T,String>` 混合 | 统一 `Result<T, String>` | Tauri 把 Ok/Err 映射为 promise resolve/reject，前端 try/catch 契约不变；JoinError 有了可观察的错误通道 |
| 保留同步的命令 | — | `format_bytes_*`、`get_home_dir`（纯 env 读取） | 微秒级，无冻结风险 |
| uninstall 目录遍历 | `entry.file_type().is_dir()`（不跟踪链接） | `.app` 按名匹配 + `fs::metadata().is_dir()`（跟踪链接，对标 `-d`）；下钻仅限真实目录（对标 find 不进入链接目录） | 1:1 对标 `find -maxdepth 3 -iname "*.app"` |

### 验证

- `sample` 复测：主线程 0 样本位于 mole_rs 代码（修复前 >50%），命令处理
  全部出现在后台线程。
- `cargo test` 104 通过；真机冒烟 7 项通过；卸载清单 42→44 个应用、
  Safari 正确标记 🛡（protected=1）。
- 前端 `vue-tsc` + `vite build` 通过；13 个 invoke 命令名与注册表一一对应；
  确认对话框接入 Clean/Purge/Analyze 三个执行入口。

---

<a name="optimize-优化维护"></a>
## optimize 优化维护（模块7第一片：任务目录 + 执行框架 + saved_state_cleanup）

### 对标记录

- 原代码：`lib/optimize/catalog.sh`（21 项任务注册，含 validate 校验）、`lib/optimize/outcomes.sh`（六态结果）、`lib/optimize/tasks.sh`（1,887 行处理器）。
- 目录 1:1：21 项任务的 action/health_name/description 逐条搬运，顺序一致；`optimize_catalog_validate` 的约束（action 唯一、命名规范、元数据完整）转为测试。
- 结果语义 1:1：applied/unchanged/skipped/unavailable/attention/failed 六态计数与摘要。
- `saved_state_cleanup` 处理器 1:1：`~/Library/Saved Application State` 下 `*.savedState` 且 mtime 超 30 天；有界扫描（超时整批放弃，对标 "materialize only completed scans"）；逐项 should_protect_path（复用完整保护层）；删除走统一 Trash + 双日志。

### 变更前后对照

| 项 | 原实现（bash） | Rust 实现 | 是否变更 | 原因 |
|----|------------|----------|---------|------|
| 其余 20 个处理器 | tasks.sh 各自实现（多数含 sudo/launchctl/mdfind 交互） | 框架就位，执行返回 unavailable，UI 标注"待迁移" | **暂缓** | 每个处理器都是独立系统交互面（DNS flush、SQLite vacuum、LaunchServices 重建等），按计划逐个对标移植后开放，不做行为猜测 |
| 白名单项过滤（opt_* 对应的 whitelist names） | 按 whitelist name 跳过 | 未接入（保护层已覆盖路径类任务） | **暂缓** | 与其余处理器一起移植 |
| sudo 类任务授权 | MOLE_TEST_NO_AUTH 体系 | 不适用 | 平台差异 | GUI 场景后续用 macOS 授权 API 评估 |

### 测试

- 104 个测试通过（新增：目录完整性/唯一性/命名规范、implemented 标记一致性、未移植任务 unavailable 语义）。
- 真机冒烟：本机无 `~/Library/Saved Application State`（macOS 26 惰性创建）→ Unavailable，与原实现 `-d` 分支行为一致。

---

<a name="optimize-7b"></a>
## optimize 优化维护（模块7第二片：cache_refresh / prevent_network_dsstore / legacy_overrides_audit）

### 对标记录

- `opt_cache_refresh` 1:1：`qlmanage -r cache`（缩略图）+ `qlmanage -r`（图标）刷新（dry-run 跳过）；三个固定缓存目标（QuickLook.thumbnailcache、iconservices.store、iconservices）存在性 → 保护检查 → 删除。
- `opt_prevent_network_dsstore` 1:1：`defaults read/write com.apple.desktopservices` 两个键（DSDontWriteNetworkStores / DSDontWriteUSBStores），already/changed/failed 计数与结果映射。
- `opt_legacy_overrides_audit` 1:1：App Nap 全局开关（NSAppSleepDisabled，truthy 判定 1/TRUE/YES）+ DiskImages skip-verify 家族三个键；白名单检查对应 plist 后 `defaults delete`；只删显式覆盖键，不写替代偏好（#1242/#1243）。

### 变更前后对照

| 项 | 原实现（bash） | Rust 实现 | 是否变更 | 原因 |
|----|------------|----------|---------|------|
| cache_refresh 删除 | safe_remove（永久） | delete_to_trash（回收站） | **加固** | 统一 Trash 可恢复契约（同 clean/purge/analyze） |
| 结果映射 | optimize_task_result_from_counts | failed>0→Failed；changed>0→Applied；skipped-only→Skipped；否则 Unchanged | 无行为变更 | 与六态语义对齐 |

### 测试

- 105 个测试通过；真机 dry-run 冒烟四项全部正常（本机状态：.DS_Store 预防已生效、无遗留覆盖、无缓存待清、无 Saved State 目录）。

---

<a name="manage-设置页"></a>
## manage 设置页（模块8b：白名单 + purge_paths 配置管理）

### 对标记录

- 原代码：`lib/manage/whitelist.sh` 的读取/校验语义、`lib/clean/project.sh` 的 `mole_purge_read_paths_config`。
- 读取 1:1：配置文件原文逐行（含注释）；`~` 展开与系统路径拒绝校验复用清理模块同一规则集（保证"写进白名单的条目在清理时必然生效"）。
- 写回采用 tmp + rename 原子写，逐行校验（系统路径、`//`、`..` 组件、相对路径）后才落盘。

### 变更前后对照

| 项 | 原实现（bash） | Rust 实现 | 是否变更 | 原因 |
|----|------------|----------|---------|------|
| 交互式菜单管理 | `mo clean --whitelist` TUI | 设置页列表 + 添加/删除 + 保存 | 平台差异 | TUI → GUI |
| 写回校验 | 读取侧拒绝规则 | 写入侧同规则预检 | 加固 | 非法行在落盘前即报错，避免污染清理判定 |
| 原子写 | 无（直接覆盖） | tmp + rename | 加固 | 中断不产生半截配置 |

### 测试

- 108 个测试通过（新增：白名单行校验矩阵、purge 路径校验、配置往返一致）。

---

<a name="uninstall-6b"></a>
## uninstall 应用卸载（模块6b：应用本体 + 精确 bundle ID 残留删除）

### 对标记录

- `mole_is_reverse_dns_bundle_id`（base.sh:795）1:1：至少两段、每段字母数字开头、允许内部连字符。
- `find_app_files` 的 bundle ID 字面路径集合 1:1（reverse-DNS 校验后）：Application Support、Caches、Logs、Saved Application State/{id}.savedState、Containers、WebKit（含 WebContent 子目录）、HTTPStorages（含 .binarycookies）、Cookies/{id}.binarycookies、Application Scripts、Input Methods/{id}.app、Autosave Information、SyncedPreferences/{id}.plist。
- 卸载模式保护前置：should_protect_from_uninstall 拒绝系统关键应用（含其残留）。
- 删除统一走 delete_to_trash（回收站可恢复）+ 双日志。

### 变更前后对照

| 项 | 原实现（bash） | Rust 实现 | 是否变更 | 原因 |
|----|------------|----------|---------|------|
| 名称变体路径（nospace/hyphen/lowercase/base_name） | find_app_files 的 user_patterns 大集合 | 未实现 | **暂缓（6c）** | 名称变体命中宽，需连同共享兄弟守卫一起逐行复核 |
| bundle leaf 推导（camel 边界规则） | 复杂派生 | 未实现 | **暂缓（6c）** | 需完整移植推导条件（≥8 字符、驼峰转换、显示名前缀） |
| LaunchAgents/Daemons 扫描、Receipts、登录项 | 独立扫描族 | 未实现 | **暂缓（6c）** | 删除汇逐行复核 |
| 共享 bundle ID 兄弟守卫 | /Volumes 副本、逆名、共享身份变体守卫 | 未实现 | **暂缓（6c）** | AGENTS.md 要求每个变体一条回归测试 |
| 删除入口 | remove_file_list + 确认 | 单应用逐项 Trash | 无行为变更（保守子集） | 精确证据优先 |

### 测试

- 112 个测试通过（新增：reverse-DNS 校验矩阵、残留路径精确性与越界检查、不存在的应用跳过、受保护应用整体跳过且文件保留）。

---

<a name="uninstall-6c"></a>
## uninstall 应用卸载（模块6c：名称变体残留查找 + 卸载模式保护语义）

### 对标记录

- `find_app_files` 的用户级名称模式 1:1：主名 Library 位置（含插件类 23 处）+ dotdirs（.config/.cache/.local/share 的原样与变体）+ base_name 变体（版本/渠道后缀剥离，大小写敏感、多词后缀）。
- 变体派生 1:1：nospace/hyphen/underscore/lowercase 四类 + base；`_mole_uninstall_name_variant_matches` 前缀边界五种形态（== v、v+" "、v+"-"、v+"_"、v+"."）。
- 安全过滤 1:1：常用目录根跳过（空名称/空 bundle 产物防整目录删除）、`_mole_is_shared_home_state_root` 共享根跳过、`_path_belongs_to_independent_cli` 独立 CLI dotdir 保护（#993：claude/opencode/codex/gemini）。
- vendor-nested 1:1（`find_vendor_nested_app_paths`）：vendor 段目录（≥3 字符校验）下深度 2 子项变体匹配，通用词名拒绝。
- ByHost 与用户 LaunchAgents 1:1：bundle ID 边界匹配（`mole_name_starts_with_bundle_id_boundary`）+ `${bundle_id}.*.plist` 前缀。
- Zed 渠道特例（#422）：dev.zed.Zed-* HTTPStorages 前缀变体。
- **卸载模式保护语义**（MOLE_UNINSTALL_MODE=1）：跳过文件名级检查；容器数据保护跳过；步骤 6 只判 SYSTEM_CRITICAL_BUNDLES（APPLE_UNINSTALLABLE 先放行）；DATA_PROTECTED 不拦用户显式选择的卸载。

### 变更前后对照

| 项 | 原实现（bash） | Rust 实现 | 是否变更 | 原因 |
|----|------------|----------|---------|------|
| find 通配扫描 | _mole_uninstall_materialize_find0（有界、完整产出） | Rust 遍历（同语义：只消费完整扫描） | 无行为变更 | — |
| should_protect_path 模式 | 全局 env MOLE_UNINSTALL_MODE | 参数化 uninstall_mode（clean/卸载双入口） | 无行为变更 | 显式参数替代隐式环境变量 |
| 系统级 LaunchAgents/Daemons、Receipts、共享兄弟守卫 | find_app_system_files / find_app_receipt_files | 未实现 | **暂缓（6d）** | 涉 sudo 读取与删除，逐行复核后移植 |

### 编译/移植问题

1. 6b 残留模板三处漏 `/{id}`（HTTPStorages/Application Scripts/Autosave Information/WebContent），导致裸系统目录被列为残留——真机冒烟捕获并修复（这正是 dry-run 冒烟的价值）。
2. 卸载模式下残留被 clean 语义误拦（DATA_PROTECTED 的 clash-verge 模式）——冒烟暴露，按原实现 MOLE_UNINSTALL_MODE 语义修复并加回归测试。

### 测试

- 122 个测试通过（新增：通用词拒绝、vendor/product 段提取含 com 边界、变体前缀五形态、base_name 剥离含多词后缀与大小写敏感、bundle ID 边界、独立 CLI dotdir、常用目录根、Zed 特例、卸载模式保护语义）。
- 真机 dry-run：Clash Verge 本体 + 5 个真实残留（Application Support/WebKit/Application Scripts/Preferences/LaunchAgents）正确发现，无裸目录。

---

<a name="optimize-7c"></a>
## optimize 优化维护（模块7第三片：sqlite_vacuum + quarantine_cleanup）

### 对标记录

- `opt_sqlite_vacuum` 1:1：pgrep -x 三态探针（Mail/Safari/Messages；0=运行中→Skipped，1=未运行，其他→Failed）→ sqlite3 可用性 → 四个目标 glob（Mail/V*/MailData/Envelope Index*、Messages/chat.db、Safari/History.db、TopSites.db；跳过 -wal/-shm）→ 保护检查 → SQLite 魔数（对标 `file -b`）→ 100MB 上限（#1367）→ PRAGMA freelist <5% 视为已压缩 → integrity_check == ok → VACUUM（dry-run 计数，超时单独记账）。
- `opt_quarantine_cleanup` 1:1：sqlite3 可用性 → 库存在 → 保护检查 → COUNT(*)==0 → Unchanged → DELETE + VACUUM（dry-run 计 Applied）。

### 变更前后对照

| 项 | 原实现（bash） | Rust 实现 | 是否变更 | 原因 |
|----|------------|----------|---------|------|
| SQLite 类型判定 | `file -b` 子进程（*SQLite* 子串） | 魔数头 16 字节直读 | 无行为变更 | SQLite 文件头固定，避免每库 fork |
| 超时记账 | timed_out 独立计数 | 相同（VACUUM 超时 → Attention） | 无行为变更 | — |
| quarantine 保护分支 | 文案 "already clean" | 文案 "数据库受保护"（同 Unchanged） | 仅文案 | 行为一致：该文件名命中 com.apple.* 保护，原实现同样命中 |
| 结果映射 | 原六态 | 同六态（vacuumed→Applied / 超时→Attention / 上限→Skipped） | 无行为变更 | — |

### 测试

- 124 个测试通过（新增：SQLite 魔数检测矩阵、pgrep 探针语义）。
- 真机 dry-run 冒烟：sqlite_vacuum（本机无 Mail/Safari/Messages 库→Unchanged）、quarantine_cleanup（保护分支→Unchanged）均与原语义一致。

---

<a name="optimize-7d"></a>
## optimize 优化维护（模块7第四片：launch_agents_cleanup）

### 对标记录

- `opt_launch_agents_cleanup` 1:1：`~/Library/LaunchAgents/*.plist` 逐项解析程序路径（ProgramArguments 首元素 → 回退 Program，对标 PlistBuddy 两连探针）；仅"绝对路径 + 真实缺失 + 卷可达"计为损坏（裸名走 PATH、拔盘卷不算坏，对标 launch_agent_volume_mounted）；损坏项尽力 `launchctl unload` 后删除。
- 加固差异：原实现 safe_remove 永久删除 → Trash 可恢复；原实现 dry-run 时仍执行 unload（疑似缺陷）→ Rust 侧 dry-run 完全跳过副作用（记入差异）。

### 变更前后对照

| 项 | 原实现（bash） | Rust 实现 | 是否变更 | 原因 |
|----|------------|----------|---------|------|
| plist 读取 | PlistBuddy 子进程 ×2 | plist crate 直读（ProgramArguments → Program 回退） | 无行为变更 | 免每文件 fork |
| dry-run 语义 | unload 无 dry-run 分支 | 完全跳过 | **加固** | dry-run 不应有系统副作用 |
| 删除 | safe_remove 永久 | delete_to_trash（回收站） | **加固** | 与其余优化任务一致 |

### 测试

- 126 个测试通过（新增：卷可达语义、损坏判定五例——缺失路径/健康路径/裸名/拔盘卷/Program 回退）。真机冒烟：本机 LaunchAgents 全部健康 → Unchanged。

---

<a name="optimize-7e"></a>
## optimize 优化维护（模块7第五片：coreduet_cleanup）

### 对标记录

- `opt_coreduet_cleanup` 1:1：knowledgeC.db 缺失 → Unchanged；db+wal+shm 合计 <100MB → Unchanged（健康）；dry-run → Applied；真实：sqlite3 不可用 → Unavailable；删除 wal/shm（SQLite 自动重建）→ `DELETE FROM ZOBJECT WHERE ZCREATIONDATE < (now-90d) - 2001-01-01`（CoreTime 纪元换算）→ VACUUM。
- 加固差异：WAL/SHM 原 safe_remove 永久 → Trash 可恢复。

### 变更前后对照

| 项 | 原实现（bash） | Rust 实现 | 是否变更 | 原因 |
|----|------------|----------|---------|------|
| 大小合计 | du -skcP 多文件 | metadata 逐文件求和 | 无行为变更 | 语义一致 |
| WAL/SHM 删除 | safe_remove 永久 | delete_to_trash | **加固** | 与其余优化任务一致 |
| DELETE+VACUUM | sqlite3 子进程 | 相同（run_sqlite 超时 30s） | 无行为变更 | — |
| 有界性 | 90 天记录删除（非整表） | 相同 | 无行为变更 | 不触碰近期使用数据 |

### 测试

- 126 个测试通过；真机 dry-run：本机 Knowledge 库 3.5MB → 健康 Unchanged（与原阈值逻辑一致）。

---

<a name="optimize-7f"></a>
## optimize 优化维护（模块7第六片：notification_cleanup）

### 对标记录

- `resolve_notification_center_db` 1:1：Group Containers/group.com.apple.usernoted/db2/db → getconf DARWIN_USER_DIR 回退；两者皆无 → **Unavailable**（#1368：路径缺失 ≠ 健康空状态）。
- `opt_notification_cleanup` 1:1：>50MB 阈值 → dry-run Applied → sqlite3 删除 30 天前投递记录 + VACUUM → 成功 killall NotificationCenter（尽力刷新）→ Applied；busy/locked → Failed。

### 变更前后对照

| 项 | 原实现（bash） | Rust 实现 | 是否变更 | 原因 |
|----|------------|----------|---------|------|
| 路径解析 | getconf 子进程 | 相同 | 无行为变更 | — |
| 有界性 | 30 天投递记录（非整表） | 相同 | 无行为变更 | 不触碰近期通知 |

### 测试

- 126 个测试通过；真机 dry-run：本机通知库（Group Containers 路径）424 KB → 健康 Unchanged（与原阈值逻辑一致）。
- optimize 已移植 **9/21** 处理器。
