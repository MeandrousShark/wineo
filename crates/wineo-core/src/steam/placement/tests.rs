use std::cell::Cell;
use std::fs;

use super::*;
use crate::source::Platform::{Native, Windows};
use crate::steam::appinfo::{self, AppInfo};
use crate::steam::appinfo_fixture::{self, App};
use crate::steam::parse_manifest;

const ID: &str = "42";
const INSTALLED: u32 = 4;

/// What appinfo says about the game under test.
#[derive(Clone, Copy)]
enum Info {
    /// The whole file can't be read.
    Unreadable,
    /// The file has other apps but not this one.
    Missing,
    /// The game has an entry, with this `oslist` or none.
    Oslist(Option<&'static str>),
}

/// One game on disk: its manifest flags (None for no manifest), whether it has
/// a Proton prefix, the tools map, and what appinfo says.
struct Case {
    flags: Option<u32>,
    pfx: bool,
    empty_compatdata: bool,
    tools: Vec<(&'static str, &'static str)>,
    info: Info,
}

fn case(info: Info) -> Case {
    Case {
        flags: Some(INSTALLED),
        pfx: false,
        empty_compatdata: false,
        tools: Vec::new(),
        info,
    }
}

impl Case {
    fn pfx(mut self) -> Self {
        self.pfx = true;
        self
    }

    fn tools(mut self, tools: &[(&'static str, &'static str)]) -> Self {
        self.tools = tools.to_vec();
        self
    }

    fn flags(mut self, flags: Option<u32>) -> Self {
        self.flags = flags;
        self
    }

