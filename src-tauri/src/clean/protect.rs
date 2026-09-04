//! 保护检查与清理目录，对标 `Mole lib/core/app_protection.sh` 的
//! `should_protect_path` 与 `lib/clean/user.sh` 的 `clean_app_caches` 目录。
//!
//! 3a 版本包含硬性系统路径保护层；`should_protect_path` 的完整
//! bundle ID / 模式匹配数据（app_protection_data.sh，627 行）在 3b
//! 子模块 1:1 移植——在那之前本模块只提供只读预览，不提供删除。

use super::whitelist::Whitelist;

/// 硬性保护路径前缀（对标 AGENTS.md "Never modify protected paths such as
/// /System, /Library/Apple"，以及 should_protect_path 的系统级前缀）。
const PROTECTED_PREFIXES: &[&str] = &[
    "/System",
    "/Library/Apple",
    "/private/etc",
    "/etc",
    "/usr",
    "/bin",
    "/sbin",
    "/private/var/db",
];

/// 返回跳过原因（Some = 不可清理），对标 `_safe_clean_impl` 的逐路径
/// 检查顺序：保护路径 → 白名单。
pub fn skip_reason(path: &str, whitelist: &Whitelist) -> Option<&'static str> {
    if is_protected_path(path) {
        return Some("protected");
    }
    if whitelist.is_whitelisted(path) {
        return Some("whitelist");
    }
    None
}

fn is_protected_path(path: &str) -> bool {
    PROTECTED_PREFIXES
        .iter()
        .any(|p| path == *p || path.starts_with(&format!("{p}/")))
}

/// 一个清理目录条目，对标一行 `safe_clean ~/Library/... "描述"`。
pub struct CatalogEntry {
    /// 相对主目录的路径模式（`~/` 前缀）。
    pub path: &'static str,
    pub description: &'static str,
}

/// Apple 用户缓存族，1:1 移植自 `clean_app_caches` 的显式
/// `safe_clean` 行（含原项目的注释性排除项，见下方测试锁定）。
pub fn apple_user_cache_catalog() -> Vec<CatalogEntry> {
    vec![
        CatalogEntry {
            path: "~/Library/Saved Application State/*",
            description: "Saved application states",
        },
        CatalogEntry {
            path: "~/Library/Caches/com.apple.photoanalysisd",
            description: "Photo analysis cache",
        },
        CatalogEntry {
            path: "~/Library/Caches/com.apple.akd",
            description: "Apple ID cache",
        },
        CatalogEntry {
            path: "~/Library/Caches/com.apple.WebKit.Networking/*",
            description: "WebKit network cache",
        },
        CatalogEntry {
            path: "~/Library/DiagnosticReports/*",
            description: "Diagnostic reports",
        },
        CatalogEntry {
            path: "~/Library/Caches/com.apple.QuickLook.thumbnailcache",
            description: "QuickLook thumbnails",
        },
        CatalogEntry {
            path: "~/Library/Caches/Quick Look/*",
            description: "QuickLook cache",
        },
        CatalogEntry {
            path: "~/Library/Caches/com.apple.iconservices*",
            description: "Icon services cache",
        },
        CatalogEntry {
            path: "~/Library/IdentityCaches/*",
            description: "Identity caches",
        },
        CatalogEntry {
            path: "~/Library/Suggestions/*",
            description: "Siri suggestions cache",
        },
        CatalogEntry {
            path: "~/Library/Application Support/AddressBook/Sources/*/Photos.cache",
            description: "Address Book photo cache",
        },
        // 沙盒应用缓存（Sandboxed app caches）。
        CatalogEntry {
            path: "~/Library/Containers/com.apple.wallpaper.agent/Data/Library/Caches/*",
            description: "Wallpaper agent cache",
        },
        CatalogEntry {
            path: "~/Library/Containers/com.apple.mediaanalysisd/Data/Library/Caches/*",
            description: "Media analysis cache",
        },
        CatalogEntry {
            path: "~/Library/Containers/com.apple.mediaanalysisd/Data/tmp/*",
            description: "Media analysis temp files",
        },
        CatalogEntry {
            path: "~/Library/Containers/com.apple.AppStore/Data/Library/Caches/*",
            description: "App Store cache",
        },
        CatalogEntry {
            path: "~/Library/Containers/com.apple.configurator.xpc.InternetService/Data/tmp/*",
            description: "Apple Configurator temp files",
        },
        CatalogEntry {
            path: "~/Library/Containers/com.apple.wallpaper.extension.aerials/Data/tmp/*",
            description: "Wallpaper aerials temp files",
        },
        CatalogEntry {
            path: "~/Library/Containers/com.apple.geod/Data/tmp/*",
            description: "Geod temp files",
        },
        CatalogEntry {
            path: "~/Library/Containers/com.apple.stocks/Data/Library/Caches/*",
            description: "Stocks cache",
        },
        CatalogEntry {
            path: "~/Library/Caches/com.apple.helpd/*",
            description: "macOS Help system cache",
        },
        CatalogEntry {
            path: "~/Library/Caches/GeoServices/*",
            description: "Maps geo tile cache",
        },
        CatalogEntry {
            path: "~/Library/Containers/com.apple.AvatarUI.AvatarPickerMemojiPicker/Data/Library/Caches/*",
            description: "Memoji picker cache",
        },
        CatalogEntry {
            path: "~/Library/Containers/com.apple.AMPArtworkAgent/Data/Library/Caches/*",
            description: "Music album art cache",
        },
        CatalogEntry {
            path: "~/Library/Containers/com.apple.CoreDevice.CoreDeviceService/Data/Library/Caches/*",
            description: "CoreDevice service cache",
        },
        CatalogEntry {
            path: "~/Library/Containers/com.apple.NeptuneOneExtension/Data/Library/Caches/*",
            description: "Apple Intelligence extension cache",
        },
        CatalogEntry {
            path: "~/Library/Containers/com.apple.AppleMediaServicesUI.UtilityExtension/Data/tmp/*",
            description: "Apple Media Services temp files",
        },
        CatalogEntry {
            path: "~/Library/Caches/com.apple.AppleMediaServices/*",
            description: "Apple Media Services cache",
        },
        CatalogEntry {
            path: "~/Library/Caches/com.apple.duetexpertd/*",
            description: "Duet Expert cache",
        },
        CatalogEntry {
            path: "~/Library/Caches/com.apple.parsecd/*",
            description: "Parsecd cache",
        },
        CatalogEntry {
            path: "~/Library/Caches/com.apple.python/*",
            description: "Apple Python cache",
        },
    ]
}

