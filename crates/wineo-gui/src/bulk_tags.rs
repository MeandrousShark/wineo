//! Adding a tag to every marked game, or removing it from them: the cards
//! change at once, each environment's config is written off the UI thread
//! through core's `change_tags`, and the ones that fail go back.

use std::collections::BTreeMap;

use iced::Task;
use jiff::Timestamp;
use wineo_core::error::Error;
use wineo_core::paths::Paths;
use wineo_core::settings::{self, TagChange};

use crate::library::Card;
use crate::{App, Message};

/// How many failed games a notice names before it counts the rest.
const NAMED_FAILURES: usize = 3;

/// The writes in flight, one per game and tag. A change made meanwhile is
/// written when the one in flight returns, so the last state wins.
#[derive(Debug, Default)]
pub(crate) struct Writes(BTreeMap<(String, String), Pending>);

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
        self.0.get(&(name.to_owned(), tag.to_owned())).copied()
    }

    /// Matches the writes to cards just read. A write whose environment is
    /// gone, or was removed and made again, is forgotten: its queued change
    /// never starts and its reply is ignored. The rest put the window's
    /// choice over the cards, so a read made before a write landed doesn't
    /// undo it on screen meanwhile.
    pub(crate) fn reconcile(&mut self, cards: &mut [Card]) {
        self.0.retain(|(name, tag), pending| {
            match cards
                .iter_mut()
                .find(|card| &card.name == name && card.added == pending.added)
            {
                Some(card) => {
                    set_tag(card, tag, pending.wanted());
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
    pub(crate) tag: String,
    /// Whether the batch added the tag.
    pub(crate) value: bool,
    /// Each game in the batch, with its `added` when the change was made.
    pub(crate) games: Vec<(String, Option<Timestamp>)>,
    /// One result per game, in order, or why the worker gave none.
    pub(crate) results: Result<Vec<Result<Outcome, String>>, String>,
}

/// Changes `tag` on each of `names` in turn. Each stands alone, so one that
/// fails leaves the rest changed, as with `wineo tag`.
pub(crate) fn save(
    paths: &Paths,
    names: &[String],
    tag: &str,
    value: bool,
) -> Vec<Result<Outcome, String>> {
    let change = if value {
        TagChange::Add(tag)
    } else {
        TagChange::Remove(tag)
    };
    names
        .iter()
        .map(|name| match settings::change_tags(paths, name, change) {
            Ok(_) => Ok(Outcome::Done),
            Err(Error::UnknownEnvironment(_)) => Ok(Outcome::Gone),
            // A parse error spans lines, which a notice can't.
            Err(error) => Err(wineo_core::error::one_line(
                crate::sort_control::error_text(paths, &error),
            )),
        })
        .collect()
}

/// The notice for a batch where `failed` of `total` games failed, naming
/// them and the first reason.
fn failure_notice(value: bool, failed: &[(String, String)], total: usize) -> String {
    let verb = if value { "tag" } else { "untag" };
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
        let targets: Vec<(String, Option<Timestamp>)> = self
            .marked_cards()
            .into_iter()
            .filter(|card| has_tag(card, tag) != value)
            .map(|card| (card.name.clone(), card.added))
            .collect();
        if targets.is_empty() {
            return Task::none();
        }
        let mut start = Vec::new();
        for (name, added) in &targets {
            let key = (name.clone(), tag.to_owned());
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
        let shown = self.show_tag(&names, tag, value);
        let write = self.write_tags(start, tag, value);
        Task::batch([shown, write])
    }

    fn write_tags(
        &mut self,
        games: Vec<(String, Option<Timestamp>)>,
        tag: &str,
        value: bool,
    ) -> Task<Message> {
        if games.is_empty() {
            return Task::none();
        }
        let paths = self.paths.clone();
        let names: Vec<String> = games.iter().map(|(name, _)| name.clone()).collect();
        let tag = tag.to_owned();
        let reply = tag.clone();
        crate::task::blocking_value(
            move || save(&paths, &names, &tag, value),
            move |results| {
                Message::Tags(crate::action_bar::Event::Saved(Saved {
                    tag: reply,
                    value,
                    games,
                    results,
                }))
            },
        )
    }

    pub(crate) fn tags_saved(&mut self, saved: Saved) -> Task<Message> {
        let Saved {
            tag,
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
            let key = (name.clone(), tag.clone());
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
            let notice = failure_notice(value, &failed, games.len());
            self.flag_notice(notice);
        }
        // A worker that died may have written some before it did, so the
        // file says which.
        let shown = if worker.is_some() || gone {
            self.reload_library_once()
        } else {
            self.show_tag(&back, &tag, !value)
        };
        // Everything queued behind this batch wants the opposite of it.
        for (name, added) in &again {
            self.tag_writes.0.insert(
                (name.clone(), tag.clone()),
                Pending {
                    added: *added,
                    writing: !value,
                    next: None,
                },
            );
        }
        let follow = self.write_tags(again, &tag, !value);
        Task::batch([reads, shown, follow])
    }

    /// Shows `tag` on or off the cards `names`, as `set_flag` does a flag.
    fn show_tag(&mut self, names: &[String], tag: &str, value: bool) -> Task<Message> {
        let before = self.shown_order();
        let mut changed = false;
        for card in self
            .cards
            .iter_mut()
            .filter(|card| names.contains(&card.name))
        {
            if has_tag(card, tag) != value {
                set_tag(card, tag, value);
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
    use super::failure_notice;

    fn failed(titles: &[&str]) -> Vec<(String, String)> {
        titles
            .iter()
            .map(|title| ((*title).to_owned(), format!("{title} broke")))
            .collect()
    }

    #[test]
    fn a_failure_notice_names_the_games_and_the_first_reason() {
        assert_eq!(
            failure_notice(true, &failed(&["Hades"]), 1),
            "Couldn't tag Hades: Hades broke"
        );
        assert_eq!(
            failure_notice(true, &failed(&["Hades", "Celeste"]), 3),
            "Couldn't tag 2 of 3 (Hades, Celeste): Hades broke"
        );
        assert_eq!(
            failure_notice(false, &failed(&["A", "B", "C", "D", "E"]), 6),
            "Couldn't untag 5 of 6 (A, B, C, and 2 more): A broke"
        );
    }
}
