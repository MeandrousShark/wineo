//! What the SteamGridDB and Steam key rows show, pinned for both providers
//! side by side so they can share one model without drifting.

use wineo_core::key_store::KeySource;

use super::*;

const SOURCES: [Option<KeySource>; 4] = [
    None,
    Some(KeySource::Variable),
    Some(KeySource::Keyring),
    Some(KeySource::File),
];

fn plain_text_warning(path: &Path) -> String {
    format!(
        "warning: no keyring, so the key is stored in plain text in {}",
        path.display()
    )
}

#[test]
fn status_labels_and_warnings_match_for_each_source() {
    let (_root, paths) = setup();
    let griddb_file = wineo_core::steamgriddb::key_file(&paths);
    let steam_file = wineo_core::steam::key::key_file(&paths);
    let expected = |variable: &str, file: &Path| {
        [
            ("No key".to_owned(), None),
            (format!("From {variable}"), None),
            ("Saved in the keyring".to_owned(), None),
            (
                format!("Saved in a plain text file: {}", file.display()),
                Some(plain_text_warning(file)),
            ),
        ]
    };
    let griddb = expected("WINEO_STEAMGRIDDB_KEY", &griddb_file);
    let steam = expected("WINEO_STEAM_KEY", &steam_file);
    for (index, source) in SOURCES.into_iter().enumerate() {
        let override_note = source == Some(KeySource::Variable);
        let status = steamgriddb::Status {
            source,
            override_note,
        };
        assert_eq!(status.label(&paths), griddb[index].0, "{source:?}");
        assert_eq!(status.warning(&paths), griddb[index].1, "{source:?}");
        let status = steam_key::Status {
            source,
            override_note,
        };
        assert_eq!(status.label(&paths), steam[index].0, "{source:?}");
        assert_eq!(status.warning(&paths), steam[index].1, "{source:?}");
    }
}

/// A page with each provider's row in the same state, which `edit` sets up
/// through the fields both share.
fn rows_with(
    paths: &Paths,
    source: Option<KeySource>,
    edit: impl Fn(&mut bool, &mut Option<String>, &mut Option<String>, &mut Option<String>),
) -> (Vec<String>, Vec<String>) {
    let mut page = global_page(paths);
    let status = |source: Option<KeySource>| (source, source == Some(KeySource::Variable));
    let (source, override_note) = status(source);
    page.steamgriddb.status = Some(steamgriddb::Status {
        source,
        override_note,
    });
    page.steam_key.status = Some(steam_key::Status {
        source,
        override_note,
    });
    page.steamgriddb.input_open = false;
    page.steam_key.input_open = false;
    edit(
        &mut page.steamgriddb.input_open,
        &mut page.steamgriddb.result,
        &mut page.steamgriddb.error,
        &mut page.steamgriddb.warning,
    );
    edit(
        &mut page.steam_key.input_open,
        &mut page.steam_key.result,
        &mut page.steam_key.error,
        &mut page.steam_key.warning,
    );
    (
        view::steamgriddb_row(&page.steamgriddb, paths).texts(),
        view::steam_key_row(&page.steam_key, paths).texts(),
    )
}

#[test]
fn rows_show_the_same_pieces_in_the_same_order() {
    let (_root, paths) = setup();
    let hint = "Get a key at https://steamcommunity.com/dev/apikey.".to_owned();

    // Nothing saved, nothing said.
    let (griddb, steam) = rows_with(&paths, None, |_, _, _, _| {});
    assert_eq!(griddb, ["Key", "No key", "Set key..."]);
    assert_eq!(steam, ["Key", "No key", hint.as_str(), "Set key..."]);

    // A variable names itself, says it overrides, and leaves both buttons.
    let (griddb, steam) = rows_with(&paths, Some(KeySource::Variable), |_, _, _, _| {});
    assert_eq!(
        griddb,
        [
            "Key",
            "From WINEO_STEAMGRIDDB_KEY",
            "WINEO_STEAMGRIDDB_KEY is set and overrides the saved key",
            "Set key...",
            "Remove key",
        ]
    );
    assert_eq!(
        steam,
        [
            "Key",
            "From WINEO_STEAM_KEY",
            "WINEO_STEAM_KEY is set and overrides the saved key",
            hint.as_str(),
            "Set key...",
            "Remove key",
        ]
    );

    // A result, then an error, come after the warning and before the hint.
    let (griddb, steam) = rows_with(
        &paths,
        Some(KeySource::Keyring),
        |_, result, error, warning| {
            *result = Some("Saved it".to_owned());
            *error = Some("It broke".to_owned());
            *warning = Some("careful".to_owned());
        },
    );
    assert_eq!(
        griddb,
        [
            "Key",
            "Saved in the keyring",
            "careful",
            "Saved it",
            "It broke",
            "Remove key"
        ]
    );
    assert_eq!(
        steam,
        [
            "Key",
            "Saved in the keyring",
            "careful",
            "Saved it",
            "It broke",
            hint.as_str(),
            "Remove key"
        ]
    );

    // The input swaps the buttons for the field's own.
    let (griddb, steam) = rows_with(&paths, None, |open, _, _, _| *open = true);
    assert_eq!(
        griddb,
        ["Key", "No key", "SteamGridDB API key", "Save", "Cancel"]
    );
    assert_eq!(
        steam,
        [
            "Key",
            "No key",
            hint.as_str(),
            "Steam Web API key",
            "Save",
            "Cancel"
        ]
    );
}

#[test]
fn rows_show_checking_then_failed_status_and_busy_labels() {
    let (_root, paths) = setup();
    let mut page = global_page(&paths);
    let texts = |page: &Page| {
        (
            view::steamgriddb_row(&page.steamgriddb, &paths).texts(),
            view::steam_key_row(&page.steam_key, &paths).texts(),
        )
    };
    page.steamgriddb.status = None;
    page.steam_key.status = None;
    let (griddb, steam) = texts(&page);
    assert_eq!(griddb[..2], ["Key", "Checking..."]);
    assert_eq!(steam[..2], ["Key", "Checking..."]);

    page.steamgriddb.status_error = Some("no bus".to_owned());
    page.steam_key.status_error = Some("no bus".to_owned());
    let (griddb, steam) = texts(&page);
    assert_eq!(griddb[..3], ["Key", "Key status unavailable", "no bus"]);
    assert_eq!(steam[..3], ["Key", "Key status unavailable", "no bus"]);

    for (busy, label) in [
        (steamgriddb::Busy::Saving, "Checking and saving..."),
        (steamgriddb::Busy::Removing, "Removing..."),
    ] {
        page.steamgriddb.busy = Some(busy);
        page.steam_key.busy = Some(busy);
        let (griddb, steam) = texts(&page);
        assert_eq!(griddb[3], label);
        assert_eq!(steam[3], label);
    }
}
