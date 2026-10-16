//! The Version picker in a GOG Windows game's panel: the builds GOG lists
//! for each branch, a private branch's password, and Repair or a move to
//! the other track. Each action ends on the update page's review.

use std::fmt;
use std::time::Instant;

use iced::widget::{Space, column, pick_list, row, text, text_input};
use iced::{Center, Element, Fill, Task};
use wineo_core::gog::account::Session;
use wineo_core::gog::api::{Client, Secret};
use wineo_core::gog::folder::{Platform, Track};
use wineo_core::gog::migrate::MigrateError;
use wineo_core::gog::update::galaxy::listing::{self, Listed, Listing};
use wineo_core::gog::update::galaxy::record::Follow;
use wineo_core::gog::update::galaxy::{self as core_galaxy, Choice, GalaxyError};
use wineo_core::gog::update::install::Request;
use wineo_core::gog::update::{Options, Status, UpdateError};
use wineo_core::key_store::Keyring;
use wineo_core::paths::Paths;

use crate::library::{Card, CardKind};
use crate::prefix_facts::Facts;
use crate::progress::Delayed;
use crate::ui::{
    PickListStyle, accent_base, list_group, list_row, list_row_text, secondary_button,
};
use crate::{App, Message};

/// What the view found when it opened.
#[derive(Debug, Clone)]
pub(crate) enum Loaded {
    Galaxy(Box<Listing>),
    /// The game updates from offline installers. `galaxy` says whether GOG
    /// has a Galaxy build to move it to.
    Installer {
        galaxy: bool,
    },
}

/// A branch in the dropdown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Branch {
    /// None for the default branch.
    pub(crate) name: Option<String>,
    pub(crate) private: bool,
}

impl fmt::Display for Branch {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (&self.name, self.private) {
            (None, _) => formatter.write_str("Default"),
            (Some(name), true) => write!(formatter, "{name} (private)"),
            (Some(name), false) => formatter.write_str(name),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) enum Event {
    Open(String),
    Back,
    /// A listing answered, for the generation it was asked in, with the
    /// private branch entered for it.
    Loaded(u64, Option<String>, Result<Loaded, String>),
    /// The loading indicator's wakeup.
    Reveal(u64),
    Branch(Branch),
    /// A build row, by its id.
    Pick(String),
    Review,
    Repair,
    MoveToInstallers,
    MoveToGalaxy,
    EnterPrivate,
    PrivateBranch(String),
    PrivatePassword(Secret),
    ListBuilds,
}

impl Event {
    /// Whether it comes from the person, so an open overlay stops it.
    pub(crate) fn is_input(&self) -> bool {
        !matches!(self, Self::Loaded(..) | Self::Reveal(_))
    }
}

pub(crate) struct Picker {
    pub(crate) name: String,
    generation: u64,
    loading: Delayed<Message>,
    busy: bool,
    loaded: Option<Loaded>,
    error: Option<String>,
    /// Why the view offers nothing, said instead of loading.
    note: Option<String>,
    branch: Option<String>,
    picked: Option<String>,
    private_open: bool,
    private_branch: String,
    private_password: Secret,
    private_error: Option<String>,
}

fn send(event: Event) -> Message {
    Message::Version(event)
}

fn reveal(generation: u64) -> Message {
    Message::Version(Event::Reveal(generation))
}

impl Picker {
    fn new(name: String) -> Self {
        Self {
            name,
            generation: 0,
            loading: Delayed::new(reveal),
            busy: false,
            loaded: None,
            error: None,
            note: None,
            branch: None,
            picked: None,
            private_open: false,
            private_branch: String::new(),
            private_password: Secret::new(""),
            private_error: None,
        }
    }

    fn listing(&self) -> Option<&Listing> {
        match &self.loaded {
            Some(Loaded::Galaxy(listing)) => Some(listing),
            _ => None,
        }
    }

