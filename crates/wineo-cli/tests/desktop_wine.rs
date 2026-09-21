use std::fs;
use std::path::Path;
use std::process::{Command, Output};
use std::thread;
use std::time::{Duration, Instant};

use tempfile::TempDir;

const SHORTCUT_SCRIPT: &str = r#"
Set shell = CreateObject("WScript.Shell")
Set shortcut = shell.CreateShortcut(WScript.Arguments(0))
shortcut.TargetPath = WScript.Arguments(1)
shortcut.WorkingDirectory = WScript.Arguments(2)
shortcut.IconLocation = WScript.Arguments(1) & ",0"
shortcut.Save
"#;

fn wineo(root: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_wineo"));
    command
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("XDG_DATA_HOME", root.join("data"))
        .env("XDG_STATE_HOME", root.join("state"))
        .env("WINEDEBUG", "-all")
        .env("WINEPREFIX", root.join("data/wineo/prefixes/app"));
    command
}

fn run_wineo(root: &Path, args: &[&str]) -> Output {
    wineo(root).args(args).output().expect("wineo should start")
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "command failed with {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn windows_path(path: &Path) -> String {
    format!(
        "Z:{}",
        path.to_str()
            .expect("temporary paths are UTF-8")
            .replace('/', "\\")
    )
}

fn make_shortcut(root: &Path, prefix: &Path) {
    let build = root.join("build");
    fs::create_dir_all(&build).unwrap();
    let script = build.join("make-shortcut.vbs");
    fs::write(&script, SHORTCUT_SCRIPT).unwrap();

    let menu = prefix.join("drive_c/ProgramData/Microsoft/Windows/Start Menu/Programs");
    fs::create_dir_all(&menu).unwrap();
    let shortcut = menu.join("Wineo Test.lnk");
    let target = r"C:\windows\system32\notepad.exe";
    let working_directory = r"C:\windows\system32";
    let result = Command::new("wine")
        .env("WINEPREFIX", prefix)
        .env("WINEARCH", "win64")
        .env("WINEDEBUG", "-all")
        .env("WINEDLLOVERRIDES", "winemenubuilder.exe=d")
        .args([
            "cscript.exe",
            "//nologo",
            &windows_path(&script),
            r"C:\ProgramData\Microsoft\Windows\Start Menu\Programs\Wineo Test.lnk",
            target,
            working_directory,
        ])
        .output()
        .expect("wine cscript should start");
    assert_success(&result);

    assert!(
        shortcut.is_file(),
        "cscript did not create {}",
        shortcut.display()
    );
    let wait = Command::new("wineserver")
        .env("WINEPREFIX", prefix)
        .arg("-w")
        .status()
        .expect("wineserver should start");
    assert!(wait.success(), "wineserver -w failed");
}

#[test]
#[ignore = "runs wine and opens notepad"]
fn real_wine_programs_desktop_entry_icon_and_launch() {
    let root = TempDir::new().unwrap();

    let created = run_wineo(root.path(), &["new", "-n", "app"]);
    assert_success(&created);
    let prefix = root.path().join("data/wineo/prefixes/app");
    make_shortcut(root.path(), &prefix);

    let programs = run_wineo(root.path(), &["programs", "app"]);
    assert_success(&programs);
    let programs = String::from_utf8_lossy(&programs.stdout);
    assert!(programs.contains("shortcut"), "{programs}");
    assert!(programs.contains("Wineo Test"), "{programs}");
    assert!(
        programs.contains(r"C:\windows\system32\notepad.exe"),
        "{programs}"
    );

    let set = wineo(root.path())
        .args([
            "set",
            "app",
            r"targets.test.exe=C:\windows\system32\notepad.exe",
            "targets.test.name=Wineo Test",
            "targets.test.desktop=true",
        ])
        .output()
        .unwrap();
    assert_success(&set);

    let entry = root.path().join("data/applications/wineo-app-test.desktop");
    assert!(entry.is_file(), "desktop entry was not written");
    let entry_text = fs::read_to_string(&entry).unwrap();
    assert!(entry_text.contains("Name=Wineo Test\n"), "{entry_text}");
    assert!(
        entry_text.contains("Exec=wineo start app --target test\n"),
        "{entry_text}"
    );
    assert!(entry_text.contains("Icon=wineo-app-test\n"), "{entry_text}");
    assert!(
        entry_text.contains("StartupWMClass=notepad.exe\n"),
        "{entry_text}"
    );

    let icons = root
        .path()
        .join("data/icons/hicolor")
        .read_dir()
        .unwrap()
        .filter_map(Result::ok)
        .map(|size| size.path().join("apps/wineo-app-test.png"))
        .filter(|path| path.is_file())
        .collect::<Vec<_>>();
    assert!(!icons.is_empty(), "wineo did not extract an icon");
    for icon in &icons {
        let bytes = fs::read(icon).unwrap();
        assert!(
            bytes.starts_with(b"\x89PNG\r\n\x1a\n"),
            "{} is not a PNG",
            icon.display()
        );
        assert!(
            bytes.len() > 100,
            "{} is unexpectedly empty",
            icon.display()
        );
        let width = u32::from_be_bytes(bytes[16..20].try_into().unwrap());
        let height = u32::from_be_bytes(bytes[20..24].try_into().unwrap());
        assert!(
            width > 1 && height > 1,
            "{} has no usable image",
            icon.display()
        );
    }

    let desktop = run_wineo(root.path(), &["desktop"]);
    assert_success(&desktop);
    assert!(
        String::from_utf8_lossy(&desktop.stdout).contains("app  test  Wineo Test"),
        "{}",
        String::from_utf8_lossy(&desktop.stdout)
    );

    let exec = entry_text
        .lines()
        .find_map(|line| line.strip_prefix("Exec="))
        .expect("desktop entry should have Exec")
        .split_whitespace()
        .skip(1)
        .collect::<Vec<_>>();
    let launched = wineo(root.path()).args(exec).output().unwrap();
    assert_success(&launched);
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let tasks = run_wineo(root.path(), &["exec", "app", "tasklist.exe"]);
        assert_success(&tasks);
        if String::from_utf8_lossy(&tasks.stdout)
            .to_ascii_lowercase()
            .contains("notepad.exe")
        {
            break;
        }
        assert!(Instant::now() < deadline, "notepad did not start");
        thread::sleep(Duration::from_millis(100));
    }
    let stopped = run_wineo(root.path(), &["stop", "app"]);
    assert_success(&stopped);

    let removed = run_wineo(root.path(), &["rm", "app", "--delete-prefix"]);
    assert_success(&removed);
    assert!(!entry.exists(), "rm left the desktop entry behind");
    assert!(
        icons.iter().all(|icon| !icon.exists()),
        "rm left extracted icons behind"
    );
}
