use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use iced::widget::{button, checkbox, column, container, image, pick_list, row, text, text_input};
use iced::{ContentFit, Element, Task, border};
use wineo_core::art::{self, Kind};
use wineo_core::art_fetch::{self, Mode};
use wineo_core::config::{ArtFetch, GlobalConfig};
use wineo_core::download::Https;
use wineo_core::paths::Paths;
use wineo_core::secret_service::SecretServiceKeyring;
use wineo_core::steamgriddb::{self, Api, Game};

use crate::theme::Palette;

#[derive(Clone, Debug)]
pub(crate) enum Event {
    Path(Kind, String),
    Browse(Kind),
    Picked(Kind, u64, Result<Option<PathBuf>, String>),
    Preview(Kind, u64, Result<image::Handle, String>),
    Clear(Kind),
    ManualEntry(bool),
    Configured(u64, Result<(ArtFetch, bool), String>),
    Fetch(bool),
    SearchText(String),
    Search,
    Searched(u64, Result<Vec<Game>, String>),
    Game(Match),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Match(Game);

impl std::fmt::Display for Match {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.0.year {
            Some(year) => write!(f, "{} ({year})", self.0.name),
            None => f.write_str(&self.0.name),
        }
    }
}

#[derive(Clone, Default)]
struct Local {
    path: String,
    sequence: u64,
    chooser_sequence: u64,
    preview: Option<image::Handle>,
    error: Option<String>,
    checking: bool,
}

#[derive(Clone, Default)]
pub(crate) struct Draft {
    paths: Option<Paths>,
    local: BTreeMap<Kind, Local>,
    fetch: bool,
    kinds: Vec<Kind>,
    key_available: bool,
    search: String,
    search_edited: bool,
    searching: bool,
    matches: Vec<Match>,
    selected: Option<Match>,
    sequence: u64,
    error: Option<String>,
    configuration_sequence: u64,
    manual_entry: bool,
}

impl Draft {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn is_dirty(&self) -> bool {
        self.fetch
            || self.search_edited
            || self.selected.is_some()
            || self.local.values().any(|local| !local.path.is_empty())
    }

    pub(crate) fn set_title(&mut self, title: &str) -> Task<Event> {
        if !self.search_edited && self.search != title {
            self.search = title.to_owned();
            self.sequence = self.sequence.wrapping_add(1);
            self.searching = false;
            self.matches.clear();
            self.selected = None;
            self.error = None;
        }
        Task::none()
    }

