//! What one Heroic game would become, and the rules that decide it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::Context;

use crate::components::Component;
use crate::config::{Arch, GlobalConfig, Target};
use crate::error::Error;
use crate::gamescope::GamescopeSetting;
use crate::heroic::{
    HeroicImport, HeroicIssue, HeroicIssueKind, HeroicSharedPrefix, HeroicSource, ReviewItem,
    parse_launcher_args,
};
use crate::import::{self, ExternalDependency, ImportRequest, ImportedSettings, Inspection};
use crate::install::{self, Wanted};
use crate::name;
use crate::paths::Paths;
use crate::runner::Runner;
use crate::runners::Runners;

use super::SkipReason;

/// Heroic settings wineo already covers by way of the chosen runner, so they
/// don't need review or a place in the output: Proton bundles its own DXVK,
/// vkd3d-proton, and NVAPI, so a Proton import can ignore whatever Heroic
/// recorded for them. What matters is the runner wineo ends up using, not what
/// Heroic itself recorded.
pub const COVERED_BY_PROTON: [&str; 3] = [
    "autoInstallDxvk",
    "autoInstallVkd3d",
    "autoInstallDxvkNvapi",
];

/// How strictly a plan's findings are settled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Policy {
    /// Flags supplied everything, so anything that isn't understood blocks the
    /// import until a person reviews it.
    Strict,
    /// Conflicts wineo manages itself are dropped and unsupported settings are
    /// omitted. Launcher arguments that can't be parsed need input.
    Defaults,
}

/// A value only a person can supply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputKind {
    /// Heroic's launcher arguments, which couldn't be parsed.
    LauncherArgs,
}

/// What happens to one thing Heroic reported.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolution {
    /// It became a setting, so nothing is left to review.
    Carried,
    /// The chosen runner makes it moot.
    Covered,
    /// A setting wineo manages itself replaces it.
    Dropped,
    /// Wineo has nowhere to put it, so it isn't imported.
    Omitted,
    /// It stays, as something to know about rather than fix.
    Kept,
    /// The import can't go ahead until it's dealt with.
    Blocking,
    /// A person has to supply a value.
    NeedsInput(InputKind),
    /// A person supplied the value it needed.
    Fixed,
}

/// One issue Heroic reported and what the plan does about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub issue: HeroicIssue,
    pub resolution: Resolution,
}

impl Finding {
    /// Whether the finding still matters after the runner was chosen.
    pub fn is_active(&self) -> bool {
        !matches!(self.resolution, Resolution::Carried | Resolution::Covered)
    }
}

/// A conflict with a setting wineo manages itself, which import drops rather
/// than refusing over. Runner and prefix conflicts still block.
pub fn is_dropped_conflict(issue: &HeroicIssue) -> bool {
    issue.kind == HeroicIssueKind::Conflict
        && !matches!(
            issue.field.as_deref(),
            Some("wineVersion") | Some("winePrefix")
        )
}

fn is_anti_cheat(issue: &HeroicIssue) -> bool {
    matches!(
        issue.field.as_deref(),
        Some("eacRuntime") | Some("battlEyeRuntime")
    )
}

/// Something worth knowing about a plan before it is applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Warning {
    /// Other Heroic games use the same prefix.
    SharedPrefix(Vec<HeroicSharedPrefix>),
    /// Heroic disabled umu, which wineo uses for Proton. Holds Heroic's note.
    UmuDisabled(String),
    /// Wine NVAPI needs DLLs that enabling variables doesn't install. Holds
    /// Heroic's note.
    NvapiNotInstalled(String),
    /// Heroic's EAC or BattlEye runtime isn't imported.
    AntiCheatNotImported,
    /// This many unsupported settings aren't imported.
    Omitted(usize),
    /// This many settings are dropped because wineo manages them.
    Dropped(usize),
    /// Settings that need a look, like a runner Wayland doesn't support.
    NeedsAttention(Vec<HeroicIssue>),
}

/// A DXVK or vkd3d-proton build the plan needs that isn't installed.
#[derive(Debug)]
pub struct MissingBuild {
    pub component: Component,
    /// The exact version asked for, or None for the latest.
    pub version: Option<String>,
    error: Error,
}

impl MissingBuild {
    fn from_error(error: Error) -> std::result::Result<Self, Error> {
        let (name, version) = match &error {
            Error::NoBuilds { component, .. } => (*component, None),
            Error::UnknownBuild {
                component, name, ..
            } => (*component, Some(name.clone())),
            _ => return Err(error),
        };
        match Component::ALL.into_iter().find(|c| c.as_str() == name) {
            Some(component) => Ok(Self {
                component,
                version,
                error,
            }),
            None => Err(error),
        }
    }

    /// The error's own message, hint included.
    pub fn message(&self) -> String {
        self.error.to_string()
    }

    /// The error the build's absence would have caused.
    pub fn into_error(self) -> Error {
        self.error
    }
}

/// Whether to write a desktop entry for the imported target, and what to
/// call it. Kept separate from `HeroicImport` since it isn't a Heroic
/// setting, and separate from `ImportRequest` since it isn't generic to
/// every import.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesktopEntry {
    pub enabled: bool,
    pub name: String,
}

/// What a plan starts from, before anything is edited.
#[derive(Debug, Clone, Default)]
pub struct Options {
    /// The environment's name. Defaults to one suggested from the title.
    pub name: Option<String>,
    /// Keep the prefix where it is instead of copying it.
    pub adopt: bool,
    /// Where a copy goes. Defaults to the configured prefix root.
    pub destination: Option<PathBuf>,
    /// Resolves a missing registry architecture marker.
    pub arch: Option<Arch>,
}

/// A setter's reason for refusing a value.
#[derive(Debug, thiserror::Error)]
pub enum PlanError {
    #[error("invalid environment name: {0}")]
    InvalidName(String),
    #[error("Executable path cannot be empty")]
    EmptyExecutable,
    #[error("{0}")]
    InvalidArguments(String),
    #[error("Enter a name for the desktop entry")]
    EmptyDesktopName,
    #[error("environment entries must use KEY=VALUE")]
    EnvironmentFormat,
    #[error("invalid environment variable name `{0}`")]
    EnvironmentName(String),
    #[error("{0} is managed by wineo and cannot be imported")]
    ManagedEnvironment(String),
    /// Something Heroic stored, or the game's exe, keeps it from being
    /// imported. Holds why.
    #[error("{0}")]
    Blocked(String),
    #[error(transparent)]
    Config(#[from] Error),
}

/// A Heroic game and everything decided about importing it: the settings it
/// carries over, what happens to each thing Heroic reported, and where the
/// prefix goes. Nothing is read or written after it's built, apart from the
/// checks its methods say they make.
#[derive(Debug, Clone)]
pub struct GamePlan {
    original: HeroicImport,
    import: HeroicImport,
    findings: Vec<Finding>,
    shared_prefix: Vec<HeroicSharedPrefix>,
    runner_dependencies: Vec<ExternalDependency>,
    runner: Runner,
    inspection: Inspection,
    request: ImportRequest,
    desktop: DesktopEntry,
    /// Set only when migrating a GOG game.
    gog_id: Option<String>,
}

/// Inspects the Windows game `id` and plans it against `runner`, an installed
/// runner name or alias.
pub fn plan_game(
    paths: &Paths,
    global: &GlobalConfig,
    source: &HeroicSource,
    id: &str,
    runner: &str,
    options: Options,
) -> anyhow::Result<GamePlan> {
    let imported = source.inspect(id)?;
    let shared = source.shared_prefix_candidates(id)?;
    GamePlan::new(paths, global, imported, shared, runner, options)
}

impl GamePlan {
    /// Plans a game that was already inspected. This is the one place the
    /// runner pipeline runs: whatever the runner covers or carries over is
    /// settled here, and the rest is left as findings.
    pub fn new(
        paths: &Paths,
        global: &GlobalConfig,
        imported: HeroicImport,
        shared_prefix: Vec<HeroicSharedPrefix>,
        runner_name: &str,
        options: Options,
    ) -> anyhow::Result<Self> {
        let runner = Runners::new(paths, global)?.resolve(runner_name)?;
        Self::with_runner(
            paths,
            global,
            imported,
            shared_prefix,
            runner_name,
            runner,
            options,
        )
    }

    /// `new` for a runner that's already resolved, as `runner_name` names it.
    pub fn with_runner(
        paths: &Paths,
        global: &GlobalConfig,
        imported: HeroicImport,
        shared_prefix: Vec<HeroicSharedPrefix>,
        runner_name: &str,
        runner: Runner,
        options: Options,
    ) -> anyhow::Result<Self> {
        let inspection = import::inspect(&imported.prefix, Some(&runner), options.arch)
            .context("inspect Heroic prefix")?;
        let mut findings: Vec<Finding> = imported
            .issues
            .iter()
            .map(|issue| Finding {
                resolution: classify(issue, Policy::Defaults),
                issue: issue.clone(),
            })
            .collect();
        let mut import = imported.clone();
        clear_runner_dependency(&mut import, &mut findings);
        drop_runner_conflicts(&mut findings);
        suppress_covered_settings(&mut import, &mut findings, &runner);
        carry_over_components(&mut import, &mut findings, &runner);
        carry_over_wayland(&mut import, &mut findings, &runner);
        import.issues = active_issues(&findings);

        let name = options.name.unwrap_or_else(|| {
            crate::targets::suggest_name(&import.title, &import.exe.to_string_lossy())
        });
        let destination = if options.adopt {
            None
        } else {
            Some(match options.destination {
                Some(destination) => destination,
                None => global.prefix_root(paths)?.join(&name),
            })
        };
        let request = ImportRequest {
            source: import.prefix.clone(),
            name,
            runner: runner_name.to_owned(),
            destination,
            adopt: options.adopt,
            arch: options.arch,
        };
        let desktop = DesktopEntry {
            enabled: false,
            name: import.title.clone(),
        };
        Ok(Self {
            original: imported,
            import,
            findings,
            shared_prefix,
            runner_dependencies: Vec::new(),
            runner,
            inspection,
            request,
            desktop,
            gog_id: None,
        })
    }

