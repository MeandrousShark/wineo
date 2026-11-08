//! Saved presets, at the top of the Filters popup: applying one, saving the
//! view as a new one or over the active one, renaming, and deleting (plan
//! 0062, decisions 14 to 16 and 19). Applying works in memory only, so
//! `[view]` keeps what the controls last saved, and launching still opens
//! the saved view.

use iced::widget::{Space, button, column, container, row, text, text_input};
use iced::{Center, Element, Fill, Font, Task, border};
use wineo_core::error::Error;
use wineo_core::library::{self, GroupBy, Library, Preset, ViewLayout};
use wineo_core::paths::Paths;

use crate::grid::{ACTION_MENU_PADDING, ACTION_MENU_WIDTH, context_menu_style};
use crate::library_menu::menu_row;
use crate::order::{Filters, Query};
use crate::sort_control::Field;
use crate::theme::Palette;
use crate::{App, Message, focus_button, rescan, ui};

pub(crate) const SAVE_ID: &str = "library-preset-save";
pub(crate) const UPDATE_ID: &str = "library-preset-update";
pub(crate) const NAME_ID: &str = "library-preset-name";
/// The Update offer under a name that's taken.
pub(crate) const OFFER_ID: &str = "library-preset-offer";
pub(crate) const RENAME_ID: &str = "library-preset-rename";
pub(crate) const DELETE_ID: &str = "library-preset-delete";
pub(crate) const KEEP_ID: &str = "library-preset-keep";
pub(crate) const CONFIRM_ID: &str = "library-preset-delete-confirm";
pub(crate) const CHECKUP_ID: &str = "library-preset-checkup";

/// The room a row's label has: the popup less its padding and the row's.
const ROW_TEXT: f32 = ACTION_MENU_WIDTH - 2.0 * ACTION_MENU_PADDING - 20.0;
/// The most of a preset's name the Filters button shows, so a long one
/// can't push the rest of the toolbar off the window.
pub(crate) const BUTTON_NAME: f32 = 120.0;
/// The action rows' indent under their preset's row.
const ACTION_INDENT: f32 = 12.0;

pub(crate) fn row_id(name: &str) -> String {
    format!("library-preset-row-{name}")
}

#[derive(Debug, Clone)]
pub(crate) enum Event {
    /// A click, Enter, or Space on a preset's row.
    Apply(String),
    /// **Save as preset**, which opens the name field.
    StartSave,
    /// **Update "X"**, which saves the view over that preset.
    Update(String),
    /// The name field's text.
    Draft(String),
    /// Enter in the name field.
    Submit,
    /// A right-click, Shift+F10, or the Menu key on a preset's row, which
    /// shows Rename and Delete under it.
    Actions(String),
    /// R on a preset's row, or its Rename.
    Rename(String),
    /// Delete on a preset's row, or its Delete, which asks first.
    AskDelete(String),
    /// **Keep it** in the delete question.
    Keep,
    /// **Delete** in the delete question.
    Delete(String),
    OpenCheckup,
    /// A write finished.
    Saved(Box<Done>),
}

impl Event {
    /// Input from the controls, which an overlay or dialog blocks.
    pub(crate) fn is_input(&self) -> bool {
        !matches!(self, Self::Saved(_))
    }
}

/// What the presets part of the popup is in the middle of.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Edit {
    /// Naming a new preset, with the name typed so far.
    Save(String),
    Rename {
        old: String,
        draft: String,
    },
    /// A preset's Rename and Delete rows are showing.
    Actions(String),
    /// Asking before deleting a preset.
    Delete(String),
}

/// One write to `library.toml`'s presets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Write {
    /// Saves a new preset, refused when the name is taken on disk by then.
    Create(String, Box<Preset>),
    /// Saves over the preset, or creates it again if it's gone.
    Save(String, Box<Preset>),
    Rename(String, String),
    Delete(String),
}

