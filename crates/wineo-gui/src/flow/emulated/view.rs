//! The emulated games page's view.

use iced::widget::{checkbox, column, container, pick_list, row, text, text_input};
use iced::{Center, Element, Fill, border};
use wineo_core::emulator::Emulator;
use wineo_core::rom_import::EntryState;

use crate::flow::frame::{PAGE_MAX_WIDTH, page_frame, pinned, scroll_body};
use crate::flow::import_grid::Tile;
use crate::theme::Palette;
use crate::ui::{
    ListRow, PickListStyle, accent_base, list_group, list_row, list_row_text, secondary_button,
};

use super::work::{self, Chosen};
use super::{
    CORE_LIST, CUSTOM_ROW, EMULATOR_LIST, EmulatorChoice, Event, OTHER_ROW, Page, SYSTEM_LIST, Step,
};

/// A labeled text field, with `placeholder` as its example.
fn field<'a>(
    label: &'a str,
    placeholder: &'a str,
    value: &'a str,
    on_input: fn(String) -> Event,
) -> Element<'a, Event> {
    column![
        text(label).size(13),
        text_input(placeholder, value).on_input(on_input).padding(8)
    ]
    .spacing(4)
    .into()
}

/// A choice row with what it is underneath.
fn choice<'a>(
    id: String,
    title: String,
    detail: String,
    event: Event,
    palette: Palette,
) -> ListRow<'a, Event> {
    ListRow::new(
        id,
        list_row(
            column![
                list_row_text(title),
                text(detail).size(12).color(palette.muted)
            ]
            .spacing(2),
            palette,
        ),
        event,
    )
}

