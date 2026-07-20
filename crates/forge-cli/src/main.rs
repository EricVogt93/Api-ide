mod print;

use std::path::{Path, PathBuf};

use clap::{Args, Parser, Subcommand};

use forge_core::exec::HttpEngine;
use forge_core::runner::{junit_xml, run, CancellationToken, DataSource, RunOptions, RunScope};
use forge_core::store::{TreeNode, Workspace, COLLECTIONS_DIR};

use print::{print_summary, run_printer, supports_color};

#[derive(Parser)]
#[command(
    name = "forge",
    version,
    about = "Headless runner for Forge API test workspaces"
)]
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
    /// gRPC: list methods of a .proto or call a unary method.
    #[command(subcommand)]
    Grpc(GrpcCommand),
    /// Request-format v1: validate a request document (no network).
    Validate(V1Args),
    /// Request-format v1: run a request document.
    RunV1(V1RunArgs),
    /// Request-format v1: list the project's asset store (usage, broken refs).
    Assets(AssetsArgs),
    /// Request-format v1: write .forge/lock.json (asset integrity hashes).
    Lock(LockArgs),
    /// Request-format v1: serve request documents' mocks over HTTP.
    Mock(MockArgs),
}

#[derive(Args)]
struct LockArgs {
    /// Project root (holds project.json).
    root: PathBuf,
    /// Verify against the existing lockfile instead of writing a new one.
    #[arg(long)]
    check: bool,
}

#[derive(Args)]
struct MockArgs {
    /// Project root (holds project.json).
    root: PathBuf,
    /// Port to listen on.
    #[arg(long, default_value_t = 8080)]
    port: u16,
    /// Environment name under environments/ (for `${env.*}` in URLs/mocks).
    #[arg(long = "env")]
    env: Option<String>,
}

#[derive(Args)]
struct AssetsArgs {
    /// Project root (holds project.json).
    root: PathBuf,
    /// Emit the full index as JSON instead of the table.
    #[arg(long)]
    json: bool,
}

#[derive(Args)]
struct V1Args {
    /// The `*.request.json` document.
    request: PathBuf,
    /// Project root (holds project.json); defaults to walking up from the request.
    #[arg(long)]
    root: Option<PathBuf>,
    /// Environment name under environments/.
    #[arg(long = "env")]
    env: Option<String>,
}

#[derive(Args)]
struct V1RunArgs {
    /// One or more `*.request.json` documents. Multiple files run as a
    /// sequence, threading extracted `${runtime.*}` forward.
    #[arg(required = true)]
    requests: Vec<PathBuf>,
    /// Project root (holds project.json); defaults to walking up from the first request.
    #[arg(long)]
    root: Option<PathBuf>,
    /// Environment name under environments/.
    #[arg(long = "env")]
    env: Option<String>,
    /// Serve each document's mock instead of sending over HTTP.
    #[arg(long)]
    mock: bool,
    /// Verify assets against .forge/lock.json before running; abort on drift.
    #[arg(long)]
    frozen: bool,
}

#[derive(Subcommand)]
enum GrpcCommand {
    /// List services and methods defined in .proto files.
    List(GrpcListArgs),
    /// Call a unary method with a JSON request message.
    Call(GrpcCallArgs),
}

#[derive(Args)]
struct GrpcListArgs {
    /// One or more .proto files.
    #[arg(required = true)]
    protos: Vec<PathBuf>,
    /// Import search path(s); defaults to each proto's directory.
    #[arg(long = "include", short = 'I')]
    includes: Vec<PathBuf>,
}

