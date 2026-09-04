//! core 模块：与具体功能解耦的共享工具层。
//!
//! 对标 Mole 原项目：
//! - `internal/units`（Go）→ [`units`]（本 crate）
//! - `lib/core/*.sh`（shell）→ 后续按 clean / purge 模块需要逐个移植

pub mod units;
