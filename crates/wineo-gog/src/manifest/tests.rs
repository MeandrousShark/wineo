use std::sync::Arc;

use serde_json::json;

use super::*;
use crate::error::Class;
use crate::fake::content::{
    ContentSystem, FakeBuild, FakeDependency, FakeDepot, FakeFile, md5_hex, zlib,
};
use crate::fake::{FakeGog, Reply, json as reply, response};

const MD5_A: &str = "0123456789abcdef0123456789abcdef";
const MD5_B: &str = "fedcba9876543210FEDCBA9876543210";
const SHA: &str = "48ba5e8ac91d9d62143d66635ccad4b24bd832e137ab9e7a4f859e0e46ce5844";

fn chunk_json() -> Value {
    json!({"compressedMd5": MD5_A, "compressedSize": 19, "md5": MD5_B, "size": 11})
}

/// The build id `repository_json` carries.
const LISTED: &str = "55898712513408984";

fn repository_json() -> Value {
    json!({
        "baseProductId": "1455877361",
        "buildId": "55898712513408984",
        "clientId": "54025927574744244",
        "clientSecret": "REPOSECRET",
        "depots": [
            {
                "compressedSize": 744,
                "languages": ["en-US"],
                "manifest": MD5_A,
                "osBitness": ["64"],
                "productId": "1455877361",
                "size": 1283,
            },
            {
                "compressedSize": 5,
                "isGogDepot": true,
                "languages": ["*"],
                "manifest": MD5_B,
                "productId": 1951514936_u64,
                "size": 6,
            },
        ],
        "dependencies": ["MSVC2017", "DirectX"],
        "installDirectory": "HuniePop 2 - Double Date",
        "offlineDepot": {"manifest": "not checked", "languages": ["*"]},
        "platform": "windows",
        "products": [
            {"name": "Base", "productId": "1455877361", "temp_arguments": "", "temp_executable": ""},
            {"name": "DLC", "productId": "1951514936", "temp_arguments": "/S", "temp_executable": "setup.exe"},
        ],
        "scriptInterpreter": true,
        "tags": ["galaxy"],
        "version": 2,
    })
}

/// A build list entry for `link`.
fn listed(link: &str) -> Build {
    Build {
        build_id: LISTED.to_owned(),
        version_name: None,
        branch: None,
        date_published: None,
        generation: Some(2),
        link: Some(link.to_owned()),
    }
}

#[test]
fn a_repository_manifest_gives_depots_dependencies_and_products() {
    let repository = parse_repository(&repository_json(), LISTED).unwrap();
    assert_eq!(repository.build_id, "55898712513408984");
    assert_eq!(repository.base_product_id, 1455877361);
    assert_eq!(
        repository.install_directory.as_deref(),
        Some("HuniePop 2 - Double Date")
    );
    assert!(repository.script_interpreter);
    assert_eq!(repository.dependencies, ["MSVC2017", "DirectX"]);

    let [game, gog] = repository.depots.as_slice() else {
        panic!("{:?}", repository.depots);
    };
    assert_eq!(
        (
            game.product_id,
            game.manifest.as_str(),
            game.size,
            game.compressed_size
        ),
        (1455877361, MD5_A, 1283, 744)
    );
    assert_eq!(game.os_bitness, ["64"]);
    assert!(game.has_language("en-us") && !game.has_language("de-DE"));
    assert!(game.fits_bitness("64") && !game.fits_bitness("32"));
    assert!(!game.is_gog_depot);
    assert_eq!(gog.product_id, 1951514936);
    assert_eq!(gog.manifest, MD5_B.to_ascii_lowercase());
    assert!(gog.has_language("de-DE") && gog.fits_bitness("32") && gog.is_gog_depot);

    assert_eq!(repository.products.len(), 2);
    assert_eq!(repository.products[0].temp_executable, None);
    assert_eq!(
        repository.products[1].temp_executable.as_deref(),
        Some("setup.exe")
    );
    assert_eq!(repository.products[1].temp_arguments.as_deref(), Some("/S"));
    assert!(!format!("{repository:?}").contains("REPOSECRET"));
}

#[test]
fn a_repository_without_a_build_id_takes_the_listed_one() {
    let mut json = repository_json();
    json.as_object_mut().unwrap().remove("buildId");
    let repository = parse_repository(&json, "42").unwrap();
    assert_eq!(repository.build_id, "42");
    assert_eq!(repository.base_product_id, 1455877361);
}

#[test]
fn a_repository_for_another_build_is_refused() {
    let json = repository_json();
    assert_eq!(parse_repository(&json, "42"), None);
    assert!(parse_repository(&json, LISTED).is_some());
    let mut numeric = repository_json();
    numeric["buildId"] = json!(55898712513408984_u64);
    assert!(parse_repository(&numeric, LISTED).is_some());
}

#[test]
fn a_repository_without_a_script_interpreter_or_dependencies() {
    let mut json = repository_json();
    let object = json.as_object_mut().unwrap();
    object.remove("scriptInterpreter");
    object.remove("dependencies");
    let repository = parse_repository(&json, LISTED).unwrap();
    assert!(!repository.script_interpreter);
    assert!(repository.dependencies.is_empty());
}