    /// The build rows of the branch shown.
    fn rows(&self) -> Vec<&Listed> {
        self.listing()
            .map(|listing| {
                listing
                    .builds
                    .iter()
                    .filter(|build| build.branch == self.branch)
                    .collect()
            })
            .unwrap_or_default()
    }

    fn picked_row(&self) -> Option<&Listed> {
        let picked = self.picked.as_deref()?;
        self.rows().into_iter().find(|row| row.build == picked)
    }

    /// The choice the picked row makes, when it changes something.
    pub(crate) fn choice(&self) -> Option<Choice> {
        let listing = self.listing()?;
        let row = self.picked_row()?;
        (!is_current(listing, row)).then(|| choice_for(listing, row))
    }
}

/// The branches GOG lists builds on, in its order, default first.
pub(crate) fn branches(listing: &Listing) -> Vec<Branch> {
    let mut branches: Vec<Branch> = Vec::new();
    for build in &listing.builds {
        if !branches.iter().any(|branch| branch.name == build.branch) {
            branches.push(Branch {
                name: build.branch.clone(),
                private: build.private,
            });
        }
    }
    branches
}

/// What picking `row` asks core for: the newest build on a branch follows
/// it, and an older one pins it.
pub(crate) fn choice_for(listing: &Listing, row: &Listed) -> Choice {
    if !row.latest {
        return Choice::Build(row.build.clone());
    }
    if row.branch == listing.follow.branch && listing.follow.pinned.is_some() {
        return Choice::Latest;
    }
    match &row.branch {
        None => Choice::DefaultBranch,
        Some(branch) => Choice::Branch(branch.clone()),
    }
}

/// Whether picking `row` leaves the game as it is: its build installed,
/// followed the way that row would follow it.
pub(crate) fn is_current(listing: &Listing, row: &Listed) -> bool {
    let after = Follow {
        branch: row.branch.clone(),
        pinned: (!row.latest).then(|| row.build.clone()),
    };
    row.installed && after == listing.follow
}

/// Loads what the view shows for `name`: its builds on the Galaxy track,
/// else whether GOG has a Galaxy build to move it to. `entered` lists a
/// private branch with its password, which core saves once GOG lists the
/// branch with it.
pub(crate) fn load_with(
    paths: &Paths,
    keyring: &dyn Keyring,
    client: Client,
    name: &str,
    gog_id: u64,
    entered: Option<(&str, &Secret)>,
) -> Result<Loaded, String> {
    if paths.portable_root().is_some() {
        return Err(MigrateError::Portable.to_string());
    }
    let Some(mut session) =
        Session::open(paths, keyring, client).map_err(|error| error.to_string())?
    else {
        return Err(crate::flow::LOGGED_OUT.to_owned());
    };
    match listing::list(paths, &mut session, name, entered, false, false) {
        Ok(listing) => Ok(Loaded::Galaxy(Box::new(listing))),
        Err(error)
            if matches!(
                error.downcast_ref::<UpdateError>(),
                Some(UpdateError::NotGalaxy { .. })
            ) =>
        {
            let request = Request {
                name: name.to_owned(),
                gog_id,
                platform: Some(Platform::Windows),
                ..Request::default()
            };
            let track = core_galaxy::track_for(paths, &mut session, &request, None)
                .map_err(|error| format!("{error:#}"))?;
            Ok(Loaded::Installer {
                galaxy: track == Track::Galaxy,
            })
        }
        Err(error) => Err(format!("{error:#}")),
    }
}

/// Whether `card` gets a Version row: a GOG game that runs on Wine, since
/// Galaxy has no Linux builds.
pub(crate) fn has_version(card: &Card) -> bool {
    card.error.is_none()
        && card.kind == Some(CardKind::Wine)
        && card.gog.as_ref().is_some_and(|gog| gog.gog_id.is_some())
}

/// A build as the Version row shows it, with its pin or branch.
fn described(version: String, branch: Option<&str>, pinned: bool) -> String {
    match (pinned, branch) {
        (true, _) => format!("{version} (pinned)"),
        (false, Some(branch)) => format!("{version} (branch {branch})"),
        (false, None) => version,
    }
}

/// The Version row's value from a background check.
pub(crate) fn status_value(status: &Status) -> Option<String> {
    if status.track != Track::Galaxy {
        return status.installed.clone();
    }
    let installed = status.installed.as_deref()?;
    let version = core_galaxy::present::version(status.installed_name.as_deref(), installed);
    Some(described(
        version,
        status.branch.as_deref(),
        status.pinned.is_some(),
    ))
}

/// The Version row's value from a listing.
pub(crate) fn listing_value(listing: &Listing) -> Option<String> {
    let installed = listing.installed.as_deref()?;
    let name = listing
        .builds
        .iter()
        .find(|build| build.build == installed)
        .and_then(|build| build.version.as_deref());
    Some(described(
        core_galaxy::present::version(name, installed),
        listing.follow.branch.as_deref(),
        listing.follow.pinned.is_some(),
    ))
}

impl App {
    pub(crate) fn version_visible(&self) -> bool {
        self.panel_open
            && self
                .version
                .as_ref()
                .is_some_and(|picker| self.selected.as_deref() == Some(picker.name.as_str()))
    }

