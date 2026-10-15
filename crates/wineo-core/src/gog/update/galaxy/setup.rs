//! Setup in the prefix: what an installer does besides placing files. For
//! each installed product, base game first, GOG's script interpreter (ISI)
//! when the build sets `scriptInterpreter`, else the product's own setup
//! program from its support folder; then each listed redistributable from
//! the store, `.msi` packages through msiexec.
//!
//! Each step runs from a batch file in the run directory, through the
//! environment's runner as `cmd /c`, the way 0050's installers run: under
//! the environment's operation lock, supervised, with a log. The batch file
//! keeps GOG's arguments out of Wine's command-line quoting and writes the
//! program's full exit code to a file, since the host only sees its low 8
//! bits. That file alone decides. Its paths and arguments come from
//! variables set on that run, so the file itself is plain ASCII.
//!
//! A step is recorded in the record's `setup` as soon as it succeeds, so a
//! failed or interrupted setup carries on from the first step not done. A
//! crash between a step and its record runs it again, which installers
//! allow.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};

use super::install::{GalaxyError, SCRIPT_INTERPRETER};
use super::record::{self, Manifest, Record, SetupStep, StoreVersion};
use super::store;
use crate::cancel::Token;
use crate::config::GlobalConfig;
use crate::gog::folder::{FolderLock, ProductStatus, Target, create_dir_durable, failpoint};
use crate::gog::language;
use crate::gog::update::UpdateError;
use crate::gog::update::windows::{Event, z_path};
use crate::installer::{self, Installer, Outcome, Output};
use crate::operation::Guard;
use crate::paths::Paths;

/// What a redistributable ends with when it installed but asks for a
/// restart, which Wine has no use for.
const RESTART: [i64; 2] = [3010, 1641];
/// What a redistributable ends with when a newer version is installed.
const NEWER: i64 = 1638;

/// One setup step, resolved to what runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Step {
    /// What the record keeps once it succeeds.
    pub(super) identity: SetupStep,
    /// How messages name it.
    pub(super) label: String,
    /// The program, or the package msiexec installs.
    pub(super) program: PathBuf,
    /// As the program gets them, after its own path.
    pub(super) arguments: String,
    /// Where it runs from.
    pub(super) dir: PathBuf,
    /// A redistributable, whose restart and newer-version codes are success.
    pub(super) redist: bool,
}

impl Step {
    fn msi(&self) -> bool {
        self.program
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("msi"))
    }
}

/// Where a Galaxy install's setup reads from.
pub(super) struct Sources<'a> {
    pub(super) paths: &'a Paths,
    /// The game folder.
    pub(super) folder: &'a Path,
    /// The folder's support folder, holding one folder per product.
    pub(super) support: &'a Path,
    /// With every file's MD5 known.
    pub(super) manifest: &'a Manifest,
    pub(super) target: &'a Target,
    /// How messages name a product.
    pub(super) label: &'a dyn Fn(u64) -> String,
}

