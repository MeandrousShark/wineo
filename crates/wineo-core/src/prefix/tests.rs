use super::*;

#[test]
fn detects_arch_from_registry_header() {
    let dir = tempfile::tempdir().unwrap();
    assert!(!is_prefix(dir.path()));
    fs::write(
        dir.path().join("system.reg"),
        "WINE REGISTRY Version 2\n;; All keys relative to REGISTRY\\\\Machine\n\n#arch=win64\n",
    )
    .unwrap();
    assert!(is_prefix(dir.path()));
    assert_eq!(detect_arch(dir.path()).unwrap(), Some(Arch::Win64));

    fs::write(dir.path().join("system.reg"), "WINE REGISTRY Version 2\n").unwrap();
    assert_eq!(detect_arch(dir.path()).unwrap(), None);
}

#[test]
fn prepare_dir_creates_missing_and_accepts_empty() {
    let root = tempfile::tempdir().unwrap();
    let missing = root.path().join("a/b");
    assert!(prepare_dir(&missing).unwrap());
    assert!(missing.is_dir());
    assert!(!prepare_dir(&missing).unwrap());
}

#[test]
fn prepare_dir_refuses_non_empty_dirs_and_files() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("file"), "").unwrap();
    assert!(matches!(
        prepare_dir(root.path()),
        Err(Error::NotAPrefix(_))
    ));
    assert!(prepare_dir(&root.path().join("file")).is_err());
}

struct Layout {
    _temp: tempfile::TempDir,
    paths: Paths,
    root: PathBuf,
    outside: PathBuf,
}

fn layout() -> Layout {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let paths = Paths::from_lookup(|key| match key {
        "HOME" => Some(home.clone().into()),
        _ => None,
    })
    .unwrap();
    let outside = temp.path().join("mnt");
    fs::create_dir_all(&outside).unwrap();
    Layout {
        root: paths.default_prefix_root(),
        _temp: temp,
        paths,
        outside,
    }
}

fn plan(layout: &Layout, runner: &Runner, dir: &Path, confirmed: bool) -> Result<BuildPlan> {
    build_plan(&layout.paths, &layout.root, runner, dir, confirmed)
}

fn fake_prefix(dir: &Path) {
    fs::create_dir_all(dir).unwrap();
    fs::write(dir.join("system.reg"), "WINE REGISTRY Version 2\n").unwrap();
}

#[test]
fn build_plan_finds_a_prefix_of_the_runners_layout_ready() {
    let l = layout();
    let wine = l.outside.join("wine");
    fake_prefix(&wine);
    let proton = l.outside.join("proton");
    fake_prefix(&proton.join("pfx"));
    for confirmed in [false, true] {
        assert_eq!(
            plan(&l, &Runner::fake("wine"), &wine, confirmed).unwrap(),
            BuildPlan::Ready
        );
        assert_eq!(
            plan(
                &l,
                &Runner::fake_proton("GE-Proton10-4"),
                &proton,
                confirmed
            )
            .unwrap(),
            BuildPlan::Ready
        );
    }
}

#[test]
fn build_plan_refuses_a_prefix_of_the_other_layout() {
    let l = layout();
    let wine = l.root.join("wine");
    fake_prefix(&wine);
    let proton = l.root.join("proton");
    fake_prefix(&proton.join("pfx"));

    let err = plan(&l, &Runner::fake_proton("GE-Proton10-4"), &wine, true).unwrap_err();
    assert!(matches!(err, Error::PrefixLayout { .. }), "{err}");
    assert_eq!(
        err.to_string(),
        format!(
            "{} holds a Wine prefix, but runner GE-Proton10-4 needs a Proton one",
            wine.display()
        )
    );
    let err = plan(&l, &Runner::fake("wine"), &proton, true).unwrap_err();
    assert_eq!(
        err.to_string(),
        format!(
            "{} holds a Proton prefix, but runner wine needs a Wine one",
            proton.display()
        )
    );
}

#[test]
fn build_plan_builds_a_missing_dir_whose_parent_holds_something() {
    let l = layout();
    fs::create_dir_all(l.outside.join("other")).unwrap();
    let dir = l.outside.join("palworld");
    assert_eq!(
        plan(&l, &Runner::fake("wine"), &dir, false).unwrap(),
        BuildPlan::Build
    );
    assert!(!dir.exists());
}

#[test]
fn build_plan_needs_confirmation_for_an_empty_parent_outside_the_prefix_root() {
    let l = layout();
    let games = l.outside.join("games");
    fs::create_dir_all(&games).unwrap();
    let dir = games.join("palworld");
    let err = plan(&l, &Runner::fake("wine"), &dir, false).unwrap_err();
    assert!(matches!(err, Error::EmptyPrefixParent { .. }), "{err}");
    assert_eq!(
        err.to_string(),
        format!(
            "prefix {} is missing, and {} is empty",
            dir.display(),
            games.display()
        )
    );
    assert_eq!(
        plan(&l, &Runner::fake("wine"), &dir, true).unwrap(),
        BuildPlan::Build
    );
}

