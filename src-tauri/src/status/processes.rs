//! 进程采样与僵尸进程汇总，对标 `cmd/status/metrics_process.go`。

use super::types::{ProcessInfo, ZombieParent};
use std::time::Duration;

/// 对标 `zombieParentLimit`。
pub const ZOMBIE_PARENT_LIMIT: usize = 3;

/// 1:1 移植 `collectProcesses`：优先严格解析 `ps -Aceo`（父 PID 可用），
/// 失败回退 `ps aux`。
pub fn collect_processes() -> Vec<ProcessInfo> {
    if let Ok(out) = super::run_cmd(
        "ps",
        &["-Aceo", "pid=,ppid=,state=,pcpu=,pmem=,rss=,comm=", "-r"],
        Duration::from_secs(3),
    ) {
        if let Ok(procs) = parse_process_output_strict(&out) {
            return procs;
        }
    }
    match super::run_cmd("ps", &["aux"], Duration::from_secs(3)) {
        Ok(out) => parse_ps_aux_output_strict(&out).unwrap_or_default(),
        Err(_) => Vec::new(),
    }
}

/// 1:1 移植 `parseProcessOutputStrict`。
pub fn parse_process_output_strict(raw: &str) -> Result<Vec<ProcessInfo>, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("empty ps process table".into());
    }

    let mut procs = Vec::with_capacity(trimmed.lines().count());
    for row in trimmed.lines() {
        let fields: Vec<&str> = row.split_whitespace().collect();
        if fields.len() < 7 || !is_process_state_token(fields[2]) {
            return Err("unexpected ps process row".into());
        }
        let pid: i64 = fields[0]
            .parse()
            .map_err(|_| "unexpected ps process row".to_string())?;
        let ppid: i64 = fields[1]
            .parse()
            .map_err(|_| "unexpected ps process row".to_string())?;
        let cpu: f64 = fields[3]
            .parse()
            .map_err(|_| "unexpected ps process row".to_string())?;
        let mem: f64 = fields[4]
            .parse()
            .map_err(|_| "unexpected ps process row".to_string())?;
        let rss_kb: u64 = fields[5]
            .parse()
            .map_err(|_| "unexpected ps process row".to_string())?;
        let command = fields[6..].join(" ");
        if pid <= 0 || ppid < 0 || command.is_empty() {
            return Err("unexpected ps process row".into());
        }
        procs.push(ProcessInfo {
            pid,
            ppid,
            state: fields[2].to_string(),
            name: process_name_from_comm(command.trim_end()),
            command,
            cpu,
            memory: mem,
            memory_bytes: rss_kb * 1024,
        });
    }
    Ok(procs)
}

/// 1:1 移植 `parsePsAuxOutputStrict`。
pub fn parse_ps_aux_output_strict(raw: &str) -> Result<Vec<ProcessInfo>, String> {
    let trimmed = raw.trim();
    let rows: Vec<&str> = trimmed.lines().collect();
    if rows.len() < 2 {
        return Err("unexpected ps aux header".into());
    }
    let expected_header: &[&str] = &[
        "USER", "PID", "%CPU", "%MEM", "VSZ", "RSS", "TT", "STAT", "STARTED", "TIME", "COMMAND",
    ];
    if rows[0].split_whitespace().collect::<Vec<_>>() != expected_header {
        return Err("unexpected ps aux header".into());
    }

    let mut procs = Vec::with_capacity(rows.len() - 1);
    for row in &rows[1..] {
        let fields: Vec<&str> = row.split_whitespace().collect();
        if fields.len() < 11 || !is_process_state_token(fields[7]) {
            return Err("unexpected ps aux process row".into());
        }
        let pid: i64 = fields[1]
            .parse()
            .map_err(|_| "unexpected ps aux process row".to_string())?;
        let cpu: f64 = fields[2]
            .parse()
            .map_err(|_| "unexpected ps aux process row".to_string())?;
        let mem: f64 = fields[3]
            .parse()
            .map_err(|_| "unexpected ps aux process row".to_string())?;
        fields[4]
            .parse::<u64>()
            .map_err(|_| "unexpected ps aux process row".to_string())?;
        let rss_kb: u64 = fields[5]
            .parse()
            .map_err(|_| "unexpected ps aux process row".to_string())?;
        let command = fields[10..].join(" ");
        if pid <= 0 || command.is_empty() {
            return Err("unexpected ps aux process row".into());
        }
        procs.push(ProcessInfo {
            pid,
            ppid: 0,
            state: fields[7].to_string(),
            name: process_name_from_command(&command),
            command,
            cpu,
            memory: mem,
            memory_bytes: rss_kb * 1024,
        });
    }
    Ok(procs)
}

