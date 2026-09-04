# 变更文档（对标 → 移植 → 差异记录）

> 用途：按模块记录「原实现（变更前）→ Rust 实现（变更后）→ 变更原因」。
> 原则：默认 1:1 移植原行为；只有编译问题 / 平台差异 / 终端 TUI 特有逻辑才允许变更，且必须留痕。

## 模块索引

| 模块 | 原代码 | Rust 代码 | 变更记录 |
|------|--------|----------|---------|
| core 工具层 | `internal/units/bytes.go` | `src-tauri/src/core/units.rs` | [§core](#core-工具层) |

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
