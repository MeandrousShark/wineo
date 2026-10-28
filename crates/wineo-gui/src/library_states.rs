//! What the Library says instead of showing an empty grid, and the banner
//! for a `library.toml` that can't be used. An empty result must never look
//! like an empty or deleted library.

use iced::widget::{Space, column, container, row, text};
use iced::{Center, Element, Fill, Padding};
use wineo_core::library::Problem;

use crate::filter_control::Event;
use crate::library_menu::icon;
use crate::theme::Palette;
use crate::ui;
use crate::{App, Message, focus_button};

pub(crate) const SHOW_HIDDEN_ID: &str = "library-empty-show-hidden";
pub(crate) const CLEAR_ID: &str = "library-empty-clear";
pub(crate) const CHECKUP_ID: &str = "library-banner-checkup";
pub(crate) const TRASH_CHECKUP_ID: &str = "trash-banner-checkup";

const SEARCH_ICON: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><circle cx="10.5" cy="10.5" r="6.5" fill="none" stroke="#000" stroke-width="1.8"/><path d="M15.5 15.5L20 20" fill="none" stroke="#000" stroke-width="1.8" stroke-linecap="round"/></svg>"##;
const EYE_ICON: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><path d="M2.5 12S6 5.5 12 5.5 21.5 12 21.5 12 18 18.5 12 18.5 2.5 12 2.5 12z" fill="none" stroke="#000" stroke-width="1.8" stroke-linejoin="round"/><circle cx="12" cy="12" r="3" fill="none" stroke="#000" stroke-width="1.8"/><path d="M4 20L20 4" fill="none" stroke="#000" stroke-width="1.8" stroke-linecap="round"/></svg>"##;
const SURFACE_WIDTH: f32 = 440.0;

/// Why nothing is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Empty {
    /// No environments at all.
    Library,
    /// Every environment is hidden, with no search or filters on.
    AllHidden { hidden: usize },
    /// The search or filters exclude everything shown.
    NoMatches { hidden_matching: usize },
}

impl App {
    /// Which empty state applies, from the counts the last rebuild took.
    pub(crate) fn empty_state(&self) -> Option<Empty> {
        // Before the first load answers, an empty library isn't known yet.
        if !self.loaded && self.cards.is_empty() {
            return None;
        }
        if self.cards.is_empty() {
            return Some(Empty::Library);
        }
        let counts = self.order.counts(self.order_inputs());
        if counts.shown > 0 {
            None
        } else if !self.filters.show_hidden
            && !self.filters_clearable()
            && counts.hidden == self.cards.len()
        {
            Some(Empty::AllHidden {
                hidden: counts.hidden,
            })
        } else {
            Some(Empty::NoMatches {
                hidden_matching: counts.hidden_matching,
            })
        }
    }

