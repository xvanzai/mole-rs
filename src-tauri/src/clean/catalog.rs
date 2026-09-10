//! 清理目录表，对标 `Mole lib/clean/*.sh` 的显式 `safe_clean` 行（1:1，
//! 含原项目的注释性排除项）。
//!
//! 3a：Apple 用户缓存族（user.sh clean_app_caches）；
//! 3c：开发工具链族（dev.sh 的普通 safe_clean 行）。
//!
//! 需要环境探测/进程守卫的行（npm/bun/corepack/uv/mise/pip 缓存目录解析、
//! cargo registry 的 owner-process guard）不在本片，留待后续子片 1:1 移植。

/// 一个清理目录条目，对标一行 `safe_clean <path> "描述"`。
pub struct CatalogEntry {
    /// 所属清理族（对标 dev.sh 的 clean_dev_* 分组函数）。
    pub family: &'static str,
    /// 相对主目录的路径模式（`~/` 前缀）。
    pub path: &'static str,
    /// 可选的环境变量基址（对标 `resolve_tool_home "${ENV:-}" default`）：
    /// 环境变量存在且为绝对路径时使用之，否则用 `~/` 下的 path。
    pub home_env: Option<&'static str>,
    pub description: &'static str,
}

/// 清理族元信息（id → 展示名）。
pub fn family_label(family: &str) -> &'static str {
    match family {
        "apple_user" => "Apple 系统缓存",
        "dev_frontend" => "前端构建缓存",
        "dev_python" => "Python 工具链",
        "dev_rust" => "Rust 工具链",
        "dev_ruby" => "Ruby 工具链",
        "dev_perl" => "Perl 工具链",
        "dev_cloud" => "云 CLI 与容器",
        "dev_ci" => "CI 与 DevOps",
        "browser" => "浏览器缓存",
        "apple_silicon" => "Apple Silicon 更新",
        "virtualization" => "虚拟化工具",
        "app_support" => "Application Support",
        "cloud_office" => "云与 Office",
        "user_essentials" => "用户基础",
        "service_worker" => "Service Worker",
        _ => "其他",
    }
}

/// Apple 用户缓存族，对标 `clean_app_caches` 的显式 `safe_clean` 行。
pub fn apple_user_cache_catalog() -> Vec<CatalogEntry> {
    let rows: &[(&str, &str)] = &[
        ("~/Library/Saved Application State/*", "Saved application states"),
        ("~/Library/Caches/com.apple.photoanalysisd", "Photo analysis cache"),
        ("~/Library/Caches/com.apple.akd", "Apple ID cache"),
        ("~/Library/Caches/com.apple.WebKit.Networking/*", "WebKit network cache"),
        ("~/Library/DiagnosticReports/*", "Diagnostic reports"),
        ("~/Library/Caches/com.apple.QuickLook.thumbnailcache", "QuickLook thumbnails"),
        ("~/Library/Caches/Quick Look/*", "QuickLook cache"),
        ("~/Library/Caches/com.apple.iconservices*", "Icon services cache"),
        ("~/Library/IdentityCaches/*", "Identity caches"),
        ("~/Library/Suggestions/*", "Siri suggestions cache"),
        (
            "~/Library/Application Support/AddressBook/Sources/*/Photos.cache",
            "Address Book photo cache",
        ),
        // 沙盒应用缓存（Sandboxed app caches）。
        (
            "~/Library/Containers/com.apple.wallpaper.agent/Data/Library/Caches/*",
            "Wallpaper agent cache",
        ),
        (
            "~/Library/Containers/com.apple.mediaanalysisd/Data/Library/Caches/*",
            "Media analysis cache",
        ),
        (
            "~/Library/Containers/com.apple.mediaanalysisd/Data/tmp/*",
            "Media analysis temp files",
        ),
        (
            "~/Library/Containers/com.apple.AppStore/Data/Library/Caches/*",
            "App Store cache",
        ),
        (
            "~/Library/Containers/com.apple.configurator.xpc.InternetService/Data/tmp/*",
            "Apple Configurator temp files",
        ),
        (
            "~/Library/Containers/com.apple.wallpaper.extension.aerials/Data/tmp/*",
            "Wallpaper aerials temp files",
        ),
        ("~/Library/Containers/com.apple.geod/Data/tmp/*", "Geod temp files"),
        (
            "~/Library/Containers/com.apple.stocks/Data/Library/Caches/*",
            "Stocks cache",
        ),
        ("~/Library/Caches/com.apple.helpd/*", "macOS Help system cache"),
        ("~/Library/Caches/GeoServices/*", "Maps geo tile cache"),
        (
            "~/Library/Containers/com.apple.AvatarUI.AvatarPickerMemojiPicker/Data/Library/Caches/*",
            "Memoji picker cache",
        ),
        (
            "~/Library/Containers/com.apple.AMPArtworkAgent/Data/Library/Caches/*",
            "Music album art cache",
        ),
        (
            "~/Library/Containers/com.apple.CoreDevice.CoreDeviceService/Data/Library/Caches/*",
            "CoreDevice service cache",
        ),
        (
            "~/Library/Containers/com.apple.NeptuneOneExtension/Data/Library/Caches/*",
            "Apple Intelligence extension cache",
        ),
        (
            "~/Library/Containers/com.apple.AppleMediaServicesUI.UtilityExtension/Data/tmp/*",
            "Apple Media Services temp files",
        ),
        ("~/Library/Caches/com.apple.AppleMediaServices/*", "Apple Media Services cache"),
        ("~/Library/Caches/com.apple.duetexpertd/*", "Duet Expert cache"),
        ("~/Library/Caches/com.apple.parsecd/*", "Parsecd cache"),
        ("~/Library/Caches/com.apple.python/*", "Apple Python cache"),
    ];
    rows.iter()
        .map(|(path, description)| CatalogEntry {
            family: "apple_user",
            path,
            home_env: None,
            description,
        })
        .collect()
}

