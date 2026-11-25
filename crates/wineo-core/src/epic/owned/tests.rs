use super::*;
use crate::epic::fake::{Fake, Setup, fixture};

const T: i64 = 1_800_000_000;

fn titles(owned: &Owned) -> Vec<&str> {
    owned.games.iter().map(|game| game.title.as_str()).collect()
}

fn list_text() -> String {
    fs::read_to_string(fixture("list.json")).unwrap()
}

#[test]
fn the_probes_list_reads_whole() {
    let games = parse_list(&list_text()).unwrap();
    assert_eq!(
        games
            .iter()
            .map(|game| game.app_name.as_str())
            .collect::<Vec<_>>(),
        [
            "AppToken",
            "Dlc Game",
            "AppPrereq",
            "AppOffline",
            "AppThirdParty"
        ]
    );
    let token = &games[0];
    assert_eq!(token.title, "Token Game 2020");
    assert_eq!(token.version.as_deref(), Some("1260-win"));
    assert!(token.can_run_offline);
    assert_eq!(token.partner_link, None);
    assert!(token.dlc.is_empty());

    let dlc = &games[1];
    assert!(!dlc.can_run_offline);
    assert_eq!(
        dlc.dlc
            .iter()
            .map(|dlc| (dlc.app_name.as_str(), dlc.title.as_str()))
            .collect::<Vec<_>>(),
        [
            (
                "0000000000000000000000000000000c",
                "Dlc Game Extra One Content"
            ),
            (
                "0000000000000000000000000000000e",
                "Extra Two Dlc Game Content"
            ),
        ]
    );
    assert_eq!(games[2].title, "Prereq Game\u{ae}");
    assert_eq!(games[4].partner_link.as_deref(), Some("ubisoft"));
    assert_eq!(games[4].third_party, None);
}

/// The fixture list with one field of one game changed.
fn edited(edit: impl FnOnce(&mut Value)) -> String {
    let mut list: Value = serde_json::from_str(&list_text()).unwrap();
    edit(&mut list);
    list.to_string()
}

#[test]
fn a_missing_or_mistyped_field_is_an_error_naming_it() {
    let cases: Vec<(String, &str)> = vec![
        (
            edited(|list| {
                list[1].as_object_mut().unwrap().remove("app_title");
            }),
            "[1] (Dlc Game): `app_title` is missing or isn't text",
        ),
        (
            edited(|list| list[0]["app_name"] = Value::from(7)),
            "[0]: `app_name` is missing or isn't text",
        ),
        (
            edited(|list| {
                list[1]["dlcs"][1]
                    .as_object_mut()
                    .unwrap()
                    .remove("app_name");
            }),
            "[1] (Dlc Game).dlcs[1]: `app_name` is missing or isn't text",
        ),
        (
            edited(|list| list[0]["dlcs"] = Value::Null),
            "[0] (AppToken): `dlcs` is missing or isn't a list",
        ),
        (
            edited(|list| list[3]["asset_infos"]["Windows"]["build_version"] = Value::from(1)),
            "[3] (AppOffline): `asset_infos.Windows.build_version` is missing or isn't text",
        ),
        (
            edited(|list| {
                list[0]["metadata"]["customAttributes"]["CanRunOffline"] =
                    serde_json::json!({"type": "BOOLEAN", "value": true})
            }),
            "[0] (AppToken): `metadata.customAttributes.CanRunOffline.value` is missing or isn't text",
        ),
        ("{}".to_owned(), "it isn't a list"),
        (
            "Something Legendary never printed\n[]".to_owned(),
            "it isn't JSON",
        ),
    ];
    for (text, problem) in cases {
        assert_eq!(
            parse_list(&text).unwrap_err().to_string(),
            format!("Couldn't read what `legendary list --json -T` printed: {problem}.")
        );
    }
    // A game with no Windows build reads, without a version.
    let mac = parse_list(&edited(|list| {
        list[0]["asset_infos"] = serde_json::json!({"Mac": {"build_version": "1"}})
    }))
    .unwrap();
    assert_eq!(mac[0].version, None);
}

#[test]
fn the_list_is_fetched_cached_and_used_for_an_hour() {
    let setup = Setup::new();
    setup.log_in();
    let legendary = setup.legendary(&Fake::default());
    let token = Token::new();
    let owned = load_at(&legendary, false, T, &token).unwrap();
    assert_eq!(owned.account, "REDACTED");
    assert_eq!(owned.games.len(), 5);
    assert_eq!(owned.warning, None);
    let lists = || {
        setup
            .calls()
            .iter()
            .filter(|call| call.starts_with("list"))
            .count()
    };
    assert_eq!(lists(), 1);
    assert!(setup.calls().contains(&"list --json -T".to_owned()));

    assert_eq!(load_at(&legendary, false, T + 3599, &token).unwrap(), owned);
    assert_eq!(lists(), 1);
    load_at(&legendary, false, T + 3600, &token).unwrap();
    assert_eq!(lists(), 2);
    load_at(&legendary, true, T + 3601, &token).unwrap();
    assert_eq!(lists(), 3);
    assert!(setup.sweep().is_empty());
}

