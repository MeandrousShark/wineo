use iced::Task;
use iced::keyboard::{Key, Modifiers, key::Named};
use wineo_core::paths::resolve;

use crate::library::CardKind;
use crate::{App, Message, portable_page, sidebar::Section, snapshots_page, tools_page};

impl App {
    pub(crate) fn filesystem_busy(&self) -> bool {
        self.snapshots_page
            .as_ref()
            .is_some_and(snapshots_page::Page::busy)
            || self.portable_pages.values().any(portable_page::Page::busy)
    }

    pub(crate) fn tools_busy(&self) -> bool {
        self.tool_pages.values().any(tools_page::Page::busy)
    }

    pub(crate) fn feature_page_open(&self) -> bool {
        self.tool_page.is_some()
            || self.portable_page.is_some()
            || self.section == Section::Snapshots
    }

    pub(crate) fn same_prefix(&self, first: &str, second: &str) -> bool {
        if first == second {
            return true;
        }
        let prefix = |name: &str| {
            self.cards
                .iter()
                .find(|card| card.name == name)
                .and_then(|card| card.prefix.as_deref())
                .map(resolve)
        };
        matches!((prefix(first), prefix(second)), (Some(a), Some(b)) if a == b)
    }

    pub(crate) fn tool_prefix_busy(&self, name: &str) -> bool {
        self.tool_pages
            .iter()
            .any(|(key, page)| page.busy() && self.tool_matches_prefix(name, key))
    }

    pub(crate) fn tool_reason(&self, name: &str) -> Option<String> {
        self.tool_prefix_busy(name)
            .then(|| "Busy: a tool is running in this prefix.".to_owned())
    }

    pub(crate) fn tool_blocks_launch(&self, name: &str) -> bool {
        self.tool_pages
            .iter()
            .any(|(key, page)| page.blocks_launch() && self.tool_matches_prefix(name, key))
    }

    /// Whether the Wine-only actions apply: Snapshots, Run program, and
    /// Winetricks. A pending restore keeps them, so it can be finished.
    pub(crate) fn wine_actions(&self, name: &str) -> bool {
        self.cards.iter().any(|card| {
            card.name == name
                && card.prefix.is_some()
                && match card.kind {
                    Some(CardKind::Wine) | None => true,
                    Some(CardKind::Native | CardKind::Proxied) => false,
                }
        }) || self.restore_pending(name)
    }

    /// Whether Clone and Copy to portable apply. Native environments have
    /// their own paths for both, but a proxied one's launcher owns it.
    pub(crate) fn transfer_actions(&self, name: &str) -> bool {
        !self.cards.iter().any(|card| {
            card.name == name
                && match card.kind {
                    Some(CardKind::Proxied) => true,
                    Some(CardKind::Wine | CardKind::Native) | None => false,
                }
        })
    }

    pub(crate) fn restore_pending(&self, name: &str) -> bool {
        wineo_core::snapshot::check_recovery_marker(&self.paths, name).is_err()
    }

    fn tool_matches_prefix(&self, name: &str, key: &(String, tools_page::Kind)) -> bool {
        if name == key.0 {
            return true;
        }
        let current = self
            .cards
            .iter()
            .find(|card| card.name == name)
            .and_then(|card| card.prefix.as_deref())
            .map(resolve);
        match (current, self.tool_prefixes.get(key)) {
            (Some(current), Some(active)) => current == *active,
            _ => self.same_prefix(name, &key.0),
        }
    }

    fn remember_tool_prefix(&mut self, name: &str, kind: tools_page::Kind) {
        if let Some(prefix) = self
            .cards
            .iter()
            .find(|card| card.name == name)
            .and_then(|card| card.prefix.as_deref())
        {
            self.tool_prefixes
                .insert((name.to_owned(), kind), resolve(prefix));
        }
    }

    pub(crate) fn tool_start_reason(&self, name: &str, kind: tools_page::Kind) -> Option<String> {
        if self.restore_pending(name) {
            return Some("Roll back restore first.".into());
        }
        if self.management_busy() {
            return Some("Busy: changing files.".into());
        }
        if let Some(reason) = self.tool_reason(name) {
            return Some(reason);
        }
        if kind == tools_page::Kind::Winetricks {
            return self.remove_reason(name);
        }
        if self
            .cards
            .iter()
            .any(|card| self.same_prefix(name, &card.name) && (card.lost || card.unresolved))
        {
            return Some("Stop it first. Use Clean up in the panel.".into());
        }
        if self
            .operation
            .as_ref()
            .is_some_and(|operation| self.same_prefix(name, &operation.name))
        {
            return Some("Busy: starting or stopping this prefix.".into());
        }
        None
    }

