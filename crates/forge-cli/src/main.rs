use clap::Parser;

#[derive(Parser)]
#[command(name = "forge", version, about = "Headless runner for Forge API test workspaces")]
struct Cli {}

fn main() -> anyhow::Result<()> {
    let _cli = Cli::parse();
    println!("forge {} (core format v{})", env!("CARGO_PKG_VERSION"), forge_core::FORMAT_VERSION);
    Ok(())
}
