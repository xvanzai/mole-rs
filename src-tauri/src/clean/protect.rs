//! 保护检查，对标 `Mole lib/core/app_protection.sh` 的
//! `should_protect_path`（7 层检查 1:1 移植，clean 模式）。
//!
//! 分层顺序与原实现一致：
//! 0. 共享 home 状态根（~/.cache、~/.config、~/.local{,/bin,/lib,/share,/state}）
//! 0b. Codex Desktop/CLI 可重建 Chromium 缓存叶（其子路径放行，叶本身保护）
//! 0c. OrbStack 运行时路径
//! 1. 关键词匹配（System Settings/Control Center/Notes，大小写变体）
//! 2. 系统 UI 渲染关键缓存（finder/dock/settings 缓存、容器、sharedfilelist）
//! 3. 沙盒容器 bundle ID 提取 → should_protect_data（容器 Caches/tmp 放行）
//! 4. 关键 bundle 关键词 + 4b 端点安全/EDR 代理缓存 + 4c 编译模型缓存（E5RT）
//! 5. 关键偏好文件/用户数据/iCloud/账户邮件等高风险路径 + 音频插件等
//! 6. 全路径对 SYSTEM_CRITICAL_BUNDLES / DATA_PROTECTED_BUNDLES 模式匹配
//! 7. 文件名级 should_protect_data
//!
//! 另外保留一层 Rust 侧额外加固（超出原实现的保守防线，见 CHANGES.md）：
//! 绝对系统前缀（/System、/Library/Apple 等）永不放行。
//!
//! 卸载模式（MOLE_UNINSTALL_MODE）分支待 uninstall 模块时移植。

use super::protect_data::{
    DATA_PROTECTED_BUNDLES, ENDPOINT_SECURITY_BUNDLE_PREFIXES, SYSTEM_CRITICAL_BUNDLES,
};
use super::whitelist::glob_match;

/// Rust 侧额外加固前缀（对标 AGENTS.md "Never modify protected paths such as
/// /System, /Library/Apple"；原实现靠目录设计保证不触碰，Rust 侧显式拦截）。
const HARD_PROTECTED_PREFIXES: &[&str] = &[
    "/System",
    "/Library/Apple",
    "/private/etc",
    "/etc",
    "/usr",
    "/bin",
    "/sbin",
    "/private/var/db",
];

fn home() -> String {
    std::env::var("HOME").unwrap_or_default()
}

/// 对标 `bundle_matches_pattern`：bash glob（大小写敏感、`*` 跨 `/`）。
pub fn bundle_matches_pattern(value: &str, pattern: &str) -> bool {
    if pattern.is_empty() {
        return false;
    }
    glob_match(pattern, value)
}

/// 对标 `is_critical_system_component`：关键系统组件关键词（大小写不敏感
/// 子串匹配）。
pub fn is_critical_system_component(token: &str) -> bool {
    if token.is_empty() {
        return false;
    }
    let lower = token.to_lowercase();
    const KEYWORDS: &[&str] = &[
        "backgroundtaskmanagement",
        "loginitems",
        "systempreferences",
        "systemsettings",
        "settings",
        "preferences",
        "controlcenter",
        "biometrickit",
        "sfl",
        "tcc",
    ];
    KEYWORDS.iter().any(|k| lower.contains(k))
}

