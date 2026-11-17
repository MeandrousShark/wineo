use super::*;
use std::cell::RefCell;
use std::collections::BTreeSet;
use std::os::unix::fs::symlink;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};

#[test]
fn argument_match_is_positional_and_app_id_is_whole() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("ubuntu12_32/reaper");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, "").unwrap();
    let args = |app: &str| {
        vec![
            path.as_os_str().as_bytes().to_vec(),
            b"SteamLaunch".to_vec(),
            format!("AppId={app}").into_bytes(),
            b"--".to_vec(),
        ]
    };
    assert!(matches_arguments(
        &args("289070"),
        "AppId=289070",
        std::slice::from_ref(&path)
    ));
    assert!(!matches_arguments(
        &args("289070"),
        "AppId=2890",
        std::slice::from_ref(&path)
    ));
    let mut misplaced = args("289070");
    misplaced.swap(1, 2);
    assert!(!matches_arguments(
        &misplaced,
        "AppId=289070",
        std::slice::from_ref(&path)
    ));
    let other = root.path().join("other/ubuntu12_32/reaper");
    assert!(!matches_arguments(
        &args("289070"),
        "AppId=289070",
        &[other]
    ));
    let alias = root.path().join("steam-alias");
    symlink(root.path(), &alias).unwrap();
    let mut aliased = args("289070");
    aliased[0] = alias
        .join("ubuntu12_32/reaper")
        .as_os_str()
        .as_bytes()
        .to_vec();
    assert!(matches_arguments(&aliased, "AppId=289070", &[path]));
}

#[test]
fn reaper_match_accepts_verified_exe_when_argv_path_differs() {
    let root = tempfile::tempdir().unwrap();
    let expected = root.path().join("ubuntu12_32/reaper");
    fs::create_dir_all(expected.parent().unwrap()).unwrap();
    fs::write(&expected, "").unwrap();
    let mut process = Inspected {
        fd: (),
        stat: stat(10, 1, 20),
        arguments: vec![
            b"reaper".to_vec(),
            b"SteamLaunch".to_vec(),
            b"AppId=289070".to_vec(),
        ],
        exe: Some(expected.clone()),
    };
    assert!(matches_reaper(
        &process,
        "AppId=289070",
        std::slice::from_ref(&expected)
    ));
    assert!(!matches_reaper(
        &process,
        "AppId=2890",
        std::slice::from_ref(&expected)
    ));
    process.exe = Some(root.path().join("other/reaper"));
    assert!(!matches_reaper(
        &process,
        "AppId=289070",
        std::slice::from_ref(&expected)
    ));
}

struct FakeProc {
    stats: RefCell<BTreeMap<u32, VecDeque<ProcStat>>>,
    exited: BTreeSet<u32>,
    calls: RefCell<Vec<&'static str>>,
    open_error: Option<u32>,
    stat_error: Option<u32>,
    arguments_error: Option<(u32, i32)>,
    exe_error: Option<(u32, i32)>,
    missing_arguments: BTreeSet<u32>,
    missing_exe: BTreeSet<u32>,
    arguments_by_pid: BTreeMap<u32, Vec<Vec<u8>>>,
    exe_by_pid: BTreeMap<u32, PathBuf>,
    fds_by_pid: BTreeMap<u32, Vec<PathBuf>>,
    fds_error: Option<(u32, i32)>,
}

impl FakeProc {
    fn new(stats: &[ProcStat]) -> Self {
        Self {
            stats: RefCell::new(
                stats
                    .iter()
                    .map(|stat| (stat.pid, VecDeque::from([*stat])))
                    .collect(),
            ),
            exited: BTreeSet::new(),
            calls: RefCell::new(Vec::new()),
            open_error: None,
            stat_error: None,
            arguments_error: None,
            exe_error: None,
            missing_arguments: BTreeSet::new(),
            missing_exe: BTreeSet::new(),
            arguments_by_pid: BTreeMap::new(),
            exe_by_pid: BTreeMap::new(),
            fds_by_pid: BTreeMap::new(),
            fds_error: None,
        }
    }
}

impl ProcessReader for FakeProc {
    type Fd = u32;

    fn open(&self, pid: u32) -> io::Result<u32> {
        self.calls.borrow_mut().push("open");
        if self.open_error == Some(pid) {
            return Err(io::Error::from_raw_os_error(
                rustix::io::Errno::MFILE.raw_os_error(),
            ));
        }
        Ok(pid)
    }

    fn stat(&self, pid: u32) -> io::Result<Option<ProcStat>> {
        self.calls.borrow_mut().push("stat");
        if self.stat_error == Some(pid) {
            return Err(io::Error::from_raw_os_error(
                rustix::io::Errno::MFILE.raw_os_error(),
            ));
        }
        let mut stats = self.stats.borrow_mut();
        let Some(reads) = stats.get_mut(&pid) else {
            return Ok(None);
        };
        Ok(if reads.len() > 1 {
            reads.pop_front()
        } else {
            reads.front().copied()
        })
    }

    fn arguments(&self, pid: u32) -> io::Result<Option<Vec<Vec<u8>>>> {
        self.calls.borrow_mut().push("cmdline");
        if let Some((_, error)) = self
            .arguments_error
            .filter(|(error_pid, _)| *error_pid == pid)
        {
            return Err(io::Error::from_raw_os_error(error));
        }
        if self.missing_arguments.contains(&pid) {
            return Ok(None);
        }
        if let Some(arguments) = self.arguments_by_pid.get(&pid) {
            return Ok(Some(arguments.clone()));
        }
        Ok(Some(vec![
            b"reaper".to_vec(),
            b"SteamLaunch".to_vec(),
            b"AppId=289070".to_vec(),
        ]))
    }

