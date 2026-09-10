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
        "browser_old_versions" => "浏览器旧版本",
        "apple_silicon" => "Apple Silicon 更新",
        "virtualization" => "虚拟化工具",
        "app_support" => "Application Support",
        "cloud_office" => "云与 Office",
        "user_essentials" => "用户基础",
        "service_worker" => "Service Worker",
        "app_xcode" => "Xcode",
        "app_editors" => "代码编辑器",
        "app_comm" => "通讯协作",
        "app_design_video" => "设计与视频",
        "app_3d" => "3D 工具",
        "app_notes" => "笔记与游戏",
        "app_media" => "媒体播放",
        "app_browser" => "浏览器扩展",
        "app_gaming" => "游戏平台",
        "app_virt" => "虚拟化应用",
        "app_download" => "下载工具",
        "app_translate" => "翻译工具",
        "app_utils" => "系统工具",
        "app_email" => "邮件客户端",
        "app_productivity" => "效率日历",
        "app_screenshot" => "截图工具",
        "app_shell" => "终端工具",
        "app_launcher" => "启动器",
        "app_remote" => "远程桌面",
        "app_misc" => "其他应用缓存",
        "owner_command" => "Owner 命令清理",
        "deep_system" => "系统级清理",
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


/// 应用缓存族（对标 app_caches.sh 中除 apple_user 外的全部显式 safe_clean 行）。
/// 动态/循环行（Xcode 多版本、Fusion 旧 bundle、NeatDM 分段等）留待独立子片。
pub fn app_cache_catalog() -> Vec<CatalogEntry> {
    // (family, path, description)
    let rows: &[(&str, &str, &str)] = &[
// ---- 代码编辑器 (19) ----
("app_editors", "~/Library/Application Support/CodeBuddy CN/DawnGraphiteCache/*", "CodeBuddy CN Dawn cache"),
("app_editors", "~/Library/Application Support/CodeBuddy CN/GPUCache/*", "CodeBuddy CN GPU cache"),
("app_editors", "~/Library/Application Support/CodeBuddy CN/DawnWebGPUCache/*", "CodeBuddy CN WebGPU cache"),
("app_editors", "~/Library/Application Support/CodeBuddy CN/Cache/*", "CodeBuddy CN cache"),
("app_editors", "~/Library/Application Support/CodeBuddy CN/CachedData/*", "CodeBuddy CN cached data"),
("app_editors", "~/Library/Application Support/CodeBuddy CN/Code Cache/*", "CodeBuddy CN code cache"),
("app_editors", "~/Library/Application Support/CodeBuddy CN/CachedExtensionVSIXs/*", "CodeBuddy CN extension cache"),
("app_editors", "~/Library/Application Support/CodeBuddy CN/logs/*", "CodeBuddy CN logs"),
("app_editors", "~/Library/Application Support/CodeBuddyExtension/Cache/*", "CodeBuddy Extension cache"),
("app_editors", "~/Library/Application Support/CodeBuddyExtension/logs/*", "CodeBuddy Extension logs"),
("app_editors", "~/Library/Caches/com.sublimetext.*/*", "Sublime Text cache"),
("app_editors", "~/Library/Application Support/Code/Cache/*", "VS Code cache"),
("app_editors", "~/Library/Application Support/Code/CachedData/*", "VS Code data cache"),
("app_editors", "~/Library/Application Support/Code/CachedExtensions/*", "VS Code extension cache"),
("app_editors", "~/Library/Application Support/Code/logs/*", "VS Code logs"),
("app_editors", "~/Library/Application Support/Code/WebStorage/*/CacheStorage/*", "VS Code webview cache"),
("app_editors", "~/Library/Caches/Zed/*", "Zed cache"),
("app_editors", "~/Library/Logs/Zed/*", "Zed logs"),
("app_editors", "~/Library/Application Support/Zed/node/cache/*", "Zed npm cache"),
// ---- 通讯协作 (25) ----
("app_comm", "~/Library/Caches/com.alibaba.AliLang.osx/*", "AliLang security component"),
("app_comm", "~/Library/Application Support/iDingTalk/holmeslogs/*", "DingTalk holmes logs"),
("app_comm", "~/Library/Caches/dd.work.exclusive4aliding/*", "DingTalk iDingTalk cache"),
("app_comm", "~/Library/Application Support/iDingTalk/log/*", "DingTalk logs"),
("app_comm", "~/Library/Application Support/discord/Cache/*", "Discord cache"),
("app_comm", "~/Library/Caches/com.feishu.*/*", "Feishu cache"),
("app_comm", "~/Library/Caches/com.microsoft.teams2/*", "Microsoft Teams cache"),
("app_comm", "~/Library/Application Support/Microsoft/Teams/GPUCache/*", "Microsoft Teams legacy GPU cache"),
("app_comm", "~/Library/Application Support/Microsoft/Teams/Application Cache/*", "Microsoft Teams legacy application cache"),
("app_comm", "~/Library/Application Support/Microsoft/Teams/Cache/*", "Microsoft Teams legacy cache"),
("app_comm", "~/Library/Application Support/Microsoft/Teams/Code Cache/*", "Microsoft Teams legacy code cache"),
("app_comm", "~/Library/Application Support/Microsoft/Teams/logs/*", "Microsoft Teams legacy logs"),
("app_comm", "~/Library/Application Support/Microsoft/Teams/tmp/*", "Microsoft Teams legacy temp files"),
("app_comm", "~/Library/Caches/com.tencent.QQMusicMac/*", "QQ Music Mac cache"),
("app_comm", "~/Library/Caches/com.tencent.QQMusic/*", "QQ Music cache"),
("app_comm", "~/Library/Containers/com.tencent.QQMusicMac/Data/Library/Caches/*", "QQ Music container cache"),
("app_comm", "~/Library/Caches/com.tencent.qq/*", "QQ cache"),
("app_comm", "~/Library/Caches/com.skype.skype/*", "Skype cache"),
("app_comm", "~/Library/Application Support/Slack/Cache/*", "Slack cache"),
("app_comm", "~/Library/Caches/ru.keepcoder.Telegram/*", "Telegram cache"),
("app_comm", "~/Library/Caches/com.tencent.meeting/*", "Tencent Meeting cache"),
("app_comm", "~/Library/Caches/com.tencent.xinWeChat/*", "WeChat cache"),
("app_comm", "~/Library/Caches/com.tencent.WeWorkMac/*", "WeCom cache"),
("app_comm", "~/Library/Caches/net.whatsapp.WhatsApp/*", "WhatsApp cache"),
("app_comm", "~/Library/Caches/us.zoom.xos/*", "Zoom cache"),
// ---- 其他应用 (68) ----
("app_misc", "~/Library/Caches/com.any.do.*", "Any.do cache"),
("app_misc", "~/Library/Caches/com.apple.podcasts", "Apple Podcasts cache"),
("app_misc", "~/Library/Caches/com.apple.TV/*", "Apple TV cache"),
("app_misc", "~/Library/Caches/net.xmac.aria2gui", "Aria2 cache"),
("app_misc", "~/Library/Caches/tv.danmaku.bili/*", "Bilibili cache"),
("app_misc", "~/Library/Caches/com.bob-build.Bob", "Bob Translation cache"),
("app_misc", "~/.cacher/logs/*", "Cacher logs"),
("app_misc", "~/Library/Caches/com.reincubate.camo", "Camo cache"),
("app_misc", "~/Library/Caches/com.openai.chat/*", "ChatGPT cache"),
// Claude Desktop 缓存/日志：AGENTS.md 要求 AI 工具清理保守（可能含会话状态），不入目录。
("app_misc", "~/Library/Caches/com.douyu.*/*", "Douyu cache"),
("app_misc", "~/Library/Caches/com.downie.Downie-*", "Downie cache"),
("app_misc", "~/Library/Caches/com.eudic.*", "Eudict cache"),
("app_misc", "~/Library/Caches/com.filo.client/*", "Filo cache"),
("app_misc", "~/Library/Caches/com.flomoapp.mac/*", "Flomo cache"),
("app_misc", "~/Library/Containers/is.follow/Data/Library/Application Support/Folo/Cache/Cache_Data/*", "Folo cache"),
("app_misc", "~/Library/Caches/com.huya.*/*", "Huya cache"),
("app_misc", "~/Library/Caches/com.runjuu.Input-Source-Pro/*", "Input Source Pro cache"),
("app_misc", "~/.cache/kaku/*", "Kaku cache"),
("app_misc", "~/.kite/logs/*", "Kite logs"),
("app_misc", "~/Library/Caches/com.klee.desktop/*", "Klee cache"),
("app_misc", "~/Library/Caches/klee_desktop/*", "Klee desktop cache"),
("app_misc", "~/Library/Caches/com.lmstudio.lmstudio/*", "LM Studio cache"),
("app_misc", "~/Library/Application Support/legcord/Cache/*", "Legcord cache"),
("app_misc", "~/Library/Caches/com.tw93.MiaoYan/*", "MiaoYan cache"),
("app_misc", "~/Library/Containers/com.ideasoncanvas.mindnode/Data/Library/Caches/*", "MindNode cache"),
("app_misc", "~/Library/Containers/com.ranchero.NetNewsWire-Evergreen/Data/Library/Caches/*", "NetNewsWire cache"),
("app_misc", "~/Library/Caches/com.orabrowser.app/*", "Ora browser cache"),
("app_misc", "~/Library/Caches/net.pcsx2.PCSX2/*", "PCSX2 cache"),
("app_misc", "~/Library/Logs/PCSX2/*", "PCSX2 logs"),
("app_misc", "~/Library/Application Support/PCSX2/cache/*", "PCSX2 shader cache"),
("app_misc", "~/Library/Caches/com.charlessoft.pacifist/*", "Pacifist cache"),
("app_misc", "~/Library/Caches/tv.plex.player.desktop", "Plex cache"),
("app_misc", "~/Library/Containers/com.apple.podcasts/Data/tmp/*.heic", "Podcasts artwork cache"),
("app_misc", "~/Library/Containers/com.apple.podcasts/Data/tmp/*.img", "Podcasts image cache"),
("app_misc", "~/Library/Containers/com.apple.podcasts/Data/tmp/StreamedMedia", "Podcasts streamed media"),
("app_misc", "~/Library/Application Support/Quark/Cache/videoCache/*", "Quark video cache"),
("app_misc", "~/Library/Caches/net.rpcs3.rpcs3/*", "RPCS3 cache"),
("app_misc", "~/Library/Application Support/rpcs3/logs/*", "RPCS3 logs"),
("app_misc", "~/Library/Caches/com.riotgames.*/*", "Riot Games cache"),
("app_misc", "~/Library/Containers/com.wuziqi.SenPlayer/Data/tmp/videoCache/*", "SenPlayer video cache"),
("app_misc", "~/Library/Application Support/spacedrive/thumbnails/*", "Spacedrive thumbnail cache"),
("app_misc", "~/Library/Caches/ws.stash.app.mac/*", "Stash cache"),
("app_misc", "~/Library/Caches/smart.stremio*/*", "Stremio cache"),
("app_misc", "~/Library/Application Support/stremio/stremio-server/stremio-cache/*", "Stremio server cache"),
("app_misc", "~/Library/Caches/com.sunlogin.*/*", "Sunlogin cache"),
("app_misc", "~/Library/Caches/com.tencent.tenvideo", "Tencent Video cache"),
("app_misc", "~/Library/Caches/com.todesk.*/*", "ToDesk cache"),
("app_misc", "~/Library/Caches/org.m0k.transmission", "Transmission cache"),
("app_misc", "~/.viminfo.tmp", "Vim temporary files"),
("app_misc", "~/Library/Caches/macos-wakatime.WakaTime/*", "WakaTime cache"),
("app_misc", "~/Library/Application Support/WeType/DictUpdate/*", "WeType dict update cache"),
("app_misc", "~/Library/Application Support/WeType/com.onevcat.Kingfisher.ImageCache.WeType/*", "WeType image cache"),
("app_misc", "~/Library/Caches/com.xnipapp.xnip", "Xnip cache"),
("app_misc", "~/Library/Caches/com.yinxiang.*/*", "Yinxiang Note cache"),
("app_misc", "~/Library/Caches/com.youdao.YoudaoDict", "Youdao Dictionary cache"),
("app_misc", "~/.zcompdump*", "Zsh completion cache"),
("app_misc", "~/Library/Caches/com.iqiyi.player", "iQIYI cache"),
("app_misc", "~/.lesshst", "less history"),
("app_misc", "~/Library/Application Support/mihomo-party/DawnGraphiteCache/*", "mihomo-party Dawn cache"),
("app_misc", "~/Library/Application Support/mihomo-party/GPUCache/*", "mihomo-party GPU cache"),
("app_misc", "~/Library/Application Support/mihomo-party/DawnWebGPUCache/*", "mihomo-party WebGPU cache"),
("app_misc", "~/Library/Application Support/mihomo-party/Cache/*", "mihomo-party cache"),
("app_misc", "~/Library/Application Support/mihomo-party/Code Cache/*", "mihomo-party code cache"),
("app_misc", "~/Library/Application Support/mihomo-party/logs/*", "mihomo-party logs"),
("app_misc", "~/Library/Caches/com.qbittorrent.qBittorrent", "qBittorrent cache"),
("app_misc", "~/.wget-hsts", "wget HSTS cache"),
// ---- 浏览器扩展 (7) ----
("app_browser", "~/Library/Caches/CCTClearcutLogger", "Google Clearcut logs"),
("app_browser", "~/.lunarclient/game-cache/*", "Lunar Client game cache"),
("app_browser", "~/.lunarclient/launcher-cache/*", "Lunar Client launcher cache"),
("app_browser", "~/.lunarclient/logs/*", "Lunar Client logs"),
("app_browser", "~/.lunarclient/offline/files/*/logs/*", "Lunar Client offline file logs"),
("app_browser", "~/.lunarclient/offline/*/logs/*", "Lunar Client offline logs"),
("app_browser", "~/Library/Caches/cx.c3.theunarchiver/*", "The Unarchiver cache"),
// ---- 设计与视频 (14) ----
("app_design_video", "~/Library/Caches/com.adobe.*/*", "Adobe app caches"),
("app_design_video", "~/Library/Caches/Adobe/*", "Adobe cache"),
("app_design_video", "~/Library/Application Support/Adobe/Common/Media Cache Files/*", "Adobe media cache files"),
("app_design_video", "~/Library/Caches/org.blenderfoundation.blender/*", "Blender cache"),
("app_design_video", "~/Library/Caches/com.maxon.cinema4d/*", "Cinema 4D cache"),
("app_design_video", "~/Movies/CacheClip/*", "DaVinci Resolve CacheClip"),
("app_design_video", "~/Library/Caches/com.blackmagic-design.DaVinciResolve/*", "DaVinci Resolve cache"),
("app_design_video", "~/Library/Caches/com.figma.Desktop/*", "Figma cache"),
("app_design_video", "~/Library/Caches/com.apple.FinalCut/*", "Final Cut Pro cache"),
("app_design_video", "~/Library/Caches/com.adobe.PremierePro.*/*", "Premiere Pro cache"),
("app_design_video", "~/Library/Caches/net.telestream.screenflow10/*", "ScreenFlow cache"),
("app_design_video", "~/Library/Application Support/com.bohemiancoding.sketch3/cache/*", "Sketch app cache"),
("app_design_video", "~/Library/Caches/com.bohemiancoding.sketch3/*", "Sketch cache"),
("app_design_video", "~/Library/Caches/com.sketchup.*/*", "SketchUp cache"),
// ---- 媒体播放 (8) ----
("app_media", "~/Library/Caches/com.apple.Music", "Apple Music cache"),
("app_media", "~/Library/Caches/com.colliderli.iina", "IINA cache"),
("app_media", "~/Library/Caches/com.kugou.mac/*", "Kugou Music cache"),
("app_media", "~/Library/Caches/com.kuwo.mac/*", "Kuwo Music cache"),
("app_media", "~/Library/Caches/io.mpv", "MPV cache"),
("app_media", "~/Library/Caches/com.netease.163music", "NetEase Music cache"),
("app_media", "~/Library/Caches/com.spotify.client/*", "Spotify cache"),
("app_media", "~/Library/Caches/org.videolan.vlc", "VLC cache"),
// ---- 下载工具 (2) ----
("app_download", "~/Library/Caches/com.folx.*/*", "Folx cache"),
("app_download", "~/Library/Containers/com.apple.podcasts/Data/tmp/*CFNetworkDownload*.tmp", "Podcasts download temp"),
// ---- 游戏平台 (12) ----
("app_gaming", "~/Library/Application Support/Battle.net/Cache/*", "Battle.net app cache"),
("app_gaming", "~/Library/Caches/com.blizzard.Battle.net/*", "Battle.net cache"),
("app_gaming", "~/Library/Caches/com.ea.*/*", "EA Origin cache"),
("app_gaming", "~/Library/Caches/com.epicgames.EpicGamesLauncher/*", "Epic Games cache"),
("app_gaming", "~/Library/Caches/com.gog.galaxy/*", "GOG Galaxy cache"),
("app_gaming", "~/Library/Caches/com.mitchellh.ghostty/*", "Ghostty cache"),
("app_gaming", "~/Library/Application Support/Steam/appcache/*", "Steam app cache"),
("app_gaming", "~/Library/Caches/com.valvesoftware.steam/*", "Steam cache"),
("app_gaming", "~/Library/Application Support/Steam/depotcache/*", "Steam depot cache"),
("app_gaming", "~/Library/Application Support/Steam/logs/*", "Steam logs"),
("app_gaming", "~/Library/Application Support/Steam/steamapps/shadercache/*", "Steam shader cache"),
("app_gaming", "~/Library/Application Support/Steam/htmlcache/*", "Steam web cache"),
// ---- 笔记与游戏 (9) ----
("app_notes", "~/Library/Caches/com.bear-writer.*/*", "Bear cache"),
("app_notes", "~/Library/Caches/com.evernote.*/*", "Evernote cache"),
("app_notes", "~/Library/Caches/com.logseq.*/*", "Logseq cache"),
("app_notes", "~/Library/Application Support/minecraft/crash-reports/*", "Minecraft crash reports"),
("app_notes", "~/Library/Application Support/minecraft/logs/*", "Minecraft logs"),
("app_notes", "~/Library/Application Support/minecraft/webcache/*", "Minecraft web cache"),
("app_notes", "~/Library/Application Support/minecraft/webcache2/*", "Minecraft web cache 2"),
("app_notes", "~/Library/Caches/notion.id/*", "Notion cache"),
("app_notes", "~/Library/Caches/md.obsidian/*", "Obsidian cache"),
// ---- 截图工具 (1) ----
("app_screenshot", "~/Library/Caches/com.cleanshot.*", "CleanShot cache"),
// ---- 邮件客户端 (2) ----
("app_email", "~/Library/Caches/com.airmail.*", "Airmail cache"),
("app_email", "~/Library/Caches/com.readdle.smartemail-Mac", "Spark cache"),
// ---- 效率日历 (1) ----
("app_productivity", "~/Library/Caches/com.todoist.mac.Todoist", "Todoist cache"),
// ---- 终端工具 (3) ----
("app_shell", "~/Library/Caches/SentryCrash/Warp/*", "Warp Sentry crash reports"),
("app_shell", "~/Library/Caches/dev.warp.Warp-Stable/*", "Warp cache"),
("app_shell", "~/Library/Logs/warp.log", "Warp log"),
// ---- 系统工具 (1) ----
("app_utils", "~/Library/Caches/com.runningwithcrayons.Alfred/*", "Alfred cache"),
// ---- 远程桌面 (2) ----
("app_remote", "~/Library/Caches/com.anydesk.*/*", "AnyDesk cache"),
("app_remote", "~/Library/Caches/com.teamviewer.*/*", "TeamViewer cache"),
    ];
    rows.iter()
        .map(|(family, path, description)| CatalogEntry {
            family,
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
    all.extend(app_cache_catalog());
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