#[test]
fn a_repository_that_is_not_generation_2_or_is_missing_parts_is_refused() {
    let broken: [fn(&mut Value); 7] = [
        |json| json["version"] = json!(1),
        |json| json["depots"][0]["manifest"] = json!("abc"),
        |json| {
            json["depots"][0]
                .as_object_mut()
                .unwrap()
                .remove("languages");
        },
        |json| json["depots"][1]["productId"] = json!("x"),
        |json| json["dependencies"] = json!([""]),
        |json| json["products"][0]["productId"] = json!(null),
        |json| json["buildId"] = json!(null),
    ];
    for (index, breaking) in broken.iter().enumerate() {
        let mut json = repository_json();
        breaking(&mut json);
        assert_eq!(parse_repository(&json, LISTED), None, "{index}");
    }
}

fn depot_json(items: Value) -> Value {
    json!({
        "depot": {
            "items": items,
            "smallFilesContainer": {"chunks": [chunk_json()]},
        },
        "version": 2,
    })
}

#[test]
fn a_depot_manifest_gives_files_directories_and_links() {
    let manifest = parse_depot_manifest(&depot_json(json!([
        {
            "path": "Data\\cutscenes00.bif",
            "md5": MD5_B,
            "chunks": [chunk_json(), {"compressedMd5": MD5_B, "compressedSize": 5, "md5": MD5_A, "size": 4}],
            "type": "DepotFile",
        },
        {
            "path": "launcher.exe",
            "flags": ["executable", "support"],
            "sha256": SHA,
            "chunks": [chunk_json()],
            "type": "DepotFile",
        },
        {"path": "installed", "chunks": [chunk_json()], "type": "DepotFile", "sfcRef": {"offset": 3, "size": 11}},
        {"path": "Radio\\Stations\\Region\\krgn.id", "chunks": [], "type": "DepotFile"},
        {"path": "PapersPlease_Data/Plugins/x86_64", "type": "DepotDirectory"},
        {"path": "lib/libgame.so", "target": "libgame.so.1", "type": "DepotLink"},
    ])))
    .unwrap();
    let [
        DepotItem::File(big),
        DepotItem::File(launcher),
        DepotItem::File(small),
        DepotItem::File(empty),
        DepotItem::Directory(directory),
        DepotItem::Link(link),
    ] = manifest.items.as_slice()
    else {
        panic!("{:?}", manifest.items);
    };
    assert_eq!(big.parts(), ["Data", "cutscenes00.bif"]);
    assert_eq!(big.size(), 15);
    assert_eq!(big.chunks[1].compressed_md5, MD5_B.to_ascii_lowercase());
    assert_eq!(big.md5, Some(MD5_B.to_ascii_lowercase()));
    assert_eq!(big.sha256, None);
    assert!(!big.is_executable() && !big.is_support());
    assert!(launcher.is_executable() && launcher.is_support());
    assert_eq!(launcher.sha256.as_deref(), Some(SHA));
    assert_eq!(small.chunks.len(), 1);
    assert_eq!(small.size(), 11);
    assert!(empty.chunks.is_empty());
    assert_eq!(empty.size(), 0);
    assert_eq!(
        directory.parts(),
        ["PapersPlease_Data", "Plugins", "x86_64"]
    );
    assert_eq!(link.parts(), ["lib", "libgame.so"]);
    assert_eq!(link.target, "libgame.so.1");
}

#[test]
fn a_file_only_in_the_small_file_container_is_refused() {
    for item in [
        json!({"path": "a.txt", "chunks": [], "type": "DepotFile", "sfcRef": {"offset": 0, "size": 3}}),
        json!({"path": "a.txt", "type": "DepotFile", "sfcRef": {"offset": 0, "size": 3}}),
        json!({"path": "a.txt", "chunks": [], "size": 3, "type": "DepotFile"}),
    ] {
        assert_eq!(parse_depot_manifest(&depot_json(json!([item]))), None);
    }
    // Empty in the container too is just an empty file.
    let empty = json!({"path": "a.txt", "chunks": [], "type": "DepotFile", "sfcRef": {"offset": 0, "size": 0}});
    assert!(parse_depot_manifest(&depot_json(json!([empty]))).is_some());
}

#[test]
fn a_redistributables_leading_separator_means_the_game_folder() {
    let file = |path: &str| json!({"path": path, "chunks": [chunk_json()], "type": "DepotFile"});
    // As GOG lists nGlide and language_setup.
    let manifest = depot_json(json!([file("/glide3x.dll"), file("\\sub\\a.dll")]));
    let parsed = parse_depot_manifest_with(&manifest, Root::Stripped).unwrap();
    let paths: Vec<_> = parsed
        .items
        .iter()
        .map(|item| match item {
            DepotItem::File(file) => file.path.as_str(),
            _ => unreachable!(),
        })
        .collect();
    assert_eq!(paths, ["glide3x.dll", "sub\\a.dll"]);
    // A game's own depots never do, so there it's still refused.
    assert_eq!(parse_depot_manifest(&manifest), None);
    // Only one separator goes, and every other rule still holds.
    for path in ["//etc/evil", "/../evil.dll", "/C:\\evil.dll", "/"] {
        let manifest = depot_json(json!([file(path)]));
        assert_eq!(
            parse_depot_manifest_with(&manifest, Root::Stripped),
            None,
            "{path}"
        );
    }
}