    fn exe(&self, pid: u32) -> io::Result<Option<PathBuf>> {
        self.calls.borrow_mut().push("exe");
        if let Some((_, error)) = self.exe_error.filter(|(error_pid, _)| *error_pid == pid) {
            return Err(io::Error::from_raw_os_error(error));
        }
        if self.missing_exe.contains(&pid) {
            return Ok(None);
        }
        if let Some(exe) = self.exe_by_pid.get(&pid) {
            return Ok(Some(exe.clone()));
        }
        Ok(Some(PathBuf::from("reaper")))
    }

    fn exited(&self, pid: &u32) -> io::Result<bool> {
        self.calls.borrow_mut().push("exited");
        Ok(self.exited.contains(pid))
    }

    fn fd_links(&self, pid: u32) -> io::Result<Option<Vec<PathBuf>>> {
        if let Some((_, error)) = self.fds_error.filter(|(error_pid, _)| *error_pid == pid) {
            return Err(io::Error::from_raw_os_error(error));
        }
        Ok(self.fds_by_pid.get(&pid).cloned())
    }
}

fn stat(pid: u32, ppid: u32, start_time: u64) -> ProcStat {
    ProcStat {
        pid,
        ppid,
        start_time,
        state: 'S',
    }
}

fn tree_members(
    reader: &FakeProc,
    stats: &[ProcStat],
    root: u32,
    start_time: u64,
) -> Vec<ProcStat> {
    let processes = stats.to_vec();
    let mut seen = BTreeMap::new();
    if let Some(process) = inspect(reader, root).unwrap()
        && process.stat.start_time == start_time
    {
        seen.insert(
            (root, start_time),
            Member {
                fd: process.fd,
                term_sent: false,
            },
        );
    }
    capture_members(reader, processes, &mut seen).unwrap();
    stats
        .iter()
        .filter(|stat| seen.contains_key(&(stat.pid, stat.start_time)))
        .copied()
        .collect()
}

#[test]
fn inspection_opens_pidfd_before_metadata_and_confirms_identity_afterwards() {
    let reader = FakeProc::new(&[stat(11, 10, 20)]);
    assert!(inspect(&reader, 11).unwrap().is_some());
    assert_eq!(
        *reader.calls.borrow(),
        ["open", "stat", "cmdline", "exe", "exited", "stat"]
    );
}

#[test]
fn inspection_rejects_pid_reused_between_metadata_reads() {
    let original = stat(11, 10, 20);
    let stranger = stat(11, 10, 30);
    let reader = FakeProc::new(&[original]);
    reader
        .stats
        .borrow_mut()
        .insert(11, VecDeque::from([original, stranger]));
    assert!(inspect(&reader, 11).unwrap().is_none());
}

#[test]
fn inspection_rejects_an_exited_pidfd_even_if_stat_still_matches() {
    let mut reader = FakeProc::new(&[stat(11, 10, 20)]);
    reader.exited.insert(11);
    assert!(inspect(&reader, 11).unwrap().is_none());
}

#[test]
fn tree_walk_follows_parent_chain_without_process_groups() {
    let stats = [
        stat(10, 1, 9),
        stat(11, 10, 10),
        stat(12, 11, 11),
        stat(13, 1, 12),
        stat(14, 12, 13),
    ];
    let reader = FakeProc::new(&stats);
    assert_eq!(
        tree_members(&reader, &stats, 11, 10),
        vec![stats[1], stats[2], stats[4]]
    );
    assert!(tree_members(&reader, &stats, 11, 999).is_empty());
}

#[test]
fn tree_walk_rejects_stale_edge_when_child_predates_parent() {
    let stats = [stat(10, 1, 20), stat(11, 10, 10), stat(12, 11, 30)];
    let reader = FakeProc::new(&stats);
    assert_eq!(tree_members(&reader, &stats, 10, 20), vec![stats[0]]);
}

#[test]
fn tree_walk_never_anchors_children_to_an_exited_retained_pidfd() {
    let stats = [stat(10, 1, 20), stat(11, 10, 30)];
    let mut reader = FakeProc::new(&stats);
    let processes = stats.to_vec();
    let mut seen = BTreeMap::from([(
        (10, 20),
        Member {
            fd: 10,
            term_sent: true,
        },
    )]);
    // Numeric PID 10 may now belong to a stranger. The retained pidfd
    // remains exited, even if a later /proc snapshot has the same ppid.
    reader.exited.insert(10);
    capture_members(&reader, processes, &mut seen).unwrap();
    assert_eq!(seen.keys().copied().collect::<Vec<_>>(), [(10, 20)]);
}

#[test]
fn tree_walk_rechecks_parent_pidfd_when_linking_a_child() {
    let stats = [stat(10, 1, 20), stat(11, 10, 30)];
    let mut reader = FakeProc::new(&stats);
    let child = inspect_stat(&reader, 11).unwrap().unwrap();
    reader.exited.insert(10);
    assert!(!current_child(&reader, &child, stats[0], &10).unwrap());
}

#[test]
fn tree_walk_opens_only_descendants_and_propagates_pidfd_errors() {
    let stats = [stat(10, 1, 20), stat(11, 10, 30), stat(12, 1, 40)];
    let mut reader = FakeProc::new(&stats);
    reader.open_error = Some(12);
    let mut seen = BTreeMap::from([(
        (10, 20),
        Member {
            fd: 10,
            term_sent: false,
        },
    )]);
    capture_members(&reader, stats.to_vec(), &mut seen).unwrap();
    assert!(seen.contains_key(&(11, 30)));
    assert!(!seen.contains_key(&(12, 40)));
    reader.open_error = Some(11);
    let mut seen = BTreeMap::from([(
        (10, 20),
        Member {
            fd: 10,
            term_sent: false,
        },
    )]);
    let error = capture_members(&reader, stats.to_vec(), &mut seen).unwrap_err();
    assert_eq!(
        error.raw_os_error(),
        Some(rustix::io::Errno::MFILE.raw_os_error())
    );
}

