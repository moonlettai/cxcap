//! CXCAP — See complexity before it compounds.
//!
//! Fast, local, read-only software complexity analysis CLI for humans and
//! coding agents.

use clap::{Parser, Subcommand};
use rayon::prelude::*;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use cxcap::dispatch;
use cxcap::relate;
use cxcap::report;
use cxcap::scan::{self, Kind};
use cxcap::update;

const VERSION: &str = env!("CARGO_PKG_VERSION");
const DEFAULT_MAX_BYTES: u64 = 2_000_000;

#[derive(Parser)]
#[command(name = "cxcap", about = "CXCAP — See complexity before it compounds.\nAnalyze software complexity and change exposure locally.")]
struct Cli {
    #[command(subcommand)]
    cmd: Option<Commands>,
    /// Print version
    #[arg(long)]
    version: bool,
}

#[derive(Subcommand)]
enum Commands {
    /// Audit a project directory (read-only)
    Audit {
        /// Project directory to audit
        path: PathBuf,
        /// Hotspots to show
        #[arg(long, default_value_t = 12)]
        top: usize,
        /// Path filter for proposed change area
        #[arg(long)]
        focus: Option<String>,
        /// Natural-language change intent (e.g. "add session expiry")
        #[arg(long)]
        intent: Option<String>,
        /// Machine-readable JSON output
        #[arg(long)]
        json: bool,
        /// Skip files larger than this (bytes)
        #[arg(long, default_value_t = DEFAULT_MAX_BYTES)]
        max_bytes: u64,
        /// Parallel workers for per-file analysis (default: all cores; 1 forces serial)
        #[arg(long)]
        jobs: Option<usize>,
    },
    /// Update cxcap to the newest release (checksum-verified, staged,
    /// smoke-tested, atomic where possible; failed updates keep the
    /// working binary)
    Update {
        /// Only report availability; do not install anything
        #[arg(long)]
        check: bool,
    },
}

fn available_cores() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
}