impl Write {
    /// The lead-in of a notice when this write fails.
    fn failed(&self) -> String {
        match self {
            Self::Create(name, _) | Self::Save(name, _) => format!("Couldn't save \"{name}\""),
            Self::Rename(old, _) => format!("Couldn't rename \"{old}\""),
            Self::Delete(name) => format!("Couldn't delete \"{name}\""),
        }
    }

    /// Whether it writes the name typed in the field.
    fn typed(&self) -> bool {
        matches!(self, Self::Create(..) | Self::Rename(..))
    }
}

/// A write that didn't land, as the popup shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Failure {
    pub(crate) text: String,
    /// `library.toml` refused it in a way Checkup can repair, so the popup
    /// offers Open Checkup.
    pub(crate) checkup: bool,
    /// Core refused a name another preset has.
    pub(crate) taken: bool,
    /// It was Save as preset or a rename, so it goes with the typed name.
    pub(crate) typed: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct Done {
    pub(crate) write: Write,
    pub(crate) result: Result<(), Failure>,
    /// The presets on disk once the write was over, or None when the file
    /// couldn't be read or the worker died before it could look.
    pub(crate) presets: Option<List>,
}

/// What `library.toml` holds under `[presets]`: the presets by name, A to Z,
/// and the names of entries that aren't presets, which are taken all the same.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct List {
    presets: Vec<(String, Preset)>,
    other: Vec<String>,
}

impl List {
    pub(crate) fn of(library: &Library) -> Self {
        Self {
            presets: library.presets.clone(),
            other: library.other_presets.clone(),
        }
    }
}

/// The window's presets and what's being done to them.
#[derive(Debug, Default)]
pub(crate) struct Presets {
    /// `library.toml`'s presets as last read; the rest of it reaches the
    /// window by other paths.
    saved: List,
    /// The preset applied or saved last this session. The Filters button
    /// names it while the view still matches it, and Update offers to save
    /// the view over it once it doesn't.
    pub(crate) active: Option<String>,
    pub(crate) edit: Option<Edit>,
    /// The write in flight. Only one runs at a time.
    pub(crate) writing: Option<Write>,
    pub(crate) failure: Option<Failure>,
    /// Bumped by every apply and every save started, so a save that lands
    /// after a later choice doesn't take the active preset back.
    intent: u64,
    /// `intent` when the write in flight started.
    started: u64,
}

impl Presets {
    pub(crate) fn list(&self) -> &[(String, Preset)] {
        &self.saved.presets
    }

    pub(crate) fn get(&self, name: &str) -> Option<&Preset> {
        self.saved
            .presets
            .iter()
            .find(|(saved, _)| saved == name)
            .map(|(_, preset)| preset)
    }

    /// The entry under `[presets]` that `name` would collide with, a
    /// non-preset's included; `except` is a name that doesn't count.
    pub(crate) fn taken(&self, name: &str, except: Option<&str>) -> Option<&str> {
        let names = self
            .saved
            .presets
            .iter()
            .map(|(saved, _)| saved.as_str())
            .chain(self.saved.other.iter().map(String::as_str));
        library::preset_name_taken(names, name, except)
    }
}

/// The filters a preset applies. An include that's also excluded is left
/// out, since a popup row is never both; a tag no game has stays, so its
/// chip shows until it's cleared.
pub(crate) fn filters_of(preset: &Preset) -> Filters {
    Filters {
        stores: preset
            .stores
            .difference(&preset.not_stores)
            .copied()
            .collect(),
        platforms: preset
            .platforms
            .difference(&preset.not_platforms)
            .copied()
            .collect(),
        tags: preset.tags.difference(&preset.not_tags).cloned().collect(),
        not_stores: preset.not_stores.clone(),
        not_platforms: preset.not_platforms.clone(),
        not_tags: preset.not_tags.clone(),
        show_hidden: preset.show_hidden,
    }
}