#[derive(Args)]
struct GrpcCallArgs {
    /// Endpoint, e.g. http://localhost:50051 or https://api.example.com
    #[arg(long)]
    endpoint: String,
    /// Full method path: package.Service/Method
    #[arg(long)]
    method: String,
    /// Request message as JSON (use @file.json to read from a file, - for stdin).
    #[arg(long, default_value = "{}")]
    data: String,
    /// Metadata entries as key:value (repeatable).
    #[arg(long = "meta", short = 'm')]
    metadata: Vec<String>,
    /// One or more .proto files.
    #[arg(required = true)]
    protos: Vec<PathBuf>,
    /// Import search path(s); defaults to each proto's directory.
    #[arg(long = "include", short = 'I')]
    includes: Vec<PathBuf>,
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
        Command::Grpc(GrpcCommand::List(args)) => cmd_grpc_list(&args),
        Command::Grpc(GrpcCommand::Call(args)) => cmd_grpc_call(args).await,
        Command::Validate(args) => cmd_validate(&args),
        Command::RunV1(args) => cmd_run_v1(args).await,
        Command::Assets(args) => cmd_assets(&args),
        Command::Lock(args) => cmd_lock(&args),
        Command::Mock(args) => cmd_mock(&args),
    };
    std::process::exit(code);
}

fn cmd_lock(args: &LockArgs) -> i32 {
    use forge_core::reqv1::Lockfile;

    if args.check {
        let lock = match Lockfile::read(&args.root) {
            Ok(l) => l,
            Err(d) => {
                eprintln!("error: {}", d.message);
                return 2;
            }
        };
        match lock.verify(&args.root) {
            Ok(diags) if diags.is_empty() => {
                println!("lockfile is clean ({} asset(s))", lock.assets.len());
                0
            }
            Ok(diags) => {
                for d in &diags {
                    eprintln!("  [{}] {}", d.code, d.message);
                }
                eprintln!("{} drift(s)", diags.len());
                1
            }
            Err(d) => {
                eprintln!("error: {}", d.message);
                2
            }
        }
    } else {
        match Lockfile::build(&args.root).and_then(|l| l.write(&args.root).map(|_| l)) {
            Ok(l) => {
                println!("wrote .forge/lock.json ({} asset(s))", l.assets.len());
                0
            }
            Err(d) => {
                eprintln!("error: {}", d.message);
                2
            }
        }
    }
}

fn cmd_mock(args: &MockArgs) -> i32 {
    use forge_core::reqv1::{self, MockServerConfig};

    let env = match reqv1::load_environment(&args.root, args.env.as_deref()) {
        Ok(e) => e,
        Err(diagnostic) => {
            eprintln!("error: {}", diagnostic.message);
            return 2;
        }
    };
    // A mock server serves canned responses; it must not demand production
    // secrets to resolve a document. Real secrets (if a dynamic mock needs
    // one) still come from .env.local/env; a missing one gets a placeholder
    // so routing never silently drops a route.
    let real = make_secret_provider(&args.root);
    let secret = move |name: &str| real(name).or_else(|| Some("<secret>".to_string()));
    let config = match MockServerConfig::scan(&args.root, env, &secret) {
        Ok(c) => c,
        Err(errors) => {
            eprint!("{errors}");
            return 2;
        }
    };
    let server = match tiny_http::Server::http(("0.0.0.0", args.port)) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: cannot bind port {}: {e}", args.port);
            return 2;
        }
    };
    println!(
        "mock server on http://0.0.0.0:{} — {} route(s):",
        args.port,
        config.route_count()
    );
    for (method, path, id) in config.routes() {
        println!("  {method} {path}  → {id}");
    }
    serve_mock(&config, &server, &secret);
    0
}

fn serve_mock(
    config: &forge_core::reqv1::MockServerConfig,
    server: &tiny_http::Server,
    secret: &(dyn Fn(&str) -> Option<String> + Sync),
) {
    for request in server.incoming_requests() {
        let method = request.method().as_str().to_string();
        let url = request.url().to_string();
        let path = url.split('?').next().unwrap_or(&url);

        let response = match config.handle(&method, path, secret) {
            Ok(Some(mock)) => {
                let mut response =
                    tiny_http::Response::from_data(mock.body).with_status_code(mock.status);
                for (name, value) in mock.headers {
                    if let Ok(header) =
                        tiny_http::Header::from_bytes(name.as_bytes(), value.as_bytes())
                    {
                        response = response.with_header(header);
                    }
                }
                request.respond(response)
            }
            Ok(None) => request
                .respond(tiny_http::Response::from_string("no mock route").with_status_code(404)),
            Err(errors) => {
                eprint!("{errors}");
                request.respond(
                    tiny_http::Response::from_string(errors.to_string()).with_status_code(500),
                )
            }
        };
        if let Err(error) = response {
            eprintln!("error: failed to send mock response: {error}");
        }
    }
}

