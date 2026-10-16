pub(crate) mod artwork;
mod clone;
mod frame;
mod gog;
mod gog_install;
mod gog_posters;
mod gog_update;
mod heroic;
mod import_grid;
pub(crate) mod new;
mod posters;
mod prefix;
mod steam;

pub(crate) use gog_install::Offer;
pub(crate) use gog_update::LOGGED_OUT;

use iced::widget::{column, container, scrollable, text};
use iced::{Element, Fill, Task, keyboard};
use wineo_core::paths::Paths;

use crate::theme::Palette;
use crate::ui::{ScrollStyle, list_group, list_row, list_row_text, secondary_button};

/// One Add choice: its name, with what it does underneath.
fn choice<'a>(
    title: &'static str,
    detail: &'static str,
    palette: Palette,
) -> iced::widget::Button<'a, Event> {
    list_row(
        column![
            list_row_text(title),
            text(detail).size(12).color(palette.muted)
        ]
        .spacing(2),
        palette,
    )
}

#[derive(Debug, Clone)]
pub(crate) enum Action {
    Choose,
    Library {
        name: Option<String>,
        launch: bool,
    },
    Settings {
        name: String,
        launch: bool,
    },
    /// Leaves the dialog for Settings, at the Steam Web API key row.
    SteamKey,
}

#[derive(Debug, Clone)]
pub(crate) enum Event {
    New,
    Prefix,
    Heroic,
    Steam,
    Gog,
    Back,
    AfterSubmit,
    Create(Box<new::Event>),
    Import(Box<prefix::Event>),
    Copy(Box<clone::Event>),
    HeroicMessage(Box<heroic::Event>),
    SteamMessage(Box<steam::Event>),
    GogMessage(Box<gog::Event>),
    UpdateMessage(Box<gog_update::Event>),
}

/// Boxes each page's own event into the flow's, so `.map(Event::from)` replaces
/// a closure per page.
macro_rules! wrap_events {
    ($($event:ty => $variant:ident),* $(,)?) => {$(
        impl From<$event> for Event {
            fn from(event: $event) -> Self {
                Self::$variant(Box::new(event))
            }
        }
    )*};
}

wrap_events! {
    new::Event => Create,
    prefix::Event => Import,
    clone::Event => Copy,
    heroic::Event => HeroicMessage,
    steam::Event => SteamMessage,
    gog::Event => GogMessage,
    gog_update::Event => UpdateMessage,
}

/// Runs `$body` with `$page` bound to whichever page is open. The pages' event
/// types differ, so one match has to be spelled out per call; this spells it
/// once. `Choose` has no page and takes `$choose`.
macro_rules! on_page {
    ($flow:expr, $page:ident => $body:expr, Choose => $choose:expr) => {
        match $flow {
            Self::New($page) => $body,
            Self::Prefix($page) => $body,
            Self::Clone($page) => $body,
            Self::Heroic($page) => $body,
            Self::Steam($page) => $body,
            Self::Gog($page) => $body,
            Self::GogUpdate($page) => $body,
            Self::Choose(_) => $choose,
        }
    };
}

pub(crate) enum Page {
    Choose(Paths),
    New(Box<new::Page>),
    Prefix(Box<prefix::Page>),
    Clone(Box<clone::Page>),
    Heroic(Box<heroic::Page>),
    Steam(Box<steam::Page>),
    Gog(Box<gog::Page>),
    GogUpdate(Box<gog_update::Page>),
}

impl Page {
    pub(crate) fn choose(paths: Paths) -> Self {
        Self::Choose(paths)
    }

    pub(crate) fn clone(paths: Paths, name: String) -> (Self, Task<Event>) {
        let (page, task) = clone::Page::new(paths, name);
        (Self::Clone(Box::new(page)), task.map(Event::from))
    }

    /// Updates `name`'s GOG game, or finishes its interrupted update.
    pub(crate) fn gog_update(paths: Paths, name: String) -> (Self, Task<Event>) {
        let (page, task) = gog_update::Page::new(paths, name);
        (Self::GogUpdate(Box::new(page)), task.map(Event::from))
    }

