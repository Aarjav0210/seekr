/// "3 minutes ago", "2 days ago", ...
pub fn relative_time(ts: i64, now: i64) -> String {
    if ts <= 0 {
        return "—".into();
    }
    let d = (now - ts).max(0);
    let (n, unit) = if d < 60 {
        return "just now".into();
    } else if d < 3600 {
        (d / 60, "minute")
    } else if d < 86_400 {
        (d / 3600, "hour")
    } else if d < 30 * 86_400 {
        (d / 86_400, "day")
    } else if d < 365 * 86_400 {
        (d / (30 * 86_400), "month")
    } else {
        (d / (365 * 86_400), "year")
    };
    format!("{n} {unit}{} ago", if n == 1 { "" } else { "s" })
}

pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = bytes as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{bytes} B")
    } else {
        format!("{v:.1} {}", UNITS[i])
    }
}

/// Middle-truncate to `max` chars.
pub fn ellipsize(s: &str, max: usize) -> String {
    let n = s.chars().count();
    if n <= max || max < 5 {
        return s.to_string();
    }
    let keep = max - 1;
    let head = keep / 2;
    let tail = keep - head;
    let h: String = s.chars().take(head).collect();
    let t: String = s.chars().skip(n - tail).collect();
    format!("{h}…{t}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_times() {
        let now = 1_000_000_000;
        assert_eq!(relative_time(0, now), "—");
        assert_eq!(relative_time(now - 30, now), "just now");
        assert_eq!(relative_time(now - 60, now), "1 minute ago");
        assert_eq!(relative_time(now - 7200, now), "2 hours ago");
        assert_eq!(relative_time(now - 3 * 86_400, now), "3 days ago");
        assert_eq!(relative_time(now - 400 * 86_400, now), "1 year ago");
        assert_eq!(relative_time(now + 100, now), "just now", "clock skew");
    }

    #[test]
    fn sizes() {
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(1536), "1.5 KB");
        assert_eq!(human_size(5 * 1024 * 1024 * 1024), "5.0 GB");
    }

    #[test]
    fn ellipsizes_in_the_middle() {
        assert_eq!(ellipsize("short", 10), "short");
        assert_eq!(ellipsize("~/Downloads/offer-letter.pdf", 12), "~/Dow…er.pdf");
        assert_eq!(ellipsize("~/Downloads/offer-letter.pdf", 12).chars().count(), 12);
        assert_eq!(ellipsize("abcdefgh", 4), "abcdefgh", "too narrow to cut");
    }
}
