use std::time::Duration;

pub fn truncate_for_log(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_string();
    }
    let mut end = max_bytes;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…[+{} bytes]", &value[..end], value.len() - end)
}

pub fn humanize_duration(d: Duration) -> String {
    let secs = d.as_secs();
    let days = secs / 86_400;
    let h = (secs % 86_400) / 3_600;
    let m = (secs % 3_600) / 60;
    let s = secs % 60;
    match (days, h, m, s) {
        (0, 0, 0, s) => format!("{s}s"),
        (0, 0, m, 0) => format!("{m}m"),
        (0, 0, m, s) => format!("{m}m {s}s"),
        (0, h, 0, _) => format!("{h}h"),
        (0, h, m, _) => format!("{h}h {m}m"),
        (d, 0, _, _) => format!("{d}d"),
        (d, h, _, _) => format!("{d}d {h}h"),
    }
}
