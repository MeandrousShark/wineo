pub(crate) mod artwork;
mod clone;
mod gog;
mod gog_install;
mod gog_posters;
mod gog_update;
mod heroic;
pub(crate) mod new;
mod prefix;
mod steam;

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
        (
            Self::Clone(Box::new(page)),
            task.map(|event| Event::Copy(Box::new(event))),
        )
    }

    /// Updates `name`'s GOG game, or finishes its interrupted update.
    pub(crate) fn gog_update(paths: Paths, name: String) -> (Self, Task<Event>) {
        let (page, task) = gog_update::Page::new(paths, name);
        (
            Self::GogUpdate(Box::new(page)),
            task.map(|event| Event::UpdateMessage(Box::new(event))),
        )
    }

    /// The environment a GOG update just finished for, once.
    pub(crate) fn take_updated(&mut self) -> Option<String> {
        match self {
            Self::GogUpdate(page) => page.take_updated(),
            _ => None,
        }
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
            (page @ Self::Choose(_), Event::Gog) => {
                let Self::Choose(paths) = page else {
                    unreachable!()
                };
                let (next, task) = gog::Page::new(paths.clone());
                *page = Self::Gog(Box::new(next));
                task.map(|event| Event::GogMessage(Box::new(event)))
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
            (Self::Gog(page), Event::GogMessage(event)) => page
                .update(*event)
                .map(|event| Event::GogMessage(Box::new(event))),
            (Self::GogUpdate(page), Event::UpdateMessage(event)) => page
                .update(*event)
                .map(|event| Event::UpdateMessage(Box::new(event))),
            (page, Event::Back) => page.back(),
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
    pub(crate) fn view_with_key_availability(
        &self,
        palette: Palette,
        width: f32,
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
                .view_with_key_availability(palette, width, key_available)
                .map(|event| Event::HeroicMessage(Box::new(event))),
            Self::Steam(page) => page
                .view(palette, width)
                .map(|event| Event::SteamMessage(Box::new(event))),
            Self::Gog(page) => page
                .view_at(palette, width)
                .map(|event| Event::GogMessage(Box::new(event))),
            Self::GogUpdate(page) => page
                .view(palette)
                .map(|event| Event::UpdateMessage(Box::new(event))),
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
            Self::Heroic(page) => page
                .key(key, modifiers, width)
                .map(|task| task.map(|event| Event::HeroicMessage(Box::new(event)))),
            Self::Steam(page) => page
                .key(key, modifiers, width)
                .map(|task| task.map(|event| Event::SteamMessage(Box::new(event)))),
            Self::Gog(page) => page
                .key(key, modifiers, width)
                .map(|task| task.map(|event| Event::GogMessage(Box::new(event)))),
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
            Self::Gog(page) => page.busy(),
            Self::GogUpdate(page) => page.busy(),
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
            Self::Gog(page) => page.cancel(force),
            Self::GogUpdate(page) => page.cancel(force),
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
            Self::Gog(page) => page
                .submit()
                .map(|event| Event::GogMessage(Box::new(event))),
            Self::GogUpdate(page) => page
                .submit()
                .map(|event| Event::UpdateMessage(Box::new(event))),
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
            Self::Gog(page) => page.back().map(|event| Event::GogMessage(Box::new(event))),
            Self::GogUpdate(page) => page
                .back()
                .map(|event| Event::UpdateMessage(Box::new(event))),
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
            Self::Gog(page) => page.take_action(),
            Self::GogUpdate(page) => page.take_action(),
            Self::Choose(_) => None,
        }
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
            let mut view = page.view_with_key_availability(Palette::DARK, width, false);
            let probe = probe(&mut view, iced::Size::new(width, height));
            assert_eq!(probe.scrollables.len(), 1, "one scrolling region");
            let spilled = probe.outside(inset);
            assert!(spilled.is_empty(), "{width}x{height}: {spilled:?}");
        }
    }
}
