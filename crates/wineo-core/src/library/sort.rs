//! The library's display order.

use std::cmp::Ordering;

use jiff::Timestamp;

use super::SortKey;

/// What one environment sorts by. The caller fills it once per sort, so
/// nothing here is recomputed per comparison.
#[derive(Debug, Clone, Copy)]
pub struct SortFields<'a> {
    /// The displayed title, already lowercased.
    pub title: &'a str,
    pub name: &'a str,
    pub seconds: i64,
    /// Playtime that couldn't be read, which is unknown, not never played.
    pub playtime_unknown: bool,
    pub last_played: Option<Timestamp>,
    pub added: Option<Timestamp>,
    pub favorite: bool,
}

/// A key's value, where an unknown one sorts after every known one in both
/// directions, since unknown isn't the least or the oldest.
#[derive(PartialEq, Eq, PartialOrd, Ord)]
enum Value<T> {
    Known(T),
    Unknown,
}

/// Orders two environments. Favorites lead when `favorites_first` is on,
/// whatever the direction. `descending` turns only the key around; ties
/// fall back to the title and then the identifier, ascending, so the order
/// is total.
pub fn compare(
    a: &SortFields<'_>,
    b: &SortFields<'_>,
    sort: SortKey,
    descending: bool,
    favorites_first: bool,
) -> Ordering {
    let favorites = if favorites_first {
        b.favorite.cmp(&a.favorite)
    } else {
        Ordering::Equal
    };
    favorites
        .then_with(|| compare_key(a, b, sort, descending))
        .then_with(|| a.title.cmp(b.title))
        .then_with(|| a.name.cmp(b.name))
}

fn compare_key(
    a: &SortFields<'_>,
    b: &SortFields<'_>,
    sort: SortKey,
    descending: bool,
) -> Ordering {
    match sort {
        SortKey::Name => directed(Value::Known(a.title), Value::Known(b.title), descending),
        SortKey::Playtime => directed(playtime(a), playtime(b), descending),
        // Never played is None, which is older than any time.
        SortKey::LastPlayed => directed(last_played(a), last_played(b), descending),
        SortKey::Added => directed(added(a), added(b), descending),
    }
}

fn directed<T: Ord>(a: Value<T>, b: Value<T>, descending: bool) -> Ordering {
    match (&a, &b) {
        (Value::Known(a), Value::Known(b)) if descending => b.cmp(a),
        _ => a.cmp(&b),
    }
}

fn playtime(fields: &SortFields<'_>) -> Value<i64> {
    if fields.playtime_unknown {
        Value::Unknown
    } else {
        Value::Known(fields.seconds)
    }
}

fn last_played(fields: &SortFields<'_>) -> Value<Option<Timestamp>> {
    if fields.playtime_unknown {
        Value::Unknown
    } else {
        Value::Known(fields.last_played)
    }
}