/// 1:1 移植 `isProcessStateToken`：Darwin 可能临时上报 "?"，
/// 保留该行以免瞬态状态使整个采样失效。
pub fn is_process_state_token(state: &str) -> bool {
    if state.is_empty() {
        return false;
    }
    let first = state.as_bytes()[0] as char;
    if !"?DIRSTUWZ".contains(first) {
        return false;
    }
    state.chars().skip(1).all(|m| "+<>AELNSsVWX".contains(m))
}

/// 1:1 移植 `isZombieState`。
pub fn is_zombie_state(state: &str) -> bool {
    state.trim().starts_with('Z')
}

/// 1:1 移植 `summarizeZombies`：返回 (数量, Top 父进程, 是否完整)。
pub fn summarize_zombies(
    processes: &[ProcessInfo],
    limit: usize,
    parents_available: bool,
) -> (i64, Vec<ZombieParent>, bool) {
    let by_pid: std::collections::HashMap<i64, &ProcessInfo> =
        processes.iter().map(|p| (p.pid, p)).collect();

    let mut count = 0i64;
    let mut complete = parents_available;
    let mut by_parent: std::collections::HashMap<i64, i64> = std::collections::HashMap::new();
    for proc in processes {
        if !is_zombie_state(&proc.state) {
            continue;
        }
        count += 1;
        if !parents_available {
            continue;
        }
        if proc.ppid <= 0 {
            complete = false;
            continue;
        }
        // 对标：!ok || parent.Name == "" → incomplete。
        match by_pid.get(&proc.ppid) {
            Some(p) if !p.name.is_empty() => {}
            _ => {
                complete = false;
                continue;
            }
        }
        *by_parent.entry(proc.ppid).or_insert(0) += 1;
    }

    let mut parents: Vec<ZombieParent> = by_parent
        .iter()
        .map(|(&ppid, &zombie_count)| ZombieParent {
            pid: ppid,
            name: by_pid.get(&ppid).map(|p| p.name.clone()).unwrap_or_default(),
            count: zombie_count,
        })
        .collect();
    parents.sort_by(|a, b| {
        b.count
            .cmp(&a.count)
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.pid.cmp(&b.pid))
    });
    if limit == 0 {
        if !parents.is_empty() {
            complete = false;
        }
        return (count, Vec::new(), complete);
    }
    if parents.len() > limit {
        parents.truncate(limit);
        complete = false;
    }
    (count, parents, complete)
}

/// 1:1 移植 `processNameFromComm`。
fn process_name_from_comm(command: &str) -> String {
    let name = match command.rfind('/') {
        Some(idx) => &command[idx + 1..],
        None => command,
    };
    name.trim().to_string()
}

/// 1:1 移植 `processNameFromCommand`。
fn process_name_from_command(command: &str) -> String {
    let name = match command.rfind('/') {
        Some(idx) => &command[idx + 1..],
        None => command,
    };
    match name.find(' ') {
        Some(idx) => name[..idx].to_string(),
        None => name.to_string(),
    }
}