    /// Changes `name`'s GOG game as `options` ask, from the Version picker:
    /// a branch or build, a repair, or a move to the other track.
    pub(crate) fn gog_update_with(
        paths: Paths,
        name: String,
        options: wineo_core::gog::update::Options,
    ) -> (Self, Task<Event>) {
        let (page, task) = gog_update::Page::with_options(paths, name, options);
        (Self::GogUpdate(Box::new(page)), task.map(Event::from))
    }

    /// Finishes the interrupted install of the game `offer` names, on the
    /// install page.
    pub(crate) fn gog_finish_install(paths: Paths, offer: Offer) -> (Self, Task<Event>) {
        let (page, task) = gog::Page::finish_install(paths, offer);
        (Self::Gog(Box::new(page)), task.map(Event::from))
    }

    /// The environment a GOG update just finished for, once.
    pub(crate) fn take_updated(&mut self) -> Option<String> {
        match self {
            Self::GogUpdate(page) => page.take_updated(),
            _ => None,
        }
    }

    /// The page a Choose event opens, with its first task.
    fn open(paths: &Paths, event: &Event) -> Option<(Self, Task<Event>)> {
        let paths = paths.clone();
        Some(match event {
            Event::New => {
                let (page, task) = new::Page::new(paths);
                (Self::New(Box::new(page)), task.map(Event::from))
            }
            Event::Prefix => {
                let (page, task) = prefix::Page::new(paths);
                (Self::Prefix(Box::new(page)), task.map(Event::from))
            }
            Event::Heroic => {
                let (page, task) = heroic::Page::new(paths);
                (Self::Heroic(Box::new(page)), task.map(Event::from))
            }
            Event::Steam => {
                let (page, task) = steam::Page::new(paths);
                (Self::Steam(Box::new(page)), task.map(Event::from))
            }
            Event::Gog => {
                let (page, task) = gog::Page::new(paths);
                (Self::Gog(Box::new(page)), task.map(Event::from))
            }
            _ => return None,
        })
    }

    pub(crate) fn update(&mut self, event: Event, key_available: bool) -> Task<Event> {
        if let Self::Choose(paths) = self
            && let Some((next, task)) = Self::open(paths, &event)
        {
            *self = next;
            return task;
        }
        match (self, event) {
            (page, Event::Back) => page.back(),
            (Self::New(page), Event::Create(event)) => page
                .update_with_key_availability(*event, key_available)
                .map(Event::from),
            (Self::Prefix(page), Event::Import(event)) => page
                .update_with_key_availability(*event, key_available)
                .map(Event::from),
            (Self::Clone(page), Event::Copy(event)) => page.update(*event).map(Event::from),
            (Self::Heroic(page), Event::HeroicMessage(event)) => page
                .update_with_key_availability(*event, key_available)
                .map(Event::from),
            (Self::Steam(page), Event::SteamMessage(event)) => page.update(*event).map(Event::from),
            (Self::Gog(page), Event::GogMessage(event)) => page.update(*event).map(Event::from),
            (Self::GogUpdate(page), Event::UpdateMessage(event)) => {
                page.update(*event).map(Event::from)
            }
            _ => Task::none(),
        }
    }

    /// Background work that should show the spinner.
    #[cfg(test)]
    pub(crate) fn spinning(&self) -> bool {
        match self {
            Self::Heroic(page) => page.waiting(),
            Self::Steam(page) => page.spinning(),
            Self::Gog(page) => page.waiting(),
            Self::GogUpdate(page) => page.waiting(),
            _ => false,
        }
    }

    #[cfg(test)]
    pub(crate) fn art_fetching(&self) -> bool {
        matches!(self, Self::New(page) if page.art_fetching())
    }