/// A preset holding this view.
pub(crate) fn preset_of(
    query: Query,
    filters: &Filters,
    group: GroupBy,
    layout: ViewLayout,
) -> Preset {
    Preset {
        sort: query.sort,
        descending: query.descending,
        favorites_first: query.favorites_first,
        stores: filters.stores.clone(),
        platforms: filters.platforms.clone(),
        tags: filters.tags.clone(),
        not_stores: filters.not_stores.clone(),
        not_platforms: filters.not_platforms.clone(),
        not_tags: filters.not_tags.clone(),
        show_hidden: filters.show_hidden,
        group,
        layout,
    }
}

/// The view applying `preset` gives, as a preset, so a saved one that
/// both includes and excludes a value still matches once applied.
fn applied(preset: &Preset) -> Preset {
    let query = Query {
        sort: preset.sort,
        descending: preset.descending,
        favorites_first: preset.favorites_first,
    };
    preset_of(query, &filters_of(preset), preset.group, preset.layout)
}

/// Does `write`, then rereads the rescan's cache whatever happened and
/// returns the presets as the file has them now, unless it can't be read.
pub(crate) fn write(
    paths: &Paths,
    watch: &rescan::Watch,
    write: &Write,
) -> (Result<(), Failure>, Option<List>) {
    let result = match write {
        Write::Create(name, preset) => library::create_preset(paths, name, preset),
        Write::Save(name, preset) => library::set_preset(paths, name, preset),
        Write::Rename(old, new) => library::rename_preset(paths, old, new),
        Write::Delete(name) => library::remove_preset(paths, name),
    };
    let presets = watch.refresh_library_presets(paths);
    let result = result.map(|_| ()).map_err(|error| Failure {
        text: format!(
            "{}: {}",
            write.failed(),
            wineo_core::error::one_line(crate::sort_control::error_text(paths, &error))
        ),
        checkup: repairable(&error),
        taken: matches!(error, Error::PresetNameTaken(_)),
        typed: write.typed(),
    });
    (result, presets)
}

/// Like the library's banner: a newer wineo's file has nothing for Checkup
/// to repair, and the error already says to update.
fn repairable(error: &Error) -> bool {
    matches!(
        error,
        Error::LibraryUnreadable {
            newer_version: false,
            ..
        }
    )
}

impl App {
    /// The view as a preset.
    pub(crate) fn view_preset(&self) -> Preset {
        preset_of(self.query, &self.filters, self.group, self.layout)
    }

    /// Whether the view is what applying the saved preset `name` gives.
    fn view_matches(&self, name: &str) -> bool {
        self.presets
            .get(name)
            .is_some_and(|preset| applied(preset) == self.view_preset())
    }

    /// The active preset while the view still matches it, which the Filters
    /// button names.
    pub(crate) fn preset_shown(&self) -> Option<&str> {
        self.presets
            .active
            .as_deref()
            .filter(|name| self.view_matches(name))
    }

    /// The active preset once the view differs from it, which **Update**
    /// offers to save over.
    pub(crate) fn preset_to_update(&self) -> Option<&str> {
        self.presets
            .active
            .as_deref()
            .filter(|name| self.presets.get(name).is_some() && !self.view_matches(name))
    }

    /// Takes the presets from a read of `library.toml`, which the rescan
    /// then compares against. A preset gone from the file stops being
    /// active, and an edit of it closes.
    pub(crate) fn presets_read(&mut self, presets: List) {
        self.shown.show_presets(presets.clone());
        self.presets.saved = presets;
        // A rename of the active preset that's landing on disk takes its
        // name along when its reply comes, so a read in between doesn't
        // count it as removed.
        let renaming = matches!(
            (&self.presets.writing, &self.presets.active),
            (Some(Write::Rename(old, _)), Some(active)) if old == active
        );
        if let Some(active) = &self.presets.active
            && !renaming
            && self.presets.get(active).is_none()
        {
            self.presets.active = None;
        }
        let gone = match &self.presets.edit {
            Some(Edit::Rename { old: name, .. } | Edit::Actions(name) | Edit::Delete(name)) => {
                self.presets.get(name).is_none()
            }
            _ => false,
        };
        if gone && self.presets.writing.is_none() {
            self.presets.edit = None;
        }
    }

