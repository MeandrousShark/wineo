use iced::keyboard::{Key, Modifiers};
use iced::widget::{Space, button, column, container, row, text};
use iced::{Center, Element, Fill, Task, border};
use wineo_core::paths::Paths;
use wineo_core::trash::{self, Phase};

use crate::{App, Message};

pub(crate) const WIDTH: f32 = 140.0;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum Section {
    #[default]
    Library,
    Trash,
    Checkup,
}

impl Section {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Library => "Library",
            Self::Trash => "Trash",
            Self::Checkup => "Checkup",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct TrashBadge {
    pub(crate) count: Option<usize>,
    pub(crate) warning: bool,
}

impl TrashBadge {
    pub(crate) fn read(paths: &Paths) -> Self {
        let entries = trash::list(paths);
        let pending = trash::pending_entries(paths);
        Self {
            count: entries.as_ref().ok().map(|entries| {
                entries
                    .iter()
                    .filter(|entry| entry.phase == Phase::Trashed)
                    .count()
            }),
            warning: entries.is_err()
                || pending.as_ref().map_or(true, |entries| !entries.is_empty()),
        }
    }
}

pub(crate) fn shortcut(key: &Key, modifiers: Modifiers) -> Option<Section> {
    if !modifiers.control() || modifiers.alt() || modifiers.logo() || modifiers.shift() {
        return None;
    }
    match key.as_ref() {
        Key::Character("1") => Some(Section::Library),
        Key::Character("2") => Some(Section::Trash),
        Key::Character("3") => Some(Section::Checkup),
        _ => None,
    }
}

impl App {
    pub(crate) fn load_trash_badge(paths: Paths) -> Task<Message> {
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || TrashBadge::read(&paths))
                    .await
                    .unwrap_or(TrashBadge {
                        count: None,
                        warning: true,
                    })
            },
            Message::TrashBadgeLoaded,
        )
    }

    fn sidebar_blocked(&self) -> bool {
        self.close_prompt
            || self.flow.is_some()
            || self
                .settings
                .as_ref()
                .is_some_and(|page| page.navigation_blocked())
    }

    pub(crate) fn navigate_section(&mut self, section: Section) -> Task<Message> {
        if self.sidebar_blocked() {
            return Task::none();
        }
        if self.settings.is_some() {
            let task = self.settings_update(crate::settings::Event::Back);
            if let Some(page) = &self.settings {
                if page.leave_prompt {
                    self.pending_section = Some(section);
                }
                return task;
            }
            return Task::batch([task, self.switch_section(section)]);
        }
        self.switch_section(section)
    }

    pub(crate) fn switch_section(&mut self, section: Section) -> Task<Message> {
        self.section = section;
        self.action_menu = None;
        self.hovered = None;
        self.search_focused = false;
        Task::batch([
            iced::advanced::widget::operate(iced::advanced::widget::operation::focusable::unfocus()),
            if section == Section::Library {
                self.load_visible_art()
            } else {
                Task::none()
            },
        ])
    }

    pub(crate) fn sidebar(&self) -> Element<'_, Message> {
        let palette = self.palette;
        let blocked = self.sidebar_blocked();
        let rows = [Section::Library, Section::Trash, Section::Checkup].map(|section| {
            let mut label = row![text(section.label()).size(14), Space::new().width(Fill)]
                .spacing(6)
                .align_y(Center);
            if section == Section::Trash {
                if let Some(count) = self.trash_badge.count {
                    label = label.push(text(count).size(12).color(palette.muted));
                }
                if self.trash_badge.warning {
                    label =
                        label.push(container(Space::new()).width(8).height(8).style(move |_| {
                            container::Style::default()
                                .background(palette.warning)
                                .border(border::rounded(4))
                        }));
                }
            }
            let selected = self.section == section;
            button(label)
                .on_press_maybe((!blocked).then_some(Message::Section(section)))
                .width(Fill)
                .padding([10, 8])
                .style(move |_, status| {
                    let mut style = crate::chips::secondary(palette, status);
                    if selected {
                        style.background = Some(palette.primary.scale_alpha(0.15).into());
                    }
                    style
                })
                .into()
        });
        container(column(rows).spacing(6))
            .width(WIDTH)
            .height(Fill)
            .padding(8)
            .style(move |_| container::Style::default().background(palette.surface))
            .into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::MetadataExt;

    use iced::event::Status;
    use sha2::{Digest, Sha256};

    fn fixture() -> (tempfile::TempDir, App) {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().to_owned();
        let paths =
            Paths::from_lookup(move |key| (key == "HOME").then(|| home.as_os_str().to_owned()))
                .unwrap();
        (root, App::boot(paths).0)
    }

    fn key(app: &mut App, digit: &str) {
        crate::shortcuts::tests::press(app, digit, Modifiers::CTRL);
    }

    #[test]
    fn sections_switch_by_message_and_control_number() {
        let (_root, mut app) = fixture();
        assert_eq!(app.section, Section::Library);
        for (digit, section) in [
            ("2", Section::Trash),
            ("3", Section::Checkup),
            ("1", Section::Library),
        ] {
            let _ = app.update(Message::Section(section));
            assert_eq!(app.section, section);
            let _ = app.update(Message::Section(if section == Section::Library {
                Section::Checkup
            } else {
                Section::Library
            }));
            key(&mut app, digit);
            assert_eq!(app.section, section);
        }
    }

    #[test]
    fn captured_keys_and_extra_modifiers_do_not_switch_sections() {
        let (_root, mut app) = fixture();
        for digit in ["1", "2", "3"] {
            let _ = app.update(Message::Key(
                Key::Character(digit.into()),
                Modifiers::CTRL,
                Status::Captured,
            ));
            let _ = app.update(Message::KeyWithFocus(
                Key::Character(digit.into()),
                Modifiers::CTRL,
                Status::Captured,
                true,
            ));
            assert_eq!(app.section, Section::Library);
        }
        for modifiers in [
            Modifiers::empty(),
            Modifiers::CTRL | Modifiers::ALT,
            Modifiers::CTRL | Modifiers::SHIFT,
        ] {
            assert_eq!(shortcut(&Key::Character("2".into()), modifiers), None);
        }
    }

    #[test]
    fn overlays_and_close_prompt_isolate_section_navigation() {
        let (_root, mut app) = fixture();
        let _ = app.update(Message::Add);
        for choice in [
            None,
            Some(crate::flow::Event::New),
            Some(crate::flow::Event::Prefix),
            Some(crate::flow::Event::Heroic),
        ] {
            app.flow = Some(crate::flow::Page::choose(app.paths.clone()));
            if let Some(choice) = choice {
                let _ = app.update(Message::Flow(app.flow_generation, choice));
            }
            let kind = std::mem::discriminant(app.flow.as_ref().unwrap());
            for digit in ["1", "2", "3"] {
                key(&mut app, digit);
                let _ = app.update(Message::KeyWithFocus(
                    Key::Character(digit.into()),
                    Modifiers::CTRL,
                    Status::Ignored,
                    false,
                ));
                let _ = app.update(Message::Section(Section::Trash));
                assert_eq!(app.section, Section::Library);
                assert_eq!(std::mem::discriminant(app.flow.as_ref().unwrap()), kind);
            }
        }
        app.flow = Some(crate::flow::Page::clone(app.paths.clone(), "game".into()).0);
        for digit in ["1", "2", "3"] {
            key(&mut app, digit);
            let _ = app.update(Message::Section(Section::Checkup));
            assert_eq!(app.section, Section::Library);
        }
        app.flow = None;
        app.close_prompt = true;
        for digit in ["1", "2", "3"] {
            key(&mut app, digit);
            let _ = app.update(Message::KeyWithFocus(
                Key::Character(digit.into()),
                Modifiers::CTRL,
                Status::Ignored,
                false,
            ));
            let _ = app.update(Message::Section(Section::Checkup));
            assert_eq!(app.section, Section::Library);
        }
    }

    fn remove_native(paths: &Paths, name: &str) -> trash::Entry {
        let config = paths.environment_config(name);
        fs::create_dir_all(config.parent().unwrap()).unwrap();
        fs::write(config, "schema_version = 1\nkind = 'native'\n").unwrap();
        trash::remove(paths, name, false).unwrap()
    }

    fn make_pending(paths: &Paths, entry: &trash::Entry, phase: &str, intent: &str) {
        let file = paths.trash_dir().join(&entry.id).join("manifest.toml");
        let manifest = fs::read_to_string(&file)
            .unwrap()
            .replace("phase = \"trashed\"", &format!("phase = \"{phase}\""));
        fs::write(file, &manifest).unwrap();
        let nested = manifest
            .lines()
            .map(|line| {
                if let Some(header) = line.strip_prefix("[[") {
                    format!("[[manifest.{header}\n")
                } else if let Some(header) = line.strip_prefix('[') {
                    format!("[manifest.{header}\n")
                } else {
                    format!("{line}\n")
                }
            })
            .collect::<String>();
        let root = paths.environments_dir().parent().unwrap().to_owned();
        let metadata = fs::metadata(paths.trash_dir()).unwrap();
        let marker = format!(
            "version = 1\nintent = '{intent}'\nfinished = false\nroot = '{}'\n\n[metadata_identity]\ndevice = {}\ninode = {}\n\n[manifest]\n{nested}",
            root.display(),
            metadata.dev(),
            metadata.ino(),
        );
        let hash = Sha256::digest(root.as_os_str().as_encoded_bytes());
        let filename = format!(
            "trash-{}-{:016x}.toml",
            entry.id,
            u64::from_le_bytes(hash[..8].try_into().unwrap())
        );
        let dir = paths.prefix_locks_dir().join(".trash-pending");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(filename), marker).unwrap();
    }

    #[test]
    fn badge_counts_only_trashed_entries_and_warns_about_pending_work() {
        let (_root, mut app) = fixture();
        let _ = app.update(Message::TrashBadgeLoaded(TrashBadge::read(&app.paths)));
        assert_eq!(
            app.trash_badge,
            TrashBadge {
                count: Some(0),
                warning: false
            }
        );
        remove_native(&app.paths, "kept");
        let removing = remove_native(&app.paths, "removing");
        let restoring = remove_native(&app.paths, "restoring");
        let _ = app.update(Message::TrashBadgeLoaded(TrashBadge::read(&app.paths)));
        assert_eq!(
            app.trash_badge,
            TrashBadge {
                count: Some(3),
                warning: false
            }
        );
        make_pending(&app.paths, &removing, "removing", "remove");
        make_pending(&app.paths, &restoring, "restoring", "restore");
        assert_eq!(trash::pending_entries(&app.paths).unwrap().len(), 2);
        assert_eq!(trash::list(&app.paths).unwrap().len(), 3);
        let _ = app.update(Message::TrashBadgeLoaded(TrashBadge::read(&app.paths)));
        assert_eq!(
            app.trash_badge,
            TrashBadge {
                count: Some(1),
                warning: true
            }
        );
        assert!(!app.trash_refreshing);
        let _ = app.update(Message::Refresh);
        assert!(app.trash_refreshing);
    }

    #[test]
    fn unreadable_trash_or_pending_markers_warn_instead_of_claiming_empty() {
        let (_root, mut app) = fixture();
        let dir = app.paths.trash_dir();
        fs::create_dir_all(dir.parent().unwrap()).unwrap();
        fs::write(&dir, "unreadable storage").unwrap();
        assert!(trash::list(&app.paths).is_err());
        let _ = app.update(Message::TrashBadgeLoaded(TrashBadge::read(&app.paths)));
        assert_eq!(
            app.trash_badge,
            TrashBadge {
                count: None,
                warning: true
            }
        );
        fs::remove_file(dir).unwrap();
        remove_native(&app.paths, "kept");
        let pending = app.paths.prefix_locks_dir().join(".trash-pending");
        fs::create_dir_all(&pending).unwrap();
        fs::write(pending.join("trash-broken.toml"), "broken").unwrap();
        assert!(trash::pending_entries(&app.paths).is_err());
        assert_eq!(trash::list(&app.paths).unwrap().len(), 1);
        let _ = app.update(Message::TrashBadgeLoaded(TrashBadge::read(&app.paths)));
        assert_eq!(
            app.trash_badge,
            TrashBadge {
                count: Some(1),
                warning: true
            }
        );
    }

    #[test]
    fn placeholder_keys_leave_library_selection_and_running_work_alone() {
        let (_root, mut app) = fixture();
        app.search = "game".into();
        app.selected = Some("game".into());
        app.panel_open = true;
        let token = wineo_core::cancel::Token::new();
        app.operation = Some(crate::Operation {
            name: "game".into(),
            kind: crate::OperationKind::Start,
            token: Some(token.clone()),
            cancelled: false,
            status: "Starting...".into(),
            install_done: None,
            installing: false,
        });
        for section in [Section::Trash, Section::Checkup] {
            let _ = app.update(Message::Section(section));
            for key in [
                Key::Named(iced::keyboard::key::Named::Escape),
                Key::Named(iced::keyboard::key::Named::ArrowRight),
                Key::Named(iced::keyboard::key::Named::Enter),
                Key::Character("/".into()),
            ] {
                let _ = app.update(Message::KeyWithFocus(
                    key,
                    Modifiers::empty(),
                    Status::Ignored,
                    false,
                ));
            }
            let _ = app.update(Message::Key(
                Key::Character("n".into()),
                Modifiers::CTRL,
                Status::Ignored,
            ));
            assert!(app.flow.is_none());
            assert!(app.panel_open);
            assert_eq!(app.search, "game");
            assert_eq!(app.selected.as_deref(), Some("game"));
            assert_eq!(app.operation.as_ref().unwrap().status, "Starting...");
            assert!(!token.is_canceled());
        }
        key(&mut app, "1");
        assert_eq!(app.section, Section::Library);
        assert!(app.panel_open);
        assert!(app.operation.is_some());
    }
}
