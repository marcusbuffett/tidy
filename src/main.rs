mod analysis;
mod config;
mod deps;
mod extract;
mod graph;
mod issues;
mod jsvalue;
mod patterns;
mod project;
mod report;
mod resolve;
mod scripts;
mod workspace;

use anyhow::{Context, Result};
use clap::Parser;
use issues::IssueKind;
use std::path::PathBuf;
use std::time::Instant;

/// Find unused files, exports, types, enum members and dependencies in JavaScript/TypeScript projects.
#[derive(Parser, Debug)]
#[command(name = "tidy", version, about)]
struct Cli {
    /// Project root (defaults to the current directory).
    dir: Option<PathBuf>,
    /// Config file (defaults to tidy.json / knip.json / package.json "knip").
    #[arg(short, long)]
    config: Option<PathBuf>,
    /// Reporter: text, json (knip-compatible), flat (one JSON object per issue).
    #[arg(short, long, default_value = "text")]
    reporter: String,
    /// Only report these issue types (comma-separated; e.g. files,exports).
    #[arg(long, value_delimiter = ',')]
    include: Vec<String>,
    /// Do not report these issue types.
    #[arg(long, value_delimiter = ',')]
    exclude: Vec<String>,
    /// Production mode: skip test/dev entry files and devDependencies.
    #[arg(long)]
    production: bool,
    /// Only report issues for this workspace (root-relative directory).
    #[arg(short = 'W', long)]
    workspace: Option<String>,
    /// Print timing information.
    #[arg(long)]
    perf: bool,
    /// Always exit with code 0.
    #[arg(long)]
    no_exit_code: bool,
    /// Number of threads (defaults to all cores).
    #[arg(long)]
    threads: Option<usize>,
    /// Explain how a file is classified, resolved and used (root-relative path).
    #[arg(long)]
    explain: Option<String>,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    if let Some(t) = cli.threads {
        rayon::ThreadPoolBuilder::new().num_threads(t).build_global().ok();
    }
    let t_start = Instant::now();
    let root = cli.dir.clone().unwrap_or_else(|| std::env::current_dir().unwrap());
    let root = root.canonicalize().with_context(|| format!("{} is not a directory", root.display()))?;
    if !root.join("package.json").is_file() {
        anyhow::bail!("no package.json in {}", root.display());
    }

    let (cfg, cfg_path) = config::load(&root, cli.config.as_deref())?;
    let workspaces = workspace::discover(&root)?;
    let t_setup = Instant::now();
    let project = project::build(&root, &cfg, &workspaces, cli.production)?;
    let setup_ms = t_setup.elapsed().as_secs_f64() * 1000.0;

    let mut resolvers = resolve::Resolvers::new(&root);
    for w in &workspaces {
        if let Some(name) = w.pkg_name() {
            resolvers.workspace_packages.insert(
                name.to_string(),
                resolve::WorkspacePackage { dir: w.dir.clone(), exports: w.manifest.raw.get("exports").cloned(), entry_refs: w.manifest.entry_refs.clone() },
            );
        }
    }
    let ws_dirs: Vec<PathBuf> = workspaces.iter().map(|w| w.dir.clone()).collect();
    let (graph, timings) = graph::build(&project, &resolvers, &ws_dirs);

    if let Some(target) = &cli.explain {
        explain(&root, &project, &graph, &workspaces, target);
        return Ok(());
    }

    let t_issues = Instant::now();
    let opts = analysis::Options { production: cli.production, tags: &cfg.tags, class_members: cli.include.iter().any(|s| s == "classMembers") };
    let mut issues = analysis::analyze(&project, &graph, &project.setups, &opts);
    issues.extend(deps::analyze(&project, &graph, &workspaces, &project.setups));
    let issues_ms = t_issues.elapsed().as_secs_f64() * 1000.0;

    // Filter issue types: CLI include/exclude override config include/exclude override defaults.
    let include: Vec<IssueKind> = if !cli.include.is_empty() { cli.include.iter().filter_map(|s| IssueKind::parse(s)).collect() } else { cfg.include.iter().filter_map(|s| IssueKind::parse(s)).collect() };
    let rules_off = cfg.rules_off();
    let exclude: Vec<IssueKind> = cli.exclude.iter().chain(cfg.exclude.iter()).chain(rules_off.iter()).filter_map(|s| IssueKind::parse(s)).collect();
    issues.retain(|i| {
        let on = if !include.is_empty() { include.contains(&i.kind) } else { i.kind.default_on() };
        on && !exclude.contains(&i.kind)
    });
    if cli.production {
        issues.retain(|i| i.kind != IssueKind::DevDependencies);
    }
    if let Some(w) = &cli.workspace {
        let w = w.trim_end_matches('/');
        issues.retain(|i| i.file.starts_with(&format!("{w}/")) || (w == "." && !i.file.contains('/')));
    }
    issues.sort_by(|a, b| (a.kind, &a.file, a.line, a.col, &a.name).cmp(&(b.kind, &b.file, b.line, b.col, &b.name)));

