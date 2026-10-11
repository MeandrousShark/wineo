use std::time::Duration;

use serde_json::json;

use super::*;
use crate::error::Class;
use crate::fake::{FakeGog, Reply, Request, json as reply, raw, refused};
use crate::owned::MAX_PAGES;
use crate::{Compatibility, Platforms, parse_paste};

const CODE: &str = "CODESECRET0123456789";
const REFRESH: &str = "REFRESHSECRET0123456789";
const ACCESS: &str = "ACCESSSECRET0123456789";

fn tokens_body(refresh: &str) -> String {
    json!({
        "access_token": ACCESS,
        "refresh_token": refresh,
        "expires_in": 3600,
        "user_id": "48628349957132247",
        "token_type": "bearer",
        "session_id": "ignored",
    })
    .to_string()
}

/// Every way an error could be shown, which must hold no secret and no URL.
fn assert_clean(error: &Error) {
    let shown = format!("{error} {error:?} {error:#?} {}", error.kind());
    for secret in [
        CODE,
        REFRESH,
        ACCESS,
        CLIENT_SECRET,
        "127.0.0.1",
        "?",
        "token=",
    ] {
        assert!(!shown.contains(secret), "{secret} in {shown}");
    }
}

#[test]
fn the_code_exchange_sends_galaxy_s_credentials_and_reads_tokens() {
    let server = FakeGog::start(|_| reply(200, &tokens_body(REFRESH)));
    let code = parse_paste(CODE).unwrap();
    let tokens = server.client().exchange_code(&code).unwrap();
    assert_eq!(tokens.access_token.expose(), ACCESS);
    assert_eq!(tokens.refresh_token.expose(), REFRESH);
    assert_eq!(tokens.expires_in, 3600);
    assert_eq!(tokens.user_id.to_string(), "48628349957132247");

    let request = &server.requests()[0];
    assert_eq!((request.method.as_str(), request.path()), ("GET", "/token"));
    assert_eq!(request.query("client_id").as_deref(), Some(CLIENT_ID));
    assert_eq!(
        request.query("client_secret").as_deref(),
        Some(CLIENT_SECRET)
    );
    assert_eq!(
        request.query("grant_type").as_deref(),
        Some("authorization_code")
    );
    assert_eq!(request.query("code").as_deref(), Some(CODE));
    assert_eq!(request.query("redirect_uri").as_deref(), Some(REDIRECT_URI));
    assert_eq!(request.header("user-agent"), Some("wineo-test/1"));
    assert_eq!(request.header("authorization"), None);
}

#[test]
fn a_refresh_sends_the_refresh_token_and_reads_the_new_one() {
    let server = FakeGog::start(|_| reply(200, &tokens_body("NEWREFRESH")));
    let tokens = server.client().refresh(REFRESH).unwrap();
    assert_eq!(tokens.refresh_token.expose(), "NEWREFRESH");
    let request = &server.requests()[0];
    assert_eq!(
        request.query("grant_type").as_deref(),
        Some("refresh_token")
    );
    assert_eq!(request.query("refresh_token").as_deref(), Some(REFRESH));
    assert_eq!(
        request.query("client_secret").as_deref(),
        Some(CLIENT_SECRET)
    );
}

#[test]
fn a_token_answer_missing_a_field_is_malformed() {
    for missing in ["access_token", "refresh_token", "expires_in", "user_id"] {
        let mut body: serde_json::Value = serde_json::from_str(&tokens_body(REFRESH)).unwrap();
        body.as_object_mut().unwrap().remove(missing);
        let body = body.to_string();
        let server = FakeGog::start(move |_| reply(200, &body));
        let error = server.client().refresh(REFRESH).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::Malformed, "{missing}");
        assert_eq!(error.class(), Class::Malformed);
        assert_clean(&error);
    }
}

type Handler = Box<dyn Fn(&Request) -> Reply + Send + Sync>;

fn refresh_error(answer: impl Fn(&Request) -> Reply + Send + Sync + 'static) -> Error {
    let server = FakeGog::start(answer);
    server.client().refresh(REFRESH).unwrap_err()
}

