use super::*;
use std::os::unix::fs::symlink;
use tempfile::TempDir;

/// A `.lnk` holding the one field a scan reads.
fn lnk_bytes(target: &str) -> Vec<u8> {
    fn put(bytes: &mut [u8], at: usize, value: u32) {
        bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
    }
    let mut info = vec![0u8; 0x1C];
    let base_at = u32::try_from(info.len()).unwrap();
    info.extend(target.bytes());
    info.push(0);
    let suffix_at = u32::try_from(info.len()).unwrap();
    info.push(0);
    let size = u32::try_from(info.len()).unwrap();
    put(&mut info, 0, size);
    put(&mut info, 4, 0x1C);
    put(&mut info, 8, 1);
    put(&mut info, 16, base_at);
    put(&mut info, 24, suffix_at);

    let mut out = vec![0u8; 0x4C];
    put(&mut out, 0, 0x4C);
    out[4..20].copy_from_slice(&[
        0x01, 0x14, 0x02, 0, 0, 0, 0, 0, 0xC0, 0, 0, 0, 0, 0, 0, 0x46,
    ]);
    put(&mut out, 20, 2); // HasLinkInfo, with no ID list to step over
    out.extend(info);
    out
}

/// Enough of wine's layout for a scan, including the executables every
/// fresh prefix already has.
struct Fake(TempDir);

impl Fake {
    fn new() -> Self {
        let fake = Self(tempfile::tempdir().unwrap());
        fs::create_dir_all(fake.0.path().join("dosdevices")).unwrap();
        symlink("../drive_c", fake.0.path().join("dosdevices/c:")).unwrap();
        fake.file("drive_c/windows/notepad.exe", b"");
        fake.file("drive_c/windows/system32/winecfg.exe", b"");
        for root in ["Program Files", "Program Files (x86)"] {
            fake.file(
                &format!("drive_c/{root}/Internet Explorer/iexplore.exe"),
                b"",
            );
            fake.file(
                &format!("drive_c/{root}/Windows Media Player/wmplayer.exe"),
                b"",
            );
            fake.file(
                &format!("drive_c/{root}/Windows NT/Accessories/wordpad.exe"),
                b"",
            );
        }
        fake
    }

    fn file(&self, path: &str, contents: &[u8]) -> &Self {
        let path = self.0.path().join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
        self
    }

    fn exe(&self, path: &str) -> &Self {
        self.file(path, b"")
    }

    fn lnk(&self, path: &str, target: &str) -> &Self {
        self.file(path, &lnk_bytes(target))
    }

    fn scan(&self) -> Listing {
        scan(self.0.path()).unwrap()
    }
}

/// `source name exe` per program, which is what the verb prints.
fn rows(programs: &[Program]) -> Vec<String> {
    programs
        .iter()
        .map(|program| {
            format!(
                "{} {} {}",
                program.source.as_str(),
                program.name,
                program.exe
            )
        })
        .collect()
}

const MACHINE: &str = "drive_c/ProgramData/Microsoft/Windows/Start Menu/Programs";
const USER: &str = "drive_c/users/me/AppData/Roaming/Microsoft/Windows/Start Menu/Programs";

// The premise the whole scan rests on: with nothing installed, it finds
// nothing, so it never needs a snapshot to compare against.
#[test]
fn a_fresh_prefix_has_nothing_installed() {
    let fake = Fake::new();
    fs::create_dir_all(fake.0.path().join(MACHINE).join("StartUp")).unwrap();
    let listing = fake.scan();
    assert!(listing.is_empty(), "{listing:?}");
    assert!(listing.unreadable.is_empty());
    // Wine's own Program Files entries are there, just held back.
    assert_eq!(
        rows(&listing.filtered),
        [
            r"exe iexplore C:\Program Files (x86)\Internet Explorer\iexplore.exe",
            r"exe iexplore C:\Program Files\Internet Explorer\iexplore.exe",
            r"exe wmplayer C:\Program Files (x86)\Windows Media Player\wmplayer.exe",
            r"exe wmplayer C:\Program Files\Windows Media Player\wmplayer.exe",
            r"exe wordpad C:\Program Files (x86)\Windows NT\Accessories\wordpad.exe",
            r"exe wordpad C:\Program Files\Windows NT\Accessories\wordpad.exe",
        ]
    );
}

#[test]
fn a_game_folder_lists_its_executables_by_unix_path() {
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path();
    for path in ["Game.exe", "windows/Tool.exe", "unins000.exe", "notes.txt"] {
        let path = folder.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, "").unwrap();
    }
    let listing = scan_folder(folder);
    let shown = |path: &str| folder.join(path).display().to_string();
    assert_eq!(
        rows(&listing.programs),
        [
            format!("exe Game {}", shown("Game.exe")),
            format!("exe Tool {}", shown("windows/Tool.exe")),
        ]
    );
    assert_eq!(
        rows(&listing.filtered),
        [format!("exe unins000 {}", shown("unins000.exe"))]
    );
    assert!(scan_folder(&folder.join("missing")).is_empty());
}

#[test]
fn missing_prefix_is_an_error() {
    let empty = tempfile::tempdir().unwrap();
    assert!(matches!(scan(empty.path()), Err(Error::MissingPrefix(_))));
}

#[test]
fn finds_shortcuts_in_both_start_menus() {
    let fake = Fake::new();
    fake.lnk(
        &format!("{MACHINE}/Hades/Hades.lnk"),
        r"C:\Program Files\Hades\Hades.exe",
    )
    .lnk(&format!("{USER}/Editor.lnk"), r"C:\Tools\editor.exe");
    assert_eq!(
        rows(&fake.scan().programs),
        [
            r"shortcut Editor C:\Tools\editor.exe",
            r"shortcut Hades C:\Program Files\Hades\Hades.exe",
        ]
    );
}

