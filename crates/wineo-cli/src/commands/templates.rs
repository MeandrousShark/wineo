use std::process::ExitCode;

use wineo_core::paths::Paths;
use wineo_core::templates::Templates;

use crate::text;

#[derive(clap::Args)]
pub struct Args {}

pub fn run(paths: &Paths, _args: Args) -> anyhow::Result<ExitCode> {
    let templates = Templates::new(paths);
    let rows = templates.list()?;

    let names = text::widest(rows.iter().map(|row| row.name.as_str()));
    for row in &rows {
        let detail = if row.built_in { "built-in" } else { "" };
        println!(
            "{}",
            format!("{}  {detail}", text::pad(&row.name, names)).trim_end()
        );
    }
    eprintln!("Add your own in {}", templates.dir().display());
    Ok(ExitCode::SUCCESS)
}