#[test]
fn stat_errors_are_not_treated_as_process_exits() {
    let mut reader = FakeProc::new(&[stat(10, 1, 20)]);
    reader.stat_error = Some(10);
    let error = inspect_stat(&reader, 10).err().unwrap();
    assert_eq!(
        error.raw_os_error(),
        Some(rustix::io::Errno::MFILE.raw_os_error())
    );
}

#[test]
fn reaper_candidate_reads_exe_only_after_positional_match() {
    let path = Path::new("/steam/ubuntu12_32/reaper");
    let mut reader = FakeProc::new(&[stat(10, 2, 20)]);
    reader.exe_error = Some((10, rustix::io::Errno::MFILE.raw_os_error()));
    assert!(
        reaper_candidate(&reader, 10, "AppId=2890", &[path.to_owned()])
            .unwrap()
            .is_none()
    );
    assert!(!reader.calls.borrow().contains(&"exe"));
    let error = reaper_candidate(&reader, 10, "AppId=289070", &[path.to_owned()])
        .err()
        .unwrap();
    assert_eq!(
        error.raw_os_error(),
        Some(rustix::io::Errno::MFILE.raw_os_error())
    );
}

#[test]
fn candidate_metadata_errors_propagate() {
    let path = Path::new("/steam/ubuntu12_32/reaper");
    let mut reader = FakeProc::new(&[stat(10, 2, 20)]);
    reader.arguments_error = Some((10, rustix::io::Errno::MFILE.raw_os_error()));
    let error = reaper_candidate(&reader, 10, "AppId=289070", &[path.to_owned()])
        .err()
        .unwrap();
    assert_eq!(
        error.raw_os_error(),
        Some(rustix::io::Errno::MFILE.raw_os_error())
    );
    reader.arguments_error = None;
    reader.exe_error = Some((10, rustix::io::Errno::MFILE.raw_os_error()));
    let error = client_candidate(&reader, 10).err().unwrap();
    assert_eq!(
        error.raw_os_error(),
        Some(rustix::io::Errno::MFILE.raw_os_error())
    );
}

#[test]
fn candidate_permission_errors_are_skipped() {
    let path = Path::new("/steam/ubuntu12_32/reaper");
    let mut reader = FakeProc::new(&[stat(10, 2, 20)]);
    reader.arguments_error = Some((10, rustix::io::Errno::ACCESS.raw_os_error()));
    assert!(
        reaper_candidate(&reader, 10, "AppId=289070", &[path.to_owned()])
            .unwrap()
            .is_none()
    );

    reader.arguments_error = None;
    reader.exe_error = Some((10, rustix::io::Errno::PERM.raw_os_error()));
    assert!(
        reaper_candidate(&reader, 10, "AppId=289070", &[path.to_owned()])
            .unwrap()
            .is_none()
    );

    reader.exe_error = Some((10, rustix::io::Errno::ACCESS.raw_os_error()));
    assert!(client_candidate(&reader, 10).unwrap().is_none());

    reader.exe_error = None;
    reader.arguments_error = Some((10, rustix::io::Errno::PERM.raw_os_error()));
    assert!(client_candidate(&reader, 10).unwrap().is_none());
}

/// A fake process table: a client (pid 5), reapers for apps 10 (two), 20,
/// and 30 under it, a reaper for app 30 under a foreign parent, and bystanders.
/// `dir` is the client's binary folder.
fn reaper_table(root: &Path, dir: &str) -> (FakeProc, Vec<u32>) {
    let reaper = root.join(dir).join("reaper");
    let client = root.join(dir).join("steam");
    let launch = |app: &str| {
        vec![
            reaper.as_os_str().as_bytes().to_vec(),
            b"SteamLaunch".to_vec(),
            format!("AppId={app}").into_bytes(),
            b"--".to_vec(),
        ]
    };
    let mut reader = FakeProc::new(&[
        stat(5, 2, 100),
        stat(6, 5, 110),
        stat(7, 5, 120),
        stat(8, 5, 130),
        stat(9, 4, 140),
        stat(11, 5, 150),
        stat(12, 1, 160),
        stat(13, 5, 170),
        stat(4, 2, 90),
    ]);
    reader.exe_by_pid.insert(5, client.clone());
    reader.exe_by_pid.insert(4, PathBuf::from("/usr/bin/other"));
    reader
        .arguments_by_pid
        .insert(5, vec![client.as_os_str().as_bytes().to_vec()]);
    reader.arguments_by_pid.insert(4, vec![b"other".to_vec()]);
    for (pid, app) in [(6, "10"), (7, "20"), (8, "30"), (9, "30")] {
        reader.arguments_by_pid.insert(pid, launch(app));
        reader.exe_by_pid.insert(pid, reaper.clone());
    }
    reader
        .arguments_by_pid
        .insert(11, vec![b"bash".to_vec(), b"-c".to_vec()]);
    reader.arguments_by_pid.insert(12, launch("10"));
    // A second game process for app 10, listed ahead of the first so the
    // result has to be sorted.
    reader.arguments_by_pid.insert(13, launch("10"));
    reader.exe_by_pid.insert(13, reaper.clone());
    (reader, vec![13, 4, 5, 6, 7, 8, 9, 11, 12])
}

#[test]
fn one_scan_gives_each_app_what_a_scan_for_it_alone_gives() {
    // Steam's own folder names, not CLIENT_DIRS, so dropping one fails here.
    for dir in ["steamrt64", "ubuntu12_32"] {
        one_scan_matches_scans_alone(dir);
    }
}

