//! What is installed in a prefix.
//!
//! A fresh prefix has no shortcuts, and the only executables it has outside
//! `drive_c/windows` are the fixed handful wine puts in Program Files. Both
//! are known up front, so anything else was installed. That is why nothing
//! here compares against a snapshot taken beforehand, and why the scan works
//! the same on a prefix wineo adopted as on one it created.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::shortcut::{self, Unreadable};
use crate::winpath;

/// Where wine keeps the all-users Start Menu.
const MACHINE_START_MENU: &str = r"C:\ProgramData\Microsoft\Windows\Start Menu\Programs";
/// The same thing inside a user profile, which is where most installers write.
const USER_START_MENU: &str = r"AppData\Roaming\Microsoft\Windows\Start Menu\Programs";
/// Wine's own directory, holding all but six of a fresh prefix's executables.
const WINE_DIR: &str = "windows";
/// The rest of what wine ships, under either Program Files. Verified against
/// wine 11.17: a fresh prefix has these and nothing else installed.
const WINE_PROGRAMS: [&str; 3] = [
    r"internet explorer\iexplore.exe",
    r"windows media player\wmplayer.exe",
    r"windows nt\accessories\wordpad.exe",
];
/// How deep the executable walk goes. Installed trees are nowhere near this,
/// and the cap means a strange prefix cannot recurse without end.
const MAX_DEPTH: usize = 24;

/// How wineo found a program.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Source {
    /// A Start Menu shortcut, so the installer meant to advertise it.
    Shortcut,
    /// An executable sitting in the prefix, found by looking.
    Executable,
}

impl Source {
    pub fn as_str(self) -> &'static str {
        match self {
            Source::Shortcut => "shortcut",
            Source::Executable => "exe",
        }
    }
}

/// A program a prefix has in it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Program {
    pub source: Source,
    /// What to call it: the shortcut's name, or the executable's file stem.
    pub name: String,
    /// The Windows path, which is what a target's `exe` holds.
    pub exe: String,
}

/// Everything a scan turned up.
#[derive(Debug, Default)]
pub struct Listing {
    pub programs: Vec<Program>,
    /// Executables held back as noise, such as uninstallers.
    pub filtered: Vec<Program>,
    /// Shortcuts with no usable target, by name, and why.
    pub unreadable: Vec<(String, Unreadable)>,
}

impl Listing {
    pub fn is_empty(&self) -> bool {
        self.programs.is_empty()
    }

    /// Every program found, the noise included, in one list.
    pub fn all(&self) -> Vec<Program> {
        let mut all = self.programs.clone();
        all.extend(self.filtered.iter().cloned());
        tidy(&mut all);
        all
    }
}

/// Everything installed in `wine_prefix`, which is the directory holding
/// `drive_c`. A caller with a configured prefix passes
/// `runner.wine_prefix(prefix)`, since Proton keeps its prefix a level down.
pub fn scan(wine_prefix: &Path) -> Result<Listing> {
    let drive_c = wine_prefix.join("drive_c");
    if !drive_c.is_dir() {
        return Err(Error::MissingPrefix(wine_prefix.to_owned()));
    }

    let mut listing = Listing::default();
    for dir in start_menus(wine_prefix) {
        collect_shortcuts(&dir, &mut listing);
    }
    tidy(&mut listing.programs);
    listing.unreadable.sort();
    listing.unreadable.dedup();

    // A shortcut already says what a program is called, so the same executable
    // is not offered a second time under its file name.
    let advertised: BTreeSet<String> = listing
        .programs
        .iter()
        .map(|program| program.exe.to_ascii_lowercase())
        .collect();
    collect_executables(&drive_c, &advertised, &mut listing);
    Ok(listing)
}

/// The Start Menu directories to look in. Resolved as Windows paths so the
/// lookup is case-insensitive, since wine and installers disagree on case.
fn start_menus(wine_prefix: &Path) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = winpath::to_unix(wine_prefix, MACHINE_START_MENU)
        .into_iter()
        .collect();
    // Which profile exists depends on the runner: wine uses the host user name
    // and Proton uses `steamuser`, so every profile is checked.
    if let Some(users) = winpath::to_unix(wine_prefix, r"C:\users") {
        let mut names: Vec<_> = read_dir_sorted(&users)
            .into_iter()
            .filter_map(|entry| entry.file_name().into_string().ok())
            .collect();
        names.sort();
        for name in names {
            let path = format!(r"C:\users\{name}\{USER_START_MENU}");
            dirs.extend(winpath::to_unix(wine_prefix, &path));
        }
    }
    dirs
}

