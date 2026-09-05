//! 清理目录表，对标 `Mole lib/clean/user.sh` 的 `clean_app_caches`
//! 显式 `safe_clean` 行（1:1，含原项目的注释性排除项）。

/// 一个清理目录条目，对标一行 `safe_clean ~/Library/... "描述"`。
pub struct CatalogEntry {
    /// 相对主目录的路径模式（`~/` 前缀）。
    pub path: &'static str,
    pub description: &'static str,
}

/// Apple 用户缓存族。
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

#[cfg(test)]
mod tests {
    use super::*;

    /// 对标原项目排除项（以测试锁定，防止回归）：
    /// - Autosave Information：可恢复用户文档；
    /// - Calendar Cache：CalendarAgent 持有索引（#1508）；
    /// - 壁纸封面缩略图（#1118）；
    /// - E5RT bundle cache：holds_compiled_model_cache 保护。
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
}