    pub(crate) fn close_version(&mut self) {
        if self.version.take().is_some() {
            self.version_generation = self.version_generation.wrapping_add(1);
        }
    }

    /// What the panel's Version row says for `name`.
    pub(crate) fn version_value(&self, name: &str) -> String {
        self.gog_versions
            .get(name)
            .cloned()
            .or_else(|| self.gog_updates.get(name).and_then(status_value))
            .unwrap_or_else(|| "Unknown".to_owned())
    }

    fn version_card(&self, name: &str) -> Option<&Card> {
        self.cards
            .iter()
            .find(|card| card.name == name)
            .filter(|card| has_version(card))
    }

    fn version_load(&mut self, entered: Option<(String, Secret)>) -> Task<Message> {
        let Some(picker) = self.version.as_mut() else {
            return Task::none();
        };
        let Some(gog_id) = self
            .cards
            .iter()
            .find(|card| card.name == picker.name)
            .and_then(|card| card.gog.as_ref()?.gog_id.as_deref()?.parse::<u64>().ok())
        else {
            return Task::none();
        };
        self.version_generation = self.version_generation.wrapping_add(1);
        let generation = self.version_generation;
        picker.generation = generation;
        picker.busy = true;
        picker.loading.reset();
        let reveal = picker.loading.add(1, Instant::now());
        let (paths, name) = (self.paths.clone(), picker.name.clone());
        let branch = entered.as_ref().map(|(branch, _)| branch.clone());
        let load = crate::task::blocking(
            move || {
                let client =
                    wineo_core::gog::client_from_env().map_err(|error| error.to_string())?;
                let keyring = wineo_core::secret_service::SecretServiceKeyring::gog();
                let entered = entered
                    .as_ref()
                    .map(|(branch, secret)| (branch.as_str(), secret));
                load_with(&paths, &keyring, client, &name, gog_id, entered)
            },
            move |result| Message::Version(Event::Loaded(generation, branch, result)),
        );
        Task::batch([reveal, load])
    }

    /// Opens the update page with `options` from the picker.
    fn version_review(&mut self, options: Options) -> Task<Message> {
        let Some(name) = self.version.as_ref().map(|picker| picker.name.clone()) else {
            return Task::none();
        };
        let enabled = self
            .version_card(&name)
            .is_some_and(|card| !card.finish_update() && self.gog_update_enabled(card));
        if !enabled {
            return Task::none();
        }
        self.open_gog_update(name, Some(options))
    }