/// 浏览器族静态缓存行（对标 clean_browsers 中无进程守卫的 safe_clean 行）。
///
/// 需要进程守卫的 Chrome/Firefox/Arc/Brave/Dia/Vivaldi/QQBrowser3 档案缓存
/// 在 mod.rs 的 guarded_entries 中按三态探针动态挂载。
/// Service Worker CacheStorage 与旧版本清理（table-driven）留待后续子片。
pub fn browser_catalog() -> Vec<CatalogEntry> {
    let rows: &[(&str, &str)] = &[
        ("~/Library/Caches/com.apple.Safari/*", "Safari cache"),
        ("~/Library/Caches/Chromium/*", "Chromium cache"),
        ("~/.cache/puppeteer/*", "Puppeteer browser cache"),
        ("~/Library/Caches/com.microsoft.edgemac/*", "Edge cache"),
        (
            "~/Library/Application Support/Google/GoogleUpdater/crx_cache/*",
            "GoogleUpdater CRX cache",
        ),
        (
            "~/Library/Application Support/Google/GoogleUpdater/*.old",
            "GoogleUpdater old files",
        ),
        ("~/Library/Caches/company.thebrowser.Browser/*", "Arc cache"),
        ("~/Library/Caches/company.thebrowser.dia/*", "Dia cache"),
        ("~/Library/Caches/BraveSoftware/Brave-Browser/*", "Brave cache"),
        ("~/Library/Caches/net.imput.helium/*", "Helium cache"),
        ("~/Library/Caches/Yandex/YandexBrowser/*", "Yandex cache"),
        ("~/Library/Caches/com.operasoftware.Opera/*", "Opera cache"),
        ("~/Library/Caches/com.vivaldi.Vivaldi/*", "Vivaldi cache"),
        ("~/Library/Caches/Comet/*", "Comet cache"),
        ("~/Library/Caches/com.kagi.kagimacOS/*", "Orion cache"),
        ("~/Library/Caches/zen/*", "Zen cache"),
        ("~/Library/Caches/com.tencent.QQBrowser3/*", "QQ Browser cache"),
    ];
    rows.iter()
        .map(|(path, description)| CatalogEntry {
            family: "browser",
            path,
            home_env: None,
            description,
        })
        .collect()
}