    /// The everything-hidden or no-matches surface over the empty grid, or
    /// nothing. The empty library keeps its own message above the grid.
    pub(crate) fn empty_state_view(&self) -> Element<'_, Message> {
        let palette = self.palette;
        // It waits for the last leaving card to fade, so the two never overlap.
        let empty = if self.grid_motion.leaving().is_empty() {
            self.empty_state()
        } else {
            None
        };
        let content: Element<'_, Message> = match empty {
            Some(Empty::AllHidden { hidden }) => column![
                icon(EYE_ICON, palette.muted, 32.0),
                ui::section_heading("Everything here is hidden", palette),
                text(all_hidden_note(hidden)).size(14).color(palette.muted),
                show_hidden(ui::accent_control(
                    "Show hidden",
                    palette.primary,
                    true,
                    Message::Filter(Event::ShowHidden),
                )),
            ]
            .spacing(8)
            .into(),
            Some(Empty::NoMatches { hidden_matching }) => {
                let mut content = column![
                    icon(SEARCH_ICON, palette.muted, 32.0),
                    ui::section_heading("Nothing matches your filters", palette),
                    text("Try another search or clear your filters.")
                        .size(14)
                        .color(palette.muted),
                    focus_button::control(
                        ui::accent_control(
                            "Clear filters",
                            palette.primary,
                            self.filters_clearable(),
                            Message::Filter(Event::Clear),
                        )
                        .into(),
                        self.filters_clearable(),
                        CLEAR_ID,
                        Message::Filter(Event::Clear),
                    ),
                ]
                .spacing(8);
                if hidden_matching > 0 {
                    content = content.push(
                        row![
                            text(matching_note(hidden_matching))
                                .size(14)
                                .color(palette.muted),
                            show_hidden(
                                ui::secondary_button("Show hidden", palette)
                                    .on_press(Message::Filter(Event::ShowHidden))
                            ),
                        ]
                        .spacing(12)
                        .align_y(Center),
                    );
                }
                content.into()
            }
            Some(Empty::Library) | None => return Space::new().into(),
        };
        // The docked panel covers the right side, so the message stays
        // clear of it.
        let right = if self.panel_reserved() {
            crate::grid_motion::DOCK_WIDTH
        } else {
            0.0
        };
        container(
            ui::section(content, palette)
                .max_width(SURFACE_WIDTH)
                .padding(16),
        )
        .padding(Padding {
            top: 24.0,
            right: 24.0 + right,
            bottom: 24.0,
            left: 24.0,
        })
        .width(Fill)
        .into()
    }

    /// The banner for a `library.toml` that can't be used, if it can't.
    pub(crate) fn library_banner(&self) -> Option<Element<'_, Message>> {
        let problem = self.library_problem.as_ref()?;
        // A newer wineo's file has no repair to offer.
        let id = (!problem.newer_version).then_some(CHECKUP_ID);
        Some(self.banner(banner_text(problem), id))
    }

    /// What the strip says about unreadable trash entries, if there are any.
    pub(crate) fn trash_banner_text(&self) -> Option<String> {
        let scopes = &self.trash_badge.unreadable;
        Some(match scopes.as_slice() {
            [] => return None,
            scopes if scopes.iter().any(Option::is_none) => {
                "An unreadable trash entry blocks every environment.".into()
            }
            [Some(name)] => format!("An unreadable trash entry blocks {name}."),
            _ => "Unreadable trash entries block some environments.".into(),
        })
    }

    pub(crate) fn trash_banner(&self) -> Option<Element<'_, Message>> {
        let words = self.trash_banner_text()?;
        Some(self.banner(words, Some(TRASH_CHECKUP_ID)))
    }

    /// A warning strip under the toolbar, with Open Checkup when `checkup`
    /// names its focus ID.
    fn banner(&self, words: String, checkup: Option<&'static str>) -> Element<'_, Message> {
        let palette = self.palette;
        let message = text(words).size(13).color(palette.foreground).width(Fill);
        let content: Element<'_, Message> = match checkup {
            None => message.into(),
            Some(id) => crate::ends::ends(
                message,
                focus_button::control(
                    ui::secondary_button("Open Checkup", palette)
                        .on_press(Message::Section(crate::sidebar::Section::Checkup))
                        .into(),
                    true,
                    id,
                    Message::Section(crate::sidebar::Section::Checkup),
                ),
                12.0,
                crate::ends::MIN_LEFT,
            ),
        };
        container(
            container(
                row![warning_mark(palette), content]
                    .spacing(10)
                    .align_y(Center),
            )
            .padding([8, 12])
            .width(Fill)
            .style(move |_| banner_style(palette)),
        )
        .padding([8, 16])
        .into()
    }
}

/// The empty states' Show hidden, reachable from the keyboard.
fn show_hidden(button: iced::widget::Button<'_, Message>) -> Element<'_, Message> {
    focus_button::control(
        button.into(),
        true,
        SHOW_HIDDEN_ID,
        Message::Filter(Event::ShowHidden),
    )
}

fn warning_mark(palette: Palette) -> Element<'static, Message> {
    container(Space::new().width(8).height(8))
        .style(move |_| {
            container::Style::default()
                .background(palette.warning)
                .border(iced::border::rounded(4))
        })
        .into()
}

fn banner_style(palette: Palette) -> container::Style {
    container::Style::default()
        .background(palette.warning.scale_alpha(0.10))
        .border(
            iced::border::rounded(ui::CONTROL_RADIUS)
                .width(1)
                .color(palette.warning.scale_alpha(0.6)),
        )
}

/// The banner's words: what `validate` says about the file, and what that
/// means for the library.
pub(crate) fn banner_text(problem: &Problem) -> String {
    let message = problem.message();
    // The newer-wineo message is already a sentence.
    let message = message.trim_end_matches('.');
    if problem.newer_version {
        format!("{message}. Favorites and hiding are off until then.")
    } else {
        format!("{message}. Favorites and hiding are off until it's fixed.")
    }
}

/// The everything-hidden state's count.
pub(crate) fn all_hidden_note(hidden: usize) -> String {
    if hidden == 1 {
        "1 hidden. Show it to unhide it.".to_owned()
    } else {
        format!("{hidden} hidden. Show them to unhide any.")
    }
}

/// How many hidden cards the search and filters would show.
pub(crate) fn matching_note(count: usize) -> String {
    if count == 1 {
        "1 match is hidden".to_owned()
    } else {
        format!("{count} matches are hidden")
    }
}

#[cfg(test)]
pub(crate) mod tests;