    fn place(&self) -> Placement {
        let library = tempfile::tempdir().unwrap();
        let root = library.path();
        let compatdata = root.join("steamapps/compatdata").join(ID);
        if self.pfx {
            fs::create_dir_all(compatdata.join("pfx")).unwrap();
        } else if self.empty_compatdata {
            fs::create_dir_all(&compatdata).unwrap();
        }
        let entry = self.flags.map(|flags| ManifestEntry {
            app_id: ID.to_owned(),
            library: root.to_owned(),
            manifest: parse_manifest(
                &format!(
                    "\"AppState\" {{ \"appid\" \"{ID}\" \"name\" \"Game\" \"StateFlags\" \"{flags}\" }}"
                ),
                ID,
            ),
        });
        let tools: HashMap<_, _> = self
            .tools
            .iter()
            .map(|(id, name)| (id.to_ascii_lowercase(), (*name).to_owned()))
            .collect();
        let parsed = match self.info {
            Info::Unreadable => None,
            Info::Missing => Some(file(&[appinfo_fixture::game(7, Some("linux"))])),
            Info::Oslist(oslist) => Some(file(&[appinfo_fixture::game(42, oslist)])),
        };
        place(entry.as_ref(), &tools, ID, || parsed.as_ref())
    }
}

fn file(apps: &[App<'_>]) -> AppInfo {
    appinfo::parse(&appinfo_fixture::build(apps)).unwrap()
}

fn platform(case: Case) -> Option<crate::source::Platform> {
    case.place().platform
}

#[test]
fn an_own_mapping_is_windows_even_when_appinfo_has_linux() {
    let tools = [(ID, "GE-Proton9-20")];
    for oslist in [Some("windows,linux"), Some("linux"), Some("windows"), None] {
        let placed = case(Info::Oslist(oslist)).tools(&tools).place();
        assert_eq!(placed.platform, Some(Windows), "{oslist:?}");
        assert_eq!(placed.proton.as_deref(), Some("GE-Proton9-20"));
    }
    // With no appinfo at all, and no prefix, the mapping still decides.
    let placed = case(Info::Unreadable).tools(&tools).place();
    assert_eq!(placed.platform, Some(Windows));
}

#[test]
fn an_own_mapping_is_found_whatever_the_case_of_the_id() {
    let mut tools = HashMap::new();
    tools.insert("abc".to_owned(), "GE-Proton".to_owned());
    let entry = ManifestEntry {
        app_id: "abc".to_owned(),
        library: std::env::temp_dir(),
        manifest: parse_manifest("\"AppState\" { \"appid\" \"1\" \"StateFlags\" \"4\" }", "1"),
    };
    for app_id in ["abc", "AbC", "ABC"] {
        let placed = place(Some(&entry), &tools, app_id, || None);
        assert_eq!(placed.platform, Some(Windows), "{app_id}");
    }
}

#[test]
fn a_steam_linux_runtime_is_not_a_mapping_in_any_case() {
    for name in [
        "steamlinuxruntime_soldier",
        "SteamLinuxRuntime_sniper",
        "STEAMLINUXRUNTIME",
        "SteamLinuxRuntime",
    ] {
        let tools = [(ID, name)];
        // It falls through to oslist...
        let placed = case(Info::Oslist(Some("linux"))).tools(&tools).place();
        assert_eq!(placed, Placement::default().with(Native), "{name}");
        let placed = case(Info::Oslist(Some("windows"))).tools(&tools).place();
        assert_eq!(placed.platform, Some(Windows), "{name}");
        // ... and with nothing else to go on, to the prefix rule.
        assert_eq!(platform(case(Info::Oslist(None)).tools(&tools)), None);
        assert_eq!(
            platform(case(Info::Oslist(None)).tools(&tools).pfx()),
            Some(Windows)
        );
        // It doesn't show as the Proton build either, so the default does.
        let tools = [(ID, name), ("0", "proton_experimental")];
        let placed = case(Info::Oslist(Some("windows"))).tools(&tools).place();
        assert_eq!(placed.proton.as_deref(), Some("proton_experimental"));
        let placed = case(Info::Oslist(Some("windows")))
            .tools(&[(ID, name)])
            .place();
        assert_eq!(placed.proton, None, "{name}");
    }
}

#[test]
fn a_custom_name_counts_as_proton() {
    for name in [
        "GE-Proton9-20",
        "my-own-build",
        "proton_experimental",
        "steamlinux",
    ] {
        let placed = case(Info::Oslist(Some("linux")))
            .tools(&[(ID, name)])
            .place();
        assert_eq!(placed.platform, Some(Windows), "{name}");
        assert_eq!(placed.proton.as_deref(), Some(name));
    }
}

impl Placement {
    fn with(mut self, platform: crate::source::Platform) -> Self {
        self.platform = Some(platform);
        self
    }
}

#[test]
fn an_empty_own_entry_is_not_a_mapping() {
    let placed = case(Info::Oslist(Some("linux"))).tools(&[(ID, "")]).place();
    assert_eq!(placed.platform, Some(Native));
}

#[test]
fn linux_in_the_oslist_beats_a_stale_prefix() {
    for oslist in [
        "linux",
        "windows,linux",
        "windows,macos,linux",
        "linux,windows",
    ] {
        let placed = case(Info::Oslist(Some(oslist)))
            .pfx()
            .tools(&[("0", "proton_experimental")])
            .place();
        assert_eq!(placed, Placement::default().with(Native), "{oslist}");
    }
}

#[test]
fn a_windows_only_game_is_windows_with_no_prefix() {
    // Before its first run there is no compatdata folder at all.
    let placed = case(Info::Oslist(Some("windows")))
        .tools(&[("0", "proton_experimental")])
        .place();
    assert_eq!(placed.platform, Some(Windows));
    assert_eq!(placed.proton.as_deref(), Some("proton_experimental"));
}

#[test]
fn oslist_is_matched_as_whole_items() {
    for (oslist, expected) in [
        ("macos", Windows),
        ("windows,macos", Windows),
        ("windows,macos,linux", Native),
        ("linuxish", Windows),
        ("nolinux", Windows),
        ("linux64", Windows),
        ("macos,linux", Native),
        ("windows, linux", Native),
    ] {
        let placed = platform(case(Info::Oslist(Some(oslist))));
        assert_eq!(placed, Some(expected), "{oslist}");
    }
}

#[test]
fn thin_appinfo_takes_the_prefix_fallback_with_and_without_a_prefix() {
    for info in [
        Info::Oslist(Some("")),
        Info::Oslist(None),
        Info::Missing,
        Info::Unreadable,
    ] {
        assert_eq!(platform(case(info)), None);
        assert_eq!(platform(case(info).pfx()), Some(Windows));
    }
}

#[test]
fn only_a_prefix_inside_compatdata_counts() {
    let mut empty = case(Info::Oslist(None));
    empty.empty_compatdata = true;
    assert_eq!(empty.place(), Placement::default());
}

#[test]
fn a_game_that_is_not_fully_installed_gets_nothing() {
    let tools = [(ID, "GE-Proton9-20"), ("0", "proton_experimental")];
    // No manifest, a manifest that isn't installed, being installed or
    // removed, and one that doesn't parse.
    for flags in [
        None,
        Some(0),
        Some(1),
        Some(2),
        Some(1024),
        Some(2048),
        Some(4 | 2048),
    ] {
        for info in [
            Info::Oslist(Some("windows")),
            Info::Oslist(Some("linux")),
            Info::Unreadable,
        ] {
            let placed = case(info).tools(&tools).pfx().flags(flags).place();
            assert_eq!(placed, Placement::default(), "{flags:?}");
        }
    }
    let broken = ManifestEntry {
        app_id: ID.to_owned(),
        library: std::env::temp_dir(),
        manifest: Err("has no StateFlags".to_owned()),
    };
    let tools: HashMap<_, _> = tools
        .iter()
        .map(|(id, name)| ((*id).to_owned(), (*name).to_owned()))
        .collect();
    assert_eq!(
        place(Some(&broken), &tools, ID, || None),
        Placement::default()
    );
    assert_eq!(place(None, &tools, ID, || None), Placement::default());
}

#[test]
fn a_game_being_updated_is_still_placed() {
    let placed = case(Info::Oslist(Some("windows")))
        .flags(Some(INSTALLED | 1024))
        .place();
    assert_eq!(placed.platform, Some(Windows));
    let placed = case(Info::Oslist(None))
        .pfx()
        .flags(Some(INSTALLED | 2))
        .place();
    assert_eq!(placed.platform, Some(Windows));
}

#[test]
fn the_proton_row_comes_from_the_own_entry_then_the_default() {
    let both =
        case(Info::Oslist(Some("windows"))).tools(&[(ID, "GE-Proton"), ("0", "proton_default")]);
    assert_eq!(both.place().proton.as_deref(), Some("GE-Proton"));
    let default_only = case(Info::Oslist(Some("windows"))).tools(&[("0", "proton_default")]);
    assert_eq!(
        default_only.place().proton.as_deref(),
        Some("proton_default")
    );
    let none = case(Info::Oslist(Some("windows")));
    assert_eq!(none.place().proton, None);
    // Windows by prefix takes the default too.
    let by_prefix = case(Info::Unreadable)
        .pfx()
        .tools(&[("0", "proton_default")]);
    assert_eq!(by_prefix.place().proton.as_deref(), Some("proton_default"));
}

#[test]
fn native_and_unplaced_games_show_no_proton() {
    let tools = [("0", "proton_default")];
    let native = case(Info::Oslist(Some("linux"))).tools(&tools).place();
    assert_eq!(native, Placement::default().with(Native));
    let unplaced = case(Info::Oslist(None)).tools(&tools).place();
    assert_eq!(unplaced, Placement::default());
    let thin = case(Info::Missing).tools(&tools).place();
    assert_eq!(thin, Placement::default());
}

#[test]
fn a_runtime_default_is_not_a_proton_row() {
    let placed = case(Info::Oslist(Some("windows")))
        .tools(&[("0", "SteamLinuxRuntime_sniper")])
        .place();
    assert_eq!(placed.platform, Some(Windows));
    assert_eq!(placed.proton, None);
}

#[test]
fn a_native_game_with_an_empty_compatdata_folder_and_a_default_shows_native_only() {
    // The reported case: a Linux build, Steam made the folder but no prefix,
    // there is no entry of its own, and the global default is set.
    let mut factorio =
        case(Info::Oslist(Some("windows,linux"))).tools(&[("0", "proton_experimental")]);
    factorio.empty_compatdata = true;
    assert_eq!(factorio.place(), Placement::default().with(Native));
}

#[test]
fn an_app_id_of_zero_does_not_find_the_default_as_its_own_entry() {
    let entry = ManifestEntry {
        app_id: "0".to_owned(),
        library: std::env::temp_dir(),
        manifest: parse_manifest("\"AppState\" { \"appid\" \"0\" \"StateFlags\" \"4\" }", "0"),
    };
    let tools = HashMap::from([("0".to_owned(), "proton_default".to_owned())]);
    let placed = place(Some(&entry), &tools, "0", || None);
    assert_eq!(placed.platform, None);
    assert_eq!(placed.proton, None);
}

#[test]
fn an_app_id_that_is_not_digits_never_becomes_a_path_or_a_lookup() {
    let library = tempfile::tempdir().unwrap();
    let library = library.path();
    // compatdata/../pfx would be steamapps/pfx.
    fs::create_dir_all(library.join("steamapps/compatdata")).unwrap();
    fs::create_dir_all(library.join("steamapps/pfx")).unwrap();
    let entry = ManifestEntry {
        app_id: ID.to_owned(),
        library: library.to_owned(),
        manifest: parse_manifest("\"AppState\" { \"appid\" \"42\" \"StateFlags\" \"4\" }", ID),
    };
    let info = file(&[appinfo_fixture::game(42, Some("linux"))]);
    for app_id in ["..", "", "1/../..", "4x", "+42"] {
        let placed = place(Some(&entry), &HashMap::new(), app_id, || Some(&info));
        assert_eq!(placed, Placement::default(), "{app_id:?}");
    }
}

#[test]
fn appinfo_is_asked_only_when_the_oslist_rule_needs_it() {
    let asked = Cell::new(0);
    let info = file(&[appinfo_fixture::game(42, Some("linux"))]);
    let run = |flags: Option<u32>, tools: &[(&str, &str)]| {
        let entry = flags.map(|flags| ManifestEntry {
            app_id: ID.to_owned(),
            library: std::env::temp_dir(),
            manifest: parse_manifest(
                &format!("\"AppState\" {{ \"appid\" \"42\" \"StateFlags\" \"{flags}\" }}"),
                ID,
            ),
        });
        let tools: HashMap<_, _> = tools
            .iter()
            .map(|(id, name)| ((*id).to_owned(), (*name).to_owned()))
            .collect();
        place(entry.as_ref(), &tools, ID, || {
            asked.set(asked.get() + 1);
            Some(&info)
        })
    };
    run(None, &[]);
    run(Some(0), &[]);
    run(Some(4), &[(ID, "GE-Proton")]);
    assert_eq!(asked.get(), 0);
    run(Some(4), &[]);
    assert_eq!(asked.get(), 1);
}