/// 原项目显式排除项（以测试锁定，防止后续误加入目录）：
/// - `~/Library/Autosave Information`：可能包含可恢复的用户文档；
/// - `~/Library/Calendars/Calendar Cache*`：CalendarAgent 持有 SQLite 索引，
///   运行中删除会令 Calendar.app 崩溃至注销/重启（#1508）；
/// - `~/Library/Application Support/com.apple.wallpaper/aerials/thumbnails`：
///   ~50KB 的壁纸封面预览，删除几乎不省空间却让设置页全变占位图（#1118）；
/// - E5RT bundle cache：`holds_compiled_model_cache()` 保护，运行中的
///   daemon 被清缓存后识别失效。
#[cfg(test)]
mod tests {
    use super::*;

    /// 对标：排除项不进入目录。
    #[test]
    fn excluded_targets_are_not_in_catalog() {
        let catalog: Vec<&str> = apple_user_cache_catalog()
            .iter()
            .map(|e| e.path)
            .collect();
        assert!(!catalog
            .iter()
            .any(|p| p.contains("Autosave Information")));
        assert!(!catalog.iter().any(|p| p.contains("Calendar Cache")));
        assert!(!catalog
            .iter()
            .any(|p| p.contains("com.apple.wallpaper/aerials/thumbnails")));
        assert!(!catalog.iter().any(|p| p.contains("E5RT")));
    }

    /// 对标：受保护路径返回 skip。
    #[test]
    fn protected_paths_are_skipped() {
        let wl = Whitelist {
            patterns: vec![],
            source: "default",
        };
        assert_eq!(skip_reason("/System/Library", &wl), Some("protected"));
        assert_eq!(skip_reason("/Library/Apple", &wl), Some("protected"));
        assert_eq!(skip_reason("/etc/hosts", &wl), Some("protected"));
        assert_eq!(skip_reason("/usr/bin/true", &wl), Some("protected"));
        assert_eq!(skip_reason("/Users/x/Library/Caches", &wl), None);
    }
}
