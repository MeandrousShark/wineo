//! How runners from Steam, Heroic, and Lutris are told apart from wineo's
//! own.

use wineo_core::external_runners::Source;
use wineo_core::runner::Runner;

/// The launcher that owns a runner, or None for wineo's own builds.
pub(crate) fn launcher(runner: &Runner) -> Option<Source> {
    // A runners root inside a launcher folder makes its builds that
    // launcher's too, which needs core's owner lookup to see.
    match runner.source() {
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
    }
}