/// Apple Silicon 缓存（对标 clean_apple_silicon_caches）：仅 arm64 主机
/// 在运行时挂载（见 mod.rs 的 is_apple_silicon）。
pub fn apple_silicon_catalog() -> Vec<CatalogEntry> {
    let rows: &[(&str, &str)] = &[
        (
            "/Library/Apple/usr/share/rosetta/rosetta_update_bundle",
            "Rosetta 2 cache",
        ),
        (
            "~/Library/Caches/com.apple.rosetta.update",
            "Rosetta 2 user cache",
        ),
        (
            "~/Library/Caches/com.apple.amp.mediasevicesd",
            "Apple Silicon media service cache",
        ),
    ];
    rows.iter()
        .map(|(path, description)| CatalogEntry {
            family: "apple_silicon",
            path,
            home_env: None,
            description,
        })
        .collect()
}

/// 虚拟化工具静态缓存行（对标 clean_virtualization_tools 中无守卫的行）。
/// UTM 有进程守卫、Tart 走 owner prune——见 mod.rs。
pub fn virtualization_catalog() -> Vec<CatalogEntry> {
    let rows: &[(&str, &str)] = &[
        ("~/Library/Caches/com.vmware.fusion", "VMware Fusion cache"),
        ("~/Library/Caches/com.parallels.*", "Parallels cache"),
        ("~/VirtualBox VMs/.cache", "VirtualBox cache"),
        (
            "~/Library/Caches/lima/download/by-url-sha256/*",
            "Lima download cache",
        ),
        ("~/.vagrant.d/tmp/*", "Vagrant temporary files"),
    ];
    rows.iter()
        .map(|(path, description)| CatalogEntry {
            family: "virtualization",
            path,
            home_env: None,
            description,
        })
        .collect()
}

/// 云存储与 Office 静态缓存行（对标 clean_cloud_storage / clean_office_applications
/// 中无进程守卫的行；Dropbox/GoogleDrive/OneDrive 守卫行见 mod.rs）。
pub fn cloud_office_catalog() -> Vec<CatalogEntry> {
    let rows: &[(&str, &str)] = &[
        ("~/Library/Caches/com.baidu.netdisk", "Baidu Netdisk cache"),
        (
            "~/Library/Caches/com.alibaba.teambitiondisk",
            "Alibaba Cloud cache",
        ),
        ("~/Library/Caches/com.box.desktop", "Box cache"),
        ("~/Library/Caches/com.microsoft.Word", "Microsoft Word cache"),
        (
            "~/Library/Containers/com.microsoft.Word/Data/Library/Caches/*",
            "Microsoft Word container cache",
        ),
        (
            "~/Library/Containers/com.microsoft.Word/Data/tmp/*",
            "Microsoft Word temp files",
        ),
        (
            "~/Library/Containers/com.microsoft.Word/Data/Library/Logs/*",
            "Microsoft Word container logs",
        ),
        ("~/Library/Caches/com.microsoft.Excel", "Microsoft Excel cache"),
        (
            "~/Library/Containers/com.microsoft.Excel/Data/Library/Caches/*",
            "Microsoft Excel container cache",
        ),
        (
            "~/Library/Containers/com.microsoft.Excel/Data/tmp/*",
            "Microsoft Excel temp files",
        ),
        (
            "~/Library/Containers/com.microsoft.Excel/Data/Library/Logs/*",
            "Microsoft Excel container logs",
        ),
        (
            "~/Library/Caches/com.microsoft.Powerpoint",
            "Microsoft PowerPoint cache",
        ),
        (
            "~/Library/Caches/com.microsoft.Outlook/*",
            "Microsoft Outlook cache",
        ),
        ("~/Library/Caches/com.apple.iWork.*", "Apple iWork cache"),
        (
            "~/Library/Caches/com.kingsoft.wpsoffice.mac",
            "WPS Office cache",
        ),
        (
            "~/Library/Caches/org.mozilla.thunderbird/*",
            "Thunderbird cache",
        ),
        ("~/Library/Caches/com.apple.mail/*", "Apple Mail cache"),
    ];
    rows.iter()
        .map(|(path, description)| CatalogEntry {
            family: "cloud_office",
            path,
            home_env: None,
            description,
        })
        .collect()
}

/// 用户基础行（对标 clean_user_essentials 中的显式 safe_clean 行）。
/// Trash 清空、Recent Items、Mail Downloads 见 mod.rs 动态行。
pub fn user_essentials_catalog() -> Vec<CatalogEntry> {
    let rows: &[(&str, &str)] = &[("~/Library/Logs/*", "User app logs")];
    rows.iter()
        .map(|(path, description)| CatalogEntry {
            family: "user_essentials",
            path,
            home_env: None,
            description,
        })
        .collect()
}

