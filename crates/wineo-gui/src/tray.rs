//! The tray icon (plan 0063): a StatusNotifierItem with the common actions,
//! served through ksni, and the light poll that keeps its Stop rows current
//! while the full rescan is paused.

use std::collections::{BTreeSet, HashMap};
use std::hash::{Hash, Hasher};

use iced::futures::{SinkExt, Stream};
use jiff::Timestamp;
use ksni::TrayMethods;
use tokio::sync::{mpsc, watch};
use unicode_segmentation::UnicodeSegmentation;
use wineo_core::paths::Paths;
use wineo_core::report::{self, OverviewFilter};
use wineo_core::runtime::State as RunState;

use crate::Message;
use crate::panel::PanelError;

/// How many recently played things the menu offers to start.
pub(crate) const PLAY_ROWS: usize = 3;
/// How many Stop rows show before "More in wineo..." takes over.
pub(crate) const STOP_ROWS: usize = 8;
/// Longer names are cut to this many characters, ellipsis included.
const NAME_LIMIT: usize = 40;

pub(crate) const ID: &str = "wineo-gui";
pub(crate) const TITLE: &str = "wineo";
pub(crate) const OPEN: &str = "Open wineo";
pub(crate) const ADD: &str = "Add to library...";
pub(crate) const MORE: &str = "More in wineo...";
pub(crate) const QUIT: &str = "Quit";
/// The notice when a blocked tray Play or Stop finds a page or flow open.
pub(crate) const PLAY_BLOCKED: &str = "Close this page to play.";
pub(crate) const STOP_BLOCKED: &str = "Close this page to stop it.";

/// What the tray's menu lists between its fixed rows.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Menu {
    pub(crate) play: Vec<Row>,
    pub(crate) stop: Vec<Row>,
    /// More is running than the Stop rows show.
    pub(crate) more: bool,
}

/// One Play or Stop row: the environment it acts on and its menu label.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Row {
    pub(crate) name: String,
    pub(crate) label: String,
}

impl Row {
    pub(crate) fn play(name: &str, title: &str) -> Self {
        Self {
            name: name.to_owned(),
            label: format!("Play {}", menu_text(title)),
        }
    }

    pub(crate) fn stop(name: &str, title: &str) -> Self {
        Self {
            name: name.to_owned(),
            label: format!("Stop {}", menu_text(title)),
        }
    }
}

/// One entry of the menu as a host shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Entry {
    Item(String, Event),
    Separator,
}

impl Menu {
    /// Every entry top to bottom, with a separator only between groups that
    /// have rows.
    pub(crate) fn entries(&self) -> Vec<Entry> {
        let mut entries = vec![
            Entry::Item(OPEN.to_owned(), Event::Open),
            Entry::Item(ADD.to_owned(), Event::Add),
            Entry::Separator,
        ];
        if !self.play.is_empty() {
            for row in &self.play {
                entries.push(Entry::Item(
                    row.label.clone(),
                    Event::Play(row.name.clone()),
                ));
            }
            entries.push(Entry::Separator);
        }
        if !self.stop.is_empty() {
            for row in &self.stop {
                entries.push(Entry::Item(
                    row.label.clone(),
                    Event::Stop(row.name.clone()),
                ));
            }
            if self.more {
                entries.push(Entry::Item(MORE.to_owned(), Event::Open));
            }
            entries.push(Entry::Separator);
        }
        entries.push(Entry::Item(QUIT.to_owned(), Event::Quit));
        entries
    }

    /// The labels as a host draws them, separators as "-".
    #[cfg(test)]
    pub(crate) fn labels(&self) -> Vec<String> {
        self.entries()
            .into_iter()
            .map(|entry| match entry {
                Entry::Item(label, _) => label,
                Entry::Separator => "-".to_owned(),
            })
            .collect()
    }
}

