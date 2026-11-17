//! Building the target a game runs as: the emulator's `exe` and `args` for
//! one ROM, written out in full at import. Nothing here goes through a shell,
//! so a path with spaces or quotes stays one argument.

use std::path::{Component, Path, PathBuf};

use super::{Emulator, Install};
use crate::error::{Error, Result};

/// A target's `exe` and `args`, as a native `Target` holds them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetCommand {
    pub exe: String,
    pub args: Vec<String>,
}

fn invalid(message: String) -> Error {
    Error::Emulator(message)
}

fn utf8(path: &Path) -> Result<&str> {
    path.to_str()
        .ok_or_else(|| invalid(format!("Path `{}` isn't valid UTF-8.", path.display())))
}

fn absolute_rom(rom: &Path) -> Result<&str> {
    if !rom.is_absolute() {
        return Err(invalid(format!(
            "ROM path `{}` isn't absolute.",
            rom.display()
        )));
    }
    // Flatpak refuses a `--filesystem` path with `..` in it.
    if rom.components().any(|part| part == Component::ParentDir) {
        return Err(invalid(format!(
            "ROM path `{}` can't contain `..`.",
            rom.display()
        )));
    }
    utf8(rom)
}

/// The arguments after the emulator's name. `fullscreen` is ignored where
/// there's no flag for it. The ROM is absolute, so it never reads as an
/// option, and `--` is used only where the emulator's parser takes it.
fn emulator_args(emulator: Emulator, rom: &str, fullscreen: bool) -> Result<Vec<String>> {
    let flag = |flag: &'static str| fullscreen.then_some(flag);
    let words: Vec<&str> = match emulator {
        Emulator::Dolphin => vec!["-b", "-e", rom],
        Emulator::Cemu => flag("-f").into_iter().chain(["-g", rom]).collect(),
        Emulator::Pcsx2 | Emulator::DuckStation => ["-batch"]
            .into_iter()
            .chain(flag("-fullscreen"))
            .chain(["--", rom])
            .collect(),
        Emulator::Ppsspp => ["--pause-menu-exit"]
            .into_iter()
            .chain(flag("--fullscreen"))
            .chain([rom])
            .collect(),
        Emulator::Rpcs3 => ["--no-gui"]
            .into_iter()
            .chain(flag("--fullscreen"))
            .chain([rom])
            .collect(),
        // Azahar needs the ROM last.
        Emulator::MelonDs | Emulator::Mgba | Emulator::Azahar => {
            flag("-f").into_iter().chain([rom]).collect()
        }
        Emulator::Xemu => flag("-full-screen")
            .into_iter()
            .chain(["-dvd_path", rom])
            .collect(),
        Emulator::RetroArch => {
            return Err(invalid("RetroArch games aren't supported yet.".to_owned()));
        }
        Emulator::Other => {
            return Err(invalid(
                "A custom emulator has no built-in command line.".to_owned(),
            ));
        }
    };
    Ok(words.into_iter().map(str::to_owned).collect())
}

/// Flatpak reads `--filesystem` up to the first unescaped `:`, where the
/// access mode starts.
fn escape_filesystem(dir: &str) -> String {
    dir.replace('\\', "\\\\").replace(':', "\\:")
}

/// The folders a sandboxed emulator needs for `rom`, escaped for
/// `--filesystem`: the ROM's folder (the ROM itself for a folder game), and
/// the real one when a link leads somewhere else.
fn filesystem_dirs(rom: &Path) -> Result<Vec<String>> {
    let folder_of = |path: &Path| {
        if path.is_dir() {
            Some(path.to_path_buf())
        } else {
            path.parent().map(Path::to_path_buf)
        }
    };
    let mut dirs: Vec<PathBuf> = folder_of(rom).into_iter().collect();
    if let Ok(real) = rom.canonicalize()
        && let Some(dir) = folder_of(&real)
        && !dirs.contains(&dir)
    {
        dirs.push(dir);
    }
    // Flatpak refuses to share the whole filesystem this way.
    if dirs.iter().any(|dir| dir.parent().is_none()) {
        return Err(invalid(
            "A Flatpak emulator can't read a game directly in /.".to_owned(),
        ));
    }
    dirs.iter()
        .map(|dir| utf8(dir).map(escape_filesystem))
        .collect()
}

/// The target that runs `rom` in `emulator` as `install` finds it.
/// `fullscreen` is ignored for an emulator with no such flag.
pub fn build_target(
    emulator: Emulator,
    install: &Install,
    rom: &Path,
    fullscreen: bool,
) -> Result<TargetCommand> {
    let rom_str = absolute_rom(rom)?;
    let emulator_args = emulator_args(emulator, rom_str, fullscreen)?;
    match install {
        Install::Path { name, .. } => Ok(TargetCommand {
            exe: (*name).to_owned(),
            args: emulator_args,
        }),
        Install::File(path) => Ok(TargetCommand {
            exe: utf8(path)?.to_owned(),
            args: emulator_args,
        }),
        Install::Flatpak { .. } => {
            let app = emulator
                .flatpak_id()
                .ok_or_else(|| invalid(format!("{} has no Flatpak.", emulator.name())))?;
            let mut args = vec!["run".to_owned()];
            for dir in filesystem_dirs(rom)? {
                args.push(format!("--filesystem={dir}"));
            }
            args.push(app.to_owned());
            args.extend(emulator_args);
            Ok(TargetCommand {
                exe: "flatpak".to_owned(),
                args,
            })
        }
    }
}

/// The target for a custom emulator: `exe` (a path or a bare PATH name) and
/// `args`, split like a shell would, with `{rom}` replaced by the ROM, whole
/// or inside an argument.
pub fn custom_target(exe: &str, args: &str, rom: &Path) -> Result<TargetCommand> {
    if exe.trim().is_empty() {
        return Err(invalid("The emulator's command is empty.".to_owned()));
    }
    let rom = absolute_rom(rom)?;
    let words = shlex::split(args).ok_or_else(|| {
        invalid("The arguments have an unclosed quote or a trailing backslash.".to_owned())
    })?;
    // shlex reads an unquoted `#` as the start of a comment and drops the rest.
    let hashes = |text: &str| text.matches('#').count();
    if words.iter().map(|word| hashes(word)).sum::<usize>() < hashes(args) {
        return Err(invalid(
            "Quote a # that starts an argument, like '#2'.".to_owned(),
        ));
    }
    if !words.iter().any(|word| word.contains(PLACEHOLDER)) {
        return Err(invalid(format!(
            "The arguments need {PLACEHOLDER} for the game."
        )));
    }
    Ok(TargetCommand {
        exe: exe.to_owned(),
        args: words
            .iter()
            .map(|word| word.replace(PLACEHOLDER, rom))
            .collect(),
    })
}

const PLACEHOLDER: &str = "{rom}";
