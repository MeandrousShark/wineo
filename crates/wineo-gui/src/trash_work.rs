use iced::Task;
use wineo_core::cancel::Token;
use wineo_core::paths::Paths;
use wineo_core::trash::{self, Phase, PruneReview};
use wineo_core::validate::{self, Fix};

use crate::Message;
use crate::trash_page::{Event, Outcome, Selection, Snapshot};

pub(crate) fn read(paths: &Paths) -> Result<Snapshot, String> {
    let mut entries =
        trash::list(paths).map_err(|error| paths.contract_text(&format!("{error:#}")))?;
    let mut pending = trash::pending_entries(paths)
        .map_err(|error| paths.contract_text(&format!("{error:#}")))?;
    entries.sort_by(|a, b| {
        b.removed_at
            .parse::<jiff::Timestamp>()
            .ok()
            .cmp(&a.removed_at.parse::<jiff::Timestamp>().ok())
            .then_with(|| b.id.cmp(&a.id))
    });
    // Legacy interrupted entries can lack a host marker but still need recovery.
    for entry in &entries {
        if entry.phase != Phase::Trashed && !pending.iter().any(|item| item.id == entry.id) {
            pending.push(trash::PendingEntry {
                id: entry.id.clone(),
                name: entry.name.clone(),
                finished: false,
            });
        }
    }
    Ok(Snapshot { entries, pending })
}

pub(crate) fn load(generation: u64, paths: Paths) -> Task<Message> {
    work(
        move || read(&paths),
        move |result| Event::Loaded(generation, result),
    )
}

pub(crate) fn review(generation: u64, paths: Paths, selection: Selection) -> Task<Message> {
    work(
        move || review_selection(&paths, &selection),
        move |result| Event::Reviewed(generation, result),
    )
}

pub(crate) fn review_selection(
    paths: &Paths,
    selection: &Selection,
) -> Result<PruneReview, String> {
    let selector = match selection {
        Selection::Id(id) => trash::Selector::Id(id),
        Selection::OlderThan(days) => trash::Selector::OlderThan(*days),
        Selection::All => trash::Selector::All,
    };
    trash::review_prune(paths, selector).map_err(|error| paths.contract_text(&format!("{error:#}")))
}

pub(crate) fn restore(generation: u64, paths: Paths, id: String) -> Task<Message> {
    work(
        move || {
            trash::restore(&paths, &id).map_err(|error| paths.contract_text(&format!("{error:#}")))
        },
        move |result| Event::Restored(generation, result),
    )
}

pub(crate) fn recover(generation: u64, paths: Paths, id: String) -> Task<Message> {
    work(
        move || recover_entry(&paths, id),
        move |result| Event::Recovered(generation, result),
    )
}

pub(crate) fn recover_entry(paths: &Paths, id: String) -> Result<(), String> {
    validate::apply(
        paths,
        &Fix::RecoverTrash { id },
        &Token::new(),
        wineo_core::winetricks::EnsureOutput::Stderr,
        |_| {},
        |_, _| anyhow::bail!("trash recovery does not download components"),
    )
    .map(|_| ())
    .map_err(|error| paths.contract_text(&format!("{error:#}")))
}

pub(crate) fn prune(generation: u64, paths: Paths, review: PruneReview) -> Task<Message> {
    work(
        move || Ok(prune_selection(&paths, &review)),
        move |result| Event::Pruned(generation, result),
    )
}

pub(crate) fn prune_selection(paths: &Paths, review: &PruneReview) -> Vec<Outcome> {
    trash::prune_reviewed(paths, review)
        .into_iter()
        .zip(&review.entries)
        .map(|((id, result), entry)| Outcome {
            name: entry.name.clone(),
            id,
            result: result.map_err(|error| paths.contract_text(&format!("{error:#}"))),
        })
        .collect()
}

fn work<T: Send + 'static>(
    run: impl FnOnce() -> Result<T, String> + Send + 'static,
    done: impl Fn(Result<T, String>) -> Event + Send + 'static,
) -> Task<Message> {
    Task::perform(
        async move {
            tokio::task::spawn_blocking(run)
                .await
                .map_err(|error| error.to_string())?
        },
        move |result| Message::Trash(done(result)),
    )
}
