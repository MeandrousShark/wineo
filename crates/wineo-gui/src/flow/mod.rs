pub(crate) mod artwork;
mod clone;
mod heroic;
mod new;
mod prefix;

use iced::widget::{button, column, container, text};
use iced::{Element, Fill, Task};
use wineo_core::paths::Paths;

use crate::theme::Palette;

#[derive(Debug, Clone)]
pub(crate) enum Action {
    Choose,
    Library { name: Option<String>, launch: bool },
    Settings { name: String, launch: bool },
}

#[derive(Debug, Clone)]
pub(crate) enum Event {
    New,
    Prefix,
    Heroic,
    Back,
    AfterSubmit,
    Create(Box<new::Event>),
    Import(Box<prefix::Event>),
    Copy(Box<clone::Event>),
    HeroicMessage(Box<heroic::Event>),
}

pub(crate) enum Page {
    Choose(Paths),
    New(Box<new::Page>),
    Prefix(Box<prefix::Page>),
    Clone(Box<clone::Page>),
    Heroic(Box<heroic::Page>),
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
            (Self::New(page), Event::Create(event)) => page
                .update(*event)
                .map(|event| Event::Create(Box::new(event))),
            (Self::Prefix(page), Event::Import(event)) => page
                .update(*event)
                .map(|event| Event::Import(Box::new(event))),
            (Self::Clone(page), Event::Copy(event)) => page
                .update(*event)
                .map(|event| Event::Copy(Box::new(event))),
            (Self::Heroic(page), Event::HeroicMessage(event)) => page
                .update(*event)
                .map(|event| Event::HeroicMessage(Box::new(event))),
            (page, Event::Back) => page.back(),
            _ => Task::none(),
        }
    }

    pub(crate) fn view(&self, palette: Palette) -> Element<'_, Event> {
        match self {
            Self::New(page) => page
                .view(palette)
                .map(|event| Event::Create(Box::new(event))),
            Self::Prefix(page) => page
                .view(palette)
                .map(|event| Event::Import(Box::new(event))),
            Self::Clone(page) => page.view(palette).map(|event| Event::Copy(Box::new(event))),
            Self::Heroic(page) => page
                .view(palette)
                .map(|event| Event::HeroicMessage(Box::new(event))),
            Self::Choose(_) => container(
                column![
                    button("Back").on_press(Event::Back),
                    text("Add to your library").size(28),
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
                ]
                .spacing(16),
            )
            .padding(24)
            .width(Fill)
            .height(Fill)
            .into(),
        }
    }

    pub(crate) fn busy(&self) -> bool {
        match self {
            Self::New(page) => page.busy(),
            Self::Prefix(page) => page.busy(),
            Self::Clone(page) => page.busy(),
            Self::Heroic(page) => page.busy(),
            Self::Choose(_) => false,
        }
    }

    pub(crate) fn cancel(&mut self, force: bool) {
        match self {
            Self::New(page) => page.cancel(force),
            Self::Prefix(page) => page.cancel(force),
            Self::Clone(page) => page.cancel(force),
            Self::Heroic(page) => page.cancel(force),
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
            Self::Choose(_) => Task::none(),
        }
    }

    pub(crate) fn take_action(&mut self) -> Option<Action> {
        match self {
            Self::New(page) => page.take_action(),
            Self::Prefix(page) => page.take_action(),
            Self::Clone(page) => page.take_action(),
            Self::Heroic(page) => page.take_action(),
            Self::Choose(_) => None,
        }
    }
}