#[test]
fn token_failures_are_classified() {
    let invalid_grant = json!({
        "error": "invalid_grant",
        "error_description": format!("refresh token {REFRESH} is invalid"),
    })
    .to_string();
    let cases: Vec<(Handler, ErrorKind, Class)> = vec![
        (
            Box::new(move |_| reply(400, &invalid_grant)),
            ErrorKind::InvalidGrant,
            Class::LoggedOut,
        ),
        (
            Box::new(|_| reply(400, r#"{"error":"invalid_client"}"#)),
            ErrorKind::Status(400),
            Class::Other,
        ),
        (
            Box::new(|_| reply(400, "not json")),
            ErrorKind::Status(400),
            Class::Other,
        ),
        (
            Box::new(|_| reply(401, "{}")),
            ErrorKind::Unauthorized,
            Class::LoggedOut,
        ),
        (
            Box::new(|_| reply(429, "{}")),
            ErrorKind::RateLimited,
            Class::Temporary,
        ),
        (
            Box::new(|_| reply(500, "{}")),
            ErrorKind::Server(500),
            Class::Temporary,
        ),
        (
            Box::new(|_| reply(503, "{}")),
            ErrorKind::Server(503),
            Class::Temporary,
        ),
        (
            Box::new(|_| reply(404, "{}")),
            ErrorKind::Status(404),
            Class::Other,
        ),
        (
            Box::new(move |_| reply(200, &format!("{{\"echo\":\"{REFRESH}\" oops"))),
            ErrorKind::Json,
            Class::Malformed,
        ),
    ];
    for (answer, kind, class) in cases {
        let error = refresh_error(answer);
        assert_eq!((error.kind(), error.class()), (kind, class));
        assert_eq!(error.endpoint(), Endpoint::Token);
        assert_eq!(
            error.to_string(),
            format!("GOG token request failed: {kind}")
        );
        assert_clean(&error);
    }
}

#[test]
fn a_refused_connection_is_temporary() {
    let client = Client::with_base("wineo-test/1", &refused()).unwrap();
    let error = client.refresh(REFRESH).unwrap_err();
    assert_eq!(
        (error.kind(), error.class()),
        (ErrorKind::Connect, Class::Temporary)
    );
    assert_clean(&error);
}

#[test]
fn stalled_headers_and_bodies_time_out() {
    let server = FakeGog::start(|_| Reply::Stall(Vec::new(), Duration::from_secs(2)));
    let error = server.quick_client().refresh(REFRESH).unwrap_err();
    assert_eq!(
        (error.kind(), error.class()),
        (ErrorKind::Timeout, Class::Temporary)
    );
    assert_clean(&error);

    let cut = b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\n{\"acc".to_vec();
    let server = FakeGog::start(move |_| Reply::Stall(cut.clone(), Duration::from_secs(2)));
    let error = server.quick_client().refresh(REFRESH).unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Timeout);
    assert_clean(&error);
}

#[test]
fn a_body_cut_short_is_temporary() {
    let cut = b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\nConnection: close\r\n\r\n{\"a".to_vec();
    let server = FakeGog::start(move |_| Reply::Send(cut.clone()));
    let error = server.client().refresh(REFRESH).unwrap_err();
    assert_eq!(
        (error.kind(), error.class()),
        (ErrorKind::Body, Class::Temporary)
    );
}

#[test]
fn an_oversized_answer_is_too_large() {
    let server = FakeGog::start(|_| reply(200, &format!("{{\"pad\":\"{}\"}}", "x".repeat(64))));
    let mut client = server.client();
    client.max_body = 32;
    let error = client.refresh(REFRESH).unwrap_err();
    assert_eq!(error.kind(), ErrorKind::TooLarge);
}

#[test]
fn redirects_are_not_followed() {
    let target = FakeGog::start(|_| reply(200, &tokens_body(REFRESH)));
    let location = format!("{}/elsewhere", target.base);
    let server = FakeGog::start(move |_| {
        Reply::Send(
            format!("HTTP/1.1 302 Found\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .into_bytes(),
        )
    });
    let error = server.client().refresh(REFRESH).unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Status(302));
    assert!(target.requests().is_empty());
}

#[test]
fn the_production_client_refuses_plain_http_and_defaults_to_gog() {
    let client = Client::build(
        "wineo-test/1",
        Endpoints::all_at("http://127.0.0.1:9").unwrap(),
        true,
        CONNECT_TIMEOUT,
        READ_TIMEOUT,
    )
    .unwrap();
    let error = client.refresh(REFRESH).unwrap_err();
    assert_eq!(error.class(), Class::Temporary);
    assert_clean(&error);

    let production = Client::new("wineo/0").unwrap();
    for url in [
        &production.endpoints().auth,
        &production.endpoints().embed,
        &production.endpoints().api,
        &production.endpoints().content_system,
    ] {
        assert_eq!(url.scheme(), "https");
        assert!(url.host_str().unwrap().ends_with(".gog.com"));
    }
    assert_eq!(production.login_url(), crate::login_url());
}

fn owned_page(page: u32, total: u32, ids: &[u64]) -> String {
    let products: Vec<_> = ids
        .iter()
        .map(|id| {
            json!({
                "id": id,
                "title": format!("Game {id}"),
                "image": format!("//images-1.gog-statics.com/{id}"),
                "url": "/game/x",
                "worksOn": {"Windows": true, "Mac": false, "Linux": id % 2 == 0},
                "slug": format!("game_{id}"),
                "dlcCount": 3,
                "category": "Strategy",
            })
        })
        .collect();
    json!({
        "page": page,
        "totalPages": total,
        "totalProducts": 99,
        "productsPerPage": 100,
        "products": products,
    })
    .to_string()
}

#[test]
fn the_owned_list_follows_every_page_with_the_token() {
    let server = FakeGog::start(|request| match request.query("page").as_deref() {
        Some("1") => reply(200, &owned_page(1, 3, &[1, 2])),
        Some("2") => reply(200, &owned_page(2, 3, &[3, 2])),
        Some("3") => reply(200, &owned_page(3, 3, &[4])),
        _ => reply(404, "{}"),
    });
    let games = server.client().owned_games(ACCESS).unwrap();
    let ids: Vec<_> = games.iter().map(|game| game.id).collect();
    assert_eq!(ids, [1, 2, 3, 4]);
    assert_eq!(
        games[1],
        OwnedProduct {
            id: 2,
            title: "Game 2".to_owned(),
            image: Some("https://images-1.gog-statics.com/2".to_owned()),
            works_on: Platforms {
                windows: true,
                mac: false,
                linux: true,
            },
            slug: Some("game_2".to_owned()),
            dlc_count: 3,
        }
    );
    let requests = server.requests();
    assert_eq!(requests.len(), 3);
    for request in &requests {
        assert_eq!(request.path(), "/account/getFilteredProducts");
        assert_eq!(request.query("mediaType").as_deref(), Some("1"));
        assert_eq!(
            request.header("authorization"),
            Some(format!("Bearer {ACCESS}").as_str())
        );
    }
}

#[test]
fn an_empty_library_is_one_page() {
    let server = FakeGog::start(|_| reply(200, &owned_page(1, 0, &[])));
    assert_eq!(server.client().owned_games(ACCESS).unwrap(), []);
    assert_eq!(server.requests().len(), 1);
}

#[test]
fn any_page_failing_fails_the_whole_list() {
    let failures: Vec<(&str, ErrorKind)> = vec![
        ("500", ErrorKind::Server(500)),
        ("junk", ErrorKind::Json),
        ("shape", ErrorKind::Malformed),
        ("wrong-page", ErrorKind::Malformed),
        ("401", ErrorKind::Unauthorized),
    ];
    for (failure, kind) in failures {
        let server = FakeGog::start(move |request| match request.query("page").as_deref() {
            Some("1") => reply(200, &owned_page(1, 2, &[1])),
            _ => match failure {
                "500" => reply(500, "{}"),
                "junk" => reply(200, "{"),
                "shape" => reply(200, r#"{"page":2,"totalPages":2}"#),
                "wrong-page" => reply(200, &owned_page(1, 2, &[1])),
                _ => reply(401, "{}"),
            },
        });
        let error = server.client().owned_games(ACCESS).unwrap_err();
        assert_eq!(error.kind(), kind, "{failure}");
        assert_eq!(error.endpoint(), Endpoint::Owned);
        // A 401 here is the access token, not the saved login.
        assert_ne!(error.class(), Class::LoggedOut);
        assert_clean(&error);
    }
}

#[test]
fn an_absurd_page_count_is_malformed() {
    let server = FakeGog::start(|_| reply(200, &owned_page(1, MAX_PAGES + 1, &[1])));
    let error = server.client().owned_games(ACCESS).unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Malformed);
    assert_eq!(server.requests().len(), 1);
}

#[test]
fn a_page_total_that_changes_mid_fetch_fails_the_whole_list() {
    // Grows on page 2, and shrinks on page 2 of a longer list.
    for (first_total, second_total) in [(2, 3), (3, 2)] {
        let server = FakeGog::start(move |request| match request.query("page").as_deref() {
            Some("1") => reply(200, &owned_page(1, first_total, &[1])),
            Some("2") => reply(200, &owned_page(2, second_total, &[2])),
            _ => reply(200, &owned_page(3, 3, &[3])),
        });
        let error = server.client().owned_games(ACCESS).unwrap_err();
        assert_eq!(
            (error.endpoint(), error.kind()),
            (Endpoint::Owned, ErrorKind::Malformed),
            "{first_total} then {second_total}"
        );
        // Nothing past the page that disagreed is asked for.
        assert_eq!(server.requests().len(), 2);
    }
}

#[test]
fn product_details_round_trip_through_serde() {
    let server = FakeGog::start(|_| reply(200, &product_body("https://api.gog.com")));
    let product = server.client().product(1094900565, None).unwrap();
    let text = serde_json::to_string(&product).unwrap();
    assert_eq!(serde_json::from_str::<Product>(&text).unwrap(), product);
}

#[test]
fn details_for_another_product_are_malformed() {
    let server = FakeGog::start(|_| reply(200, &product_body("https://api.gog.com")));
    let error = server.client().product(7, None).unwrap_err();
    assert_eq!(
        (error.endpoint(), error.kind()),
        (Endpoint::Product, ErrorKind::Malformed)
    );
}

fn product_body(base: &str) -> String {
    json!({
        "id": 1094900565,
        "title": "RimWorld",
        "slug": "rimworld",
        "game_type": "game",
        "content_system_compatibility": {"windows": true, "osx": true, "linux": false},
        "languages": {"en": "English", "de": "Deutsch"},
        "images": {
            "background": "//images-4.gog-statics.com/bg.jpg",
            "logo": "//images-4.gog-statics.com/logo.jpg",
            "logo2x": "http://images-4.gog-statics.com/logo2x.jpg",
            "icon": "//images-4.gog-statics.com/icon.png",
            "sidebarIcon": "https://images-4.gog-statics.com/side.png",
            "sidebarIcon2x": null,
        },
        "dlcs": {"products": [{"id": 1, "link": "x"}, {"id": "2"}, {"nope": 3}]},
        "downloads": {
            "installers": [
                {
                    "id": "installer_windows_en",
                    "name": "RimWorld",
                    "os": "windows",
                    "language": "en",
                    "language_full": "English",
                    "version": "1.6.4871 rev588",
                    "total_size": 500,
                    "files": [
                        {"id": "en1installer0", "size": 400, "downlink": format!("{base}/products/1094900565/downlink/installer/en1installer0")},
                        {"id": "en1installer1", "size": 100, "downlink": format!("{base}/products/1094900565/downlink/installer/en1installer1")},
                    ],
                },
                {
                    "id": "installer_linux_en",
                    "os": "linux",
                    "language": "en",
                    "version": null,
                    "total_size": "300",
                    "files": [{"id": "en3installer0", "size": 300, "downlink": format!("{base}/x")}],
                },
                {"id": "broken", "os": "linux", "language": "en", "files": []},
                {"id": "no-os", "language": "en", "files": [{"id": "f", "downlink": "y"}]},
            ],
            "patches": [
                {
                    "id": "patch_rimworld",
                    "os": "windows",
                    "language": "en",
                    "version": "1.6.1 rev1",
                    "total_size": 10,
                    "files": [{"id": "en1patch0", "size": 10, "downlink": format!("{base}/p")}],
                }
            ],
            "language_packs": [],
            "bonus_content": [],
        },
        "expanded_dlcs": [
            {
                "id": 1,
                "title": "RimWorld - Royalty",
                "game_type": "dlc",
                "content_system_compatibility": {"windows": true, "osx": false, "linux": true},
                "downloads": {"installers": [{
                    "id": "installer_linux_en",
                    "os": "linux",
                    "language": "en",
                    "version": "1.6",
                    "total_size": 7,
                    "files": [{"id": "en3installer0", "size": 7, "downlink": format!("{base}/d")}],
                }]},
                "expanded_dlcs": [{"id": 99}],
            },
            {"title": "no id, skipped"},
        ],
        "description": {"lead": "ignored"},
    })
    .to_string()
}

#[test]
fn product_details_carry_installers_patches_dlc_and_https_images() {
    let server = FakeGog::start(|request| {
        let host = request.header("host").unwrap_or_default().to_owned();
        reply(200, &product_body(&format!("http://{host}")))
    });
    let product = server.client().product(1094900565, Some(ACCESS)).unwrap();

    let request = &server.requests()[0];
    assert_eq!(request.path(), "/products/1094900565");
    assert_eq!(
        request.query("expand").as_deref(),
        Some("downloads,expanded_dlcs")
    );
    assert_eq!(request.query("locale").as_deref(), Some("en-US"));
    assert_eq!(
        request.header("authorization"),
        Some(format!("Bearer {ACCESS}").as_str())
    );

    assert_eq!(product.id, 1094900565);
    assert_eq!(product.title, "RimWorld");
    assert_eq!(product.game_type.as_deref(), Some("game"));
    assert_eq!(product.languages.len(), 2);
    assert_eq!(product.languages["de"], "Deutsch");
    assert_eq!(
        product.images.background.as_deref(),
        Some("https://images-4.gog-statics.com/bg.jpg")
    );
    assert_eq!(
        product.images.logo2x.as_deref(),
        Some("https://images-4.gog-statics.com/logo2x.jpg")
    );
    assert_eq!(product.images.sidebar_icon2x, None);

    // The flags say Mac and not Linux; the installers say otherwise.
    assert_eq!(
        product.compatibility,
        Compatibility {
            windows: true,
            mac: true,
            linux: false
        }
    );
    assert_eq!(
        product.installer_platforms(),
        Compatibility {
            windows: true,
            mac: false,
            linux: true
        }
    );

    assert_eq!(product.installers.len(), 2);
    let windows = &product.installers[0];
    assert_eq!(windows.version.as_deref(), Some("1.6.4871 rev588"));
    assert_eq!(windows.language_full.as_deref(), Some("English"));
    assert_eq!(windows.total_size, 500);
    assert_eq!(windows.files.len(), 2);
    assert_eq!(windows.files[1].id, "en1installer1");
    assert_eq!(windows.files[1].size, 100);
    assert_eq!(product.installers[1].version, None);
    assert_eq!(product.installers[1].total_size, 300);
    assert_eq!(product.patches.len(), 1);
    assert_eq!(product.patches[0].version.as_deref(), Some("1.6.1 rev1"));

    assert_eq!(product.dlc_ids, [1, 2]);
    assert_eq!(product.dlcs.len(), 1);
    let dlc = &product.dlcs[0];
    assert_eq!((dlc.id, dlc.title.as_str()), (1, "RimWorld - Royalty"));
    assert_eq!(dlc.installers[0].os, "linux");
    assert!(dlc.dlcs.is_empty());
}

#[test]
fn product_details_without_an_id_are_malformed() {
    let server = FakeGog::start(|_| reply(200, r#"{"title":"x"}"#));
    let error = server.client().product(1, None).unwrap_err();
    assert_eq!(
        (error.endpoint(), error.kind()),
        (Endpoint::Product, ErrorKind::Malformed)
    );
    assert_eq!(server.requests()[0].header("authorization"), None);
}

#[test]
fn a_downlink_resolves_to_the_real_link_and_checksum() {
    let server = FakeGog::start(|_| {
        reply(
            200,
            r#"{"downlink":"https://cdn.gog.com/secure/file?token=SIGNED","checksum":"https://cdn.gog.com/file.xml"}"#,
        )
    });
    let downlink = format!(
        "{}/products/1094900565/downlink/installer/en1installer0",
        server.base
    );
    let link = server.client().resolve_link(ACCESS, &downlink).unwrap();
    assert_eq!(
        link.downlink,
        "https://cdn.gog.com/secure/file?token=SIGNED"
    );
    assert_eq!(
        link.checksum.as_deref(),
        Some("https://cdn.gog.com/file.xml")
    );
    assert!(!format!("{link:?}").contains("SIGNED"));

    let request = &server.requests()[0];
    assert_eq!(
        request.path(),
        "/products/1094900565/downlink/installer/en1installer0"
    );
    assert_eq!(
        request.header("authorization"),
        Some(format!("Bearer {ACCESS}").as_str())
    );
}

#[test]
fn a_downlink_without_a_checksum_or_link() {
    let server = FakeGog::start(|request| match request.path() {
        "/a" => reply(200, r#"{"downlink":"https://cdn.gog.com/f","checksum":""}"#),
        _ => reply(200, r#"{"checksum":"x"}"#),
    });
    let client = server.client();
    let link = client
        .resolve_link(ACCESS, &format!("{}/a", server.base))
        .unwrap();
    assert_eq!(link.checksum, None);
    let error = client
        .resolve_link(ACCESS, &format!("{}/b", server.base))
        .unwrap_err();
    assert_eq!(
        (error.endpoint(), error.kind()),
        (Endpoint::Link, ErrorKind::Malformed)
    );
}

#[test]
fn a_downlink_elsewhere_never_gets_the_token() {
    let elsewhere = FakeGog::start(|_| reply(200, r#"{"downlink":"x"}"#));
    let server = FakeGog::start(|_| reply(200, "{}"));
    let client = server.client();
    for downlink in [
        format!("{}/products/1/downlink", elsewhere.base),
        "https://api.gog.com.evil.example/products/1".to_owned(),
        "not a url".to_owned(),
    ] {
        let error = client.resolve_link(ACCESS, &downlink).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::UntrustedLink);
        assert_clean(&error);
    }
    assert!(elsewhere.requests().is_empty());
    assert!(server.requests().is_empty());
}

#[test]
fn the_windows_build_list() {
    let server = FakeGog::start(|_| {
        reply(
            200,
            &json!({
                "total_count": 2,
                "count": 2,
                "items": [
                    {
                        "build_id": "59781086834418085",
                        "product_id": "1771589310",
                        "os": "windows",
                        "branch": null,
                        "version_name": "2.0",
                        "tags": [],
                        "public": true,
                        "date_published": "2024-01-02T03:04:05+0000",
                        "generation": 2,
                        "link": "https://cdn.gog.com/content-system/v2/meta/ab/cd/abcd",
                    },
                    {"build_id": 5, "branch": "beta", "generation": "2"},
                    {"version_name": "no id, skipped"},
                ],
                "has_private_branches": true,
            })
            .to_string(),
        )
    });
    let builds = server
        .client()
        .windows_builds(1771589310, Some(ACCESS))
        .unwrap();
    assert!(builds.has_private_branches);
    assert_eq!(builds.items.len(), 2);
    let first = &builds.items[0];
    assert_eq!(first.build_id, "59781086834418085");
    assert_eq!(first.version_name.as_deref(), Some("2.0"));
    assert_eq!(first.branch, None);
    assert_eq!(
        first.date_published.as_deref(),
        Some("2024-01-02T03:04:05+0000")
    );
    assert_eq!(first.generation, Some(2));
    assert!(first.link.is_some());
    assert_eq!(builds.items[1].build_id, "5");
    assert_eq!(builds.items[1].branch.as_deref(), Some("beta"));

    let request = &server.requests()[0];
    assert_eq!(request.path(), "/products/1771589310/os/windows/builds");
    assert_eq!(request.query("generation").as_deref(), Some("2"));
}

#[test]
fn a_missing_build_list_is_a_404() {
    let server = FakeGog::start(|_| reply(404, "{}"));
    let error = server.client().windows_builds(1, None).unwrap_err();
    assert_eq!(
        (error.endpoint(), error.kind(), error.class()),
        (Endpoint::Builds, ErrorKind::Status(404), Class::Other)
    );
}

#[test]
fn no_error_from_any_endpoint_shows_a_secret() {
    for status in [400, 401, 403, 404, 429, 500, 502] {
        let server = FakeGog::start(move |_| {
            Reply::Send(raw(
                status,
                &format!(r#"{{"error":"invalid_grant","echo":"{REFRESH} {ACCESS} {CODE}"}}"#),
            ))
        });
        let client = server.client();
        let errors = [
            client
                .exchange_code(&parse_paste(CODE).unwrap())
                .unwrap_err(),
            client.refresh(REFRESH).unwrap_err(),
            client.owned_games(ACCESS).unwrap_err(),
            client.product(1, Some(ACCESS)).unwrap_err(),
            client
                .resolve_link(ACCESS, &format!("{}/products/1/downlink", server.base))
                .unwrap_err(),
            client.windows_builds(1, Some(ACCESS)).unwrap_err(),
        ];
        for error in &errors {
            assert_clean(error);
        }
    }
}
