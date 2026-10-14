//! Human-readable quantities shared by every frontend.

/// Formats a byte count in binary units, e.g. `320 MiB`, rounded to one
/// decimal place.
pub fn size(bytes: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        return format!("{bytes} B");
    }
    let rounded = (value * 10.0).round() / 10.0;
    if rounded.fract() == 0.0 {
        format!("{rounded:.0} {}", UNITS[unit])
    } else {
        format!("{rounded:.1} {}", UNITS[unit])
    }
}

/// Lowercase hex, as checksums and cache keys are written.
pub fn hex(bytes: impl AsRef<[u8]>) -> String {
    bytes
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_use_binary_units() {
        assert_eq!(size(0), "0 B");
        assert_eq!(size(1023), "1023 B");
        assert_eq!(size(1024), "1 KiB");
        assert_eq!(size(1_288_490_189), "1.2 GiB");
        assert_eq!(size(320 * 1024 * 1024), "320 MiB");
        assert_eq!(size(1536 * 1024), "1.5 MiB");
        assert_eq!(size(2_254_857_830), "2.1 GiB");
    }
}
