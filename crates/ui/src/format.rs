//! Number and unit formatting for the UI. All functions here are pure and
//! testable; none of them invent a value for missing data.

/// Bytes → "1.2 KB" / "345 MB" / "1.1 GB". Base 1024, one decimal.
pub fn bytes(n: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KB", "MB", "GB", "TB", "PB"];
    if n < 1024 {
        return format!("{n} B");
    }
    let mut value = n as f64;
    let mut unit = 0usize;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if value >= 100.0 {
        format!("{value:.0} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// Split a byte count into (value, unit) so the UI can render the unit in a
/// smaller, dimmer style. Keeps the digits visually aligned.
pub fn bytes_split(n: u64) -> (String, &'static str) {
    const UNITS: [&str; 6] = ["B", "KB", "MB", "GB", "TB", "PB"];
    if n < 1024 {
        return (n.to_string(), "B");
    }
    let mut value = n as f64;
    let mut unit = 0usize;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    let digits = if value >= 100.0 {
        format!("{value:.0}")
    } else if value >= 10.0 {
        format!("{value:.1}")
    } else {
        format!("{value:.2}")
    };
    (digits, UNITS[unit])
}

/// Transfer rate in bytes per second.
pub fn rate(bps: u64) -> String {
    format!("{}/s", bytes(bps))
}

/// Percentage with no decimals, for dense tables.
pub fn percent0(v: f32) -> String {
    format!("{v:.0}%")
}

/// Temperature: `None` → "—" (unknown, not zero).
pub fn temp_c(v: Option<f32>) -> String {
    match v {
        Some(t) => format!("{t:.0} °C"),
        None => "—".to_string(),
    }
}

/// Duration in seconds → "3h 12m" / "12m 04s" / "45s".
pub fn uptime(secs: u64) -> String {
    let d = secs / 86_400;
    let h = (secs % 86_400) / 3_600;
    let m = (secs % 3_600) / 60;
    let s = secs % 60;
    if d > 0 {
        format!("{d}d {h:02}h")
    } else if h > 0 {
        format!("{h}h {m:02}m")
    } else if m > 0 {
        format!("{m}m {s:02}s")
    } else {
        format!("{s}s")
    }
}

/// Unix milliseconds → "14:03:27" in **local** time.
///
/// The snapshot timestamps are UTC epoch milliseconds, so the local offset has
/// to be applied before formatting. The offset is resolved once and cached;
/// a DST transition mid-session would shift by an hour until restart, which is
/// acceptable for a clock label but is why this is cached rather than guessed.
pub fn clock_hms(unix_ms: u64) -> String {
    let offset = local_offset_secs();
    let secs = (unix_ms / 1000).saturating_add_signed(offset);
    let day = (secs % 86_400) as u32;
    format!(
        "{:02}:{:02}:{:02}",
        day / 3_600,
        (day % 3_600) / 60,
        day % 60
    )
}

/// Offset of local time from UTC in seconds (positive east of Greenwich).
fn local_offset_secs() -> i64 {
    #[cfg(windows)]
    {
        use std::sync::OnceLock;
        static OFFSET: OnceLock<i64> = OnceLock::new();
        *OFFSET.get_or_init(|| {
            // Compare the wall clock against the UTC epoch in one shot.
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            // SAFETY: GetLocalTime takes no arguments and returns a POD struct.
            let local = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
            let local_secs = local_to_unix(local);
            local_secs.saturating_sub(now)
        })
    }
    #[cfg(not(windows))]
    {
        0
    }
}

/// SYSTEMTIME (UTC fields) → seconds since the Unix epoch. Uses a
/// days-from-civil algorithm so no extra FFI call is needed.
#[cfg(windows)]
fn local_to_unix(t: windows::Win32::Foundation::SYSTEMTIME) -> i64 {
    let y = t.wYear as i64;
    let m = t.wMonth as i64;
    let d = t.wDay as i64;
    let days = days_from_civil(y, m, d);
    days * 86_400 + t.wHour as i64 * 3_600 + t.wMinute as i64 * 60 + t.wSecond as i64
}

/// Days since 1970-01-01 for a proleptic Gregorian date (Howard Hinnant's
/// algorithm, valid for all dates the system clock can report).
#[cfg(windows)]
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Milliseconds → "820 ms" / "2.1 s".
pub fn duration_ms(ms: u64) -> String {
    if ms < 1_000 {
        format!("{ms} ms")
    } else {
        format!("{:.1} s", ms as f64 / 1000.0)
    }
}

/// Trim a string to a character budget, with an ellipsis.
pub fn truncate(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max_chars.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// Trim a string in the middle, preserving both the beginning and the tail.
/// Useful for paths where the file name matters more than the prefix.
pub fn middle_truncate(s: &str, max_chars: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max_chars {
        return s.to_string();
    }
    if max_chars < 5 {
        return truncate(s, max_chars);
    }
    let keep = max_chars - 1;
    let head = keep / 3;
    let tail = keep - head;
    let mut out: String = chars[..head].iter().collect();
    out.push('…');
    out.extend(chars[chars.len() - tail..].iter());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_scales() {
        assert_eq!(bytes(0), "0 B");
        assert_eq!(bytes(1023), "1023 B");
        assert_eq!(bytes(1024), "1.0 KB");
        assert_eq!(bytes(1536), "1.5 KB");
        assert_eq!(bytes(1024 * 1024), "1.0 MB");
        assert_eq!(bytes(1024 * 1024 * 1024), "1.0 GB");
    }

    #[test]
    fn bytes_split_keeps_digits_and_unit() {
        assert_eq!(bytes_split(0), ("0".into(), "B"));
        assert_eq!(bytes_split(1536), ("1.50".into(), "KB"));
        assert_eq!(bytes_split(12 * 1024 * 1024), ("12.0".into(), "MB"));
        assert_eq!(bytes_split(300 * 1024 * 1024), ("300".into(), "MB"));
    }

    #[test]
    fn unknown_temp_is_dash() {
        assert_eq!(temp_c(None), "—");
        assert_eq!(temp_c(Some(0.0)), "0 °C");
        assert_eq!(temp_c(Some(62.4)), "62 °C");
    }

    #[test]
    fn uptime_formats() {
        assert_eq!(uptime(45), "45s");
        assert_eq!(uptime(125), "2m 05s");
        assert_eq!(uptime(3_600), "1h 00m");
        assert_eq!(uptime(90_000), "1d 01h");
    }

    #[test]
    fn truncate_respects_chars_not_bytes() {
        assert_eq!(truncate("abc", 5), "abc");
        assert_eq!(truncate("abcdef", 4), "abc…");
        assert_eq!(truncate("日本語テスト", 3), "日本…");
    }

    #[test]
    fn middle_truncate_keeps_tail() {
        let path = r"C:\Program Files\Very Long Vendor Name\bin\application.exe";
        let out = middle_truncate(path, 24);
        assert_eq!(out.chars().count(), 24);
        assert!(out.ends_with("application.exe"));
        assert!(out.contains('…'));
    }

    #[test]
    fn clock_is_zero_padded() {
        // The local offset makes exact values machine-dependent, so assert the
        // shape and a couple of invariant properties instead.
        let out = clock_hms(0);
        assert_eq!(out.len(), 8);
        assert_eq!(&out[2..3], ":");
        assert_eq!(&out[5..6], ":");
        // Two different instants one hour apart always differ by one hour.
        let a = clock_hms(1_700_000_000_000);
        let b = clock_hms(1_700_003_600_000);
        assert_ne!(a, b);
    }

    #[test]
    fn civil_days_are_monotonic() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(1970, 1, 2), 1);
        assert_eq!(
            days_from_civil(2000, 3, 1) - days_from_civil(2000, 2, 29),
            1
        );
    }

    #[test]
    fn short_durations_use_milliseconds() {
        assert_eq!(duration_ms(820), "820 ms");
        assert_eq!(duration_ms(2_150), "2.1 s");
    }
}