    /// The game as Heroic reported it, before the runner shaped anything.
    pub fn heroic(&self) -> &HeroicImport {
        &self.original
    }

    /// The game as it will be imported. Its issues are the findings that still
    /// matter, and it reflects every edit made to the plan.
    pub fn resolved(&self) -> &HeroicImport {
        &self.import
    }

    pub fn findings(&self) -> &[Finding] {
        &self.findings
    }

    /// Other Heroic games that use this game's prefix.
    pub fn shared_prefix(&self) -> &[HeroicSharedPrefix] {
        &self.shared_prefix
    }

    /// Links a copied Heroic runner still needs, when it was copied.
    pub fn runner_dependencies(&self) -> &[ExternalDependency] {
        &self.runner_dependencies
    }

    /// Records what copying Heroic's runner left linked outside wineo.
    pub fn set_runner_dependencies(&mut self, dependencies: Vec<ExternalDependency>) {
        self.runner_dependencies = dependencies;
    }

    pub fn runner(&self) -> &Runner {
        &self.runner
    }

    pub fn inspection(&self) -> &Inspection {
        &self.inspection
    }

    pub fn request(&self) -> &ImportRequest {
        &self.request
    }

    pub fn name(&self) -> &str {
        &self.request.name
    }

    pub fn is_adopt(&self) -> bool {
        self.request.adopt
    }

    /// Where the prefix will live once imported.
    pub fn destination(&self) -> &Path {
        if self.request.adopt {
            return &self.inspection.source_root;
        }
        self.request
            .destination
            .as_deref()
            .unwrap_or(&self.inspection.source_root)
    }

    pub fn desktop(&self) -> &DesktopEntry {
        &self.desktop
    }

    /// The GOG game the environment is created for, when migrating one.
    pub fn gog_id(&self) -> Option<&str> {
        self.gog_id.as_deref()
    }

    pub(crate) fn set_gog_id(&mut self, gog_id: &str) {
        self.gog_id = Some(gog_id.to_owned());
    }

    /// The DXVK and vkd3d-proton builds the import asks for.
    pub fn wanted(&self) -> Wanted {
        Wanted {
            dxvk: self.import.dxvk.clone(),
            vkd3d: self.import.vkd3d.clone(),
        }
    }

    /// Settles every finding by `policy`. Findings already carried, covered,
    /// or fixed by a person stay as they are.
    pub fn resolve(&mut self, policy: Policy) {
        for finding in &mut self.findings {
            if matches!(
                finding.resolution,
                Resolution::Carried | Resolution::Covered | Resolution::Fixed
            ) {
                continue;
            }
            finding.resolution = classify(&finding.issue, policy);
        }
    }

    /// The findings that stop the import.
    pub fn blocking(&self) -> impl Iterator<Item = &Finding> {
        self.findings
            .iter()
            .filter(|finding| finding.resolution == Resolution::Blocking)
    }

    /// The value a person still has to supply, if any.
    pub fn needs_input(&self) -> Option<InputKind> {
        self.findings
            .iter()
            .find_map(|finding| match finding.resolution {
                Resolution::NeedsInput(kind) => Some(kind),
                _ => None,
            })
    }

    /// Why this game can't be imported without anyone reviewing it, after
    /// `resolve(Policy::Defaults)`. The first of a setting that can't be
    /// imported, unparseable launcher arguments, or an unresolved setting.
    pub fn skip_reason(&self) -> Option<SkipReason> {
        if let Some(finding) = self.findings.iter().find(|finding| {
            finding.is_active()
                && finding.issue.kind == HeroicIssueKind::Invalid
                && finding.issue.field.as_deref() != Some("launcherArgs")
        }) {
            return Some(SkipReason::Blocked(finding.issue.message.clone()));
        }
        if self.findings.iter().any(|finding| {
            finding.is_active() && finding.issue.field.as_deref() == Some("launcherArgs")
        }) {
            return Some(SkipReason::LauncherArgs);
        }
        self.blocking()
            .next()
            .map(|finding| SkipReason::Blocked(finding.issue.message.clone()))
    }

    /// What's worth knowing before applying, in the order to show it.
    pub fn warnings(&self) -> Vec<Warning> {
        let issues: Vec<&HeroicIssue> = self
            .findings
            .iter()
            .filter(|finding| finding.is_active())
            .map(|finding| &finding.issue)
            .collect();
        let mut warnings = Vec::new();
        if !self.shared_prefix.is_empty() {
            warnings.push(Warning::SharedPrefix(self.shared_prefix.clone()));
        }
        let find = |field: &str| {
            issues
                .iter()
                .find(|issue| issue.field.as_deref() == Some(field))
        };
        if let Some(issue) = find("disableUMU") {
            warnings.push(Warning::UmuDisabled(issue.message.clone()));
        }
        if let Some(issue) = find("autoInstallDxvkNvapi") {
            warnings.push(Warning::NvapiNotInstalled(issue.message.clone()));
        }
        if issues.iter().any(|issue| is_anti_cheat(issue)) {
            warnings.push(Warning::AntiCheatNotImported);
        }
        let omitted = issues
            .iter()
            .filter(|issue| {
                issue.kind == HeroicIssueKind::Unsupported
                    && !is_anti_cheat(issue)
                    && issue.field.as_deref() != Some("disableUMU")
            })
            .count();
        if omitted > 0 {
            warnings.push(Warning::Omitted(omitted));
        }
        let dropped = issues
            .iter()
            .filter(|issue| is_dropped_conflict(issue))
            .count();
        if dropped > 0 {
            warnings.push(Warning::Dropped(dropped));
        }
        let attention: Vec<HeroicIssue> = issues
            .iter()
            .filter(|issue| {
                matches!(
                    issue.kind,
                    HeroicIssueKind::NeedsAttention | HeroicIssueKind::External
                ) && !is_anti_cheat(issue)
                    && issue.field.as_deref() != Some("autoInstallDxvkNvapi")
            })
            .map(|issue| (*issue).clone())
            .collect();
        if !attention.is_empty() {
            warnings.push(Warning::NeedsAttention(attention));
        }
        warnings
    }

    /// Whether the DXVK and vkd3d-proton builds the import asks for are
    /// installed, as an error when they aren't. Planning takes no build locks,
    /// so a dry run leaves nothing behind; apply locks them.
    pub fn check_builds(&self, paths: &Paths, global: &GlobalConfig) -> crate::Result<()> {
        install::resolve_available(
            paths,
            global,
            &self.request.name,
            &self.runner,
            &self.wanted(),
        )
        .map(|_| ())
    }

    /// The build that's missing, if one is. Any other reason the import
    /// can't get its builds is an error.
    pub fn missing_build(
        &self,
        paths: &Paths,
        global: &GlobalConfig,
    ) -> crate::Result<Option<MissingBuild>> {
        match self.check_builds(paths, global) {
            Ok(()) => Ok(None),
            Err(error) => MissingBuild::from_error(error).map(Some),
        }
    }

    // -- Editing --

    /// Renames the environment. A copy destination that followed the old name
    /// follows the new one.
    pub fn set_name(
        &mut self,
        paths: &Paths,
        global: &GlobalConfig,
        new_name: &str,
    ) -> Result<(), PlanError> {
        if let Some(problem) = name::problem(new_name) {
            return Err(PlanError::InvalidName(problem));
        }
        let root = global.prefix_root(paths)?;
        let uses_default =
            self.request.destination.as_deref() == Some(root.join(&self.request.name).as_path());
        self.request.name = new_name.to_owned();
        if uses_default && !self.request.adopt {
            self.request.destination = Some(root.join(new_name));
        }
        Ok(())
    }

    pub fn set_executable(&mut self, exe: PathBuf) -> Result<(), PlanError> {
        if exe.as_os_str().is_empty() {
            return Err(PlanError::EmptyExecutable);
        }
        self.import.exe = exe;
        Ok(())
    }

    /// None goes back to the default working directory.
    pub fn set_working_directory(&mut self, cwd: Option<PathBuf>) {
        self.import.cwd = cwd;
    }

    /// Reads a path the way a person types it: absolute, `~/...`, or relative
    /// to the prefix.
    pub fn resolve_path(&self, paths: &Paths, value: &str) -> Result<PathBuf, PlanError> {
        let path = Path::new(value);
        if path.is_absolute() || value == "~" || value.starts_with("~/") {
            Ok(paths.absolute(value)?)
        } else {
            Ok(self.import.prefix.join(path))
        }
    }

