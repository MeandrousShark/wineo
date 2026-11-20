//! Runs tasks in tests without waiting on the wall clock, so a loaded
//! machine can't cut one short and drop what it sends.

use std::time::Duration;

use iced::Task;
use iced::futures::{Stream, StreamExt};
use iced_runtime::Action;

/// How far ahead of the paused clock a run lets a task's timers fire before
/// it calls the task done. Only paused time passes, so this costs nothing.
const QUIET: Duration = Duration::from_millis(500);

/// A runtime on tokio's paused clock, for `next_action`.
///
/// The clock moves only when nothing can run, and then jumps to the next
/// timer, so a task's sleeps pass at once however busy the machine is.
/// Blocking pool work holds the clock still while it runs, so a file write
/// or a config lock wait finishes before any timeout can fire, however long
/// it takes. Work on a thread of its own doesn't hold it; the app sends all
/// of its blocking work through `crate::task`, which uses the pool.
pub(crate) fn paused_runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .start_paused(true)
        .build()
        .unwrap()
}

/// The next action `stream` sends, or None once it has ended or has only
/// waits left that no timer under `QUIET` ends, like an answer from a
/// window the test doesn't have. Await it in a `paused_runtime`.
pub(crate) async fn next_action<S: Stream + Unpin>(stream: &mut S) -> Option<S::Item> {
    tokio::time::timeout(QUIET, stream.next())
        .await
        .ok()
        .flatten()
}

/// Runs `task` the way the runtime would, handing each action to `take`,
/// until `next_action` says it's done.
pub(crate) fn each_action<M>(task: Task<M>, mut take: impl FnMut(Action<M>)) {
    let Some(mut stream) = iced_runtime::task::into_stream(task) else {
        return;
    };
    paused_runtime().block_on(async {
        while let Some(action) = next_action(&mut stream).await {
            take(action);
        }
    });
}

/// Takes every action `stream` has ready now, and stops when it has none
/// and nothing woke it: a wakeup's sleep, or work on another thread.
/// Nothing here waits on the clock or runs the runtime, so a task's sleeps
/// never fire and work it hands to the blocking pool is only seen if it's
/// already done; use `each_action` for those. A pad press's key path is all
/// widget operations and their answers, so a loaded machine can't cut it
/// short. A batch's stream says Pending now and then just to take turns,
/// waking itself first, so a wake means poll again.
pub(crate) fn each_ready<M, S>(
    runtime: &tokio::runtime::Runtime,
    stream: &mut S,
    mut take: impl FnMut(Action<M>),
) where
    S: Stream<Item = Action<M>> + Unpin,
{
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::task::{Context, Poll, Wake, Waker};

    struct Woken(AtomicBool);
    impl Wake for Woken {
        fn wake(self: Arc<Self>) {
            self.0.store(true, Ordering::SeqCst);
        }
        fn wake_by_ref(self: &Arc<Self>) {
            self.0.store(true, Ordering::SeqCst);
        }
    }
    // Sleeps in the stream need the runtime's timer to exist, not to run.
    let _runtime = runtime.enter();
    let woken = Arc::new(Woken(AtomicBool::new(false)));
    let waker = Waker::from(woken.clone());
    let mut context = Context::from_waker(&waker);
    // A stream that only ever wakes itself would never finish.
    for _ in 0..100_000 {
        woken.0.store(false, Ordering::SeqCst);
        match stream.poll_next_unpin(&mut context) {
            Poll::Ready(Some(action)) => take(action),
            Poll::Ready(None) => return,
            Poll::Pending if woken.0.load(Ordering::SeqCst) => {}
            Poll::Pending => return,
        }
    }
    panic!("the task kept waking itself without finishing");
}
