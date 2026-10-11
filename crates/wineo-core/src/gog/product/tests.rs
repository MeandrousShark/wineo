use super::*;
use crate::gog::test_support::{client, setup};
use crate::steam::api::fixture::{Server, reply};

const ID: u64 = 1_094_900_565;
const T: i64 = 1_800_000_000;

fn details(id: u64, title: &str) -> Vec<u8> {
    let body = serde_json::json!({
        "id": id,
        "title": title,
        "content_system_compatibility": {"windows": true, "osx": false, "linux": true},
        "downloads": {"installers": [{
            "id": "installer_linux_en",
            "os": "linux",
            "language": "en",
            "version": "1.6",
            "total_size": 7,
            "files": [{"id": "en3installer0", "size": 7, "downlink": "https://api.gog.com/x"}],
        }]},
    });
    reply(200, &body.to_string())
}

fn run(
    paths: &Paths,
    replies: Vec<Vec<u8>>,
    id: u64,
    refresh: bool,
    now: i64,
) -> (Server, Result<Details, LoadError>) {
    let server = Server::start(replies);
    let result = load_at(paths, &client(&server), id, refresh, now);
    (server, result)
}

#[test]
fn details_are_fetched_without_a_login_and_cached_per_product() {
    let (_root, paths) = setup();
    let (server, loaded) = run(&paths, vec![details(ID, "RimWorld")], ID, false, T);
    let loaded = loaded.unwrap();
    assert_eq!(loaded.product.title, "RimWorld");
    assert_eq!(loaded.product.installers[0].version.as_deref(), Some("1.6"));
    assert_eq!(loaded.warning, None);
    let request = &server.requests()[0];
    assert!(
        request.starts_with(&format!(
            "GET /products/{ID}?expand=downloads%2Cexpanded_dlcs"
        )),
        "{request}"
    );

    let path = paths.gog_product_cache(ID);
    assert!(path.ends_with(format!("wineo/gog-product-{ID}.json")));
    let cache = fs::read_to_string(&path).unwrap();
    assert!(cache.contains("\"fetched_at\":1800000000"));
    assert!(!paths.gog_product_cache(ID + 1).exists());
}

#[test]
fn a_fresh_cache_is_used_and_a_stale_or_forced_one_fetches() {
    let (_root, paths) = setup();
    run(&paths, vec![details(ID, "Old")], ID, false, T)
        .1
        .unwrap();

    let (quiet, loaded) = run(&paths, vec![], ID, false, T + 3599);
    assert_eq!(loaded.unwrap().product.title, "Old");
    assert!(quiet.requests().is_empty());

    let (stale, loaded) = run(&paths, vec![details(ID, "Stale")], ID, false, T + 3600);
    assert_eq!(loaded.unwrap().product.title, "Stale");
    assert_eq!(stale.requests().len(), 1);

    let (forced, loaded) = run(&paths, vec![details(ID, "Forced")], ID, true, T + 3601);
    assert_eq!(loaded.unwrap().product.title, "Forced");
    assert_eq!(forced.requests().len(), 1);
}

#[test]
fn a_failed_fetch_falls_back_with_a_warning_or_fails_without_a_cache() {
    let (_root, paths) = setup();
    let (_, loaded) = run(&paths, vec![reply(500, "{}")], ID, false, T);
    assert_eq!(
        loaded.unwrap_err().to_string(),
        "Couldn't get GOG product details: GOG product request failed: GOG answered with status 500"
    );
    assert!(!paths.gog_product_cache(ID).exists());

    run(&paths, vec![details(ID, "Kept")], ID, false, T)
        .1
        .unwrap();
    let before = fs::read(paths.gog_product_cache(ID)).unwrap();
    let (_, loaded) = run(&paths, vec![reply(200, "{")], ID, true, T + 10);
    let loaded = loaded.unwrap();
    assert_eq!(loaded.product.title, "Kept");
    assert_eq!(
        loaded.warning.as_deref(),
        Some(
            "Couldn't refresh GOG product details: GOG product request failed: GOG's answer isn't valid JSON"
        )
    );
    assert_eq!(fs::read(paths.gog_product_cache(ID)).unwrap(), before);
}

#[test]
fn details_for_another_product_are_never_cached_or_used() {
    let (_root, paths) = setup();
    let (_, loaded) = run(&paths, vec![details(ID + 1, "Wrong")], ID, false, T);
    loaded.unwrap_err();
    assert!(!paths.gog_product_cache(ID).exists());

    // A cache file holding another product counts as none.
    run(&paths, vec![details(ID + 1, "Other")], ID + 1, false, T)
        .1
        .unwrap();
    fs::copy(paths.gog_product_cache(ID + 1), paths.gog_product_cache(ID)).unwrap();
    let (server, loaded) = run(&paths, vec![details(ID, "Right")], ID, false, T + 1);
    assert_eq!(loaded.unwrap().product.title, "Right");
    assert_eq!(server.requests().len(), 1);
}

#[test]
fn a_cache_that_cannot_be_written_is_a_warning() {
    let (root, paths) = setup();
    fs::create_dir_all(root.path().join(".cache")).unwrap();
    fs::write(root.path().join(".cache/wineo"), "in the way").unwrap();
    let (_, loaded) = run(&paths, vec![details(ID, "RimWorld")], ID, false, T);
    let loaded = loaded.unwrap();
    assert_eq!(loaded.product.title, "RimWorld");
    assert!(
        loaded
            .warning
            .unwrap()
            .starts_with("Couldn't save GOG product details: ")
    );
}
