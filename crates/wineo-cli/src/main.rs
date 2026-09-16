use clap::Parser;

#[derive(Parser)]
#[command(name = "wineo", version, about = "Manage Wine environments")]
struct Cli {}

fn main() {
    Cli::parse();
}
