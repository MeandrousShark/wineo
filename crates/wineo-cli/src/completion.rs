use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::PathBuf;

use clap_complete::CompletionCandidate;
use wineo_core::components::{Builds, Component};
use wineo_core::config::GlobalConfig;
use wineo_core::environment::{self, Environment};
use wineo_core::paths::Paths;
use wineo_core::runners::Runners;
use wineo_core::snapshot;
use wineo_core::trash;

pub fn trash_ids(current: &OsStr) -> Vec<CompletionCandidate> {
    let Some(prefix) = current.to_str() else {
        return Vec::new();
    };
    let (portable, _) = split_portable(words_before_current());
    let Ok(paths) = Paths::from_env(portable.as_deref()) else {
        return Vec::new();
    };
    trash::list(&paths)
        .unwrap_or_default()
        .into_iter()
        .filter(|entry| entry.id.starts_with(prefix))
        .map(|entry| CompletionCandidate::new(entry.id))
        .collect()
}

pub fn removable_build_names(current: &OsStr) -> Vec<CompletionCandidate> {
    let Some(prefix) = current.to_str() else {
        return Vec::new();
    };
    let (portable, words) = split_portable(words_before_current());
    let Some((command, args)) = command_args(&words) else {
        return Vec::new();
    };
    if !args.iter().any(|arg| arg == "remove") {
        return Vec::new();
    }
    let Ok(paths) = Paths::from_env(portable.as_deref()) else {
        return Vec::new();
    };
    let names = match command {
        "runners" => {
            let Ok(global) = GlobalConfig::load(&paths) else {
                return Vec::new();
            };
            let Ok(runners) = Runners::new(&paths, &global) else {
                return Vec::new();
            };
            let Ok(listing) = runners.list() else {
                return Vec::new();
            };
            listing
                .runners
                .into_iter()
                .filter(|runner| {
                    fs::symlink_metadata(runners.root().join(runner.name()))
                        .is_ok_and(|metadata| metadata.file_type().is_dir())
                })
                .map(|runner| runner.name().to_owned())
                .collect::<Vec<_>>()
        }
        "dxvk" | "vkd3d" => {
            let component = if command == "dxvk" {
                Component::Dxvk
            } else {
                Component::Vkd3d
            };
            let Ok(global) = GlobalConfig::load(&paths) else {
                return Vec::new();
            };
            let Ok(builds) = Builds::new(&paths, &global, component) else {
                return Vec::new();
            };
            let Ok(listing) = builds.list() else {
                return Vec::new();
            };
            listing
                .builds
                .into_iter()
                .filter(|build| {
                    fs::symlink_metadata(builds.root().join(build.name()))
                        .is_ok_and(|metadata| metadata.file_type().is_dir())
                })
                .map(|build| build.name().to_owned())
                .collect::<Vec<_>>()
        }
        _ => return Vec::new(),
    };
    names
        .into_iter()
        .filter(|name| name.starts_with(prefix))
        .map(CompletionCandidate::new)
        .collect()
}

pub fn environments(current: &OsStr) -> Vec<CompletionCandidate> {
    let Some(prefix) = current.to_str() else {
        return Vec::new();
    };
    let (portable, words) = split_portable(words_before_current());
    let Some((command, args)) = command_args(&words) else {
        return Vec::new();
    };
    if command == "set" && args.iter().any(|arg| arg == "--global") {
        return Vec::new();
    }
    if command == "winetricks" && !first_winetricks_value(args) {
        return Vec::new();
    }
    let Ok(paths) = Paths::from_env(portable.as_deref()) else {
        return Vec::new();
    };
    environment::names(&paths)
        .unwrap_or_default()
        .into_iter()
        .filter(|name| name.starts_with(prefix))
        .map(CompletionCandidate::new)
        .collect()
}

pub fn tags(current: &OsStr) -> Vec<CompletionCandidate> {
    let Some(prefix) = current.to_str() else {
        return Vec::new();
    };
    let (portable, _) = split_portable(words_before_current());
    let Ok(paths) = Paths::from_env(portable.as_deref()) else {
        return Vec::new();
    };
    environment::tags(&paths)
        .unwrap_or_default()
        .into_iter()
        .filter(|tag| tag.starts_with(prefix))
        .map(CompletionCandidate::new)
        .collect()
}

