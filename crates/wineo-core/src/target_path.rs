//! Paths entered while adding a target.

use std::collections::BTreeMap;
use std::path::Path;

use crate::launch;
use crate::paths::Paths;
use crate::winpath;

/// Removes shell quoting and escaped spaces without changing Windows separators.
pub fn clean_path(answer: &str) -> String {
    let answer = answer.trim();
    let unquoted = ['"', '\'']
        .iter()
        .find_map(|quote| answer.strip_prefix(*quote)?.strip_suffix(*quote))
        .unwrap_or(answer);
    if winpath::is_windows_path(unquoted) {
        unquoted.to_owned()
    } else {
        unquoted.replace("\\ ", " ")
    }
}

/// Resolves a Wine target's exe to the value stored in config.
pub fn resolve_exe(
    paths: &Paths,
    wine_prefix: Option<&Path>,
    answer: &str,
) -> Result<String, String> {
    let answer = clean_path(answer);
    if answer.is_empty() {
        return Err("Enter the path to the program".to_owned());
    }
    let missing = || format!("`{answer}` does not exist");
    if winpath::is_windows_path(&answer) {
        let wine_prefix = wine_prefix.ok_or_else(missing)?;
        return match winpath::to_unix(wine_prefix, &answer) {
            Some(unix) if unix.is_file() => Ok(answer),
            _ => Err(missing()),
        };
    }
    let unix = paths.absolute(&answer).map_err(|err| err.to_string())?;
    if !unix.is_file() {
        return Err(missing());
    }
    Ok(config_path(paths, wine_prefix, &unix))
}

/// Gives an entered path its stored form for a duplicate hint.
pub fn candidate_exe(paths: &Paths, wine_prefix: Option<&Path>, answer: &str) -> Option<String> {
    let answer = clean_path(answer);
    if answer.is_empty() {
        return None;
    }
    if winpath::is_windows_path(&answer) {
        let unix = winpath::to_unix(wine_prefix?, &answer)?;
        return unix.is_file().then_some(answer);
    }
    if !answer.contains('/') && !answer.starts_with('~') {
        return Some(answer);
    }
    let unix = paths.absolute(&answer).ok()?;
    unix.is_file()
        .then(|| config_path(paths, wine_prefix, &unix))
}

/// Resolves a native target's exe to the value stored in config.
pub fn native_exe(
    paths: &Paths,
    name: &str,
    env: &BTreeMap<String, String>,
    answer: &str,
) -> Result<String, String> {
    let answer = clean_path(answer);
    if answer.is_empty() {
        return Err("Enter the path to the program".to_owned());
    }
    let path_var = launch::native_path_var(env);
    if !answer.contains('/') && !answer.starts_with('~') {
        launch::resolve_native_exe(paths, name, &answer, &path_var)
            .map_err(|err| err.to_string())?;
        return Ok(answer);
    }
    let unix = paths.absolute(&answer).map_err(|err| err.to_string())?;
    let stored = paths
        .store(&unix)
        .unwrap_or_else(|_| unix.display().to_string());
    launch::resolve_native_exe(paths, name, &stored, &path_var).map_err(|err| err.to_string())?;
    Ok(stored)
}

fn config_path(paths: &Paths, wine_prefix: Option<&Path>, unix: &Path) -> String {
    wine_prefix
        .and_then(|wine_prefix| winpath::from_unix(wine_prefix, unix))
        .or_else(|| paths.store(unix).ok())
        .unwrap_or_else(|| unix.display().to_string())
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::fs::symlink;

    use super::*;

    #[test]
    fn wine_paths_keep_windows_and_convert_drive_c_and_home() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().to_owned();
        let paths = Paths::from_lookup(move |key| {
            (key == "HOME").then(|| OsString::from(home.as_os_str()))
        })
        .unwrap();
        let prefix = root.path().join("prefix");
        let wine = prefix.join("drive_c/Games/Game.exe");
        fs::create_dir_all(wine.parent().unwrap()).unwrap();
        fs::create_dir_all(prefix.join("dosdevices")).unwrap();
        symlink("../drive_c", prefix.join("dosdevices/c:")).unwrap();
        fs::write(&wine, "").unwrap();
        assert_eq!(
            resolve_exe(&paths, Some(&prefix), r"C:\Games\Game.exe"),
            Ok(r"C:\Games\Game.exe".into())
        );
        assert_eq!(
            resolve_exe(&paths, Some(&prefix), &wine.display().to_string()),
            Ok(r"C:\Games\Game.exe".into())
        );
        let outside = root.path().join("My Games/other.exe");
        fs::create_dir_all(outside.parent().unwrap()).unwrap();
        fs::write(&outside, "").unwrap();
        assert_eq!(
            resolve_exe(&paths, Some(&prefix), &outside.display().to_string()),
            Ok("~/My Games/other.exe".into())
        );
    }

    #[test]
    fn native_bare_name_uses_its_path() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().to_owned();
        let paths = Paths::from_lookup(move |key| {
            (key == "HOME").then(|| OsString::from(home.as_os_str()))
        })
        .unwrap();
        let bin = root.path().join("bin");
        fs::create_dir_all(&bin).unwrap();
        let exe = bin.join("tool");
        fs::write(&exe, "").unwrap();
        fs::set_permissions(&exe, fs::Permissions::from_mode(0o755)).unwrap();
        let env = BTreeMap::from([("PATH".to_owned(), bin.display().to_string())]);
        assert_eq!(
            native_exe(&paths, "native", &env, "tool"),
            Ok("tool".into())
        );
    }
}
