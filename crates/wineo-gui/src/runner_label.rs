//! How runners from Steam, Heroic, and Lutris are told apart from wineo's
//! own, and the one-time question about using them.

use std::fmt;

use iced::widget::{Space, column, row, text};
use iced::{Center, Element, Fill};
use wineo_core::config::GlobalConfig;
use wineo_core::external_runners::Source;
use wineo_core::paths::Paths;
use wineo_core::runner::Runner;

use crate::theme::Palette;
use crate::ui;

const OFFER: &str = "Use Proton and Wine builds from Steam, Heroic, and Lutris too?";
const NO_ID: &str = "external-runners-no";
const YES_ID: &str = "external-runners-yes";

/// The launcher that owns a runner, or None for wineo's own builds. A
/// runners root inside a launcher folder makes its builds that launcher's
/// too. It reads the disk, so call it from a worker, not a view.
pub(crate) fn launcher(paths: &Paths, runner: &Runner) -> Option<Source> {
    match runner.owner(paths) {
        Source::Wineo => None,
        other => Some(other),
    }
}

/// `name`, followed by its launcher when it isn't wineo's, like
/// `GE-Proton-latest (Heroic)`.
pub(crate) fn labeled(name: &str, launcher: Option<Source>) -> String {
    match launcher {
        Some(launcher) => format!("{name} ({launcher})"),
        None => name.to_owned(),
    }
}

/// `text` with a note in parentheses, plus the launcher when it isn't
/// wineo's, like `proton (GE-Proton11-3, Steam)`.
pub(crate) fn noted(text: &str, note: &str, launcher: Option<Source>) -> String {
    match launcher {
        Some(launcher) => format!("{text} ({note}, {launcher})"),
        None => format!("{text} ({note})"),
    }
}

/// A review's runner: the name as configured, then the build it resolved
/// to and its launcher. A build named as itself only needs the launcher.
pub(crate) fn resolved(paths: &Paths, runner: &Runner) -> String {
    match launcher(paths, runner) {
        Some(launcher) if runner.selection() == runner.name() => {
            labeled(runner.name(), Some(launcher))
        }
        launcher => noted(runner.selection(), runner.name(), launcher),
    }
}

/// A runner name in a picker, shown with its launcher.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Choice {
    pub(crate) name: String,
    pub(crate) launcher: Option<Source>,
}

impl Choice {
    pub(crate) fn plain(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            launcher: None,
        }
    }

    pub(crate) fn of(paths: &Paths, runner: &Runner) -> Self {
        Self {
            name: runner.selection().to_owned(),
            launcher: launcher(paths, runner),
        }
    }

    /// The choice named `name`, or a plain one for a typed name the list
    /// doesn't have.
    pub(crate) fn find(choices: &[Choice], name: &str) -> Self {
        choices
            .iter()
            .find(|choice| choice.name == name)
            .cloned()
            .unwrap_or_else(|| Self::plain(name))
    }
}

impl fmt::Display for Choice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&labeled(&self.name, self.launcher))
    }
}

/// Whether to ask about other launchers' builds: only until it's answered,
/// and never in a portable folder, which doesn't use them.
pub(crate) fn offer(paths: &Paths, global: &GlobalConfig) -> bool {
    global.external_runners.is_none() && paths.portable_root().is_none()
}

pub(crate) fn answer(paths: &Paths, on: bool) -> Result<(), String> {
    wineo_core::settings::set_external_runners(paths, on).map_err(|error| format!("{error:#}"))
}

/// The question, with its buttons off while an answer is being saved.
pub(crate) fn offer_card<'a, M: Clone + 'static>(
    palette: Palette,
    enabled: bool,
    on_answer: impl Fn(bool) -> M,
) -> Element<'a, M> {
    let no = on_answer(false);
    let yes = on_answer(true);
    ui::section(
        column![
            text(OFFER).size(14).color(palette.foreground),
            row![
                Space::new().width(Fill),
                crate::focus_button::control(
                    ui::secondary_button("No", palette)
                        .on_press_maybe(enabled.then(|| no.clone()))
                        .into(),
                    enabled,
                    NO_ID,
                    no,
                ),
                crate::focus_button::control(
                    ui::accent_button("Yes", palette.primary, enabled, yes.clone()),
                    enabled,
                    YES_ID,
                    yes,
                ),
            ]
            .spacing(8)
            .align_y(Center),
        ]
        .spacing(12),
        palette,
    )
    .into()
}

/// A Proton build in a fake Steam's compatibility tools under `home`.
#[cfg(test)]
pub(crate) fn fake_steam_build(home: &std::path::Path, name: &str) -> std::path::PathBuf {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    let dir = home.join(".steam/root/compatibilitytools.d").join(name);
    for program in ["proton", "files/bin/wine", "files/bin/wineserver"] {
        let file = dir.join(program);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(&file, "#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(file, fs::Permissions::from_mode(0o755)).unwrap();
    }
    dir
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_other_launchers_get_a_label() {
        assert_eq!(
            labeled("GE-Proton-latest", Some(Source::Heroic)),
            "GE-Proton-latest (Heroic)"
        );
        assert_eq!(labeled("GE-Proton11-7", None), "GE-Proton11-7");
        let choices = [Choice {
            name: "GE-Proton11-3".to_owned(),
            launcher: Some(Source::Steam),
        }];
        assert_eq!(
            Choice::find(&choices, "GE-Proton11-3").to_string(),
            "GE-Proton11-3 (Steam)"
        );
        assert_eq!(Choice::find(&choices, "typed").to_string(), "typed");
        assert_eq!(
            noted("proton", "GE-Proton11-3", Some(Source::Steam)),
            "proton (GE-Proton11-3, Steam)"
        );
        assert_eq!(noted("fake", "fake", None), "fake (fake)");
    }
}
