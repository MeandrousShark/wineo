use std::fs;
use std::path::Path;

use crate::error::{IoContext, Result};

/// Writes through a temp file and a rename, so readers never see a half-written file.
pub(crate) fn write_atomic(path: &Path, contents: impl AsRef<[u8]>) -> Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).context(|| format!("failed to create {}", dir.display()))?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    fs::write(&tmp, contents.as_ref()).context(|| format!("failed to write {}", path.display()))?;
    fs::rename(&tmp, path).context(|| format!("failed to write {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_contents_and_creates_parents() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("a/b/state.toml");
        write_atomic(&path, "one").unwrap();
        write_atomic(&path, "two").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "two");
        assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
    }
}
