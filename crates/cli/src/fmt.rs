use humansize::{FormatSizeOptions, BINARY};

/// Sizes are shown in binary units because that is what filesystems allocate in.
pub fn size(bytes: u64) -> String {
    humansize::format_size(bytes, opts())
}

/// Signed size, always with an explicit sign so a list of changes scans well.
pub fn delta(bytes: i64) -> String {
    let sign = if bytes < 0 { '-' } else { '+' };
    format!(
        "{sign}{}",
        humansize::format_size(bytes.unsigned_abs(), opts())
    )
}

fn opts() -> FormatSizeOptions {
    FormatSizeOptions::from(BINARY).decimal_places(1)
}

pub fn count(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(' ');
        }
        out.push(c);
    }
    out
}

pub fn duration(ms: u64) -> String {
    if ms < 1000 {
        format!("{ms} ms")
    } else if ms < 60_000 {
        format!("{:.1} s", ms as f64 / 1000.0)
    } else {
        format!("{} m {} s", ms / 60_000, (ms % 60_000) / 1000)
    }
}

/// Unix seconds as a local-ish `YYYY-MM-DD HH:MM` stamp.
///
/// Deliberately naive: no timezone database, no chrono dependency. Snapshots
/// are compared by id and by relative age, so this is a label, not arithmetic.
pub fn timestamp(unix: i64) -> String {
    let days = unix.div_euclid(86_400);
    let secs = unix.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}",
        secs / 3600,
        (secs % 3600) / 60
    )
}

/// Howard Hinnant's days-from-civil, inverted. Valid for any Gregorian date.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Truncate a path in the middle so long ones still fit a column.
pub fn ellipsize(s: &str, max: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max || max < 6 {
        return s.to_string();
    }
    let keep_end = (max - 1) / 2;
    let keep_start = max - 1 - keep_end;
    let start: String = chars[..keep_start].iter().collect();
    let end: String = chars[chars.len() - keep_end..].iter().collect();
    format!("{start}…{end}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_are_binary_with_one_decimal() {
        assert_eq!(size(0), "0 B");
        assert_eq!(size(1024), "1 KiB");
        assert_eq!(size(1536), "1.5 KiB");
    }

    #[test]
    fn deltas_always_carry_a_sign() {
        assert_eq!(delta(0), "+0 B");
        assert_eq!(delta(2048), "+2 KiB");
        assert_eq!(delta(-2048), "-2 KiB");
    }

    #[test]
    fn counts_are_grouped() {
        assert_eq!(count(7), "7");
        assert_eq!(count(1234), "1 234");
        assert_eq!(count(1_234_567), "1 234 567");
    }

    #[test]
    fn timestamps_render_known_dates() {
        assert_eq!(timestamp(0), "1970-01-01 00:00");
        // 2026-09-06T17:00:00Z
        assert_eq!(timestamp(1_788_714_000), "2026-09-06 17:00");
    }

    #[test]
    fn long_paths_are_shortened_in_the_middle() {
        assert_eq!(ellipsize("short", 10), "short");
        let out = ellipsize("a/very/long/path/that/keeps/going/on", 15);
        assert_eq!(out.chars().count(), 15);
        assert!(out.contains('…'));
    }

    #[test]
    fn durations_switch_units() {
        assert_eq!(duration(250), "250 ms");
        assert_eq!(duration(1500), "1.5 s");
        assert_eq!(duration(65_000), "1 m 5 s");
    }
}
