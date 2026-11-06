//! Changing every marked game at once: adding or removing a tag, favoriting,
//! or hiding. The cards change at once, each game is written off the UI
//! thread, and the ones that fail go back. Tags go through core's
//! `change_tags`, the flags through `set_favorite_for` and `set_hidden_for`.

use std::collections::BTreeMap;

use iced::Task;
use jiff::Timestamp;
use wineo_core::error::Error;
use wineo_core::library::{self, Guarded};
use wineo_core::paths::Paths;
use wineo_core::settings::{self, TagChange};

use crate::library::Card;
use crate::library_actions::Flag;
use crate::{App, Message, rescan};

/// How many failed games a notice names before it counts the rest.
const NAMED_FAILURES: usize = 3;

/// What a batch changes on each game. For a tag the value is whether the
/// game has it, for a flag whether it's set.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Change {
    Tag(String),
    Flag(Flag),
}

impl Change {
    /// Whether `card` has this change's value now.
    fn present(&self, card: &Card) -> bool {
        match self {
            Self::Tag(tag) => has_tag(card, tag),
            Self::Flag(flag) => flag.of(card),
        }
    }

    fn set(&self, card: &mut Card, value: bool) {
        match self {
            Self::Tag(tag) => set_tag(card, tag, value),
            Self::Flag(flag) => flag.set(card, value),
        }
    }

    /// The verb a failure notice leads with when the batch sets `value`.
    fn verb(&self, value: bool) -> &'static str {
        match (self, value) {
            (Self::Tag(_), true) => "tag",
            (Self::Tag(_), false) => "untag",
            (Self::Flag(Flag::Favorite), true) => "favorite",
            (Self::Flag(Flag::Favorite), false) => "unfavorite",
            (Self::Flag(Flag::Hidden), true) => "hide",
            (Self::Flag(Flag::Hidden), false) => "show",
        }
    }
}

/// The writes in flight, one per game and change. A change made meanwhile is
/// written when the one in flight returns, so the last state wins.
#[derive(Debug, Default)]
pub(crate) struct Writes(BTreeMap<(String, Change), Pending>);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Pending {
    /// The card's `added` when it was changed, so a removed and recreated
    /// environment of the same name gets none of the old change.
    pub(crate) added: Option<Timestamp>,
    /// Whether the write in flight adds the tag.
    pub(crate) writing: bool,
    /// What to write once `writing` lands, when the window wants otherwise.
    pub(crate) next: Option<bool>,
}

impl Pending {
    fn wanted(self) -> bool {
        self.next.unwrap_or(self.writing)
    }
}

impl Writes {
    /// A write is in flight, which closing the window waits for.
    pub(crate) fn pending(&self) -> bool {
        !self.0.is_empty()
    }

    #[cfg(test)]
    pub(crate) fn get(&self, name: &str, tag: &str) -> Option<Pending> {
        self.get_change(name, &Change::Tag(tag.to_owned()))
    }

    #[cfg(test)]
    pub(crate) fn get_flag(&self, name: &str, flag: Flag) -> Option<Pending> {
        self.get_change(name, &Change::Flag(flag))
    }

    #[cfg(test)]
    fn get_change(&self, name: &str, change: &Change) -> Option<Pending> {
        self.0.get(&(name.to_owned(), change.clone())).copied()
    }

    /// Matches the writes to cards just read. A write whose environment is
    /// gone, or was removed and made again, is forgotten: its queued change
    /// never starts and its reply is ignored. The rest put the window's
    /// choice over the cards, so a read made before a write landed doesn't
    /// undo it on screen meanwhile.
    pub(crate) fn reconcile(&mut self, cards: &mut [Card]) {
        self.0.retain(|(name, change), pending| {
            match cards
                .iter_mut()
                .find(|card| &card.name == name && card.added == pending.added)
            {
                Some(card) => {
                    change.set(card, pending.wanted());
                    true
                }
                None => false,
            }
        });
    }
}

/// Whether `card` has `tag`.
pub(crate) fn has_tag(card: &Card, tag: &str) -> bool {
    card.tags.iter().any(|existing| existing == tag)
}