    /// Opening the popup starts the presets part afresh, except for a write
    /// still landing and a typed name, with its own failure if it has one,
    /// so it isn't lost. A field opened with nothing typed in it closes, and
    /// a failed Update or Delete is forgotten.
    pub(crate) fn presets_opened(&mut self) {
        if self.presets.writing.is_some() {
            return;
        }
        let typed = match &self.presets.edit {
            Some(Edit::Save(draft)) => !draft.is_empty(),
            Some(Edit::Rename { old, draft }) => draft != old,
            _ => false,
        };
        if !typed {
            self.presets.edit = None;
        }
        if !(typed
            && self
                .presets
                .failure
                .as_ref()
                .is_some_and(|failure| failure.typed))
        {
            self.presets.failure = None;
        }
    }

    /// The presets part's focusable controls, in order.
    pub(crate) fn preset_ids(&self) -> Vec<String> {
        let mut ids = Vec::new();
        let edit = self.presets.edit.as_ref();
        for (name, _) in self.presets.list() {
            match edit {
                Some(Edit::Rename { old, .. }) if old == name => ids.push(NAME_ID.to_owned()),
                _ => ids.push(row_id(name)),
            }
            match edit {
                Some(Edit::Actions(shown)) if shown == name => {
                    ids.extend([RENAME_ID.to_owned(), DELETE_ID.to_owned()]);
                }
                Some(Edit::Delete(shown)) if shown == name => {
                    ids.extend([KEEP_ID.to_owned(), CONFIRM_ID.to_owned()]);
                }
                _ => {}
            }
        }
        if self.preset_to_update().is_some() {
            ids.push(UPDATE_ID.to_owned());
        }
        if let Some(Edit::Save(draft)) = edit {
            ids.push(NAME_ID.to_owned());
            if self.update_offer(draft).is_some() {
                ids.push(OFFER_ID.to_owned());
            }
        } else {
            ids.push(SAVE_ID.to_owned());
        }
        if self
            .presets
            .failure
            .as_ref()
            .is_some_and(|failure| failure.checkup)
        {
            ids.push(CHECKUP_ID.to_owned());
        }
        ids
    }

    /// The entry a new preset named `draft` would collide with, which is
    /// only worth updating when it's a preset.
    fn save_taken(&self, draft: &str) -> Option<&str> {
        let name = library::normalize_preset_name(draft).ok()?;
        self.presets.taken(&name, None)
    }

    /// The preset a taken name offers to update, none when the entry is
    /// something else, which core would refuse to save over.
    fn update_offer(&self, draft: &str) -> Option<&str> {
        self.save_taken(draft)
            .filter(|taken| self.presets.get(taken).is_some())
    }

    /// The entry renaming `old` to `draft` would collide with. A change of
    /// case alone is fine.
    fn rename_taken(&self, old: &str, draft: &str) -> Option<&str> {
        let name = library::normalize_preset_name(draft).ok()?;
        self.presets.taken(&name, Some(old))
    }

    /// Escape while the presets part is mid-edit closes the edit and gives
    /// focus back to where it started, leaving the popup open. None when
    /// there's no edit, so Escape closes the popup.
    pub(crate) fn cancel_preset_edit(&mut self) -> Option<Task<Message>> {
        let edit = self.presets.edit.take()?;
        self.presets.failure = None;
        let id = match edit {
            Edit::Save(_) => SAVE_ID.to_owned(),
            Edit::Rename { old: name, .. } | Edit::Actions(name) | Edit::Delete(name) => {
                row_id(&name)
            }
        };
        Some(self.focus_in_popup(id))
    }

    fn focus_in_popup(&self, id: String) -> Task<Message> {
        if self.filter_menu_shown() {
            crate::library_menu::focus(id, crate::filter_control::MENU_SCROLL_ID)
        } else {
            Task::none()
        }
    }

