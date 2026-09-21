//! Telling a process apart from a later one that reused its id, so a record
//! that outlived what it names never has wineo signal something else.

use std::fs;

/// This boot's id. A process recorded under another one is long gone.
pub fn boot_id() -> Option<String> {
    let id = fs::read_to_string("/proc/sys/kernel/random/boot_id").ok()?;
    Some(id.trim().to_owned())
}

/// When a process started, in clock ticks since boot, as `/proc/<pid>/stat`
/// records it. None once it is gone.
pub fn start_time(pid: u32) -> Option<u64> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // The command name is in parentheses and can hold spaces, so the fields
    // are counted from the last `)`. The start time is the 22nd field.
    let fields = &stat[stat.rfind(')')? + 1..];
    fields.split_whitespace().nth(19)?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_running_process_has_a_start_time_and_the_boot_an_id() {
        let own = start_time(std::process::id()).unwrap();
        assert!(own > 0);
        assert_eq!(start_time(std::process::id()), Some(own));
        assert!(!boot_id().unwrap().is_empty());
        assert_eq!(start_time(u32::MAX), None);
    }
}