#[test]
fn a_shortcut_stands_in_for_its_executable() {
    let fake = Fake::new();
    fake.exe("drive_c/Program Files/Hades/Hades.exe")
        .exe("drive_c/Program Files/Hades/Helper.exe")
        .lnk(
            &format!("{MACHINE}/Hades.lnk"),
            r"C:\Program Files\hades\HADES.EXE",
        );
    // The shortcut names it, and the helper beside it is still listed.
    assert_eq!(
        rows(&fake.scan().programs),
        [
            r"shortcut Hades C:\Program Files\hades\HADES.EXE",
            r"exe Helper C:\Program Files\Hades\Helper.exe",
        ]
    );
}

#[test]
fn finds_loose_executables() {
    let fake = Fake::new();
    fake.exe("drive_c/GOG Games/Fly Corp/Fly Corp.exe")
        .exe("drive_c/users/me/AppData/Local/Programs/tool/tool.exe");
    assert_eq!(
        rows(&fake.scan().programs),
        [
            r"exe Fly Corp C:\GOG Games\Fly Corp\Fly Corp.exe",
            r"exe tool C:\users\me\AppData\Local\Programs\tool\tool.exe",
        ]
    );
}

#[test]
fn holds_back_noise() {
    let fake = Fake::new();
    fake.exe("drive_c/Program Files/Hades/Hades.exe")
        .exe("drive_c/Program Files/Hades/unins000.exe")
        .exe("drive_c/Program Files/Hades/UnrealCEFSubProcess.exe")
        .exe("drive_c/Program Files/Hades/HadesCrashHandler.exe")
        .exe("drive_c/Program Files/Common Files/vendor/DXSETUP.exe")
        .exe("drive_c/Program Files (x86)/Common Files/helper.exe")
        .exe(&format!("{MACHINE}/StartUp/autorun.exe"));
    let listing = fake.scan();
    assert_eq!(
        rows(&listing.programs),
        [
            r"exe Hades C:\Program Files\Hades\Hades.exe",
            r"exe UnrealCEFSubProcess C:\Program Files\Hades\UnrealCEFSubProcess.exe",
        ]
    );
    // Compared by membership rather than as a whole list, since wine's own
    // Program Files entries are in here too.
    for path in [
        r"C:\Program Files\Hades\unins000.exe",
        r"C:\Program Files\Hades\HadesCrashHandler.exe",
        r"C:\Program Files\Common Files\vendor\DXSETUP.exe",
        r"C:\Program Files (x86)\Common Files\helper.exe",
        r"C:\ProgramData\Microsoft\Windows\Start Menu\Programs\StartUp\autorun.exe",
    ] {
        assert!(
            listing.filtered.iter().any(|program| program.exe == path),
            "{path} was not held back"
        );
    }
}

// An uninstaller shortcut is still shown, because the installer chose to
// advertise it and hiding it would contradict the Start Menu.
#[test]
fn never_filters_a_shortcut() {
    let fake = Fake::new();
    fake.lnk(
        &format!("{MACHINE}/Uninstall Hades.lnk"),
        r"C:\Program Files\Hades\unins000.exe",
    );
    assert_eq!(
        rows(&fake.scan().programs),
        [r"shortcut Uninstall Hades C:\Program Files\Hades\unins000.exe"]
    );
}

#[test]
fn reports_shortcuts_it_cannot_read() {
    let fake = Fake::new();
    fake.file(&format!("{MACHINE}/Broken.lnk"), b"not a shortcut at all")
        .lnk(&format!("{MACHINE}/Command.lnk"), "C:")
        .lnk(&format!("{MACHINE}/Readme.lnk"), r"C:\Hades\readme.txt");
    let listing = fake.scan();
    assert!(listing.is_empty(), "{listing:?}");
    assert_eq!(
        listing.unreadable,
        [
            ("Broken".to_owned(), Unreadable::NotAShortcut),
            ("Command".to_owned(), Unreadable::DriveOnly),
            ("Readme".to_owned(), Unreadable::NotAProgram),
        ]
    );
}

#[test]
fn matches_start_menu_directories_whatever_the_case() {
    let fake = Fake::new();
    fake.lnk(
        "drive_c/programdata/microsoft/windows/start menu/programs/Hades.lnk",
        r"C:\Hades\Hades.exe",
    );
    assert_eq!(
        rows(&fake.scan().programs),
        [r"shortcut Hades C:\Hades\Hades.exe"]
    );
}

// A prefix's drive_c often links out to a game installed on the host, and
// walking through one would scan the whole filesystem.
#[test]
fn does_not_walk_through_symlinks() {
    let outside = tempfile::tempdir().unwrap();
    fs::write(outside.path().join("elsewhere.exe"), b"").unwrap();
    let fake = Fake::new();
    fake.exe("drive_c/Games/keep.exe");
    symlink(outside.path(), fake.0.path().join("drive_c/Linked")).unwrap();
    assert_eq!(rows(&fake.scan().programs), [r"exe keep C:\Games\keep.exe"]);
}

#[test]
fn stops_at_the_depth_cap() {
    let fake = Fake::new();
    let deep: String = (0..MAX_DEPTH + 4)
        .map(|level| format!("d{level}/"))
        .collect();
    fake.exe(&format!("drive_c/{deep}buried.exe"))
        .exe("drive_c/shallow.exe");
    assert_eq!(rows(&fake.scan().programs), [r"exe shallow C:\shallow.exe"]);
}
