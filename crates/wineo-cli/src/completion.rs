use std::ffi::{OsStr, OsString};

use clap_complete::CompletionCandidate;
use wineo_core::environment::{self, Environment};
use wineo_core::paths::Paths;
use wineo_core::snapshot;

pub fn environments(current: &OsStr) -> Vec<CompletionCandidate> {
    let Some(prefix) = current.to_str() else {
        return Vec::new();
    };
    let words = words_before_current();
    let Some((command, args)) = command_args(&words) else {
        return Vec::new();
    };
    if command == "set" && args.iter().any(|arg| arg == "--global") {
        return Vec::new();
    }
    if command == "winetricks" && !first_winetricks_value(args) {
        return Vec::new();
    }
    let Ok(paths) = Paths::from_env() else {
        return Vec::new();
    };
    environment::names(&paths)
        .unwrap_or_default()
        .into_iter()
        .filter(|name| name.starts_with(prefix))
        .map(CompletionCandidate::new)
        .collect()
}

pub fn targets(current: &OsStr) -> Vec<CompletionCandidate> {
    let Some(prefix) = current.to_str() else {
        return Vec::new();
    };
    let words = words_before_current();
    let Some((command, args)) = command_args(&words) else {
        return Vec::new();
    };
    if !matches!(command, "start" | "run" | "restart") {
        return Vec::new();
    }
    let Some(name) = selected_environment(args) else {
        return Vec::new();
    };
    let Ok(paths) = Paths::from_env() else {
        return Vec::new();
    };
    let Ok(environment) = Environment::load(&paths, name) else {
        return Vec::new();
    };
    environment
        .config
        .targets
        .keys()
        .filter(|target| target.starts_with(prefix))
        .cloned()
        .map(CompletionCandidate::new)
        .collect()
}

pub fn snapshots(current: &OsStr) -> Vec<CompletionCandidate> {
    let Some(prefix) = current.to_str() else {
        return Vec::new();
    };
    let words = words_before_current();
    let Some(("snapshot", args)) = command_args(&words) else {
        return Vec::new();
    };
    let [action, name, ..] = args else {
        return Vec::new();
    };
    if !matches!(action.as_str(), "restore" | "delete") {
        return Vec::new();
    }
    let Ok(paths) = Paths::from_env() else {
        return Vec::new();
    };
    snapshot::list(&paths, name)
        .unwrap_or_default()
        .into_iter()
        .filter(|item| item.label.starts_with(prefix))
        .map(|item| CompletionCandidate::new(item.label))
        .collect()
}

fn words_before_current() -> Vec<String> {
    let args: Vec<OsString> = std::env::args_os().collect();
    let Some(separator) = args.iter().position(|arg| arg == "--") else {
        return Vec::new();
    };
    let words = &args[separator + 1..];
    let index = std::env::var("_CLAP_COMPLETE_INDEX")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or_else(|| words.len().saturating_sub(1));
    words
        .iter()
        .take(index)
        .map(|word| word.to_string_lossy().into_owned())
        .collect()
}

fn command_args(words: &[String]) -> Option<(&str, &[String])> {
    let args = words.get(1..)?.iter().enumerate();
    for (index, word) in args {
        match word.as_str() {
            "-v" | "--verbose" | "--" => continue,
            command if !command.starts_with('-') => return Some((command, &words[index + 2..])),
            _ => return None,
        }
    }
    None
}

fn selected_environment(args: &[String]) -> Option<&str> {
    let mut awaiting_value = false;
    for arg in args {
        if awaiting_value {
            awaiting_value = false;
            continue;
        }
        match arg.as_str() {
            "--" => break,
            "--target" | "-t" | "--env" | "-e" => awaiting_value = true,
            "-v" | "--verbose" => {}
            value if value.starts_with("--target=") || value.starts_with("--env=") => {}
            value if value.starts_with("-t") || value.starts_with("-e") => {}
            value if value.starts_with('-') => return None,
            name => return Some(name),
        }
    }
    None
}

fn first_winetricks_value(args: &[String]) -> bool {
    let mut awaiting_env = false;
    let mut forwarded = false;
    for arg in args {
        if forwarded {
            return false;
        }
        if awaiting_env {
            awaiting_env = false;
            continue;
        }
        match arg.as_str() {
            "--" => forwarded = true,
            "--env" | "-e" => awaiting_env = true,
            "-v" | "--verbose" | "-h" | "--help" => {}
            value if value.starts_with("--env=") || value.starts_with("-e") => {}
            value if value.starts_with('-') => return false,
            _ => return false,
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::{first_winetricks_value, selected_environment};

    fn strings(words: &[&str]) -> Vec<String> {
        words.iter().map(|word| (*word).to_owned()).collect()
    }

    #[test]
    fn selected_environment_skips_options_and_stops_at_passthrough() {
        for args in [
            vec!["hades", "--target"],
            vec!["--env", "KEY=VALUE", "hades", "--target"],
            vec!["-eKEY=VALUE", "--target=game", "hades", "-t"],
        ] {
            let args = strings(&args);
            assert_eq!(selected_environment(&args), Some("hades"));
        }
        for args in [
            vec!["--target"],
            vec!["--env", "KEY=VALUE", "--target"],
            vec!["--", "hades", "--target"],
            vec!["--unknown", "hades", "--target"],
        ] {
            let args = strings(&args);
            assert_eq!(selected_environment(&args), None);
        }
    }

    #[test]
    fn winetricks_only_completes_its_first_forwarded_value() {
        assert!(first_winetricks_value(&strings(&["-e", "KEY=VALUE", "--"])));
        assert!(!first_winetricks_value(&strings(&["hades"])));
        assert!(!first_winetricks_value(&strings(&["--", "hades", "-q"])));
        assert!(!first_winetricks_value(&strings(&["--", "--env"])));
    }
}
