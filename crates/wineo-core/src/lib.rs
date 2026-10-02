pub mod art;
pub mod art_fetch;
pub mod art_icon;
pub mod build_lock;
pub mod build_removal;
pub mod cancel;
pub mod clone;
pub mod components;
pub mod config;
pub mod create;
pub mod desktop;
pub mod download;
pub mod edit;
pub mod environment;
pub mod error;
mod files;
pub mod gamescope;
pub mod heroic;
pub(crate) mod heroic_args;
pub mod heroic_import;
pub mod import;
pub mod install;
pub mod installer;
pub mod launch;
pub mod log;
pub mod name;
pub mod operation;
pub mod paths;
pub mod portable;
pub mod prefix;
pub mod process;
pub mod programs;
pub mod report;
pub mod runner;
pub mod runner_alias;
pub mod runners;
pub mod runtime;
pub mod secret_service;
pub mod settings;
pub mod shortcut;
pub mod snapshot;
pub mod stats;
pub mod steamgriddb;
pub mod supervisor;
pub mod target_path;
pub mod targets;
pub mod templates;
pub mod time;
pub mod trash;
pub mod validate;
pub mod view;
pub mod winetricks;
pub mod winpath;

pub use error::{Error, Result};

#[cfg(test)]
mod source_tests {
    use std::fs;
    use std::path::Path;

    #[test]
    fn core_output_is_confined_to_supervisor() {
        let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let print = ["print", "ln!("].concat();
        let eprint = ["eprint", "ln!("].concat();
        for entry in fs::read_dir(source).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_none_or(|extension| extension != "rs")
                || path.file_name().is_some_and(|name| name == "supervisor.rs")
            {
                continue;
            }
            let contents = fs::read_to_string(&path).unwrap();
            assert!(
                !contents.contains(&print),
                "{} prints to stdout",
                path.display()
            );
            assert!(
                !contents.contains(&eprint),
                "{} prints to stderr",
                path.display()
            );
        }
    }
}