/// A name cut to `NAME_LIMIT` characters with an ellipsis, with `_` doubled
/// since dbusmenu reads a single one as a mnemonic.
pub(crate) fn menu_text(name: &str) -> String {
    let characters: Vec<&str> = name.graphemes(true).collect();
    let cut = if characters.len() > NAME_LIMIT {
        let mut cut = characters[..NAME_LIMIT - 1].concat().trim_end().to_owned();
        cut.push('…');
        cut
    } else {
        name.to_owned()
    };
    cut.replace('_', "__")
}

/// A click in the tray, or the host coming or going.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Event {
    /// The icon's left click, Open wineo, and More in wineo...
    Open,
    Add,
    Play(String),
    Stop(String),
    Quit,
    /// Whether a tray host is on the bus to show the icon.
    Available(bool),
}

/// The app's side of the tray.
#[derive(Debug)]
pub(crate) struct Tray {
    /// The service runs at all. Only `run()` turns it on, so tests never
    /// reach D-Bus.
    pub(crate) service: bool,
    /// A tray host shows the icon, as of the watcher's last word.
    pub(crate) available: bool,
    /// The light poll's running set, which stands in for the cards' states
    /// while the full rescan is paused. Cleared whenever cards load.
    pub(crate) polled: Option<BTreeSet<String>>,
    /// Last play times read when the poll saw something stop, newer than
    /// the cards' until the next load.
    pub(crate) played: HashMap<String, Timestamp>,
    /// The stop the tray asked for, whose error has to outlive the panel
    /// not showing it.
    pub(crate) stopping: Option<String>,
    /// A tray stop's error, kept until the window shows.
    pub(crate) held_error: Option<(String, PanelError)>,
    /// What the window is hidden for with Hide to the tray: it comes back
    /// once the poll sees all of them stopped.
    pub(crate) hidden_for: BTreeSet<String>,
    /// Bumped by each hide, which restarts the poll so its first reading
    /// always arrives, and tells a reading from before the hide apart.
    pub(crate) poll_generation: u64,
    pub(crate) menu: watch::Sender<Menu>,
}

impl Default for Tray {
    fn default() -> Self {
        Self {
            service: false,
            available: false,
            polled: None,
            played: HashMap::new(),
            stopping: None,
            held_error: None,
            hidden_for: BTreeSet::new(),
            poll_generation: 0,
            menu: watch::channel(Menu::default()).0,
        }
    }
}

/// The tray service's subscription. Its identity never changes, so it runs
/// once for as long as it's subscribed.
pub(crate) struct Service {
    pub(crate) menu: watch::Receiver<Menu>,
}

impl Hash for Service {
    fn hash<H: Hasher>(&self, state: &mut H) {
        "tray".hash(state);
    }
}

pub(crate) fn service(service: &Service) -> impl Stream<Item = Message> + use<> {
    use iced::futures::StreamExt;
    events(service.menu.clone(), crate::tray_icons()).map(Message::Tray)
}

/// Serves the tray until the stream is dropped, with the menu `menu` holds,
/// and yields its clicks. The first event says whether a host is there.
pub(crate) fn events(
    mut menu: watch::Receiver<Menu>,
    icons: Vec<ksni::Icon>,
) -> impl Stream<Item = Event> + use<> {
    iced::stream::channel(16, async move |mut output| {
        let (send, mut clicks) = mpsc::unbounded_channel();
        let item = Item {
            menu: menu.borrow_and_update().clone(),
            send,
            icons,
        };
        // A bar that starts after the app still gets the icon, through
        // `watcher_online`.
        let Ok(handle) = item.assume_sni_available(true).spawn().await else {
            // No session bus: carry on without a tray.
            let _ = output.send(Event::Available(false)).await;
            std::future::pending::<()>().await;
            return;
        };
        let _shutdown = Shutdown(handle.clone());
        // Sent before anything `spawn` queued, so a watcher that was already
        // missing reads as offline after this.
        if output.send(Event::Available(true)).await.is_err() {
            return;
        }
        loop {
            tokio::select! {
                changed = menu.changed() => {
                    if changed.is_err() {
                        return;
                    }
                    let next = menu.borrow_and_update().clone();
                    handle.update(|item| item.menu = next).await;
                }
                Some(event) = clicks.recv() => {
                    if output.send(event).await.is_err() {
                        return;
                    }
                }
            }
        }
    })
}