#[test]
fn a_failed_refresh_falls_back_to_the_last_whole_list() {
    let setup = Setup::new();
    setup.log_in();
    let legendary = setup.legendary(&Fake::default());
    let before = load_at(&legendary, false, T, &Token::new()).unwrap();

    let cut = setup.root.path().join("cut.json");
    fs::write(&cut, &list_text()[..200]).unwrap();
    let broken = setup.legendary(&Fake {
        list: cut,
        ..Fake::default()
    });
    let owned = load_at(&broken, true, T + 10, &Token::new()).unwrap();
    assert_eq!(titles(&owned), titles(&before));
    assert_eq!(
        owned.warning.as_deref(),
        Some(
            "Couldn't refresh owned Epic games: Couldn't read what `legendary list --json -T` printed: it isn't JSON."
        )
    );

    let failing = setup.legendary(&Fake {
        list_fails: Some(fixture("logged-out-list.stderr")),
        ..Fake::default()
    });
    let owned = load_at(&failing, true, T + 20, &Token::new()).unwrap();
    assert_eq!(titles(&owned), titles(&before));
    assert!(
        owned
            .warning
            .as_deref()
            .unwrap()
            .ends_with("failed: exit code 1: ValueError: No saved credentials"),
        "{owned:?}"
    );
    // The old list stands in; it isn't replaced.
    assert_eq!(
        read_cache(&setup.paths.epic_owned_cache())
            .unwrap()
            .fetched_at,
        T
    );
    assert!(setup.sweep().is_empty());
}

#[test]
fn a_failed_fetch_with_nothing_cached_is_an_error() {
    let setup = Setup::new();
    setup.log_in();
    let missing = setup.root.path().join("missing.json");
    fs::write(
        &missing,
        edited(|list| {
            list[2].as_object_mut().unwrap().remove("app_title");
        }),
    )
    .unwrap();
    let legendary = setup.legendary(&Fake {
        list: missing,
        ..Fake::default()
    });
    let error = load_at(&legendary, false, T, &Token::new()).unwrap_err();
    assert_eq!(
        error.to_string(),
        "Couldn't get owned Epic games: Couldn't read what `legendary list --json -T` printed: \
         [2] (AppPrereq): `app_title` is missing or isn't text."
    );
    assert!(!setup.paths.epic_owned_cache().exists());
    assert!(setup.sweep().is_empty());
}

#[test]
fn logged_out_is_an_error_even_with_a_cache() {
    let setup = Setup::new();
    setup.log_in();
    let legendary = setup.legendary(&Fake::default());
    load_at(&legendary, false, T, &Token::new()).unwrap();
    fs::remove_file(crate::epic::state_dir(&setup.paths).join("user.json")).unwrap();
    let error = load_at(&legendary, false, T + 1, &Token::new()).unwrap_err();
    assert!(matches!(error, Error::LoggedOut), "{error}");
    let lists = setup
        .calls()
        .iter()
        .filter(|call| call.starts_with("list"))
        .count();
    assert_eq!(lists, 1, "{:?}", setup.calls());
    assert!(setup.sweep().is_empty());
}

#[test]
fn another_accounts_cache_is_never_used() {
    let setup = Setup::new();
    setup.log_in();
    let legendary = setup.legendary(&Fake::default());
    let cache = Cache {
        fetched_at: T,
        account: "SomeoneElse".to_owned(),
        games: Vec::new(),
    };
    write_cache(&setup.paths.epic_owned_cache(), &cache).unwrap();
    let owned = load_at(&legendary, false, T + 1, &Token::new()).unwrap();
    assert_eq!(owned.games.len(), 5);
    assert_eq!(owned.account, "REDACTED");
    assert!(setup.sweep().is_empty());
}

#[test]
fn a_hanging_list_is_stopped_and_falls_back() {
    let setup = Setup::new();
    setup.log_in();
    let legendary = setup.legendary(&Fake::default());
    let before = load_at(&legendary, false, T, &Token::new()).unwrap();
    let mut hanging = setup.legendary(&Fake {
        hang: Some("list"),
        ..Fake::default()
    });
    hanging.set_timeout(std::time::Duration::from_millis(500));
    let owned = load_at(&hanging, true, T + 1, &Token::new()).unwrap();
    assert_eq!(titles(&owned), titles(&before));
    assert_eq!(
        owned.warning.as_deref(),
        Some(
            "Couldn't refresh owned Epic games: `legendary list --json -T` didn't finish in 0 seconds."
        )
    );
    assert!(setup.sweep().is_empty());
}