fn cmd_assets(args: &AssetsArgs) -> i32 {
    use forge_core::reqv1::ProjectIndex;

    let index = match ProjectIndex::scan(&args.root) {
        Ok(i) => i,
        Err(d) => {
            eprintln!("error: {}", d.message);
            return 2;
        }
    };

    if args.json {
        let json = match serde_json::to_string_pretty(&index) {
            Ok(json) => json,
            Err(error) => {
                eprintln!("error: cannot serialize asset index: {error}");
                return 2;
            }
        };
        println!("{json}");
        return if index.broken.is_empty() { 0 } else { 1 };
    }

    let mut current_kind = None;
    for asset in &index.assets {
        if current_kind != Some(asset.kind) {
            println!("{}:", asset.kind.label());
            current_kind = Some(asset.kind);
        }
        let ref_form = asset
            .alias
            .clone()
            .or_else(|| asset.prefix_ref.clone())
            .unwrap_or_else(|| asset.rel_path.clone());
        println!(
            "  {ref_form}  ({}, used by {})",
            asset.rel_path,
            asset.used_by.len()
        );
    }
    if !index.requests.is_empty() {
        println!("requests:");
        for r in &index.requests {
            println!("  {}  ({}, {} ref(s))", r.id, r.rel_path, r.refs.len());
        }
    }
    if !index.environments.is_empty() {
        println!("environments: {}", index.environments.join(", "));
    }
    if !index.broken.is_empty() {
        eprintln!("broken refs:");
        for b in &index.broken {
            eprintln!(
                "  {} {} {:?}: {}",
                b.request, b.instance_path, b.reference, b.message
            );
        }
        return 1;
    }
    0
}

/// v1 secret provider (§14): a gitignored `<root>/.env.local` (KEY=value
/// lines) first, then the process environment. `${secret.API_TOKEN}` reads
/// either source; .env.local wins on collision (declared order, no implicit
/// precedence beyond it).
fn make_secret_provider(root: &Path) -> impl Fn(&str) -> Option<String> {
    let mut file_vars = std::collections::HashMap::new();
    if let Ok(text) = std::fs::read_to_string(root.join(".env.local")) {
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some((k, v)) = line.split_once('=') {
                file_vars.insert(k.trim().to_string(), v.trim().trim_matches('"').to_string());
            }
        }
    }
    move |name: &str| {
        file_vars
            .get(name)
            .cloned()
            .or_else(|| std::env::var(name).ok())
    }
}

fn v1_root(args: &V1Args) -> PathBuf {
    v1_root_for(&args.request, args.root.as_deref())
}

/// Project root: the explicit override, else the nearest ancestor of
/// `request` containing a project.json, else the request's directory.
fn v1_root_for(request: &Path, root_override: Option<&Path>) -> PathBuf {
    if let Some(root) = root_override {
        return root.to_path_buf();
    }
    let mut dir = request.parent().map(Path::to_path_buf);
    while let Some(d) = dir {
        if d.join("project.json").exists() {
            return d;
        }
        dir = d.parent().map(Path::to_path_buf);
    }
    request.parent().unwrap_or(Path::new(".")).to_path_buf()
}