#[test]
fn depot_items_that_are_unsafe_or_odd_fail_the_whole_manifest() {
    let file = |path: &str| json!({"path": path, "chunks": [chunk_json()], "type": "DepotFile"});
    for item in [
        file(""),
        file("..\\evil.dll"),
        file("game/../../evil.dll"),
        file("\\Windows\\evil.dll"),
        file("/etc/evil"),
        file("C:\\evil.dll"),
        file("a\\\\b"),
        file("a/./b"),
        file("a\u{0}b"),
        file("file.txt:stream"),
        json!({"path": "a", "chunks": [{"md5": MD5_A}], "type": "DepotFile"}),
        json!({"path": "a", "chunks": [chunk_json()], "md5": "short", "type": "DepotFile"}),
        json!({"path": "a", "chunks": [chunk_json()], "sha256": MD5_A, "type": "DepotFile"}),
        json!({"path": "a", "chunks": [chunk_json()], "flags": [1], "type": "DepotFile"}),
        json!({"path": "a", "type": "DepotLink"}),
        json!({"path": "a", "type": "DepotSomethingNew"}),
        json!({"path": "a"}),
    ] {
        let manifest = depot_json(json!([file("ok.txt"), item.clone()]));
        assert_eq!(parse_depot_manifest(&manifest), None, "{item}");
    }
    assert_eq!(parse_depot_manifest(&json!({"depot": {}})), None);
}

fn dependency_manifest_json() -> Value {
    json!({
        "depots": [
            {
                "compressedSize": 1167849,
                "dependencyId": "ISI",
                "executable": {"arguments": "", "path": "__redist/ISI/scriptinterpreter.exe"},
                "internal": true,
                "languages": ["*"],
                "manifest": MD5_A,
                "readableName": "Install Script Interpreter",
                "signature": "ignored",
                "size": 1253424,
            },
            {
                "compressedSize": 2684678,
                "dependencyId": "MSVC2005",
                "executable": {
                    "arguments": "/q:a /c:\"VCREDI~3.EXE /q:a /c:\"\"msiexec /i vcredist.msi /qb\"\" \"",
                    "path": "__redist/MSVC2005/vcredist_x86.exe",
                },
                "internal": false,
                "languages": ["*"],
                "manifest": MD5_B,
                "readableName": "Visual C++ Redist 2005",
                "size": 2723264,
            },
            {
                "compressedSize": 5,
                "dependencyId": "DOSBox063",
                "executable": {"arguments": "", "path": ""},
                "internal": true,
                "languages": ["*"],
                "manifest": MD5_A,
                "readableName": "DOSBox",
                "size": 6,
            },
        ],
    })
}

#[test]
fn the_dependency_manifest_gives_each_redistributable() {
    let manifest = parse_dependency_manifest(&dependency_manifest_json()).unwrap();
    let [isi, msvc, dosbox] = manifest.dependencies.as_slice() else {
        panic!("{:?}", manifest.dependencies);
    };
    assert_eq!(isi.id, "ISI");
    assert!(isi.internal);
    assert_eq!(
        isi.executable,
        Some(DependencyExecutable {
            path: "__redist/ISI/scriptinterpreter.exe".to_owned(),
            arguments: String::new(),
        })
    );
    assert_eq!(
        isi.readable_name.as_deref(),
        Some("Install Script Interpreter")
    );
    assert_eq!((isi.size, isi.compressed_size), (1253424, 1167849));
    assert_eq!(isi.languages, ["*"]);
    assert!(!msvc.internal);
    assert_eq!(
        msvc.executable.as_ref().unwrap().arguments,
        "/q:a /c:\"VCREDI~3.EXE /q:a /c:\"\"msiexec /i vcredist.msi /qb\"\" \""
    );
    assert_eq!(msvc.manifest, MD5_B.to_ascii_lowercase());
    assert_eq!(dosbox.executable, None);

    let mut unsafe_path = dependency_manifest_json();
    unsafe_path["depots"][0]["executable"]["path"] = json!("../evil.exe");
    assert_eq!(parse_dependency_manifest(&unsafe_path), None);
    let mut no_id = dependency_manifest_json();
    no_id["depots"][1]["dependencyId"] = json!("");
    assert_eq!(parse_dependency_manifest(&no_id), None);
}