    /// Sets the launcher arguments from the text Heroic keeps them as. It
    /// also answers a finding that asked for them.
    pub fn set_launcher_args(&mut self, value: &str) -> Result<(), PlanError> {
        let args = parse_launcher_args(value).map_err(PlanError::InvalidArguments)?;
        self.import.args = args;
        self.import.launcher_args = value.to_owned();
        for finding in &mut self.findings {
            if finding.resolution == Resolution::NeedsInput(InputKind::LauncherArgs) {
                finding.resolution = Resolution::Fixed;
            }
        }
        Ok(())
    }

    /// Replaces the environment with `KEY=VALUE` entries. Variables wineo
    /// manages can't be set.
    pub fn set_environment<S: AsRef<str>>(
        &mut self,
        entries: impl IntoIterator<Item = S>,
    ) -> Result<(), PlanError> {
        let mut env = BTreeMap::new();
        for entry in entries {
            let Some((key, value)) = entry.as_ref().split_once('=') else {
                return Err(PlanError::EnvironmentFormat);
            };
            if key.is_empty() || key.contains(['=', '\0']) {
                return Err(PlanError::EnvironmentName(key.to_owned()));
            }
            if crate::heroic::MANAGED_ENV.contains(&key) {
                return Err(PlanError::ManagedEnvironment(key.to_owned()));
            }
            env.insert(key.to_owned(), value.to_owned());
        }
        self.import.env = env;
        for finding in &mut self.findings {
            if finding.issue.field.as_deref().is_some_and(|field| {
                field.starts_with("enviromentOptions")
                    || crate::heroic::MANAGED_ENV.contains(&field)
            }) {
                finding.resolution = Resolution::Fixed;
            }
        }
        Ok(())
    }

    pub fn set_gamemode(&mut self, enabled: bool) {
        self.import.gamemode = Some(enabled);
    }

    pub fn set_wayland(&mut self, enabled: bool) -> Result<(), PlanError> {
        if enabled && !self.runner.supports_wayland() {
            return Err(PlanError::Blocked(format!(
                "{} does not support Wine Wayland",
                self.runner.name()
            )));
        }
        self.import.wayland = Some(enabled);
        if !enabled && self.import.enable_hdr {
            self.import.enable_hdr = false;
            self.import.env.remove(if self.runner.is_proton() {
                "PROTON_ENABLE_HDR"
            } else {
                "DXVK_HDR"
            });
        }
        self.fix_findings(|field| field == "enableWineWayland");
        Ok(())
    }

    pub fn set_mangohud(&mut self, enabled: bool) {
        self.import.mangohud = Some(enabled);
    }

    pub fn set_gamescope(&mut self, setting: Option<GamescopeSetting>) {
        self.import.gamescope = setting;
        self.fix_findings(|field| field.starts_with("gamescope."));
    }

    pub fn set_hdr(&mut self, enabled: bool) -> Result<(), PlanError> {
        if enabled && self.import.wayland != Some(true) {
            return Err(PlanError::Blocked(
                "Enable Wine Wayland before HDR".to_owned(),
            ));
        }
        let key = if self.runner.is_proton() {
            "PROTON_ENABLE_HDR"
        } else {
            "DXVK_HDR"
        };
        if enabled {
            self.import.env.insert(key.to_owned(), "1".to_owned());
        } else {
            self.import.env.remove(key);
        }
        self.import.enable_hdr = enabled;
        self.fix_findings(|field| field == "enableHDR");
        Ok(())
    }

    fn fix_findings(&mut self, applies: impl Fn(&str) -> bool) {
        for finding in &mut self.findings {
            if finding.issue.field.as_deref().is_some_and(&applies) {
                finding.resolution = Resolution::Fixed;
            }
        }
    }

    /// Keeps the prefix where it is. Later launches modify it.
    pub fn set_adopt(&mut self) {
        self.request.adopt = true;
        self.request.destination = None;
    }

    /// Copies the prefix into `destination`, or the configured prefix root.
    pub fn set_copy(
        &mut self,
        paths: &Paths,
        global: &GlobalConfig,
        destination: Option<PathBuf>,
    ) -> Result<(), PlanError> {
        let destination = match destination.or_else(|| self.request.destination.clone()) {
            Some(destination) => destination,
            None => global.prefix_root(paths)?.join(&self.request.name),
        };
        self.request.adopt = false;
        self.request.destination = Some(destination);
        Ok(())
    }

    pub fn set_desktop_enabled(&mut self, enabled: bool) {
        self.desktop.enabled = enabled;
    }

    pub fn set_desktop_name(&mut self, name: &str) -> Result<(), PlanError> {
        if name.is_empty() {
            return Err(PlanError::EmptyDesktopName);
        }
        self.desktop.name = name.to_owned();
        Ok(())
    }

    // -- Output --

    /// The settings the environment is registered with.
    pub fn settings(&self) -> ImportedSettings {
        let imported = &self.import;
        let destination = self
            .request
            .destination
            .as_deref()
            .unwrap_or(&self.inspection.source_root);
        let rebase = |path: &Path| {
            if self.request.adopt {
                return path.to_owned();
            }
            path.strip_prefix(&self.inspection.source_root)
                .map(|relative| destination.join(relative))
                .unwrap_or_else(|_| path.to_owned())
        };
        let exe = rebase(&imported.exe);
        let cwd = imported.cwd.as_deref().map(rebase);
        let target_name = crate::targets::suggest_name(&imported.title, &exe.to_string_lossy());
        let mut targets = BTreeMap::new();
        targets.insert(
            target_name,
            Target {
                exe: exe.to_string_lossy().into_owned(),
                args: imported.args.clone(),
                cwd: cwd.map(|path| path.to_string_lossy().into_owned()),
                default: true,
                gamemode: None,
                wayland: None,
                mangohud: None,
                gamescope: None,
                hooks: None,
                desktop: self.desktop.enabled,
                name: self.desktop.enabled.then(|| self.desktop.name.clone()),
            },
        );
        ImportedSettings {
            targets,
            env: imported.env.clone(),
            gamemode: imported.gamemode,
            wayland: imported.wayland,
            mangohud: imported.mangohud,
            gamescope: imported.gamescope.clone(),
            hooks: imported.hooks.clone(),
            dxvk: imported.dxvk.clone(),
            vkd3d: imported.vkd3d.clone(),
            heroic_id: Some(imported.id.clone()),
            gog_id: self.gog_id.clone(),
            ..ImportedSettings::default()
        }
    }

    /// The full detail an import log records: every Heroic field, every link,
    /// and the global settings that apply, none of it collapsed or hidden.
    pub fn log_text(&self, global: &GlobalConfig, source: &HeroicSource) -> String {
        let imported = &self.import;
        let inspection = &self.inspection;
        let request = &self.request;
        let mut out = String::new();
        out.push_str(&format!(
            "Heroic game: {} ({})\n",
            imported.title, imported.id
        ));
        out.push_str(&format!("  source: {}\n", source.config_dir().display()));
        out.push_str(&format!("  prefix: {}\n", imported.prefix.display()));
        out.push_str(&format!(
            "  runner: {} ({:?})\n",
            imported.runner.name, imported.runner.kind
        ));
        if let Some(runner_source) = &imported.runner.source {
            out.push_str(&format!("    copied from: {}\n", runner_source.display()));
        }
        out.push_str(&format!("  executable: {}\n", imported.exe.display()));
        out.push_str(&format!("  arguments: {:?}\n", imported.args));
        out.push_str(&format!(
            "  working directory: {}\n",
            imported
                .cwd
                .as_deref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "default".to_owned())
        ));
        out.push_str(&format!("  gamemode: {:?}\n", imported.gamemode));
        out.push_str(&format!("  wayland: {:?}\n", imported.wayland));
        out.push_str(&format!("  mangohud: {:?}\n", imported.mangohud));
        out.push_str(&format!("  gamescope: {:?}\n", imported.gamescope));
        if !imported.env.is_empty() {
            out.push_str("  environment:\n");
            for (key, value) in &imported.env {
                out.push_str(&format!("    {key}={}\n", redact_env_value(key, value)));
            }
        }
        if !imported.external_dependencies.is_empty() {
            out.push_str("  external files:\n");
            for path in &imported.external_dependencies {
                out.push_str(&format!("    {}\n", path.display()));
            }
        }
        out.push_str("  settings review:\n");
        for item in &imported.settings_review.carried {
            out.push_str(&format!("    carried: {} = {}\n", item.field, item.detail));
        }
        for item in &imported.settings_review.needs_attention {
            out.push_str(&format!("    review: {} = {}\n", item.field, item.detail));
        }
        for item in &imported.settings_review.unsupported {
            out.push_str(&format!(
                "    unsupported: {} = {}\n",
                item.field, item.detail
            ));
        }
        if !imported.issues.is_empty() {
            out.push_str("  issues:\n");
            for issue in &imported.issues {
                out.push_str(&format!(
                    "    {:?}{}: {}\n",
                    issue.kind,
                    issue
                        .field
                        .as_deref()
                        .map(|field| format!(" {field}"))
                        .unwrap_or_default(),
                    issue.message
                ));
            }
        }
        out.push_str(&format!(
            "  inspection: layout {:?}, arch {}, source {}\n",
            inspection.layout,
            inspection.arch,
            inspection.source_root.display()
        ));
        if !inspection.external_dependencies.is_empty() {
            out.push_str("  links:\n");
            for link in &inspection.external_dependencies {
                out.push_str(&format!(
                    "    {} -> {}\n",
                    link.source.display(),
                    link.target.display()
                ));
            }
        }
        out.push_str(&format!(
            "  import: name {}, runner {}, {}\n",
            request.name,
            request.runner,
            if request.adopt {
                "adopted in place".to_owned()
            } else {
                format!(
                    "copied to {}",
                    request
                        .destination
                        .as_deref()
                        .map(|path| path.display().to_string())
                        .unwrap_or_default()
                )
            }
        ));
        out.push_str(&format!(
            "  desktop entry: {}\n",
            if self.desktop.enabled {
                self.desktop.name.as_str()
            } else {
                "no"
            }
        ));
        if global.gamemode.is_some() || !global.env.is_empty() {
            out.push_str("  global settings:\n");
            if let Some(enabled) = global.gamemode {
                out.push_str(&format!("    gamemode: {enabled}\n"));
            }
            for (key, value) in &global.env {
                out.push_str(&format!("    {key}={}\n", redact_env_value(key, value)));
            }
        }
        for line in shared_prefix_lines(&self.shared_prefix, request.adopt, true) {
            out.push_str(&format!("  shared prefix: {line}\n"));
        }
        for line in runner_dependency_lines(&self.runner_dependencies, true) {
            out.push_str(&format!("  runner dependency: {line}\n"));
        }
        out
    }
}