    /// Shows the preset `name`'s filters, Show hidden, sort, direction,
    /// Favorites first, grouping, and layout all at once. Nothing is
    /// written (decision 16), while a control changed later still saves
    /// its own field. Search, zoom, and poster-only stay, and marks keep
    /// only the games still shown.
    fn apply_preset(&mut self, name: String) -> Task<Message> {
        let Some(preset) = self.presets.get(&name).cloned() else {
            return Task::none();
        };
        // Measured from what's drawn now, before the order changes under it.
        let anchor = (preset.layout != self.layout).then(|| self.find_anchor());
        self.filter_menu = false;
        self.filter_opening.cancel();
        self.presets.edit = None;
        self.presets.failure = None;
        self.presets.active = Some(name);
        self.presets.intent += 1;
        // A `[view]` write in flight was for the view as the controls left
        // it, so it mustn't follow up with the preset's.
        self.view_writes.overtake(&[
            Field::Sort(preset.sort, preset.descending),
            Field::Descending(preset.descending),
            Field::FavoritesFirst(preset.favorites_first),
            Field::Group(preset.group),
            Field::Layout(preset.layout),
        ]);
        self.filters = filters_of(&preset);
        self.query = Query {
            sort: preset.sort,
            descending: preset.descending,
            favorites_first: preset.favorites_first,
        };
        self.group = preset.group;
        // Rebuilt before the switch lays the new layout out from it.
        self.reorder();
        let switched = anchor.map_or_else(Task::none, |anchor| {
            self.switch_layout_from(preset.layout, anchor, true)
        });
        let resorted = self.reflowed();
        let focus = iced::widget::operation::focus(iced::widget::Id::new(
            crate::filter_control::FILTER_BUTTON_ID,
        ));
        Task::batch([switched, resorted, focus])
    }

    /// Why nothing can be written while `library.toml` can't be used, as
    /// the single favorite toggle refuses up front.
    fn presets_refused(&mut self, write: &Write) -> bool {
        let Some(problem) = &self.library_problem else {
            return false;
        };
        let error = Error::LibraryUnreadable {
            newer_version: problem.newer_version,
            source: problem.message().into(),
        };
        self.presets.failure = Some(Failure {
            text: format!(
                "{}: {}",
                write.failed(),
                wineo_core::error::one_line(error.to_string())
            ),
            checkup: repairable(&error),
            taken: false,
            typed: write.typed(),
        });
        true
    }

    fn start_preset_write(&mut self, write: Write) -> Task<Message> {
        if self.presets.writing.is_some() || self.presets_refused(&write) {
            return Task::none();
        }
        self.presets.failure = None;
        if matches!(write, Write::Create(..) | Write::Save(..)) {
            self.presets.intent += 1;
        }
        self.presets.started = self.presets.intent;
        self.presets.writing = Some(write.clone());
        let paths = self.paths.clone();
        let watch = self.shown.clone();
        let work = write.clone();
        crate::task::blocking_value(
            move || self::write(&paths, &watch, &work),
            move |result| {
                let (result, presets) = match result {
                    Ok((result, presets)) => (result, presets),
                    Err(error) => (
                        Err(Failure {
                            text: format!("{}: {error}", write.failed()),
                            checkup: false,
                            taken: false,
                            typed: write.typed(),
                        }),
                        None,
                    ),
                };
                preset_message(Event::Saved(Box::new(Done {
                    write,
                    result,
                    presets,
                })))
            },
        )
    }

