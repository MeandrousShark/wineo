//! Proxied environments, whose launcher owns the game, its prefix, and its
//! process.

mod common;

use std::fs;

use common::steam::SteamFixture;
use common::*;

const BG3: &str = "kind = \"proxied\"\nlauncher = \"steam\"\napp_id = \"1086940\"\n";

trait ProxiedFixture {
    fn write_config(&self, name: &str, text: &str);
    fn config_text(&self, name: &str) -> String;
}

impl ProxiedFixture for Fixture {
    fn write_config(&self, name: &str, text: &str) {
        let config = self.config_dir().join(format!("environments/{name}.toml"));
        fs::create_dir_all(config.parent().unwrap()).unwrap();
        fs::write(config, text).unwrap();
    }

    fn config_text(&self, name: &str) -> String {
        fs::read_to_string(self.config_dir().join(format!("environments/{name}.toml"))).unwrap()
    }
}

fn json(output: &std::process::Output) -> serde_json::Value {
    assert_success(output);
    serde_json::from_str(&stdout(output)).unwrap()
}

#[test]
fn list_and_status_show_a_proxied_environment() {
    let f = Fixture::new();
    let steamapps = f.root.path().join(".local/share/Steam/steamapps");
    fs::create_dir_all(&steamapps).unwrap();
    fs::write(
        steamapps.join("appmanifest_1086940.acf"),
        "\"AppState\" { \"appid\" \"1086940\" \"name\" \"Baldur's Gate 3\" \"StateFlags\" \"4\" }",
    )
    .unwrap();
    f.write_config(
        "bg3",
        &format!("{BG3}title = \"Baldur's Gate 3\"\ntags = [\"game\"]\n"),
    );
    f.write_config("dusk", "kind = \"native\"\n");
    f.environment("notepad", "");

    let listed = f.run_at_home(&["list"]);
    assert_success(&listed);
    assert!(stdout(&listed).starts_with("bg3"), "{}", stdout(&listed));

    let listed = json(&f.run_at_home(&["list", "--json"]));
    let environments = listed["environments"].as_array().unwrap();
    let by_name = |name: &str| {
        environments
            .iter()
            .find(|environment| environment["name"] == name)
            .unwrap()
            .clone()
    };
    let bg3 = by_name("bg3");
    assert_eq!(bg3["kind"], "proxied");
    assert_eq!(bg3["launcher"], "steam");
    assert_eq!(bg3["app_id"], "1086940");
    for (name, kind) in [("dusk", "native"), ("notepad", "wine")] {
        let environment = by_name(name);
        assert_eq!(environment["kind"], kind, "{name}");
        assert!(environment.get("launcher").is_none(), "{name}");
        assert!(environment.get("app_id").is_none(), "{name}");
    }

    let status = f.run_at_home(&["status", "bg3"]);
    assert_success(&status);
    let text = stdout(&status);
    assert!(text.starts_with("bg3: inactive\n"), "{text}");
    for line in [
        "  kind        proxied\n",
        "  launcher    steam\n",
        "  app id      1086940\n",
        "  title       Baldur's Gate 3\n",
        "  tags        game\n",
    ] {
        assert!(text.contains(line), "{line:?} in {text}");
    }
    assert!(!text.contains("runner"), "{text}");
    assert!(!text.contains("prefix"), "{text}");

    let status = json(&f.run_at_home(&["status", "bg3", "--json"]));
    assert_eq!(status["kind"], "proxied");
    assert_eq!(status["launcher"], "steam");
    assert_eq!(status["app_id"], "1086940");
    assert_eq!(status["runner"], serde_json::Value::Null);
    assert_eq!(status["prefix"], serde_json::Value::Null);

    for (name, kind) in [("dusk", "native"), ("notepad", "wine")] {
        let status = json(&f.run_at_home(&["status", name, "--json"]));
        assert_eq!(status["kind"], kind, "{name}");
        assert!(status.get("launcher").is_none(), "{name}");
        assert!(status.get("app_id").is_none(), "{name}");
    }
}