impl Page {
    fn grid_view<'a>(
        &'a self,
        palette: Palette,
        width: f32,
        header: Element<'a, Event>,
    ) -> Element<'a, Event> {
        let entries = self
            .listing
            .as_ref()
            .map(|listing| listing.entries.as_slice())
            .unwrap_or_default();
        let tiles = entries.iter().enumerate().map(|(index, entry)| {
            let reason = match &entry.state {
                EntryState::New => None,
                EntryState::AlreadyImported(name) => Some(format!("Already imported as {name}")),
                EntryState::Unusable(reason) => Some(reason.clone()),
            };
            (
                Tile {
                    title: &entry.title,
                    poster: None,
                    reason,
                    note: entry.note.clone(),
                    chosen: self.grid.chosen.contains(&*entry.rom.to_string_lossy()),
                    focus_ring: self.grid.focus_ring(index),
                    dimmed: false,
                },
                Event::Toggle(index),
            )
        });
        self.grid.view(header, tiles, width, palette)
    }

    /// The options above the games: fullscreen where the emulator has it,
    /// and art. They hold still while the games are checked, since Review
    /// shows what they were then.
    fn options(&self, key_available: bool, palette: Palette) -> Element<'_, Event> {
        let idle = !self.waiting();
        let mut options = column![].spacing(8);
        if self.chosen.as_ref().is_some_and(Chosen::has_fullscreen) {
            options = options.push(
                checkbox(self.fullscreen)
                    .label("Start in fullscreen")
                    .on_toggle_maybe(idle.then_some(Event::Fullscreen)),
            );
        }
        if !self.art_enabled() {
            options = options.push(
                text("Automatic artwork is disabled in Global Settings").color(palette.muted),
            );
        } else if key_available {
            options = options.push(
                checkbox(self.fetch_art)
                    .label("Fetch art using the top SteamGridDB match")
                    .on_toggle_maybe(idle.then_some(Event::FetchArt)),
            );
        } else {
            options = options.push(
                text("SteamGridDB needs a configured key. Art can be chosen after import.")
                    .color(palette.muted),
            );
        }
        options.into()
    }

    pub(crate) fn view(
        &self,
        palette: Palette,
        width: f32,
        key_available: bool,
    ) -> Element<'_, Event> {
        let title = match self.step {
            Step::Emulator => "Add emulated games",
            Step::Other => "Emulator from a file",
            Step::Custom => "Custom emulator",
            Step::Folder => "Folder or file of games",
            Step::Core => "Choose a RetroArch core",
            Step::System => "Choose a system",
            Step::Games => "Choose games",
            Step::Review => "Review import",
            Step::Working => "Importing games",
            Step::Finished => "Import finished",
        };
        let mut body = column![crate::ui::page_title(title, palette)]
            .spacing(14)
            .width(Fill);
        if !self.status.is_empty() {
            body = body.push(
                row![
                    crate::spinner::spinner(palette, 16.0),
                    text(&self.status).color(palette.muted)
                ]
                .spacing(8)
                .align_y(Center),
            );
        }
        if let Some(error) = &self.error {
            body = body.push(text(error).color(palette.danger));
        }
        let mut games_actions: Option<Element<'_, Event>> = None;
        match self.step {
            Step::Emulator => {
                if let Some(found) = &self.found {
                    if found.installs.is_empty() {
                        body = body.push(
                            text("wineo found no emulators on this computer.").color(palette.muted),
                        );
                    }
                    let rows =
                        found
                            .installs
                            .iter()
                            .enumerate()
                            .map(|(index, (emulator, install))| {
                                choice(
                                    self.install_id(index),
                                    work::install_label(*emulator, install),
                                    work::install_detail(*emulator, install),
                                    Event::Install(index),
                                    palette,
                                )
                            });
                    let others = [
                        choice(
                            OTHER_ROW.to_owned(),
                            "Other...".to_owned(),
                            "An emulator wineo knows, run from a file like an AppImage".to_owned(),
                            Event::Other,
                            palette,
                        ),
                        choice(
                            CUSTOM_ROW.to_owned(),
                            "Custom".to_owned(),
                            "Any emulator, with the arguments and file types you give".to_owned(),
                            Event::Custom,
                            palette,
                        ),
                    ];
                    body = body.push(list_group(EMULATOR_LIST, rows.chain(others), palette));
                }
            }
            Step::Other => {
                let choices: Vec<EmulatorChoice> =
                    Emulator::BUILT_IN.into_iter().map(EmulatorChoice).collect();
                body = body.push(
                    column![
                        text("Which emulator is it?").size(13),
                        pick_list(
                            choices,
                            self.other_emulator.map(EmulatorChoice),
                            Event::OtherEmulator
                        )
                        .placeholder("Choose an emulator")
                        .wineo_style(palette),
                    ]
                    .spacing(4),
                );
                body = body.push(field(
                    "Emulator file",
                    "~/Apps/Emulator.AppImage",
                    &self.other_file,
                    Event::OtherFile,
                ));
                body = body
                    .push(secondary_button("Browse...", palette).on_press(Event::BrowseOtherFile));
            }
            Step::Custom => {
                let draft = &self.custom;
                body = body.push(field(
                    "Command",
                    "~/Apps/emulator.AppImage",
                    &draft.exe,
                    Event::CustomExe,
                ));
                body = body
                    .push(secondary_button("Browse...", palette).on_press(Event::BrowseCustomExe));
                body = body.push(field(
                    "Arguments",
                    "-f -g {rom}",
                    &draft.args,
                    Event::CustomArgs,
                ));
                body = body.push(
                    text("{rom} stands for the game. Quote an argument that has spaces.")
                        .size(12)
                        .color(palette.muted),
                );
                body = body.push(field(
                    "File types",
                    "nsp, xci",
                    &draft.extensions,
                    Event::CustomExtensions,
                ));
                body = body.push(field(
                    "System (optional)",
                    "switch",
                    &draft.system,
                    Event::CustomSystem,
                ));
                body = body.push(
                    text("Each game is tagged with the system.")
                        .size(12)
                        .color(palette.muted),
                );
            }
            Step::Folder => {
                body = body.push(field(
                    "Folder or file",
                    "~/Games/roms",
                    &self.folder,
                    Event::Folder,
                ));
                body = body.push(
                    row![
                        secondary_button("Browse folder...", palette).on_press(Event::BrowseFolder),
                        secondary_button("Browse file...", palette).on_press(Event::BrowseFile),
                    ]
                    .spacing(8),
                );
            }
            Step::Core => {
                if let Some(cores) = &self.cores {
                    body = body.push(
                        text("These cores play the files in the folder.").color(palette.muted),
                    );
                    body = body.push(list_group(
                        CORE_LIST,
                        cores.iter().enumerate().map(|(index, core)| {
                            choice(
                                Self::core_id(core),
                                work::core_label(core, cores),
                                core.system().unwrap_or("No system").to_owned(),
                                Event::Core(index),
                                palette,
                            )
                        }),
                        palette,
                    ));
                }
            }
            Step::System => {
                body = body.push(text(
                    "Which system are the games whose files could be either?",
                ));
                let systems = self.chosen.as_ref().map(Chosen::systems).unwrap_or(&[]);
                body = body.push(list_group(
                    SYSTEM_LIST,
                    systems.iter().enumerate().map(|(index, system)| {
                        ListRow::new(
                            Self::system_id(system),
                            list_row(list_row_text(*system), palette),
                            Event::System(index),
                        )
                    }),
                    palette,
                ));
            }
            Step::Games => {
                body = body.push(self.options(key_available, palette));
                if let Some(listing) = &self.listing {
                    for skipped in &listing.skipped {
                        body = body.push(
                            text(format!("Couldn't read {}, skipped.", skipped.display()))
                                .size(12)
                                .color(palette.warning),
                        );
                    }
                    if listing.entries.is_empty() {
                        let folder = self
                            .listed_folder
                            .as_ref()
                            .map(|folder| folder.display().to_string())
                            .unwrap_or_default();
                        body = body.push(
                            text(format!("No games found in {folder}.")).color(palette.muted),
                        );
                    } else {
                        let count = self.grid.chosen.len();
                        let idle = !self.waiting();
                        games_actions = Some(self.grid.footer(
                            idle.then_some(Event::SelectAll),
                            (idle && count > 0).then_some(Event::SelectNone),
                            None::<Element<'_, Event>>,
                            (idle && count > 0).then_some(Event::ImportSelected),
                            palette,
                        ));
                    }
                }
            }
            Step::Review => {
                if let Some(review) = &self.review {
                    let mut summary =
                        column![crate::ui::section_heading("Games", palette)].spacing(8);
                    for plan in &review.ready {
                        summary = summary.push(text(match &plan.system {
                            Some(system) => format!("{}: {} - {system}", plan.name, plan.title),
                            None => format!("{}: {}", plan.name, plan.title),
                        }));
                    }
                    if !review.ready.is_empty() {
                        let source = self.chosen.as_ref().map(Chosen::source).unwrap_or_default();
                        summary = summary.push(text(format!("Source: {source}")));
                        summary =
                            summary.push(text(format!("Emulator: {}", self.emulator_label())));
                        if self.chosen.as_ref().is_some_and(Chosen::has_fullscreen) {
                            summary = summary.push(text(if self.fullscreen {
                                "Fullscreen: on"
                            } else {
                                "Fullscreen: off"
                            }));
                        }
                        summary = summary.push(text("Desktop: off"));
                        summary = summary.push(text(if self.fetch_art {
                            "Art: SteamGridDB, top match"
                        } else {
                            "Art: none"
                        }));
                    }
                    for already in &review.already {
                        summary =
                            summary.push(text(format!("Already imported as {}.", already.name)));
                    }
                    for failure in &review.failures {
                        summary = summary.push(
                            text(format!("Failed {}: {}", failure.title, failure.reason))
                                .color(palette.warning),
                        );
                    }
                    if review.ready.is_empty() {
                        summary = summary
                            .push(text("No games are ready to import.").color(palette.muted));
                    }
                    body = body.push(container(summary).padding(16).width(Fill).style(move |_| {
                        container::Style::default()
                            .background(palette.surface)
                            .border(border::rounded(12).width(1).color(palette.border))
                    }));
                }
            }
            Step::Working => {}
            Step::Finished => {
                if let Some(report) = &self.report {
                    if report.canceled {
                        body = body.push(text("Completed imports remain after canceling."));
                    }
                    for line in work::report_lines(report) {
                        body = body.push(text(line));
                    }
                    for failure in &report.failures {
                        body = body.push(
                            text(format!("Failed {}: {}", failure.title, failure.reason))
                                .size(12)
                                .color(palette.warning),
                        );
                    }
                    for warning in &report.warnings {
                        body = body.push(text(warning).size(12).color(palette.warning));
                    }
                }
            }
        }
        let back = secondary_button("Back", palette)
            .on_press_maybe((!self.busy() && self.step != Step::Finished).then_some(Event::Back));
        let content: Element<'_, Event> = match (&self.listing, self.step) {
            // The title and options scroll with the grid, above the fixed action row.
            (Some(listing), Step::Games) if !listing.entries.is_empty() => {
                let grid = self.grid_view(palette, width, body.into());
                column![grid]
                    .push(games_actions)
                    .spacing(14)
                    .height(Fill)
                    .into()
            }
            _ => scroll_body(body, None, palette),
        };
        let mut page = column![row![back], content]
            .spacing(12)
            .max_width(PAGE_MAX_WIDTH)
            .height(Fill);
        let idle = !self.waiting();
        let footer: Option<Element<'_, Event>> = match self.step {
            Step::Other | Step::Custom | Step::Folder => Some(
                accent_base("Next", palette.primary)
                    .on_press_maybe(idle.then_some(Event::Next))
                    .into(),
            ),
            Step::Review => self.review.as_ref().map(|review| {
                accent_base("Import", palette.primary)
                    .on_press_maybe((idle && !review.ready.is_empty()).then_some(Event::Import))
                    .into()
            }),
            Step::Working => self.token.as_ref().map(|token| {
                let (label, event) = if token.is_canceled() {
                    ("Force stop", Event::Force)
                } else {
                    ("Cancel", Event::Cancel)
                };
                secondary_button(label, palette).on_press(event).into()
            }),
            Step::Finished => Some(
                secondary_button("Back to library", palette)
                    .on_press(Event::BackToLibrary)
                    .into(),
            ),
            Step::Emulator | Step::Core | Step::System | Step::Games => None,
        };
        if let Some(footer) = footer {
            page = page.push(pinned(footer, palette));
        }
        page_frame(page)
    }
}