/// Takes the icon down when the stream goes, say when the setting is
/// turned off.
struct Shutdown(ksni::Handle<Item>);

impl Drop for Shutdown {
    fn drop(&mut self) {
        drop(self.0.shutdown());
    }
}

struct Item {
    menu: Menu,
    send: mpsc::UnboundedSender<Event>,
    icons: Vec<ksni::Icon>,
}

impl Item {
    fn send(&self, event: Event) {
        let _ = self.send.send(event);
    }
}

impl ksni::Tray for Item {
    fn id(&self) -> String {
        ID.to_owned()
    }

    fn title(&self) -> String {
        TITLE.to_owned()
    }

    fn icon_name(&self) -> String {
        ID.to_owned()
    }

    // For a build that isn't installed, whose icon name finds nothing.
    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        self.icons.clone()
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        self.send(Event::Open);
    }

    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        self.menu
            .entries()
            .into_iter()
            .map(|entry| match entry {
                Entry::Item(label, event) => ksni::menu::StandardItem {
                    label,
                    activate: Box::new(move |item: &mut Self| item.send(event.clone())),
                    ..Default::default()
                }
                .into(),
                Entry::Separator => ksni::MenuItem::Separator,
            })
            .collect()
    }

    fn watcher_online(&self) {
        self.send(Event::Available(true));
    }

    fn watcher_offline(&self, _reason: ksni::OfflineReason) -> bool {
        self.send(Event::Available(false));
        // Keep serving, so the icon comes back with the bar.
        true
    }
}

/// The light poll's subscription. Like the rescan, it sleeps first. A new
/// generation is a new subscription, which starts over and sends its first
/// reading even if nothing changed.
pub(crate) struct Poll {
    pub(crate) paths: Paths,
    pub(crate) generation: u64,
}

impl Hash for Poll {
    fn hash<H: Hasher>(&self, state: &mut H) {
        "tray-poll".hash(state);
        self.generation.hash(state);
    }
}

pub(crate) fn poll(poll: &Poll) -> impl Stream<Item = Message> + use<> {
    let paths = poll.paths.clone();
    let generation = poll.generation;
    iced::stream::channel(1, async move |mut output| {
        let mut sent: Option<BTreeSet<String>> = None;
        loop {
            tokio::time::sleep(crate::rescan::INTERVAL).await;
            let paths = paths.clone();
            let Ok(Some(running)) = tokio::task::spawn_blocking(move || running(&paths)).await
            else {
                continue;
            };
            // Only a change reaches the app, which redraws on every message.
            if sent.as_ref() == Some(&running) {
                continue;
            }
            sent = Some(running.clone());
            if output
                .send(Message::TrayPolled(generation, running))
                .await
                .is_err()
            {
                return;
            }
        }
    })
}

/// What's running, by the same status the cards show: one config load and
/// lock check per environment and at most one Steam process scan. None when
/// the library can't be read.
pub(crate) fn running(paths: &Paths) -> Option<BTreeSet<String>> {
    let overview = report::overview(
        paths,
        OverviewFilter {
            tags: None,
            running_only: true,
        },
    )
    .ok()?;
    Some(
        overview
            .environments
            .into_iter()
            .filter(|environment| {
                matches!(
                    environment.status.state,
                    RunState::Starting | RunState::Running | RunState::SteamRunning
                )
            })
            .map(|environment| environment.name)
            .collect(),
    )
}

/// `name`'s last play time from its stats, or None if it has none or they
/// can't be read.
pub(crate) fn last_played(paths: &Paths, name: &str) -> Option<Timestamp> {
    wineo_core::stats::summary(
        paths,
        name,
        Timestamp::now(),
        &wineo_core::time::LocalZone::new(),
    )
    .ok()
    .and_then(|(summary, _)| summary.last_played)
}

#[cfg(test)]
pub(crate) mod tests;
