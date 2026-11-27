use std::path::PathBuf;

use super::*;
use crate::epic::game::{Dlc, InFlight};
use crate::epic::owned;

const DLC_ONE: &str = "0000000000000000000000000000000c";
const DLC_TWO: &str = "0000000000000000000000000000000e";
const BUILD: &str = "++Dlc Game+Release-43.11-CL-60803671-Windows";

fn owned() -> Owned {
    let list = fs::read_to_string(crate::epic::fake::fixture("list.json")).unwrap();
    Owned {
        account: "someone".to_owned(),
        games: owned::parse_list(&list).unwrap(),
        warning: None,
    }
}

fn dlc_game(version: &str, dlc: &[(&str, Option<&str>)]) -> Record {
    let mut record = Record::new("Dlc Game", PathBuf::from("/games/Dlc Game"));
    record.installed_version = Some(version.to_owned());
    record.dlc = dlc
        .iter()
        .map(|(app, version)| Dlc {
            app_name: (*app).to_owned(),
            version: version.map(str::to_owned),
        })
        .collect();
    record
}

fn states(status: &Status) -> Vec<(&str, DlcState)> {
    status
        .dlc
        .iter()
        .map(|dlc| (dlc.app_name.as_str(), dlc.state))
        .collect()
}

#[test]
fn a_game_at_epics_version_with_all_its_dlc_is_up_to_date() {
    let record = dlc_game(BUILD, &[(DLC_ONE, Some(BUILD)), (DLC_TWO, Some(BUILD))]);
    let status = status_of("dlc-game", &record, &owned());
    assert_eq!(status.state, UpdateState::UpToDate);
    assert!(!status.applicable);
    assert_eq!(status.reason, None);
    assert_eq!(status.installed.as_deref(), Some(BUILD));
    assert_eq!(status.available.as_deref(), Some(BUILD));
    assert_eq!(
        states(&status),
        [(DLC_ONE, DlcState::UpToDate), (DLC_TWO, DlcState::UpToDate)]
    );
}

#[test]
fn another_version_of_the_game_or_a_dlc_is_an_update() {
    let status = status_of("dlc-game", &dlc_game("old", &[]), &owned());
    assert_eq!(status.state, UpdateState::Available);
    assert!(status.applicable);

    let record = dlc_game(BUILD, &[(DLC_ONE, Some("old")), (DLC_TWO, Some(BUILD))]);
    let status = status_of("dlc-game", &record, &owned());
    assert_eq!(status.state, UpdateState::Available);
    assert_eq!(
        states(&status),
        [(DLC_ONE, DlcState::Available), (DLC_TWO, DlcState::UpToDate)]
    );
}

#[test]
fn an_owned_dlc_the_game_lacks_is_not_installed_and_an_update() {
    // Bought since, or failed at install: either way not in the record.
    let record = dlc_game(BUILD, &[(DLC_ONE, Some(BUILD))]);
    let status = status_of("dlc-game", &record, &owned());
    assert_eq!(status.state, UpdateState::Available);
    assert!(status.applicable);
    assert_eq!(
        states(&status),
        [(DLC_ONE, DlcState::UpToDate), (DLC_TWO, DlcState::NotInstalled)]
    );
    assert_eq!(
        status.dlc[1].title.as_deref(),
        Some("Extra Two Dlc Game Content")
    );
}

#[test]
fn a_game_epic_doesnt_list_or_without_a_version_cant_be_compared() {
    let mut record = dlc_game(BUILD, &[(DLC_ONE, Some(BUILD))]);
    record.app_name = "Gone".to_owned();
    let status = status_of("gone", &record, &owned());
    assert_eq!(status.state, UpdateState::CantCompare);
    assert!(!status.applicable);
    assert_eq!(
        status.reason.as_deref(),
        Some("Epic doesn't list Gone among your games now.")
    );
    assert_eq!(states(&status), [(DLC_ONE, DlcState::CantCompare)]);

    let mut record = dlc_game(BUILD, &[]);
    record.installed_version = None;
    let status = status_of("dlc-game", &record, &owned());
    assert_eq!(status.state, UpdateState::CantCompare);
    assert_eq!(
        status.reason.as_deref(),
        Some("wineo has no record of which version of Dlc Game is installed.")
    );
}

#[test]
fn a_mark_is_unfinished_work_with_the_command_that_finishes_it() {
    let mut record = dlc_game(BUILD, &[]);
    for (work, command) in [
        (Work::Install, "wineo import epic --game Dlc Game"),
        (Work::Update, "wineo update dlc-game"),
        (Work::Repair, "wineo update dlc-game --repair"),
        (Work::Unknown, "wineo update dlc-game --repair"),
    ] {
        record.in_flight = Some(InFlight {
            work,
            app: None,
        });
        let status = status_of("dlc-game", &record, &owned());
        assert_eq!(status.unfinished, Some(work));
        assert_eq!(status.finish_command().as_deref(), Some(command));
    }
}

#[test]
fn a_repair_ignores_what_its_check_found_but_not_other_errors() {
    assert!(found_by_check(
        "[cli] ERROR: File is missing: \"Binaries/Win64/Game.exe\""
    ));
    assert!(found_by_check(
        "[cli] ERROR: Verification failed, 0 file(s) corrupted, 1 file(s) are missing."
    ));
    assert!(found_by_check("[cli] ERROR: File does not match hash: \"a.pak\""));
    assert!(!found_by_check(
        "[FileWorker] ERROR: Writing file failed: [Errno 28] No space left on device"
    ));
}
