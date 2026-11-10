//! One app at a time (plan 0063): the first launch for a root owns a session
//! bus name and serves `Show` on it, and a later launch for the same root
//! calls that and exits instead of opening a second app.

use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use iced::futures::{SinkExt, Stream};
use sha2::{Digest, Sha256};
use tokio::sync::{mpsc, oneshot, watch};
use wineo_core::paths::{Paths, Root};

use crate::Message;

/// The name for the default root. Any other root appends `_` and its hash.
pub(crate) const NAME: &str = "io.github.MeandrousShark.wineo.Gui";
pub(crate) const PATH: &str = "/io/github/MeandrousShark/wineo/Gui";
pub(crate) const INTERFACE: &str = "io.github.MeandrousShark.wineo.Gui";

/// How long connecting to the bus may take before the app starts without it.
const CONNECT_WAIT: Duration = Duration::from_secs(5);
/// How long the first `Show` call may go unanswered, say by an owner that's
/// stuck. A launch waits at most this plus `RELEASE_WAIT`.
const SHOW_WAIT: Duration = Duration::from_secs(2);
/// After a `Show` that failed, how long to keep trying for a name an exiting
/// owner is about to let go before starting without it.
const RELEASE_WAIT: Duration = Duration::from_secs(3);
const RETRY: Duration = Duration::from_millis(50);

/// The bus name for the root `paths` points at, so two roots stay two apps.
pub(crate) fn bus_name(paths: &Paths) -> String {
    let (tag, root): (&[u8], &Path) = match paths.root() {
        Root::Default => return NAME.to_owned(),
        Root::Portable(root) => (b"portable\0", root),
        Root::Data(root) => (b"data\0", root),
    };
    // A fixed algorithm, so the name stays the same across builds. The tag
    // keeps a portable folder from sharing a name with the same path as a
    // data dir.
    let mut hash = Sha256::new();
    hash.update(tag);
    hash.update(root.as_os_str().as_bytes());
    let digest = hash.finalize();
    let short: String = digest[..6]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!("{NAME}_{short}")
}

/// What a launch found on the bus.
#[derive(Debug)]
pub(crate) enum Claim {
    /// This launch owns the name and serves `Show`.
    Owner(Instance),
    /// Another app for this root took the `Show`, so this launch exits.
    Shown,
    /// No bus, or an owner that never answered: the app runs on its own.
    Alone,
}

/// The owner's side: the `Show` calls it gets and the way out.
#[derive(Debug, Clone)]
pub(crate) struct Instance {
    shows: watch::Receiver<u64>,
    leaving: Arc<AtomicBool>,
    release: mpsc::UnboundedSender<oneshot::Sender<()>>,
}

impl Instance {
    /// From now on a `Show` is refused, so a launch that calls it waits for
    /// the name instead of trusting an app on its way out.
    pub(crate) fn refuse_shows(&self) {
        self.leaving.store(true, Ordering::SeqCst);
    }

    /// Refuses shows and lets the name go. The future ends once the bus has
    /// taken it back, or at once if the bus thread is gone.
    pub(crate) fn leave(&self) -> impl Future<Output = ()> + use<> {
        self.refuse_shows();
        let (done, released) = oneshot::channel();
        let sent = self.release.send(done).is_ok();
        async move {
            if sent {
                let _ = released.await;
            }
        }
    }

    pub(crate) fn shows(&self) -> Shows {
        Shows(self.shows.clone())
    }
}

/// Claims the name for `paths`' root on the session bus. A second launch
/// shows the first app's window and gets `Shown`. Blocks until the bus has
/// answered; the bus side then lives on a thread of its own, so a runtime
/// stuck on the way out can't hold the name.
pub(crate) fn claim(paths: &Paths) -> Claim {
    let name = bus_name(paths);
    let (decided, decision) = std::sync::mpsc::channel();
    let thread_name = name.clone();
    let spawned = std::thread::Builder::new()
        .name("single-instance".to_owned())
        .spawn(move || {
            let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            else {
                let _ = decided.send(Claim::Alone);
                return;
            };
            runtime.block_on(serve(thread_name, decided));
        });
    if spawned.is_err() {
        return Claim::Alone;
    }
    decision.recv().unwrap_or(Claim::Alone)
}

struct Server {
    shows: watch::Sender<u64>,
    leaving: Arc<AtomicBool>,
}

#[zbus::interface(name = "io.github.MeandrousShark.wineo.Gui")]
impl Server {
    /// Shows the window, as the user asking for it.
    async fn show(&self) -> zbus::fdo::Result<()> {
        if self.leaving.load(Ordering::SeqCst) {
            return Err(zbus::fdo::Error::Failed("wineo is quitting".to_owned()));
        }
        self.shows.send_modify(|count| *count += 1);
        Ok(())
    }
}

