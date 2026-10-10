pub(crate) mod artwork;
mod clone;
mod heroic;
pub(crate) mod new;
mod prefix;
mod steam;

use iced::widget::{button, column, container, scrollable, text};
use iced::{Element, Fill, Task, keyboard};
use wineo_core::paths::Paths;

use crate::theme::Palette;

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
    Back,
    AfterSubmit,
    Create(Box<new::Event>),
    Import(Box<prefix::Event>),
    Copy(Box<clone::Event>),
    HeroicMessage(Box<heroic::Event>),
    SteamMessage(Box<steam::Event>),
}

pub(crate) enum Page {
    Choose(Paths),
    New(Box<new::Page>),
    Prefix(Box<prefix::Page>),
    Clone(Box<clone::Page>),
    Heroic(Box<heroic::Page>),
    Steam(Box<steam::Page>),
}

impl Page {
    pub(crate) fn choose(paths: Paths) -> Self {
        Self::Choose(paths)
    }

    pub(crate) fn clone(paths: Paths, name: String) -> (Self, Task<Event>) {
        let (page, task) = clone::Page::new(paths, name);
        (
            Self::Clone(Box::new(page)),
            task.map(|event| Event::Copy(Box::new(event))),
        )
    }

    pub(crate) fn update(&mut self, event: Event) -> Task<Event> {
        self.update_with_key_availability(event, false)
    }

    pub(crate) fn update_with_key_availability(
        &mut self,
        event: Event,
        key_available: bool,
    ) -> Task<Event> {
        match (self, event) {
            (page @ Self::Choose(_), Event::New) => {
                let Self::Choose(paths) = page else {
                    unreachable!()
                };
                let (next, task) = new::Page::new(paths.clone());
                *page = Self::New(Box::new(next));
                task.map(|event| Event::Create(Box::new(event)))
            }
            (page @ Self::Choose(_), Event::Prefix) => {
                let Self::Choose(paths) = page else {
                    unreachable!()
                };
                let (next, task) = prefix::Page::new(paths.clone());
                *page = Self::Prefix(Box::new(next));
                task.map(|event| Event::Import(Box::new(event)))
            }
            (page @ Self::Choose(_), Event::Heroic) => {
                let Self::Choose(paths) = page else {
                    unreachable!()
                };
                let (next, task) = heroic::Page::new(paths.clone());
                *page = Self::Heroic(Box::new(next));
                task.map(|event| Event::HeroicMessage(Box::new(event)))
            }
            (page @ Self::Choose(_), Event::Steam) => {
                let Self::Choose(paths) = page else {
                    unreachable!()
                };
                let (next, task) = steam::Page::new(paths.clone());
                *page = Self::Steam(Box::new(next));
                task.map(|event| Event::SteamMessage(Box::new(event)))
            }
            (Self::New(page), Event::Create(event)) => page
                .update_with_key_availability(*event, key_available)
                .map(|event| Event::Create(Box::new(event))),
            (Self::Prefix(page), Event::Import(event)) => page
                .update_with_key_availability(*event, key_available)
                .map(|event| Event::Import(Box::new(event))),
            (Self::Clone(page), Event::Copy(event)) => page
                .update(*event)
                .map(|event| Event::Copy(Box::new(event))),
            (Self::Heroic(page), Event::HeroicMessage(event)) => page
                .update_with_key_availability(*event, key_available)
                .map(|event| Event::HeroicMessage(Box::new(event))),
            (Self::Steam(page), Event::SteamMessage(event)) => page
                .update(*event)
                .map(|event| Event::SteamMessage(Box::new(event))),
            (page, Event::Back) => page.back(),
            _ => Task::none(),
        }
    }

    /// Background work that should show the spinner.
    pub(crate) fn spinning(&self) -> bool {
        match self {
            Self::Heroic(page) => page.waiting(),
            Self::Steam(page) => page.spinning(),
            _ => false,
        }
    }

    #[cfg(test)]
    pub(crate) fn art_fetching(&self) -> bool {
        matches!(self, Self::New(page) if page.art_fetching())
    }