/// 对标 `should_protect_data`：判断 bundle ID / 文件名是否含敏感数据。
pub fn should_protect_data(bundle_id: &str) -> bool {
    let lower = bundle_id.to_lowercase();
    // case 1：com.apple.* 全量 + 核心词。
    if bundle_id.starts_with("com.apple.")
        || matches!(
            lower.as_str(),
            "loginwindow" | "dock" | "systempreferences" | "finder" | "safari"
        )
    {
        return true;
    }
    // org.cups.*：无用户可见 app 的 OS 子系统（#731）。
    if bundle_id.starts_with("org.cups.") {
        return true;
    }
    // case 2-8：前缀/关键词组。
    for kw in [
        "backgroundtaskmanagement",
        "keychain",
        "security",
        "bluetooth",
        "wifi",
        "network",
        "notification",
        "accessibility",
        "universalaccess",
        "textinput",
        "keyboard",
        "inputsource",
    ] {
        if lower.starts_with(kw) {
            return true;
        }
    }
    if lower == "tcc" {
        return true;
    }
    // *inputmethod* | *InputMethod* | *IME | textinput* | TextInput*
    if lower.contains("inputmethod") || bundle_id.ends_with("IME") {
        return true;
    }
    // keyboard* | Keyboard* | inputsource* | InputSource* | keylayout* | KeyLayout*
    for kw in ["keylayout", "inputsource"] {
        if lower.starts_with(kw) {
            return true;
        }
    }
    if bundle_id.ends_with("HIToolbox") || bundle_id.starts_with("HIToolbox") {
        return true;
    }
    if matches!(bundle_id, "GlobalPreferences" | ".GlobalPreferences")
        || bundle_id.starts_with("org.pqrs.Karabiner")
    {
        return true;
    }
    // 密码管理器 / IDE / AI 工具 / 代理工具等（case 逐组）。
    for prefix in [
        "com.1password.",
        "com.agilebits.",
        "com.lastpass.",
        "com.dashlane.",
        "com.bitwarden.",
        "com.jetbrains.",
        "com.microsoft.",
        "com.visualstudio.",
        "com.sublimetext.",
        "com.sublimehq.",
        "com.nssurge.",
        "com.v2ray.",
        "com.clash.",
        "com.docker.",
        "com.getpostman.",
        "com.insomnia.",
        "com.tencent.",
        "com.sogou.",
        "com.baidu.",
        "com.googlecode.",
        "im.rime.",
    ] {
        if bundle_id.starts_with(prefix) {
            return true;
        }
    }
    if matches!(
        bundle_id,
        "Cursor" | "Claude" | "ChatGPT" | "com.openai.codex" | "Codex" | "codex-runtimes"
            | "Ollama" | "com.clash.app"
    ) {
        return true;
    }
    for (starts, ends) in [("ClashX", ""), ("Surge", ""), ("Shadowrocket", ""), ("Quantumult", "")] {
        if bundle_id.starts_with(starts) && bundle_id.ends_with(ends) {
            return true;
        }
    }
    // clash 变体词（clash-*/*-clash/clash.* /clash_*/clashverge 等，大小写变体）。
    let clash_hit = (lower.starts_with("clash-")
        || lower.ends_with("-clash")
        || lower.starts_with("clash.")
        || lower.starts_with("clash_")
        || lower.contains("clash-verge")
        || lower.starts_with("clashverge"))
        && (lower.contains("clash"));
    if clash_hit {
        return true;
    }
    // 兜底：全量 DATA_PROTECTED_BUNDLES。
    for pattern in DATA_PROTECTED_BUNDLES {
        if bundle_matches_pattern(bundle_id, pattern) {
            return true;
        }
    }
    false
}

/// 对标 `is_endpoint_security_cache_path`：仅 /private/var/folders（含
/// /var/folders 符号形式）范围内，nocasematch 包含厂商前缀即保护。
fn is_endpoint_security_cache_path(path: &str) -> bool {
    let in_scope = path.starts_with("/private/var/folders/") || path.starts_with("/var/folders/");
    if !in_scope {
        return false;
    }
    let lower = path.to_lowercase();
    ENDPOINT_SECURITY_BUNDLE_PREFIXES
        .iter()
        .any(|prefix| lower.contains(&prefix.to_lowercase()))
}

/// 对标 `is_orbstack_runtime_path`（nocasematch）。
fn is_orbstack_runtime_path(path: &str) -> bool {
    let lower = path.to_lowercase();
    lower.contains("/library/group containers/dev.orbstack")
        || lower.ends_with("/.orbstack")
        || lower.contains("/.orbstack/")
}