fn one_scan_matches_scans_alone(dir: &str) {
    let root = tempfile::tempdir().unwrap();
    let install = Install {
        kind: InstallKind::Native,
        root: root.path().to_owned(),
    };
    let (reader, pids) = reaper_table(root.path(), dir);
    let ids = |ids: &[&str]| ids.iter().map(|id| (*id).to_owned()).collect::<Vec<_>>();
    let all = ids(&["10", "20", "30", "40"]);
    let together = scan_reapers(&reader, pids.clone(), &install, &all, "boot").unwrap();

    // The parent of 9 is not the client, and 12 is orphaned to init.
    let pids_of = |app: &str| {
        together[app]
            .iter()
            .map(|reaper| reaper.pid)
            .collect::<Vec<_>>()
    };
    assert_eq!(pids_of("10"), [6, 13]);
    assert_eq!(pids_of("20"), [7]);
    assert_eq!(pids_of("30"), [8]);
    assert_eq!(pids_of("40"), Vec::<u32>::new());
    assert_eq!(together["10"][0].start_time, 110);
    assert_eq!(together["10"][0].boot_id, "boot");

    // Only the client's own folders hold its reaper.
    let (mut stray, pids) = reaper_table(root.path(), dir);
    let elsewhere = root.path().join("bin/reaper");
    let mut moved = stray.arguments_by_pid[&7].clone();
    moved[0] = elsewhere.as_os_str().as_bytes().to_vec();
    stray.arguments_by_pid.insert(7, moved);
    stray.exe_by_pid.insert(7, elsewhere);
    let found = scan_reapers(&stray, pids, &install, &ids(&["20"]), "boot").unwrap();
    assert!(found["20"].is_empty(), "{dir}");

    // A client in one folder with its reaper in the other still counts: both
    // are this install's binaries, and the parent link is checked as before.
    let other = if dir == "steamrt64" {
        "ubuntu12_32"
    } else {
        "steamrt64"
    };
    let (mut mixed, pids) = reaper_table(root.path(), dir);
    let client = root.path().join(other).join("steam");
    mixed.exe_by_pid.insert(5, client.clone());
    mixed
        .arguments_by_pid
        .insert(5, vec![client.as_os_str().as_bytes().to_vec()]);
    let found = scan_reapers(&mixed, pids, &install, &ids(&["20"]), "boot").unwrap();
    assert_eq!(found["20"].len(), 1, "{dir}");

    // Through a root alias like ~/.steam/root, the processes show the
    // canonical paths, and the binaries must exist to be resolved.
    for binary in ["steam", "reaper"] {
        let path = root.path().join(dir).join(binary);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "").unwrap();
    }
    let alias = root.path().join("alias");
    symlink(root.path(), &alias).unwrap();
    let aliased = Install {
        kind: InstallKind::Native,
        root: alias,
    };
    let (reader, pids) = reaper_table(root.path(), dir);
    let found = scan_reapers(&reader, pids, &aliased, &ids(&["20"]), "boot").unwrap();
    assert_eq!(found["20"].len(), 1, "{dir}");

    for app in &all {
        let (reader, pids) = reaper_table(root.path(), dir);
        let alone =
            scan_reapers(&reader, pids, &install, std::slice::from_ref(app), "boot").unwrap();
        assert_eq!(alone[app], together[app], "app {app}");
    }
}

#[test]
fn one_scan_reads_each_process_once_however_many_apps_it_serves() {
    let root = tempfile::tempdir().unwrap();
    let install = Install {
        kind: InstallKind::Native,
        root: root.path().to_owned(),
    };
    let count = |ids: &[&str], what: &str| {
        let (reader, pids) = reaper_table(root.path(), "ubuntu12_32");
        let ids = ids.iter().map(|id| (*id).to_owned()).collect::<Vec<_>>();
        scan_reapers(&reader, pids, &install, &ids, "boot").unwrap();
        let calls = reader.calls.borrow();
        calls.iter().filter(|call| **call == what).count()
    };
    // Only launches of a requested app (6, 7, 8, 9, 12, 13) are opened,
    // plus the parent again for each one with a live parent (6, 7, 8, 9,
    // 13). The command-line prefilter keeps the bystanders (4, 5, 11)
    // out of the pidfd path.
    assert_eq!(count(&["10", "20", "30"], "open"), 6 + 5);
    assert_eq!(count(&["20"], "open"), 1 + 1);
    // Three sweeps for the same apps would repeat the nine prefilter
    // reads each time.
    let together = count(&["10", "20", "30"], "cmdline");
    let apart = count(&["10"], "cmdline") + count(&["20"], "cmdline") + count(&["30"], "cmdline");
    assert!(apart > together);
}

/// The 2026-11 pressure-vessel launch chain: client 5998, client container
/// 6043, launcher service 6054, supervisor 8698, reaper 8699 for app 5335510.
/// A direct child reaper 7000 for app 440 sits beside it.
fn pv_table(root: &Path) -> (FakeProc, Vec<u32>) {
    let pv = root.join("steamrt64/pv-runtime/steam-runtime-steamrt");
    let reaper = root.join("steamrt64/reaper");
    let client = root.join("steamrt64/steam");
    let mut reader = FakeProc::new(&[
        stat(5985, 2, 90),
        stat(5998, 5985, 100),
        stat(6043, 5998, 110),
        stat(6054, 6043, 120),
        stat(8698, 6054, 200),
        stat(8699, 8698, 210),
        stat(7000, 5998, 220),
    ]);
    reader
        .exe_by_pid
        .insert(5985, PathBuf::from("/usr/bin/bash"));
    reader.exe_by_pid.insert(5998, client.clone());
    reader.exe_by_pid.insert(
        6043,
        pv.join("pressure-vessel/libexec/steam-runtime-tools-0/srt-client-container"),
    );
    reader.exe_by_pid.insert(
        6054,
        pv.join(
            "pressure-vessel/libexec/steam-runtime-tools-0/x86_64-linux-gnu-srt-launcher-service",
        ),
    );
    reader.exe_by_pid.insert(
        8698,
        pv.join("pressure-vessel/bin/steam-runtime-supervisor"),
    );
    reader.arguments_by_pid.insert(
        5998,
        vec![client.as_os_str().as_bytes().to_vec(), b"-silent".to_vec()],
    );
    for pid in [5985, 6043, 6054, 8698] {
        reader
            .arguments_by_pid
            .insert(pid, vec![b"helper".to_vec()]);
    }
    for (pid, app) in [(8699, "5335510"), (7000, "440")] {
        reader.exe_by_pid.insert(pid, reaper.clone());
        reader.arguments_by_pid.insert(
            pid,
            vec![
                reaper.as_os_str().as_bytes().to_vec(),
                b"SteamLaunch".to_vec(),
                format!("AppId={app}").into_bytes(),
                b"--".to_vec(),
            ],
        );
    }
    (reader, vec![5985, 5998, 6043, 6054, 7000, 8698, 8699])
}