#[test]
fn build_plan_builds_under_an_empty_prefix_root() {
    let l = layout();
    fs::create_dir_all(&l.root).unwrap();
    assert_eq!(
        plan(&l, &Runner::fake("wine"), &l.root.join("palworld"), false).unwrap(),
        BuildPlan::Build
    );
}

#[test]
fn build_plan_builds_under_a_missing_prefix_root() {
    let l = layout();
    assert!(!l.root.exists());
    for dir in [l.root.join("palworld"), l.root.join("games/palworld")] {
        assert_eq!(
            plan(&l, &Runner::fake("wine"), &dir, false).unwrap(),
            BuildPlan::Build
        );
    }
    assert!(!l.root.exists());
}

#[test]
fn build_plan_refuses_a_missing_parent_outside_the_prefix_root() {
    let l = layout();
    let dir = l.outside.join("games/prefixes/palworld");
    let err = plan(&l, &Runner::fake("wine"), &dir, true).unwrap_err();
    assert!(matches!(err, Error::PrefixParentMissing { .. }), "{err}");
    assert_eq!(
        err.to_string(),
        format!(
            "prefix {} is missing, and so is {}",
            dir.display(),
            l.outside.join("games/prefixes").display()
        )
    );
}

#[test]
fn build_plan_builds_in_an_empty_dir_under_the_prefix_root() {
    let l = layout();
    let dir = l.root.join("palworld");
    fs::create_dir_all(&dir).unwrap();
    assert_eq!(
        plan(&l, &Runner::fake("wine"), &dir, false).unwrap(),
        BuildPlan::Build
    );
}

#[test]
fn build_plan_needs_confirmation_for_an_empty_dir_outside_the_prefix_root() {
    let l = layout();
    let dir = l.outside.join("game-ssd");
    fs::create_dir_all(&dir).unwrap();
    let err = plan(&l, &Runner::fake("wine"), &dir, false).unwrap_err();
    assert!(matches!(err, Error::EmptyPrefix(_)), "{err}");
    assert_eq!(
        err.to_string(),
        format!("prefix {} is empty", dir.display())
    );
    assert_eq!(
        plan(&l, &Runner::fake("wine"), &dir, true).unwrap(),
        BuildPlan::Build
    );

    // The root itself isn't under the root.
    fs::create_dir_all(&l.root).unwrap();
    assert!(matches!(
        plan(&l, &Runner::fake("wine"), &l.root, false),
        Err(Error::EmptyPrefix(_))
    ));
}

#[test]
fn build_plan_compares_the_prefix_root_resolved() {
    let l = layout();
    let real = l.outside.join("real-root");
    fs::create_dir_all(real.join("palworld")).unwrap();
    fs::create_dir_all(l.root.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(&real, &l.root).unwrap();
    for dir in [real.join("palworld"), l.root.join("palworld")] {
        assert_eq!(
            plan(&l, &Runner::fake("wine"), &dir, false).unwrap(),
            BuildPlan::Build
        );
    }
}

#[test]
fn build_plan_refuses_non_empty_dirs_and_files() {
    let l = layout();
    let dir = l.root.join("stuff");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("notes.txt"), "keep me").unwrap();
    let file = l.root.join("file");
    fs::write(&file, "").unwrap();
    for path in [&dir, &file] {
        assert!(
            matches!(
                plan(&l, &Runner::fake("wine"), path, true),
                Err(Error::NotAPrefix(_))
            ),
            "{}",
            path.display()
        );
    }
}

#[test]
fn build_plan_refuses_a_dangling_symlink() {
    let l = layout();
    fs::create_dir_all(&l.root).unwrap();
    let dir = l.root.join("palworld");
    std::os::unix::fs::symlink(l.outside.join("unmounted/palworld"), &dir).unwrap();
    assert!(matches!(
        plan(&l, &Runner::fake("wine"), &dir, true),
        Err(Error::MissingPrefix(_))
    ));
}

#[test]
fn build_plan_keeps_portable_prefixes_direct_children() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let folder = temp.path().join("usb");
    let paths = Paths::select(
        |key| match key {
            "HOME" => Some(home.clone().into()),
            _ => None,
        },
        Some(&folder),
    )
    .unwrap();
    let root = paths.default_prefix_root();
    let runner = Runner::fake("wine");
    assert_eq!(
        build_plan(&paths, &root, &runner, &root.join("palworld"), false).unwrap(),
        BuildPlan::Build
    );
    assert!(matches!(
        build_plan(&paths, &root, &runner, &root.join("games/palworld"), false),
        Err(Error::PrefixOutsidePortable(_))
    ));
}

#[test]
fn finds_last_loader_message() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("create.log");
    fs::write(
            &log,
            "0054:fixme:ole:thing\nwine: first\n0060:err:x:y\nwine: WINEARCH is set to 'win32' but this is not supported in wow64 mode.\n",
        )
        .unwrap();
    assert_eq!(
        last_wine_message(&log).as_deref(),
        Some("WINEARCH is set to 'win32' but this is not supported in wow64 mode")
    );
    fs::write(&log, "0054:fixme:ole:thing\n").unwrap();
    assert_eq!(last_wine_message(&log), None);
}
