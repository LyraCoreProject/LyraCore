pub struct ProcessUsage {
    pub cpu_ticks: u64,
    pub rss_kib: u64,
    pub peak_rss_kib: u64,
}

pub fn process_usage(pid: u32) -> ProcessUsage {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap();
    let fields: Vec<_> = stat
        .rsplit_once(')')
        .unwrap()
        .1
        .split_whitespace()
        .collect();
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).unwrap();
    let memory = |label| -> u64 {
        status
            .lines()
            .find_map(|line| line.strip_prefix(label))
            .unwrap()
            .split_whitespace()
            .next()
            .unwrap()
            .parse()
            .unwrap()
    };
    ProcessUsage {
        // Linux stat fields 14 and 15 count user and system CPU, including all process threads.
        cpu_ticks: fields[11].parse::<u64>().unwrap() + fields[12].parse::<u64>().unwrap(),
        rss_kib: memory("VmRSS:"),
        peak_rss_kib: memory("VmHWM:"),
    }
}

pub fn native_libraries(pid: u32) -> usize {
    let mappings = std::fs::read_to_string(format!("/proc/{pid}/maps")).unwrap();
    let libraries: std::collections::BTreeSet<_> = mappings
        .lines()
        .filter(|line| {
            let Some((_, path)) = line.split_once('/') else {
                return false;
            };
            std::path::Path::new(path.strip_suffix(" (deleted)").unwrap_or(path))
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("lyracore-native-") && name.ends_with(".so"))
        })
        .filter_map(|line| line.split_whitespace().nth(4))
        .collect();
    libraries.len()
}
