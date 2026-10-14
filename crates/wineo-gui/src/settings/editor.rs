//! A settings file's editor: the loaded file, its form, and the inputs typed but not added yet.

use wineo_core::components::Component;

use std::collections::{BTreeMap, BTreeSet};

use super::form::{EnvRow, Form, List, TargetValues};
use super::load::{Current, Loaded, Scan};
use super::{Event, TargetEdit, load};

/// The add target form's fields, typed but not added yet.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct NewTarget {
    pub(crate) name: String,
    pub(crate) path: String,
    /// The picked program's name, which titles its desktop entry.
    pub(crate) label: Option<String>,
}

pub(crate) struct Editor {
    pub(crate) info: Loaded,
    pub(crate) form: Form,
    /// Why the last runner picked was refused.
    pub(crate) refusal: Option<String>,
    /// An extra gamescope arg typed but not added yet.
    pub(crate) args_input: String,
    /// Why the last build picked was refused.
    pub(crate) build_refusal: Option<String>,
    /// Chips and a variable typed but not added yet, which a save leaves out.
    pub(crate) winetricks_input: String,
    pub(crate) tags_input: String,
    pub(crate) new_env: EnvRow,
    /// Per target: an arg and a gamescope arg typed but not added yet.
    pub(crate) target_args_input: BTreeMap<String, String>,
    pub(crate) target_gamescope_args_input: BTreeMap<String, String>,
    pub(crate) new_target: NewTarget,
    pub(crate) candidate_exe: Option<String>,
    /// Why the last target add was refused.
    pub(crate) add_error: Option<String>,
    /// The target asking to be removed.
    pub(crate) removing: Option<String>,
    /// View state, kept across every reload.
    pub(crate) open_cards: BTreeSet<String>,
    pub(crate) add_open: bool,
    pub(crate) scan: Option<Scan>,
    pub(crate) show_held_back: bool,
}