fn cmd_validate(args: &V1Args) -> i32 {
    use forge_core::reqv1;
    let root = v1_root(args);
    let text = match std::fs::read_to_string(&args.request) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("error: cannot read {}: {e}", args.request.display());
            return 2;
        }
    };
    let doc = match reqv1::RequestDocument::parse(&text) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("error: invalid request document: {e}");
            return 2;
        }
    };
    let env = match reqv1::load_environment(&root, args.env.as_deref()) {
        Ok(e) => e,
        Err(d) => {
            eprintln!("error: {}", d.message);
            return 2;
        }
    };
    // validate is a structural, no-network check — it must not require the
    // actual secrets to be present. A placeholder provider proves the
    // reference is well-formed without needing the value.
    let validate_secret = |_name: &str| Some("<secret>".to_string());
    match reqv1::validate(&doc, &root, &args.request, env, &validate_secret) {
        Ok(ir) => {
            println!("ok: {} ({})", ir.id, ir.name);
            println!("  {} {}", ir.method, ir.url);
            println!(
                "  {} pipeline step(s), {} header(s)",
                ir.pipeline.len(),
                ir.headers.len()
            );
            0
        }
        Err(diags) => {
            for d in &diags {
                let loc = d.instance_path.as_deref().unwrap_or("");
                eprintln!("  [{}] {} {}", d.code, loc, d.message);
            }
            eprintln!("{} diagnostic(s)", diags.len());
            1
        }
    }
}

async fn cmd_run_v1(args: V1RunArgs) -> i32 {
    use forge_core::exec::HttpEngine;
    use forge_core::reqv1::{self, RunMode, RunResult, RunStatus};
    use forge_core::runner::CancellationToken;

    let first = &args.requests[0];
    let root = v1_root_for(first, args.root.as_deref());

    if args.frozen {
        match reqv1::Lockfile::read(&root).and_then(|l| l.verify(&root)) {
            Ok(diags) if diags.is_empty() => {}
            Ok(diags) => {
                for d in &diags {
                    eprintln!("  [{}] {}", d.code, d.message);
                }
                eprintln!("error: --frozen and the project drifted from .forge/lock.json");
                return 2;
            }
            Err(d) => {
                eprintln!("error: --frozen: {}", d.message);
                return 2;
            }
        }
    }

    let env = match reqv1::load_environment(&root, args.env.as_deref()) {
        Ok(e) => e,
        Err(d) => {
            eprintln!("error: {}", d.message);
            return 2;
        }
    };
    let engine = HttpEngine::new();
    let mode = if args.mock {
        RunMode::Mock
    } else {
        RunMode::Http
    };
    let secret = make_secret_provider(&root);

    // Multiple files run as a sequence (runtime threaded forward); a single
    // file goes through run_matrix so matrix documents expand.
    let results: Vec<RunResult> = if args.requests.len() > 1 {
        reqv1::run_sequence(
            &args.requests,
            &root,
            env,
            &secret,
            &engine,
            mode,
            CancellationToken::new(),
        )
        .await
    } else {
        let text = match std::fs::read_to_string(first) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("error: cannot read {}: {e}", first.display());
                return 2;
            }
        };
        let doc = match reqv1::RequestDocument::parse(&text) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("error: invalid request document: {e}");
                return 2;
            }
        };
        match reqv1::run_matrix(
            &doc,
            &root,
            first,
            env,
            &secret,
            &engine,
            mode,
            CancellationToken::new(),
        )
        .await
        {
            Ok(cases) => cases.into_iter().map(|(_, r)| r).collect(),
            Err(errs) => {
                for d in &errs.0 {
                    let loc = d.instance_path.as_deref().unwrap_or("");
                    eprintln!("  [{}] {} {}", d.code, loc, d.message);
                }
                return 2;
            }
        }
    };

    let multi = results.len() > 1;
    let mut worst = RunStatus::Passed;
    for result in &results {
        if multi {
            println!("--- {}", result.request_id);
        }
        if let Some(http) = &result.http {
            println!(
                "{} — {} ({} ms, {} bytes)",
                result.request_id, http.status, http.time_ms, http.bytes
            );
        }
        for a in &result.assertions {
            println!("  {} {}", if a.passed { "✓" } else { "✗" }, a.message);
        }
        for (k, v) in &result.runtime {
            println!("  → {k} = {v}");
        }
        for d in &result.diagnostics {
            eprintln!("  [{}] {}", d.code, d.message);
        }
        println!("{:?}", result.status);
        worst = match (worst, result.status) {
            (_, RunStatus::Error) | (RunStatus::Error, _) => RunStatus::Error,
            (_, RunStatus::Failed) | (RunStatus::Failed, _) => RunStatus::Failed,
            _ => RunStatus::Passed,
        };
    }
    match worst {
        RunStatus::Passed => 0,
        RunStatus::Failed => 1,
        RunStatus::Error => 2,
    }
}