/// Global launch defaults never reach a proxied environment, so `status`
/// doesn't list them either.
#[test]
fn global_defaults_never_show_for_a_proxied_environment() {
    let f = Fixture::new();
    f.global_config("gamemode = true\nmangohud = true\nwayland = true\n[env]\nA = \"1\"\n");
    f.write_config("bg3", BG3);
    let status = json(&f.run(&["status", "bg3", "--json"]));
    for key in ["gamemode", "mangohud", "wayland", "gamescope"] {
        assert_eq!(status[key], false, "{key}");
    }
    let text = stdout(&f.run(&["status", "bg3"]));
    for key in ["gamemode", "mangohud", "wayland"] {
        assert!(!text.contains(key), "{key} in {text}");
    }
}

#[test]
fn launching_a_proxied_environment_hands_off_to_steam_and_runs_hooks() {
    let s = SteamFixture::new();
    let marker = s.f.root.path().join("hooks");
    s.config(&format!(
        "[hooks]\nbefore = 'printf \"before\\n\" >> {}'\nafter = 'printf \"after\\n\" >> {}'\n",
        marker.display(),
        marker.display()
    ));
    let started = s.run(&["start", "civ", "steam"]);
    assert_success(&started);
    assert!(stdout(&started).contains("Asked Steam to start civ."));
    wait_for("claimed Steam game", || {
        s.f.state("civ").contains("running_at = ")
    });
    assert_success(&s.run(&["stop", "civ"]));
    s.f.wait_for_run_end("civ", "stopped");
    wait_for("first after hook", || {
        fs::read_to_string(&marker).is_ok_and(|text| text.lines().count() == 2)
    });
    assert_eq!(fs::read_to_string(&marker).unwrap().lines().count(), 2);

    s.set_mode("exit");
    let run = s.run(&["run", "civ", "--target", "steam"]);
    assert_eq!(run.status.code(), Some(0), "{}", stderr(&run));
    assert_eq!(fs::read_to_string(&marker).unwrap().lines().count(), 4);

    s.set_mode("hold");
    assert_success(&s.run(&["start", "civ"]));
    wait_for("second Steam game", || {
        s.f.state("civ").contains("running_at = ")
    });
    assert_success(&s.run(&["restart", "civ"]));
    wait_for("restarted Steam game", || s.dispatch_count() == 4);
    assert_success(&s.run(&["stop", "civ"]));
    s.f.wait_for_run_end("civ", "stopped");
    wait_for("last after hook", || {
        fs::read_to_string(&marker).is_ok_and(|text| text.lines().count() == 8)
    });
    assert_eq!(fs::read_to_string(&marker).unwrap().lines().count(), 8);
}

#[test]
fn prefix_commands_refuse_a_proxied_environment() {
    let f = Fixture::new();
    f.write_config("bg3", BG3);
    let portable = f.root.path().join("portable");
    for args in [
        &["exec", "bg3", "winecfg"][..],
        &["winetricks", "bg3", "corefonts"][..],
        &["programs", "bg3"][..],
        &["snapshot", "create", "bg3", "clean"][..],
        &["snapshot", "list", "bg3"][..],
        &["snapshot", "restore", "bg3", "clean"][..],
        &["snapshot", "delete", "bg3", "clean"][..],
        &["snapshot", "recover", "bg3"][..],
        &["clone", "bg3", "bg3-copy"][..],
        &["clone", "bg3", "bg3-copy", "--dir", "/tmp"][..],
        &[
            "portable",
            "copy",
            "bg3",
            "--to",
            portable.to_str().unwrap(),
        ][..],
        &["rm", "bg3", "--delete-prefix"][..],
    ] {
        let refused = f.run(args);
        assert!(!refused.status.success(), "{args:?}");
        assert_eq!(
            stderr(&refused),
            "error: Steam manages bg3's prefix.\n",
            "{args:?}"
        );
    }
    assert_eq!(f.config_text("bg3"), BG3);
    assert!(!f.config_dir().join("environments/bg3-copy.toml").exists());
    assert!(!portable.join("config").exists());
}

#[test]
fn setup_refuses_a_proxied_environment() {
    let f = Fixture::new();
    f.write_config("bg3", BG3);
    let refused = f.run(&["setup", "bg3"]);
    assert!(!refused.status.success());
    assert_eq!(
        stderr(&refused),
        "error: bg3 is a Steam game, so it can't set targets.\n"
    );
}

