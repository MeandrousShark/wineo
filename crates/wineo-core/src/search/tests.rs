use super::{Field, Search};

fn matches(field: &str, search: &str) -> bool {
    Search::new(search).matches(&Field::new(field))
}

fn check(cases: &[(&str, &str, bool)]) {
    let wrong: Vec<String> = cases
        .iter()
        .filter(|(field, search, expected)| matches(field, search) != *expected)
        .map(|(field, search, expected)| format!("{search:?} in {field:?}: expected {expected}"))
        .collect();
    assert!(wrong.is_empty(), "{wrong:#?}");
}

#[test]
fn substring_still_matches_and_folds_accents() {
    check(&[
        ("Harbor Lights", "harbor", true),
        ("Harbor Lights", "BOR LI", true),
        ("Baldur's Gate 3", "baldurs", true),
        ("Éclair Tactics", "eclair", true),
        ("E\u{301}clair Tactics", "eclair", true),
        ("Pokémon Crystal", "pokemon", true),
        ("Iron Harbor: Director's Cut", "directors", true),
    ]);
}

#[test]
fn joined_ignores_everything_but_letters_and_digits() {
    check(&[
        ("Spider-Man", "spiderman", true),
        ("Half-Life 2", "halflife", true),
        ("co-op", "coop", true),
        ("Sky-Runner", "skyrunner", true),
    ]);
}

#[test]
fn word_starts_in_any_order_with_numerals_by_value() {
    check(&[
        ("Sid Meier's Civilization VI", "civ 6", true),
        ("Grand Theft Auto IV", "auto gr", true),
        ("Fallout 4", "fallout iv", true),
        ("StarForge II", "starforge 2", true),
        ("StarForge II", "StarForge 2", true),
        ("Spider-Man II", "spiderman 2", true),
        ("Mega Man X", "mega man 10", true),
        ("I Am Bread", "1", true),
        ("Vortex 2", "v 2", true),
        ("Strip 3 of 8", "strip iii", true),
        ("Grand Tidal Armada IV", "tidal iv", true),
        ("Grand Tidal Armada IV", "armada 4", true),
        ("Grand Tidal Armada IV", "gr ar", true),
        ("Звёздный архив", "зв арх", true),
        ("Звёздный архив", "zvezdny", false),
        ("Sid Meier's Civilization VI", "civilisation", false),
    ]);
}

#[test]
fn initials_spell_a_run_of_words() {
    check(&[
        ("Grand Theft Auto IV", "gta", true),
        ("Grand Theft Auto IV", "gta4", true),
        ("Grand Theft Auto IV", "gta 4", true),
        ("Grand Theft Auto IV", " gta 4 ", true),
        ("Grand Theft Auto IV", "gtaiv", true),
        ("Grand Theft Auto V", "gta v", true),
        ("Final Fantasy VII", "ff7", true),
        ("Final Fantasy VII", "ff 7", true),
        ("Final Fantasy VII", "ffvii", true),
        ("Final Fantasy VII", "f.f. 7", true),
        ("Baldur's Gate 3", "bg3", true),
        ("Left 4 Dead 2", "l4d2", true),
        ("Half-Life 2", "hl2", true),
        ("StarCraft II", "sc2", true),
        ("Halo2", "h2", true),
        ("Sid Meier's Civilization", "smc", true),
        ("Under_score_Quest_2", "usq2", true),
        ("Assassin's Creed: Origins", "aco", true),
        (
            "Grand Arena 99999999999999999999",
            "ga 99999999999999999999",
            true,
        ),
    ]);
}

#[test]
fn near_misses() {
    check(&[
        ("Sid Meier's Civilization VI", "civ6", false),
        ("Sid Meier's Civilization VI", "CIV6", false),
        ("Grand Theft Auto IV", "atg", false),
        ("Grand Theft", "gta", false),
        ("Fallout 4", "foiv", false),
        ("Strip 5 of 8", "v", false),
        ("Fallout vi", "fallout 6", false),
        ("Final Fantasy X", "ffx", true),
        ("Final Fantasy XXX", "ffx", true),
        ("Final Fantasy V", "ffv", true),
        ("Final Fantasy VIII", "ffv", true),
        ("Final Fantasy 13", "ff1", true),
        ("Final Fantasy XIII", "ff1", false),
    ]);
}