fn pv_claims(reader: &FakeProc, pids: Vec<u32>, install: &Install) -> Vec<u32> {
    let apps = ["5335510".to_owned()];
    scan_reapers(reader, pids, install, &apps, "boot").unwrap()["5335510"]
        .iter()
        .map(|reaper| reaper.pid)
        .collect()
}

fn native(root: &Path) -> Install {
    Install {
        kind: InstallKind::Native,
        root: root.to_owned(),
    }
}

#[test]
fn reaper_under_the_pressure_vessel_launcher_is_claimed() {
    let temp = tempfile::tempdir().unwrap();
    let install = native(temp.path());
    let (reader, pids) = pv_table(temp.path());
    let apps = ["5335510".to_owned(), "440".to_owned()];
    let found = scan_reapers(&reader, pids, &install, &apps, "boot").unwrap();
    assert_eq!(found["5335510"].len(), 1);
    assert_eq!(found["5335510"][0].pid, 8699);
    assert_eq!(found["5335510"][0].start_time, 210);
    // The old layout, a reaper straight under the client, still counts.
    assert_eq!(found["440"].len(), 1);
    assert_eq!(found["440"][0].pid, 7000);

    // Through a root alias like ~/.steam/root the processes show the
    // canonical paths, and the client and reaper must exist to be resolved.
    for binary in ["steam", "reaper"] {
        let path = temp.path().join("steamrt64").join(binary);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "").unwrap();
    }
    let alias = temp.path().join("alias");
    symlink(temp.path(), &alias).unwrap();
    let (reader, pids) = pv_table(temp.path());
    assert_eq!(pv_claims(&reader, pids, &native(&alias)), [8699]);
}

#[test]
fn a_symlinked_runtime_folder_still_anchors_its_helpers() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("Steam");
    let target = temp.path().join("elsewhere/pv-runtime");
    fs::create_dir_all(&target).unwrap();
    fs::create_dir_all(root.join("steamrt64")).unwrap();
    symlink(&target, root.join("steamrt64/pv-runtime")).unwrap();
    let (mut reader, pids) = pv_table(&root);
    // The kernel reports the helpers under the link's target.
    let target = fs::canonicalize(&target).unwrap();
    for pid in [6043, 6054, 8698] {
        let exe = reader.exe_by_pid[&pid].clone();
        let inside = exe.strip_prefix(root.join("steamrt64/pv-runtime")).unwrap();
        reader.exe_by_pid.insert(pid, target.join(inside));
    }
    assert_eq!(pv_claims(&reader, pids, &native(&root)), [8699]);
}

#[test]
fn pressure_vessel_helpers_must_come_from_this_install_runtime() {
    let temp = tempfile::tempdir().unwrap();
    let install = native(&temp.path().join("mine"));
    let other_pv = temp
        .path()
        .join("other/steamrt64/pv-runtime/steam-runtime-steamrt/pressure-vessel/bin/x");
    for exe in [
        Some(other_pv),
        Some(PathBuf::from("/usr/bin/something")),
        // This install's folder, but outside its runtime.
        Some(install.root.join("steamrt64/steam-launch-wrapper")),
        Some(install.root.join("steamrt64/pv-runtime-x/helper")),
        None,
    ] {
        let (mut reader, pids) = pv_table(&install.root);
        match &exe {
            Some(exe) => {
                reader.exe_by_pid.insert(6054, exe.clone());
            }
            None => {
                reader.missing_exe.insert(6054);
            }
        }
        assert!(pv_claims(&reader, pids, &install).is_empty(), "{exe:?}");
    }
}

#[test]
fn pressure_vessel_chain_must_reach_this_install_client() {
    let temp = tempfile::tempdir().unwrap();
    let install = native(&temp.path().join("mine"));
    let (mut reader, pids) = pv_table(&install.root);
    let other = temp.path().join("other/steamrt64/steam");
    reader.exe_by_pid.insert(5998, other.clone());
    reader
        .arguments_by_pid
        .insert(5998, vec![other.as_os_str().as_bytes().to_vec()]);
    assert!(pv_claims(&reader, pids, &install).is_empty());
}