fn added(fields: &SortFields<'_>) -> Value<Timestamp> {
    fields.added.map_or(Value::Unknown, Value::Known)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone)]
    struct Game {
        title: String,
        name: &'static str,
        seconds: i64,
        playtime_unknown: bool,
        last_played: Option<Timestamp>,
        added: Option<Timestamp>,
        favorite: bool,
    }

    fn at(seconds: i64) -> Option<Timestamp> {
        Some(Timestamp::from_second(1_700_000_000 + seconds).unwrap())
    }

    fn game(name: &'static str) -> Game {
        Game {
            title: name.to_owned(),
            name,
            seconds: 0,
            playtime_unknown: false,
            last_played: None,
            added: None,
            favorite: false,
        }
    }

    fn played(name: &'static str, seconds: i64, last: i64) -> Game {
        Game {
            seconds,
            last_played: at(last),
            ..game(name)
        }
    }

    /// The question 3 library: hades and balatro are favorites, and
    /// notepad has never been played.
    fn library() -> Vec<Game> {
        vec![
            Game {
                favorite: true,
                added: at(40),
                ..played("hades", 40 * 3600, 90)
            },
            Game {
                added: at(10),
                ..played("papers-please", 6 * 3600, 60)
            },
            Game {
                added: at(30),
                ..game("notepad")
            },
            Game {
                favorite: true,
                ..played("balatro", 20 * 3600, 100)
            },
            Game {
                added: at(20),
                ..played("old-demo", 3600, 10)
            },
        ]
    }

    fn order(games: &[Game], sort: SortKey, descending: bool, favorites_first: bool) -> Vec<&str> {
        let fields: Vec<_> = games
            .iter()
            .map(|game| SortFields {
                title: &game.title,
                name: game.name,
                seconds: game.seconds,
                playtime_unknown: game.playtime_unknown,
                last_played: game.last_played,
                added: game.added,
                favorite: game.favorite,
            })
            .collect();
        let mut indices: Vec<usize> = (0..games.len()).collect();
        indices
            .sort_by(|&a, &b| compare(&fields[a], &fields[b], sort, descending, favorites_first));
        indices.into_iter().map(|index| games[index].name).collect()
    }

    #[test]
    fn name_sorts_by_the_lowercased_title_both_ways() {
        let mut games = library();
        games[2].title = "a notepad".to_owned();
        assert_eq!(
            order(&games, SortKey::Name, false, false),
            ["notepad", "balatro", "hades", "old-demo", "papers-please"]
        );
        assert_eq!(
            order(&games, SortKey::Name, true, false),
            ["papers-please", "old-demo", "hades", "balatro", "notepad"]
        );
    }

    #[test]
    fn playtime_puts_never_played_at_the_least_played_end() {
        let games = library();
        assert_eq!(
            order(&games, SortKey::Playtime, true, false),
            ["hades", "balatro", "papers-please", "old-demo", "notepad"]
        );
        assert_eq!(
            order(&games, SortKey::Playtime, false, false),
            ["notepad", "old-demo", "papers-please", "balatro", "hades"]
        );
    }

    #[test]
    fn last_played_puts_never_played_at_the_least_recent_end() {
        let games = library();
        assert_eq!(
            order(&games, SortKey::LastPlayed, true, false),
            ["balatro", "hades", "papers-please", "old-demo", "notepad"]
        );
        assert_eq!(
            order(&games, SortKey::LastPlayed, false, false),
            ["notepad", "old-demo", "papers-please", "hades", "balatro"]
        );
    }

    #[test]
    fn unknown_playtime_sorts_last_both_ways_for_playtime_and_last_played() {
        let mut games = library();
        games.push(Game {
            playtime_unknown: true,
            ..played("steam-game", 0, 200)
        });
        for sort in [SortKey::Playtime, SortKey::LastPlayed] {
            for descending in [false, true] {
                let order = order(&games, sort, descending, false);
                assert_eq!(order.last(), Some(&"steam-game"), "{sort:?} {descending}");
            }
        }
    }

    #[test]
    fn unknown_added_dates_sort_last_both_ways_by_name() {
        let games = library();
        assert_eq!(
            order(&games, SortKey::Added, true, false),
            ["hades", "notepad", "old-demo", "papers-please", "balatro"]
        );
        assert_eq!(
            order(&games, SortKey::Added, false, false),
            ["papers-please", "old-demo", "notepad", "hades", "balatro"]
        );
        let mut undated = library();
        for game in &mut undated {
            game.added = None;
        }
        for descending in [false, true] {
            assert_eq!(
                order(&undated, SortKey::Added, descending, false),
                ["balatro", "hades", "notepad", "old-demo", "papers-please"]
            );
        }
    }

    #[test]
    fn favorites_lead_either_direction_only_when_asked() {
        let games = library();
        assert_eq!(
            order(&games, SortKey::Playtime, true, true),
            ["hades", "balatro", "papers-please", "old-demo", "notepad"]
        );
        assert_eq!(
            order(&games, SortKey::Playtime, false, true),
            ["balatro", "hades", "notepad", "old-demo", "papers-please"]
        );
        assert_eq!(
            order(&games, SortKey::Name, true, true),
            ["hades", "balatro", "papers-please", "old-demo", "notepad"]
        );
        assert_eq!(
            order(&games, SortKey::Name, true, false),
            ["papers-please", "old-demo", "notepad", "hades", "balatro"]
        );
    }

    #[test]
    fn ties_fall_back_to_title_then_identifier_ascending_both_ways() {
        let mut games = vec![
            played("b-copy", 60, 5),
            played("a-copy", 60, 5),
            played("c", 60, 5),
        ];
        games[0].title = "same".to_owned();
        games[1].title = "same".to_owned();
        games[2].title = "other".to_owned();
        for sort in [
            SortKey::Name,
            SortKey::LastPlayed,
            SortKey::Playtime,
            SortKey::Added,
        ] {
            assert_eq!(
                order(&games, sort, false, false),
                ["c", "a-copy", "b-copy"],
                "{sort:?}"
            );
            let descending: &[&str] = if sort == SortKey::Name {
                &["a-copy", "b-copy", "c"]
            } else {
                &["c", "a-copy", "b-copy"]
            };
            assert_eq!(order(&games, sort, true, false), descending, "{sort:?}");
        }
    }
}