    fn submit_preset_name(&mut self) -> Task<Message> {
        match self.presets.edit.clone() {
            Some(Edit::Save(draft)) => {
                // A bad or taken name stays in the field with its reason
                // under it, and a taken one offers Update instead.
                let Ok(name) = library::normalize_preset_name(&draft) else {
                    return Task::none();
                };
                if self.presets.taken(&name, None).is_some() {
                    return Task::none();
                }
                let preset = self.view_preset();
                self.start_preset_write(Write::Create(name, Box::new(preset)))
            }
            Some(Edit::Rename { old, draft }) => {
                let Ok(name) = library::normalize_preset_name(&draft) else {
                    return Task::none();
                };
                // The same name writes nothing (decision 19).
                if name == old {
                    self.presets.edit = None;
                    self.presets.failure = None;
                    return self.focus_in_popup(row_id(&old));
                }
                if self.rename_taken(&old, &name).is_some() {
                    return Task::none();
                }
                self.start_preset_write(Write::Rename(old, name))
            }
            _ => Task::none(),
        }
    }

    fn preset_saved(&mut self, done: Done) -> Task<Message> {
        self.presets.writing = None;
        if self.exiting {
            return Task::none();
        }
        let reads = self.outdate_reads();
        // A name taken on disk since the last read, which the list read
        // below may show as taken just as a name it already had.
        let mut created = None;
        let focus = match done.result {
            Ok(()) => {
                let id = match &done.write {
                    Write::Create(name, _) | Write::Save(name, _) => {
                        // Only when nothing was applied or saved since.
                        if self.presets.intent == self.presets.started {
                            self.presets.active = Some(name.clone());
                        }
                        row_id(name)
                    }
                    Write::Rename(old, new) => {
                        // The active preset's name follows it.
                        if self.presets.active.as_deref() == Some(old.as_str()) {
                            self.presets.active = Some(new.clone());
                        }
                        row_id(new)
                    }
                    Write::Delete(name) => {
                        if self.presets.active.as_deref() == Some(name.as_str()) {
                            self.presets.active = None;
                        }
                        SAVE_ID.to_owned()
                    }
                };
                self.presets.edit = None;
                self.presets.failure = None;
                Some(id)
            }
            Err(failure) => {
                // The view and the label stay; what was typed stays in its
                // field for another try.
                let shown = self.filter_menu_shown();
                if !shown {
                    self.flag_notice(failure.text.clone());
                }
                if let (true, Write::Create(name, _)) = (shown && failure.taken, &done.write) {
                    created = Some(name.clone());
                }
                self.presets.failure = Some(failure);
                None
            }
        };
        if let Some(presets) = done.presets {
            self.presets_read(presets);
            // Only when the list now shows it is the failure line redundant;
            // an unreadable file or a taker gone again leaves it to say why.
            if created.is_some_and(|name| self.presets.taken(&name, None).is_some()) {
                self.presets.failure = None;
            }
        }
        let focus = focus.map_or_else(Task::none, |id| self.focus_in_popup(id));
        Task::batch([reads, focus])
    }
}

pub(crate) fn preset_message(event: Event) -> Message {
    Message::Filter(crate::filter_control::Event::Preset(event))
}

pub(crate) fn update(app: &mut App, event: Event) -> Task<Message> {
    if let Event::Saved(done) = event {
        return app.preset_saved(*done);
    }
    // Every control is in the popup, so nothing acts once it's gone.
    if !app.filter_menu_shown() {
        return Task::none();
    }
    match event {
        Event::Apply(name) => app.apply_preset(name),
        Event::StartSave => {
            app.presets.edit = Some(Edit::Save(String::new()));
            app.presets.failure = None;
            app.focus_in_popup(NAME_ID.to_owned())
        }
        Event::Update(name) => {
            let preset = app.view_preset();
            app.start_preset_write(Write::Save(name, Box::new(preset)))
        }
        Event::Draft(draft) => {
            if let Some(Edit::Save(text) | Edit::Rename { draft: text, .. }) = &mut app.presets.edit
            {
                *text = draft;
                app.presets.failure = None;
            }
            Task::none()
        }
        Event::Submit => app.submit_preset_name(),
        Event::Actions(name) => {
            app.presets.edit = Some(Edit::Actions(name));
            app.presets.failure = None;
            app.focus_in_popup(RENAME_ID.to_owned())
        }
        Event::Rename(name) => {
            app.presets.edit = Some(Edit::Rename {
                old: name.clone(),
                draft: name,
            });
            app.presets.failure = None;
            app.focus_in_popup(NAME_ID.to_owned())
        }
        Event::AskDelete(name) => {
            app.presets.edit = Some(Edit::Delete(name));
            app.presets.failure = None;
            // The safe answer has focus, so a second Delete or Enter keeps it.
            app.focus_in_popup(KEEP_ID.to_owned())
        }
        Event::Keep => app.cancel_preset_edit().unwrap_or_else(Task::none),
        Event::Delete(name) => app.start_preset_write(Write::Delete(name)),
        Event::OpenCheckup => {
            app.filter_menu = false;
            app.navigate_section(crate::sidebar::Section::Checkup)
        }
        Event::Saved(_) => unreachable!("handled above"),
    }
}