/// Adds `tag` at the end, where core appends it, or removes it.
fn set_tag(card: &mut Card, tag: &str, value: bool) {
    if value {
        if !has_tag(card, tag) {
            card.tags.push(tag.to_owned());
        }
    } else {
        card.tags.retain(|existing| existing != tag);
    }
}

/// One game's write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// The config has the tag as asked, whether it was written or already so.
    Done,
    /// The environment was gone by the time its write ran.
    Gone,
}

#[derive(Debug, Clone)]
pub(crate) struct Saved {
    pub(crate) change: Change,
    /// Whether the batch added the tag, or set the flag.
    pub(crate) value: bool,
    /// Each game in the batch, with its `added` when the change was made.
    pub(crate) games: Vec<(String, Option<Timestamp>)>,
    /// One result per game, in order, or why the worker gave none.
    pub(crate) results: Result<Vec<Result<Outcome, String>>, String>,
}

/// Makes `change` on each of `games` in turn. Each stands alone, so one that
/// fails leaves the rest changed, as with `wineo tag`.
pub(crate) fn save(
    paths: &Paths,
    watch: &rescan::Watch,
    games: &[(String, Option<Timestamp>)],
    change: &Change,
    value: bool,
) -> Vec<Result<Outcome, String>> {
    let results = games
        .iter()
        .map(|(name, added)| {
            let written = match change {
                Change::Tag(tag) => write_tag(paths, name, tag, value),
                Change::Flag(flag) => write_flag(paths, name, *added, *flag, value),
            };
            match written {
                Ok(outcome) => Ok(outcome),
                // A parse error spans lines, which a notice can't.
                Err(error) => Err(wineo_core::error::one_line(
                    crate::sort_control::error_text(paths, &error),
                )),
            }
        })
        .collect();
    if matches!(change, Change::Flag(_)) {
        // Whether the writes saved or not, so the rescan's cache never holds
        // a value that wasn't saved.
        watch.refresh_library(paths);
    }
    results
}

fn write_tag(paths: &Paths, name: &str, tag: &str, value: bool) -> Result<Outcome, Error> {
    let change = if value {
        TagChange::Add(tag)
    } else {
        TagChange::Remove(tag)
    };
    match settings::change_tags(paths, name, change) {
        Ok(_) => Ok(Outcome::Done),
        Err(Error::UnknownEnvironment(_)) => Ok(Outcome::Gone),
        Err(error) => Err(error),
    }
}

/// The `added` check happens in core under the library lock, so a game
/// removed or recreated since the window read it is `Gone`.
fn write_flag(
    paths: &Paths,
    name: &str,
    added: Option<Timestamp>,
    flag: Flag,
    value: bool,
) -> Result<Outcome, Error> {
    let guarded = match flag {
        Flag::Favorite => library::set_favorite_for(paths, name, added, value),
        Flag::Hidden => library::set_hidden_for(paths, name, added, value),
    }?;
    Ok(match guarded {
        Guarded::Set(_) => Outcome::Done,
        Guarded::Gone => Outcome::Gone,
    })
}

/// The notice for a batch where `failed` of `total` games failed to `verb`,
/// naming them and the first reason.
fn failure_notice(verb: &str, failed: &[(String, String)], total: usize) -> String {
    let reason = &failed[0].1;
    if total == 1 {
        return format!("Couldn't {verb} {}: {reason}", failed[0].0);
    }
    let mut titles: Vec<&str> = failed
        .iter()
        .take(NAMED_FAILURES)
        .map(|(title, _)| title.as_str())
        .collect();
    let more = failed.len().saturating_sub(NAMED_FAILURES);
    let more = format!("and {more} more");
    if failed.len() > NAMED_FAILURES {
        titles.push(&more);
    }
    format!(
        "Couldn't {verb} {} of {total} ({}): {reason}",
        failed.len(),
        titles.join(", ")
    )
}

impl App {
    /// The marked games' cards, in name order.
    pub(crate) fn marked_cards(&self) -> Vec<&Card> {
        self.marked
            .iter()
            .filter_map(|name| self.cards.iter().find(|card| &card.name == name))
            .collect()
    }

