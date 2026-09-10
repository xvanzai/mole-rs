//! Steam 启动器识别，对标 `lib/uninstall/steam.sh`。
//!
//! Steam 的"创建桌面快捷方式"写的是极小 shell 包装，只 open steam://run/<id>。
//! 其 bundle 大小是启动器大小而非游戏本体——识别后标注为 Steam 管理，
//! 不把快捷方式大小当可卸载应用大小。

use std::path::Path;

/// 对标 uninstall_steam_launcher_appid：解析 steam://run/<appid>。
/// 返回 Some(appid) 表示是 Steam 生成的启动器。
pub(crate) fn steam_launcher_appid(app_path: &str) -> Option<String> {
    let p = Path::new(app_path);
    if app_path.is_empty() || !p.is_dir() {
        return None;
    }

    // CFBundleExecutable → 回退 app 名去 .app。
    let exec_name = read_info_executable(p).unwrap_or_else(|| {
        let stem = p
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        stem.strip_suffix(".app")
            .or_else(|| stem.strip_suffix(".App"))
            .unwrap_or(&stem)
            .to_string()
    });

    let script = p.join("Contents/MacOS").join(&exec_name);
    if !script.is_file() {
        return None;
    }
    // 可读可执行。
    use std::os::unix::fs::PermissionsExt;
    let meta = std::fs::metadata(&script).ok()?;
    if meta.permissions().mode() & 0o111 == 0 {
        return None;
    }
    // 大小 ≤ 4096（对标 script_size bound）。
    if meta.len() > 4096 {
        return None;
    }
    let content = std::fs::read_to_string(&script).ok()?;
    parse_steam_script(&content)
}

/// 解析脚本：shebang sh/bash + 恰好一条活动命令
/// `open steam://(run|rungameid|launch)/<digits>`。
pub(crate) fn parse_steam_script(content: &str) -> Option<String> {
    let mut lines = content.lines();
    let first = lines.next()?;
    // shebang：#!/bin/sh | #!/usr/bin/env bash | #!/bin/bash | #!/bin/zsh 等。
    let shebang_ok = {
        let t = first.trim_start();
        if !t.starts_with("#!") {
            false
        } else {
            let rest = &t[2..];
            rest.contains("/bin/sh")
                || rest.contains("/bin/bash")
                || rest.contains("/bin/zsh")
                || rest.contains(" env ")
                || rest.ends_with("sh")
                || rest.ends_with("bash")
                || rest.ends_with("zsh")
        }
    };
    if !shebang_ok {
        return None;
    }

    let mut active = 0u32;
    let mut appid: Option<String> = None;
    for line in lines {
        let line = line.trim_end_matches('\r');
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        active += 1;
        if active > 1 {
            return None;
        }
        // 允许 exec 前缀。
        let cmd = trimmed.strip_prefix("exec ").unwrap_or(trimmed);
        // open [quote]steam://(run|rungameid|launch)/digits[quote]
        let Some(url_part) = cmd.find("steam://") else {
            return None;
        };
        let url = &cmd[url_part..];
        // 去掉可能的引号与尾部空白。
        let url = url.trim().trim_matches(|c| c == '\'' || c == '"');
        let ok_scheme = url.starts_with("steam://run/")
            || url.starts_with("steam://rungameid/")
            || url.starts_with("steam://launch/");
        if !ok_scheme {
            return None;
        }
        // 前缀必须是 open（可带 exec）；允许 open 后跟引号再跟 URL。
        let before = cmd[..url_part]
            .trim()
            .trim_matches(|c| c == '\'' || c == '"')
            .trim();
        if before != "open" {
            return None;
        }
        let id = url.rsplit('/').next()?;
        if !id.chars().all(|c| c.is_ascii_digit()) || id.is_empty() {
            return None;
        }
        appid = Some(id.to_string());
    }
    if active == 1 {
        appid
    } else {
        None
    }
}

fn read_info_executable(app: &Path) -> Option<String> {
    let plist = app.join("Contents/Info.plist");
    let dict = plist::Value::from_file(plist).ok()?.into_dictionary()?;
    dict.get("CFBundleExecutable")
        .and_then(|v| v.as_string())
        .map(|s| s.to_string())
}

/// 对标 uninstall_app_is_steam_launcher。
pub fn is_steam_launcher(app_path: &str) -> bool {
    steam_launcher_appid(app_path).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 典型 Steam 生成的启动器脚本。
    #[test]
    fn parses_typical_launcher() {
        let script = "#!/bin/sh\n\nexec open \"steam://run/480\"\n";
        assert_eq!(parse_steam_script(script), Some("480".into()));
    }

    #[test]
    fn parses_rungameid_and_launch() {
        assert_eq!(
            parse_steam_script("#!/bin/bash\nopen steam://rungameid/730\n"),
            Some("730".into())
        );
        assert_eq!(
            parse_steam_script("#!/bin/sh\nopen 'steam://launch/12345'\n"),
            Some("12345".into())
        );
    }

    /// 多条活动命令 / 非 shebang / 非 open / 非 steam → None。
    #[test]
    fn rejects_non_launchers() {
        assert_eq!(parse_steam_script("#!/bin/sh\nls\nopen steam://run/1\n"), None);
        assert_eq!(parse_steam_script("open steam://run/1\n"), None);
        assert_eq!(parse_steam_script("#!/bin/sh\necho steam://run/1\n"), None);
        assert_eq!(parse_steam_script("#!/bin/sh\nopen https://example.com\n"), None);
        assert_eq!(parse_steam_script("#!/bin/sh\n"), None);
        // 大脚本由大小门控，这里测解析层多命令拒绝。
        assert_eq!(parse_steam_script("#!/bin/sh\ntrue\ntrue\n"), None);
    }

    /// fixture：构造最小 Steam 启动器 bundle。
    #[test]
    fn launcher_fixture() {
        let pid = std::process::id();
        let app_name = format!("MyGame{pid}");
        let tmp = std::env::temp_dir().join(format!("{app_name}.app"));
        let macos = tmp.join("Contents/MacOS");
        std::fs::create_dir_all(&macos).unwrap();
        // 无 Info.plist → 回退 app 名 MyGame<pid>。
        let script = macos.join(&app_name);
        std::fs::write(&script, "#!/bin/sh\nopen \"steam://run/480\"\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(steam_launcher_appid(&tmp.to_string_lossy()), Some("480".into()));
        assert!(is_steam_launcher(&tmp.to_string_lossy()));
        std::fs::remove_dir_all(&tmp).ok();
    }
}