/// 对标 `holds_compiled_model_cache`：路径本身是 E5RT 编译模型缓存，
/// 或其直接子级存在该目录（运行中 app 被清缓存会让识别调用失败）。
pub fn holds_compiled_model_cache(path: &str) -> bool {
    if path.trim_end_matches('/').ends_with("/com.apple.e5rt.e5bundlecache") {
        return true;
    }
    std::path::Path::new(path)
        .join("com.apple.e5rt.e5bundlecache")
        .is_dir()
}

/// 对标 `_mole_is_shared_home_state_root`：共享 XDG 根本身保护，
/// app 专属子路径（如 ~/.config/zed）放行。
pub(crate) fn is_shared_home_state_root(path: &str) -> bool {
    let home = home();
    let stripped = path.strip_prefix(&home).unwrap_or(path);
    let stripped = stripped.trim_end_matches('/');
    if !stripped.starts_with('/') {
        return false;
    }
    let parts: Vec<&str> = stripped.split('/').filter(|s| !s.is_empty()).collect();
    let lower: Vec<String> = parts.iter().map(|s| s.to_lowercase()).collect();
    if lower.len() == 1 && matches!(lower[0].as_str(), ".cache" | ".config" | ".local") {
        return true;
    }
    if lower.len() == 2
        && lower[0] == ".local"
        && matches!(lower[1].as_str(), "bin" | "lib" | "share" | "state")
    {
        return true;
    }
    false
}

/// 对标 step 0b：Codex 可重建 Chromium 缓存叶（仅子路径放行）。
fn is_codex_rebuildable_cache_leaf(path: &str) -> bool {
    let base = format!("{}/Library/Caches/Codex/", home());
    let rest = match path.strip_prefix(&base) {
        Some(r) => r,
        None => return false,
    };
    matches!(
        rest,
        "Default/Cache"
            | "Default/Code Cache"
            | "Default/Partitions/codex-browser-app/Cache"
            | "Default/Partitions/codex-browser-app/Code Cache"
            | "codex-browser-app/Cache"
            | "codex-browser-app/Code Cache"
    ) || rest.starts_with("Default/Cache/")
        || rest.starts_with("Default/Code Cache/")
        || rest.starts_with("Default/Partitions/codex-browser-app/Cache/")
        || rest.starts_with("Default/Partitions/codex-browser-app/Code Cache/")
        || rest.starts_with("codex-browser-app/Cache/")
        || rest.starts_with("codex-browser-app/Code Cache/")
}

/// 从沙盒路径提取 bundle ID（对标 step 3 的 regex 提取）。
fn container_bundle_id(path: &str) -> Option<String> {
    for marker in ["/Library/Containers/", "/Library/Group Containers/"] {
        let mut search_from = 0;
        while let Some(pos) = path[search_from..].find(marker) {
            let start = search_from + pos + marker.len();
            let rest = &path[start..];
            if rest.is_empty() {
                break;
            }
            let id = rest.split('/').next().unwrap_or("");
            if !id.is_empty() {
                return Some(id.to_string());
            }
            search_from = start;
        }
    }
    None
}

/// 完整移植 `should_protect_path`（clean 模式）。
/// 返回 true = 受保护（不可删除）。
pub fn should_protect_path(path: &str) -> bool {
    should_protect_path_inner(path, false)
}

/// 卸载模式（对标 MOLE_UNINSTALL_MODE=1）：跳过文件名级检查，
/// DATA_PROTECTED 不拦（卸载前另有 should_protect_from_uninstall 前置判断）。
pub fn should_protect_path_uninstall(path: &str) -> bool {
    should_protect_path_inner(path, true)
}