#[test]
fn pressure_vessel_chain_is_bounded() {
    let temp = tempfile::tempdir().unwrap();
    let install = native(temp.path());
    let helper = temp
        .path()
        .join("steamrt64/pv-runtime/steam-runtime-steamrt/bin/helper");
    // `extra` more runtime helpers between the launcher service and the
    // supervisor, on top of the real three.
    let chain = |extra: u32| {
        let (mut reader, mut pids) = pv_table(temp.path());
        let mut parent = 6054;
        for pid in 6100..6100 + extra {
            reader.stats.borrow_mut().insert(
                pid,
                VecDeque::from([stat(pid, parent, 130 + u64::from(pid))]),
            );
            reader.exe_by_pid.insert(pid, helper.clone());
            reader
                .arguments_by_pid
                .insert(pid, vec![b"helper".to_vec()]);
            pids.push(pid);
            parent = pid;
        }
        reader
            .stats
            .borrow_mut()
            .insert(8698, VecDeque::from([stat(8698, parent, 7000)]));
        reader
            .stats
            .borrow_mut()
            .insert(8699, VecDeque::from([stat(8699, 8698, 7010)]));
        pv_claims(&reader, pids, &install)
    };
    // Literal counts, so changing the bound fails here.
    assert_eq!(chain(0), [8699]);
    assert_eq!(chain(1), [8699]);
    assert!(chain(2).is_empty());
}

#[test]
fn pressure_vessel_helper_identity_is_verified_at_each_hop() {
    let temp = tempfile::tempdir().unwrap();
    let install = native(temp.path());

    let (mut reader, pids) = pv_table(temp.path());
    reader.exited.insert(6054);
    assert!(pv_claims(&reader, pids, &install).is_empty());

    // The launcher service's pid is reused, or it is reparented, while the
    // walk reads it.
    for changed in [stat(6054, 6043, 125), stat(6054, 1, 120)] {
        let (reader, pids) = pv_table(temp.path());
        reader
            .stats
            .borrow_mut()
            .insert(6054, VecDeque::from([stat(6054, 6043, 120), changed]));
        assert!(pv_claims(&reader, pids, &install).is_empty(), "{changed:?}");
    }

    // A helper that started after its supposed child is a reused pid, not
    // the child's real parent.
    let (reader, pids) = pv_table(temp.path());
    reader
        .stats
        .borrow_mut()
        .insert(6054, VecDeque::from([stat(6054, 6043, 205)]));
    assert!(pv_claims(&reader, pids, &install).is_empty());
}

#[test]
fn missing_client_exe_keeps_the_argv_fallback() {
    let mut reader = FakeProc::new(&[stat(10, 2, 20)]);
    reader.missing_exe.insert(10);
    let client = client_candidate(&reader, 10).unwrap().unwrap();
    assert!(client.exe.is_none());
    assert!(!client.arguments.is_empty());
}

#[test]
fn client_match_requires_install_exe_with_limited_argv_fallback() {
    // Steam's own folder names, not CLIENT_DIRS, so dropping one fails here.
    for dir in ["steamrt64", "ubuntu12_32"] {
        client_match_in(dir);
    }
}

fn client_match_in(dir: &str) {
    let matches_client = |install: &Install, client: &Inspected<()>| {
        matches_client(install, &install_binaries(install, "steam"), client)
    };
    let temp = tempfile::tempdir().unwrap();
    let mut install = Install {
        kind: InstallKind::Native,
        root: temp.path().to_owned(),
    };
    let path = install.root().join(dir).join("steam");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, "").unwrap();
    let mut client = Inspected {
        fd: (),
        stat: stat(10, 1, 10),
        arguments: vec![path.as_os_str().as_bytes().to_vec()],
        exe: Some(path.clone()),
    };
    assert!(matches_client(&install, &client));
    client.exe = Some(PathBuf::from("/unrelated/steam"));
    assert!(!matches_client(&install, &client));
    client.exe = None;
    assert!(matches_client(&install, &client));
    client.arguments[0] = b"/unrelated/steam".to_vec();
    assert!(!matches_client(&install, &client));
    install.kind = InstallKind::Flatpak;
    client.exe = Some(PathBuf::from("/sandbox/steam"));
    client.arguments[0] = path.as_os_str().as_bytes().to_vec();
    assert!(matches_client(&install, &client));
    let alias = temp.path().join("alias");
    symlink(temp.path(), &alias).unwrap();
    install.kind = InstallKind::Native;
    install.root = alias;
    client.exe = Some(path);
    assert!(matches_client(&install, &client));
}

#[test]
fn only_a_live_client_of_this_install_counts_as_running() {
    // Steam's own folder names, not CLIENT_DIRS, so dropping one fails here.
    for dir in ["steamrt64", "ubuntu12_32"] {
        only_a_live_client_counts(dir);
    }
}

fn only_a_live_client_counts(dir: &str) {
    let temp = tempfile::tempdir().unwrap();
    let install = Install {
        kind: InstallKind::Native,
        root: temp.path().join("mine"),
    };
    let other = temp.path().join("other");
    let client = |root: &Path| root.join(dir).join("steam");
    for root in [&install.root, &other] {
        fs::create_dir_all(client(root).parent().unwrap()).unwrap();
        fs::write(client(root), "").unwrap();
    }
    let reader = |pid: u32, state: char, root: &Path| {
        let mut reader = FakeProc::new(&[ProcStat {
            state,
            ..stat(pid, 1, 10)
        }]);
        reader.exe_by_pid.insert(pid, client(root));
        reader
            .arguments_by_pid
            .insert(pid, vec![client(root).as_os_str().as_bytes().to_vec()]);
        reader
    };
    assert!(scan_client(&reader(7, 'S', &install.root), vec![7], &install).unwrap());
    // Another install's client, an unrelated process, and a zombie.
    assert!(!scan_client(&reader(7, 'S', &other), vec![7], &install).unwrap());
    assert!(!scan_client(&reader(7, 'S', &install.root), vec![8], &install).unwrap());
    assert!(!scan_client(&reader(7, 'Z', &install.root), vec![7], &install).unwrap());
    assert!(!scan_client(&reader(7, 'S', &install.root), Vec::new(), &install).unwrap());
    // A `steam` binary outside the client's folders isn't the client.
    let stray = install.root.join("bin/steam");
    fs::create_dir_all(stray.parent().unwrap()).unwrap();
    fs::write(&stray, "").unwrap();
    let mut outside = reader(7, 'S', &install.root);
    outside.exe_by_pid.insert(7, stray.clone());
    outside
        .arguments_by_pid
        .insert(7, vec![stray.as_os_str().as_bytes().to_vec()]);
    assert!(!scan_client(&outside, vec![7], &install).unwrap());
}