/// The steps setup takes, in order, and warnings for a product's setup
/// program that isn't among its support files, which is left out.
pub(super) fn steps(sources: &Sources) -> Result<(Vec<Step>, Vec<String>)> {
    let Sources {
        paths,
        folder,
        support,
        manifest,
        target,
        label,
    } = *sources;
    let setup = &manifest.setup;
    let mut steps = Vec::new();
    let mut warnings = Vec::new();
    let interpreter = setup
        .redist
        .iter()
        .find(|redist| redist.dependency == SCRIPT_INTERPRETER)
        .and_then(|redist| {
            let exe = redist.executable.as_deref()?;
            Some((redist, exe))
        });
    // A build that sets `scriptInterpreter` when GOG doesn't offer ISI was
    // warned about while planning, and its products get no script step.
    let products = target.products.iter().filter(|product| {
        product.status == ProductStatus::Installed
            && setup
                .products
                .iter()
                .any(|known| known.product == product.id)
    });
    for product in products {
        let id = product.id;
        let dir = support.join(id.to_string());
        let code = product.language.as_deref().unwrap_or(&target.language);
        let arguments = galaxy_arguments(
            folder,
            &dir,
            id,
            &manifest.build,
            setup.version_name.as_deref(),
            code,
        )?;
        if setup.script_interpreter {
            let Some((redist, exe)) = interpreter else {
                continue;
            };
            let version = StoreVersion {
                dependency: redist.dependency.clone(),
                manifest: redist.manifest.clone(),
            };
            let script = format!("goggame-{id}.script");
            let script = manifest
                .files
                .iter()
                .find(|file| file.path.eq_ignore_ascii_case(&script))
                .map(|file| {
                    file.md5
                        .clone()
                        .ok_or_else(|| anyhow!("{}'s MD5 isn't known", file.path))
                })
                .transpose()?;
            steps.push(Step {
                identity: SetupStep::ScriptInterpreter {
                    product: id,
                    build: manifest.build.clone(),
                    script,
                    interpreter: redist.manifest.clone(),
                },
                label: format!("GOG's setup script for {}", label(id)),
                program: in_folder(&store::version_dir(paths, &version), exe),
                arguments,
                dir,
                redist: false,
            });
            continue;
        }
        let Some(program) = setup
            .products
            .iter()
            .find(|known| known.product == id)
            .and_then(|known| known.temp_executable.as_deref())
            .filter(|path| !path.is_empty())
        else {
            continue;
        };
        let temp_arguments = setup
            .products
            .iter()
            .find(|known| known.product == id)
            .and_then(|known| known.temp_arguments.as_deref())
            .map(str::trim)
            .unwrap_or_default();
        let Some(file) = manifest
            .support
            .iter()
            .find(|file| file.product == Some(id) && file.path.eq_ignore_ascii_case(program))
        else {
            warnings.push(format!(
                "{}'s setup program {program} isn't among its files, so it doesn't run.",
                label(id)
            ));
            continue;
        };
        let md5 = file
            .md5
            .clone()
            .ok_or_else(|| anyhow!("{}'s MD5 isn't known", file.path))?;
        let arguments = if temp_arguments.is_empty() {
            arguments
        } else {
            format!("{arguments} {temp_arguments}")
        };
        steps.push(Step {
            identity: SetupStep::Program {
                product: id,
                path: file.path.clone(),
                arguments: arguments.clone(),
                md5,
            },
            label: format!("{}'s setup program", label(id)),
            program: dir.join(&file.path),
            arguments,
            dir,
            redist: false,
        });
    }
    for redist in &setup.redist {
        if redist.dependency == SCRIPT_INTERPRETER {
            continue;
        }
        let Some(exe) = redist.executable.as_deref().filter(|exe| !exe.is_empty()) else {
            continue;
        };
        let version = StoreVersion {
            dependency: redist.dependency.clone(),
            manifest: redist.manifest.clone(),
        };
        let program = in_folder(&store::version_dir(paths, &version), exe);
        let dir = program
            .parent()
            .expect("a program is in a folder")
            .to_owned();
        steps.push(Step {
            identity: SetupStep::Redistributable {
                dependency: redist.dependency.clone(),
                manifest: redist.manifest.clone(),
            },
            label: redist.dependency.clone(),
            program,
            arguments: redist.arguments.trim().to_owned(),
            dir,
            redist: true,
        });
    }
    Ok((steps, warnings))
}

/// `relative` under `root`, each part spelled as it is on disk when only its
/// case differs, since GOG's paths mix cases freely.
fn in_folder(root: &Path, relative: &str) -> PathBuf {
    let mut path = root.to_owned();
    for part in relative.split('/').filter(|part| !part.is_empty()) {
        let exact = path.join(part);
        if exact.exists() {
            path = exact;
            continue;
        }
        let found = fs::read_dir(&path).ok().and_then(|entries| {
            entries.flatten().find_map(|entry| {
                let name = entry.file_name();
                name.to_str()
                    .is_some_and(|name| name.eq_ignore_ascii_case(part))
                    .then(|| entry.path())
            })
        });
        path = found.unwrap_or(exact);
    }
    path
}