pub fn targets(current: &OsStr) -> Vec<CompletionCandidate> {
    let Some(prefix) = current.to_str() else {
        return Vec::new();
    };
    let (portable, words) = split_portable(words_before_current());
    let Some((command, args)) = command_args(&words) else {
        return Vec::new();
    };
    if !matches!(command, "start" | "run" | "restart") {
        return Vec::new();
    }
    let Some(name) = selected_environment(args) else {
        return Vec::new();
    };
    let Ok(paths) = Paths::from_env(portable.as_deref()) else {
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

/// Completes `ENV`, and `ENV.TARGET` once a dot is typed.
pub fn spots(current: &OsStr) -> Vec<CompletionCandidate> {
    let Some(prefix) = current.to_str() else {
        return Vec::new();
    };
    let (portable, _) = split_portable(words_before_current());
    let Ok(paths) = Paths::from_env(portable.as_deref()) else {
        return Vec::new();
    };
    let Some((name, target)) = prefix.split_once('.') else {
        return environment::names(&paths)
            .unwrap_or_default()
            .into_iter()
            .filter(|name| name.starts_with(prefix))
            .map(CompletionCandidate::new)
            .collect();
    };
    let Ok(environment) = Environment::load(&paths, name) else {
        return Vec::new();
    };
    environment
        .config
        .targets
        .keys()
        .filter(|candidate| candidate.starts_with(target))
        .map(|candidate| CompletionCandidate::new(format!("{name}.{candidate}")))
        .collect()
}

pub fn snapshots(current: &OsStr) -> Vec<CompletionCandidate> {
    let Some(prefix) = current.to_str() else {
        return Vec::new();
    };
    let (portable, words) = split_portable(words_before_current());
    let Some(("snapshot", args)) = command_args(&words) else {
        return Vec::new();
    };
    let [action, name, ..] = args else {
        return Vec::new();
    };
    if !matches!(action.as_str(), "restore" | "delete") {
        return Vec::new();
    }
    let Ok(paths) = Paths::from_env(portable.as_deref()) else {
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

/// Takes the global `--portable DIR` out of the words, wherever it appears
/// before a `--`, so completion reads the same folder the command would.
fn split_portable(words: Vec<String>) -> (Option<PathBuf>, Vec<String>) {
    let mut portable = None;
    let mut kept = Vec::with_capacity(words.len());
    let mut words = words.into_iter();
    let mut value_next = false;
    while let Some(word) = words.next() {
        if value_next {
            value_next = false;
            kept.push(word);
            continue;
        }
        if word == "--" {
            kept.push(word);
            kept.extend(words);
            break;
        }
        if word == "--portable" {
            if let Some(dir) = words.next() {
                portable = Some(PathBuf::from(dir));
            }
            continue;
        }
        if let Some(dir) = word.strip_prefix("--portable=") {
            portable = Some(PathBuf::from(dir));
            continue;
        }
        value_next = matches!(word.as_str(), "--env" | "-e" | "--target" | "-t");
        kept.push(word);
    }
    (portable, kept)
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
    use std::path::PathBuf;

    use super::{first_winetricks_value, selected_environment, split_portable};

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
    fn portable_is_taken_out_of_the_words() {
        let (portable, words) = split_portable(strings(&[
            "wineo",
            "--portable",
            "/mnt/a",
            "start",
            "--portable=/mnt/b",
            "hades",
        ]));
        assert_eq!(portable, Some(PathBuf::from("/mnt/b")));
        assert_eq!(words, strings(&["wineo", "start", "hades"]));

        let (portable, words) = split_portable(strings(&[
            "wineo",
            "run",
            "-e",
            "--portable",
            "hades",
            "--",
            "--portable",
            "x",
        ]));
        assert_eq!(portable, None);
        assert_eq!(
            words,
            strings(&[
                "wineo",
                "run",
                "-e",
                "--portable",
                "hades",
                "--",
                "--portable",
                "x"
            ])
        );
    }

    #[test]
    fn winetricks_only_completes_its_first_forwarded_value() {
        assert!(first_winetricks_value(&strings(&["-e", "KEY=VALUE", "--"])));
        assert!(!first_winetricks_value(&strings(&["hades"])));
        assert!(!first_winetricks_value(&strings(&["--", "hades", "-q"])));
        assert!(!first_winetricks_value(&strings(&["--", "--env"])));
    }
}