#[test]
fn the_dependency_repository_points_at_its_manifest() {
    let repository = parse_dependency_repository(&json!({
        "repository_manifest": "https://cdn.gog.com/content-system/v2/dependencies/meta/e4/be/e4be",
        "build_id": "59705672826648994",
        "generation": 2,
    }))
    .unwrap();
    assert_eq!(repository.build_id, "59705672826648994");
    assert!(repository.manifest_url.ends_with("/e4/be/e4be"));
    assert_eq!(parse_dependency_repository(&json!({"build_id": "1"})), None);
}

#[test]
fn manifests_come_zlib_compressed_or_as_plain_json() {
    let text = repository_json().to_string();
    assert_eq!(
        decode_manifest(&zlib(text.as_bytes()), MAX_MANIFEST).unwrap(),
        repository_json()
    );
    assert_eq!(
        decode_manifest(text.as_bytes(), MAX_MANIFEST).unwrap(),
        repository_json()
    );
    assert_eq!(
        decode_manifest(format!("\n  {text}").as_bytes(), MAX_MANIFEST).unwrap(),
        repository_json()
    );
    assert_eq!(
        decode_manifest(b"not json", MAX_MANIFEST),
        Err(ErrorKind::Json)
    );
    assert_eq!(decode_manifest(b"", MAX_MANIFEST), Err(ErrorKind::Json));
    // A zlib header and then garbage.
    assert_eq!(
        decode_manifest(&[0x78, 0x9c, 1, 2, 3, 4], MAX_MANIFEST),
        Err(ErrorKind::Malformed)
    );
    assert_eq!(
        decode_manifest(&zlib(b"{"), MAX_MANIFEST),
        Err(ErrorKind::Json)
    );
}

#[test]
fn manifests_are_capped_at_64_mib_unpacked() {
    assert_eq!(MAX_MANIFEST, 64 * 1024 * 1024);
    let text = format!("{{\"pad\":\"{}\"}}", "x".repeat(1000));
    let limit = text.len() as u64;
    assert!(decode_manifest(text.as_bytes(), limit).is_ok());
    assert!(decode_manifest(&zlib(text.as_bytes()), limit).is_ok());
    // Small packed, too large unpacked.
    assert_eq!(
        decode_manifest(&zlib(text.as_bytes()), limit - 1),
        Err(ErrorKind::TooLarge)
    );
    assert_eq!(
        decode_manifest(text.as_bytes(), limit - 1),
        Err(ErrorKind::TooLarge)
    );
}

pub(crate) fn serve(content: ContentSystem) -> (Arc<ContentSystem>, FakeGog) {
    let content = Arc::new(content);
    let served = Arc::clone(&content);
    let server =
        FakeGog::start(move |request| served.answer(request).unwrap_or_else(|| reply(404, "{}")));
    (content, server)
}

fn game_depot() -> FakeDepot {
    FakeDepot::new(7)
        .languages(&["en-US"])
        .bitness(&["64"])
        .file(
            FakeFile::new("bin\\game.exe", b"MZ game")
                .flag("executable")
                .with_sha256(),
        )
        .file(FakeFile::new("readme.txt", b"hello").in_container())
        .file(FakeFile::new("setup.exe", b"setup").flag("support"))
        .directory("saves")
        .link("game", "bin/game.exe")
}

#[test]
fn a_private_branch_is_listed_only_with_its_password() {
    let content = ContentSystem::new();
    content.add_build(FakeBuild::new(7, "100").depot(game_depot()));
    content.add_build(FakeBuild::new(7, "200").depot(game_depot()).branch("beta"));
    content.set_branch_password("beta", "open sesame");
    let (_content, server) = serve(content);
    let client = server.client();
    let branches = |password: Option<&str>| {
        let secret = password.map(crate::Secret::new);
        let builds = client
            .windows_builds(7, Some("ACCESS"), secret.as_ref())
            .unwrap();
        (
            builds.has_private_branches,
            builds
                .items
                .iter()
                .map(|build| build.branch.clone())
                .collect::<Vec<_>>(),
        )
    };
    let public = (true, vec![None]);
    // A wrong password isn't an error: GOG answers it like none at all.
    assert_eq!(branches(None), public);
    assert_eq!(branches(Some("wrong")), public);
    assert_eq!(
        branches(Some("open sesame")),
        (true, vec![None, Some("beta".to_owned())])
    );
}

#[test]
fn a_served_manifest_without_a_build_id_reads_as_the_listed_build() {
    let content = ContentSystem::new();
    content.add_build(
        FakeBuild::new(7, "100")
            .depot(game_depot())
            .without_build_id(),
    );
    let (_content, server) = serve(content);
    let client = server.client();
    let builds = client.windows_builds(7, Some("ACCESS"), None).unwrap();
    let repository = client.repository(&builds.items[0]).unwrap();
    assert_eq!(repository.build_id, "100");
}

