use std::path::{Path, PathBuf};

/// True for drive-letter paths such as `C:\foo`, `c:/foo`, `C:\`, or `C:`.
pub fn is_windows_path(s: &str) -> bool {
    let bytes = s.as_bytes();
    bytes.len() >= 2
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes.get(2), None | Some(b'\\') | Some(b'/'))
}

/// Maps a Windows path to the Unix path it refers to inside `prefix`.
pub fn to_unix(prefix: &Path, windows_path: &str) -> Option<PathBuf> {
    if !is_windows_path(windows_path) {
        return None;
    }

    let drive = char::from(windows_path.as_bytes()[0].to_ascii_lowercase());
    // Only the drive root is canonicalized; everything below it is walked by
    // hand so the case-insensitive fallback below decides the result.
    let drive_link = prefix.join("dosdevices").join(format!("{drive}:"));
    let drive_root = std::fs::canonicalize(drive_link).ok()?;

    // Normalize "." and ".." lexically first. ".." past the drive root is
    // simply dropped since Vec::pop on an empty list is a no-op.
    let mut components: Vec<&str> = Vec::new();
    for part in windows_path[2..].split(['\\', '/']) {
        match part {
            "" | "." => {}
            ".." => {
                components.pop();
            }
            name => components.push(name),
        }
    }

    let mut current = drive_root;
    for name in components {
        current = resolve_component(&current, name)?;
    }
    Some(current)
}

// Windows names are case-insensitive: try the exact name first, then fall
// back to scanning the directory. Sorting keeps ties deterministic.
fn resolve_component(dir: &Path, name: &str) -> Option<PathBuf> {
    let exact = dir.join(name);
    if exact.exists() {
        return Some(exact);
    }

    let mut names: Vec<_> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name())
        .collect();
    names.sort();

    names
        .into_iter()
        .find(|entry| entry.to_str().is_some_and(|s| s.eq_ignore_ascii_case(name)))
        .map(|entry| dir.join(entry))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::symlink;
    use tempfile::TempDir;

    fn setup() -> (TempDir, PathBuf, TempDir, PathBuf) {
        let root = tempfile::tempdir().unwrap();
        let canonical_root = root.path().canonicalize().unwrap();

        let adobe = canonical_root.join("drive_c/Program Files/Adobe");
        fs::create_dir_all(&adobe).unwrap();
        fs::write(adobe.join("Photoshop.exe"), b"").unwrap();
        fs::create_dir_all(canonical_root.join("drive_c/users/Public")).unwrap();

        fs::create_dir_all(canonical_root.join("dosdevices")).unwrap();
        symlink("../drive_c", canonical_root.join("dosdevices/c:")).unwrap();

        let z = tempfile::tempdir().unwrap();
        let canonical_z = z.path().canonicalize().unwrap();
        fs::write(canonical_z.join("file.txt"), b"").unwrap();
        symlink(&canonical_z, canonical_root.join("dosdevices/z:")).unwrap();

        (root, canonical_root, z, canonical_z)
    }

    #[test]
    fn is_windows_path_true_for_drive_paths() {
        for s in [r"C:\foo", "c:/foo", r"C:\", "C:"] {
            assert!(is_windows_path(s), "{s}");
        }
    }

    #[test]
    fn is_windows_path_false_for_non_drive_paths() {
        for s in [
            "notepad",
            "/home/x",
            r"\\server\share",
            r"CC:\x",
            r"1:\x",
            "",
        ] {
            assert!(!is_windows_path(s), "{s}");
        }
    }

    #[test]
    fn exact_match() {
        let (root, canonical_root, _z, _canonical_z) = setup();
        let expected = canonical_root.join("drive_c/Program Files/Adobe/Photoshop.exe");
        assert_eq!(
            to_unix(root.path(), r"C:\Program Files\Adobe\Photoshop.exe"),
            Some(expected)
        );
    }

    #[test]
    fn matches_case_insensitively() {
        let (root, canonical_root, _z, _canonical_z) = setup();
        let expected = canonical_root.join("drive_c/Program Files/Adobe/Photoshop.exe");
        assert_eq!(
            to_unix(root.path(), r"c:\PROGRAM FILES\adobe\photoshop.EXE"),
            Some(expected)
        );
    }

    #[test]
    fn accepts_forward_slashes() {
        let (root, canonical_root, _z, _canonical_z) = setup();
        let expected = canonical_root.join("drive_c/Program Files/Adobe/Photoshop.exe");
        assert_eq!(
            to_unix(root.path(), "C:/Program Files/Adobe/Photoshop.exe"),
            Some(expected)
        );
    }

    #[test]
    fn skips_repeated_and_trailing_separators() {
        let (root, canonical_root, _z, _canonical_z) = setup();
        let expected = canonical_root.join("drive_c/Program Files/Adobe/Photoshop.exe");
        assert_eq!(
            to_unix(root.path(), r"C:\\Program Files\\Adobe\Photoshop.exe\\"),
            Some(expected)
        );
    }

    #[test]
    fn resolves_dot_and_dotdot() {
        let (root, canonical_root, _z, _canonical_z) = setup();
        let expected = canonical_root.join("drive_c/Program Files/Adobe/Photoshop.exe");
        assert_eq!(
            to_unix(
                root.path(),
                r"C:\Program Files\.\Adobe\..\Adobe\Photoshop.exe"
            ),
            Some(expected)
        );
    }

    #[test]
    fn dotdot_cannot_escape_drive_root() {
        let (root, canonical_root, _z, _canonical_z) = setup();
        let expected = canonical_root.join("drive_c/Program Files/Adobe/Photoshop.exe");
        assert_eq!(
            to_unix(root.path(), r"C:\..\Program Files\Adobe\Photoshop.exe"),
            Some(expected)
        );
    }

    #[test]
    fn bare_drive_returns_drive_root() {
        let (root, canonical_root, _z, _canonical_z) = setup();
        let drive_c = canonical_root.join("drive_c");
        assert_eq!(to_unix(root.path(), "C:"), Some(drive_c.clone()));
        assert_eq!(to_unix(root.path(), r"C:\"), Some(drive_c));
    }

    #[test]
    fn resolves_directories_case_insensitively() {
        let (root, canonical_root, _z, _canonical_z) = setup();
        let expected = canonical_root.join("drive_c/users/Public");
        assert_eq!(to_unix(root.path(), r"c:\USERS\public"), Some(expected));
    }

    #[test]
    fn resolves_other_drive_letters() {
        let (root, _canonical_root, _z, canonical_z) = setup();
        assert_eq!(
            to_unix(root.path(), r"Z:\file.txt"),
            Some(canonical_z.join("file.txt"))
        );
    }

    #[test]
    fn missing_drive_returns_none() {
        let (root, _canonical_root, _z, _canonical_z) = setup();
        assert_eq!(to_unix(root.path(), r"D:\x"), None);
    }

    #[test]
    fn missing_component_returns_none() {
        let (root, _canonical_root, _z, _canonical_z) = setup();
        assert_eq!(to_unix(root.path(), r"C:\Program Files\Nonexistent"), None);
    }

    #[test]
    fn rejects_non_windows_paths() {
        assert_eq!(to_unix(Path::new("/irrelevant"), "/home/x"), None);
    }
}