/// `name` cut to fit `width` at the popup's text size.
fn fit(name: &str, width: f32) -> String {
    crate::tile_title::fit(name, width, 14.0, Font::DEFAULT)
}

/// The label of **Update "X"**, with the name cut to fit the row.
fn update_label(name: &str) -> String {
    let frame = crate::tile_title::line_width("Update \"\"", 14.0, Font::DEFAULT);
    format!("Update \"{}\"", fit(name, ROW_TEXT - frame))
}

fn danger_line(words: String, palette: Palette) -> Element<'static, Message> {
    text(words).size(12).color(palette.danger).into()
}

/// A quiet action row under a preset, indented so it reads as that
/// preset's, in the danger color when it destroys.
fn action_row(
    label: &'static str,
    id: &'static str,
    message: Message,
    danger: bool,
    palette: Palette,
) -> Element<'static, Message> {
    let row = button(container(text(label).size(14)).height(Fill).center_y(Fill))
        .width(Fill)
        .height(ui::BUTTON_HEIGHT)
        .padding([0, 10])
        .on_press(message.clone())
        .style(move |_, status| context_menu_style(palette, danger, status));
    container(focus_button::control(row.into(), true, id, message))
        .padding(iced::Padding {
            left: ACTION_INDENT,
            ..iced::Padding::ZERO
        })
        .into()
}

/// The question before a preset is deleted, as custom themes ask it.
fn delete_card(name: &str, palette: Palette) -> Element<'static, Message> {
    let keep = preset_message(Event::Keep);
    let delete = preset_message(Event::Delete(name.to_owned()));
    let actions = row![
        Space::new().width(Fill),
        focus_button::control(
            ui::secondary_button("Keep it", palette)
                .on_press(keep.clone())
                .into(),
            true,
            KEEP_ID,
            keep,
        ),
        focus_button::control(
            ui::accent_button("Delete", palette.danger, true, delete.clone()),
            true,
            CONFIRM_ID,
            delete,
        ),
    ]
    .spacing(8)
    .align_y(Center);
    container(
        column![
            text(format!("Delete \"{}\"?", fit(name, ROW_TEXT - 60.0)))
                .size(14)
                .color(palette.foreground),
            actions,
        ]
        .spacing(8),
    )
    .padding(8)
    .width(Fill)
    .style(move |_| {
        container::Style::default()
            .background(palette.surface)
            .border(
                border::rounded(ui::CONTROL_RADIUS)
                    .width(1)
                    .color(palette.danger),
            )
    })
    .into()
}