#[test]
fn a_build_s_manifests_are_fetched_from_the_cdn_without_a_token() {
    for plain in [false, true] {
        let content = ContentSystem::new();
        content.serve_plain_json(plain);
        content.add_build(
            FakeBuild::new(7, "100")
                .depot(game_depot())
                .depot(FakeDepot::new(8).gog_depot())
                .dependency("MSVC2017")
                .product(7, "")
                .product(8, "dlc_setup.exe"),
        );
        let (_content, server) = serve(content);
        let client = server.client();
        let builds = client.windows_builds(7, Some("ACCESS"), None).unwrap();
        let repository = client.repository(&builds.items[0]).unwrap();
        assert_eq!(repository.build_id, "100");
        assert_eq!(repository.dependencies, ["MSVC2017"]);
        assert_eq!(
            repository.products[1].temp_executable.as_deref(),
            Some("dlc_setup.exe")
        );
        assert_eq!(repository.depots.len(), 2);
        assert_eq!(repository.depots[0].os_bitness, ["64"]);
        assert!(repository.depots[1].is_gog_depot);

        let manifest = client
            .depot_manifest(&repository.depots[0].manifest)
            .unwrap();
        assert_eq!(manifest.items.len(), 5);
        let DepotItem::File(exe) = &manifest.items[0] else {
            panic!("{:?}", manifest.items[0]);
        };
        assert_eq!(exe.parts(), ["bin", "game.exe"]);
        assert_eq!(exe.sha256.as_deref().map(str::len), Some(64));
        assert!(matches!(&manifest.items[2], DepotItem::File(file) if file.is_support()));
        assert!(matches!(&manifest.items[3], DepotItem::Directory(_)));
        assert!(
            matches!(&manifest.items[4], DepotItem::Link(link) if link.target == "bin/game.exe")
        );

        let requests = server.requests();
        let hash = &repository.depots[0].manifest;
        assert_eq!(
            requests.last().unwrap().path(),
            format!(
                "/content-system/v2/meta/{}/{}/{hash}",
                &hash[0..2],
                &hash[2..4]
            )
        );
        for request in &requests[1..] {
            assert_eq!(request.header("authorization"), None, "{}", request.target);
        }
    }
}

#[test]
fn a_game_local_redistributables_files_are_fetched_relative() {
    let content = ContentSystem::new();
    // GOG lists nGlide with no program and a leading slash.
    content.set_dependencies(
        "555",
        &[FakeDependency::new(
            "nGlide",
            "",
            "",
            FakeDepot::new(0).file(FakeFile::new("/glide3x.dll", b"glide")),
        )],
    );
    let (_content, server) = serve(content);
    let client = server.client();
    let repository = client.dependency_repository(None).unwrap();
    let manifest = client.dependency_manifest(&repository).unwrap();
    let [glide] = manifest.dependencies.as_slice() else {
        panic!("{:?}", manifest.dependencies);
    };
    assert_eq!(glide.executable, None);
    let files = client.dependency_depot_manifest(&glide.manifest).unwrap();
    assert!(matches!(
        files.items.as_slice(),
        [DepotItem::File(file)] if file.path == "glide3x.dll"
    ));
}

#[test]
fn the_redistributables_and_their_files() {
    let content = ContentSystem::new();
    content.set_dependencies(
        "555",
        &[FakeDependency::new(
            "MSVC2017",
            "__redist/MSVC2017/VC_redist.x86.exe",
            "/install /quiet /norestart",
            FakeDepot::new(0).file(FakeFile::new(
                "__redist\\MSVC2017\\VC_redist.x86.exe",
                b"MZ redist",
            )),
        )],
    );
    let (_content, server) = serve(content);
    let client = server.client();
    let repository = client.dependency_repository(None).unwrap();
    assert_eq!(repository.build_id, "555");
    let manifest = client.dependency_manifest(&repository).unwrap();
    let [msvc] = manifest.dependencies.as_slice() else {
        panic!("{:?}", manifest.dependencies);
    };
    assert_eq!(msvc.id, "MSVC2017");
    assert_eq!(
        msvc.executable.as_ref().unwrap().arguments,
        "/install /quiet /norestart"
    );
    let files = client.dependency_depot_manifest(&msvc.manifest).unwrap();
    assert!(matches!(
        files.items.as_slice(),
        [DepotItem::File(file)] if file.path == "__redist\\MSVC2017\\VC_redist.x86.exe"
    ));

    let requests = server.requests();
    assert_eq!(requests[0].path(), "/dependencies/repository");
    assert_eq!(requests[0].query("generation").as_deref(), Some("2"));
    assert!(
        requests[2]
            .path()
            .starts_with("/content-system/v2/dependencies/meta/")
    );
}

