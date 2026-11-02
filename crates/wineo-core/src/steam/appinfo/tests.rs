use std::fs;

use super::*;
use crate::steam::InstallKind;

/// Key names, written once each into the string table.
#[derive(Default)]
struct Names(Vec<Vec<u8>>);

impl Names {
    fn index(&mut self, name: impl AsRef<[u8]>) -> i32 {
        let name = name.as_ref();
        let at = self.0.iter().position(|known| known == name);
        let at = at.unwrap_or_else(|| {
            self.0.push(name.to_vec());
            self.0.len() - 1
        });
        i32::try_from(at).unwrap()
    }
}

/// One key/value, encoded as type byte, key index, then payload.
enum Kv {
    /// A child object closed by `end`, which Steam writes as 8.
    Obj(&'static str, Vec<Kv>, u8),
    Text(&'static str, Vec<u8>),
    /// A type with a fixed payload of `len` bytes, all 8, so a misread size
    /// shows up as a stray end marker.
    Fixed(u8, &'static str, usize),
    Blob(&'static str, Vec<u8>),
}

fn obj(name: &'static str, children: Vec<Kv>) -> Kv {
    Kv::Obj(name, children, 8)
}

fn text(name: &'static str, value: &str) -> Kv {
    Kv::Text(name, value.as_bytes().to_vec())
}

/// The shape of a real entry: `appinfo` holding `common`.
fn app_with(common: Vec<Kv>) -> Kv {
    obj("appinfo", vec![obj("common", common)])
}

fn game(oslist: &str) -> Kv {
    app_with(vec![text("type", "game"), text("oslist", oslist)])
}

fn encode(kv: &Kv, names: &mut Names, out: &mut Vec<u8>) {
    match kv {
        Kv::Obj(name, children, end) => {
            out.push(0);
            out.extend(names.index(name).to_le_bytes());
            for child in children {
                encode(child, names, out);
            }
            out.push(*end);
        }
        Kv::Text(name, value) => {
            out.push(1);
            out.extend(names.index(name).to_le_bytes());
            out.extend(value);
            out.push(0);
        }
        Kv::Fixed(kind, name, len) => {
            out.push(*kind);
            out.extend(names.index(name).to_le_bytes());
            out.extend(std::iter::repeat_n(8u8, *len));
        }
        Kv::Blob(name, bytes) => {
            out.push(9);
            out.extend(names.index(name).to_le_bytes());
            out.extend(u32::try_from(bytes.len()).unwrap().to_le_bytes());
            out.extend(bytes);
        }
    }
}

struct Fixture {
    names: Names,
    entries: Vec<u8>,
    /// What closes the list. Steam writes a zero id.
    end: Vec<u8>,
    /// Bytes between the end of the list and the string table.
    after_end: Vec<u8>,
}

impl Fixture {
    fn new() -> Self {
        Self {
            names: Names::default(),
            entries: Vec::new(),
            end: vec![0; 4],
            after_end: Vec::new(),
        }
    }

    /// An entry holding `root` and the level's end marker.
    fn app(&mut self, id: u32, root: Kv) -> &mut Self {
        let mut kv = Vec::new();
        encode(&root, &mut self.names, &mut kv);
        kv.push(8);
        self.raw(id, &kv)
    }

    /// An entry holding exactly these key/value bytes.
    fn raw(&mut self, id: u32, kv: &[u8]) -> &mut Self {
        let mut body = vec![0xAA; ENTRY_META_LEN];
        body.extend(kv);
        self.sized(id, u32::try_from(body.len()).unwrap(), &body)
    }

    /// An entry whose size field may disagree with its body.
    fn sized(&mut self, id: u32, size: u32, body: &[u8]) -> &mut Self {
        self.entries.extend(id.to_le_bytes());
        self.entries.extend(size.to_le_bytes());
        self.entries.extend(body);
        self
    }

    fn build(&self) -> Vec<u8> {
        let table_start = HEADER_LEN + self.entries.len() + self.end.len() + self.after_end.len();
        let mut bytes = Vec::new();
        bytes.extend(MAGIC.to_le_bytes());
        bytes.extend(1u32.to_le_bytes());
        bytes.extend(i64::try_from(table_start).unwrap().to_le_bytes());
        bytes.extend(&self.entries);
        bytes.extend(&self.end);
        bytes.extend(&self.after_end);
        bytes.extend(u32::try_from(self.names.0.len()).unwrap().to_le_bytes());
        for name in &self.names.0 {
            bytes.extend(name);
            bytes.push(0);
        }
        bytes
    }
}

fn facts(oslist: Option<&str>, kind: Option<&str>) -> AppFacts {
    AppFacts {
        oslist: oslist.map(str::to_owned),
        kind: kind.map(str::to_owned),
    }
}

fn parse_ok(bytes: &[u8]) -> AppInfo {
    parse(bytes).unwrap()
}

fn oslist_of(common: Vec<Kv>) -> Option<AppFacts> {
    let mut fixture = Fixture::new();
    fixture.app(7, app_with(common));
    parse_ok(&fixture.build()).get(7).cloned()
}

#[test]
fn reads_every_app_in_the_file() {
    let mut fixture = Fixture::new();
    fixture
        .app(10, game("windows"))
        .app(20, game("linux"))
        .app(30, game("windows,linux"));
    let info = parse_ok(&fixture.build());
    assert_eq!(info.len(), 3);
    assert_eq!(info.get(10), Some(&facts(Some("windows"), Some("game"))));
    assert_eq!(info.get(20), Some(&facts(Some("linux"), Some("game"))));
    assert_eq!(
        info.get(30),
        Some(&facts(Some("windows,linux"), Some("game")))
    );
    assert_eq!(info.get(40), None);
}

#[test]
fn an_empty_list_is_readable() {
    let info = parse_ok(&Fixture::new().build());
    assert!(info.is_empty());
}

#[test]
fn skips_every_value_type_without_losing_its_place() {
    let skipped = vec![
        Kv::Fixed(2, "a", 4),
        Kv::Fixed(3, "b", 4),
        Kv::Fixed(6, "c", 4),
        Kv::Fixed(7, "d", 8),
        Kv::Fixed(10, "e", 8),
        Kv::Blob("f", vec![8, 8, 8]),
        Kv::Blob("g", Vec::new()),
        text("h", "x"),
        obj("i", vec![Kv::Fixed(2, "a", 4), obj("j", Vec::new())]),
        // Type 11 also ends an object.
        Kv::Obj("k", vec![Kv::Fixed(3, "b", 4)], 11),
    ];
    let wanted = || vec![text("type", "Game"), text("oslist", "windows")];
    let want = facts(Some("windows"), Some("game"));

    // Skipped values before `common`, inside it, and after it.
    let mut before = skipped.clone_all();
    before.push(obj("common", wanted()));
    let mut inside = wanted();
    inside.extend(skipped.clone_all());
    let mut after = vec![obj("common", wanted())];
    after.extend(skipped.clone_all());

    let mut fixture = Fixture::new();
    fixture
        .app(1, obj("appinfo", before))
        .app(2, app_with(inside))
        .app(3, obj("appinfo", after))
        .app(4, game("linux"));
    let info = parse_ok(&fixture.build());
    for id in 1..=3 {
        assert_eq!(info.get(id), Some(&want), "app {id}");
    }
    assert_eq!(info.get(4), Some(&facts(Some("linux"), Some("game"))));
}

trait CloneAll {
    fn clone_all(&self) -> Vec<Kv>;
}

impl CloneAll for Vec<Kv> {
    fn clone_all(&self) -> Vec<Kv> {
        self.iter().map(clone_kv).collect()
    }
}

fn clone_kv(kv: &Kv) -> Kv {
    match kv {
        Kv::Obj(name, children, end) => Kv::Obj(name, children.clone_all(), *end),
        Kv::Text(name, value) => Kv::Text(name, value.clone()),
        Kv::Fixed(kind, name, len) => Kv::Fixed(*kind, name, *len),
        Kv::Blob(name, bytes) => Kv::Blob(name, bytes.clone()),
    }
}

#[test]
fn oslist_shapes() {
    assert_eq!(
        oslist_of(vec![text("type", "game")]),
        Some(facts(None, Some("game")))
    );
    assert_eq!(
        oslist_of(vec![text("oslist", ""), text("type", "game")]),
        Some(facts(Some(""), Some("game")))
    );
    assert_eq!(
        oslist_of(vec![text("oslist", "windows")]),
        Some(facts(Some("windows"), None))
    );
    assert_eq!(
        oslist_of(vec![text("oslist", "linux")]),
        Some(facts(Some("linux"), None))
    );
    assert_eq!(
        oslist_of(vec![text("oslist", "windows,linux")]),
        Some(facts(Some("windows,linux"), None))
    );
    assert_eq!(
        oslist_of(vec![text("oslist", " Windows , macos,LINUX,, ")]),
        Some(facts(Some("windows,macos,linux"), None))
    );
}

#[test]
fn type_is_lowercased() {
    for (written, kept) in [("Game", "game"), ("DLC", "dlc"), (" Music ", "music")] {
        assert_eq!(
            oslist_of(vec![text("type", written)]),
            Some(facts(None, Some(kept)))
        );
    }
}

#[test]
fn the_first_oslist_and_type_win() {
    assert_eq!(
        oslist_of(vec![
            text("oslist", "linux"),
            text("oslist", "windows"),
            text("type", "game"),
            text("type", "demo"),
        ]),
        Some(facts(Some("linux"), Some("game")))
    );
}

#[test]
fn an_app_without_common_has_no_facts_but_is_present() {
    let mut fixture = Fixture::new();
    fixture
        .app(1, obj("appinfo", vec![text("appid", "1")]))
        .app(2, obj("appinfo", Vec::new()))
        .app(3, obj("appinfo", vec![text("common", "game")]))
        .app(4, obj("appinfo", vec![Kv::Fixed(2, "common", 4)]));
    let info = parse_ok(&fixture.build());
    for id in 1..=4 {
        assert_eq!(info.get(id), Some(&facts(None, None)), "app {id}");
    }
}

#[test]
fn oslist_and_type_outside_common_are_ignored() {
    let mut fixture = Fixture::new();
    fixture.app(
        1,
        obj(
            "appinfo",
            vec![
                // Alongside `common`, not in it.
                text("oslist", "linux"),
                text("type", "demo"),
                obj(
                    "depots",
                    vec![obj(
                        "1001",
                        vec![text("oslist", "linux"), text("type", "demo")],
                    )],
                ),
                obj(
                    "common",
                    vec![
                        obj(
                            "config",
                            vec![text("oslist", "linux"), text("type", "demo")],
                        ),
                        text("oslist", "windows"),
                    ],
                ),
            ],
        ),
    );
    let info = parse_ok(&fixture.build());
    assert_eq!(info.get(1), Some(&facts(Some("windows"), None)));
}

#[test]
fn only_a_root_level_common_counts() {
    let mut fixture = Fixture::new();
    fixture.app(
        1,
        obj(
            "appinfo",
            vec![obj(
                "extended",
                vec![obj("common", vec![text("oslist", "linux")])],
            )],
        ),
    );
    assert_eq!(parse_ok(&fixture.build()).get(1), Some(&facts(None, None)));
}

#[test]
fn key_names_match_without_regard_to_case() {
    let mut fixture = Fixture::new();
    fixture.app(
        1,
        obj(
            "appinfo",
            vec![obj(
                "Common",
                vec![text("OSList", "linux"), text("Type", "game")],
            )],
        ),
    );
    assert_eq!(
        parse_ok(&fixture.build()).get(1),
        Some(&facts(Some("linux"), Some("game")))
    );
}

#[test]
fn a_duplicate_app_id_keeps_the_first() {
    let mut fixture = Fixture::new();
    fixture.app(1, game("linux")).app(1, game("windows"));
    assert_eq!(
        parse_ok(&fixture.build()).get(1),
        Some(&facts(Some("linux"), Some("game")))
    );
}

#[test]
fn unknown_magic_is_unreadable() {
    let mut bytes = Fixture::new().build();
    for magic in [0x0756_4428u32, 0x0756_4427, 0x0756_442A, 0, u32::MAX] {
        bytes[..4].copy_from_slice(&magic.to_le_bytes());
        assert!(
            matches!(parse(&bytes), Err(ReadError::UnknownMagic(found)) if found == magic),
            "{magic:#x}"
        );
    }
}

#[test]
fn a_short_or_empty_file_is_unreadable() {
    for len in 0..HEADER_LEN {
        let bytes = Fixture::new().build();
        assert!(parse(&bytes[..len]).is_err(), "{len}");
    }
}

#[test]
fn every_truncation_is_unreadable_and_never_panics() {
    let mut fixture = Fixture::new();
    fixture.app(1, game("windows")).app(2, game("linux"));
    let bytes = fixture.build();
    assert!(parse(&bytes).is_ok());
    // The string table is last, so a file cut anywhere has lost part of it.
    for len in 0..bytes.len() {
        assert!(parse(&bytes[..len]).is_err(), "cut at {len}");
    }
}

#[test]
fn every_cut_of_an_entry_is_a_miss_for_that_app_only() {
    let mut names = Names::default();
    let mut kv = Vec::new();
    encode(&game("windows"), &mut names, &mut kv);
    kv.push(8);
    for cut in 0..kv.len() {
        let mut fixture = Fixture::new();
        fixture.names = Names(names.0.clone());
        fixture
            .app(1, game("linux"))
            .raw(2, &kv[..cut])
            .app(3, game("windows"));
        let info = parse_ok(&fixture.build());
        assert_eq!(info.get(2), None, "cut at {cut}");
        assert!(info.get(1).is_some() && info.get(3).is_some(), "cut {cut}");
    }
}

#[test]
fn a_table_offset_outside_the_file_is_unreadable() {
    let mut fixture = Fixture::new();
    fixture.app(1, game("windows"));
    let good = fixture.build();
    let len = i64::try_from(good.len()).unwrap();
    for offset in [-1, 0, 15, len + 1, len + 1000, i64::MAX, i64::MIN] {
        let mut bytes = good.clone();
        bytes[8..16].copy_from_slice(&offset.to_le_bytes());
        assert!(parse(&bytes).is_err(), "offset {offset}");
    }
    // At the very end there is no room for the count.
    let mut bytes = good.clone();
    bytes[8..16].copy_from_slice(&len.to_le_bytes());
    assert!(parse(&bytes).is_err());
}

#[test]
fn a_table_offset_inside_the_entries_is_unreadable_or_harmless() {
    let mut fixture = Fixture::new();
    fixture.app(1, game("windows")).app(2, game("linux"));
    let good = fixture.build();
    for offset in 16..good.len() {
        let mut bytes = good.clone();
        bytes[8..16].copy_from_slice(&i64::try_from(offset).unwrap().to_le_bytes());
        // Keys now index garbage, so an app can only be missing or still right.
        if let Ok(info) = parse(&bytes) {
            assert!(info.len() <= 2, "offset {offset}");
            for (app_id, oslist) in [(1, "windows"), (2, "linux")] {
                assert!(
                    info.get(app_id)
                        .is_none_or(|facts| facts.oslist.as_deref() == Some(oslist)),
                    "offset {offset}"
                );
            }
        }
    }
}

#[test]
fn parsing_honors_the_size_cap_too() {
    let mut fixture = Fixture::new();
    fixture.app(1, game("windows"));
    let bytes = fixture.build();
    let len = u64::try_from(bytes.len()).unwrap();
    let limits = |file_bytes| Limits {
        file_bytes,
        ..LIMITS
    };
    assert!(parse_with(&bytes, limits(len)).is_ok());
    assert!(matches!(
        parse_with(&bytes, limits(len - 1)),
        Err(ReadError::TooLarge)
    ));
}

#[test]
fn a_kept_value_over_the_cap_is_a_miss_for_that_app() {
    let mut fixture = Fixture::new();
    fixture
        .app(1, game(&",".repeat(16)))
        .app(2, game("windows"));
    let bytes = fixture.build();

    let limits = Limits {
        value_bytes: 7,
        ..LIMITS
    };
    let info = parse_with(&bytes, limits).unwrap();
    assert_eq!(info.get(1), None);
    assert_eq!(info.get(2).unwrap().oslist.as_deref(), Some("windows"));

    let limits = Limits {
        value_bytes: 16,
        ..LIMITS
    };
    assert_eq!(
        parse_with(&bytes, limits)
            .unwrap()
            .get(1)
            .unwrap()
            .oslist
            .as_deref(),
        Some("")
    );
}

#[test]
fn an_entry_size_past_the_end_is_unreadable() {
    let mut fixture = Fixture::new();
    fixture.app(1, game("windows"));
    fixture.sized(2, 100_000, &[0; ENTRY_META_LEN]);
    assert!(parse(&fixture.build()).is_err());

    let mut fixture = Fixture::new();
    fixture.sized(2, u32::MAX, &[]);
    assert!(parse(&fixture.build()).is_err());

    // Past the list but still inside the file, where the string table sits.
    let mut fixture = Fixture::new();
    fixture.app(1, game("windows")).sized(2, 20, &[0; 4]);
    assert!(parse(&fixture.build()).is_err());
}

#[test]
fn a_cut_id_or_size_in_the_list_is_unreadable() {
    for end in [vec![0, 0], vec![5, 0, 0, 0], vec![5, 0, 0, 0, 1, 0]] {
        let mut fixture = Fixture::new();
        fixture.app(1, game("windows"));
        fixture.end = end;
        assert!(parse(&fixture.build()).is_err());
    }
}

#[test]
fn a_list_may_run_to_the_table_without_a_terminator() {
    let mut fixture = Fixture::new();
    fixture.app(1, game("windows"));
    fixture.end = Vec::new();
    assert_eq!(parse_ok(&fixture.build()).len(), 1);
}

#[test]
fn app_id_zero_ends_the_list() {
    let mut fixture = Fixture::new();
    fixture.app(1, game("windows"));
    // A well-formed entry past the terminator must not be read.
    let mut extra = Fixture {
        names: Names(fixture.names.0.clone()),
        ..Fixture::new()
    };
    extra.app(2, game("linux"));
    fixture.after_end = extra.entries;
    let info = parse_ok(&fixture.build());
    assert!(info.get(1).is_some());
    assert_eq!(info.get(2), None);
}

#[test]
fn a_broken_string_table_is_unreadable() {
    let mut fixture = Fixture::new();
    fixture.app(1, game("windows"));
    let good = fixture.build();
    let table = usize::try_from(i64::from_le_bytes(good[8..16].try_into().unwrap())).unwrap();

    // Counts beyond the bytes left, beyond the cap, and absurd.
    for count in [1_000u32, 0x10_0000, u32::MAX, 0x8000_0000] {
        let mut bytes = good.clone();
        bytes[table..table + 4].copy_from_slice(&count.to_le_bytes());
        assert!(parse(&bytes).is_err(), "count {count}");
    }

    // The last string loses its terminator.
    let mut bytes = good.clone();
    bytes.pop();
    assert!(parse(&bytes).is_err());

    // A count of zero leaves every key out of range, so the entry is a miss.
    let mut bytes = good.clone();
    bytes[table..table + 4].copy_from_slice(&0u32.to_le_bytes());
    assert_eq!(parse_ok(&bytes).get(1), None);
}

#[test]
fn a_key_index_out_of_range_is_a_miss() {
    for index in [i32::MAX, 1000, -1, i32::MIN] {
        let mut fixture = Fixture::new();
        let root = fixture.names.index("appinfo");
        let common = fixture.names.index("common");
        let mut kv = vec![0];
        kv.extend(root.to_le_bytes());
        kv.push(0);
        kv.extend(common.to_le_bytes());
        kv.push(1);
        kv.extend(index.to_le_bytes());
        kv.extend(b"game\0");
        kv.extend([8, 8, 8]);
        fixture
            .app(1, game("linux"))
            .raw(2, &kv)
            .app(3, game("linux"));
        let info = parse_ok(&fixture.build());
        assert_eq!(info.get(2), None, "index {index}");
        assert!(info.get(1).is_some() && info.get(3).is_some());

        // The same index on the root object and on a closing sibling.
        let mut root_kv = vec![0];
        root_kv.extend(index.to_le_bytes());
        root_kv.extend([8, 8]);
        let mut fixture = Fixture::new();
        fixture
            .app(1, game("linux"))
            .raw(2, &root_kv)
            .app(3, game("linux"));
        let info = parse_ok(&fixture.build());
        assert_eq!(info.get(2), None, "root index {index}");
        assert!(info.get(1).is_some() && info.get(3).is_some());
    }
}

#[test]
fn types_4_5_and_unknown_are_a_bad_entry() {
    for kind in [4u8, 5, 12, 13, 100, 255] {
        let mut fixture = Fixture::new();
        let unknown = obj(
            "appinfo",
            vec![
                Kv::Fixed(kind, "odd", 4),
                obj("common", vec![text("oslist", "linux")]),
            ],
        );
        fixture
            .app(1, game("windows"))
            .app(2, unknown)
            .app(3, game("linux"));
        let info = parse_ok(&fixture.build());
        assert_eq!(info.get(2), None, "type {kind}");
        assert_eq!(
            info.get(1),
            Some(&facts(Some("windows"), Some("game"))),
            "type {kind}"
        );
        assert_eq!(
            info.get(3),
            Some(&facts(Some("linux"), Some("game"))),
            "type {kind}"
        );
    }
}

#[test]
fn a_bad_entry_between_two_good_ones_costs_only_itself() {
    let mut fixture = Fixture::new();
    fixture
        .app(1, game("windows"))
        .sized(2, 3, &[1, 2, 3])
        .app(3, game("linux"))
        .app(4, game("windows,linux"));
    let info = parse_ok(&fixture.build());
    assert_eq!(info.get(2), None);
    assert_eq!(info.len(), 3);
    assert_eq!(info.get(1), Some(&facts(Some("windows"), Some("game"))));
    assert_eq!(info.get(3), Some(&facts(Some("linux"), Some("game"))));
}

#[test]
fn entries_too_short_for_key_values_are_misses() {
    for size in [0, 1, 59, 60] {
        let mut fixture = Fixture::new();
        fixture
            .app(1, game("windows"))
            .sized(2, size, &vec![0xAA; size as usize])
            .app(3, game("linux"));
        let info = parse_ok(&fixture.build());
        assert_eq!(info.get(2), None, "size {size}");
        assert!(info.get(1).is_some() && info.get(3).is_some());
    }
}

#[test]
fn the_bookkeeping_bytes_are_never_read_as_values() {
    // 0xAA bytes in the 60 bytes would be an unknown type if they were read.
    let mut fixture = Fixture::new();
    fixture.app(1, game("linux"));
    assert!(parse_ok(&fixture.build()).get(1).is_some());
}

#[test]
fn a_blob_longer_than_its_entry_is_a_miss_that_stays_inside_the_entry() {
    for length in [1000u32, u32::MAX] {
        let mut fixture = Fixture::new();
        let names = &mut fixture.names;
        let (root, blob) = (names.index("appinfo"), names.index("blob"));
        let mut kv = vec![0];
        kv.extend(root.to_le_bytes());
        kv.push(9);
        kv.extend(blob.to_le_bytes());
        kv.extend(length.to_le_bytes());
        kv.extend([8, 8]);
        // The next entry's bytes lie within `length`, and must not be taken.
        fixture.raw(1, &kv).app(2, game("linux"));
        let info = parse_ok(&fixture.build());
        assert_eq!(info.get(1), None, "length {length}");
        assert!(info.get(2).is_some());
    }
}

#[test]
fn a_string_without_a_terminator_is_a_miss() {
    let mut fixture = Fixture::new();
    let names = &mut fixture.names;
    let (root, kind) = (names.index("appinfo"), names.index("x"));
    let mut kv = vec![0];
    kv.extend(root.to_le_bytes());
    kv.push(1);
    kv.extend(kind.to_le_bytes());
    kv.extend(b"no end");
    fixture.raw(1, &kv).app(2, game("linux"));
    let info = parse_ok(&fixture.build());
    assert_eq!(info.get(1), None);
    assert!(info.get(2).is_some());
}

#[test]
fn a_missing_end_marker_is_a_miss() {
    let mut names = Names::default();
    let mut kv = Vec::new();
    encode(&game("linux"), &mut names, &mut kv);
    // Only the root's own end marker, with none for the level above it.
    let mut fixture = Fixture::new();
    fixture.names = names;
    fixture.raw(1, &kv).app(2, game("linux"));
    let info = parse_ok(&fixture.build());
    assert_eq!(info.get(1), None);
    assert!(info.get(2).is_some());
}

#[test]
fn the_root_must_be_an_object() {
    let mut fixture = Fixture::new();
    fixture
        .app(1, text("appinfo", "x"))
        .app(2, Kv::Fixed(2, "appinfo", 4))
        .app(3, game("linux"));
    let info = parse_ok(&fixture.build());
    assert_eq!(info.get(1), None);
    assert_eq!(info.get(2), None);
    assert!(info.get(3).is_some());
}

#[test]
fn bytes_after_the_root_are_ignored() {
    let mut names = Names::default();
    let mut kv = Vec::new();
    encode(&game("linux"), &mut names, &mut kv);
    kv.extend([8, 1, 2, 3]);
    let mut fixture = Fixture::new();
    fixture.names = names;
    fixture.raw(1, &kv);
    assert_eq!(
        parse_ok(&fixture.build()).get(1),
        Some(&facts(Some("linux"), Some("game")))
    );
}

fn nested(levels: usize) -> Kv {
    let mut inner = obj("n", Vec::new());
    for _ in 1..levels {
        inner = obj("n", vec![inner]);
    }
    inner
}

#[test]
fn a_deeply_nested_entry_is_a_miss() {
    let mut fixture = Fixture::new();
    fixture
        .app(1, app_with(vec![nested(MAX_DEPTH - 2)]))
        .app(2, app_with(vec![nested(MAX_DEPTH)]))
        .app(4, game("linux"));
    // Built flat, since a tree this deep would overflow the test's own stack.
    let key = fixture.names.index("n");
    let mut deep = Vec::new();
    for _ in 0..100_000 {
        deep.push(0);
        deep.extend(key.to_le_bytes());
    }
    fixture.raw(3, &deep);
    let info = parse_ok(&fixture.build());
    // Root and `common` take two levels of the cap.
    assert!(info.get(1).is_some());
    assert_eq!(info.get(2), None);
    assert_eq!(info.get(3), None);
    assert!(info.get(4).is_some());
}

#[test]
fn the_depth_cap_counts_objects_from_the_root() {
    let limits = Limits { depth: 3, ..LIMITS };
    let build = |levels| {
        let mut fixture = Fixture::new();
        fixture.app(1, app_with(vec![nested(levels)]));
        fixture.build()
    };
    // appinfo, common, and one more.
    assert!(parse_with(&build(1), limits).unwrap().get(1).is_some());
    assert_eq!(parse_with(&build(2), limits).unwrap().get(1), None);
}

#[test]
fn too_many_entries_is_unreadable() {
    let limits = Limits { apps: 2, ..LIMITS };
    let mut fixture = Fixture::new();
    fixture.app(1, game("linux")).app(2, game("linux"));
    assert_eq!(parse_with(&fixture.build(), limits).unwrap().len(), 2);
    fixture.app(3, game("linux"));
    assert!(parse_with(&fixture.build(), limits).is_err());

    // Bad entries count too, since they cost the same to walk.
    let mut fixture = Fixture::new();
    fixture.sized(1, 0, &[]).sized(2, 0, &[]).sized(3, 0, &[]);
    assert!(parse_with(&fixture.build(), limits).is_err());
}

#[test]
fn too_many_strings_is_unreadable() {
    let limits = Limits {
        strings: 3,
        ..LIMITS
    };
    // appinfo, common, type, oslist
    let mut fixture = Fixture::new();
    fixture.app(1, game("linux"));
    assert_eq!(fixture.names.0.len(), 4);
    assert!(parse_with(&fixture.build(), limits).is_err());
    assert!(parse_with(&fixture.build(), LIMITS).is_ok());
}

#[test]
fn text_that_is_not_utf8_is_handled_per_use() {
    let mut fixture = Fixture::new();
    let names = &mut fixture.names;
    // A key that is not UTF-8 can't match anything, and doesn't break the file.
    let odd_key = names.index([0xff, 0xfe]);
    let mut odd = vec![0];
    odd.extend(names.index("appinfo").to_le_bytes());
    odd.push(1);
    odd.extend(odd_key.to_le_bytes());
    odd.extend(b"fine\0");
    odd.push(0);
    odd.extend(names.index("common").to_le_bytes());
    odd.push(1);
    odd.extend(names.index("oslist").to_le_bytes());
    odd.extend(b"linux\0");
    odd.extend([8, 8, 8]);

    fixture
        .app(
            1,
            app_with(vec![
                Kv::Text("note", vec![0xff, 0xfe, 0xfd]),
                text("oslist", "windows"),
            ]),
        )
        // A kept value that is not UTF-8 makes the entry a miss.
        .app(2, app_with(vec![Kv::Text("oslist", vec![0xff, 0xfe])]))
        .app(3, app_with(vec![Kv::Text("type", vec![b'g', 0xc3])]))
        .raw(4, &odd)
        .app(5, game("linux"));
    let info = parse_ok(&fixture.build());
    assert_eq!(info.get(1), Some(&facts(Some("windows"), None)));
    assert_eq!(info.get(2), None);
    assert_eq!(info.get(3), None);
    assert_eq!(info.get(4), Some(&facts(Some("linux"), None)));
    assert!(info.get(5).is_some());
}

#[test]
fn nothing_but_facts_is_kept() {
    let mut fixture = Fixture::new();
    fixture.app(
        1,
        obj(
            "appinfo",
            vec![
                text("token", "SECRET-ACCESS-TOKEN"),
                obj(
                    "common",
                    vec![text("oslist", "linux"), text("name", "SECRET")],
                ),
            ],
        ),
    );
    let info = parse_ok(&fixture.build());
    let shown = format!("{info:?}");
    assert!(!shown.contains("SECRET"), "{shown}");
}

#[test]
fn errors_never_carry_file_contents() {
    let mut fixture = Fixture::new();
    fixture.app(1, text("SECRET", "SECRET"));
    let mut bytes = fixture.build();
    bytes[8..16].copy_from_slice(&i64::MAX.to_le_bytes());
    let error = parse(&bytes).unwrap_err().to_string();
    assert!(!error.contains("SECRET"), "{error}");
}

fn install_at(root: &std::path::Path) -> Install {
    Install {
        kind: InstallKind::Native,
        root: root.to_owned(),
    }
}

#[test]
fn reads_the_file_under_the_install() {
    let dir = tempfile::tempdir().unwrap();
    let install = install_at(dir.path());
    assert!(matches!(read(&install), Err(ReadError::Io(_))));

    fs::create_dir_all(dir.path().join("appcache")).unwrap();
    let mut fixture = Fixture::new();
    fixture.app(1, game("linux"));
    fs::write(dir.path().join("appcache/appinfo.vdf"), fixture.build()).unwrap();
    assert_eq!(
        read(&install).unwrap().get(1),
        Some(&facts(Some("linux"), Some("game")))
    );
}

#[test]
fn a_file_over_the_size_cap_is_unreadable() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("appinfo.vdf");
    let mut fixture = Fixture::new();
    fixture.app(1, game("linux"));
    let bytes = fixture.build();
    fs::write(&path, &bytes).unwrap();
    let len = bytes.len() as u64;
    assert!(
        read_file(
            &path,
            Limits {
                file_bytes: len,
                ..LIMITS
            }
        )
        .is_ok()
    );
    assert!(matches!(
        read_file(
            &path,
            Limits {
                file_bytes: len - 1,
                ..LIMITS
            }
        ),
        Err(ReadError::TooLarge)
    ));
}

#[test]
fn a_directory_in_the_way_is_an_error_not_a_panic() {
    let dir = tempfile::tempdir().unwrap();
    assert!(read_file(dir.path(), LIMITS).is_err());
}
