use super::*;
use crate::epic::fake::{CODE, Fake, Setup, fixture};

fn code() -> LoginCode {
    LoginCode::parse(CODE).unwrap()
}

#[test]
fn status_reads_the_account_and_the_logged_out_text() {
    let setup = Setup::new();
    let legendary = setup.legendary(&Fake::default());
    assert_eq!(
        status(&legendary, &Token::new()).unwrap(),
        Status::LoggedOut
    );
    setup.log_in();
    assert_eq!(
        status(&legendary, &Token::new()).unwrap(),
        Status::LoggedIn {
            account: "REDACTED".to_owned()
        }
    );
    assert!(setup.sweep().is_empty());
}

#[test]
fn a_status_without_an_account_is_an_error_naming_it() {
    let setup = Setup::new();
    let missing = setup.root.path().join("no-account.json");
    std::fs::write(
        &missing,
        r#"{"config_directory": "/x", "games_available": 0}"#,
    )
    .unwrap();
    let mistyped = setup.root.path().join("mistyped.json");
    std::fs::write(&mistyped, r#"{"account": null}"#).unwrap();
    for status_out in [missing, mistyped] {
        let legendary = setup.legendary(&Fake {
            status_out,
            ..Fake::default()
        });
        assert_eq!(
            status(&legendary, &Token::new()).unwrap_err().to_string(),
            "Couldn't read what `legendary status --offline --json` printed: `account` is missing or isn't text."
        );
    }
    let legendary = setup.legendary(&Fake {
        noise: Some("status"),
        ..Fake::default()
    });
    assert_eq!(
        status(&legendary, &Token::new()).unwrap_err().to_string(),
        "Couldn't read what `legendary status --offline --json` printed: it isn't JSON."
    );
    assert!(setup.sweep().is_empty());
}

#[test]
fn login_checks_who_is_logged_in_afterwards() {
    let setup = Setup::new();
    let legendary = setup.legendary(&Fake::default());
    assert_eq!(
        login(&legendary, &code(), &Token::new()).unwrap(),
        "REDACTED"
    );
    assert!(setup.logged_in());
    let calls = setup.calls();
    assert!(calls.contains(&format!("auth --code {CODE}")), "{calls:?}");
    assert!(!calls.contains(&"auth --delete".to_owned()), "{calls:?}");
    assert!(setup.sweep().is_empty());
}

#[test]
fn a_refused_login_that_exits_0_is_an_error() {
    let setup = Setup::new();
    let legendary = setup.legendary(&Fake {
        code: "somethingelse".to_owned(),
        ..Fake::default()
    });
    let error = login(&legendary, &code(), &Token::new()).unwrap_err();
    assert_eq!(
        error.to_string(),
        "Epic didn't accept that login (Login to EGS API failed with errorCode: \
         errors.com.epicgames.account.oauth.authorization_code_not_found). \
         Log in again and paste the new code."
    );
    assert!(!setup.logged_in());
    assert!(fixture("login-failed.stderr").exists());
    assert!(setup.sweep().is_empty());
}

#[test]
fn a_saved_login_is_deleted_before_the_new_code_is_used() {
    let setup = Setup::new();
    setup.log_in();
    let legendary = setup.legendary(&Fake::default());
    assert_eq!(
        login(&legendary, &code(), &Token::new()).unwrap(),
        "REDACTED"
    );
    let calls: Vec<String> = setup
        .calls()
        .into_iter()
        .filter(|call| call.starts_with("auth"))
        .collect();
    assert_eq!(
        calls,
        ["auth --delete".to_owned(), format!("auth --code {CODE}")]
    );
    assert!(setup.sweep().is_empty());
}

#[test]
fn a_logout_that_keeps_the_old_account_stops_the_login() {
    let setup = Setup::new();
    setup.log_in();
    let legendary = setup.legendary(&Fake {
        delete_works: false,
        ..Fake::default()
    });
    let error = login(&legendary, &code(), &Token::new()).unwrap_err();
    assert_eq!(
        error.to_string(),
        "Couldn't log out of Epic: Legendary still has REDACTED logged in."
    );
    // `auth --code` would have kept the old account and exited 0.
    assert!(
        !setup
            .calls()
            .iter()
            .any(|call| call.starts_with("auth --code")),
        "{:?}",
        setup.calls()
    );
    assert!(setup.sweep().is_empty());
}

#[test]
fn logout_deletes_the_login_and_the_cached_list() {
    let setup = Setup::new();
    setup.log_in();
    let cache = setup.paths.epic_owned_cache();
    std::fs::create_dir_all(cache.parent().unwrap()).unwrap();
    std::fs::write(&cache, "{}").unwrap();
    let legendary = setup.legendary(&Fake::default());
    assert!(logout(&legendary, &Token::new()).unwrap());
    assert!(!setup.logged_in());
    assert!(!cache.exists());
    assert!(!logout(&legendary, &Token::new()).unwrap());

    setup.log_in();
    let stuck = setup.legendary(&Fake {
        delete_works: false,
        ..Fake::default()
    });
    assert!(matches!(
        logout(&stuck, &Token::new()),
        Err(Error::StillLoggedIn { .. })
    ));
    assert!(setup.sweep().is_empty());
}

#[test]
fn a_hanging_login_is_stopped() {
    let setup = Setup::new();
    let mut legendary = setup.legendary(&Fake {
        hang: Some("auth"),
        ..Fake::default()
    });
    legendary.set_timeout(std::time::Duration::from_millis(500));
    let error = login(&legendary, &code(), &Token::new()).unwrap_err();
    assert_eq!(
        error.to_string(),
        "`legendary auth --code` didn't finish in 0 seconds."
    );
    assert!(!error.to_string().contains(CODE));
    assert!(setup.sweep().is_empty());
}

#[test]
fn the_pasted_page_or_code_is_read() {
    let page = format!(
        r#"{{"warning":"Do not share this code with any 3rd party service.","redirectUrl":"https://localhost/launcher/authorized?code={CODE}","authorizationCode":"{CODE}","exchangeCode":null,"sid":null}}"#
    );
    for pasted in [
        CODE.to_owned(),
        format!("  {CODE}\n"),
        format!("\"{CODE}\""),
        page,
    ] {
        assert_eq!(
            LoginCode::parse(&pasted).unwrap().reveal(),
            CODE,
            "{pasted}"
        );
    }
    for pasted in [
        "",
        "--delete",
        "abc def",
        "{\"authorizationCode\": null}",
        "{not json",
    ] {
        assert!(
            matches!(LoginCode::parse(pasted), Err(Error::NotACode)),
            "{pasted}"
        );
    }
}

#[test]
fn the_code_never_shows_in_debug_or_errors() {
    let secret = "SecretCode0123456789";
    let code = LoginCode::parse(secret).unwrap();
    assert_eq!(format!("{code:?}"), "<redacted>");
    assert!(!format!("{:?}", Some(&code)).contains(secret));

    let near_miss = format!("{secret}!");
    let error = LoginCode::parse(&near_miss).unwrap_err();
    assert!(!error.to_string().contains(secret));
    assert!(!format!("{error:?}").contains(secret));
    let page = format!(r#"{{"authorizationCode": 5, "redirectUrl": "{secret}"}}"#);
    let error = LoginCode::parse(&page).unwrap_err();
    assert!(!error.to_string().contains(secret));
    assert!(!format!("{error:?}").contains(secret));
}

#[test]
fn the_code_is_hidden_when_legendary_repeats_it() {
    let setup = Setup::new();
    let secret = "SecretCode0123456789";
    let code = LoginCode::parse(secret).unwrap();
    let legendary = setup.legendary(&Fake {
        echo_code: true,
        ..Fake::default()
    });
    let output = legendary
        .run(&["auth", "--code"], Some(code.reveal()), &Token::new())
        .unwrap();
    assert!(
        output.stderr.contains("Login with code <redacted> failed"),
        "{}",
        output.stderr
    );
    assert!(!format!("{output:?}").contains(secret));

    let error = login(&legendary, &code, &Token::new()).unwrap_err();
    assert_eq!(
        error.to_string(),
        "Epic didn't accept that login (Login with code <redacted> failed). \
         Log in again and paste the new code."
    );
    assert!(!format!("{error:?}").contains(secret), "{error:?}");
    assert!(setup.sweep().is_empty());
}

#[test]
fn a_failing_auth_never_quotes_the_code() {
    let setup = Setup::new();
    let secret = "SecretCode0123456789";
    let code = LoginCode::parse(secret).unwrap();
    // Exits 1 with the code as its last line, as a traceback would.
    let crashing = setup.bin.join("legendary");
    crate::test_script::write_script(
        &crashing,
        "#!/bin/sh\nif [ \"$1\" = --version ]; then echo 'legendary version \"0.21.1\", codename \"x\"'; exit 0; fi\nif [ \"$1\" = status ]; then echo '{\"account\": \"<not logged in>\"}'; exit 0; fi\necho \"ValueError: bad code $3\" >&2\nexit 1\n",
    );
    let legendary = setup.find(setup.env());
    let error = login(&legendary, &code, &Token::new()).unwrap_err();
    assert_eq!(
        error.to_string(),
        "`legendary auth --code` failed: exit code 1: ValueError: bad code <redacted>"
    );
    assert!(!format!("{error:?}").contains(secret));
    assert!(setup.sweep().is_empty());
}
