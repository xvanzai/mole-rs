use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    // 构建期计算 sizeof(vm_statistics64_data_t)/sizeof(integer_t)，
    // 等价于 gopsutil 经 cgo 在构建期展开 HOST_VM_INFO64_COUNT 的行为：
    // host_statistics64 要求 count 与内核侧结构大小精确匹配。
    // macOS 结构是追加式演进（REV0..REV5），早期字段偏移（free/inactive/
    // purgeable）固定不变，因此只需要总字数跟随 SDK。
    let probe = r#"
#include <mach/vm_statistics.h>
#include <sys/sysctl.h>
#include <stdio.h>
int main(void) {
    printf("VM64_WORDS=%zu\n", sizeof(vm_statistics64_data_t) / sizeof(integer_t));
    printf("KINFO_PROC_BYTES=%zu\n", sizeof(struct kinfo_proc));
    return 0;
}
"#;
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap_or_default());
    let src = out_dir.join("vm_stat64_count.c");
    let bin = out_dir.join("vm_stat64_count");
    fs::write(&src, probe).expect("write vm_stat64_count.c");
    let cc = Command::new("cc")
        .arg("-o")
        .arg(&bin)
        .arg(&src)
        .output()
        .expect("compile vm_stat64_count probe (clang required)");
    if cc.status.success() {
        if let Ok(run) = Command::new(&bin).output() {
            if run.status.success() {
                if let Ok(text) = String::from_utf8(run.stdout) {
                    for line in text.lines() {
                        if let Some((k, v)) = line.split_once('=') {
                            println!("cargo:rustc-env={k}={v}");
                        }
                    }
                }
            }
        }
    } else {
        // 无 cc 时退回 macOS 27 SDK 的当前值；结构追加演进不影响早期字段。
        println!("cargo:rustc-env=VM64_WORDS=44");
        println!("cargo:rustc-env=KINFO_PROC_BYTES=648");
        println!("cargo:warning=cc unavailable, using fallback sizes");
    }

    println!("cargo:rerun-if-changed=build.rs");
    tauri_build::build()
}