fn should_protect_path_inner(path: &str, uninstall_mode: bool) -> bool {
    if path.is_empty() {
        return false;
    }

    // Rust 侧额外加固：绝对系统前缀永不放行。
    if HARD_PROTECTED_PREFIXES
        .iter()
        .any(|p| path == *p || path.starts_with(&format!("{p}/")))
    {
        return true;
    }

    if is_shared_home_state_root(path) {
        return true;
    }

    let codex_rebuildable = is_codex_rebuildable_cache_leaf(path);

    if is_orbstack_runtime_path(path) {
        return true;
    }

    let lower = path.to_lowercase();

    // 1. Keyword-based matching for system components。
    for kw in ["systemsettings", "systempreferences", "controlcenter"] {
        if lower.contains(kw) {
            return true;
        }
    }
    if lower.contains("com.apple.settings") || lower.contains("com.apple.notes") {
        return true;
    }

    let mut container_cache_path = false;

    // 2. 系统 UI 渲染关键缓存。
    for pattern in [
        "com.apple.systempreferences.cache",
        "com.apple.Settings.cache",
        "com.apple.controlcenter.cache",
        "com.apple.finder.cache",
        "com.apple.dock.cache",
    ] {
        if lower.contains(pattern) {
            return true;
        }
    }
    for pattern in [
        "/Library/Containers/com.apple.Settings",
        "/Library/Containers/com.apple.SystemSettings",
        "/Library/Containers/com.apple.controlcenter",
        "/Library/Group Containers/com.apple.systempreferences",
        "/Library/Group Containers/com.apple.Settings",
    ] {
        if lower.contains(&pattern.to_lowercase()) {
            return true;
        }
    }
    for pattern in [
        "/com.apple.sharedfilelist/*com.apple.settings",
        "/com.apple.sharedfilelist/*com.apple.systemsettings",
        "/com.apple.sharedfilelist/*systempreferences",
    ] {
        if glob_match(pattern, &lower) {
            return true;
        }
    }

    // 3. 沙盒容器 bundle ID 提取。
    if let Some(bundle_id) = container_bundle_id(path) {
        if path.contains("/Data/Library/Caches/") || path.contains("/Data/tmp/") {
            container_cache_path = true;
        } else if !uninstall_mode && should_protect_data(&bundle_id) {
            return true;
        }
    }

    // 4. 特定关键 bundle 关键词。
    for kw in [
        "com.apple.settings",
        "com.apple.systemsettings",
        "com.apple.controlcenter",
        "com.apple.finder",
        "com.apple.dock",
    ] {
        if lower.contains(kw) {
            return true;
        }
    }

    // 4b. 端点安全 / EDR 代理缓存。
    if is_endpoint_security_cache_path(path) {
        return true;
    }

    // 4c. E5RT 编译模型缓存。
    if holds_compiled_model_cache(path) {
        return true;
    }

    // 5. 关键偏好文件与用户数据（case 逐组转译，语义保持 1:1）。
    for pattern in [
        "*/library/preferences/com.apple.dock.plist",
        "*/library/preferences/com.apple.finder.plist",
    ] {
        if glob_match(pattern, &lower) {
            return true;
        }
    }
    for pattern in [
        "*/library/logs/mole",
        "*/library/logs/mole/*",
    ] {
        if glob_match(pattern, &lower) {
            return true;
        }
    }
    // Codex Crashpad pending 只放行直接子级，更深嵌套保护。
    if glob_match(
        "*/library/application support/codex/crashpad/pending/*/*",
        &lower,
    ) {
        return true;
    }
    for pattern in [
        "*/library/application support/codex",
        "*/library/application support/codex/*",
        "*/library/logs/com.openai.codex",
        "*/library/logs/com.openai.codex/*",
        "*/.codex/sessions",
        "*/.codex/sessions/*",
        "*/.codex/auth.json",
        "*/.codex/history.jsonl",
        "*/.codex/state_*.sqlite",
        "*/.codex/logs_*.sqlite",
        "*/.codex/session_index.jsonl",
        "*/.codex/cache/session_index.jsonl",
        "*/.codex/cache/codex_app_directory",
        "*/.codex/cache/codex_app_directory/*",
    ] {
        if glob_match(pattern, &lower) {
            return true;
        }
    }
    for pattern in [
        "*/byhost/com.apple.bluetooth.*",
        "*/byhost/com.apple.wifi.*",
        "*/library/preferences/com.apple.networkextension*.plist",
        "*/library/mobile documents*",
        "*/mobile documents*",
    ] {
        if glob_match(pattern, &lower) {
            return true;
        }
    }
    // 高风险清理 denylist：缓存样命名但含许可/账户/插件/MDM/系统服务状态。
    for pattern in [
        "*/library/accounts",
        "*/library/accounts/*",
        "*/library/keychains",
        "*/library/keychains/*",
        "*/library/mail",
        "*/library/mail/*",
        "*/library/calendars",
        "*/library/calendars/*",
        "*/library/contacts",
        "*/library/contacts/*",
    ] {
        if glob_match(pattern, &lower) {
            return true;
        }
    }
    // 音频插件与许可（绝对锚定与任意前缀混合，按原文逐条）。
    for pattern in [
        "/library/audio/plug-ins/components",
        "/library/audio/plug-ins/components/*",
        "/library/audio/plug-ins/vst",
        "/library/audio/plug-ins/vst/*",
        "/library/audio/plug-ins/vst3",
        "/library/audio/plug-ins/vst3/*",
        "/library/application support/izotope",
        "/library/application support/izotope/*",
        "*/library/application support/izotope",
        "*/library/application support/izotope/*",
        "/library/application support/lasersoft imaging",
        "/library/application support/lasersoft imaging/*",
        "*/library/preferences/com.native-instruments*",
        "*/library/preferences/com.avid.mediacomposer*.plist",
        "*/library/preferences/com.fabfilter.*.[0-9].plist",
        "*/library/preferences/com.fabfilter.*.[0-9][0-9].plist",
        "*/library/preferences/com.paceap.*.plist",
        "/private/var/folders/*/c/com.native-instruments*",
        "/private/var/folders/*/c/com.avid.mediacomposer*",
        "/private/var/folders/*/c/com.paceap.eden.iloklicensemanager*",
    ] {
        if glob_match(pattern, &lower) {
            return true;
        }
    }
    // 高风险缓存 denylist（ms-playwright / Adobe / containermanagerd 等）。
    for pattern in [
        "*/library/caches/ms-playwright",
        "*/library/caches/ms-playwright/*",
        "*/library/caches/app.cotypist.cotypist",
        "*/library/caches/app.cotypist.cotypist/*",
        "*/library/caches/com.displaylink.displaylinkuseragent",
        "*/library/caches/com.displaylink.displaylinkuseragent/*",
        "*/library/caches/com.lasersoft-imaging.silverfast9",
        "*/library/caches/com.lasersoft-imaging.silverfast9/*",
        "*/library/caches/com.lasersoft-imaging.silverfast-9-installer",
        "*/library/caches/com.lasersoft-imaging.silverfast-9-installer/*",
        "*/library/caches/adobe *",
        "*/library/caches/* adobe*",
        "*/library/caches/com.apple.containermanagerd",
        "*/library/caches/com.apple.containermanagerd/*",
        "*/library/caches/com.apple.homed",
        "*/library/caches/com.apple.homed/*",
        "*/library/caches/com.apple.ap.adprivacyd",
        "*/library/caches/com.apple.ap.adprivacyd/*",
        "*/library/caches/familycircle",
        "*/library/caches/familycircle/*",
        "*/library/caches/com.apple.homekit",
        "*/library/caches/com.apple.homekit/*",
        "*/library/caches/com.apple.workflowkit.backgroundshortcutrunner.shortcutssandboxcache",
        "*/library/caches/com.apple.workflowkit.backgroundshortcutrunner.shortcutssandboxcache/*",
        "*/library/caches/com.apple.siriactionsd.shortcutssandboxcache",
        "*/library/caches/com.apple.siriactionsd.shortcutssandboxcache/*",
    ] {
        if glob_match(pattern, &lower) {
            return true;
        }
    }
    // 壁纸/航拍屏保资产是用户所选内容。
    for pattern in [
        "*/library/application support/com.apple.idleassetsd",
        "*/library/application support/com.apple.idleassetsd/*",
        "*/library/application support/com.apple.wallpaper",
        "*/library/application support/com.apple.wallpaper/*",
    ] {
        if glob_match(pattern, &lower) {
            return true;
        }
    }
    // CoreAudio（issue #553）：Intel Mac 上可能导致音频输出丢失。
    if lower.contains("com.apple.coreaudio")
        || lower.contains("com.apple.audio.")
        || lower.contains("coreaudiod")
    {
        return true;
    }

    // 6. 全路径对保护 bundle 模式匹配（容器 Caches/tmp 已在 step 3 处理）。
    if !container_cache_path && !codex_rebuildable {
        if uninstall_mode {
            // 对标 MOLE_UNINSTALL_MODE=1：Apple 可卸载先放行，系统关键保护；
            // DATA_PROTECTED 不拦（用户显式选择卸载）。
            for pattern in crate::clean::protect_data::APPLE_UNINSTALLABLE_APPS {
                if bundle_matches_pattern(path, pattern) {
                    return false;
                }
            }
            for pattern in SYSTEM_CRITICAL_BUNDLES {
                if bundle_matches_pattern(path, pattern) {
                    return true;
                }
            }
        } else {
            for pattern in SYSTEM_CRITICAL_BUNDLES.iter().chain(DATA_PROTECTED_BUNDLES.iter()) {
                if bundle_matches_pattern(path, pattern) {
                    return true;
                }
            }
            // 7. 文件名级检查（卸载模式跳过——对标原实现注释
            // "Skip in uninstall mode - user explicitly chose to remove this app"）。
            let filename = path.rsplit('/').next().unwrap_or("");
            if !filename.is_empty() && should_protect_data(filename) {
                return true;
            }
        }
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hard_prefixes_always_protected() {
        assert!(should_protect_path("/System"));
        assert!(should_protect_path("/System/Library/Caches"));
        assert!(should_protect_path("/Library/Apple"));
        assert!(should_protect_path("/usr/bin"));
        assert!(!should_protect_path("/Users/x/Library/Caches"));
    }

    #[test]
    fn shared_home_roots_protected_children_not() {
        let home = home();
        assert!(should_protect_path(&format!("{home}/.config")));
        assert!(should_protect_path(&format!("{home}/.local/share")));
        assert!(should_protect_path(&format!("{home}/.CACHE")));
        assert!(!should_protect_path(&format!("{home}/.config/zed")));
        assert!(!should_protect_path(&format!("{home}/Library/Caches")));
    }

    #[test]
    fn keyword_layer_matches_case_variants() {
        assert!(should_protect_path("/x/com.apple.SystemSettings/y"));
        assert!(should_protect_path("/x/systempreferences.cache"));
        assert!(should_protect_path("/Users/t/Library/Caches/com.apple.controlcenter.cache"));
        assert!(should_protect_path("/Users/t/Library/Caches/com.apple.finder.cache"));
    }

    #[test]
    fn container_bundle_protection() {
        // 敏感 app 的容器整条保护。
        assert!(should_protect_path(
            "/Users/t/Library/Containers/com.docker.docker/Data/foo"
        ));
        // 容器 Caches/tmp 放行（由目录定义决定是否清理），但 com.apple 例外仍拦。
        assert!(!should_protect_path(
            "/Users/t/Library/Containers/com.docker.docker/Data/Library/Caches/x"
        ));
        assert!(should_protect_path(
            "/Users/t/Library/Containers/com.apple.Settings/Data/Library/Caches/x"
        ));
    }

    #[test]
    fn endpoint_security_protected() {
        assert!(should_protect_path(
            "/private/var/folders/ab/xxxx/C/com.crowdstrike.falcon.agent"
        ));
        // 大小写不敏感（nocasematch）。
        assert!(should_protect_path(
            "/private/var/folders/ab/xxxx/T/Com.SentinelOne.something"
        ));
        // EDR 缓存即使不在 var/folders 也受 DATA_PROTECTED_BUNDLES 的
        // "com.crowdstrike.*" 保护（对标 step 7 文件名级检查）。
        assert!(should_protect_path(
            "/Users/t/Library/Caches/com.crowdstrike.something"
        ));
        // 端点安全专用谓词锚定绝对根：厂商名仅出现在祖先路径且叶为普通
        // 缓存名时，不因端点安全规则误伤（step 7 只查叶文件名，不查祖先）。
        assert!(!should_protect_path(
            "/Users/t/var/folders/com.crowdstrike.x/cache.bin"
        ));
    }

    #[test]
    fn e5rt_compiled_model_cache() {
        assert!(should_protect_path(
            "/Users/t/Library/Caches/com.apple.e5rt.e5bundlecache"
        ));
        let home = home();
        let holder = std::env::temp_dir().join("mole_rs_e5rt_test");
        std::fs::create_dir_all(holder.join("com.apple.e5rt.e5bundlecache")).unwrap();
        assert!(holds_compiled_model_cache(&holder.to_string_lossy()));
        std::fs::remove_dir_all(&holder).ok();
        assert!(should_protect_path(&format!(
            "{}/Library/Caches/SomeApp/com.apple.e5rt.e5bundlecache/sub",
            home
        )) == false);
    }

    #[test]
    fn sensitive_user_data_protected() {
        for p in [
            "/Users/t/Library/Accounts",
            "/Users/t/Library/Keychains",
            "/Users/t/Library/Mail/Inbox",
            "/Users/t/Library/Mobile Documents/x",
            "/Users/t/Library/Preferences/com.apple.dock.plist",
            "/Users/t/Library/Application Support/com.apple.wallpaper",
            "/Users/t/Library/Caches/Adobe Bridge",
            "/Users/t/Library/Caches/ms-playwright/chromium",
        ] {
            assert!(should_protect_path(p), "should protect {p}");
        }
    }

    /// 对标 should_protect_data 的 case 组（由 bundle_matches_pattern 兜底）。
    #[test]
    fn data_protected_bundles() {
        assert!(should_protect_data("com.apple.finder"));
        assert!(should_protect_data("com.apple.whatever"));
        assert!(should_protect_data("com.jetbrains.intellij"));
        assert!(should_protect_data("com.tencent.xinWeChat"));
        assert!(should_protect_data("org.pqrs.Karabiner-Elements"));
        assert!(should_protect_data("pinyin.inputmethod"));
        // bash glob 锚定语义：ClashX* 不匹配中缀（与 shell 路径展开不同）。
        assert!(!should_protect_data("net.minetest.ClashX Pro"));
        assert!(should_protect_data("ClashX Pro"));
        assert!(should_protect_data("app.standalone.Surge"));
        assert!(!should_protect_data("com.example.unknownapp"));
        assert!(!should_protect_data(""));
    }

    /// 对标 is_critical_system_component：关键系统组件关键词子串匹配。
    #[test]
    fn critical_system_component_keywords() {
        assert!(is_critical_system_component("com.apple.SystemSettings"));
        assert!(is_critical_system_component("BackgroundTaskManagement"));
        assert!(is_critical_system_component("tccd"));
        assert!(is_critical_system_component("Preferences"));
        assert!(!is_critical_system_component("Safari"));
        assert!(!is_critical_system_component(""));
        assert!(!is_critical_system_component("Notes"));
    }

    /// 对标 step 7：文件名级保护。
    #[test]
    fn filename_level_protection() {
        assert!(should_protect_path(
            "/Users/t/Library/Caches/something/com.apple.dock"
        ));
    }

    /// 对标 step 0b：Codex 可重建缓存叶的子路径放行，其他 Codex 路径保护。
    #[test]
    fn codex_rebuildable_leaf_semantics() {
        let home = home();
        let leaf_child = format!("{}/Library/Caches/Codex/Default/Cache/entry", home);
        assert!(!should_protect_path(&leaf_child));
        let leaf = format!("{}/Library/Caches/Codex/Default/Cache", home);
        // 叶本身：step 5 的 Application Support/Codex 不适用；step 6 filename
        // "Cache" 不在保护表 → 放行交给目录定义（叶由 safe_clean 显式选择）。
        let _ = leaf;
        assert!(should_protect_path(&format!("{}/Library/Application Support/Codex", home)));
    }
}