#[allow(clippy::too_many_arguments)]
fn audit(
    root: &Path,
    max_bytes: u64,
    jobs: Option<usize>,
    top_n: usize,
    focus: Option<String>,
    intent: Option<String>,
) -> report::Report {
    let t0 = Instant::now();
    let mut entries: Vec<(String, PathBuf, u64)> = Vec::new();
    let mut symlinks = 0u64;
    scan::iter_files(root, &mut entries, &mut symlinks);
    // Canonical order: deterministic across filesystems, worker counts, runs.
    entries.sort();
    let mut tops = dispatch::local_toplevels(root);
    // Multi-distribution src-layout monorepos: nested `<...>/src/<pkg>`
    // packages (same economics as the root-level rule, `__init__.py`
    // witnessed, entries-derived so skip-aware).
    tops.extend(dispatch::nested_src_toplevels(&entries));
    let ws_packages = dispatch::find_workspace_packages(&entries);
    let t_walk = Instant::now();
    let n_jobs = jobs.unwrap_or_else(available_cores).max(1);
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(n_jobs)
        .build()
        .expect("thread pool");
    let mut results: Vec<scan::ScanResult> = pool.install(|| {
        entries
            .par_iter()
            .map(|e| scan::scan_one(e, max_bytes, &tops))
            .collect()
    });
    let t_read = Instant::now();
    let mut bytes_total = 0u64;
    let mut code_loc = 0u64;
    let mut doc_files = 0;
    let mut doc_loc = 0u64;
    let mut config_files = 0;
    let mut config_loc = 0u64;
    let mut skipped_oversize = 0;
    let mut skipped_unreadable = 0;
    let mut clone_input: HashMap<String, (Vec<String>, usize)> = HashMap::new();
    let mut ext_counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    // Single census pass: counters and clone-detection input both derive
    // per result, so one loop serves both (order-independent accumulation).
    for r in &results {
        bytes_total += r.bytes;
        *ext_counts.entry(r.ext_counted.clone()).or_insert(0) += 1;
        match r.kind {
            Kind::Python | Kind::Js | Kind::Rs | Kind::Test => {
                code_loc += r.delta as u64;
            }
            Kind::Doc => {
                doc_files += 1;
                doc_loc += r.rec.loc as u64;
            }
            Kind::Config => {
                config_files += 1;
                config_loc += r.rec.loc as u64;
            }
            Kind::Other => {}
        }
        if r.oversize {
            skipped_oversize += 1;
        }
        if r.unreadable {
            skipped_unreadable += 1;
        }
        if let Some((w, k)) = &r.clone {
            clone_input.insert(r.rec.path.clone(), (w.clone(), *k));
        }
    }
    // Relationships: fan-in/coupling per file, import cycles, clone lines.
    // (The reporting increment serializes these; the census Report below is
    // intentionally unchanged.)
    let code: Vec<scan::FileRec> = results
        .iter()
        .filter(|r| matches!(r.kind, Kind::Python | Kind::Js | Kind::Rs | Kind::Test))
        .map(|r| r.rec.clone())
        .collect();
    let coupling = relate::attribute_coupling(&code, &tops, &ws_packages, root);
    for r in results.iter_mut() {
        if !matches!(r.kind, Kind::Python | Kind::Js | Kind::Rs | Kind::Test) {
            continue;
        }
        let fan_in = coupling.fan_in.get(&r.rec.path).copied().unwrap_or(0);
        r.rec.fan_in = Some(fan_in);
        // Workspace-package imports are internal by definition even when the
        // entry file stays ambiguous: internal fan-out on the importer.
        let wsn = coupling.fan_out_ws.get(&r.rec.path).copied().unwrap_or(0);
        if wsn > 0 && r.rec.kind == "js" {
            r.rec.fan_out_in += wsn;
            r.rec.fan_out_ex = r.rec.fan_out_ex.saturating_sub(wsn);
        }
        r.rec.coupling = Some(r.rec.fan_out_in + fan_in);
    }
    let eager_edges: Vec<(String, String)> = coupling
        .edges
        .iter()
        .filter(|(_, _, d)| !d)
        .map(|(a, b, _)| (a.clone(), b.clone()))
        .collect();
    let all_edges: Vec<(String, String)> = coupling
        .edges
        .iter()
        .map(|(a, b, _)| (a.clone(), b.clone()))
        .collect();
    let eager = relate::find_cycles(&eager_edges);
    let eager_sets: Vec<std::collections::HashSet<String>> = eager
        .iter()
        .map(|m| m.iter().cloned().collect())
        .collect();
    let mut _cycles: Vec<(Vec<String>, bool)> =
        eager.into_iter().map(|c| (c, true)).collect();
    for c in relate::find_cycles(&all_edges) {
        let s: std::collections::HashSet<String> = c.iter().cloned().collect();
        if !eager_sets.contains(&s) {
            _cycles.push((c, false));
        }
    }
    let clones = relate::find_clones(&clone_input);
    for r in results.iter_mut() {
        if matches!(r.rec.kind, "python" | "js") {
            r.rec.dup_lines = Some(clones.dup_lines.get(&r.rec.path).copied().unwrap_or(0));
        }
    }
    let clone_pairs = clones.pairs;
    let t_rel = Instant::now();
    // Full report assembly (reporting phase).
    let files: Vec<scan::FileRec> = results.into_iter().map(|r| r.rec).collect();
    let mut skip_dirs: Vec<String> = scan::SKIP_DIRS.iter().map(|s| s.to_string()).collect();
    skip_dirs.sort();
    let r3 = |a: Instant, b: Instant| (b.duration_since(a).as_secs_f64() * 1000.0).round() / 1000.0;
    let t_rep = Instant::now();
    let mut rep = report::assemble(report::AssembleArgs {
        version: VERSION,
        root: root.to_string_lossy().into_owned(),
        elapsed_s: (t0.elapsed().as_secs_f64() * 100.0).round() / 100.0,
        discovery_s: r3(t0, t_walk),
        read_parse_s: r3(t_walk, t_read),
        relationships_s: r3(t_read, t_rel),
        reporting_s: 0.0,
        files,
        bytes_total,
        code_loc,
        doc_files,
        doc_loc,
        cfg_files: config_files,
        cfg_loc: config_loc,
        skipped_oversize,
        skipped_unreadable,
        symlinks,
        ext_counts,
        edges: coupling.edges,
        cycles: _cycles,
        clone_pairs,
        top_n,
        focus: cxcap::focus::normalize_focus(focus),
        intent,
        skip_dirs_sorted: skip_dirs,
    });
    rep.phases.reporting_s = (t_rep.elapsed().as_secs_f64() * 1000.0).round() / 1000.0;
    rep.elapsed_s = (t0.elapsed().as_secs_f64() * 100.0).round() / 100.0;
    rep
}

fn main() {
    let cli = Cli::parse();
    if cli.version {
        println!("{VERSION}");
        return;
    }
    match cli.cmd {
        Some(Commands::Update { check }) => {
            if check {
                match update::check_only() {
                    Ok(msg) => println!("{msg}"),
                    Err(e) => {
                        eprintln!("cxcap update: {e}");
                        std::process::exit(1);
                    }
                }
                return;
            }
            match update::run_update() {
                Ok(msg) => println!("{msg}"),
                Err(e) => {
                    eprintln!("cxcap update: {e}");
                    std::process::exit(1);
                }
            }
        }
        Some(Commands::Audit {
            path,
            top,
            focus,
            intent,
            json,
            max_bytes,
            jobs,
        }) => {
            if !path.is_dir() {
                eprintln!("cxcap: not a directory: {}", path.display());
                std::process::exit(1);
            }
            // Daily release check first (stderr only; never touches `--json`
            // stdout and never writes to the analyzed repository).
            update::maybe_check();
            let mut rep = audit(&path, max_bytes, jobs, top, focus, intent);
            if json {
                // Drop heavy per-func details by default; keep hotspots only.
                for h in rep
                    .hotspots
                    .iter_mut()
                    .chain(rep.test_hotspots.iter_mut())
                {
                    h.imports = None;
                    h.type_only_imports = None;
                }
                if let Some(f) = rep.focus.as_mut() {
                    for t in f.top.iter_mut() {
                        t.imports = None;
                        t.type_only_imports = None;
                    }
                }
                println!("{}", serde_json::to_string_pretty(&rep).unwrap());
            } else {
                print!("{}", report::render_text(&rep, top));
            }
        }
        None => {
            eprintln!("usage: cxcap audit <path> [--top N] [--focus PATH] [--intent TEXT] [--json] [--max-bytes N] [--jobs N]");
            std::process::exit(2);
        }
    }
}