    pub(crate) fn update_settings_work_refusal(&mut self) {
        let refusal = self.settings.as_ref().and_then(|page| {
            if self.filesystem_busy()
                || self.change.is_some()
                || self.art_busy()
                || self.flow.as_ref().is_some_and(crate::flow::Page::busy)
            {
                Some("Busy: changing files.".to_owned())
            } else if !page.is_global() {
                self.tool_reason(&page.name)
            } else {
                None
            }
        });
        if let Some(page) = &mut self.settings {
            page.work_refusal = refusal;
        }
    }

    fn feature_navigation_blocked(&self) -> bool {
        self.close_prompt
            || self.flow.is_some()
            || self.removal.is_some()
            || self.settings.is_some()
            || self.trash_confirm_open()
            || self.downloads_confirm_open()
            || self.hook_stop_confirm.is_some()
    }

    fn enter_feature(&mut self, name: Option<&str>) {
        self.leave_art();
        self.tool_page = None;
        self.portable_page = None;
        self.action_menu = None;
        self.hovered = None;
        self.search_focused = false;
        self.close_log();
        self.trash_page.cancel_review();
        self.downloads_page.cancel_review();
        if let Some(name) = name {
            self.selected = Some(name.to_owned());
        }
    }

    pub(crate) fn open_snapshots(&mut self, name: Option<String>) -> Task<Message> {
        if self.feature_navigation_blocked()
            || name.as_deref().is_some_and(|name| {
                self.cards
                    .iter()
                    .any(|card| card.name == name && card.kind == Some(CardKind::Proxied))
                    && !self.restore_pending(name)
            })
        {
            return Task::none();
        }
        self.enter_feature(name.as_deref());
        self.section = Section::Snapshots;
        let load = if let Some(page) = &mut self.snapshots_page {
            if page.busy() {
                Task::none()
            } else {
                page.update(snapshots_page::Event::Refresh)
            }
        } else {
            let (page, task) = snapshots_page::Page::new(self.paths.clone());
            self.snapshots_page = Some(page);
            task
        };
        let select = name.map_or_else(Task::none, |name| {
            self.snapshots_page.as_mut().unwrap().select(name)
        });
        Task::batch([load, select]).map(Message::Snapshots)
    }

    pub(crate) fn snapshots_update(&mut self, event: snapshots_page::Event) -> Task<Message> {
        if event.is_input() && (self.section != Section::Snapshots || self.close_prompt) {
            return Task::none();
        }
        if event.starts_work()
            && (self.management_busy()
                || self
                    .snapshots_page
                    .as_ref()
                    .and_then(|page| page.selected_name())
                    .is_some_and(|name| self.prefix_change_reason(name).is_some()))
        {
            return Task::none();
        }
        let Some(page) = &mut self.snapshots_page else {
            return Task::none();
        };
        let task = page.update(event).map(Message::Snapshots);
        let outcome = page.take_outcome();
        let name = page.selected_name().map(str::to_owned);
        let navigation = match page.take_action() {
            Some(snapshots_page::Action::Back) => self.switch_section(Section::Library),
            Some(snapshots_page::Action::Checkup) => match name {
                Some(name) => {
                    crate::checkup::update(self, crate::checkup::Event::Environment(name))
                }
                None => self.navigate_section(Section::Checkup),
            },
            None => Task::none(),
        };
        if outcome.is_some() {
            Task::batch([task, navigation, self.feature_finished()])
        } else {
            Task::batch([task, navigation])
        }
    }

    pub(crate) fn open_portable(&mut self, name: String) -> Task<Message> {
        if self.feature_navigation_blocked() || !self.transfer_actions(&name) {
            return Task::none();
        }
        self.enter_feature(Some(&name));
        self.section = Section::Library;
        self.portable_page = Some(name.clone());
        if self.portable_pages.contains_key(&name) {
            return Task::none();
        }
        let (page, task) = portable_page::Page::new(self.paths.clone(), name.clone());
        self.portable_pages.insert(name.clone(), page);
        task.map(move |event| Message::Portable(name.clone(), event))
    }

    pub(crate) fn portable_update(
        &mut self,
        name: String,
        event: portable_page::Event,
    ) -> Task<Message> {
        if event.is_input() && (self.portable_page.as_ref() != Some(&name) || self.close_prompt) {
            return Task::none();
        }
        if event.starts_work() && (self.management_busy() || self.remove_reason(&name).is_some()) {
            return Task::none();
        }
        let Some(page) = self.portable_pages.get_mut(&name) else {
            return Task::none();
        };
        let event_name = name.clone();
        let task = page
            .update(event)
            .map(move |event| Message::Portable(event_name.clone(), event));
        let action = page.take_action();
        let outcome = page.take_outcome();
        if outcome.is_some()
            && let Some(message) = page.success_message()
        {
            self.feature_notice = Some((
                message,
                std::time::Instant::now() + std::time::Duration::from_secs(8),
            ));
        }
        if action.is_some() {
            self.portable_page = None;
        }
        if outcome.is_some() {
            Task::batch([task, self.feature_finished()])
        } else {
            task
        }
    }