#[test]
fn manifest_failures_name_only_the_endpoint() {
    let (_content, server) = serve(ContentSystem::new());
    let client = server.client();
    let missing_depot = client.depot_manifest(MD5_A).unwrap_err();
    assert_eq!(
        (
            missing_depot.endpoint(),
            missing_depot.kind(),
            missing_depot.class()
        ),
        (
            Endpoint::DepotManifest,
            ErrorKind::Status(404),
            Class::Other
        )
    );
    let link = format!(
        "{}/content-system/v2/meta/ab/cd/{MD5_A}?secret=1",
        server.base
    );
    let missing_repository = client.repository(&listed(&link)).unwrap_err();
    assert_eq!(missing_repository.endpoint(), Endpoint::Repository);
    let missing_dependencies = client.dependency_repository(Some("ACCESS")).unwrap_err();
    assert_eq!(missing_dependencies.endpoint(), Endpoint::Dependencies);
    for error in [missing_depot, missing_repository, missing_dependencies] {
        let shown = format!("{error} {error:?}");
        for secret in ["127.0.0.1", "?", "secret", "ACCESS", MD5_A] {
            assert!(!shown.contains(secret), "{secret} in {shown}");
        }
    }

    let requests = server.requests().len();
    for bad in ["", "abc", "../../../../etc/passwd", &"g".repeat(32)] {
        let error = client.depot_manifest(bad).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::Malformed);
    }
    let error = client.repository(&listed("not a url")).unwrap_err();
    assert_eq!(
        (error.endpoint(), error.kind()),
        (Endpoint::Repository, ErrorKind::Malformed)
    );
    let linkless = Build {
        link: None,
        ..listed("")
    };
    let error = client.repository(&linkless).unwrap_err();
    assert_eq!(
        (error.endpoint(), error.kind()),
        (Endpoint::Repository, ErrorKind::Malformed)
    );
    assert_eq!(server.requests().len(), requests);
}

#[test]
fn a_manifest_answer_that_is_not_a_manifest_is_malformed() {
    let server = FakeGog::start(|_| Reply::Send(response(200, &[], &zlib(b"{\"depot\":{}}"))));
    let error = server.client().depot_manifest(MD5_A).unwrap_err();
    assert_eq!(
        (error.endpoint(), error.kind(), error.class()),
        (
            Endpoint::DepotManifest,
            ErrorKind::Malformed,
            Class::Malformed
        )
    );
}

#[test]
fn absurd_sizes_fail_the_manifest() {
    let mut depot = repository_json();
    depot["depots"][0]["size"] = json!(u64::MAX);
    assert_eq!(parse_repository(&depot, LISTED), None);
    let mut depot = repository_json();
    depot["depots"][0]["compressedSize"] = json!(MAX_SIZE + 1);
    assert_eq!(parse_repository(&depot, LISTED), None);
    for (name, size) in [
        ("compressedSize", u64::MAX),
        ("compressedSize", MAX_CHUNK + 1),
        ("size", u64::MAX),
        ("size", MAX_CHUNK + 1),
    ] {
        let mut chunk = chunk_json();
        chunk[name] = json!(size);
        let item = json!({"path": "a", "chunks": [chunk], "type": "DepotFile"});
        assert_eq!(
            parse_depot_manifest(&depot_json(json!([item]))),
            None,
            "{name}"
        );
    }
    let mut chunk = chunk_json();
    chunk["size"] = json!(MAX_CHUNK);
    let item = json!({"path": "a", "chunks": [chunk], "type": "DepotFile"});
    assert!(parse_depot_manifest(&depot_json(json!([item]))).is_some());
    let mut dependency = dependency_manifest_json();
    dependency["depots"][0]["size"] = json!(u64::MAX);
    assert_eq!(parse_dependency_manifest(&dependency), None);
}

#[test]
fn a_product_s_setup_program_must_be_a_safe_path() {
    for bad in [
        json!("..\\setup.exe"),
        json!("C:\\setup.exe"),
        json!("/setup.exe"),
        json!("a\u{0}.exe"),
        json!(7),
    ] {
        let mut json = repository_json();
        json["products"][1]["temp_executable"] = bad.clone();
        assert_eq!(parse_repository(&json, LISTED), None, "{bad}");
    }
    let mut json = repository_json();
    json["products"][1]["temp_arguments"] = json!(["/S"]);
    assert_eq!(parse_repository(&json, LISTED), None);
    let mut json = repository_json();
    json["products"][1]["temp_executable"] = json!("support\\setup.exe");
    let repository = parse_repository(&json, LISTED).unwrap();
    assert_eq!(
        repository.products[1].temp_executable.as_deref(),
        Some("support\\setup.exe")
    );
}

#[test]
fn a_malformed_dependency_executable_fails_the_manifest() {
    for executable in [
        json!({"path": 123, "arguments": ""}),
        json!({"path": "__redist/a.exe", "arguments": 5}),
        json!({"path": "", "arguments": ["/q"]}),
    ] {
        let mut json = dependency_manifest_json();
        json["depots"][0]["executable"] = executable.clone();
        assert_eq!(parse_dependency_manifest(&json), None, "{executable}");
    }
    for executable in [json!({}), json!({"arguments": "/q"}), json!(null)] {
        let mut json = dependency_manifest_json();
        json["depots"][0]["executable"] = executable.clone();
        let manifest = parse_dependency_manifest(&json).unwrap();
        assert_eq!(manifest.dependencies[0].executable, None, "{executable}");
    }
    let mut json = dependency_manifest_json();
    json["depots"][0]["executable"] = json!({"path": "__redist/a.exe"});
    let manifest = parse_dependency_manifest(&json).unwrap();
    assert_eq!(
        manifest.dependencies[0]
            .executable
            .as_ref()
            .unwrap()
            .arguments,
        ""
    );
}