#[test]
fn the_steam_dir_comes_from_each_native_root_shape() {
    let at = |root: &str| {
        steam_dir(&Install {
            kind: InstallKind::Native,
            root: PathBuf::from(root),
        })
    };
    let dot = Some(PathBuf::from("/home/u/.steam"));
    assert_eq!(at("/home/u/.steam/root"), dot);
    assert_eq!(at("/home/u/.steam/steam"), dot);
    assert_eq!(at("/home/u/.local/share/Steam"), dot);
    // Anything else falls back to the plain process check.
    assert_eq!(at("/opt/steam"), None);
    assert_eq!(at("/home/u/.steam/rooted"), None);
    assert_eq!(at("/home/u/share/Steam"), None);
}

/// A home with a native install at `.local/share/Steam`, and a fake process
/// table where pid 7 is its client when `client` is set.
fn client_table(home: &Path, client: bool) -> (Install, FakeProc, Vec<u32>) {
    let install = Install {
        kind: InstallKind::Native,
        root: home.join(".local/share/Steam"),
    };
    let exe = install.root.join("steamrt64/steam");
    let mut reader = FakeProc::new(&[stat(7, 1, 10), stat(9, 1, 20)]);
    reader.exe_by_pid.insert(
        7,
        if client {
            exe.clone()
        } else {
            PathBuf::from("/usr/bin/other")
        },
    );
    reader
        .arguments_by_pid
        .insert(7, vec![exe.as_os_str().as_bytes().to_vec()]);
    reader.exe_by_pid.insert(9, PathBuf::from("/usr/bin/other"));
    reader.arguments_by_pid.insert(9, vec![b"other".to_vec()]);
    fs::create_dir_all(home.join(".steam")).unwrap();
    (install, reader, vec![7, 9])
}

#[test]
fn the_client_is_ready_only_once_its_pid_file_process_holds_the_pipe() {
    use ClientState::{Absent, Ready, Starting};
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path();
    let pid_file = home.join(".steam/steam.pid");
    let pipe = home.join(".steam/steam.pipe");
    let state = |client: bool, holder: Option<(u32, Vec<PathBuf>)>| {
        let (install, mut reader, pids) = client_table(home, client);
        if let Some((pid, links)) = holder {
            reader.fds_by_pid.insert(pid, links);
        }
        scan_client_state(&reader, pids, &install).unwrap()
    };

    let _ = fs::remove_file(&pid_file);
    assert_eq!(state(false, None), Absent);
    assert_eq!(state(true, None), Starting);

    fs::write(&pid_file, "9\n").unwrap();
    let holds = || Some((9, vec![PathBuf::from("/dev/null"), pipe.clone()]));
    assert_eq!(state(true, holds()), Ready);
    // The pid file's process can hold the pipe a moment before its exe is
    // the client's, and `steam` already forwards to it then.
    assert_eq!(state(false, holds()), Ready);
    // Dead, holding other files, or holding another home's pipe.
    assert_eq!(state(true, None), Starting);
    assert_eq!(
        state(true, Some((9, vec![PathBuf::from("/dev/null")]))),
        Starting
    );
    let elsewhere = temp.path().join("other/.steam/steam.pipe");
    assert_eq!(state(true, Some((9, vec![elsewhere]))), Starting);
    // The pid file names someone else.
    assert_eq!(state(true, Some((8, vec![pipe.clone()]))), Starting);
    for text in ["", "nine\n", "0\n", "-9\n"] {
        fs::write(&pid_file, text).unwrap();
        assert_eq!(
            state(true, Some((0, vec![pipe.clone()]))),
            Starting,
            "{text:?}"
        );
    }
    assert_eq!(state(false, None), Absent);
}

#[test]
fn the_pipe_matches_through_a_symlinked_home() {
    let temp = tempfile::tempdir().unwrap();
    let real = temp.path().join("real");
    let (_, _, _) = client_table(&real, true);
    let pipe = real.join(".steam/steam.pipe");
    fs::write(&pipe, "").unwrap();
    let alias = temp.path().join("home");
    symlink(&real, &alias).unwrap();
    let (install, mut reader, pids) = client_table(&alias, true);
    fs::write(alias.join(".steam/steam.pid"), "9\n").unwrap();
    // The kernel reports the resolved path.
    reader
        .fds_by_pid
        .insert(9, vec![fs::canonicalize(&pipe).unwrap()]);
    assert_eq!(
        scan_client_state(&reader, pids, &install).unwrap(),
        ClientState::Ready
    );
}

#[test]
fn unreadable_fds_are_not_ready_and_other_errors_propagate() {
    let temp = tempfile::tempdir().unwrap();
    let (install, mut reader, pids) = client_table(temp.path(), true);
    fs::write(temp.path().join(".steam/steam.pid"), "9\n").unwrap();
    reader.fds_error = Some((9, rustix::io::Errno::ACCESS.raw_os_error()));
    assert_eq!(
        scan_client_state(&reader, pids.clone(), &install).unwrap(),
        ClientState::Starting
    );
    reader.fds_error = Some((9, rustix::io::Errno::MFILE.raw_os_error()));
    let error = scan_client_state(&reader, pids, &install).unwrap_err();
    assert_eq!(
        error.raw_os_error(),
        Some(rustix::io::Errno::MFILE.raw_os_error())
    );
}