    /// Whether the picker's actions can start for its game, as the view
    /// draws them from `facts`.
    fn version_actions_enabled(&self, facts: &dyn Facts, picker: &Picker) -> bool {
        !picker.busy
            && self.version_card(&picker.name).is_some_and(|card| {
                !card.finish_update() && self.gog_update_enabled_with(facts, card)
            })
    }
}

pub(crate) fn update(app: &mut App, event: Event) -> Task<Message> {
    match event {
        Event::Open(name) => {
            if app.flow.is_some()
                || app.removal.is_some()
                || app.trash_confirm_open()
                || app.downloads_confirm_open()
                || app.settings.is_some()
                || app.art_page.is_some()
                || app.close_prompt
            {
                return Task::none();
            }
            let Some(card) = app.version_card(&name) else {
                return Task::none();
            };
            let unfinished = card.finish_update();
            let target = card.default_target.clone();
            app.close_log();
            app.action_menu = None;
            if app.selected.as_deref() != Some(&name) {
                app.close_checkup();
                app.target = target;
                app.panel_error = None;
                app.panel_note = None;
            } else {
                app.close_checkup();
            }
            app.selected = Some(name.clone());
            if !app.panel_open {
                app.panel_open = true;
                app.panel_motion.open(Instant::now());
            }
            let mut picker = Picker::new(name.clone());
            // A branch or build waits for the unfinished operation, which
            // Finish update completes.
            if unfinished {
                picker.note = Some(GalaxyError::Unfinished { name: name.clone() }.to_string());
            }
            app.version = Some(picker);
            let art = app.load_panel_art();
            if unfinished {
                return art;
            }
            Task::batch([art, app.version_load(None)])
        }
        Event::Back => {
            app.close_version();
            Task::none()
        }
        Event::Loaded(generation, entered, result) => {
            // A listing is newer than any check of the game still on its way.
            if let (Some(picker), Ok(Loaded::Galaxy(_))) = (app.version.as_ref(), &result)
                && picker.generation == generation
            {
                let name = picker.name.clone();
                crate::gog_library::supersede(app, &name);
            }
            let Some(picker) = app
                .version
                .as_mut()
                .filter(|picker| picker.generation == generation)
            else {
                return Task::none();
            };
            picker.busy = false;
            picker.loading.finish_all();
            match result {
                Ok(loaded) => {
                    picker.error = None;
                    if let Loaded::Galaxy(listing) = &loaded {
                        if let Some(value) = listing_value(listing) {
                            app.gog_versions.insert(picker.name.clone(), value);
                        }
                        let listed = |name: &Option<String>| {
                            listing.builds.iter().any(|build| &build.branch == name)
                        };
                        match entered {
                            Some(branch) if listed(&Some(branch.clone())) => {
                                picker.branch = Some(branch);
                                picker.picked = None;
                                picker.private_open = false;
                                picker.private_branch.clear();
                                picker.private_error = None;
                            }
                            Some(branch) => {
                                picker.private_error = Some(
                                    GalaxyError::Refused {
                                        title: listing.title.clone(),
                                        branch,
                                    }
                                    .to_string(),
                                );
                            }
                            None => {
                                let followed = listing.follow.branch.clone();
                                picker.branch = if listed(&followed) {
                                    followed
                                } else {
                                    branches(listing)
                                        .first()
                                        .and_then(|branch| branch.name.clone())
                                };
                                picker.picked = None;
                            }
                        }
                        if !listed(&picker.branch) {
                            picker.branch = None;
                            picker.picked = None;
                        }
                    }
                    picker.loaded = Some(loaded);
                }
                Err(error) if entered.is_some() => picker.private_error = Some(error),
                Err(error) => {
                    picker.loaded = None;
                    picker.error = Some(error);
                }
            }
            Task::none()
        }
        Event::Reveal(generation) => {
            if let Some(picker) = app.version.as_mut() {
                picker.loading.reveal(generation);
            }
            Task::none()
        }
        Event::Branch(branch) => {
            if let Some(picker) = app.version.as_mut()
                && picker.branch != branch.name
            {
                picker.branch = branch.name;
                picker.picked = None;
            }
            Task::none()
        }
        Event::Pick(build) => {
            if let Some(picker) = app.version.as_mut()
                && picker.rows().iter().any(|row| row.build == build)
            {
                picker.picked = Some(build);
            }
            Task::none()
        }
        Event::Review => {
            let Some(choice) = app.version.as_ref().and_then(Picker::choice) else {
                return Task::none();
            };
            app.version_review(Options {
                choice,
                ..Options::default()
            })
        }
        Event::Repair => {
            if app.version.as_ref().and_then(Picker::listing).is_none() {
                return Task::none();
            }
            app.version_review(Options {
                repair: true,
                ..Options::default()
            })
        }
        Event::MoveToInstallers => {
            if app.version.as_ref().and_then(Picker::listing).is_none() {
                return Task::none();
            }
            app.version_review(Options {
                reinstall: true,
                track: Some(Track::Installer),
                ..Options::default()
            })
        }
        Event::MoveToGalaxy => {
            if !matches!(
                app.version
                    .as_ref()
                    .and_then(|picker| picker.loaded.as_ref()),
                Some(Loaded::Installer { galaxy: true })
            ) {
                return Task::none();
            }
            app.version_review(Options {
                reinstall: true,
                track: Some(Track::Galaxy),
                ..Options::default()
            })
        }
        Event::EnterPrivate => {
            if let Some(picker) = app.version.as_mut()
                && picker
                    .listing()
                    .is_some_and(|listing| listing.private_branches)
            {
                picker.private_open = true;
            }
            Task::none()
        }
        Event::PrivateBranch(branch) => {
            if let Some(picker) = app.version.as_mut() {
                picker.private_branch = branch;
            }
            Task::none()
        }
        Event::PrivatePassword(password) => {
            if let Some(picker) = app.version.as_mut() {
                picker.private_password = password;
            }
            Task::none()
        }
        Event::ListBuilds => {
            let Some(picker) = app.version.as_mut() else {
                return Task::none();
            };
            let branch = picker.private_branch.trim().to_owned();
            if !picker.private_open
                || picker.busy
                || branch.is_empty()
                || picker.private_password.expose().is_empty()
            {
                return Task::none();
            }
            // Kept only for this one request.
            let password = std::mem::replace(&mut picker.private_password, Secret::new(""));
            picker.private_error = None;
            app.version_load(Some((branch, password)))
        }
    }
}

impl App {
    /// The Version view, in the panel in place of the details.
    pub(crate) fn version_content(&self) -> Element<'_, Message> {
        let palette = self.palette;
        let Some(picker) = self.version.as_ref() else {
            return Space::new().into();
        };
        let mut body = column![
            row![
                secondary_button("Back", palette).on_press(send(Event::Back)),
                text("Version").size(14).color(palette.muted),
            ]
            .spacing(8)
            .align_y(Center)
        ]
        .spacing(12)
        .width(Fill);
        if picker.loading.shown().is_some() {
            body = body.push(
                row![
                    crate::spinner::spinner(palette, 16.0),
                    text("Checking builds with GOG...")
                        .size(14)
                        .color(palette.muted)
                ]
                .spacing(8)
                .align_y(Center),
            );
        }
        if let Some(note) = &picker.note {
            body = body.push(text(note).size(14).color(palette.warning));
        }
        if let Some(error) = &picker.error {
            body = body.push(text(error).size(14).color(palette.danger));
        }
        let enabled = self.version_actions_enabled(&self.facts, picker);
        match &picker.loaded {
            Some(Loaded::Galaxy(listing)) => {
                body = self.galaxy_view(body, picker, listing, enabled);
            }
            Some(Loaded::Installer { galaxy }) => {
                let title = self
                    .cards
                    .iter()
                    .find(|card| card.name == picker.name)
                    .map_or(picker.name.as_str(), |card| card.display_title.as_str());
                body =
                    body.push(text(format!("{title} updates from offline installers.")).size(14));
                if *galaxy {
                    body = body.push(
                        secondary_button("Move to Galaxy", palette)
                            .on_press_maybe(enabled.then_some(send(Event::MoveToGalaxy))),
                    );
                }
            }
            None => {}
        }
        body.into()
    }

