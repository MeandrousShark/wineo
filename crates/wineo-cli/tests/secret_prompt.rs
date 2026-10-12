//! A secret prompt whose output has gone away still puts the terminal back.

mod common;

use std::fs::OpenOptions;
use std::process::Stdio;

use common::Fixture;
use rustix::pty::{OpenptFlags, grantpt, openpt, ptsname, unlockpt};
use rustix::termios::{LocalModes, tcgetattr};

#[test]
fn a_closed_stdout_leaves_echo_as_it_was() {
    let f = Fixture::new();
    let master = openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY).unwrap();
    grantpt(&master).unwrap();
    unlockpt(&master).unwrap();
    let name = ptsname(&master, Vec::new()).unwrap();
    let slave = OpenOptions::new()
        .read(true)
        .write(true)
        .open(name.to_str().unwrap())
        .unwrap();
    assert!(
        tcgetattr(&slave)
            .unwrap()
            .local_modes
            .contains(LocalModes::ECHO)
    );

    // Standard output is a pipe nobody reads, so the prompt can't be shown.
    let (reader, writer) = std::io::pipe().unwrap();
    drop(reader);
    let status = f
        .wineo()
        .args(["art", "key"])
        .stdin(slave.try_clone().unwrap())
        .stdout(writer)
        .stderr(Stdio::null())
        .status()
        .unwrap();

    // The usual exit for a closed pipe, after the terminal was put back.
    assert_eq!(status.code(), Some(141));
    assert!(
        tcgetattr(&slave)
            .unwrap()
            .local_modes
            .contains(LocalModes::ECHO),
        "echo was left off"
    );
}