#[test]
fn only_canonical_uppercase_roman_and_digits_are_numerals() {
    check(&[
        ("Civ", "104", false),
        ("Mix", "1009", false),
        ("XXXI", "31", false),
        ("IIII", "4", false),
        ("XXX", "30", true),
        ("Game XIX", "game 19", true),
    ]);
}

#[test]
fn digit_runs_too_long_for_u32_have_no_value() {
    check(&[
        (
            "Big 99999999999999999999",
            "big 88888888888888888888",
            false,
        ),
        ("Big 99999999999999999999", "big 4294967295", false),
        ("Big 4294967296", "big 0", false),
        ("Big 99999999999999999999", "99999999999999999999", true),
        ("Big 4294967295", "big MMMMMMMMMMMMMMMMMMMMM", false),
    ]);
}

#[test]
fn initials_need_two_letters_or_digits() {
    assert!(Search::new("g 4").initials.is_some());
    assert!(Search::new("g4").initials.is_some());
    assert!(Search::new("g").initials.is_none());
    assert!(Search::new("g -").initials.is_none());
    assert!(Search::new("gta rpg").initials.is_none());
}

#[test]
fn empty_space_and_punctuation_searches() {
    assert!(Search::new("").is_empty());
    assert!(Search::new("   ").is_empty());
    assert!(!Search::new("-").is_empty());
    check(&[
        ("Halo", "", true),
        ("Halo", "   ", true),
        ("Halo", "-", false),
        ("Half-Life", "-", true),
        ("Baldur's Gate", "'", true),
        ("Baldurs Gate", "'", false),
        ("Halo", "!?", false),
        ("Halo!?", "!?", true),
    ]);
}

/// Titles in the style of the acceptance library, plus text whose case and
/// length change when lowercased.
const CORPUS: &[&str] = &[
    "Orchard Keeper",
    "Voidmarch: Ascension",
    "Iron Harbor: Director's Cut",
    "Frozen Garden",
    "Звёздный архив 影の塔 Café",
    "Notepad++",
    "Ōkami no Tō 影の塔 Café Ñandú",
    "Zebra Safari",
    "ancient lantern",
    "Éclair Tactics",
    "E\u{301}clair Tactics",
    "7 Days of Dust",
    "Assassin's Creed: Origins",
    "Assassin\u{2019}s Creed",
    "Under_score_Quest_2",
    "Strip 5 of 8",
    "Grand Tidal Armada IV",
    "Sky-Runner",
    "StarForge II",
    "co-op",
    "ΟΔΥΣΣΕΑΣ",
    "İstanbul",
    "Straße",
    "ẞ",
    "’",
];

fn today(field: &str, search: &str) -> bool {
    field.to_lowercase().contains(&search.to_lowercase())
}

#[test]
fn every_search_that_matches_today_still_matches() {
    let mut checked = 0;
    let mut missed = Vec::new();
    for field in CORPUS {
        let prepared = Field::new(field);
        let starts: Vec<usize> = field
            .char_indices()
            .map(|(at, _)| at)
            .chain([field.len()])
            .collect();
        for (i, &start) in starts.iter().enumerate() {
            for &end in &starts[i..] {
                let piece = &field[start..end];
                for search in [piece.to_owned(), piece.to_lowercase(), piece.to_uppercase()] {
                    if !today(field, &search) {
                        continue;
                    }
                    checked += 1;
                    if !Search::new(&search).matches(&prepared) {
                        missed.push(format!("{search:?} in {field:?}"));
                    }
                }
            }
        }
    }
    assert!(checked > 1000, "only {checked} searches checked");
    assert!(missed.is_empty(), "{missed:#?}");
}