    fn galaxy_view<'a>(
        &'a self,
        mut body: iced::widget::Column<'a, Message>,
        picker: &'a Picker,
        listing: &'a Listing,
        enabled: bool,
    ) -> iced::widget::Column<'a, Message> {
        let palette = self.palette;
        let branches = branches(listing);
        let shown = branches
            .iter()
            .find(|branch| branch.name == picker.branch)
            .cloned();
        if branches.is_empty() {
            body = body.push(
                text(
                    GalaxyError::NoBuild {
                        title: listing.title.clone(),
                    }
                    .to_string(),
                )
                .size(14)
                .color(palette.muted),
            );
        } else {
            body = body.push(
                row![
                    text("Branch:").size(14),
                    pick_list(branches, shown, |branch| send(Event::Branch(branch)))
                        .wineo_style(palette)
                        .width(Fill),
                ]
                .spacing(12)
                .align_y(Center),
            );
            body = body.push(list_group(
                picker.rows().into_iter().map(|build| {
                    let chosen = picker.picked.as_deref() == Some(build.build.as_str());
                    let mut details: Vec<&str> = Vec::new();
                    if let Some(date) = build.published_date() {
                        details.push(date);
                    }
                    if chosen {
                        details.push("selected");
                    }
                    details.extend(build.marks());
                    let version =
                        core_galaxy::present::version(build.version.as_deref(), &build.build);
                    let mut line = column![list_row_text(version).color(if chosen {
                        palette.primary
                    } else {
                        palette.foreground
                    })]
                    .spacing(2);
                    if !details.is_empty() {
                        line = line.push(text(details.join(", ")).size(12).color(palette.muted));
                    }
                    list_row(line, palette).on_press(send(Event::Pick(build.build.clone())))
                }),
                palette,
            ));
        }
        for warning in listing
            .warnings
            .iter()
            .filter(|warning| picker.private_error.as_ref() != Some(*warning))
        {
            body = body.push(text(warning).size(13).color(palette.warning));
        }
        if listing.private_branches {
            body = body.push(
                text("GOG also has private branches.")
                    .size(13)
                    .color(palette.muted),
            );
            if picker.private_open {
                let ready = !picker.busy
                    && !picker.private_branch.trim().is_empty()
                    && !picker.private_password.expose().is_empty();
                body = body.push(
                    text_input("Branch name", &picker.private_branch)
                        .on_input(|value| send(Event::PrivateBranch(value)))
                        .on_submit(send(Event::ListBuilds))
                        .padding([6, 10])
                        .width(Fill)
                        .style(move |_, status| crate::chips::input_style(palette, status)),
                );
                body = body.push(
                    text_input("Password", picker.private_password.expose())
                        .secure(true)
                        .on_input(|value| send(Event::PrivatePassword(Secret::new(value))))
                        .on_submit(send(Event::ListBuilds))
                        .padding([6, 10])
                        .width(Fill)
                        .style(move |_, status| crate::chips::input_style(palette, status)),
                );
                body = body.push(
                    secondary_button("List builds", palette)
                        .on_press_maybe(ready.then_some(send(Event::ListBuilds))),
                );
                if let Some(error) = &picker.private_error {
                    body = body.push(text(error).size(13).color(palette.danger));
                }
            } else {
                body = body.push(
                    secondary_button("Enter a private branch", palette)
                        .on_press(send(Event::EnterPrivate)),
                );
            }
        }
        body.push(crate::ui::separator(palette)).push(
            row![
                secondary_button("Repair", palette)
                    .on_press_maybe(enabled.then_some(send(Event::Repair))),
                secondary_button("Move to offline installers", palette)
                    .on_press_maybe(enabled.then_some(send(Event::MoveToInstallers))),
                accent_base("Review", palette.primary).on_press_maybe(
                    (enabled && picker.choice().is_some()).then_some(send(Event::Review)),
                ),
            ]
            .spacing(8)
            .align_y(Center)
            .wrap()
            .vertical_spacing(8),
        )
    }
}

#[cfg(test)]
mod tests;
