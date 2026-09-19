use std::process::ExitCode;

use wineo_core::paths::Paths;
use wineo_core::templates::Templates;

#[derive(clap::Args)]
pub struct Args {}

pub fn run(paths: &Paths, _args: Args) -> anyhow::Result<ExitCode> {
    let templates = Templates::new(paths);
    let rows = templates.list()?;

    let names = rows.iter().map(|row| row.name.len()).max().unwrap_or(0);
    for row in &rows {
        let detail = if row.built_in { "built-in" } else { "" };
        println!("{}", format!("{:names$}  {detail}", row.name).trim_end());
    }
    eprintln!("Add your own in {}", templates.dir().display());
    Ok(ExitCode::SUCCESS)
}
