//! Numbers as a person reads them: sizes, rates, spans of time and clock
//! times. The web page shows them and the activity log records them, so they
//! live outside both.

const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];

/// Bytes at three significant figures. Nobody reads the fourth on a phone.
pub fn bytes(n: u64) -> String {
    if n < 1024 {
        return format!("{n} B");
    }
    let mut value = n as f64 / 1024.0;
    let mut unit = 1;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    let digits = if value >= 100.0 {
        0
    } else if value >= 10.0 {
        1
    } else {
        2
    };
    format!("{value:.digits$} {}", UNITS[unit])
}

/// A transfer rate, or nothing when nothing is moving: `0 B/s` reads as a
/// measurement, and an idle row has not measured anything. Whether that
/// absence is drawn as a dash or as nothing is the caller's decision.
pub fn rate(n: u64) -> Option<String> {
    (n > 0).then(|| format!("{}/s", bytes(n)))
}

/// A span of time in the two largest units it needs. Nobody waiting on a film
/// wants the seconds off a four-hour estimate, and nobody waiting on the last
/// thirty seconds wants them rounded away.
pub fn duration(secs: u64) -> String {
    if secs >= 3600 {
        format!("{}h {}m", secs / 3600, secs % 3600 / 60)
    } else if secs >= 60 {
        format!("{}m {}s", secs / 60, secs % 60)
    } else {
        format!("{secs}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_at_the_unit_boundaries() {
        assert_eq!(bytes(0), "0 B");
        assert_eq!(bytes(1023), "1023 B");
        assert_eq!(bytes(1024), "1.00 KiB");
        assert_eq!(bytes(3 * 1024u64.pow(4)), "3.00 TiB");
    }

    #[test]
    fn a_rate_is_bytes_per_second_and_nothing_when_idle() {
        assert_eq!(rate(0), None);
        assert_eq!(rate(1024).as_deref(), Some("1.00 KiB/s"));
    }

    #[test]
    fn duration_uses_the_two_largest_units_it_needs() {
        assert_eq!(duration(45), "45s");
        assert_eq!(duration(125), "2m 5s");
        assert_eq!(duration(7_320), "2h 2m");
    }
}