    let out = match cli.reporter.as_str() {
        "json" => report::json(&issues),
        "flat" => report::json_flat(&issues),
        _ => report::text(&issues),
    };
    print!("{out}");
    if cli.reporter == "json" || cli.reporter == "flat" {
        println!();
    }
    if cli.perf {
        let total = t_start.elapsed().as_secs_f64() * 1000.0;
        eprintln!(
            "tidy: {} workspaces, {} files ({} parsed, {} extra), config {}",
            workspaces.len(),
            project.files.len(),
            timings.parsed_files,
            timings.extra_files,
            cfg_path.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| "none".into())
        );
        eprintln!(
            "  walk+classify {setup_ms:.1}ms  parse {:.1}ms  resolve {:.1}ms  graph {:.1}ms  issues {issues_ms:.1}ms  total {total:.1}ms",
            timings.parse_ms, timings.resolve_ms, timings.analyze_ms
        );
    }
    if !issues.is_empty() && !cli.no_exit_code {
        std::process::exit(1);
    }
    Ok(())
}

fn explain(root: &std::path::Path, project: &project::Project, graph: &graph::Graph, workspaces: &[workspace::Workspace], target: &str) {
    let path = root.join(target.trim_start_matches("./"));
    let Some(&mi) = graph.by_path.get(&path) else {
        if let Some(f) = project.files.iter().find(|f| f.rel == target) {
            println!("{}: in project list but not in graph (ws={} project={} entry={} config={} ignored={})", target, f.ws, f.is_project, f.is_entry, f.is_config, f.is_ignored);
        } else {
            println!("{target}: not a project/entry/config file (not walked, gitignored, or no pattern matched)");
            let ws = workspaces.iter().enumerate().rev().find(|(_, w)| path.starts_with(&w.dir)).map(|(i, _)| i).unwrap_or(0);
            let s = &project.setups[ws];
            let ws_rel = path.strip_prefix(&workspaces[ws].dir).unwrap_or(&path).to_string_lossy().replace('\\', "/");
            println!("  workspace #{ws} ({}) ws_rel={ws_rel}", workspaces[ws].rel);
            println!("  project={} entry={} dev_entry={} config={} ignore={} exact_entry={}", s.project.is_match(&ws_rel), s.entry.is_match(&ws_rel), s.dev_entry.is_match(&ws_rel), s.config.is_match(&ws_rel), s.ignore.is_match(&ws_rel), s.exact_entries.contains(&ws_rel));
        }
        return;
    };
    let m = &graph.modules[mi];
    println!("{} (module #{mi})", m.rel);
    println!("  workspace #{} ({})  project={} entry={} dev_entry={} config={} ignored={} reachable={} cjs={} parse_error={}", m.ws, workspaces[m.ws].rel, m.is_project, m.is_entry, m.dev_entry, m.is_config, m.is_ignored, m.reachable, m.facts.is_cjs, m.facts.parse_error);
    println!("  enabled tools: {}", project.setups[m.ws].enabled_tools.iter().map(|&t| patterns::TOOLS[t].name).collect::<Vec<_>>().join(", "));
    println!("  imports:");
    for (ii, imp) in m.facts.imports.iter().enumerate() {
        let targets: Vec<String> = m.edges.iter().filter(|(i, _)| *i == ii).map(|(_, r)| format!("{:?} pkg={:?}", r.target, r.package)).collect();
        println!("    {:?} {} {:?} -> {}", imp.kind, imp.specifier, imp.names, targets.join(" | "));
    }
    println!("  exports:");
    for (i, e) in m.facts.exports.iter().enumerate() {
        println!("    {} {:?} members={:?} used_in_file={} reexport={:?} tags={:?} referenced_in={:?} -> used={:?}", e.name, e.kind, e.members, e.used_in_file, e.reexport, e.tags, e.referenced_in, m.export_used[i]);
    }
    for ea in &m.facts.export_alls {
        println!("    export * from {}", m.facts.imports[ea.import].specifier);
    }
    println!("  importers:");
    for other in &graph.modules {
        for (ii, r) in &other.edges {
            if let resolve::Target::Internal(p) = &r.target {
                if p == &m.path {
                    println!("    {} ({:?} {})", other.rel, other.facts.imports[*ii].kind, other.facts.imports[*ii].specifier);
                }
            }
        }
    }
    if m.is_config {
        println!("  strings: {}", m.facts.strings.join(", "));
    }
}