/// 全量目录。
pub fn full_catalog() -> Vec<CatalogEntry> {
    let mut all = apple_user_cache_catalog();
    all.extend(dev_toolchain_catalog());
    all.extend(browser_catalog());
    all.extend(apple_silicon_catalog());
    all.extend(virtualization_catalog());
    all.extend(cloud_office_catalog());
    all.extend(user_essentials_catalog());
    all
}

/// 开发工具链族，对标 dev.sh 的普通 `safe_clean` 行。
///
/// 对标排除（不入目录，见 tests）：
/// - 混合状态存储：registry/src、Cargo git、DENO_DIR、~/.ivy2/cache、
///   ~/.m2/repository、~/.nuget/packages、~/.cabal/packages、~/.cpan/sources、
///   ~/.sbt/boot、~/.sbt/launchers；
/// - 模型/实验数据：~/.cache/huggingface、torch、tensorflow、wandb；
/// - pypoetry/virtualenvs（活跃解释器）；
/// - AI CLI 工具缓存（Claude Code、opencode 等，可能含会话/凭据）。
pub fn dev_toolchain_catalog() -> Vec<CatalogEntry> {
    // (family, path, home_env, description)
    let rows: &[(&str, &str, Option<&str>, &str)] = &[
        // ---- 前端构建缓存（clean_dev_frontend + yarn/tnpm）----
        ("dev_frontend", "~/.cache/typescript/*", None, "TypeScript cache"),
        ("dev_frontend", "~/.cache/electron/*", None, "Electron cache"),
        ("dev_frontend", "~/.cache/node-gyp/*", None, "node-gyp cache"),
        ("dev_frontend", "~/.node-gyp/*", None, "node-gyp build cache"),
        ("dev_frontend", "~/.turbo/cache/*", None, "Turbo cache"),
        ("dev_frontend", "~/.vite/cache/*", None, "Vite cache"),
        ("dev_frontend", "~/.cache/vite/*", None, "Vite global cache"),
        ("dev_frontend", "~/.cache/webpack/*", None, "Webpack cache"),
        ("dev_frontend", "~/.parcel-cache/*", None, "Parcel cache"),
        ("dev_frontend", "~/.cache/eslint/*", None, "ESLint cache"),
        ("dev_frontend", "~/.cache/prettier/*", None, "Prettier cache"),
        ("dev_frontend", "~/.yarn/cache/*", None, "Yarn cache"),
        ("dev_frontend", "~/Library/Caches/Yarn/*", None, "Yarn v1 cache"),
        ("dev_frontend", "~/.tnpm/_cacache/*", None, "tnpm cache directory"),
        ("dev_frontend", "~/.tnpm/_logs/*", None, "tnpm logs"),
        // ---- Python 工具链（clean_dev_python 的普通行）----
        ("dev_python", "~/.pyenv/cache/*", None, "pyenv cache"),
        ("dev_python", "~/.cache/poetry/*", None, "Poetry cache"),
        (
            "dev_python",
            "~/Library/Caches/pypoetry/artifacts/*",
            None,
            "Poetry artifacts cache",
        ),
        ("dev_python", "~/Library/Caches/pypoetry/cache/*", None, "Poetry package cache"),
        ("dev_python", "~/.cache/ruff/*", None, "Ruff cache"),
        ("dev_python", "~/.cache/mypy/*", None, "MyPy cache"),
        ("dev_python", "~/.pytest_cache/*", None, "Pytest cache"),
        ("dev_python", "~/.jupyter/runtime/*", None, "Jupyter runtime cache"),
        // ---- Rust 工具链（rustup downloads；cargo registry 需进程守卫，后续片）----
        (
            "dev_rust",
            "downloads/*",
            Some("RUSTUP_HOME"),
            "Rustup downloads cache",
        ),
        // ---- Ruby/Perl 工具链 ----
        ("dev_ruby", "~/.rbenv/cache/*", None, "rbenv download cache"),
        ("dev_ruby", "~/.gem/specs/*", None, "gem spec cache"),
        ("dev_ruby", "~/.gem/ruby/*/cache/*.gem", None, "gem package cache"),
        ("dev_ruby", "~/.bundle/cache/*", None, "Ruby Bundler cache"),
        ("dev_perl", "~/.cpan/build/*", None, "CPAN build artifacts"),
        // ---- 云 CLI 与容器 ----
        ("dev_cloud", "~/.docker/buildx/cache/*", None, "Docker BuildX cache"),
        ("dev_cloud", "~/.kube/cache/*", None, "Kubernetes cache"),
        (
            "dev_cloud",
            "~/.local/share/containers/storage/tmp/*",
            None,
            "Container storage temp",
        ),
        ("dev_cloud", "~/.aws/cli/cache/*", None, "AWS CLI cache"),
        ("dev_cloud", "~/.config/gcloud/logs/*", None, "Google Cloud logs"),
        ("dev_cloud", "~/.azure/logs/*", None, "Azure CLI logs"),
        // ---- CI 与 DevOps ----
        ("dev_ci", "~/.cache/bazel/*", None, "Bazel cache"),
        ("dev_ci", "~/.cache/zig/*", None, "Zig cache"),
        ("dev_ci", "~/.cache/terraform/*", None, "Terraform cache"),
        ("dev_ci", "~/.grafana/cache/*", None, "Grafana cache"),
        ("dev_ci", "~/.prometheus/data/wal/*", None, "Prometheus WAL cache"),
        ("dev_ci", "~/.jenkins/workspace/*/target/*", None, "Jenkins workspace cache"),
        ("dev_ci", "~/.cache/gitlab-runner/*", None, "GitLab Runner cache"),
        ("dev_ci", "~/.github/cache/*", None, "GitHub Actions cache"),
        ("dev_ci", "~/.circleci/cache/*", None, "CircleCI cache"),
        ("dev_ci", "~/.sonar/*", None, "SonarQube cache"),
    ];
    rows.iter()
        .map(|(family, path, home_env, description)| CatalogEntry {
            family,
            path,
            home_env: *home_env,
            description,
        })
        .collect()
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
    fn excluded_apple_targets_are_not_in_catalog() {
        let catalog = full_catalog();
        let paths: Vec<&str> = catalog.iter().map(|e| e.path).collect();
        assert!(!paths.iter().any(|p| p.contains("Autosave Information")));
        assert!(!paths.iter().any(|p| p.contains("Calendar Cache")));
        assert!(!paths
            .iter()
            .any(|p| p.contains("com.apple.wallpaper/aerials/thumbnails")));
        assert!(!paths.iter().any(|p| p.contains("E5RT")));
    }

    /// 对标 AGENTS.md 恢复契约分级：混合状态/模型/会话存储不入目录。
    #[test]
    fn mixed_state_stores_are_not_in_catalog() {
        let paths: Vec<&str> = full_catalog().iter().map(|e| e.path).collect();
        for banned in [
            "registry/src",
            "registry/git",
            ".cache/huggingface",
            ".cache/torch",
            ".cache/tensorflow",
            ".cache/wandb",
            "pypoetry/virtualenvs",
            ".cpan/sources",
            ".m2/repository",
            ".ivy2/cache",
            ".nuget/packages",
            ".cabal/packages",
            ".sbt/boot",
            "DENO_DIR",
            // AI CLI 工具（会话/凭据状态）。
            "Claude",
            "opencode",
        ] {
            assert!(
                !paths.iter().any(|p| p.contains(banned)),
                "不应入目录: {banned}"
            );
        }
    }

    /// 描述必须唯一（执行按描述选择组）。
    #[test]
    fn descriptions_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for e in full_catalog() {
            assert!(seen.insert(e.description), "重复描述: {}", e.description);
        }
    }

    #[test]
    fn family_labels_cover_all() {
        for e in full_catalog() {
            assert_ne!(family_label(e.family), "其他", "未知族: {}", e.family);
        }
    }

    /// 浏览器族入目录且描述唯一（GUI 按描述选组）。
    #[test]
    fn browser_catalog_present_and_unique() {
        let browsers = browser_catalog();
        assert!(!browsers.is_empty());
        let mut seen = std::collections::HashSet::new();
        for e in &browsers {
            assert!(seen.insert(e.description), "重复描述: {}", e.description);
            assert_eq!(e.family, "browser");
        }
    }
}