#[test]
fn flatpak_and_unknown_roots_keep_the_plain_process_check() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path();
    for root in [
        home.join(".var/app/com.valvesoftware.Steam/.local/share/Steam"),
        home.join("opt/steam"),
    ] {
        let kind = if root.ends_with(".local/share/Steam") {
            InstallKind::Flatpak
        } else {
            InstallKind::Native
        };
        let install = Install { kind, root };
        let exe = install.root.join("steamrt64/steam");
        let mut reader = FakeProc::new(&[stat(7, 1, 10)]);
        reader.exe_by_pid.insert(7, exe.clone());
        reader
            .arguments_by_pid
            .insert(7, vec![exe.as_os_str().as_bytes().to_vec()]);
        // A pid file in the home is never read for these: the Flatpak's is
        // in another pid namespace.
        fs::create_dir_all(home.join(".steam")).unwrap();
        fs::write(home.join(".steam/steam.pid"), "7\n").unwrap();
        assert_eq!(
            scan_client_state(&reader, vec![7], &install).unwrap(),
            ClientState::Ready,
            "{install:?}"
        );
        assert_eq!(
            scan_client_state(&reader, Vec::new(), &install).unwrap(),
            ClientState::Absent,
            "{install:?}"
        );
    }
}

#[test]
fn a_real_process_holding_the_pipe_is_ready() {
    let temp = tempfile::tempdir().unwrap();
    let dot = temp.path().join(".steam");
    fs::create_dir_all(&dot).unwrap();
    let pipe = dot.join("steam.pipe");
    let pid_file = dot.join("steam.pid");
    rustix::fs::mknodat(
        rustix::fs::CWD,
        &pipe,
        rustix::fs::FileType::Fifo,
        rustix::fs::Mode::from_raw_mode(0o600),
        0,
    )
    .unwrap();
    let script = temp.path().join("client");
    crate::test_script::write_script(
        &script,
        "#!/bin/sh\nexec 3<>\"$1\"\necho $$ > \"$2.tmp\"\nmv \"$2.tmp\" \"$2\"\nexec sleep 30\n",
    );
    let child = crate::test_script::KillOnDrop::new(
        Command::new(&script)
            .args([&pipe, &pid_file])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    while !pid_file.exists() {
        assert!(Instant::now() < deadline, "the client didn't start");
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(pipe_held(&Proc, &dot).unwrap());
    let install = Install {
        kind: InstallKind::Native,
        root: temp.path().join(".steam/root"),
    };
    assert_eq!(client_state(&install).unwrap(), ClientState::Ready);

    // This test process is alive but doesn't hold the pipe.
    fs::write(&pid_file, format!("{}\n", std::process::id())).unwrap();
    assert!(!pipe_held(&Proc, &dot).unwrap());
    // Once the client is gone, its pid file names nothing.
    fs::write(&pid_file, format!("{}\n", child.id())).unwrap();
    drop(child);
    assert!(!pipe_held(&Proc, &dot).unwrap());
}

/// Kills a test process's group even when an assertion panics first.
struct KillGroupOnDrop(std::process::Child);

impl Drop for KillGroupOnDrop {
    fn drop(&mut self) {
        if let Some(group) = rustix::process::Pid::from_raw(self.0.id() as i32) {
            let _ = rustix::process::kill_process_group(group, rustix::process::Signal::KILL);
        }
        let _ = self.0.wait();
    }
}

#[test]
fn a_process_whose_name_is_not_utf8_still_has_a_stat() {
    let dir = tempfile::tempdir().unwrap();
    // The kernel keeps 15 bytes of a name, which splits the final `é` here.
    let script = dir.path().join("aaaaaaaaaaaaaa\u{e9}");
    crate::test_script::write_script(&script, "#!/bin/sh\nsleep 30\nexit 0\n");
    let child = KillGroupOnDrop(
        Command::new(&script)
            .process_group(0)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let pid = child.0.id();
    let comm = format!("/proc/{pid}/comm");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !fs::read(&comm).unwrap().contains(&0xc3) && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(std::str::from_utf8(&fs::read(&comm).unwrap()).is_err());
    let stat = read_stat(pid).unwrap().unwrap();
    assert_eq!((stat.pid, stat.ppid), (pid, std::process::id()));
}

#[test]
fn uptime_ticks_track_current_process_start() {
    let ticks = uptime_ticks().unwrap();
    let own = read_stat(std::process::id()).unwrap().unwrap().start_time;
    assert!(ticks >= own);
}

#[test]
fn stop_kills_reparented_child_but_spares_same_group_foreign_process() {
    let temp = tempfile::tempdir().unwrap();
    let child_file = temp.path().join("child.pid");
    let mut root = Command::new("sh")
        .arg("-c")
        .arg("sh -c 'trap \"\" TERM; exec sleep 30' & echo $! > \"$CHILD_PID_FILE\"; wait")
        .env("CHILD_PID_FILE", &child_file)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .unwrap();
    let mut foreign = Command::new("sleep")
        .arg("30")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(root.id() as i32)
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    let child_pid = loop {
        if let Ok(text) = fs::read_to_string(&child_file)
            && let Ok(pid) = text.trim().parse::<u32>()
        {
            break pid;
        }
        assert!(Instant::now() < deadline, "child did not start");
        std::thread::sleep(Duration::from_millis(10));
    };
    let child_fd = open_pidfd(child_pid).unwrap();
    let reaper = Reaper {
        pid: root.id(),
        start_time: read_stat(root.id()).unwrap().unwrap().start_time,
        boot_id: read_boot_id().unwrap(),
    };
    stop_tree(&reaper, Duration::from_millis(100), false, || false).unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    while !pidfd_exited(&child_fd).unwrap() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(pidfd_exited(&child_fd).unwrap());
    assert!(foreign.try_wait().unwrap().is_none());
    let _ = foreign.kill();
    let _ = foreign.wait();
    let _ = root.wait();
}