    pub(crate) fn open_tool(&mut self, name: String, kind: tools_page::Kind) -> Task<Message> {
        if self.feature_navigation_blocked() || !self.wine_actions(&name) {
            return Task::none();
        }
        self.enter_feature(Some(&name));
        self.section = Section::Library;
        self.tool_page = Some((name.clone(), kind));
        if self.tool_pages.contains_key(&(name.clone(), kind)) {
            let page = self.tool_pages.get_mut(&(name.clone(), kind)).unwrap();
            return if kind == tools_page::Kind::Winetricks && !page.busy() {
                page.update(&self.paths, tools_page::Event::Refresh)
                    .map(move |event| Message::Tool(name.clone(), kind, event))
            } else {
                Task::none()
            };
        }
        let (page, task) = tools_page::Page::new(&self.paths, name.clone(), kind);
        self.tool_pages.insert((name.clone(), kind), page);
        task.map(move |event| Message::Tool(name.clone(), kind, event))
    }

    pub(crate) fn tool_update(
        &mut self,
        name: String,
        kind: tools_page::Kind,
        event: tools_page::Event,
    ) -> Task<Message> {
        if event.is_input()
            && (self.tool_page.as_ref() != Some(&(name.clone(), kind)) || self.close_prompt)
        {
            return Task::none();
        }
        if event.starts_work() && self.tool_start_reason(&name, kind).is_some() {
            return Task::none();
        }
        if event.starts_work() {
            self.remember_tool_prefix(&name, kind);
        }
        let Some(page) = self.tool_pages.get_mut(&(name.clone(), kind)) else {
            return Task::none();
        };
        let event_name = name.clone();
        let task = page
            .update(&self.paths, event)
            .map(move |event| Message::Tool(event_name.clone(), kind, event));
        let action = page.take_action();
        let outcome = page.take_outcome();
        let navigation = match action {
            Some(tools_page::Action::Back) => {
                self.tool_page = None;
                Task::none()
            }
            Some(tools_page::Action::Settings) => {
                self.tool_page = None;
                self.open_settings()
            }
            None => Task::none(),
        };
        let reload = if outcome {
            self.feature_finished()
        } else {
            Task::none()
        };
        Task::batch([task, navigation, reload])
    }

    fn feature_finished(&mut self) -> Task<Message> {
        Task::batch([
            self.reload_after_uses_change(),
            self.settings_tick(),
            self.refresh_feature_checkups(),
        ])
    }

    pub(crate) fn cancel_features(&mut self, force: bool) {
        if let Some(page) = &mut self.snapshots_page {
            page.cancel();
        }
        for page in self.portable_pages.values_mut() {
            page.cancel();
        }
        for page in self.tool_pages.values_mut() {
            page.cancel(force);
        }
    }

    pub(crate) fn feature_finishing(&self) -> bool {
        self.snapshots_page
            .as_ref()
            .is_some_and(snapshots_page::Page::finishing)
            || self
                .portable_pages
                .values()
                .any(portable_page::Page::finishing)
    }

    pub(crate) fn feature_key(
        &mut self,
        key: &Key,
        modifiers: Modifiers,
        captured: bool,
    ) -> Task<Message> {
        if captured || !modifiers.is_empty() {
            return Task::none();
        }
        if let Some((name, kind)) = self.tool_page.clone() {
            let event = match key {
                Key::Named(Named::Escape) => tools_page::Event::Back,
                Key::Named(Named::Enter) => return self.tool_submit(name, kind),
                _ => return Task::none(),
            };
            return self.tool_update(name, kind, event);
        }
        if let Some(name) = self.portable_page.clone() {
            if *key == Key::Named(Named::Escape) {
                return self.portable_update(name, portable_page::Event::Back);
            }
            if *key == Key::Named(Named::Enter)
                && !self.management_busy()
                && self.remove_reason(&name).is_none()
            {
                return self.portable_update(name, portable_page::Event::Submit);
            }
        }
        if self.section == Section::Snapshots {
            if *key == Key::Named(Named::Escape) {
                return self.snapshots_update(snapshots_page::Event::Back);
            }
            if *key == Key::Named(Named::Enter)
                && !self.management_busy()
                && !self
                    .snapshots_page
                    .as_ref()
                    .and_then(|page| page.selected_name())
                    .is_some_and(|name| self.prefix_change_reason(name).is_some())
            {
                return self
                    .snapshots_page
                    .as_mut()
                    .map_or_else(Task::none, |page| page.submit().map(Message::Snapshots));
            }
        }
        Task::none()
    }

    fn tool_submit(&mut self, name: String, kind: tools_page::Kind) -> Task<Message> {
        if self.tool_start_reason(&name, kind).is_some() {
            return Task::none();
        }
        self.remember_tool_prefix(&name, kind);
        self.tool_pages
            .get_mut(&(name.clone(), kind))
            .unwrap()
            .submit(&self.paths)
            .map(move |event| Message::Tool(name.clone(), kind, event))
    }
}