async fn serve(name: String, decided: std::sync::mpsc::Sender<Claim>) {
    let (shows, shown) = watch::channel(0);
    let leaving = Arc::new(AtomicBool::new(false));
    let server = Server {
        shows,
        leaving: leaving.clone(),
    };
    let Some(connection) = connect(server).await else {
        let _ = decided.send(Claim::Alone);
        return;
    };
    match take_name(&connection, &name).await {
        Taken::Owner => {}
        Taken::Shown => {
            let _ = decided.send(Claim::Shown);
            return;
        }
        Taken::Alone => {
            let _ = decided.send(Claim::Alone);
            return;
        }
    }
    let (release, mut releases) = mpsc::unbounded_channel();
    let instance = Instance {
        shows: shown,
        leaving,
        release,
    };
    if decided.send(Claim::Owner(instance)).is_err() {
        return;
    }
    // Every Instance gone means the app is too, and dropping the connection
    // gives the name back as well.
    if let Some(done) = releases.recv().await {
        let _ = connection.release_name(name.as_str()).await;
        let _ = done.send(());
    }
}

/// A connection that already serves `Show`, so a launch that finds the name
/// taken never calls an owner that can't answer yet.
async fn connect(server: Server) -> Option<zbus::Connection> {
    let build = async {
        zbus::connection::Builder::session()
            .ok()?
            .serve_at(PATH, server)
            .ok()?
            .build()
            .await
            .ok()
    };
    tokio::time::timeout(CONNECT_WAIT, build)
        .await
        .ok()
        .flatten()
}

enum Taken {
    Owner,
    Shown,
    Alone,
}

async fn take_name(connection: &zbus::Connection, name: &str) -> Taken {
    let Ok(bus) = zbus::fdo::DBusProxy::new(connection).await else {
        return Taken::Alone;
    };
    // Never ReplaceExisting or AllowReplacement: the first app keeps it.
    let flags = zbus::fdo::RequestNameFlags::DoNotQueue.into();
    let request = async || match connection.request_name_with_flags(name, flags).await {
        Ok(_) => Some(true),
        Err(zbus::Error::NameTaken) => Some(false),
        Err(_) => None,
    };
    match request().await {
        Some(true) => return Taken::Owner,
        Some(false) => {}
        None => return Taken::Alone,
    }
    let mut shown_to = owner(&bus, name).await;
    if let Some(owner) = &shown_to
        && call_show(connection, owner, SHOW_WAIT).await
    {
        return Taken::Shown;
    }
    // The owner is leaving or stuck. One that's leaving lets go soon, so the
    // name is polled; only a new owner, a launch that won meanwhile, is
    // asked to show again, with what's left of the wait.
    let deadline = tokio::time::Instant::now() + RELEASE_WAIT;
    loop {
        tokio::time::sleep(RETRY).await;
        match request().await {
            Some(true) => return Taken::Owner,
            Some(false) => {}
            None => return Taken::Alone,
        }
        let now = tokio::time::Instant::now();
        if now >= deadline {
            return Taken::Alone;
        }
        let current = owner(&bus, name).await;
        if current.is_some() && current != shown_to {
            shown_to = current;
            if let Some(owner) = &shown_to
                && call_show(connection, owner, deadline - now).await
            {
                return Taken::Shown;
            }
        }
    }
}

/// The unique name that owns `name` now, if any.
async fn owner(bus: &zbus::fdo::DBusProxy<'_>, name: &str) -> Option<String> {
    let name = zbus::names::BusName::try_from(name).ok()?;
    let owner = bus.get_name_owner(name).await.ok()?;
    Some(owner.to_string())
}

/// Calls `Show` on the unique name `owner`, so the answer comes from the
/// owner just looked up and not one that took the name since.
async fn call_show(connection: &zbus::Connection, owner: &str, wait: Duration) -> bool {
    let call = connection.call_method(Some(owner), PATH, Some(INTERFACE), "Show", &());
    matches!(tokio::time::timeout(wait, call).await, Ok(Ok(_)))
}

/// The `Show` calls as a subscription. Its identity never changes, so it
/// runs once for as long as it's subscribed.
pub(crate) struct Shows(watch::Receiver<u64>);

impl std::hash::Hash for Shows {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        "single-instance".hash(state);
    }
}

pub(crate) fn shows(shows: &Shows) -> impl Stream<Item = Message> + use<> {
    let mut receiver = shows.0.clone();
    iced::stream::channel(1, async move |mut output| {
        while receiver.changed().await.is_ok() {
            if output.send(Message::Show).await.is_err() {
                return;
            }
        }
        std::future::pending::<()>().await;
    })
}

#[cfg(test)]
mod tests;