fn collect_shortcuts(dir: &Path, listing: &mut Listing) {
    for entry in read_dir_sorted(dir) {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            collect_shortcuts(&path, listing);
            continue;
        }
        if !has_extension(&entry.file_name().to_string_lossy(), "lnk") {
            continue;
        }
        let name = stem(&path);
        match fs::read(&path).map(|bytes| shortcut::parse(&bytes)) {
            Ok(Ok(exe)) => listing.programs.push(Program {
                source: Source::Shortcut,
                name,
                exe,
            }),
            Ok(Err(reason)) => listing.unreadable.push((name, reason)),
            Err(_) => listing.unreadable.push((name, Unreadable::NotAShortcut)),
        }
    }
}

fn collect_executables(drive_c: &Path, advertised: &BTreeSet<String>, listing: &mut Listing) {
    let mut found = Vec::new();
    let mut filtered = Vec::new();
    walk(drive_c, &mut Vec::new(), &mut |relative| {
        let exe = windows_path(relative);
        if advertised.contains(&exe.to_ascii_lowercase()) {
            return;
        }
        let program = Program {
            source: Source::Executable,
            name: relative
                .last()
                .map(|name| stem(Path::new(name)))
                .unwrap_or_default(),
            exe,
        };
        if is_noise(relative) {
            &mut filtered
        } else {
            &mut found
        }
        .push(program);
    });
    for list in [&mut found, &mut filtered] {
        tidy(list);
    }
    listing.programs.extend(found);
    listing.filtered.extend(filtered);
}

/// Shortcuts first, then by name as someone reads it rather than by byte, so
/// `autorun` does not sort after `DXSETUP`.
fn tidy(programs: &mut Vec<Program>) {
    programs.sort_by_key(|program| {
        (
            program.source,
            program.name.to_lowercase(),
            program.exe.to_lowercase(),
        )
    });
    programs.dedup();
}

/// Calls `found` with the path components, relative to `drive_c`, of every
/// executable below it.
fn walk(dir: &Path, relative: &mut Vec<String>, found: &mut impl FnMut(&[String])) {
    if relative.len() >= MAX_DEPTH {
        return;
    }
    for entry in read_dir_sorted(dir) {
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        // `file_type` does not follow symlinks, so a link pointing out of the
        // prefix, or back into it, is never walked.
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            // Everything a fresh prefix has lives here, so it is never entered.
            if relative.is_empty() && name.eq_ignore_ascii_case(WINE_DIR) {
                continue;
            }
            relative.push(name);
            walk(&entry.path(), relative, found);
            relative.pop();
        } else if kind.is_file() && has_extension(&name, "exe") {
            relative.push(name);
            found(relative);
            relative.pop();
        }
    }
}

/// Executables that are never the program someone wanted. Only the loose ones
/// are filtered: a shortcut is what the installer chose to advertise, so
/// second-guessing it would hide the real answer.
fn is_noise(relative: &[String]) -> bool {
    let Some((file, dirs)) = relative.split_last() else {
        return true;
    };
    let file = file.to_ascii_lowercase();
    if file.starts_with("unins")
        || file.starts_with("uninstall")
        || file.starts_with("vcredist")
        || file.starts_with("dotnet")
        || file.contains("crashhandler")
        || file == "crashreportclient.exe"
        || file == "dxsetup.exe"
    {
        return true;
    }
    // Redistributables ship here, and a Start Menu StartUp entry is a stub
    // rather than the app.
    let dirs: Vec<String> = dirs.iter().map(|dir| dir.to_ascii_lowercase()).collect();
    if dirs
        .windows(2)
        .any(|pair| pair[0].starts_with("program files") && pair[1] == "common files")
        || (dirs.iter().any(|dir| dir == "start menu") && dirs.iter().any(|dir| dir == "startup"))
    {
        return true;
    }
    // Wine's own Program Files entries, which no one installed.
    match dirs.split_first() {
        Some((root, rest)) if root.starts_with("program files") => {
            let tail = rest
                .iter()
                .map(String::as_str)
                .chain([file.as_str()])
                .collect::<Vec<_>>()
                .join("\\");
            WINE_PROGRAMS.contains(&tail.as_str())
        }
        _ => false,
    }
}

/// Components under `drive_c`, as the Windows path a config holds.
fn windows_path(relative: &[String]) -> String {
    let mut path = String::from("C:");
    for part in relative {
        path.push('\\');
        path.push_str(part);
    }
    path
}

fn has_extension(name: &str, extension: &str) -> bool {
    Path::new(name)
        .extension()
        .is_some_and(|found| found.eq_ignore_ascii_case(extension))
}

fn stem(path: &Path) -> String {
    path.file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}

/// Directory entries in a stable order, and nothing at all when the directory
/// cannot be read, so one bad corner does not lose the whole scan.
fn read_dir_sorted(dir: &Path) -> Vec<fs::DirEntry> {
    let mut entries: Vec<_> = fs::read_dir(dir).into_iter().flatten().flatten().collect();
    entries.sort_by_key(|entry| entry.file_name());
    entries
}

#[cfg(test)]
mod tests {
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
}