/// 对标 `topProcesses`（Go 用最小堆取 TopN；排序取前 N 结果等价：
/// processRanksBefore 为 (CPU 降序, 内存降序, PID 升序) 全序）。
pub fn top_processes(processes: &[ProcessInfo], limit: usize) -> Vec<ProcessInfo> {
    if limit == 0 || processes.is_empty() {
        return Vec::new();
    }
    let mut sorted: Vec<ProcessInfo> = processes.to_vec();
    sorted.sort_by(|a, b| {
        b.cpu
            .partial_cmp(&a.cpu)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| {
                b.memory
                    .partial_cmp(&a.memory)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .then_with(|| a.pid.cmp(&b.pid))
    });
    sorted.truncate(limit);
    sorted
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proc(pid: i64, ppid: i64, state: &str, name: &str, cpu: f64) -> ProcessInfo {
        ProcessInfo {
            pid,
            ppid,
            state: state.into(),
            name: name.into(),
            command: name.into(),
            cpu,
            memory: 0.0,
            memory_bytes: 0,
        }
    }

    /// 对标 metrics_process_test.go 的 ps 严格解析用例。
    #[test]
    fn parse_strict_ps_output() {
        let raw = "  1     0 Ss   0.0  0.1   256 /sbin/launchd\n  42     1 Zs  12.5  1.0  5120 /usr/bin/tool\n";
        let procs = parse_process_output_strict(raw).unwrap();
        assert_eq!(procs.len(), 2);
        assert_eq!(procs[0].pid, 1);
        assert_eq!(procs[0].ppid, 0);
        assert_eq!(procs[0].name, "launchd");
        assert_eq!(procs[1].state, "Zs");
        assert_eq!(procs[1].memory_bytes, 5120 * 1024);
        assert!(parse_process_output_strict("garbage").is_err());
        assert!(parse_process_output_strict("").is_err());
    }

    #[test]
    fn parse_ps_aux_strict() {
        let raw = "USER   PID  %CPU %MEM    VSZ   RSS TT  STAT STARTED      TIME COMMAND\nroot    42  12.5  1.0  20480  5120  ??  Zs  Mon01AM  0:00.01 /usr/bin/tool arg\n";
        let procs = parse_ps_aux_output_strict(raw).unwrap();
        assert_eq!(procs.len(), 1);
        assert_eq!(procs[0].pid, 42);
        assert_eq!(procs[0].ppid, 0);
        assert_eq!(procs[0].name, "tool");
        assert_eq!(procs[0].command, "/usr/bin/tool arg");
        // 头部不符必须报错。
        assert!(parse_ps_aux_output_strict("USER PID COMMAND\n1 2 sh").is_err());
    }

    /// 对标 isProcessStateToken 用例。
    #[test]
    fn state_tokens() {
        assert!(is_process_state_token("S"));
        assert!(is_process_state_token("Ss"));
        assert!(is_process_state_token("Z"));
        assert!(is_process_state_token("?"));
        assert!(!is_process_state_token(""));
        assert!(!is_process_state_token("Xy")); // y 非法修饰符
        assert!(!is_process_state_token("Q"));
    }

    /// 对标 summarizeZombies 用例。
    #[test]
    fn zombie_summary() {
        let procs = vec![
            proc(1, 0, "Ss", "launchd", 0.0),
            proc(10, 1, "Z", "worker-a", 0.0),
            proc(11, 1, "Z", "worker-b", 0.0),
            proc(12, 2, "Z", "other", 0.0),
            proc(2, 0, "Ss", "init", 0.0),
        ];
        let (count, parents, complete) = summarize_zombies(&procs, ZOMBIE_PARENT_LIMIT, true);
        assert_eq!(count, 3);
        assert!(complete);
        assert_eq!(parents.len(), 2);
        assert_eq!(parents[0].pid, 1);
        assert_eq!(parents[0].count, 2);
        assert_eq!(parents[0].name, "launchd");
        // limit=0 时返回空列表并标记不完整。
        let (_, parents, complete) = summarize_zombies(&procs, 0, true);
        assert!(parents.is_empty());
        assert!(!complete);
        // 父 PID 不可用时 complete=false。
        let orphans = vec![proc(10, 0, "Z", "w", 0.0)];
        let (count, parents, complete) = summarize_zombies(&orphans, 3, false);
        assert_eq!(count, 1);
        assert!(parents.is_empty());
        assert!(!complete);
    }

    /// 对标 topProcesses 用例：CPU 优先，其次内存，最后 PID。
    #[test]
    fn top_process_ranking() {
        let procs = vec![
            proc(3, 1, "S", "c", 10.0),
            proc(1, 1, "S", "a", 30.0),
            proc(2, 1, "S", "b", 30.0),
            proc(4, 1, "S", "d", 5.0),
        ];
        let top = top_processes(&procs, 2);
        assert_eq!(top.len(), 2);
        assert_eq!(top[0].pid, 1); // 同 CPU 取小 PID
        assert_eq!(top[1].pid, 2);
    }

    #[test]
    fn name_extraction() {
        // processNameFromComm：取最后一段路径。
        // processNameFromCommand：再截断到第一个空格。
        let raw = "  99     1 Ss   0.0  0.1   256 /Applications/Foo.app/Contents/MacOS/Foo Helper --flag\n";
        let procs = parse_process_output_strict(raw).unwrap();
        assert_eq!(procs[0].name, "Foo Helper --flag"); // comm 未截断空格
        assert_eq!(procs[0].command, "/Applications/Foo.app/Contents/MacOS/Foo Helper --flag");
    }
}
