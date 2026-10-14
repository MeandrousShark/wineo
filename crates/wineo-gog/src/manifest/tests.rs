use std::sync::Arc;

use serde_json::json;

use super::*;
use crate::error::Class;
use crate::fake::content::{ContentSystem, FakeBuild, FakeDependency, FakeDepot, FakeFile, zlib};
use crate::fake::{FakeGog, Reply, json as reply, response};

const MD5_A: &str = "0123456789abcdef0123456789abcdef";
const MD5_B: &str = "fedcba9876543210FEDCBA9876543210";
const SHA: &str = "48ba5e8ac91d9d62143d66635ccad4b24bd832e137ab9e7a4f859e0e46ce5844";

fn chunk_json() -> Value {
    json!({"compressedMd5": MD5_A, "compressedSize": 19, "md5": MD5_B, "size": 11})
}

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

#[test]
fn a_repository_manifest_gives_depots_dependencies_and_products() {
    let repository = parse_repository(&repository_json()).unwrap();
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
fn a_repository_without_a_script_interpreter_or_dependencies() {
    let mut json = repository_json();
    let object = json.as_object_mut().unwrap();
    object.remove("scriptInterpreter");
    object.remove("dependencies");
    let repository = parse_repository(&json).unwrap();
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
        |json| {
            json.as_object_mut().unwrap().remove("buildId");
        },
    ];
    for (index, breaking) in broken.iter().enumerate() {
        let mut json = repository_json();
        breaking(&mut json);
        assert_eq!(parse_repository(&json), None, "{index}");
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
        let builds = client.windows_builds(7, Some("ACCESS")).unwrap();
        let repository = client
            .repository(builds.items[0].link.as_deref().unwrap())
            .unwrap();
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
    let missing_repository = client.repository(&link).unwrap_err();
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
    let error = client.repository("not a url").unwrap_err();
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