#[test]
fn odd_but_legal_file_names_are_accepted() {
    let names = [
        "My Game\\Game Launcher.exe",
        "データ\\файл ünïcødé.txt",
        "a.b.c.d",
        ".hashdb",
        "goggame-1207664593.hashdb",
        "Data\\.hidden\\..config",
        "trailing..",
        "with ' quote & (brackets) [1].txt",
    ];
    let items: Vec<Value> = names
        .iter()
        .map(|path| json!({"path": path, "chunks": [chunk_json()], "type": "DepotFile"}))
        .collect();
    let manifest = parse_depot_manifest(&depot_json(json!(items))).unwrap();
    let paths: Vec<&str> = manifest
        .items
        .iter()
        .map(|item| match item {
            DepotItem::File(file) => file.path.as_str(),
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(paths, names);
}

#[test]
fn the_dependency_repository_hides_its_url_in_debug() {
    let repository = DependencyRepository {
        build_id: "1".to_owned(),
        manifest_url: "https://cdn.example/secret/path".to_owned(),
    };
    let shown = format!("{repository:?} {repository:#?}");
    assert!(!shown.contains("cdn.example") && !shown.contains("secret"));
    assert!(shown.contains("build_id"));
}

#[test]
fn a_cdn_refusal_is_a_status_never_a_login_problem() {
    for status in [401, 403] {
        let server = FakeGog::start(move |_| reply(status, "{}"));
        let client = server.client();
        let repository = DependencyRepository {
            build_id: "1".to_owned(),
            manifest_url: format!(
                "{}/content-system/v2/dependencies/meta/ab/cd/x",
                server.base
            ),
        };
        for error in [
            client.depot_manifest(MD5_A).unwrap_err(),
            client.dependency_depot_manifest(MD5_A).unwrap_err(),
            client
                .repository(&listed(&format!(
                    "{}/content-system/v2/meta/ab/cd/x",
                    server.base
                )))
                .unwrap_err(),
            client.dependency_manifest(&repository).unwrap_err(),
        ] {
            assert_eq!(error.kind(), ErrorKind::Status(status));
            assert_eq!(error.class(), Class::Other);
        }
    }
}

#[test]
fn the_manifest_cap_holds_over_http_packed_and_unpacked() {
    let manifest = |pad: &str| format!("{{\"depot\":{{\"items\":[]}},\"pad\":\"{pad}\"}}");
    let plain = manifest(&"x".repeat(8000)).into_bytes();
    let packed = zlib(&plain);
    // Hex of chained MD5s barely compresses, so packed it's still large.
    let mut noise = String::new();
    let mut digest = md5_hex(b"seed");
    while noise.len() < 8000 {
        digest = md5_hex(digest.as_bytes());
        noise.push_str(&digest);
    }
    let noisy = zlib(manifest(&noise).as_bytes());
    let cap = 3000;
    assert!(noisy.len() as u64 > cap && (packed.len() as u64) < cap);
    for body in [plain, packed, noisy] {
        let served = body.clone();
        let server = FakeGog::start(move |_| Reply::Send(response(200, &[], &served)));
        let mut client = server.client();
        client.max_manifest = cap;
        let error = client.depot_manifest(MD5_A).unwrap_err();
        assert_eq!(
            (error.endpoint(), error.kind()),
            (Endpoint::DepotManifest, ErrorKind::TooLarge)
        );
        client.max_manifest = 20_000;
        assert!(client.depot_manifest(MD5_A).unwrap().items.is_empty());
    }
}

#[test]
fn a_dependency_executable_that_is_not_an_object_fails_the_manifest() {
    for executable in [json!(123), json!(true), json!([]), json!("setup.exe")] {
        let mut json = dependency_manifest_json();
        json["depots"][1]["executable"] = executable.clone();
        assert_eq!(parse_dependency_manifest(&json), None, "{executable}");
    }
    let mut json = dependency_manifest_json();
    json["depots"][1]["executable"] = json!(null);
    json["depots"][0]
        .as_object_mut()
        .unwrap()
        .remove("executable");
    let manifest = parse_dependency_manifest(&json).unwrap();
    assert_eq!(manifest.dependencies[0].executable, None);
    assert_eq!(manifest.dependencies[1].executable, None);
}

#[test]
fn a_chunkless_file_with_a_malformed_size_fails_the_manifest() {
    let sizes = [
        json!(-1),
        json!(1.5),
        json!("18446744073709551616"),
        json!("3"),
        json!(true),
    ];
    for size in sizes {
        for item in [
            json!({"path": "a", "chunks": [], "size": size, "type": "DepotFile"}),
            json!({"path": "a", "chunks": [], "sfcRef": {"offset": 0, "size": size}, "type": "DepotFile"}),
        ] {
            assert_eq!(
                parse_depot_manifest(&depot_json(json!([item]))),
                None,
                "{item}"
            );
        }
    }
    for sfc in [json!(5), json!("x"), json!([])] {
        let item = json!({"path": "a", "chunks": [], "sfcRef": sfc, "type": "DepotFile"});
        assert_eq!(
            parse_depot_manifest(&depot_json(json!([item]))),
            None,
            "{item}"
        );
    }
    for item in [
        json!({"path": "a", "chunks": [], "size": null, "sfcRef": null, "type": "DepotFile"}),
        json!({"path": "a", "chunks": [], "size": 0, "sfcRef": {"size": null}, "type": "DepotFile"}),
    ] {
        let manifest = parse_depot_manifest(&depot_json(json!([item.clone()]))).unwrap();
        assert!(
            matches!(&manifest.items[0], DepotItem::File(file) if file.size() == 0),
            "{item}"
        );
    }
}

/// Sets the field at `path` in a copy of `base`.
fn with(base: &Value, path: &[&str], value: Value) -> Value {
    let mut json = base.clone();
    let mut field = &mut json;
    for key in &path[..path.len() - 1] {
        field = match key.parse::<usize>() {
            Ok(index) => &mut field[index],
            Err(_) => &mut field[*key],
        };
    }
    field[path[path.len() - 1]] = value;
    json
}

#[test]
fn every_optional_field_fails_when_malformed_and_defaults_only_when_null() {
    let repository = repository_json();
    let depot = depot_json(json!([
        {"path": "a", "chunks": [chunk_json()], "type": "DepotFile", "sfcRef": {"offset": 0, "size": 11}},
    ]));
    // Without an `sfcRef`, so a null `chunks` is an empty file.
    let plain_depot = depot_json(json!([
        {"path": "a", "chunks": [chunk_json()], "type": "DepotFile"},
    ]));
    let dependencies = dependency_manifest_json();
    type Parse = fn(&Value) -> bool;
    let parse_repo: Parse = |json| parse_repository(json, LISTED).is_some();
    let parse_depot: Parse = |json| parse_depot_manifest(json).is_some();
    let parse_dependencies: Parse = |json| parse_dependency_manifest(json).is_some();
    let cases: Vec<(&Value, Parse, &[&str], Value)> = vec![
        (&repository, parse_repo, &["installDirectory"], json!(5)),
        (
            &repository,
            parse_repo,
            &["scriptInterpreter"],
            json!("yes"),
        ),
        (
            &repository,
            parse_repo,
            &["dependencies"],
            json!("MSVC2017"),
        ),
        (&repository, parse_repo, &["products"], json!({})),
        (
            &repository,
            parse_repo,
            &["products", "0", "name"],
            json!(5),
        ),
        (
            &repository,
            parse_repo,
            &["products", "0", "temp_executable"],
            json!([]),
        ),
        (
            &repository,
            parse_repo,
            &["products", "0", "temp_arguments"],
            json!(1),
        ),
        (
            &repository,
            parse_repo,
            &["depots", "0", "osBitness"],
            json!("64"),
        ),
        (
            &repository,
            parse_repo,
            &["depots", "0", "isGogDepot"],
            json!(1),
        ),
        (
            &plain_depot,
            parse_depot,
            &["depot", "items", "0", "chunks"],
            json!({}),
        ),
        (
            &depot,
            parse_depot,
            &["depot", "items", "0", "size"],
            json!(-1),
        ),
        (
            &depot,
            parse_depot,
            &["depot", "items", "0", "sfcRef"],
            json!(5),
        ),
        (
            &depot,
            parse_depot,
            &["depot", "items", "0", "sfcRef", "size"],
            json!(1.5),
        ),
        (
            &depot,
            parse_depot,
            &["depot", "items", "0", "md5"],
            json!(5),
        ),
        (
            &depot,
            parse_depot,
            &["depot", "items", "0", "sha256"],
            json!(5),
        ),
        (
            &depot,
            parse_depot,
            &["depot", "items", "0", "flags"],
            json!("executable"),
        ),
        (
            &dependencies,
            parse_dependencies,
            &["depots", "0", "readableName"],
            json!(5),
        ),
        (
            &dependencies,
            parse_dependencies,
            &["depots", "0", "internal"],
            json!(1),
        ),
        (
            &dependencies,
            parse_dependencies,
            &["depots", "0", "languages"],
            json!("*"),
        ),
        (
            &dependencies,
            parse_dependencies,
            &["depots", "0", "executable"],
            json!(123),
        ),
        (
            &dependencies,
            parse_dependencies,
            &["depots", "0", "executable", "path"],
            json!(1),
        ),
        (
            &dependencies,
            parse_dependencies,
            &["depots", "0", "executable", "arguments"],
            json!(1),
        ),
    ];
    for (base, parse, path, wrong) in cases {
        assert!(
            !parse(&with(base, path, wrong.clone())),
            "{path:?} = {wrong}"
        );
        assert!(parse(&with(base, path, json!(null))), "{path:?} = null");
    }
}