/// The arguments Heroic gives GOG's script interpreter and a product's
/// setup program, which were checked to set both up silently. The doubled
/// shortcut flag, one of them misspelled, is how Heroic passes it.
///
/// cmd parses the line these end up on, so every value wineo puts in is
/// either quoted or held to characters cmd gives no meaning to.
fn galaxy_arguments(
    folder: &Path,
    support: &Path,
    product: u64,
    build: &str,
    version_name: Option<&str>,
    code: &str,
) -> Result<String> {
    if !plain(code, 35) {
        bail!("GOG's language code {code:?} isn't one setup can be given");
    }
    if !plain(build, 64) {
        bail!("GOG's build id {build:?} isn't one setup can be given");
    }
    let name = language::english_name(code);
    // A quote would end the quoted value early, so it's left out.
    let version: String = version_name.unwrap_or_default().replace('"', "");
    if version.contains(['\r', '\n', '\0']) {
        bail!("GOG's version name {version:?} isn't one setup can be given");
    }
    Ok(format!(
        "/VERYSILENT /DIR={} /Language={name} /LANG={name} /ProductId={product} \
         /galaxyclient /buildId={build} /versionName=\"{version}\" \
         /nodesktopshorctut /nodesktopshortcut /lang-code={code} /supportDir={}",
        quoted_path(folder)?,
        quoted_path(support)?,
    ))
}

/// Whether `text` is 1 to `max` ASCII letters, digits, `-`, `_`, and `.`,
/// none of which mean anything to cmd.
fn plain(text: &str, max: usize) -> bool {
    (1..=max).contains(&text.len())
        && text
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

fn quoted_path(path: &Path) -> Result<String> {
    Ok(format!("\"{}\"", setup_path(path)?))
}

/// `path` as a Windows program in the prefix is given it. A quote would end
/// the quoted value it goes in early, and a backslash in a Linux name would
/// read as a separator, pointing setup at another folder, so both are
/// refused.
fn setup_path(path: &Path) -> Result<String> {
    let text = z_path(path)?;
    if path.as_os_str().as_encoded_bytes().contains(&b'\\') || text.contains(['"', '\r', '\n']) {
        bail!(
            "{} has a character in it that setup can't be given",
            path.display()
        );
    }
    Ok(text)
}

/// The folder a step runs from.
const DIR: &str = "WINEO_SETUP_DIR";
/// The program, for a step that isn't an `.msi` package.
const PROGRAM: &str = "WINEO_SETUP_PROGRAM";
/// The package msiexec installs.
const MSI: &str = "WINEO_SETUP_MSI";
/// Everything after the program or package on its command line.
const ARGS: &str = "WINEO_SETUP_ARGS";
/// The file the full exit code goes to.
const CODE: &str = "WINEO_SETUP_CODE";

/// The batch file a step runs from, with CRLF line endings: it changes to
/// the step's folder, runs the program with the rest of the line as its
/// arguments, and writes `%ERRORLEVEL%`, the full exit code, to a file.
///
/// It is pure ASCII and the same for every step: paths and arguments come
/// from variables set on the runner, since cmd reads a batch file in the
/// console's code page and can't find a path with `™` written in it. cmd
/// expands each variable once, so a `%` in a value stays as it is.
pub(super) fn batch_file(msi: bool) -> &'static str {
    if msi {
        // Named in full, since cmd looks in the current folder, a
        // redistributable's own, before its search path.
        "@echo off\r\n\
         cd /d \"%WINEO_SETUP_DIR%\"\r\n\
         \"%SystemRoot%\\system32\\msiexec.exe\" /i \"%WINEO_SETUP_MSI%\" %WINEO_SETUP_ARGS%\r\n\
         >\"%WINEO_SETUP_CODE%\" echo %ERRORLEVEL%\r\n"
    } else {
        "@echo off\r\n\
         cd /d \"%WINEO_SETUP_DIR%\"\r\n\
         \"%WINEO_SETUP_PROGRAM%\" %WINEO_SETUP_ARGS%\r\n\
         >\"%WINEO_SETUP_CODE%\" echo %ERRORLEVEL%\r\n"
    }
}

/// The variables `batch_file` reads for `step`, writing its code to `code`.
pub(super) fn variables(step: &Step, code: &Path) -> Result<BTreeMap<String, String>> {
    // A line break would end the batch file's line, and a NUL can't be in a
    // variable at all.
    if step.arguments.contains(['\r', '\n', '\0']) {
        bail!("{}'s arguments can't go on one command line", step.label);
    }
    let program = if step.msi() { MSI } else { PROGRAM };
    Ok(BTreeMap::from([
        (DIR.to_owned(), setup_path(&step.dir)?),
        (program.to_owned(), setup_path(&step.program)?),
        (ARGS.to_owned(), step.arguments.clone()),
        (CODE.to_owned(), setup_path(code)?),
    ]))
}

