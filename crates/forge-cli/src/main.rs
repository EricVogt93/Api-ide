mod print;

use std::path::{Path, PathBuf};

use clap::{Args, Parser, Subcommand};

use forge_core::exec::HttpEngine;
use forge_core::runner::{junit_xml, run, CancellationToken, DataSource, RunOptions, RunScope};
use forge_core::store::{TreeNode, Workspace, COLLECTIONS_DIR};

use print::{print_summary, run_printer, supports_color};

#[derive(Parser)]
#[command(name = "forge", version, about = "Headless runner for Forge API test workspaces")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run a request, folder, collection or the whole workspace.
    Run(RunArgs),
    /// List every collection, folder and request in a workspace.
    List(WorkspaceArgs),
    /// List environments and their variable counts.
    Envs(WorkspaceArgs),
}

#[derive(Args)]
struct WorkspaceArgs {
    /// Path to the workspace root (containing forge.json).
    workspace: PathBuf,
}

#[derive(Args)]
struct RunArgs {
    /// Path to the workspace root (containing forge.json).
    workspace: PathBuf,
    /// Workspace-relative scope: a `*.request.json` file, `collections/<name>`,
    /// a deeper folder path, or omitted to run the whole workspace.
    #[arg(long)]
    scope: Option<String>,
    /// Environment name to resolve variables against.
    #[arg(long = "env")]
    env: Option<String>,
    /// Data-driven iterations file (.csv or .json).
    #[arg(long)]
    data: Option<PathBuf>,
    /// Write a JUnit XML report to this path.
    #[arg(long)]
    report: Option<PathBuf>,
    /// Stop at the first failing request.
    #[arg(long)]
    bail: bool,
    /// Fixed delay between requests in milliseconds.
    #[arg(long = "delay-ms", default_value_t = 0)]
    delay_ms: u64,
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let code = match cli.command {
        Command::Run(args) => cmd_run(args).await,
        Command::List(args) => cmd_list(&args.workspace),
        Command::Envs(args) => cmd_envs(&args.workspace),
    };
    std::process::exit(code);
}

async fn cmd_run(args: RunArgs) -> i32 {
    let workspace = match Workspace::load(&args.workspace) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };

    let scope = parse_scope(args.scope.as_deref());
    let data = match args.data.as_deref().map(parse_data_source) {
        Some(Ok(d)) => Some(d),
        Some(Err(e)) => {
            eprintln!("error: {e}");
            return 2;
        }
        None => None,
    };

    let options = RunOptions { environment: args.env.clone(), data, bail: args.bail, delay_ms: args.delay_ms };
    let engine = HttpEngine::new();
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let cancel = CancellationToken::new();
    let color = supports_color();

    let printer = tokio::spawn(run_printer(rx, color));
    let run_result = run(&workspace, scope, options, &engine, tx, cancel).await;
    let (outcomes, _printed_summary) = printer.await.unwrap_or_default();

    match run_result {
        Ok(summary) => {
            print_summary(&summary, color);
            if let Some(report_path) = &args.report {
                let junit = junit_xml(&workspace.meta.name, &outcomes, &summary);
                if let Err(e) = std::fs::write(report_path, junit) {
                    eprintln!("error: failed to write report to {}: {e}", report_path.display());
                    return 2;
                }
            }
            if summary.failed > 0 {
                1
            } else {
                0
            }
        }
        Err(e) => {
            eprintln!("error: {e}");
            2
        }
    }
}

fn cmd_list(workspace_path: &Path) -> i32 {
    let workspace = match Workspace::load(workspace_path) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };

    for col in &workspace.collections {
        println!("{}/", col.meta.name);
        print_children(&col.children, 1);
    }
    0
}

fn print_children(children: &[TreeNode], depth: usize) {
    let indent = "  ".repeat(depth);
    for child in children {
        match child {
            TreeNode::Folder(f) => {
                println!("{indent}{}/", child.display_name());
                print_children(&f.children, depth + 1);
            }
            TreeNode::Request(r) => {
                println!("{indent}[{:<7}] {}", r.def.method.as_str(), child.display_name());
            }
        }
    }
}

fn cmd_envs(workspace_path: &Path) -> i32 {
    let workspace = match Workspace::load(workspace_path) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };

    for env in &workspace.environments {
        let total = env.env.variables.len();
        let secret_count = env.env.variables.values().filter(|v| v.secret).count();
        println!("{} ({total} variable(s), {secret_count} secret)", env.env.name);
    }
    0
}

/// Interpret a `--scope` value per the CLI contract:
/// - a path ending in `.request.json` -> `RunScope::Request`
/// - exactly `collections/<name>` -> `RunScope::Collection`
/// - any deeper directory path -> `RunScope::Folder`
/// - omitted -> `RunScope::Workspace`
fn parse_scope(rel: Option<&str>) -> RunScope {
    let Some(rel) = rel else {
        return RunScope::Workspace;
    };
    let rel = rel.trim_matches('/');
    if rel.ends_with(".request.json") {
        return RunScope::Request(rel.to_string());
    }
    let parts: Vec<&str> = rel.split('/').collect();
    if parts.len() == 2 && parts[0] == COLLECTIONS_DIR {
        RunScope::Collection(rel.to_string())
    } else {
        RunScope::Folder(rel.to_string())
    }
}

fn parse_data_source(path: &Path) -> anyhow::Result<DataSource> {
    match path.extension().and_then(|e| e.to_str()) {
        Some(ext) if ext.eq_ignore_ascii_case("csv") => Ok(DataSource::CsvFile(path.to_path_buf())),
        Some(ext) if ext.eq_ignore_ascii_case("json") => Ok(DataSource::JsonFile(path.to_path_buf())),
        _ => Err(anyhow::anyhow!(
            "unsupported data file extension (expected .csv or .json): {}",
            path.display()
        )),
    }
}
