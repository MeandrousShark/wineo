//! The shared memory Legendary's downloader leaves behind when it is
//! killed. Python names its segments at random, so they are found from what
//! the stopping group's own processes hold, in `/proc`, never by a name.

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fs;
use std::path::Path;

const SHM: &str = "/dev/shm";

/// The names of the segments in `/dev/shm` that a process of `group` has
/// open or mapped right now. Processes that can't be read are passed over.
pub(crate) fn held_by_group(group: u32) -> BTreeSet<OsString> {
    let mut held = BTreeSet::new();
    let Ok(entries) = fs::read_dir("/proc") else {
        return held;
    };
    for entry in entries.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<u32>().ok())
        else {
            continue;
        };
        if group_of(pid) != Some(group) {
            continue;
        }
        if let Ok(fds) = fs::read_dir(format!("/proc/{pid}/fd")) {
            for fd in fds.flatten() {
                if let Ok(target) = fs::read_link(fd.path()) {
                    held.extend(segment(&target.into_os_string()));
                }
            }
        }
        if let Ok(maps) = fs::read_to_string(format!("/proc/{pid}/maps")) {
            for line in maps.lines() {
                // The path is the sixth field and may hold spaces.
                if let Some(at) = line.find(" /dev/shm/") {
                    held.extend(segment(&OsString::from(&line[at + 1..])));
                }
            }
        }
    }
    held
}

/// The segment's name when `path` is a file right in `/dev/shm` that hasn't
/// been removed.
fn segment(path: &OsString) -> Option<OsString> {
    let path = Path::new(path);
    if path.parent() != Some(Path::new(SHM)) {
        return None;
    }
    let name = path.file_name()?;
    if name.as_encoded_bytes().ends_with(b" (deleted)") {
        return None;
    }
    Some(name.to_owned())
}

/// The process group from `/proc/PID/stat`, whose name field can hold
/// anything, so it is read from after the last `)`.
fn group_of(pid: u32) -> Option<u32> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let mut fields = stat.rsplit_once(')')?.1.split_whitespace();
    // State, parent, then the group.
    fields.nth(2)?.parse().ok()
}

/// Removes the segments named, those still there. Only regular files right
/// in `/dev/shm` are removed; a name is never followed elsewhere.
pub(crate) fn remove(names: &BTreeSet<OsString>) {
    for name in names {
        let path = Path::new(SHM).join(name);
        if path.parent() != Some(Path::new(SHM)) {
            continue;
        }
        if fs::symlink_metadata(&path).is_ok_and(|meta| meta.file_type().is_file()) {
            let _ = fs::remove_file(&path);
        }
    }
}