    /// Adds `tag` to every marked game that lacks it, or removes it from
    /// every one that has it. The cards change at once and the writes run
    /// off the UI thread.
    pub(crate) fn tag_marked(&mut self, tag: &str, value: bool) -> Task<Message> {
        self.change_marked(&Change::Tag(tag.to_owned()), value)
    }

    /// Sets `flag` on every marked game that doesn't have it as `value`.
    pub(crate) fn flag_marked(&mut self, flag: Flag, value: bool) -> Task<Message> {
        self.change_marked(&Change::Flag(flag), value)
    }

    fn change_marked(&mut self, change: &Change, value: bool) -> Task<Message> {
        let targets: Vec<(String, Option<Timestamp>)> = self
            .marked_cards()
            .into_iter()
            .filter(|card| change.present(card) != value)
            .map(|card| (card.name.clone(), card.added))
            .collect();
        if targets.is_empty() {
            return Task::none();
        }
        // Same as a single card's toggle: the banner already says the file
        // can't be used, so nothing is written and nothing changes.
        if matches!(change, Change::Flag(_))
            && let Some(problem) = &self.library_problem
        {
            let refused = Error::LibraryUnreadable {
                newer_version: problem.newer_version,
                source: problem.message().into(),
            };
            let failed: Vec<(String, String)> = self
                .marked_cards()
                .into_iter()
                .filter(|card| change.present(card) != value)
                .map(|card| (card.display_title.clone(), refused.to_string()))
                .collect();
            let notice = failure_notice(change.verb(value), &failed, failed.len());
            self.flag_notice(notice);
            return Task::none();
        }
        let mut start = Vec::new();
        for (name, added) in &targets {
            let key = (name.clone(), change.clone());
            if let Some(pending) = self.tag_writes.0.get_mut(&key)
                && pending.added == *added
            {
                pending.next = (value != pending.writing).then_some(value);
                continue;
            }
            // Any other write of this name was for an environment that's
            // gone, so its reply no longer matches and is ignored.
            self.tag_writes.0.insert(
                key,
                Pending {
                    added: *added,
                    writing: value,
                    next: None,
                },
            );
            start.push((name.clone(), *added));
        }
        let names: Vec<String> = targets.into_iter().map(|(name, _)| name).collect();
        let place = self.place_after_hide(&names, change, value);
        let shown = self.show_change(&names, change, value);
        let selection = place.map_or_else(Task::none, |place| self.select_in_place(place));
        let write = self.write_batch(start, change, value);
        Task::batch([shown, selection, write])
    }

    /// Where the selected card's successor sits once the batch is shown, when
    /// the batch hides the selected card out of view. It's the cursor's slot
    /// now less the batch's slots before it, so the first card after it that
    /// stays comes next, as with hiding one card.
    fn place_after_hide(&self, names: &[String], change: &Change, value: bool) -> Option<usize> {
        if *change != Change::Flag(Flag::Hidden) || !value || self.filters.show_hidden {
            return None;
        }
        let selected = self.selected.as_deref()?;
        if !names.iter().any(|name| name == selected) {
            return None;
        }
        let places = self.grid_places();
        let at = self.selected_index(&places)?;
        let before = places.ids[..at]
            .iter()
            .filter(|slot| names.iter().any(|name| name == slot.name()))
            .count();
        Some(at - before)
    }

    fn write_batch(
        &mut self,
        games: Vec<(String, Option<Timestamp>)>,
        change: &Change,
        value: bool,
    ) -> Task<Message> {
        if games.is_empty() {
            return Task::none();
        }
        let paths = self.paths.clone();
        let watch = self.shown.clone();
        let work = games.clone();
        let change = change.clone();
        let reply = change.clone();
        crate::task::blocking_value(
            move || save(&paths, &watch, &work, &change, value),
            move |results| {
                Message::Tags(crate::action_bar::Event::Saved(Saved {
                    change: reply,
                    value,
                    games,
                    results,
                }))
            },
        )
    }