/// The value to show for an environment variable, hiding the ones that look
/// like credentials.
pub fn redact_env_value(key: &str, value: &str) -> String {
    let key = key.to_ascii_lowercase();
    if ["pass", "secret", "token", "key", "auth", "credential"]
        .iter()
        .any(|part| key.contains(part))
    {
        "<redacted>".to_owned()
    } else {
        value.to_owned()
    }
}

/// The note about other Heroic games sharing a prefix, as zero or one line.
pub fn shared_prefix_lines(
    shared: &[HeroicSharedPrefix],
    adopt: bool,
    verbose: bool,
) -> Vec<String> {
    if shared.is_empty() {
        return Vec::new();
    }
    let names = shared
        .iter()
        .map(|game| {
            if verbose {
                format!("{} ({})", game.title, game.id)
            } else {
                game.title.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(", ");
    vec![format!(
        "{} other Heroic game{} {} this prefix: {names}. {}",
        shared.len(),
        if shared.len() == 1 { "" } else { "s" },
        if shared.len() == 1 { "uses" } else { "use" },
        if adopt {
            "Adoption shares mutable state."
        } else {
            "Copying includes the whole prefix."
        }
    )]
}

/// The note about links a copied runner still needs, one line and then each
/// link when `verbose`.
pub fn runner_dependency_lines(deps: &[ExternalDependency], verbose: bool) -> Vec<String> {
    if deps.is_empty() {
        return Vec::new();
    }
    let mut lines = vec![format!(
        "{} external link{} remain required by the copied runner",
        deps.len(),
        if deps.len() == 1 { "" } else { "s" }
    )];
    if verbose {
        lines.extend(
            deps.iter()
                .map(|dep| format!("{} -> {}", dep.source.display(), dep.target.display())),
        );
    }
    lines
}

fn active_issues(findings: &[Finding]) -> Vec<HeroicIssue> {
    findings
        .iter()
        .filter(|finding| finding.is_active())
        .map(|finding| finding.issue.clone())
        .collect()
}

/// What a finding becomes under `policy`, for the ones still open.
fn classify(issue: &HeroicIssue, policy: Policy) -> Resolution {
    match policy {
        Policy::Strict => {
            let understood = issue.kind == HeroicIssueKind::External
                || (issue.kind == HeroicIssueKind::NeedsAttention
                    && issue.field.as_deref().is_some_and(|field| {
                        field == "enableWineWayland" || field.starts_with("gamescope.")
                    }));
            if understood {
                Resolution::Kept
            } else {
                Resolution::Blocking
            }
        }
        Policy::Defaults => match issue.kind {
            HeroicIssueKind::Invalid if issue.field.as_deref() == Some("launcherArgs") => {
                Resolution::NeedsInput(InputKind::LauncherArgs)
            }
            HeroicIssueKind::Invalid | HeroicIssueKind::Missing => Resolution::Blocking,
            HeroicIssueKind::Conflict if is_dropped_conflict(issue) => Resolution::Dropped,
            HeroicIssueKind::Conflict => Resolution::Blocking,
            HeroicIssueKind::Unsupported => Resolution::Omitted,
            HeroicIssueKind::NeedsAttention | HeroicIssueKind::External => Resolution::Kept,
        },
    }
}

/// Moves every open finding that `matches` to `resolution`, and says whether
/// there was one.
fn retire(
    findings: &mut [Finding],
    resolution: Resolution,
    matches: impl Fn(&HeroicIssue) -> bool,
) -> bool {
    let mut any = false;
    for finding in findings
        .iter_mut()
        .filter(|finding| finding.is_active() && matches(&finding.issue))
    {
        finding.resolution = resolution.clone();
        any = true;
    }
    any
}

fn clear_runner_dependency(import: &mut HeroicImport, findings: &mut [Finding]) {
    retire(findings, Resolution::Covered, |issue| {
        issue.field.as_deref() == Some("wineVersion")
            && matches!(
                issue.kind,
                HeroicIssueKind::NeedsAttention | HeroicIssueKind::External
            )
    });
    import
        .settings_review
        .needs_attention
        .retain(|item| item.field != "wineVersion");
    if let Some(source) = import.runner.source.as_ref() {
        import.external_dependencies.retain(|path| path != source);
    }
}

/// Covers the wineVersion/winePrefix Conflict issues, which just flag that
/// Heroic's own runner choice doesn't apply once wineo picks its own.
fn drop_runner_conflicts(findings: &mut [Finding]) {
    retire(findings, Resolution::Covered, |issue| {
        issue.kind == HeroicIssueKind::Conflict
            && matches!(
                issue.field.as_deref(),
                Some("wineVersion") | Some("winePrefix")
            )
    });
}

fn suppress_covered_settings(import: &mut HeroicImport, findings: &mut [Finding], runner: &Runner) {
    if !runner.is_proton() {
        return;
    }
    retire(
        findings,
        Resolution::Covered,
        |issue| matches!(issue.field.as_deref(), Some(field) if COVERED_BY_PROTON.contains(&field)),
    );
    import
        .settings_review
        .unsupported
        .retain(|item| !COVERED_BY_PROTON.contains(&item.field.as_str()));
    import
        .settings_review
        .carried
        .retain(|item| !COVERED_BY_PROTON.contains(&item.field.as_str()));
}

/// Heroic's `autoInstallDxvk`/`autoInstallVkd3d` become `dxvk`/`vkd3d =
/// "latest"` for a Wine (or system) chosen runner, which install their own
/// way; a Proton runner still drops them, which `suppress_covered_settings`
/// already handles.
fn carry_over_components(import: &mut HeroicImport, findings: &mut [Finding], runner: &Runner) {
    if runner.is_proton() {
        return;
    }
    if take_unsupported_issue(import, findings, "autoInstallDxvk") {
        import.dxvk = Some(crate::components::LATEST.to_owned());
        import.settings_review.carried.push(ReviewItem {
            field: "autoInstallDxvk".to_owned(),
            detail: "dxvk=latest".to_owned(),
        });
    }
    if take_unsupported_issue(import, findings, "autoInstallVkd3d") {
        import.vkd3d = Some(crate::components::LATEST.to_owned());
        import.settings_review.carried.push(ReviewItem {
            field: "autoInstallVkd3d".to_owned(),
            detail: "vkd3d=latest".to_owned(),
        });
    }
}

fn carry_over_wayland(import: &mut HeroicImport, findings: &mut Vec<Finding>, runner: &Runner) {
    if !import.enable_wine_wayland {
        if import.wayland == Some(false) {
            import.settings_review.carried.push(ReviewItem {
                field: "enableWineWayland".to_owned(),
                detail: "wayland=false".to_owned(),
            });
        }
        return;
    }
    if !runner.supports_wayland() {
        let detail = format!("{} does not support Wine Wayland; skipped", runner.name());
        let issue = HeroicIssue {
            kind: HeroicIssueKind::NeedsAttention,
            field: Some("enableWineWayland".to_owned()),
            message: detail.clone(),
        };
        findings.push(Finding {
            resolution: classify(&issue, Policy::Defaults),
            issue,
        });
        import.settings_review.needs_attention.push(ReviewItem {
            field: "enableWineWayland".to_owned(),
            detail,
        });
        return;
    }
    import.wayland = Some(true);
    import.settings_review.carried.push(ReviewItem {
        field: "enableWineWayland".to_owned(),
        detail: "wayland=true".to_owned(),
    });
    if import.enable_hdr {
        let key = if runner.is_proton() {
            "PROTON_ENABLE_HDR"
        } else {
            "DXVK_HDR"
        };
        import
            .env
            .entry(key.to_owned())
            .or_insert_with(|| "1".to_owned());
        import.settings_review.carried.push(ReviewItem {
            field: "enableHDR".to_owned(),
            detail: format!("{key} retained"),
        });
        retire(findings, Resolution::Carried, |issue| {
            issue.field.as_deref() == Some("enableHDR")
        });
        import
            .settings_review
            .unsupported
            .retain(|item| item.field != "enableHDR");
    }
}

/// Carries every issue for `field` if Heroic's own setting was on, and says
/// whether it was.
fn take_unsupported_issue(
    import: &mut HeroicImport,
    findings: &mut [Finding],
    field: &str,
) -> bool {
    let had = findings.iter().any(|finding| {
        finding.is_active()
            && finding.issue.kind == HeroicIssueKind::Unsupported
            && finding.issue.field.as_deref() == Some(field)
    });
    if had {
        retire(findings, Resolution::Carried, |issue| {
            issue.field.as_deref() == Some(field)
        });
        import
            .settings_review
            .unsupported
            .retain(|item| item.field != field);
    }
    had
}

#[cfg(test)]
pub(super) mod tests {
    use crate::test_script::write_script;
    use std::fs;

    use tempfile::tempdir;

    use super::*;
    use crate::heroic::{
        HeroicGameKind, HeroicRunner, HeroicRunnerKind, InputFingerprint, SettingsReview,
    };

    pub(crate) fn paths(home: &Path) -> Paths {
        Paths::from_lookup(|key| (key == "HOME").then(|| home.as_os_str().to_owned())).unwrap()
    }

    /// A Heroic import of `Game` in a prefix at `root`, which is created.
    pub(crate) fn reviewed_import(root: &Path) -> HeroicImport {
        fs::create_dir_all(root.join("drive_c")).unwrap();
        fs::write(
            root.join("system.reg"),
            "WINE REGISTRY Version 2\n#arch=win64\n",
        )
        .unwrap();
        HeroicImport {
            id: "game".to_owned(),
            title: "Game".to_owned(),
            prefix: root.to_owned(),
            runner: HeroicRunner {
                kind: HeroicRunnerKind::System,
                name: "wine".to_owned(),
                path: None,
                source: None,
            },
            kind: HeroicGameKind::Windows,
            exe: root.join("drive_c/game.exe"),
            args: Vec::new(),
            launcher_args: String::new(),
            cwd: Some(root.join("drive_c")),
            env: BTreeMap::new(),
            gamemode: Some(true),
            wayland: None,
            mangohud: None,
            gamescope: None,
            hooks: None,
            enable_wine_wayland: false,
            enable_hdr: false,
            dxvk: None,
            vkd3d: None,
            issues: Vec::new(),
            external_dependencies: Vec::new(),
            fingerprint: InputFingerprint {
                digest: "heroic".to_owned(),
                files: Vec::new(),
            },
            settings_review: SettingsReview::default(),
        }
    }

    pub(crate) fn fake_proton_runner(root: &Path, name: &str) {
        let dir = root.join(name);
        fs::create_dir_all(dir.join("files/bin")).unwrap();
        let proton = dir.join("proton");
        write_script(&proton, "#!/bin/sh\n");
        for executable in ["wine", "wineserver"] {
            let path = dir.join("files/bin").join(executable);
            write_script(&path, "#!/bin/sh\n");
        }
        fs::write(dir.join("version"), format!("1 {name}\n")).unwrap();
    }

    pub(crate) fn fake_wine_runner(root: &Path, name: &str) -> Runner {
        let dir = root.join(name);
        fs::create_dir_all(dir.join("bin")).unwrap();
        for executable in ["wine", "wineserver"] {
            let path = dir.join("bin").join(executable);
            write_script(&path, "#!/bin/sh\n");
        }
        Runners::at(root.to_owned()).resolve(name).unwrap()
    }

    /// Plans `imported` in `root`, named `game` and copied to `root/copy`.
    pub(crate) fn planned(root: &Path, imported: HeroicImport, runner: &Runner) -> GamePlan {
        if runner.is_proton() {
            // A Proton prefix keeps its wine prefix in `pfx`.
            fs::create_dir_all(root.join("pfx/drive_c")).unwrap();
            fs::write(
                root.join("pfx/system.reg"),
                "WINE REGISTRY Version 2\n#arch=win64\n",
            )
            .unwrap();
        }
        GamePlan::with_runner(
            &paths(root),
            &GlobalConfig::default(),
            imported,
            Vec::new(),
            runner.name(),
            runner.clone(),
            Options {
                name: Some("game".to_owned()),
                destination: Some(root.join("copy")),
                ..Options::default()
            },
        )
        .unwrap()
    }

    fn issue(kind: HeroicIssueKind, field: &str, message: &str) -> HeroicIssue {
        HeroicIssue {
            kind,
            field: Some(field.to_owned()),
            message: message.to_owned(),
        }
    }

    fn resolution_of<'a>(plan: &'a GamePlan, field: &str) -> &'a Resolution {
        &plan
            .findings()
            .iter()
            .find(|finding| finding.issue.field.as_deref() == Some(field))
            .unwrap()
            .resolution
    }

    #[test]
    fn wayland_and_hdr_use_the_chosen_runner_without_overriding_heroic_env() {
        let root = tempdir().unwrap();
        let runners = root.path().join("runners");
        let wine = fake_wine_runner(&runners, "wine-tkg");
        let mut imported = reviewed_import(root.path());
        imported.enable_wine_wayland = true;
        imported.enable_hdr = true;
        let plan = planned(root.path(), imported, &wine);
        assert_eq!(plan.resolved().wayland, None);
        assert_eq!(plan.resolved().env.get("DXVK_HDR"), None);
        assert!(
            plan.resolved()
                .issues
                .iter()
                .any(|issue| issue.field.as_deref() == Some("enableWineWayland"))
        );
        assert_eq!(plan.settings().wayland, None);

        fs::create_dir_all(runners.join("wine-tkg/lib/wine/x86_64-unix")).unwrap();
        fs::write(
            runners.join("wine-tkg/lib/wine/x86_64-unix/winewayland.so"),
            "",
        )
        .unwrap();
        let wine = Runners::at(runners.clone()).resolve("wine-tkg").unwrap();
        let mut imported = reviewed_import(root.path());
        imported.enable_wine_wayland = true;
        imported.enable_hdr = true;
        imported
            .env
            .insert("DXVK_HDR".to_owned(), "custom".to_owned());
        let plan = planned(root.path(), imported, &wine);
        assert_eq!(plan.resolved().wayland, Some(true));
        assert_eq!(plan.resolved().env["DXVK_HDR"], "custom");
        assert_eq!(plan.settings().wayland, Some(true));
        assert_eq!(plan.settings().env["DXVK_HDR"], "custom");

        fake_proton_runner(&runners, "proton-test");
        write_script(
            runners.join("proton-test/proton"),
            "#!/bin/sh\n# PROTON_ENABLE_WAYLAND\n",
        );
        let proton = Runners::at(runners).resolve("proton-test").unwrap();
        let mut imported = reviewed_import(root.path());
        imported.enable_wine_wayland = true;
        imported.enable_hdr = true;
        let plan = planned(root.path(), imported, &proton);
        assert_eq!(plan.resolved().wayland, Some(true));
        assert_eq!(plan.resolved().env["PROTON_ENABLE_HDR"], "1");
        assert!(!plan.resolved().env.contains_key("DXVK_HDR"));
        let mut imported = reviewed_import(root.path());
        imported.enable_hdr = true;
        let plan = planned(root.path(), imported, &proton);
        assert_eq!(plan.resolved().env.get("PROTON_ENABLE_HDR"), None);
    }

    #[test]
    fn hdr_carried_with_wayland_is_not_a_finding_any_more() {
        let root = tempdir().unwrap();
        let runners = root.path().join("runners");
        fake_proton_runner(&runners, "proton-test");
        write_script(
            runners.join("proton-test/proton"),
            "#!/bin/sh\n# PROTON_ENABLE_WAYLAND\n",
        );
        let proton = Runners::at(runners).resolve("proton-test").unwrap();
        let mut imported = reviewed_import(root.path());
        imported.enable_wine_wayland = true;
        imported.enable_hdr = true;
        imported.issues = vec![issue(
            HeroicIssueKind::Unsupported,
            "enableHDR",
            "HDR handling is runner-specific",
        )];
        let plan = planned(root.path(), imported, &proton);
        assert_eq!(resolution_of(&plan, "enableHDR"), &Resolution::Carried);
        assert!(plan.resolved().issues.is_empty());
        // Heroic's own report is untouched.
        assert_eq!(plan.heroic().issues.len(), 1);
    }

    #[test]
    fn review_evidence_survives_bulk_defaults_and_reaches_the_log() {
        let root = tempdir().unwrap();
        let runner = fake_wine_runner(&root.path().join("runners"), "wine-tkg");
        let mut imported = reviewed_import(root.path());
        imported.args = vec!["one word".to_owned(), "say\"hello".to_owned()];
        imported.issues = vec![
            issue(HeroicIssueKind::Conflict, "WINEPREFIX", "managed by wineo"),
            issue(
                HeroicIssueKind::Unsupported,
                "wrapperOptions",
                "not imported",
            ),
            issue(
                HeroicIssueKind::NeedsAttention,
                "autoInstallDxvkNvapi",
                "Wine NVAPI requires dxvk-nvapi DLLs; enabling variables does not install them",
            ),
        ];
        imported.settings_review.unsupported.push(ReviewItem {
            field: "wrapperOptions".to_owned(),
            detail: "ignored".to_owned(),
        });
        let mut plan = planned(root.path(), imported, &runner);
        plan.resolve(Policy::Defaults);
        assert_eq!(plan.skip_reason(), None);
        let warnings = plan.warnings();
        assert!(warnings.contains(&Warning::Omitted(1)), "{warnings:?}");
        assert!(warnings.contains(&Warning::Dropped(1)), "{warnings:?}");
        assert!(
            warnings
                .iter()
                .any(|warning| matches!(warning, Warning::NvapiNotInstalled(_))),
            "{warnings:?}"
        );
        // Deciding what happens to the issues doesn't hide them from review.
        assert_eq!(plan.resolved().issues.len(), 3);
        assert_eq!(resolution_of(&plan, "WINEPREFIX"), &Resolution::Dropped);
        assert_eq!(resolution_of(&plan, "wrapperOptions"), &Resolution::Omitted);
        assert_eq!(
            resolution_of(&plan, "autoInstallDxvkNvapi"),
            &Resolution::Kept
        );

        let source = HeroicSource::from_config_dir_with_home(
            root.path().join("heroic"),
            root.path().to_owned(),
        );
        let log = plan.log_text(&GlobalConfig::default(), &source);
        assert!(log.contains("wrapperOptions"), "{log}");
        assert!(log.contains("WINEPREFIX"), "{log}");
        assert!(!log.contains("\u{1b}["), "{log}");
    }

    #[test]
    fn resolved_layout_conflicts_do_not_reappear_as_current_errors() {
        let root = tempdir().unwrap();
        let runner = fake_wine_runner(&root.path().join("runners"), "wine-tkg");
        let mut imported = reviewed_import(root.path());
        imported.issues = ["wineVersion", "winePrefix", "WINEPREFIX"]
            .into_iter()
            .map(|field| {
                issue(
                    HeroicIssueKind::Conflict,
                    field,
                    &format!("{field} conflicts"),
                )
            })
            .collect();
        let plan = planned(root.path(), imported, &runner);
        let issues = &plan.resolved().issues;
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].field.as_deref(), Some("WINEPREFIX"));
        assert_eq!(plan.warnings(), vec![Warning::Dropped(1)]);
        assert_eq!(resolution_of(&plan, "wineVersion"), &Resolution::Covered);
        assert_eq!(resolution_of(&plan, "winePrefix"), &Resolution::Covered);
        assert_eq!(plan.blocking().count(), 0);
    }

    #[test]
    fn covered_proton_settings_are_suppressed() {
        let root = tempdir().unwrap();
        let runners_root = root.path().join("runners");
        fake_proton_runner(&runners_root, "GE-Proton9-20");
        let proton = Runners::at(runners_root.clone())
            .resolve("GE-Proton9-20")
            .unwrap();
        let wine = fake_wine_runner(&runners_root, "wine-tkg");

        let mut imported = reviewed_import(root.path());
        imported.runner.kind = HeroicRunnerKind::Proton;
        for field in COVERED_BY_PROTON {
            imported.issues.push(issue(
                HeroicIssueKind::Unsupported,
                field,
                "Heroic component installation is not imported",
            ));
            imported.settings_review.unsupported.push(ReviewItem {
                field: field.to_owned(),
                detail: "true".to_owned(),
            });
        }
        let plan_proton = planned(root.path(), imported, &proton);
        assert!(plan_proton.resolved().issues.is_empty());
        assert!(
            plan_proton
                .resolved()
                .settings_review
                .unsupported
                .is_empty()
        );
        assert!(
            plan_proton
                .findings()
                .iter()
                .all(|finding| finding.resolution == Resolution::Covered)
        );

        // Suppression follows the chosen wineo runner, not what Heroic itself
        // recorded: a Wine choice leaves the issue for the component carry-over.
        let mut wine_import = reviewed_import(root.path());
        wine_import.runner.kind = HeroicRunnerKind::Proton;
        wine_import.issues.push(issue(
            HeroicIssueKind::Unsupported,
            "autoInstallVkd3d",
            "Heroic component installation is not imported",
        ));
        wine_import.issues.push(issue(
            HeroicIssueKind::Unsupported,
            "autoInstallDxvkNvapi",
            "Heroic component installation is not imported",
        ));
        let plan_wine = planned(root.path(), wine_import, &wine);
        assert_eq!(plan_wine.resolved().issues.len(), 1);
        assert_eq!(plan_wine.resolved().vkd3d.as_deref(), Some("latest"));
    }

    #[test]
    fn carry_over_components_sets_dxvk_and_vkd3d_for_a_wine_runner() {
        let root = tempdir().unwrap();
        let wine = fake_wine_runner(&root.path().join("runners"), "wine-tkg");
        let mut imported = reviewed_import(root.path());
        for field in ["autoInstallDxvk", "autoInstallVkd3d"] {
            imported.issues.push(issue(
                HeroicIssueKind::Unsupported,
                field,
                "Heroic component installation is not imported",
            ));
        }
        let plan = planned(root.path(), imported, &wine);
        assert_eq!(plan.resolved().dxvk.as_deref(), Some("latest"));
        assert_eq!(plan.resolved().vkd3d.as_deref(), Some("latest"));
        assert!(plan.resolved().issues.is_empty());
        assert_eq!(
            resolution_of(&plan, "autoInstallDxvk"),
            &Resolution::Carried
        );
        assert_eq!(plan.settings().dxvk.as_deref(), Some("latest"));
        assert_eq!(plan.settings().vkd3d.as_deref(), Some("latest"));
    }

    #[test]
    fn carry_over_components_leaves_a_proton_choice_alone() {
        let root = tempdir().unwrap();
        let runners_root = root.path().join("runners");
        fake_proton_runner(&runners_root, "GE-Proton9-20");
        let proton = Runners::at(runners_root).resolve("GE-Proton9-20").unwrap();
        let mut imported = reviewed_import(root.path());
        imported.issues.push(issue(
            HeroicIssueKind::Unsupported,
            "wrapperOptions",
            "Heroic wrappers require manual setup",
        ));
        let plan = planned(root.path(), imported, &proton);
        assert_eq!(plan.resolved().dxvk, None);
        assert_eq!(plan.resolved().issues.len(), 1);
    }

    #[test]
    fn settings_record_heroic_id_and_desktop_choice() {
        let root = tempdir().unwrap();
        let runner = fake_wine_runner(&root.path().join("runners"), "wine-tkg");
        let mut plan = planned(root.path(), reviewed_import(root.path()), &runner);
        plan.set_desktop_enabled(true);
        plan.set_desktop_name("Game").unwrap();
        let settings = plan.settings();
        assert_eq!(settings.heroic_id.as_deref(), Some("game"));
        let target = settings.targets.values().next().unwrap();
        assert!(target.desktop);
        assert_eq!(target.name.as_deref(), Some("Game"));

        plan.set_desktop_enabled(false);
        let settings = plan.settings();
        let target = settings.targets.values().next().unwrap();
        assert!(!target.desktop);
        assert_eq!(target.name, None);
    }

    #[test]
    fn edited_paths_expand_home_and_preserve_prefix_relative_paths() {
        let root = tempdir().unwrap();
        let paths = paths(root.path());
        let prefix = root.path().join("prefix");
        let runner = fake_wine_runner(&root.path().join("runners"), "wine-tkg");
        let mut plan = planned(&prefix, reviewed_import(&prefix), &runner);
        let exe = plan
            .resolve_path(&paths, "~/Games/My Game/game.exe")
            .unwrap();
        plan.set_executable(exe).unwrap();
        assert_eq!(
            plan.resolved().exe,
            root.path().join("Games/My Game/game.exe")
        );
        let cwd = plan.resolve_path(&paths, "~/Games/My Game").unwrap();
        plan.set_working_directory(Some(cwd));
        assert_eq!(plan.resolved().cwd, Some(root.path().join("Games/My Game")));
        let exe = plan
            .resolve_path(&paths, "drive_c/Program Files/game.exe")
            .unwrap();
        plan.set_executable(exe).unwrap();
        assert_eq!(
            plan.resolved().exe,
            prefix.join("drive_c/Program Files/game.exe")
        );
        let settings = plan.settings();
        let target = settings.targets.values().next().unwrap();
        // Files inside the prefix follow a copy to its destination.
        assert_eq!(
            target.exe,
            prefix
                .join("copy/drive_c/Program Files/game.exe")
                .display()
                .to_string()
        );
        assert_eq!(
            target.cwd.as_deref(),
            Some(root.path().join("Games/My Game").to_str().unwrap())
        );
        assert!(matches!(
            plan.set_executable(PathBuf::new()),
            Err(PlanError::EmptyExecutable)
        ));
    }

    #[test]
    fn a_blank_prefix_import_gets_no_findings() {
        let root = tempdir().unwrap();
        let runner = fake_wine_runner(&root.path().join("runners"), "wine-tkg");
        let mut plan = planned(root.path(), reviewed_import(root.path()), &runner);
        plan.resolve(Policy::Defaults);
        assert!(plan.findings().is_empty());
        assert_eq!(plan.skip_reason(), None);
        assert!(plan.warnings().is_empty());
    }

    #[test]
    fn defaults_drop_conflicts_and_omit_unsupported_but_keep_attention_as_a_warning() {
        let root = tempdir().unwrap();
        let runner = fake_wine_runner(&root.path().join("runners"), "wine-tkg");
        let mut imported = reviewed_import(root.path());
        imported.issues = vec![
            issue(
                HeroicIssueKind::Conflict,
                "useGameMode",
                "conflicts with wineo's own setting",
            ),
            issue(
                HeroicIssueKind::Unsupported,
                "wrapperOptions",
                "not imported",
            ),
            issue(
                HeroicIssueKind::NeedsAttention,
                "wrapperOptions2",
                "review this manually",
            ),
        ];
        let mut plan = planned(root.path(), imported, &runner);
        plan.resolve(Policy::Defaults);
        assert_eq!(plan.skip_reason(), None);
        assert_eq!(resolution_of(&plan, "useGameMode"), &Resolution::Dropped);
        assert_eq!(resolution_of(&plan, "wrapperOptions"), &Resolution::Omitted);
        assert_eq!(resolution_of(&plan, "wrapperOptions2"), &Resolution::Kept);
        assert!(plan.warnings().iter().any(
            |warning| matches!(warning, Warning::NeedsAttention(issues) if issues.len() == 1)
        ));
    }

    #[test]
    fn defaults_name_eac_and_battleye_specifically() {
        let root = tempdir().unwrap();
        let runner = fake_wine_runner(&root.path().join("runners"), "wine-tkg");
        let mut imported = reviewed_import(root.path());
        imported.issues = vec![
            issue(HeroicIssueKind::Unsupported, "eacRuntime", "not imported"),
            issue(
                HeroicIssueKind::Unsupported,
                "battlEyeRuntime",
                "not imported",
            ),
        ];
        let mut plan = planned(root.path(), imported, &runner);
        plan.resolve(Policy::Defaults);
        assert_eq!(plan.warnings(), vec![Warning::AntiCheatNotImported]);
    }

    #[test]
    fn defaults_reject_invalid_settings_with_their_message() {
        let root = tempdir().unwrap();
        let runner = fake_wine_runner(&root.path().join("runners"), "wine-tkg");
        let mut imported = reviewed_import(root.path());
        imported.issues = vec![issue(
            HeroicIssueKind::Invalid,
            "wrapperOptions",
            "cannot be parsed",
        )];
        let mut plan = planned(root.path(), imported, &runner);
        plan.resolve(Policy::Defaults);
        assert_eq!(
            plan.skip_reason(),
            Some(SkipReason::Blocked("cannot be parsed".to_owned()))
        );
    }

    #[test]
    fn defaults_skip_invalid_launcher_args_unless_someone_corrects_them() {
        let root = tempdir().unwrap();
        let runner = fake_wine_runner(&root.path().join("runners"), "wine-tkg");
        let mut imported = reviewed_import(root.path());
        imported.launcher_args = "-a \"oops".to_owned();
        imported.issues = vec![issue(
            HeroicIssueKind::Invalid,
            "launcherArgs",
            "unbalanced quotes",
        )];
        let mut plan = planned(root.path(), imported, &runner);
        plan.resolve(Policy::Defaults);
        assert_eq!(plan.skip_reason(), Some(SkipReason::LauncherArgs));
        assert_eq!(plan.needs_input(), Some(InputKind::LauncherArgs));
        assert_eq!(plan.blocking().count(), 0);

        assert!(matches!(
            plan.set_launcher_args("-a \"oops"),
            Err(PlanError::InvalidArguments(_))
        ));
        assert_eq!(plan.needs_input(), Some(InputKind::LauncherArgs));
        plan.set_launcher_args("-a \"fixed one\"").unwrap();
        assert_eq!(plan.needs_input(), None);
        assert_eq!(plan.resolved().args, ["-a", "fixed one"]);
        assert_eq!(plan.resolved().launcher_args, "-a \"fixed one\"");
        assert_eq!(resolution_of(&plan, "launcherArgs"), &Resolution::Fixed);
        // A fixed finding stays part of the record, and settling again keeps it.
        plan.resolve(Policy::Defaults);
        assert_eq!(resolution_of(&plan, "launcherArgs"), &Resolution::Fixed);
        assert_eq!(plan.resolved().issues.len(), 1);
    }

    #[test]
    fn strict_blocks_everything_but_external_and_wayland_attention() {
        let root = tempdir().unwrap();
        let runner = fake_wine_runner(&root.path().join("runners"), "wine-tkg");
        let mut imported = reviewed_import(root.path());
        imported.issues = vec![
            issue(
                HeroicIssueKind::External,
                "external files",
                "game files or saves remain outside the imported prefix",
            ),
            issue(
                HeroicIssueKind::NeedsAttention,
                "gamescope.width",
                "review the size",
            ),
            issue(
                HeroicIssueKind::NeedsAttention,
                "targetExe",
                "executable path was relative",
            ),
            issue(
                HeroicIssueKind::Unsupported,
                "wrapperOptions",
                "not imported",
            ),
            issue(HeroicIssueKind::Conflict, "WINEPREFIX", "managed by wineo"),
        ];
        let mut plan = planned(root.path(), imported, &runner);
        plan.resolve(Policy::Strict);
        let blocking: Vec<&str> = plan
            .blocking()
            .map(|finding| finding.issue.field.as_deref().unwrap())
            .collect();
        assert_eq!(blocking, ["targetExe", "wrapperOptions", "WINEPREFIX"]);
        assert_eq!(resolution_of(&plan, "external files"), &Resolution::Kept);
        assert_eq!(resolution_of(&plan, "gamescope.width"), &Resolution::Kept);

        // Settling by defaults again frees what strict blocked.
        plan.resolve(Policy::Defaults);
        assert_eq!(
            plan.blocking()
                .map(|f| f.issue.field.clone())
                .collect::<Vec<_>>(),
            Vec::<Option<String>>::new()
        );
    }

    #[test]
    fn strict_blocks_launcher_args_instead_of_asking() {
        let root = tempdir().unwrap();
        let runner = fake_wine_runner(&root.path().join("runners"), "wine-tkg");
        let mut imported = reviewed_import(root.path());
        imported.issues = vec![issue(
            HeroicIssueKind::Invalid,
            "launcherArgs",
            "unbalanced quotes",
        )];
        let mut plan = planned(root.path(), imported, &runner);
        plan.resolve(Policy::Strict);
        assert_eq!(plan.blocking().count(), 1);
        assert_eq!(plan.needs_input(), None);
    }

    #[test]
    fn missing_settings_block_under_defaults_too() {
        let root = tempdir().unwrap();
        let runner = fake_wine_runner(&root.path().join("runners"), "wine-tkg");
        let mut imported = reviewed_import(root.path());
        imported.issues = vec![issue(HeroicIssueKind::Missing, "exe", "no executable set")];
        let mut plan = planned(root.path(), imported, &runner);
        plan.resolve(Policy::Defaults);
        assert_eq!(plan.blocking().count(), 1);
        assert_eq!(
            plan.skip_reason(),
            Some(SkipReason::Blocked("no executable set".to_owned()))
        );
    }

    #[test]
    fn warnings_come_in_the_order_they_are_shown() {
        let root = tempdir().unwrap();
        let runner = fake_wine_runner(&root.path().join("runners"), "wine-tkg");
        let mut imported = reviewed_import(root.path());
        imported.issues = vec![
            issue(HeroicIssueKind::NeedsAttention, "targetExe", "relative"),
            issue(HeroicIssueKind::Unsupported, "eacRuntime", "not imported"),
            issue(
                HeroicIssueKind::Unsupported,
                "wrapperOptions",
                "not imported",
            ),
            issue(HeroicIssueKind::Unsupported, "disableUMU", "umu is off"),
            issue(HeroicIssueKind::Conflict, "WINEPREFIX", "managed"),
        ];
        let shared = vec![HeroicSharedPrefix {
            id: "other".to_owned(),
            title: "Other".to_owned(),
        }];
        let plan = GamePlan::with_runner(
            &paths(root.path()),
            &GlobalConfig::default(),
            imported,
            shared.clone(),
            runner.name(),
            runner.clone(),
            Options::default(),
        )
        .unwrap();
        assert_eq!(
            plan.warnings(),
            vec![
                Warning::SharedPrefix(shared),
                Warning::UmuDisabled("umu is off".to_owned()),
                Warning::AntiCheatNotImported,
                Warning::Omitted(1),
                Warning::Dropped(1),
                Warning::NeedsAttention(vec![issue(
                    HeroicIssueKind::NeedsAttention,
                    "targetExe",
                    "relative"
                )]),
            ]
        );
    }

    #[test]
    fn renaming_moves_a_default_destination_and_leaves_a_chosen_one() {
        let root = tempdir().unwrap();
        let paths = paths(root.path());
        let global = GlobalConfig::default();
        let runner = fake_wine_runner(&root.path().join("runners"), "wine-tkg");
        let build = |destination: Option<PathBuf>| {
            GamePlan::with_runner(
                &paths,
                &global,
                reviewed_import(root.path()),
                Vec::new(),
                runner.name(),
                runner.clone(),
                Options {
                    name: Some("game".to_owned()),
                    destination,
                    ..Options::default()
                },
            )
            .unwrap()
        };
        let mut defaulted = build(None);
        let default_root = global.prefix_root(&paths).unwrap();
        assert_eq!(defaulted.destination(), default_root.join("game"));
        defaulted.set_name(&paths, &global, "renamed").unwrap();
        assert_eq!(defaulted.name(), "renamed");
        assert_eq!(defaulted.destination(), default_root.join("renamed"));

        let mut chosen = build(Some(root.path().join("elsewhere")));
        chosen.set_name(&paths, &global, "renamed").unwrap();
        assert_eq!(chosen.destination(), root.path().join("elsewhere"));
        assert!(matches!(
            chosen.set_name(&paths, &global, "bad name"),
            Err(PlanError::InvalidName(_))
        ));
        assert_eq!(chosen.name(), "renamed");
    }

    #[test]
    fn adopting_and_copying_switch_the_destination() {
        let root = tempdir().unwrap();
        let paths = paths(root.path());
        let global = GlobalConfig::default();
        let runner = fake_wine_runner(&root.path().join("runners"), "wine-tkg");
        let mut plan = planned(root.path(), reviewed_import(root.path()), &runner);
        assert!(!plan.is_adopt());
        plan.set_adopt();
        assert!(plan.is_adopt());
        assert_eq!(plan.request().destination, None);
        assert_eq!(plan.destination(), plan.inspection().source_root);

        // Copying again without a place picks the configured prefix root.
        plan.set_copy(&paths, &global, None).unwrap();
        assert!(!plan.is_adopt());
        assert_eq!(
            plan.destination(),
            global.prefix_root(&paths).unwrap().join("game")
        );
        plan.set_copy(&paths, &global, Some(root.path().join("there")))
            .unwrap();
        assert_eq!(plan.destination(), root.path().join("there"));
    }

    #[test]
    fn environment_entries_are_validated() {
        let root = tempdir().unwrap();
        let runner = fake_wine_runner(&root.path().join("runners"), "wine-tkg");
        let mut plan = planned(root.path(), reviewed_import(root.path()), &runner);
        plan.set_environment(["A=1", "B=two=2"]).unwrap();
        assert_eq!(plan.resolved().env["B"], "two=2");
        assert!(matches!(
            plan.set_environment(["NOPE"]),
            Err(PlanError::EnvironmentFormat)
        ));
        assert!(matches!(
            plan.set_environment(["=x"]),
            Err(PlanError::EnvironmentName(_))
        ));
        let error = plan.set_environment(["WINEPREFIX=/x"]).unwrap_err();
        assert_eq!(
            error.to_string(),
            "WINEPREFIX is managed by wineo and cannot be imported"
        );
        // A refused edit leaves the environment as it was.
        assert_eq!(plan.resolved().env.len(), 2);
    }

    #[test]
    fn a_missing_build_is_told_apart_from_other_failures() {
        let root = tempdir().unwrap();
        let paths = paths(root.path());
        let global = GlobalConfig::default();
        let runner = fake_wine_runner(&root.path().join("runners"), "wine-tkg");
        let mut imported = reviewed_import(root.path());
        imported.dxvk = Some("latest".to_owned());
        let plan = planned(root.path(), imported, &runner);
        let missing = plan.missing_build(&paths, &global).unwrap().unwrap();
        assert_eq!(missing.component, Component::Dxvk);
        assert_eq!(missing.version, None);
        assert!(
            missing
                .message()
                .contains("Get one with: wineo download dxvk"),
            "{}",
            missing.message()
        );
        assert!(plan.check_builds(&paths, &global).is_err());

        let mut imported = reviewed_import(root.path());
        imported.dxvk = Some("dxvk-2.7".to_owned());
        let plan = planned(root.path(), imported, &runner);
        let missing = plan.missing_build(&paths, &global).unwrap().unwrap();
        assert_eq!(missing.version.as_deref(), Some("dxvk-2.7"));

        let plan = planned(root.path(), reviewed_import(root.path()), &runner);
        assert!(plan.missing_build(&paths, &global).unwrap().is_none());
        assert!(plan.check_builds(&paths, &global).is_ok());
    }

    #[test]
    fn import_log_records_every_field_and_link() {
        let root = tempdir().unwrap();
        let runner = fake_wine_runner(&root.path().join("runners"), "wine-tkg");
        let source = HeroicSource::from_config_dir_with_home(
            root.path().join("heroic"),
            root.path().to_owned(),
        );
        let mut imported = reviewed_import(root.path());
        imported.env.insert("FOO".to_owned(), "bar".to_owned());
        imported
            .env
            .insert("API_TOKEN".to_owned(), "hunter2".to_owned());
        let mut plan = planned(root.path(), imported, &runner);
        plan.set_desktop_enabled(true);
        plan.set_desktop_name("Game").unwrap();
        plan.set_runner_dependencies(vec![ExternalDependency {
            source: PathBuf::from("/prefix/a"),
            target: PathBuf::from("/heroic/tools/a"),
        }]);
        let body = plan.log_text(&GlobalConfig::default(), &source);
        assert!(body.contains("Heroic game: Game (game)"), "{body}");
        assert!(body.contains("FOO=bar"), "{body}");
        assert!(body.contains("API_TOKEN=<redacted>"), "{body}");
        assert!(!body.contains("hunter2"), "{body}");
        assert!(body.contains("desktop entry: Game"), "{body}");
        assert!(
            body.contains("runner dependency: 1 external link remains required")
                || body.contains("runner dependency: 1 external link remain required"),
            "{body}"
        );
    }

    #[test]
    fn plans_keep_heroics_report_untouched() {
        let root = tempdir().unwrap();
        let runner = fake_wine_runner(&root.path().join("runners"), "wine-tkg");
        let mut imported = reviewed_import(root.path());
        imported.issues = vec![issue(
            HeroicIssueKind::NeedsAttention,
            "wineVersion",
            "runner is stored outside wineo",
        )];
        let mut plan = planned(root.path(), imported, &runner);
        plan.set_gamemode(false);
        assert_eq!(plan.heroic().issues.len(), 1);
        assert_eq!(plan.heroic().gamemode, Some(true));
        assert_eq!(plan.resolved().gamemode, Some(false));
        assert!(plan.resolved().issues.is_empty());
        assert_eq!(resolution_of(&plan, "wineVersion"), &Resolution::Covered);
    }

    #[test]
    fn launch_setting_edits_fix_only_their_own_findings() {
        let root = tempdir().unwrap();
        let runner = fake_wine_runner(&root.path().join("runners"), "wine-tkg");
        let mut imported = reviewed_import(root.path());
        imported.env.insert("OLD".to_owned(), "value".to_owned());
        imported.issues = vec![
            issue(
                HeroicIssueKind::NeedsAttention,
                "enviromentOptions[0]",
                "review env",
            ),
            issue(
                HeroicIssueKind::NeedsAttention,
                "gamescope.width",
                "review width",
            ),
            issue(HeroicIssueKind::NeedsAttention, "enableHDR", "review HDR"),
        ];
        let mut plan = planned(root.path(), imported.clone(), &runner);
        plan.resolve(Policy::Defaults);
        plan.set_environment(["NEW=other"]).unwrap();
        plan.set_gamescope(Some(GamescopeSetting::On(Default::default())));
        plan.set_mangohud(true);
        assert_eq!(plan.heroic().env, imported.env);
        assert_eq!(plan.heroic().issues, imported.issues);
        assert_eq!(plan.resolved().env["NEW"], "other");
        assert_eq!(plan.resolved().mangohud, Some(true));
        assert_eq!(
            resolution_of(&plan, "enviromentOptions[0]"),
            &Resolution::Fixed
        );
        assert_eq!(resolution_of(&plan, "gamescope.width"), &Resolution::Fixed);
        assert_ne!(resolution_of(&plan, "enableHDR"), &Resolution::Fixed);
        assert!(plan.set_environment(["BROKEN"]).is_err());
        assert_eq!(plan.resolved().env["NEW"], "other");
    }

    #[test]
    fn explicit_wayland_and_hdr_edits_follow_the_selected_runner() {
        let root = tempdir().unwrap();
        let runners = root.path().join("runners");
        fake_proton_runner(&runners, "proton-test");
        write_script(
            runners.join("proton-test/proton"),
            "#!/bin/sh\n# PROTON_ENABLE_WAYLAND\n",
        );
        let proton = Runners::at(runners).resolve("proton-test").unwrap();
        let imported = reviewed_import(root.path());
        let mut plan = planned(root.path(), imported, &proton);
        plan.set_wayland(true).unwrap();
        plan.set_hdr(true).unwrap();
        assert_eq!(plan.settings().wayland, Some(true));
        assert_eq!(plan.settings().env["PROTON_ENABLE_HDR"], "1");
        assert!(!plan.heroic().enable_wine_wayland);
        assert!(!plan.heroic().enable_hdr);
        plan.set_wayland(false).unwrap();
        assert_eq!(plan.settings().wayland, Some(false));
        assert!(!plan.settings().env.contains_key("PROTON_ENABLE_HDR"));
        assert!(!plan.resolved().enable_hdr);
    }
}