    /// `width` is the dialog's width, for pages that lay out by it.
    pub(crate) fn view(
        &self,
        palette: Palette,
        width: f32,
        key_available: bool,
    ) -> Element<'_, Event> {
        match self {
            Self::New(page) => page
                .view_with_key_availability(palette, key_available)
                .map(Event::from),
            Self::Prefix(page) => page
                .view_with_key_availability(palette, key_available)
                .map(Event::from),
            Self::Clone(page) => page.view(palette).map(Event::from),
            Self::Heroic(page) => page
                .view_with_key_availability(palette, width, key_available)
                .map(Event::from),
            Self::Steam(page) => page.view(palette, width).map(Event::from),
            Self::Gog(page) => page.view_at(palette, width).map(Event::from),
            Self::GogUpdate(page) => page.view(palette).map(Event::from),
            Self::Choose(_) => container(
                column![
                    secondary_button("Back", palette).on_press(Event::Back),
                    text("Add to your library").size(28),
                    scrollable(list_group(
                        [
                            choice(
                                "1 - New environment",
                                "An installer, an existing executable, or an empty environment",
                                palette,
                            )
                            .on_press(Event::New),
                            choice(
                                "2 - Import prefix",
                                "Copy or register an existing Wine prefix or Proton compatdata",
                                palette,
                            )
                            .on_press(Event::Prefix),
                            choice(
                                "3 - Import from Heroic",
                                "Manually added Windows and native Linux games",
                                palette,
                            )
                            .on_press(Event::Heroic),
                            choice(
                                "4 - Import from Steam",
                                "Installed and owned Steam games",
                                palette,
                            )
                            .on_press(Event::Steam),
                            choice(
                                "5 - Import from GOG",
                                "Install the games you own, or move Heroic's",
                                palette,
                            )
                            .on_press(Event::Gog),
                        ],
                        palette,
                    ),)
                    .wineo_style(palette),
                ]
                .spacing(16),
            )
            .padding(24)
            .width(Fill)
            .height(Fill)
            .into(),
        }
    }

    /// A key the page handles itself, before the flow's shared keys. `width`
    /// is the dialog's width, as passed to `view`.
    pub(crate) fn key(
        &mut self,
        key: &keyboard::Key,
        modifiers: keyboard::Modifiers,
        width: f32,
    ) -> Option<Task<Event>> {
        match self {
            Self::Heroic(page) => page.key(key, modifiers, width).map(|t| t.map(Event::from)),
            Self::Steam(page) => page.key(key, modifiers, width).map(|t| t.map(Event::from)),
            Self::Gog(page) => page.key(key, modifiers, width).map(|t| t.map(Event::from)),
            _ => None,
        }
    }

    pub(crate) fn busy(&self) -> bool {
        on_page!(self, page => page.busy(), Choose => false)
    }

    pub(crate) fn cancel(&mut self, force: bool) {
        on_page!(self, page => page.cancel(force), Choose => {})
    }

    pub(crate) fn submit(&mut self) -> Task<Event> {
        on_page!(self, page => page.submit().map(Event::from), Choose => self.update(Event::New, false))
    }

    pub(crate) fn back(&mut self) -> Task<Event> {
        on_page!(self, page => page.back().map(Event::from), Choose => Task::none())
    }

    pub(crate) fn take_action(&mut self) -> Option<Action> {
        on_page!(self, page => page.take_action(), Choose => None)
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use super::*;

    #[test]
    fn add_choices_fit_or_scroll_in_small_and_narrow_dialogs() {
        use crate::layout_probe::probe;
        let root = tempfile::tempdir().unwrap();
        let paths =
            Paths::from_lookup(|key| (key == "HOME").then(|| OsString::from(root.path()))).unwrap();
        let page = Page::choose(paths);
        assert!(matches!(page, Page::Choose(_)));
        // The smallest dialog the window allows, a small one, and the widest.
        for (width, height) in [(280.0, 240.0), (360.0, 280.0), (680.0, 560.0)] {
            let inset = iced::Rectangle::new(
                iced::Point::new(24.0, 24.0),
                iced::Size::new(width - 48.0, height - 48.0),
            );
            let mut view = page.view(Palette::DARK, width, false);
            let probe = probe(&mut view, iced::Size::new(width, height));
            assert_eq!(probe.scrollables.len(), 1, "one scrolling region");
            let spilled = probe.outside(inset);
            assert!(spilled.is_empty(), "{width}x{height}: {spilled:?}");
        }
    }
}