    /// `width` is the dialog's width, for pages that lay out by it, and
    /// `spin` is the spinner's angle.
    pub(crate) fn view_with_key_availability(
        &self,
        palette: Palette,
        width: f32,
        spin: f32,
        key_available: bool,
    ) -> Element<'_, Event> {
        match self {
            Self::New(page) => page
                .view_with_key_availability(palette, key_available)
                .map(|event| Event::Create(Box::new(event))),
            Self::Prefix(page) => page
                .view_with_key_availability(palette, key_available)
                .map(|event| Event::Import(Box::new(event))),
            Self::Clone(page) => page.view(palette).map(|event| Event::Copy(Box::new(event))),
            Self::Heroic(page) => page
                .view_with_key_availability(palette, width, spin, key_available)
                .map(|event| Event::HeroicMessage(Box::new(event))),
            Self::Steam(page) => page
                .view(palette, width, spin)
                .map(|event| Event::SteamMessage(Box::new(event))),
            Self::Choose(_) => container(
                column![
                    button("Back").on_press(Event::Back),
                    text("Add to your library").size(28),
                    scrollable(
                        column![
                            button("1 - New environment")
                                .on_press(Event::New)
                                .padding(12),
                            text("An installer, an existing executable, or an empty environment"),
                            button("2 - Import prefix")
                                .on_press(Event::Prefix)
                                .padding(12),
                            text("Copy or register an existing Wine prefix or Proton compatdata"),
                            button("3 - Import from Heroic")
                                .on_press(Event::Heroic)
                                .padding(12),
                            text("Manually added Windows and native Linux games"),
                            button("4 - Import from Steam")
                                .on_press(Event::Steam)
                                .padding(12),
                            text("Installed and owned Steam games"),
                        ]
                        .spacing(16),
                    ),
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
            Self::Heroic(page) => page
                .key(key, modifiers, width)
                .map(|task| task.map(|event| Event::HeroicMessage(Box::new(event)))),
            Self::Steam(page) => page
                .key(key, modifiers, width)
                .map(|task| task.map(|event| Event::SteamMessage(Box::new(event)))),
            _ => None,
        }
    }

    pub(crate) fn busy(&self) -> bool {
        match self {
            Self::New(page) => page.busy(),
            Self::Prefix(page) => page.busy(),
            Self::Clone(page) => page.busy(),
            Self::Heroic(page) => page.busy(),
            Self::Steam(page) => page.busy(),
            Self::Choose(_) => false,
        }
    }

    pub(crate) fn cancel(&mut self, force: bool) {
        match self {
            Self::New(page) => page.cancel(force),
            Self::Prefix(page) => page.cancel(force),
            Self::Clone(page) => page.cancel(force),
            Self::Heroic(page) => page.cancel(force),
            Self::Steam(page) => page.cancel(force),
            Self::Choose(_) => {}
        }
    }

    pub(crate) fn submit(&mut self) -> Task<Event> {
        match self {
            Self::New(page) => page.submit().map(|event| Event::Create(Box::new(event))),
            Self::Prefix(page) => page.submit().map(|event| Event::Import(Box::new(event))),
            Self::Clone(page) => page.submit().map(|event| Event::Copy(Box::new(event))),
            Self::Heroic(page) => page
                .submit()
                .map(|event| Event::HeroicMessage(Box::new(event))),
            Self::Steam(page) => page
                .submit()
                .map(|event| Event::SteamMessage(Box::new(event))),
            Self::Choose(_) => self.update(Event::New),
        }
    }

    pub(crate) fn back(&mut self) -> Task<Event> {
        match self {
            Self::New(page) => page.back().map(|event| Event::Create(Box::new(event))),
            Self::Prefix(page) => page.back().map(|event| Event::Import(Box::new(event))),
            Self::Clone(page) => page.back().map(|event| Event::Copy(Box::new(event))),
            Self::Heroic(page) => page
                .back()
                .map(|event| Event::HeroicMessage(Box::new(event))),
            Self::Steam(page) => page
                .back()
                .map(|event| Event::SteamMessage(Box::new(event))),
            Self::Choose(_) => Task::none(),
        }
    }

    pub(crate) fn take_action(&mut self) -> Option<Action> {
        match self {
            Self::New(page) => page.take_action(),
            Self::Prefix(page) => page.take_action(),
            Self::Clone(page) => page.take_action(),
            Self::Heroic(page) => page.take_action(),
            Self::Steam(page) => page.take_action(),
            Self::Choose(_) => None,
        }
    }
}