fn cmd_grpc_list(args: &GrpcListArgs) -> i32 {
    let pool = match forge_core::protocols::compile_protos(&args.protos, &args.includes) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    for m in forge_core::protocols::list_methods(&pool) {
        let shape = if m.is_unary { "unary" } else { "streaming" };
        println!(
            "{}  {} -> {}  [{}]",
            m.path, m.input_type, m.output_type, shape
        );
    }
    0
}

async fn cmd_grpc_call(args: GrpcCallArgs) -> i32 {
    let pool = match forge_core::protocols::compile_protos(&args.protos, &args.includes) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };

    let data = if args.data == "-" {
        use std::io::Read as _;
        let mut buf = String::new();
        if std::io::stdin().read_to_string(&mut buf).is_err() {
            eprintln!("error: failed to read request JSON from stdin");
            return 2;
        }
        buf
    } else if let Some(path) = args.data.strip_prefix('@') {
        match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("error: failed to read {path}: {e}");
                return 2;
            }
        }
    } else {
        args.data.clone()
    };

    let mut metadata = Vec::new();
    for entry in &args.metadata {
        match entry.split_once(':') {
            Some((k, v)) => metadata.push((k.trim().to_string(), v.trim().to_string())),
            None => {
                eprintln!("error: metadata must be key:value, got {entry:?}");
                return 2;
            }
        }
    }

    match forge_core::protocols::call(&args.endpoint, &pool, &args.method, &data, &metadata).await {
        Ok(response) => {
            for message in &response.messages {
                println!("{message}");
            }
            for (k, v) in &response.metadata {
                eprintln!("# {k}: {v}");
            }
            0
        }
        Err(e) => {
            eprintln!("error: {e}");
            1
        }
    }
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

    let options = RunOptions {
        environment: args.env.clone(),
        data,
        bail: args.bail,
        delay_ms: args.delay_ms,
    };
    let engine = HttpEngine::new();
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let cancel = CancellationToken::new();
    let color = supports_color();

    let printer = tokio::spawn(run_printer(rx, color));
    let run_result = run(&workspace, scope, options, &engine, tx, cancel).await;
    let (outcomes, _printed_summary) = match printer.await {
        Ok(output) => output,
        Err(error) => {
            eprintln!("error: run output task failed: {error}");
            return 2;
        }
    };

    match run_result {
        Ok(summary) => {
            print_summary(&summary, color);
            if let Some(report_path) = &args.report {
                let junit = junit_xml(&workspace.meta.name, &outcomes, &summary);
                if let Err(e) = std::fs::write(report_path, junit) {
                    eprintln!(
                        "error: failed to write report to {}: {e}",
                        report_path.display()
                    );
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
                println!(
                    "{indent}[{:<7}] {}",
                    r.def.method.as_str(),
                    child.display_name()
                );
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
        println!(
            "{} ({total} variable(s), {secret_count} secret)",
            env.env.name
        );
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
        Some(ext) if ext.eq_ignore_ascii_case("json") => {
            Ok(DataSource::JsonFile(path.to_path_buf()))
        }
        _ => Err(anyhow::anyhow!(
            "unsupported data file extension (expected .csv or .json): {}",
            path.display()
        )),
    }
}
