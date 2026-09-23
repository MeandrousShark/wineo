//! Color and formatting helpers for command output.
//!
//! Colors follow the destination stream through `anstream`, including
//! `NO_COLOR` and `CLICOLOR_FORCE`.

use std::fmt;
use std::path::Path;

use anstyle::{AnsiColor, Color, Effects, Style};
use wineo_core::paths::Paths;

const TITLE: Style = Style::new().effects(Effects::BOLD);
const WARNING: Style = Style::new().fg_color(Some(Color::Ansi(AnsiColor::Yellow)));
const ERROR: Style = Style::new().fg_color(Some(Color::Ansi(AnsiColor::Red)));
const DIM: Style = Style::new().effects(Effects::DIMMED);

/// Whether stdout should carry ANSI styling right now.
pub fn stdout_enabled() -> bool {
    anstream::AutoStream::choice(&std::io::stdout()) != anstream::ColorChoice::Never
}

/// Whether stderr should carry ANSI styling right now.
pub fn stderr_enabled() -> bool {
    anstream::AutoStream::choice(&std::io::stderr()) != anstream::ColorChoice::Never
}

/// A bold section title styled for stdout.
pub fn title(text: impl fmt::Display) -> String {
    title_with_color(text, stdout_enabled())
}

/// A yellow warning styled for stdout.
pub fn warning(text: impl fmt::Display) -> String {
    warning_with_color(text, stdout_enabled())
}

/// A yellow warning styled for stderr.
pub fn warning_stderr(text: impl fmt::Display) -> String {
    warning_with_color(text, stderr_enabled())
}

/// A red error styled for stdout.
pub fn error(text: impl fmt::Display) -> String {
    error_with_color(text, stdout_enabled())
}

/// A red error styled for stderr.
pub fn error_stderr(text: impl fmt::Display) -> String {
    error_with_color(text, stderr_enabled())
}

/// Dimmed secondary detail styled for stderr.
pub fn dim_stderr(text: impl fmt::Display) -> String {
    dim_with_color(text, stderr_enabled())
}

/// A title styled according to an explicit color setting.
pub fn title_with_color(text: impl fmt::Display, enabled: bool) -> String {
    paint(TITLE, text, enabled)
}

/// A warning styled according to an explicit color setting.
pub fn warning_with_color(text: impl fmt::Display, enabled: bool) -> String {
    paint(WARNING, text, enabled)
}

/// An error styled according to an explicit color setting.
pub fn error_with_color(text: impl fmt::Display, enabled: bool) -> String {
    paint(ERROR, text, enabled)
}

/// Dimmed detail styled according to an explicit color setting.
pub fn dim_with_color(text: impl fmt::Display, enabled: bool) -> String {
    paint(DIM, text, enabled)
}

fn paint(style: Style, text: impl fmt::Display, enabled: bool) -> String {
    if enabled {
        format!("{}{text}{}", style.render(), style.render_reset())
    } else {
        text.to_string()
    }
}

/// A path, contracted under `$HOME` to `~` and dimmed as stdout detail.
pub fn path(paths: &Paths, path: &Path) -> String {
    path_with_color(paths, path, stdout_enabled())
}

/// A path, contracted under `$HOME` to `~` and dimmed as stderr detail.
pub fn path_stderr(paths: &Paths, path: &Path) -> String {
    path_with_color(paths, path, stderr_enabled())
}

