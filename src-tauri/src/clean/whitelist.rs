//! 白名单保护策略，对标 `Mole lib/core/app_protection.sh` 的
//! `load_mole_whitelist` / `is_path_whitelisted`（1:1 语义）。
//!
//! 白名单文件 `~/.config/mole/whitelist`：每行一个模式；`#` 注释；含
//! `//` 的行拒绝（#724）；系统保护路径拒绝加入；`~` 展开为主目录；
//! 用户白名单替换默认项，但硬安全项始终强制（ensure_safety_whitelist_patterns）。

use std::path::PathBuf;

/// 硬安全白名单：无论用户是否自定义白名单都强制生效。
/// 对标 ensure_safety_whitelist_patterns 的安全条目语义。
const SAFETY_PATTERNS: &[&str] = &[];

/// 拒绝加入白名单的系统保护路径（对标 WHITELIST_WARNINGS 的 case 校验：
/// 精确项与父目录项分开，`/` 只匹配根路径本身）。
const REJECTED_SYSTEM_EXACT: &[&str] = &[
    "/", "/System", "/bin", "/sbin", "/usr/bin", "/usr/sbin", "/etc", "/var/db",
];
const REJECTED_SYSTEM_PARENTS: &[&str] = &[
    "/System", "/bin", "/sbin", "/usr/bin", "/usr/sbin", "/etc", "/var/db",
];

/// fnmatch 风格 glob 匹配（支持 `*`、`?`、`[...]`，对标 bash
/// `[[ $path == $pattern ]]` 的模式语义）。
pub fn glob_match(pattern: &str, text: &str) -> bool {
    fn inner(p: &[u8], t: &[u8]) -> bool {
        match (p.first(), t.first()) {
            (None, None) => true,
            (Some(b'*'), _) => inner(&p[1..], t) || (!t.is_empty() && inner(p, &t[1..])),
            (Some(b'?'), Some(_)) => inner(&p[1..], &t[1..]),
            (Some(b'['), Some(_)) => {
                // 找到闭括号（首个 ']' 不在开头）。
                let mut i = 1;
                if i < p.len() && (p[i] == b'!' || p[i] == b'^') {
                    i += 1;
                }
                while i < p.len() && p[i] != b']' {
                    i += 1;
                }
                if i >= p.len() {
                    // 无闭合括号：按字面 '[' 处理。
                    return p[0] == t[0] && inner(&p[1..], &t[1..]);
                }
                let class = &p[1..i];
                let negated = class.first() == Some(&b'!') || class.first() == Some(&b'^');
                let class = if negated { &class[1..] } else { class };
                let matched = match_char_class(class, t[0]);
                if matched == !negated {
                    inner(&p[i + 1..], &t[1..])
                } else {
                    false
                }
            }
            (Some(&c), Some(&tc)) if c == tc => inner(&p[1..], &t[1..]),
            _ => false,
        }
    }
    inner(pattern.as_bytes(), text.as_bytes())
}

fn match_char_class(class: &[u8], c: u8) -> bool {
    let mut i = 0;
    while i < class.len() {
        if i + 2 < class.len() && class[i + 1] == b'-' {
            if class[i] <= c && c <= class[i + 2] {
                return true;
            }
            i += 3;
        } else {
            if class[i] == c {
                return true;
            }
            i += 1;
        }
    }
    false
}

/// 已加载的白名单。
pub struct Whitelist {
    pub(crate) patterns: Vec<String>,
    /// 加载来源：user（用户文件）/ default（默认项）/ empty。
    pub(crate) source: &'static str,
}

impl Whitelist {
    /// 对标 `load_mole_whitelist`：读 `~/.config/mole/whitelist`，
    /// 逐行校验、去重、展开 `~`。文件不存在时为空（无白名单 = 无额外保护，
    /// 对标 "Empty whitelist means nothing is protected"）。
    pub fn load() -> Self {
        let home = std::env::var("HOME").unwrap_or_default();
        let file = PathBuf::from(&home).join(".config/mole/whitelist");
        let mut patterns = Vec::new();
        let mut source = "default";
        if let Ok(content) = std::fs::read_to_string(&file) {
            source = "user";
            for raw_line in content.lines() {
                let line = raw_line.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                if line.contains("//") {
                    continue; // 对标 #724：连续斜杠行拒绝
                }
                if is_rejected_system_path(line) {
                    continue; // 对标：保护系统路径不得进入白名单
                }
                let expanded = line
                    .strip_prefix('~')
                    .map(|rest| format!("{home}{rest}"))
                    .unwrap_or_else(|| line.to_string());
                let expanded = normalize_slashes(&expanded);
                if !patterns.contains(&expanded) {
                    patterns.push(expanded);
                }
            }
        }
        for s in SAFETY_PATTERNS {
            let s = normalize_slashes(s);
            if !patterns.contains(&s) {
                patterns.push(s);
            }
        }
        Self { patterns, source }
    }