impl App {
    /// The name field, with what's wrong with the name under it.
    fn name_field(&self, draft: &str, taken: Option<&str>) -> Vec<Element<'_, Message>> {
        let palette = self.palette;
        let idle = self.presets.writing.is_none();
        let field = text_input("Preset name", draft)
            .id(iced::widget::Id::new(NAME_ID))
            .on_input_maybe(idle.then_some(|draft| preset_message(Event::Draft(draft))))
            .on_submit_maybe(idle.then(|| preset_message(Event::Submit)))
            .size(14)
            .padding([6, 10])
            .style(move |_, status| crate::chips::input_style(palette, status));
        let mut items = vec![field.into()];
        if !draft.is_empty()
            && let Err(problem) = library::normalize_preset_name(draft)
        {
            items.push(danger_line(problem, palette));
        }
        if let Some(taken) = taken {
            items.push(danger_line(
                Error::PresetNameTaken(taken.to_owned()).to_string(),
                palette,
            ));
        }
        items
    }

    /// The presets part of the Filters popup: a row per preset, Update
    /// while the view differs from the active one, and **Save as preset**
    /// or its name field.
    pub(crate) fn presets_view(&self) -> Vec<Element<'_, Message>> {
        let palette = self.palette;
        let shown = self.preset_shown();
        let edit = self.presets.edit.as_ref();
        let mut items = Vec::new();
        for (name, _) in self.presets.list() {
            if let Some(Edit::Rename { old, draft }) = edit
                && old == name
            {
                let taken = self.rename_taken(old, draft);
                items.extend(self.name_field(draft, taken));
                continue;
            }
            let apply = preset_message(Event::Apply(name.clone()));
            let chosen = shown == Some(name.as_str());
            let label = button(
                container(text(fit(name, ROW_TEXT)).size(14))
                    .height(Fill)
                    .center_y(Fill),
            )
            .width(Fill)
            .height(ui::BUTTON_HEIGHT)
            .padding([0, 10])
            .on_press(apply.clone())
            .style(move |_, status| {
                let mut style = context_menu_style(palette, false, status);
                if chosen && status == button::Status::Active {
                    style.background = Some(palette.primary.scale_alpha(0.12).into());
                }
                style
            });
            items.push(focus_button::editable_row(
                label.into(),
                row_id(name),
                apply,
                preset_message(Event::Actions(name.clone())),
                preset_message(Event::Rename(name.clone())),
                preset_message(Event::AskDelete(name.clone())),
            ));
            match edit {
                Some(Edit::Actions(open)) if open == name => {
                    items.push(action_row(
                        "Rename",
                        RENAME_ID,
                        preset_message(Event::Rename(name.clone())),
                        false,
                        palette,
                    ));
                    items.push(action_row(
                        "Delete",
                        DELETE_ID,
                        preset_message(Event::AskDelete(name.clone())),
                        true,
                        palette,
                    ));
                }
                Some(Edit::Delete(open)) if open == name => {
                    items.push(delete_card(name, palette));
                }
                _ => {}
            }
        }
        if let Some(name) = self.preset_to_update() {
            items.push(menu_row(
                text(update_label(name)).size(14).into(),
                palette,
                false,
                UPDATE_ID,
                preset_message(Event::Update(name.to_owned())),
            ));
        }
        match edit {
            Some(Edit::Save(draft)) => {
                let taken = self.save_taken(draft);
                items.extend(self.name_field(draft, taken));
                // A taken name offers to save over that preset instead.
                if let Some(taken) = self.update_offer(draft) {
                    items.push(menu_row(
                        text(update_label(taken)).size(14).into(),
                        palette,
                        false,
                        OFFER_ID,
                        preset_message(Event::Update(taken.to_owned())),
                    ));
                }
            }
            _ => items.push(menu_row(
                text("Save as preset").size(14).into(),
                palette,
                false,
                SAVE_ID,
                preset_message(Event::StartSave),
            )),
        }
        // The taken name's message under the field already says it.
        if let Some(failure) = self.presets.failure.as_ref().filter(|failure| {
            !(failure.taken
                && matches!(edit, Some(Edit::Save(draft)) if self.save_taken(draft).is_some()))
        }) {
            items.push(danger_line(failure.text.clone(), palette));
            if failure.checkup {
                let open = preset_message(Event::OpenCheckup);
                items.push(focus_button::control(
                    ui::secondary_button("Open Checkup", palette)
                        .on_press(open.clone())
                        .into(),
                    true,
                    CHECKUP_ID,
                    open,
                ));
            }
        }
        items
    }
}

#[cfg(test)]
mod tests;