#[test]
fn plain_rm_moves_a_proxied_environment_to_trash() {
    let f = Fixture::new();
    f.write_config("bg3", &format!("{BG3}desktop = true\n"));
    let entry = f
        .root
        .path()
        .join("data/applications/wineo-bg3-steam.desktop");
    assert_success(&f.run_at_home(&["set", "bg3", "desktop=true"]));
    assert!(entry.is_file());
    let removed = f.run_at_home(&["rm", "bg3"]);
    assert_success(&removed);
    assert!(
        stdout(&removed).starts_with("Removed environment bg3 to trash "),
        "{}",
        stdout(&removed)
    );
    assert!(!f.config_dir().join("environments/bg3.toml").exists());
    assert!(!entry.exists());
    let removed_text = stdout(&removed);
    let id = removed_text
        .lines()
        .next()
        .unwrap()
        .split_whitespace()
        .last()
        .unwrap()
        .to_owned();
    assert_success(&f.run_at_home(&["restore", &id]));
    assert!(entry.is_file());
}

#[test]
fn set_changes_only_what_a_proxied_environment_has() {
    let f = Fixture::new();
    f.write_config("bg3", &format!("# mine\n{BG3}"));
    for setting in [
        "tags=['game']",
        "title=Baldur's Gate 3",
        "desktop=true",
        "hooks.after=true",
    ] {
        assert_success(&f.run_at_home(&["set", "bg3", setting]));
    }
    let text = f.config_text("bg3");
    let entry = f
        .root
        .path()
        .join("data/applications/wineo-bg3-steam.desktop");
    assert!(entry.is_file());
    assert!(
        fs::read_to_string(&entry)
            .unwrap()
            .contains("Exec=wineo start bg3\n")
    );
    let desktop = f.run_at_home(&["desktop"]);
    assert_success(&desktop);
    let listed = stdout(&desktop);
    assert!(listed.contains("steam"), "{listed}");
    assert!(listed.contains("Baldur's Gate 3"), "{listed}");
    assert!(text.starts_with("# mine\n"), "{text}");
    for line in [
        "tags = ['game']",
        "title = \"Baldur's Gate 3\"",
        "desktop = true",
        "after = \"true\"",
    ] {
        assert!(text.contains(line), "{line} in {text}");
    }
    assert_success(&f.run_at_home(&["set", "bg3", "--unset", "title", "--unset", "desktop"]));
    assert!(!entry.exists());
    let unset = f.config_text("bg3");
    assert!(!unset.contains("title"), "{unset}");
    assert!(!unset.contains("desktop"), "{unset}");

    for (setting, key) in [
        ("runner=wine", "runner"),
        ("prefix=/tmp/p", "prefix"),
        ("gamemode=true", "gamemode"),
        ("mangohud=true", "mangohud"),
        ("wayland=true", "wayland"),
        ("gamescope.width=1920", "gamescope"),
        ("winetricks=['corefonts']", "winetricks"),
        ("gameid=umu-1", "gameid"),
        ("store=gog", "store"),
        ("dxvk=latest", "dxvk"),
        ("vkd3d=latest", "vkd3d"),
        ("env.A=1", "env"),
        ("targets.game.exe=/bin/true", "targets"),
    ] {
        let refused = f.run(&["set", "bg3", setting]);
        assert!(!refused.status.success(), "{setting}");
        assert_eq!(
            stderr(&refused),
            format!("error: bg3 is a Steam game, so it can't set {key}.\n"),
            "{setting}"
        );
    }
    assert_eq!(f.config_text("bg3"), unset);
}

#[test]
fn proxied_only_keys_are_refused_on_wine_and_native() {
    let f = Fixture::new();
    f.write_config("dusk", "kind = \"native\"\n");
    f.environment("notepad", "");
    for name in ["dusk", "notepad"] {
        let before = f.config_text(name);
        for (setting, key) in [("title=Dusk", "title"), ("desktop=true", "desktop")] {
            let refused = f.run(&["set", name, setting]);
            assert!(!refused.status.success(), "{name} {setting}");
            assert!(
                stderr(&refused).contains(&format!(
                    "{name} isn't a Steam game, so it can't set {key}."
                )),
                "{}",
                stderr(&refused)
            );
        }
        assert_eq!(f.config_text(name), before);
    }
}

#[test]
fn validate_checks_steam_for_a_proxied_environment() {
    let f = Fixture::new();
    f.write_config("bg3", BG3);
    let checked = f.run_at_home(&["validate", "bg3"]);
    assert!(!checked.status.success());
    assert!(
        stdout(&checked).contains("Steam isn't installed"),
        "stdout: {}\nstderr: {}",
        stdout(&checked),
        stderr(&checked)
    );
}
