mod common;

use std::fs;

use common::{Fixture, assert_success, stdout};

fn config(f: &Fixture, name: &str, text: &str) {
    let environments = f.config_dir().join("environments");
    fs::create_dir_all(&environments).unwrap();
    fs::write(environments.join(format!("{name}.toml")), text).unwrap();
}

#[test]
fn install_states_appear_in_status_list_and_json() {
    let f = Fixture::new();
    config(
        &f,
        "civ",
        "kind = \"proxied\"\nlauncher = \"steam\"\napp_id = \"289070\"\n",
    );
    let steamapps = f.root.path().join(".local/share/Steam/steamapps");
    let manifest = steamapps.join("appmanifest_289070.acf");
    let gone = f.root.path().join("unmounted-games");
    for (state, flags, problem) in [
        ("unknown", None, Some("Steam isn't found".to_owned())),
        ("installed", Some(4), None),
        ("updating", Some(1030), None),
        ("install_queued", Some(2), None),
        ("installing", Some(1026), None),
        ("uninstalling", Some(4 | 2048), None),
        ("not_installed", Some(0), None),
        (
            "unavailable",
            None,
            Some(format!(
                "Steam's library at {} isn't available",
                gone.display()
            )),
        ),
        ("unknown", None, Some("has no StateFlags".to_owned())),
    ] {
        if let Some(flags) = flags {
            fs::create_dir_all(&steamapps).unwrap();
            fs::write(&manifest, format!("\"AppState\" {{ \"appid\" \"289070\" \"name\" \"Civ\" \"StateFlags\" \"{flags}\" }}")).unwrap();
        } else if state == "unavailable" {
            fs::remove_file(&manifest).unwrap();
            fs::write(
                steamapps.join("libraryfolders.vdf"),
                format!(
                    "\"libraryfolders\" {{ \"1\" {{ \"path\" \"{}\" }} }}",
                    gone.display()
                ),
            )
            .unwrap();
        } else if steamapps.exists() {
            fs::write(&manifest, "\"AppState\" { \"appid\" \"289070\" }").unwrap();
        }
        let displayed = match state {
            "installed" => "inactive",
            "not_installed" => "not installed",
            "install_queued" => "install queued",
            other => other,
        };
        let status = f.run_at_home(&["status", "civ"]);
        assert_success(&status);
        let text = stdout(&status);
        assert!(text.starts_with(&format!("civ: {displayed}")), "{text}");
        if let Some(problem) = &problem {
            assert!(text.contains(problem), "{text}");
        }
        let list = f.run_at_home(&["list"]);
        assert_success(&list);
        assert!(stdout(&list).contains(displayed), "{}", stdout(&list));
        for args in [&["status", "civ", "--json"][..], &["list", "--json"][..]] {
            let output = f.run_at_home(args);
            assert_success(&output);
            let json: serde_json::Value = serde_json::from_str(&stdout(&output)).unwrap();
            let environment = if args[0] == "list" {
                &json["environments"][0]
            } else {
                &json
            };
            assert_eq!(environment["install"], state, "{environment}");
            // Run status stays what it was while the install state moves.
            assert_eq!(environment["state"], "inactive", "{environment}");
            assert!(environment["install_progress"].is_null(), "{environment}");
            if let Some(problem) = &problem {
                assert!(
                    environment["install_problem"]
                        .as_str()
                        .unwrap()
                        .contains(problem),
                    "{environment}"
                );
            } else {
                assert!(
                    environment.get("install_problem").is_none(),
                    "{environment}"
                );
            }
        }
    }
}

#[test]
fn wine_and_native_omit_steam_install_fields() {
    let f = Fixture::new();
    f.environment("notepad", "");
    config(&f, "dusk", "kind = \"native\"\n");
    let output = f.run_at_home(&["list", "--json"]);
    assert_success(&output);
    let json: serde_json::Value = serde_json::from_str(&stdout(&output)).unwrap();
    for environment in json["environments"].as_array().unwrap() {
        assert!(environment.get("install").is_none());
        assert!(environment.get("install_problem").is_none());
        assert!(environment.get("install_progress").is_none());
    }
    for name in ["notepad", "dusk"] {
        let output = f.run_at_home(&["status", name, "--json"]);
        assert_success(&output);
        let json: serde_json::Value = serde_json::from_str(&stdout(&output)).unwrap();
        assert!(json.get("install").is_none());
        assert!(json.get("install_problem").is_none());
        assert!(json.get("install_progress").is_none());
    }
}

#[test]
fn install_progress_is_a_percent_while_state_stays_the_run_status() {
    let f = Fixture::new();
    config(
        &f,
        "civ",
        "kind = \"proxied\"\nlauncher = \"steam\"\napp_id = \"289070\"\n",
    );
    let steamapps = f.root.path().join(".local/share/Steam/steamapps");
    fs::create_dir_all(&steamapps).unwrap();
    let manifest = steamapps.join("appmanifest_289070.acf");
    for (flags, to_download, downloaded, progress) in [
        (1026, 0, 0, None),
        (1026, 400, 100, Some(25)),
        (1026, 400, 0, Some(0)),
        (4, 0, 0, None),
    ] {
        fs::write(
            &manifest,
            format!("\"AppState\" {{ \"appid\" \"289070\" \"name\" \"Civ\" \"StateFlags\" \"{flags}\" \"BytesToDownload\" \"{to_download}\" \"BytesDownloaded\" \"{downloaded}\" }}"),
        )
        .unwrap();
        for args in [&["status", "civ", "--json"][..], &["list", "--json"][..]] {
            let output = f.run_at_home(args);
            assert_success(&output);
            let json: serde_json::Value = serde_json::from_str(&stdout(&output)).unwrap();
            let environment = if args[0] == "list" {
                &json["environments"][0]
            } else {
                &json
            };
            assert_eq!(environment["state"], "inactive", "{environment}");
            assert_eq!(
                environment["install_progress"],
                serde_json::json!(progress),
                "{environment}"
            );
        }
    }
}
