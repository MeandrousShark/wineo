//! Notes from prefix creation and launch preparation.

use std::path::Path;

use wineo_core::launch::LaunchEvent;
use wineo_core::paths::Paths;
use wineo_core::runner::Runner;

use super::{components, desktop, winetricks};

pub fn creating_note(paths: &Paths, name: &str, runner: &Runner, dir: &Path) -> String {
    let dir = paths
        .contract(dir)
        .unwrap_or_else(|_| dir.display().to_string());
    if runner.is_proton() {
        format!("Creating a Proton prefix for {name} in {dir}, this takes a while")
    } else {
        format!("Creating a wine prefix for {name} in {dir}, this takes a few seconds")
    }
}

pub fn print_event(paths: &Paths, name: &str, event: LaunchEvent) {
    match event {
        LaunchEvent::BuildingPrefix { dir, runner } => {
            eprintln!("{}", creating_note(paths, name, &runner, &dir));
        }
        LaunchEvent::Components(changes) => {
            for change in &changes {
                eprintln!("{}", components::describe(change, name));
            }
        }
        LaunchEvent::Desktop(changes) => {
            for change in &changes {
                eprintln!("{}", desktop::describe(change, name));
            }
        }
        LaunchEvent::Winetricks(event) => winetricks::print_event(event),
        LaunchEvent::NoWaylandSession => {
            eprintln!("Not a Wayland session, starting {name} on X11.");
        }
        LaunchEvent::Starting | LaunchEvent::Started => {}
    }
}