impl Editor {
    pub(super) fn new(info: Loaded, form: Form) -> Self {
        Self {
            info,
            form,
            refusal: None,
            args_input: String::new(),
            build_refusal: None,
            winetricks_input: String::new(),
            tags_input: String::new(),
            new_env: EnvRow::default(),
            target_args_input: BTreeMap::new(),
            target_gamescope_args_input: BTreeMap::new(),
            new_target: NewTarget::default(),
            candidate_exe: None,
            add_error: None,
            removing: None,
            open_cards: BTreeSet::new(),
            add_open: false,
            scan: None,
            show_held_back: false,
        }
    }
    /// Text typed into an add field but not added yet.
    pub(super) fn carry_inputs(&mut self, from: &Editor) {
        self.args_input.clone_from(&from.args_input);
        self.winetricks_input.clone_from(&from.winetricks_input);
        self.tags_input.clone_from(&from.tags_input);
        self.new_env.clone_from(&from.new_env);
        self.target_args_input.clone_from(&from.target_args_input);
        self.target_gamescope_args_input
            .clone_from(&from.target_gamescope_args_input);
        self.new_target.clone_from(&from.new_target);
        self.candidate_exe.clone_from(&from.candidate_exe);
    }
    pub(super) fn clear_inputs(&mut self) {
        self.args_input.clear();
        self.winetricks_input.clear();
        self.tags_input.clear();
        self.new_env = EnvRow::default();
        self.target_args_input.clear();
        self.target_gamescope_args_input.clear();
        self.new_target = NewTarget::default();
        self.candidate_exe = None;
        self.add_error = None;
        self.removing = None;
    }
    /// What's open on the page, which no reload should close.
    pub(super) fn carry_view(&mut self, from: &Editor) {
        self.open_cards.clone_from(&from.open_cards);
        self.add_open = from.add_open;
        self.scan.clone_from(&from.scan);
        self.show_held_back = from.show_held_back;
    }
    pub(crate) fn target_args_input(&self, target: &str) -> &str {
        self.target_args_input
            .get(target)
            .map_or("", String::as_str)
    }
    pub(crate) fn target_gamescope_args_input(&self, target: &str) -> &str {
        self.target_gamescope_args_input
            .get(target)
            .map_or("", String::as_str)
    }
    /// Whether the typed name and path can be added as they stand.
    pub(crate) fn can_add_target(&self) -> bool {
        !self.new_target.path.trim().is_empty()
            && self
                .form
                .new_target_problem(&self.new_target.name)
                .is_none()
    }
    pub(crate) fn duplicate_target(&self) -> Option<&str> {
        self.candidate_exe
            .as_deref()
            .and_then(|exe| self.form.using_exe(exe))
    }
    /// Why a target's desktop entry can't be turned on, if it can't.
    pub(crate) fn desktop_refusal(&self) -> Option<String> {
        self.info
            .portable
            .then(|| wineo_core::error::Error::PortableDesktop.to_string())
    }
    pub(crate) fn list_input(&self, list: List) -> &str {
        match list {
            List::Winetricks => &self.winetricks_input,
            List::Tags => &self.tags_input,
        }
    }
    pub(super) fn list_input_mut(&mut self, list: List) -> &mut String {
        match list {
            List::Winetricks => &mut self.winetricks_input,
            List::Tags => &mut self.tags_input,
        }
    }
    /// The runner the draft names and its kind, from what the page already
    /// resolved. `None` when it doesn't resolve, which core doesn't check.
    pub(crate) fn draft_runner(&self) -> Option<Current> {
        let runner = self.form.draft.runner.as_ref()?;
        if Some(runner) == self.form.loaded.runner.as_ref() {
            return self.info.current.clone();
        }
        let choice = self
            .info
            .runners
            .iter()
            .find(|choice| &choice.name == runner)?;
        Some(Current {
            name: choice.name.clone(),
            proton: choice.proton?,
        })
    }
    pub(crate) fn proton(&self) -> bool {
        self.draft_runner().is_some_and(|runner| runner.proton)
    }
    /// gameid and store are Proton's, but a value already there stays
    /// editable so it can be cleared.
    pub(crate) fn protonfixes_shown(&self) -> bool {
        let loaded = &self.form.loaded;
        let draft = &self.form.draft;
        !self.info.native
            && (self.proton()
                || !loaded.gameid.is_empty()
                || !loaded.store.is_empty()
                || !draft.gameid.is_empty()
                || !draft.store.is_empty())
    }
    pub(crate) fn build_refusal(&self, name: &str, component: Component) -> Option<String> {
        load::component_refusal(name, self.draft_runner().as_ref(), component)
    }
    /// Whether a component's pick list takes a pick.
    pub(crate) fn build_enabled(&self, name: &str, component: Component) -> bool {
        !self.info.native
            && self.build_refusal(name, component).is_none()
            && (component == Component::Dxvk || self.form.vkd3d_enabled())
    }
    /// Whether this edit reaches a control the page shows.
    pub(super) fn accepts(&self, name: &str, event: &Event) -> bool {
        if self.info.native && event.wine_only() {
            return false;
        }
        match event {
            Event::BuildPicked(component, _) => self.build_enabled(name, *component),
            Event::BuildRemoved(component) => self.form.draft.build(*component).is_some(),
            Event::GameId(_) | Event::Store(_) => self.protonfixes_shown(),
            // Turning one off stays possible, so a hand-written one can go.
            Event::Target(_, TargetEdit::Desktop(true)) => self.desktop_refusal().is_none(),
            _ => true,
        }
    }
}

impl Editor {
    /// Adds the checked exe as a new target, set up the way `setup` sets one
    /// up: a desktop entry titled after the program, unless portable.
    pub(super) fn add_resolved(&mut self, request: &NewTarget, exe: String) -> Result<(), String> {
        let name = if request.name.is_empty() {
            self.form
                .suggest_target(request.label.as_deref().unwrap_or(""), &exe)
        } else {
            request.name.clone()
        };
        let desktop = !self.info.portable;
        let label = request.label.clone().unwrap_or_else(|| file_stem(&exe));
        let values = TargetValues {
            exe,
            desktop,
            // A name only titles a desktop entry, so none is kept without one.
            name: if desktop { label } else { String::new() },
            ..TargetValues::default()
        };
        self.form.add_target(&name, values)?;
        self.new_target = NewTarget::default();
        self.candidate_exe = None;
        self.add_error = None;
        self.add_open = false;
        self.open_cards.insert(name);
        Ok(())
    }
}

fn file_stem(path: &str) -> String {
    let file = path.rsplit(['\\', '/']).next().unwrap_or(path);
    file.rsplit_once('.')
        .map_or(file, |(stem, _)| stem)
        .to_owned()
}
