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
        &path,
        &path
    ));
    assert!(!matches_arguments(
        &args("289070"),
        "AppId=2890",
        &path,
        &path
    ));
    let mut misplaced = args("289070");
    misplaced.swap(1, 2);
    assert!(!matches_arguments(&misplaced, "AppId=289070", &path, &path));
    let other = root.path().join("other/ubuntu12_32/reaper");
    assert!(!matches_arguments(
        &args("289070"),
        "AppId=289070",
        &other,
        &other
    ));
    let alias = root.path().join("steam-alias");
    symlink(root.path(), &alias).unwrap();
    let mut aliased = args("289070");
    aliased[0] = alias
        .join("ubuntu12_32/reaper")
        .as_os_str()
        .as_bytes()
        .to_vec();
    assert!(matches_arguments(&aliased, "AppId=289070", &path, &path));
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
        &expected,
        &expected
    ));
    assert!(!matches_reaper(
        &process,
        "AppId=2890",
        &expected,
        &expected
    ));
    process.exe = Some(root.path().join("other/reaper"));
    assert!(!matches_reaper(
        &process,
        "AppId=289070",
        &expected,
        &expected
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
        reaper_candidate(&reader, 10, "AppId=2890", path, path)
            .unwrap()
            .is_none()
    );
    assert!(!reader.calls.borrow().contains(&"exe"));
    let error = reaper_candidate(&reader, 10, "AppId=289070", path, path)
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
    let error = reaper_candidate(&reader, 10, "AppId=289070", path, path)
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
        reaper_candidate(&reader, 10, "AppId=289070", path, path)
            .unwrap()
            .is_none()
    );

    reader.arguments_error = None;
    reader.exe_error = Some((10, rustix::io::Errno::PERM.raw_os_error()));
    assert!(
        reaper_candidate(&reader, 10, "AppId=289070", path, path)
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
fn reaper_table(root: &Path) -> (FakeProc, Vec<u32>) {
    let reaper = root.join("ubuntu12_32/reaper");
    let client = root.join("ubuntu12_32/steam");
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
    let root = tempfile::tempdir().unwrap();
    let install = Install {
        kind: InstallKind::Native,
        root: root.path().to_owned(),
    };
    let (reader, pids) = reaper_table(root.path());
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

    for app in &all {
        let (reader, pids) = reaper_table(root.path());
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
        let (reader, pids) = reaper_table(root.path());
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
    let temp = tempfile::tempdir().unwrap();
    let mut install = Install {
        kind: InstallKind::Native,
        root: temp.path().to_owned(),
    };
    let path = install.root().join("ubuntu12_32/steam");
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
    let temp = tempfile::tempdir().unwrap();
    let install = Install {
        kind: InstallKind::Native,
        root: temp.path().join("mine"),
    };
    let other = temp.path().join("other");
    let client = |root: &Path| root.join("ubuntu12_32/steam");
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