/// A path, contracted under `$HOME` to `~` and dimmed when `enabled` is true.
pub fn path_with_color(paths: &Paths, path: &Path, enabled: bool) -> String {
    let text = paths
        .contract(path)
        .unwrap_or_else(|_| path.display().to_string());
    dim_with_color(text, enabled)
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
    use std::fs::{File, OpenOptions};
    use std::io::Read;
    use std::os::unix::fs::OpenOptionsExt;
    use std::process::{Command, Stdio};

    use rustix::fs::OFlags;
    use rustix::pty::{OpenptFlags, grantpt, openpt, ptsname, unlockpt};

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
        assert_eq!(dim_with_color("detail", false), "detail");
    }

    #[test]
    fn explicit_color_setting_controls_prompt_style() {
        assert_eq!(title_with_color("Heading", false), "Heading");
        assert!(title_with_color("Heading", true).contains('\u{1b}'));
    }

    #[test]
    fn path_contracts_under_home() {
        let paths =
            Paths::from_lookup(|key| (key == "HOME").then(|| std::ffi::OsString::from("/home/me")))
                .unwrap();
        assert_eq!(path(&paths, Path::new("/home/me/Games/x")), "~/Games/x");
        assert_eq!(
            path_with_color(&paths, Path::new("/home/me/Games/x"), false),
            "~/Games/x"
        );
    }

    #[test]
    fn explicit_plain_styles_support_uncolored_logs() {
        assert_eq!(title_with_color("Heading", false), "Heading");
        assert_eq!(warning_with_color("careful", false), "careful");
        assert_eq!(error_with_color("failed", false), "failed");
        assert_eq!(dim_with_color("detail", false), "detail");
    }

    #[test]
    fn stdout_and_stderr_color_choices_follow_each_stream() {
        for (stdout_terminal, stderr_terminal) in
            [(false, false), (true, false), (false, true), (true, true)]
        {
            let (stdout, stderr) = run_color_probe(stdout_terminal, stderr_terminal, false, false);
            assert_eq!(probe_value(&stdout, "STDOUT_COLOR"), stdout_terminal);
            assert_eq!(probe_value(&stderr, "STDERR_COLOR"), stderr_terminal);
        }
    }

    #[test]
    fn no_color_disables_both_terminal_streams() {
        let (stdout, stderr) = run_color_probe(true, true, true, false);
        assert!(!probe_value(&stdout, "STDOUT_COLOR"));
        assert!(!probe_value(&stderr, "STDERR_COLOR"));
    }

    #[test]
    fn color_force_enables_pipe_streams_unless_no_color_is_set() {
        let (stdout, stderr) = run_color_probe(false, false, false, true);
        assert!(probe_value(&stdout, "STDOUT_COLOR"));
        assert!(probe_value(&stderr, "STDERR_COLOR"));

        let (stdout, stderr) = run_color_probe(false, false, true, true);
        assert!(!probe_value(&stdout, "STDOUT_COLOR"));
        assert!(!probe_value(&stderr, "STDERR_COLOR"));
    }

    #[test]
    fn style_stream_probe() {
        if std::env::var_os("WINEO_STYLE_STREAM_PROBE").is_none() {
            return;
        }
        println!("STDOUT_COLOR={}", stdout_enabled());
        eprintln!("STDERR_COLOR={}", stderr_enabled());
    }

    fn run_color_probe(
        stdout_terminal: bool,
        stderr_terminal: bool,
        no_color: bool,
        force_color: bool,
    ) -> (Vec<u8>, Vec<u8>) {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args(["--exact", "style::tests::style_stream_probe", "--nocapture"])
            .env("WINEO_STYLE_STREAM_PROBE", "1")
            .env("TERM", "xterm-256color")
            .env_remove("CI")
            .env_remove("CLICOLOR")
            .env_remove("CLICOLOR_FORCE")
            .env_remove("NO_COLOR");
        if no_color {
            command.env("NO_COLOR", "1");
        }
        if force_color {
            command.env("CLICOLOR_FORCE", "1");
        }

        let stdout_pty = if stdout_terminal {
            let (master, slave) = open_terminal();
            command.stdout(Stdio::from(slave));
            Some(master)
        } else {
            command.stdout(Stdio::piped());
            None
        };
        let stderr_pty = if stderr_terminal {
            let (master, slave) = open_terminal();
            command.stderr(Stdio::from(slave));
            Some(master)
        } else {
            command.stderr(Stdio::piped());
            None
        };

        let output = command.output().unwrap();
        drop(command);
        assert!(output.status.success(), "child test failed: {output:?}");
        let stdout = stdout_pty.map_or(output.stdout, read_terminal);
        let stderr = stderr_pty.map_or(output.stderr, read_terminal);
        (stdout, stderr)
    }

    fn open_terminal() -> (File, File) {
        let master = openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY).unwrap();
        grantpt(&master).unwrap();
        unlockpt(&master).unwrap();
        let name = ptsname(&master, Vec::new()).unwrap();
        let slave = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(OFlags::NOCTTY.bits() as i32)
            .open(name.to_str().unwrap())
            .unwrap();
        (File::from(master), slave)
    }

    fn read_terminal(mut terminal: File) -> Vec<u8> {
        let mut output = Vec::new();
        let mut buffer = [0; 4096];
        loop {
            match terminal.read(&mut buffer) {
                Ok(0) => break,
                Ok(read) => output.extend_from_slice(&buffer[..read]),
                // A PTY master reports EIO after its last slave is closed.
                Err(error) if error.raw_os_error() == Some(5) => break,
                Err(error) => panic!("could not read PTY output: {error}"),
            }
        }
        output
    }

    fn probe_value(output: &[u8], key: &str) -> bool {
        String::from_utf8_lossy(output)
            .lines()
            .find_map(|line| line.split_once(&format!("{key}=")))
            .and_then(|(_, value)| value.trim().parse().ok())
            .unwrap_or_else(|| panic!("missing {key} in {:?}", String::from_utf8_lossy(output)))
    }
}