/// The batch file's path as `cmd /c` is given it. It's on cmd's own command
/// line, where cmd would expand a `%` and act on `&` and its kind, and which
/// it reads in a code page that may not have every character, so anything
/// but plain ASCII is refused.
fn batch_path(bat: &Path) -> Result<String> {
    let text = z_path(bat)?;
    let fine = !bat.as_os_str().as_encoded_bytes().contains(&b'\\')
        && text.bytes().all(|byte| {
            (byte.is_ascii_graphic() || byte == b' ')
                && !matches!(byte, b'%' | b'"' | b'&' | b'|' | b'<' | b'>' | b'^' | b'!')
        });
    if !fine {
        bail!(
            "Setup can't run from {}: cmd can't be given a path with characters other than plain letters, digits, and punctuation like - _ . /.",
            bat.parent().unwrap_or(bat).display()
        );
    }
    Ok(text)
}

/// The indexes of the steps among `wanted` still to run: every one with
/// `rerun_all`, else each that no step in `done` matches exactly, so a step
/// whose identity changed and a new step run and the rest are skipped.
pub(super) fn remaining(wanted: &[SetupStep], done: &[SetupStep], rerun_all: bool) -> Vec<usize> {
    wanted
        .iter()
        .enumerate()
        .filter(|(_, step)| rerun_all || !done.contains(step))
        .map(|(index, _)| index)
        .collect()
}

/// How a step's program ended, from its code file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Ended {
    Success,
    /// A redistributable installed and asked for a restart.
    Restart,
    Failed(Option<i64>),
}

impl Ended {
    pub(super) fn of(code: Option<i64>, redist: bool) -> Self {
        match code {
            Some(0) => Self::Success,
            Some(code) if redist && RESTART.contains(&code) => Self::Restart,
            Some(NEWER) if redist => Self::Success,
            other => Self::Failed(other),
        }
    }
}

/// What setup runs with: the folder's lock held, its journal written, and
/// the record's `pending` set for the operation.
pub(super) struct Setup<'a> {
    pub(super) paths: &'a Paths,
    pub(super) name: &'a str,
    pub(super) title: &'a str,
    /// The prefix, resolved, that the guard holds.
    pub(super) prefix: &'a Path,
    pub(super) lock: &'a FolderLock,
    pub(super) token: &'a Token,
}

/// What setup came to.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct Ran {
    /// Redistributables that asked for a restart, by id.
    pub(super) needs_restart: Vec<String>,
}

