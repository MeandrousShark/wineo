//! The one way the app hands blocking work to the runtime's blocking pool.

use iced::Task;
use tokio::task::JoinError;

/// Runs `work` on the blocking pool. `join_error` turns a panicked or
/// aborted worker into the caller's error type.
pub(crate) async fn join<T, E>(
    work: impl FnOnce() -> Result<T, E> + Send + 'static,
    join_error: impl FnOnce(JoinError) -> E,
) -> Result<T, E>
where
    T: Send + 'static,
    E: Send + 'static,
{
    tokio::task::spawn_blocking(work)
        .await
        .unwrap_or_else(|error| Err(join_error(error)))
}

/// `join` for the usual `Result<T, String>` work, with the join error's text
/// as the message.
pub(crate) async fn join_string<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    join(work, |error| error.to_string()).await
}

/// `blocking` for work that can't fail itself; only a panicked or aborted
/// worker reaches the message as an error.
pub(crate) fn blocking_value<T, M>(
    work: impl FnOnce() -> T + Send + 'static,
    map: impl FnOnce(Result<T, String>) -> M + Send + 'static,
) -> Task<M>
where
    T: Send + 'static,
    M: Send + 'static,
{
    blocking(move || Ok(work()), map)
}

/// A task that runs `work` on the blocking pool and maps its result to a
/// message.
pub(crate) fn blocking<T, M>(
    work: impl FnOnce() -> Result<T, String> + Send + 'static,
    map: impl FnOnce(Result<T, String>) -> M + Send + 'static,
) -> Task<M>
where
    T: Send + 'static,
    M: Send + 'static,
{
    Task::perform(join_string(work), map)
}