    pub(crate) fn configure(&mut self, paths: Paths, title: String) -> Task<Event> {
        self.paths = Some(paths.clone());
        if self.search.is_empty() {
            self.search = title;
        }
        self.configuration_sequence = self.configuration_sequence.wrapping_add(1);
        let sequence = self.configuration_sequence;
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    let global =
                        GlobalConfig::load_or_default(&paths).map_err(|e| e.to_string())?;
                    let available = steamgriddb::find_key(
                        &paths,
                        |key| std::env::var_os(key),
                        &SecretServiceKeyring::steamgriddb(),
                    )
                    .map_err(|e| e.to_string())?
                    .is_some();
                    Ok((global.art.fetch, available))
                })
                .await
                .unwrap_or_else(|e| Err(e.to_string()))
            },
            move |result| Event::Configured(sequence, result),
        )
    }

    pub(crate) fn update(&mut self, event: Event) -> Task<Event> {
        match event {
            Event::Path(kind, value) => {
                let paths = self.paths.clone();
                let local = self.local.entry(kind).or_default();
                local.path = value.clone();
                local.sequence = local.sequence.wrapping_add(1);
                local.chooser_sequence = local.chooser_sequence.wrapping_add(1);
                local.preview = None;
                local.error = None;
                local.checking = !value.is_empty();
                let sequence = local.sequence;
                if value.is_empty() {
                    return Task::none();
                }
                return Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || {
                            let path = resolve(paths.as_ref(), &value)?;
                            preview(&path, kind)
                        })
                        .await
                        .unwrap_or_else(|e| Err(e.to_string()))
                    },
                    move |result| Event::Preview(kind, sequence, result),
                );
            }
            Event::Browse(kind) => {
                let local = self.local.entry(kind).or_default();
                local.chooser_sequence = local.chooser_sequence.wrapping_add(1);
                let sequence = local.chooser_sequence;
                return Task::perform(
                    crate::picker::pick(format!("Select {kind} artwork"), false),
                    move |r| Event::Picked(kind, sequence, r),
                );
            }
            Event::Picked(kind, sequence, result) => {
                if self.local.entry(kind).or_default().chooser_sequence != sequence {
                    return Task::none();
                }
                match result {
                    Ok(Some(path)) => {
                        return self.update(Event::Path(kind, path.to_string_lossy().into_owned()));
                    }
                    Ok(None) => {}
                    Err(_) => self.manual_entry = true,
                }
            }
            Event::Preview(kind, sequence, result) => {
                let local = self.local.entry(kind).or_default();
                if local.sequence == sequence {
                    local.checking = false;
                    match result {
                        Ok(handle) => local.preview = Some(handle),
                        Err(error) => local.error = Some(error),
                    }
                }
            }
            Event::Clear(kind) => return self.update(Event::Path(kind, String::new())),
            Event::ManualEntry(open) => self.manual_entry = open,
            Event::Configured(sequence, result) if sequence == self.configuration_sequence => {
                match result {
                    Ok((kinds, available)) => {
                        self.kinds = kinds.kinds();
                        self.key_available = available;
                    }
                    Err(error) => self.error = Some(error),
                }
            }
            Event::Fetch(fetch) => {
                self.fetch = fetch && self.key_available && !self.kinds.is_empty()
            }
            Event::SearchText(value) => {
                self.search = value;
                self.search_edited = true;
                self.sequence = self.sequence.wrapping_add(1);
                self.searching = false;
                self.selected = None;
                self.matches.clear();
            }
            Event::Search
                if self.key_available && !self.searching && !self.search.trim().is_empty() =>
            {
                let Some(paths) = self.paths.clone() else {
                    return Task::none();
                };
                self.sequence = self.sequence.wrapping_add(1);
                let sequence = self.sequence;
                let term = self.search.clone();
                self.searching = true;
                self.error = None;
                return Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || search(&paths, &term))
                            .await
                            .unwrap_or_else(|e| Err(e.to_string()))
                    },
                    move |result| Event::Searched(sequence, result),
                );
            }
            Event::Searched(sequence, result) if sequence == self.sequence => {
                self.searching = false;
                match result {
                    Ok(games) => {
                        self.matches = games.into_iter().map(Match).collect();
                        self.selected = self.matches.first().cloned();
                        self.error = self
                            .matches
                            .is_empty()
                            .then(|| "No matches found".to_owned());
                    }
                    Err(error) => self.error = Some(error),
                }
            }
            Event::Game(game) => self.selected = Some(game),
            _ => {}
        }
        Task::none()
    }

    pub(crate) fn validate(&self) -> Result<(), String> {
        for (kind, local) in &self.local {
            if local.checking {
                return Err(format!("Checking {kind} artwork..."));
            }
            if !local.path.is_empty()
                && let Some(error) = &local.error
            {
                return Err(format!("{kind}: {error}"));
            }
        }
        if self.fetch && self.selected.is_none() {
            return Err("Search SteamGridDB and choose a match, or turn fetching off".to_owned());
        }
        Ok(())
    }

    pub(crate) fn apply(&self, paths: &Paths, name: &str) -> Result<Vec<String>, String> {
        let mut notes = Vec::new();
        let mut failures = Vec::new();
        for (kind, local) in &self.local {
            if local.path.is_empty() {
                continue;
            }
            let result = resolve(Some(paths), &local.path).and_then(|path| {
                art::set_manual(paths, name, None, *kind, &path).map_err(|e| e.to_string())
            });
            if let Err(error) = result {
                failures.push(format!("Could not store {kind}: {error}"));
            }
        }
        if self.fetch
            && let Some(game) = &self.selected
        {
            let kinds: Vec<_> = self
                .kinds
                .iter()
                .copied()
                .filter(|kind| {
                    self.local
                        .get(kind)
                        .is_none_or(|local| local.path.is_empty())
                })
                .collect();
            match if kinds.is_empty() {
                Ok(Vec::new())
            } else {
                fetch(paths, name, &game.0, kinds.into_iter())
            } {
                Ok(fetched) => notes.extend(fetched),
                Err(error) => failures.push(format!("Art fetching failed: {error}")),
            }
        }
        if failures.is_empty() {
            Ok(notes)
        } else {
            Err(failures.join("\n"))
        }
    }

    pub(crate) fn view(&self, palette: Palette) -> Element<'_, Event> {
        let mut content = column![
            text("Artwork").size(19),
            text("Click a tile to choose an image. All four are optional.")
                .size(12)
                .color(palette.muted),
            column![
                self.tile(Kind::Hero, "Hero", "Wide background", 356, 112, palette),
                row![
                    column![
                        self.tile(Kind::Logo, "Logo", "Title graphic", 196, 78, palette),
                        self.tile(Kind::Icon, "Icon", "Small square", 98, 98, palette),
                    ]
                    .spacing(10),
                    self.tile(Kind::Grid, "Poster", "Cover art", 148, 222, palette),
                ]
                .spacing(12),
            ]
            .spacing(10),
            button(if self.manual_entry {
                "Hide file paths"
            } else {
                "Enter a file path instead"
            })
            .on_press(Event::ManualEntry(!self.manual_entry)),
        ]
        .spacing(10);
        if self.manual_entry {
            for kind in Kind::ALL {
                let value = self
                    .local
                    .get(&kind)
                    .map_or("", |local| local.path.as_str());
                content = content.push(
                    row![
                        text(format!("{kind}")).width(55),
                        text_input("Image file path", value)
                            .on_input(move |value| Event::Path(kind, value)),
                    ]
                    .spacing(8),
                );
            }
        }
        if !self.kinds.is_empty() {
            if self.key_available {
                content = content.push(
                    checkbox(self.fetch)
                        .label("Fetch missing artwork from SteamGridDB")
                        .on_toggle(Event::Fetch),
                );
                if self.fetch {
                    content = content.push(
                        row![
                            text_input("Game title", &self.search).on_input(Event::SearchText),
                            button(if self.searching {
                                "Searching..."
                            } else {
                                "Search"
                            })
                            .on_press_maybe((!self.searching).then_some(Event::Search)),
                        ]
                        .spacing(8),
                    );
                    if !self.matches.is_empty() {
                        content = content.push(pick_list(
                            self.matches.clone(),
                            self.selected.clone(),
                            Event::Game,
                        ));
                    }
                }
            } else {
                content = content.push(text("SteamGridDB fetching needs a configured key. Local artwork is always available.").color(palette.muted));
            }
        }
        if let Some(error) = &self.error {
            content = content.push(text(error).color(palette.danger));
        }
        content.into()
    }

    fn tile<'a>(
        &'a self,
        kind: Kind,
        title: &'static str,
        hint: &'static str,
        width: u32,
        height: u32,
        palette: Palette,
    ) -> Element<'a, Event> {
        let local = self.local.get(&kind);
        let selected = local.is_some_and(|local| !local.path.is_empty());
        let picture: Element<'_, Event> = match local.and_then(|local| local.preview.as_ref()) {
            Some(handle) => image(handle.clone())
                .width(width - 16)
                .height(height - 16)
                .content_fit(ContentFit::Contain)
                .into(),
            None => container(
                column![
                    text(title).size(if kind == Kind::Grid { 20 } else { 16 }),
                    text(hint).size(11).color(palette.muted),
                    text("Choose image").size(11).color(palette.primary),
                ]
                .align_x(iced::Center)
                .spacing(6),
            )
            .width(width - 16)
            .height(height - 16)
            .center(iced::Fill)
            .into(),
        };
        let chooser = button(picture)
            .width(width)
            .height(height)
            .padding(8)
            .on_press(Event::Browse(kind))
            .style(move |_, status| button::Style {
                background: Some(palette.surface.into()),
                text_color: palette.foreground,
                border: border::rounded(8).width(1).color(
                    if matches!(status, button::Status::Hovered) {
                        palette.primary
                    } else if selected {
                        palette.success
                    } else {
                        palette.border
                    },
                ),
                ..button::Style::default()
            });
        let mut tile = column![text(title).size(13), chooser]
            .spacing(5)
            .width(width);
        if let Some(local) = local {
            if local.checking {
                tile = tile.push(text("Checking image...").size(11).color(palette.muted));
            } else if let Some(error) = &local.error {
                tile = tile.push(text(error).size(11).color(palette.danger));
            }
            if selected {
                tile = tile.push(button("Remove").on_press(Event::Clear(kind)).padding(2));
            }
        }
        tile.into()
    }
}

