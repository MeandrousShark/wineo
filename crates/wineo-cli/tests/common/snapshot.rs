use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Debug, PartialEq, Eq)]
pub struct TreeEntry {
    pub path: PathBuf,
    pub kind: EntryKind,
    pub size: u64,
    pub modified: Option<SystemTime>,
    pub link_target: Option<PathBuf>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum EntryKind {
    File,
    Directory,
    Symlink,
    Other,
}

pub type TreeSnapshot = Vec<TreeEntry>;

pub fn snapshot(root: &Path) -> TreeSnapshot {
    let mut entries = Vec::new();
    collect(root, root, &mut entries);
    entries.sort_by(|left, right| left.path.cmp(&right.path));
    entries
}

fn collect(root: &Path, path: &Path, entries: &mut Vec<TreeEntry>) {
    let metadata = fs::symlink_metadata(path)
        .unwrap_or_else(|error| panic!("failed to inspect {}: {error}", path.display()));
    let file_type = metadata.file_type();
    let kind = if file_type.is_symlink() {
        EntryKind::Symlink
    } else if file_type.is_dir() {
        EntryKind::Directory
    } else if file_type.is_file() {
        EntryKind::File
    } else {
        EntryKind::Other
    };
    let link_target = file_type.is_symlink().then(|| {
        fs::read_link(path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()))
    });
    entries.push(TreeEntry {
        path: path
            .strip_prefix(root)
            .unwrap_or_else(|error| {
                panic!("{} is outside {}: {error}", path.display(), root.display())
            })
            .to_owned(),
        kind,
        size: metadata.len(),
        modified: metadata.modified().ok(),
        link_target,
    });
    if !file_type.is_dir() {
        return;
    }

    let mut children = fs::read_dir(path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()))
        .map(|entry| {
            entry
                .unwrap_or_else(|error| {
                    panic!("failed to inspect child of {}: {error}", path.display())
                })
                .path()
        })
        .collect::<Vec<_>>();
    children.sort();
    for child in children {
        collect(root, &child, entries);
    }
}