/// Runs each of `steps` that `record`'s `setup` doesn't show done (every one
/// with `rerun_all`), in order, recording each as it succeeds. `guard` is
/// the environment's operation lock holding the prefix, handed back once
/// done. The redistributables' store versions are held by the caller.
///
/// A step that fails or is canceled leaves the journal for the caller, so
/// trying again carries on from it.
pub(super) fn run(
    setup: &Setup,
    mut guard: Guard,
    record: &mut Record,
    steps: &[Step],
    rerun_all: bool,
    progress: &mut dyn FnMut(usize, usize),
    events: &mut dyn FnMut(Event),
) -> Result<(Guard, Ran)> {
    let Setup {
        paths,
        name,
        title,
        prefix,
        lock,
        token,
    } = *setup;
    let wanted: Vec<SetupStep> = steps.iter().map(|step| step.identity.clone()).collect();
    let todo = remaining(&wanted, &record.setup, rerun_all);
    let mut ran = Ran::default();
    if todo.is_empty() {
        return Ok((guard, ran));
    }
    let global = GlobalConfig::load(paths)?;
    let stop = Duration::from_secs(global.stop_timeout.into());
    let runs = paths.runtime_dir(name);
    fs::create_dir_all(&runs).with_context(|| format!("failed to create {}", runs.display()))?;
    for (done, &index) in todo.iter().enumerate() {
        progress(done, todo.len());
        #[cfg(test)]
        super::tests::before_setup_step(paths);
        if token.is_canceled() {
            return Err(UpdateError::Canceled { journal: true }.into());
        }
        let step = &steps[index];
        if !step.redist {
            // wineo's own folder, which a product with no support files
            // doesn't have yet; `cd /d` into a missing one would quietly
            // leave the program somewhere else.
            create_dir_durable(&step.dir)?;
        }
        let stem = format!("gog-galaxy-setup-{:02}", index + 1);
        let bat = runs.join(format!("{stem}.bat"));
        let code = runs.join(format!("{stem}.code"));
        let log = runs.join(format!("{stem}.log"));
        // A code file an earlier attempt left must not speak for this one.
        crate::files::remove_if_present(&code)
            .with_context(|| format!("failed to remove {}", code.display()))?;
        let args = installer::Args {
            args: vec!["/c".to_owned(), batch_path(&bat)?],
            // Set on this run alone, so no later launch sees them.
            env: variables(step, &code)?,
        };
        fs::write(&bat, batch_file(step.msi()))
            .with_context(|| format!("failed to write {}", bat.display()))?;
        // Looked up by Wine in the prefix, never as a file of that name in
        // the current folder.
        let installer = Installer::prepare_prefix_program_with(
            guard, paths, &global, name, "cmd", &args, prefix,
        )?;
        let (outcome, back) = installer.run_keeping(token, stop, Output::Log(&log), |watched| {
            events(Event::Watched(watched))
        });
        guard = back;
        let failed = |detail: String| GalaxyError::Setup {
            title: title.to_owned(),
            label: step.label.clone(),
            detail,
        };
        match outcome? {
            Outcome::Canceled { .. } => return Err(UpdateError::Canceled { journal: true }.into()),
            Outcome::Unresolved { error, .. } => {
                return Err(failed(format!("{error}; stop it with wineo stop {name}")).into());
            }
            // Only the code file says how it ended.
            Outcome::Finished { .. } => {}
        }
        match Ended::of(read_code(&code), step.redist) {
            Ended::Success => {}
            Ended::Restart => {
                if let SetupStep::Redistributable { dependency, .. } = &step.identity
                    && !ran.needs_restart.contains(dependency)
                {
                    ran.needs_restart.push(dependency.clone());
                }
            }
            Ended::Failed(code) => {
                let detail = match code {
                    Some(9009) => "exit code 9009: its program wasn't found".to_owned(),
                    Some(code) => format!("exit code {code}"),
                    None => "no exit code".to_owned(),
                };
                return Err(failed(detail).into());
            }
        }
        failpoint("galaxy.setup.ran")?;
        record
            .setup
            .retain(|recorded| !recorded.same_step(&step.identity));
        record.setup.push(step.identity.clone());
        record::write(lock, record)?;
    }
    progress(todo.len(), todo.len());
    Ok((guard, ran))
}