fn resolve(paths: Option<&Paths>, value: &str) -> Result<PathBuf, String> {
    match paths {
        Some(paths) => paths.config_path(value).map_err(|e| e.to_string()),
        None if Path::new(value).is_absolute() => Ok(PathBuf::from(value)),
        None => Err("Enter an absolute file path".to_owned()),
    }
}

fn preview(path: &Path, kind: Kind) -> Result<image::Handle, String> {
    use std::io::Read;
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    file.take(art::MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > art::MAX_BYTES {
        return Err("Artwork exceeds 32 MiB".to_owned());
    }
    let format = art::detect(&bytes)
        .filter(|format| kind.accepts(*format))
        .ok_or_else(|| format!("Unsupported image format for {kind}"))?;
    if format == art::Format::Svg {
        let tree = resvg::usvg::Tree::from_data(&bytes, &resvg::usvg::Options::default())
            .map_err(|e| e.to_string())?;
        let size = tree.size();
        let scale = (160.0 / size.width()).min(100.0 / size.height()).min(1.0);
        let width = (size.width() * scale).ceil().max(1.0) as u32;
        let height = (size.height() * scale).ceil().max(1.0) as u32;
        let mut pixels =
            resvg::tiny_skia::Pixmap::new(width, height).ok_or("Invalid image dimensions")?;
        resvg::render(
            &tree,
            resvg::tiny_skia::Transform::from_scale(scale, scale),
            &mut pixels.as_mut(),
        );
        let mut rgba = pixels.take();
        for pixel in rgba.as_chunks_mut::<4>().0 {
            let alpha = u16::from(pixel[3]);
            for channel in &mut pixel[..3] {
                *channel = (u16::from(*channel) * 255)
                    .checked_div(alpha)
                    .unwrap_or(0)
                    .min(255) as u8;
            }
        }
        Ok(image::Handle::from_rgba(width, height, rgba))
    } else {
        let decoded = ::image::load_from_memory(&bytes)
            .map_err(|e| e.to_string())?
            .thumbnail(160, 100)
            .into_rgba8();
        Ok(image::Handle::from_rgba(
            decoded.width(),
            decoded.height(),
            decoded.into_raw(),
        ))
    }
}

fn search(paths: &Paths, term: &str) -> Result<Vec<Game>, String> {
    let key = steamgriddb::require_key(
        paths,
        |key| std::env::var_os(key),
        &SecretServiceKeyring::steamgriddb(),
    )
    .map_err(|e| e.to_string())?;
    let https = Https::new().map_err(|e| e.to_string())?;
    Api::new(&https, &key.value)
        .search(term)
        .map_err(|e| e.to_string())
}

fn fetch(
    paths: &Paths,
    name: &str,
    game: &Game,
    kinds: impl Iterator<Item = Kind>,
) -> Result<Vec<String>, String> {
    let key = steamgriddb::require_key(
        paths,
        |key| std::env::var_os(key),
        &SecretServiceKeyring::steamgriddb(),
    )
    .map_err(|e| e.to_string())?;
    let https = Https::new().map_err(|e| e.to_string())?;
    let api = Api::new(&https, &key.value);
    let language = steamgriddb::language(|key| std::env::var_os(key));
    let mut notes = Vec::new();
    let mut failures = Vec::new();
    art::save_game(paths, name, game.id).map_err(|e| e.to_string())?;
    for kind in kinds {
        match art_fetch::fetch_kind(
            paths,
            name,
            None,
            kind,
            &api,
            game.id,
            &language,
            Mode::Fill,
        ) {
            Ok(art_fetch::Fetched::NoneFound) => {
                notes.push(format!("No {kind} artwork found for {}", game.name))
            }
            Ok(_) => {}
            Err(error) => failures.push(format!("Could not fetch {kind}: {error}")),
        }
    }
    if failures.is_empty() {
        Ok(notes)
    } else {
        Err(failures.join("\n"))
    }
}

pub(crate) fn fetch_default(paths: &Paths, name: &str, title: &str) -> Result<Vec<String>, String> {
    let global = GlobalConfig::load_or_default(paths).map_err(|e| e.to_string())?;
    let kinds = global.art.fetch.kinds();
    if kinds.is_empty() {
        return Ok(Vec::new());
    }
    match search(paths, title)?.into_iter().next() {
        Some(game) => fetch(paths, name, &game, kinds.into_iter()),
        None => Ok(vec![format!("No artwork match for {title}")]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wineo_core::environment::Environment;

    #[test]
    fn typing_title_does_not_discard_pending_art_configuration() {
        let mut draft = Draft::new();
        draft.configuration_sequence = 1;
        let _ = draft.set_title("new title");
        let _ = draft.update(Event::Configured(1, Ok((ArtFetch::default(), true))));
        assert!(draft.key_available);
        assert!(!draft.kinds.is_empty());
    }

    fn paths(root: &Path) -> Paths {
        Paths::from_lookup(|key| (key == "HOME").then(|| root.as_os_str().to_owned())).unwrap()
    }

    #[test]
    fn local_art_works_without_fetching_and_preserves_source_bytes() {
        let root = tempfile::tempdir().unwrap();
        let paths = paths(root.path());
        Environment::create_native(&paths, &GlobalConfig::default(), "game", None, false).unwrap();
        let source = root.path().join("my-image.data");
        let bytes = br#"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="20"><rect width="20" height="20" fill="red"/></svg>"#;
        std::fs::write(&source, bytes).unwrap();
        let mut draft = Draft::new();
        draft.local.insert(
            Kind::Icon,
            Local {
                path: source.to_string_lossy().into_owned(),
                ..Local::default()
            },
        );
        assert!(!draft.fetch);
        assert!(draft.kinds.is_empty());
        assert!(draft.validate().is_ok());
        assert!(preview(&source, Kind::Icon).is_ok());
        assert!(draft.apply(&paths, "game").unwrap().is_empty());
        draft.fetch = true;
        draft.kinds = vec![Kind::Icon];
        draft.selected = Some(Match(Game {
            id: 123,
            name: "game".into(),
            year: None,
        }));
        assert!(draft.apply(&paths, "game").unwrap().is_empty());
        assert_eq!(std::fs::read(&source).unwrap(), bytes);
        assert_eq!(
            std::fs::read(paths.art_dir("game").join("icon.svg")).unwrap(),
            bytes
        );
        assert_eq!(
            art::ArtConfig::load(&paths.art_dir("game")).unwrap().kinds[&Kind::Icon].source,
            art::Source::Manual("my-image.data".to_owned())
        );
    }

    #[test]
    fn stale_previews_and_chooser_results_cannot_replace_an_edit() {
        let mut draft = Draft::new();
        draft.local.insert(
            Kind::Grid,
            Local {
                path: "/new.png".to_owned(),
                sequence: 3,
                chooser_sequence: 3,
                ..Local::default()
            },
        );
        let _ = draft.update(Event::Picked(
            Kind::Grid,
            2,
            Ok(Some(PathBuf::from("/old.png"))),
        ));
        let _ = draft.update(Event::Preview(Kind::Grid, 2, Err("old error".to_owned())));
        assert_eq!(draft.local[&Kind::Grid].path, "/new.png");
        assert!(draft.local[&Kind::Grid].error.is_none());
        let _ = draft.update(Event::Picked(Kind::Grid, 3, Ok(None)));
        assert_eq!(draft.local[&Kind::Grid].path, "/new.png");
        draft.local.get_mut(&Kind::Grid).unwrap().checking = true;
        let _ = draft.update(Event::Browse(Kind::Grid));
        let _ = draft.update(Event::Picked(Kind::Grid, 4, Ok(None)));
        assert!(!draft.manual_entry);
        let _ = draft.update(Event::Browse(Kind::Grid));
        let _ = draft.update(Event::Picked(
            Kind::Grid,
            5,
            Err("portal unavailable".to_owned()),
        ));
        assert!(draft.manual_entry);
        assert_eq!(draft.local[&Kind::Grid].path, "/new.png");
        assert!(draft.local[&Kind::Grid].error.is_none());
        let _ = draft.update(Event::Preview(
            Kind::Grid,
            3,
            Ok(image::Handle::from_rgba(1, 1, vec![0; 4])),
        ));
        assert!(!draft.local[&Kind::Grid].checking);
        assert!(draft.local[&Kind::Grid].preview.is_some());
    }

    #[test]
    fn title_change_refreshes_unedited_search_without_accepting_stale_results() {
        let mut draft = Draft::new();
        draft.search = "Old title".to_owned();
        draft.searching = true;
        draft.sequence = 5;
        let _ = draft.set_title("New title");
        assert_eq!(draft.search, "New title");
        assert!(!draft.searching);
        let _ = draft.update(Event::Searched(
            5,
            Ok(vec![Game {
                id: 1,
                name: "Old title".to_owned(),
                year: None,
            }]),
        ));
        assert!(draft.matches.is_empty());
        let _ = draft.update(Event::SearchText("Custom match".to_owned()));
        let _ = draft.set_title("Third title");
        assert_eq!(draft.search, "Custom match");
    }

    #[test]
    fn failed_art_does_not_remove_registration() {
        let root = tempfile::tempdir().unwrap();
        let paths = paths(root.path());
        Environment::create_native(&paths, &GlobalConfig::default(), "game", None, false).unwrap();
        let mut draft = Draft::new();
        draft.local.insert(
            Kind::Logo,
            Local {
                path: root
                    .path()
                    .join("missing.png")
                    .to_string_lossy()
                    .into_owned(),
                ..Local::default()
            },
        );
        assert!(draft.apply(&paths, "game").is_err());
        assert!(Environment::load(&paths, "game").is_ok());
    }
}
