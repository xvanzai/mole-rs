mod clean;
mod commands;
mod core;
mod status;

use core::units;

/// SI（1000 进制）格式化，供前端渲染磁盘容量（对标 units.BytesSI）。
#[tauri::command]
fn format_bytes_si(size: i64) -> String {
    units::bytes_si(size)
}

/// 二进制（1024 进制）格式化，供前端渲染内存/实时计数（对标 units.BytesBin）。
#[tauri::command]
fn format_bytes_bin(v: u64) -> String {
    units::bytes_bin(v)
}

/// 二进制紧凑格式化（无小数、单字母后缀，对标 units.BytesBinShort）。
#[tauri::command]
fn format_bytes_bin_short(v: u64) -> String {
    units::bytes_bin_short(v)
}

/// 二进制紧凑格式化（一位小数、单字母后缀，对标 units.BytesBinCompact）。
#[tauri::command]
fn format_bytes_bin_compact(v: u64) -> String {
    units::bytes_bin_compact(v)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(commands::status::CollectorState::default())
        .invoke_handler(tauri::generate_handler![
            format_bytes_si,
            format_bytes_bin,
            format_bytes_bin_short,
            format_bytes_bin_compact,
            commands::status::status_tick,
            commands::clean::clean_preview
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