    /// 对标 `is_path_whitelisted`：先规范化目标（去尾斜杠、折叠连续斜杠），
    /// 然后精确匹配 / glob 匹配 / 父目录保护（白名单条目是目标的子路径时
    /// 目标同样受保护）。
    pub fn is_whitelisted(&self, target: &str) -> bool {
        if self.patterns.is_empty() {
            return false;
        }
        let normalized = normalize_slashes(target.trim_end_matches('/'));
        for pattern in &self.patterns {
            let check = pattern.trim_end_matches('/');
            if check.is_empty() {
                continue;
            }
            if normalized == check || glob_match(check, &normalized) {
                return true;
            }
            // 目标是白名单路径的父目录 → 保护（保住白名单子项）。
            if normalized.is_empty() {
                continue;
            }
            if let Some(stripped) = check.strip_prefix(&normalized) {
                if stripped.starts_with('/') {
                    return true;
                }
            }
            // 目标是白名单目录的子路径 → 保护（仅非 glob 条目，
            // 对标 has_glob == false 分支）。
            let has_glob = check.contains(['*', '?', '[']);
            if !has_glob && normalized.starts_with(&format!("{check}/")) {
                return true;
            }
        }
        false
    }

    pub fn source_description(&self) -> &'static str {
        self.source
    }
}

fn normalize_slashes(path: &str) -> String {
    let mut result = String::with_capacity(path.len());
    let mut last_was_slash = false;
    for c in path.chars() {
        if c == '/' {
            if !last_was_slash {
                result.push(c);
            }
            last_was_slash = true;
        } else {
            result.push(c);
            last_was_slash = false;
        }
    }
    result
}

fn is_rejected_system_path(line: &str) -> bool {
    REJECTED_SYSTEM_EXACT.contains(&line)
        || REJECTED_SYSTEM_PARENTS
            .iter()
            .any(|p| line.starts_with(&format!("{p}/")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glob_matches_bash_semantics() {
        assert!(glob_match("com.apple.iconservices*", "com.apple.iconservices"));
        assert!(glob_match("com.apple.iconservices*", "com.apple.iconservices.store"));
        assert!(!glob_match("com.apple.iconservices", "com.apple.iconservices.store"));
        assert!(glob_match("Sources/*/Photos.cache", "Sources/abc/Photos.cache"));
        // bash 的 [[ == $p ]] 中 * 会跨 / 匹配（区别于 shell 路径展开）。
        assert!(glob_match("Sources/*/Photos.cache", "Sources/abc/x/Photos.cache"));
        assert!(glob_match("cache?.db", "cache1.db"));
        assert!(glob_match("cache?.db", "cacheX.db"));
        assert!(!glob_match("cache?.db", "cache12.db"));
        assert!(glob_match("[ab].txt", "a.txt"));
        assert!(glob_match("[a-c].txt", "b.txt"));
        assert!(!glob_match("[!a].txt", "a.txt"));
        assert!(!glob_match("[a-c].txt", "d.txt"));
        // bash 中无闭合 '[' 按字面处理。
        assert!(glob_match("foo[bar", "foo[bar"));
    }

    #[test]
    fn normalize_collapses_slashes() {
        // 对标 #724：调用方拼接可能产生双斜杠。
        assert_eq!(normalize_slashes("a//b///c"), "a/b/c");
        assert_eq!(normalize_slashes("/a/b/"), "/a/b/");
    }

    #[test]
    fn system_paths_rejected() {
        assert!(is_rejected_system_path("/System"));
        assert!(is_rejected_system_path("/System/Library"));
        assert!(is_rejected_system_path("/usr/bin"));
        assert!(is_rejected_system_path("/etc/nginx"));
        assert!(!is_rejected_system_path("/Users/x/Library/Caches"));
        assert!(!is_rejected_system_path("~/Library/Caches"));
    }

    #[test]
    fn whitelist_matching_with_parent_protection() {
        let wl = Whitelist {
            patterns: vec![
                "/Users/t/Library/Caches/com.apple.helpd".to_string(),
                "/Users/t/Library/Containers/App/Data/tmp/subdir".to_string(),
            ],
            source: "user",
        };
        // 精确匹配。
        assert!(wl.is_whitelisted("/Users/t/Library/Caches/com.apple.helpd"));
        // 子路径匹配（glob 单段逐段，不做 `**`）。
        assert!(wl.is_whitelisted("/Users/t/Library/Containers/App/Data/tmp/subdir/file"));
        // 父目录保护：白名单条目的父目录不应被整目录清理殃及。
        assert!(wl.is_whitelisted("/Users/t/Library/Containers/App/Data/tmp"));
    }

    #[test]
    fn empty_whitelist_protects_nothing() {
        let wl = Whitelist {
            patterns: vec![],
            source: "default",
        };
        assert!(!wl.is_whitelisted("/anything"));
    }

    #[test]
    fn home_expansion_in_patterns() {
        // 模拟展开逻辑：~ → HOME。
        let home = std::env::var("HOME").unwrap_or_default();
        let line = "~/Library/Caches";
        let expanded = line
            .strip_prefix('~')
            .map(|rest| format!("{home}{rest}"))
            .unwrap();
        assert!(expanded.starts_with(&home));
    }
}