    pub(crate) fn batch_saved(&mut self, saved: Saved) -> Task<Message> {
        let Saved {
            change,
            value,
            games,
            results,
        } = saved;
        let worker = results.as_ref().err().cloned();
        let results = results.unwrap_or_else(|error| vec![Err(error); games.len()]);
        let mut failed = Vec::new();
        let mut back = Vec::new();
        let mut again = Vec::new();
        let mut gone = false;
        for ((name, added), result) in games.iter().zip(results) {
            let key = (name.clone(), change.clone());
            // A write for an environment that has since left the library.
            if self
                .tag_writes
                .0
                .get(&key)
                .is_none_or(|pending| pending.added != *added)
            {
                continue;
            }
            let next = self
                .tag_writes
                .0
                .remove(&key)
                .and_then(|pending| pending.next);
            match result {
                Ok(Outcome::Done) => {
                    // Only while the card is still the one it was queued for.
                    if next.is_some()
                        && self
                            .cards
                            .iter()
                            .any(|card| &card.name == name && card.added == *added)
                    {
                        again.push((name.clone(), *added));
                    }
                }
                Ok(Outcome::Gone) => gone = true,
                Err(error) => {
                    // A change queued behind a failed write is dropped too:
                    // the card goes back to what the file has.
                    let title = self
                        .cards
                        .iter()
                        .find(|card| &card.name == name)
                        .map_or(name.as_str(), |card| card.display_title.as_str());
                    failed.push((title.to_owned(), error));
                    back.push(name.clone());
                }
            }
        }
        if self.exiting {
            return Task::none();
        }
        let reads = self.outdate_reads();
        if !failed.is_empty() {
            let notice = failure_notice(change.verb(value), &failed, games.len());
            self.flag_notice(notice);
        }
        let back = self.show_change(&back, &change, !value);
        // A worker that died may have written some before it did, so the
        // file says which.
        let reload = if worker.is_some() || gone {
            self.reload_library_once()
        } else {
            Task::none()
        };
        // Everything queued behind this batch wants the opposite of it.
        for (name, added) in &again {
            self.tag_writes.0.insert(
                (name.clone(), change.clone()),
                Pending {
                    added: *added,
                    writing: !value,
                    next: None,
                },
            );
        }
        let follow = self.write_batch(again, &change, !value);
        Task::batch([reads, back, reload, follow])
    }

    /// Shows `change` as `value` on the cards `names`, as `set_flag` does a
    /// flag. The order is rebuilt, which also drops the marks of games that
    /// a hide takes out of view.
    fn show_change(&mut self, names: &[String], change: &Change, value: bool) -> Task<Message> {
        let before = self.shown_order();
        let mut changed = false;
        for card in self
            .cards
            .iter_mut()
            .filter(|card| names.contains(&card.name))
        {
            if change.present(card) != value {
                change.set(card, value);
                changed = true;
            }
        }
        if changed {
            self.cards_edited(&before)
        } else {
            Task::none()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Change, failure_notice};
    use crate::library_actions::Flag;

    fn failed(titles: &[&str]) -> Vec<(String, String)> {
        titles
            .iter()
            .map(|title| ((*title).to_owned(), format!("{title} broke")))
            .collect()
    }

    #[test]
    fn a_failure_notice_names_the_games_and_the_first_reason() {
        assert_eq!(
            failure_notice("tag", &failed(&["Hades"]), 1),
            "Couldn't tag Hades: Hades broke"
        );
        assert_eq!(
            failure_notice("tag", &failed(&["Hades", "Celeste"]), 3),
            "Couldn't tag 2 of 3 (Hades, Celeste): Hades broke"
        );
        assert_eq!(
            failure_notice("untag", &failed(&["A", "B", "C", "D", "E"]), 6),
            "Couldn't untag 5 of 6 (A, B, C, and 2 more): A broke"
        );
    }

    #[test]
    fn each_change_has_its_own_verbs() {
        let verbs = |change: Change| (change.verb(true), change.verb(false));
        assert_eq!(verbs(Change::Tag("rpg".into())), ("tag", "untag"));
        assert_eq!(
            verbs(Change::Flag(Flag::Favorite)),
            ("favorite", "unfavorite")
        );
        assert_eq!(verbs(Change::Flag(Flag::Hidden)), ("hide", "show"));
        assert_eq!(
            failure_notice(Change::Flag(Flag::Hidden).verb(false), &failed(&["A"]), 1),
            "Couldn't show A: A broke"
        );
    }
}