/// The full exit code a batch file wrote. None when it wrote none, or
/// something that isn't one.
fn read_code(path: &Path) -> Option<i64> {
    fs::read_to_string(path).ok()?.trim().parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step(program: &str, arguments: &str, dir: &str, redist: bool) -> Step {
        Step {
            identity: SetupStep::Redistributable {
                dependency: "MSVC2005".to_owned(),
                manifest: "a".repeat(32),
            },
            label: "MSVC2005".to_owned(),
            program: PathBuf::from(program),
            arguments: arguments.to_owned(),
            dir: PathBuf::from(dir),
            redist,
        }
    }

    #[test]
    fn the_batch_file_is_ascii_and_reads_everything_from_variables() {
        for msi in [false, true] {
            let text = batch_file(msi);
            assert!(text.is_ascii());
            assert!(text.starts_with("@echo off\r\ncd /d \"%WINEO_SETUP_DIR%\"\r\n"));
            assert!(text.ends_with(">\"%WINEO_SETUP_CODE%\" echo %ERRORLEVEL%\r\n"));
            assert_eq!(text.matches("\r\n").count(), 4);
            assert!(!text.replace("\r\n", "").contains('\n'));
        }
        assert!(batch_file(false).contains("\r\n\"%WINEO_SETUP_PROGRAM%\" %WINEO_SETUP_ARGS%\r\n"));
        assert!(batch_file(true).contains(
            "\r\n\"%SystemRoot%\\system32\\msiexec.exe\" /i \"%WINEO_SETUP_MSI%\" %WINEO_SETUP_ARGS%\r\n"
        ));
    }

    #[test]
    fn variables_hold_paths_and_arguments_as_they_are() {
        let nested = r#"/q:a /c:"msiexec /i vcredist.msi /qb! /l*v %temp%\vc.log""#;
        let vars = variables(
            &step(
                "/games/SimCity™ 4 Délüxe & 100%/vcredist_x86.exe",
                nested,
                "/games/SimCity™ 4 Délüxe & 100%",
                true,
            ),
            Path::new("/state/game/gog-galaxy-setup-03.code"),
        )
        .unwrap();
        assert_eq!(
            vars,
            BTreeMap::from([
                (
                    "WINEO_SETUP_DIR".to_owned(),
                    "Z:\\games\\SimCity™ 4 Délüxe & 100%".to_owned()
                ),
                (
                    "WINEO_SETUP_PROGRAM".to_owned(),
                    "Z:\\games\\SimCity™ 4 Délüxe & 100%\\vcredist_x86.exe".to_owned()
                ),
                ("WINEO_SETUP_ARGS".to_owned(), nested.to_owned()),
                (
                    "WINEO_SETUP_CODE".to_owned(),
                    "Z:\\state\\game\\gog-galaxy-setup-03.code".to_owned()
                ),
            ])
        );
        let msi = variables(&step("/d/XNA.MSI", "/quiet", "/d", true), Path::new("/c")).unwrap();
        assert_eq!(msi["WINEO_SETUP_MSI"], "Z:\\d\\XNA.MSI");
        assert!(!msi.contains_key("WINEO_SETUP_PROGRAM"));
    }

    #[test]
    fn variables_refuse_what_would_break_the_line() {
        for (program, arguments) in [
            ("/a/b.exe", "/x\r\n/y"),
            ("/a/b.exe", "/x\0"),
            ("/a/q\"uote.exe", ""),
            ("/a/new\nline.exe", ""),
            // Read as a separator on the Windows side.
            ("/a/back\\slash.exe", ""),
        ] {
            assert!(
                variables(&step(program, arguments, "/a", true), Path::new("/c")).is_err(),
                "{program} {arguments}"
            );
        }
        for dir in ["/games/Game\\", "/games/a\\b"] {
            assert!(
                variables(&step("/a/b.exe", "", dir, true), Path::new("/c")).is_err(),
                "{dir}"
            );
        }
        assert!(quoted_path(Path::new("/games/Game\\")).is_err());
    }

    #[test]
    fn the_batch_files_own_path_is_plain_ascii() {
        assert_eq!(
            batch_path(Path::new(
                "/home/me/.local/state/wineo/my game/gog-galaxy-setup-01.bat"
            ))
            .unwrap(),
            "Z:\\home\\me\\.local\\state\\wineo\\my game\\gog-galaxy-setup-01.bat"
        );
        for bad in [
            "/home/Zoë/x.bat",
            "/home/me/100%/x.bat",
            "/home/a&b/x.bat",
            "/home/a\"b/x.bat",
            "/home/a^b/x.bat",
            "/home/a|b/x.bat",
            "/home/a\\b/x.bat",
        ] {
            let error = batch_path(Path::new(bad)).unwrap_err();
            assert!(
                error.to_string().starts_with("Setup can't run from"),
                "{bad}: {error}"
            );
        }
    }

    #[test]
    fn values_wineo_composes_cant_carry_commands() {
        let args = |build: &str, version: Option<&str>, code: &str| {
            galaxy_arguments(Path::new("/g"), Path::new("/s"), 1, build, version, code)
        };
        for code in [
            "en-US&echo injected>injected.txt&rem",
            "en US",
            "en|x",
            "en^",
            "%PATH%",
            "en\"",
            "",
            &"a".repeat(36),
        ] {
            assert!(args("100", None, code).is_err(), "{code:?}");
        }
        for build in ["1&calc", "1 2", "1%x%", "", "1\"x"] {
            assert!(args(build, None, "en-US").is_err(), "{build:?}");
        }
        // A version name stays inside its quotes, its own quotes gone, so
        // `&` and the like are only text there.
        let quoted = args("100", Some("1.0\" & echo injected & rem \""), "en-US").unwrap();
        assert!(
            quoted.contains("/versionName=\"1.0 & echo injected & rem \" "),
            "{quoted}"
        );
        assert!(args("100", Some("1.0\r\n"), "en-US").is_err());
        // Codes GOG's depots use are fine.
        for code in ["en-US", "zh-Hans", "es_mx", "pt-BR"] {
            assert!(
                args("59781086834418085", Some("v1.2"), code).is_ok(),
                "{code}"
            );
        }
    }

    #[test]
    fn exit_codes_read_the_same_for_every_step_but_redistributables() {
        for redist in [false, true] {
            assert_eq!(Ended::of(Some(0), redist), Ended::Success);
            for code in [1, 194, 255, 256, 70000, -1, -1073741819] {
                assert_eq!(Ended::of(Some(code), redist), Ended::Failed(Some(code)));
            }
            assert_eq!(Ended::of(None, redist), Ended::Failed(None));
        }
        assert_eq!(Ended::of(Some(3010), true), Ended::Restart);
        assert_eq!(Ended::of(Some(1641), true), Ended::Restart);
        assert_eq!(Ended::of(Some(1638), true), Ended::Success);
        for code in [3010, 1641, 1638] {
            assert_eq!(Ended::of(Some(code), false), Ended::Failed(Some(code)));
        }
    }

    #[test]
    fn galaxy_arguments_are_heroics() {
        let arguments = galaxy_arguments(
            Path::new("/games/The Witcher"),
            Path::new("/data/gog/support/k/1207658924"),
            1207658924,
            "56789",
            Some("1.5 \"final\""),
            "fr-FR",
        )
        .unwrap();
        assert_eq!(
            arguments,
            "/VERYSILENT /DIR=\"Z:\\games\\The Witcher\" /Language=French /LANG=French \
             /ProductId=1207658924 /galaxyclient /buildId=56789 /versionName=\"1.5 final\" \
             /nodesktopshorctut /nodesktopshortcut /lang-code=fr-FR \
             /supportDir=\"Z:\\data\\gog\\support\\k\\1207658924\""
        );
        let none = galaxy_arguments(Path::new("/g"), Path::new("/s"), 1, "2", None, "xx").unwrap();
        assert!(none.contains("/versionName=\"\""), "{none}");
        assert!(none.contains("/Language=English /LANG=English"), "{none}");
    }

    #[test]
    fn only_steps_that_changed_or_are_new_run_again() {
        let isi = |build: &str, script: &str, interpreter: &str| SetupStep::ScriptInterpreter {
            product: 1,
            build: build.to_owned(),
            script: Some(script.repeat(32)),
            interpreter: interpreter.repeat(32),
        };
        let redist = |dependency: &str, manifest: &str| SetupStep::Redistributable {
            dependency: dependency.to_owned(),
            manifest: manifest.repeat(32),
        };
        let done = vec![
            isi("100", "a", "b"),
            redist("MSVC2017", "c"),
            redist("DirectX", "d"),
        ];
        assert!(remaining(&done, &done, false).is_empty());
        assert_eq!(remaining(&done, &done, true), [0, 1, 2]);
        // A new version of one dependency runs only that one.
        let wanted = vec![
            isi("100", "a", "b"),
            redist("MSVC2017", "e"),
            redist("DirectX", "d"),
        ];
        assert_eq!(remaining(&wanted, &done, false), [1]);
        // A changed script, or a new interpreter, runs only ISI.
        for changed in [isi("100", "f", "b"), isi("100", "a", "f")] {
            let wanted = vec![changed, redist("MSVC2017", "c"), redist("DirectX", "d")];
            assert_eq!(remaining(&wanted, &done, false), [0]);
        }
        // A new dependency runs, and one no longer listed is simply left.
        let wanted = vec![isi("100", "a", "b"), redist("PhysX", "f")];
        assert_eq!(remaining(&wanted, &done, false), [1]);
        assert!(!done[1].same_step(&wanted[0]));
        assert!(redist("MSVC2017", "c").same_step(&redist("MSVC2017", "e")));
    }
}
