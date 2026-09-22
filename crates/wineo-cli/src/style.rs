//! Color and formatting helpers for command output.
//!
//! Colors are checked against the real terminal every call, through
//! `anstream`, so they are off when stdout isn't a terminal, off with
//! `NO_COLOR`, and forced on with `CLICOLOR_FORCE`.

use std::fmt;
use std::path::Path;

use anstyle::{AnsiColor, Color, Effects, Style};
use wineo_core::paths::Paths;

const TITLE: Style = Style::new().effects(Effects::BOLD);
const WARNING: Style = Style::new().fg_color(Some(Color::Ansi(AnsiColor::Yellow)));
const ERROR: Style = Style::new().fg_color(Some(Color::Ansi(AnsiColor::Red)));
const DIM: Style = Style::new().effects(Effects::DIMMED);

/// Whether output should carry ANSI styling right now. Stdout and stderr are
/// treated the same, since wineo never has a real terminal on one and a pipe
/// on the other.
pub fn enabled() -> bool {
    anstream::AutoStream::choice(&std::io::stdout()) != anstream::ColorChoice::Never
}

/// A bold section title.
pub fn title(text: impl fmt::Display) -> String {
    paint(TITLE, text)
}

/// A yellow warning.
pub fn warning(text: impl fmt::Display) -> String {
    paint(WARNING, text)
}

/// A red error.
pub fn error(text: impl fmt::Display) -> String {
    paint(ERROR, text)
}

/// Dimmed secondary detail.
pub fn dim(text: impl fmt::Display) -> String {
    paint(DIM, text)
}

fn paint(style: Style, text: impl fmt::Display) -> String {
    if enabled() {
        format!("{}{text}{}", style.render(), style.render_reset())
    } else {
        text.to_string()
    }
}

/// A path, contracted under `$HOME` to `~` and dimmed as secondary detail.
pub fn path(paths: &Paths, path: &Path) -> String {
    let text = paths
        .contract(path)
        .unwrap_or_else(|_| path.display().to_string());
    dim(text)
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn size_uses_binary_units() {
        assert_eq!(size(0), "0 B");
        assert_eq!(size(999), "999 B");
        assert_eq!(size(1024), "1 KiB");
        assert_eq!(size(320 * 1024 * 1024), "320 MiB");
        assert_eq!(size(1536 * 1024), "1.5 MiB");
        assert_eq!(size(5 * 1024 * 1024 * 1024), "5 GiB");
    }

    #[test]
    fn styling_is_off_when_not_a_terminal() {
        // Test binaries never run with a real terminal on stdout.
        assert_eq!(title("Heading"), "Heading");
        assert_eq!(warning("careful"), "careful");
        assert_eq!(error("failed"), "failed");
        assert_eq!(dim("detail"), "detail");
    }

    #[test]
    fn path_contracts_under_home() {
        let paths =
            Paths::from_lookup(|key| (key == "HOME").then(|| std::ffi::OsString::from("/home/me")))
                .unwrap();
        assert_eq!(path(&paths, Path::new("/home/me/Games/x")), "~/Games/x");
    }
}
