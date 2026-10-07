use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FileKind {
    File,
    Directory,
    Symlink,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    size: u64,
    modified: SystemTime,
    kind: FileKind,
    link_target: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot(BTreeMap<PathBuf, Entry>);

impl Snapshot {
    pub fn capture(root: &Path) -> io::Result<Self> {
        let mut entries = BTreeMap::new();
        record(root, root, &mut entries)?;
        Ok(Self(entries))
    }

    #[track_caller]
    pub fn assert_unchanged(&self, root: &Path) {
        let after = Self::capture(root).expect("capture XDG tree after command");
        let mut added = Vec::new();
        let mut removed = Vec::new();
        let mut changed = Vec::new();

        for (path, entry) in &after.0 {
            match self.0.get(path) {
                None => added.push(path.display().to_string()),
                Some(before) if before != entry => changed.push(path.display().to_string()),
                Some(_) => {}
            }
        }
        for path in self.0.keys() {
            if !after.0.contains_key(path) {
                removed.push(path.display().to_string());
            }
        }

        assert!(
            added.is_empty() && removed.is_empty() && changed.is_empty(),
            "XDG tree changed: added: {}; removed: {}; changed: {}",
            added.join(", "),
            removed.join(", "),
            changed.join(", ")
        );
    }
}

fn record(root: &Path, path: &Path, entries: &mut BTreeMap<PathBuf, Entry>) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    let file_type = metadata.file_type();
    let kind = if file_type.is_file() {
        FileKind::File
    } else if file_type.is_dir() {
        FileKind::Directory
    } else if file_type.is_symlink() {
        FileKind::Symlink
    } else {
        FileKind::Other
    };
    let relative = if path == root {
        PathBuf::from(".")
    } else {
        path.strip_prefix(root).unwrap_or(path).to_path_buf()
    };
    entries.insert(
        relative,
        Entry {
            size: metadata.len(),
            modified: metadata.modified()?,
            kind,
            link_target: if kind == FileKind::Symlink {
                Some(fs::read_link(path)?)
            } else {
                None
            },
        },
    );

    if kind == FileKind::Directory {
        for child in fs::read_dir(path)? {
            record(root, &child?.path(), entries)?;
        }
    }
    Ok(())
}
