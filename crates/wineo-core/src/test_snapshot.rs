use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct SnapshotEntry {
    path: PathBuf,
    size: u64,
    modified: SystemTime,
    kind: FileKind,
    link_target: Option<PathBuf>,
}

#[derive(Debug, PartialEq, Eq)]
enum FileKind {
    Directory,
    File,
    Symlink,
    Other,
}

pub(crate) fn snapshot_tree(root: &Path) -> Vec<SnapshotEntry> {
    fn visit(root: &Path, path: &Path, entries: &mut Vec<SnapshotEntry>) {
        let metadata = fs::symlink_metadata(path).unwrap();
        let file_type = metadata.file_type();
        let kind = if file_type.is_dir() {
            FileKind::Directory
        } else if file_type.is_file() {
            FileKind::File
        } else if file_type.is_symlink() {
            FileKind::Symlink
        } else {
            FileKind::Other
        };
        entries.push(SnapshotEntry {
            path: path.strip_prefix(root).unwrap().to_owned(),
            size: metadata.len(),
            modified: metadata.modified().unwrap(),
            kind,
            link_target: if file_type.is_symlink() {
                Some(fs::read_link(path).unwrap())
            } else {
                None
            },
        });
        if file_type.is_dir() {
            let mut children: Vec<_> = fs::read_dir(path)
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .collect();
            children.sort();
            for child in children {
                visit(root, &child, entries);
            }
        }
    }

    let mut entries = Vec::new();
    visit(root, root, &mut entries);
    entries
}

pub(crate) fn assert_unchanged(before: &[SnapshotEntry], after: &[SnapshotEntry]) {
    let before_by_path: BTreeMap<_, _> = before
        .iter()
        .map(|entry| (entry.path.as_path(), entry))
        .collect();
    let after_by_path: BTreeMap<_, _> = after
        .iter()
        .map(|entry| (entry.path.as_path(), entry))
        .collect();

    let added: Vec<_> = after
        .iter()
        .filter(|entry| !before_by_path.contains_key(entry.path.as_path()))
        .map(|entry| entry.path.as_path())
        .collect();
    let removed: Vec<_> = before
        .iter()
        .filter(|entry| !after_by_path.contains_key(entry.path.as_path()))
        .map(|entry| entry.path.as_path())
        .collect();
    let changed: Vec<_> = after
        .iter()
        .filter(|entry| {
            before_by_path
                .get(entry.path.as_path())
                .is_some_and(|previous| *previous != *entry)
        })
        .map(|entry| entry.path.as_path())
        .collect();

    let mut differences = Vec::new();
    if !added.is_empty() {
        differences.push(format!("added: {}", display_paths(&added)));
    }
    if !removed.is_empty() {
        differences.push(format!("removed: {}", display_paths(&removed)));
    }
    if !changed.is_empty() {
        differences.push(format!("changed: {}", display_paths(&changed)));
    }
    assert!(
        differences.is_empty(),
        "filesystem tree changed: {}",
        differences.join("; ")
    );
}

fn display_paths(paths: &[&Path]) -> String {
    paths
        .iter()
        .map(|path| {
            if path.as_os_str().is_empty() {
                ".".to_owned()
            } else {
                path.display().to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_records_symlink_targets_without_following_them() {
        let root = tempfile::tempdir().unwrap();
        let target = PathBuf::from("missing-target");
        std::os::unix::fs::symlink(&target, root.path().join("link")).unwrap();
        let entries = snapshot_tree(root.path());
        assert_eq!(entries.len(), 2);
        let link = entries
            .iter()
            .find(|entry| entry.path == Path::new("link"))
            .unwrap();
        assert_eq!(link.kind, FileKind::Symlink);
        assert_eq!(link.link_target.as_ref(), Some(&target));
    }
}
