//! Reporting: hotspot scoring, warnings, verdicts, focus analysis,
//! guidance, and text/JSON rendering.
//!
//! Every string, threshold, weight, and rounding rule mirrors the reference
//! implementation exactly. Float formatting uses Python-compatible
//! round-half-even to 1 decimal (Rust's {:.1} rounds half away from zero).

use crate::fmt::{FolderRow, folder_of, n1, py_fmt1, py_round1};
use crate::scan::FileRec;
use crate::verdict::Warning;
use serde::Serialize;
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Hotspots, folders, distributions
// ---------------------------------------------------------------------------

/// Hotspot 0..100 normalized over production code so test volume cannot
/// drown the files humans actually change. Mutates hotspot fields in place;
/// returns (hot, test_hot) as indices into `files`.
pub fn score_hotspots(files: &mut [FileRec], top_n: usize) -> (Vec<usize>, Vec<usize>) {
    let prod: Vec<usize> = files
        .iter()
        .enumerate()
        .filter(|(_, f)| crate::scan::is_prod_kind(f.kind))
        .map(|(i, _)| i)
        .collect();
    let test: Vec<usize> = files
        .iter()
        .enumerate()
        .filter(|(_, f)| f.kind == "test")
        .map(|(i, _)| i)
        .collect();
    if !prod.is_empty() {
        let mx_loc = prod.iter().map(|&i| files[i].loc).max().unwrap_or(1).max(1);
        let mx_cx = prod.iter().map(|&i| files[i].complexity).max().unwrap_or(1).max(1);
        let mx_cp = prod
            .iter()
            .map(|&i| files[i].coupling.unwrap_or(0))
            .max()
            .unwrap_or(1)
            .max(1);
        let mx_fn = prod.iter().map(|&i| files[i].funcs as u32).max().unwrap_or(1).max(1);
        for &i in &prod {
            let f = &files[i];
            files[i].hotspot = Some(py_round1(
                100.0 * (0.35 * (f.loc as f64 / mx_loc as f64)
                    + 0.35 * (f.complexity as f64 / mx_cx as f64)
                    + 0.20 * (f.coupling.unwrap_or(0) as f64 / mx_cp as f64)
                    + 0.10 * (f.funcs as f64 / mx_fn as f64)),
            ));
        }
    }
    for &i in &test {
        files[i].hotspot = Some(0.0);
    }
    let mut hot = prod;
    hot.sort_by(|&a, &b| {
        files[b]
            .hotspot
            .unwrap_or(0.0)
            .partial_cmp(&files[a].hotspot.unwrap_or(0.0))
            .unwrap()
    });
    hot.truncate(top_n);
    let mut test_hot = test;
    test_hot.sort_by(|&a, &b| files[b].complexity.cmp(&files[a].complexity));
    test_hot.truncate(3);
    (hot, test_hot)
}


/// Folder aggregation (top 8 by complexity at parent-dir granularity).
pub fn aggregate(idxs: &[usize], files: &[FileRec], denom: u32) -> Vec<FolderRow> {
    let mut c_cx: HashMap<String, u32> = HashMap::new();
    let mut c_loc: HashMap<String, u32> = HashMap::new();
    for &i in idxs {
        let top = folder_of(&files[i].path);
        *c_cx.entry(top.clone()).or_insert(0) += files[i].complexity;
        *c_loc.entry(top).or_insert(0) += files[i].loc;
    }
    let mut rows: Vec<FolderRow> = c_cx
        .iter()
        .map(|(d, cx)| FolderRow {
            dir: d.clone(),
            complexity: *cx,
            loc: c_loc[d],
            share: if denom > 0 {
                py_round1(100.0 * *cx as f64 / denom as f64)
            } else {
                0.0
            },
        })
        .collect();
    rows.sort_by(|a, b| {
        // Total order (complexity desc, dir asc): ties must not inherit
        // hash-map iteration order, which is random per run. The reference
        // keeps walk order here (unknowable); canonical order is the
        // deterministic choice, consistent with ext_top and clone pairs.
        b.complexity
            .cmp(&a.complexity)
            .then_with(|| a.dir.cmp(&b.dir))
    });
    rows.truncate(8);
    rows
}

// ---------------------------------------------------------------------------
// ---------------------------------------------------------------------------
// Guidance
// ---------------------------------------------------------------------------

pub fn build_guidance(
    verdict: &str,
    warnings: &[Warning],
    focus_rep: Option<&crate::focus::FocusRep>,
) -> Vec<String> {
    let mut g: Vec<String> = Vec::new();
    if verdict == "N/A" {
        g.push("OUT OF SCOPE: this audit says nothing about change risk. Check the unscored list; if the change touches unscored files, assess them by hand.".to_string());
    } else if verdict == "HIGH" || verdict == "SEVERE" {
        g.push("CONSTRAIN: default to the smallest diff that satisfies the requirement; split the change if it touches >2 hotspots.".to_string());
        g.push("TEST-FIRST: cover the touched hotspot with a characterization test before editing; complexity here punishes unverified edits.".to_string());
        g.push("NO WIDENING: do not add params/branches/exports in this change \u{2014} extend by composition, not by complicating hot functions.".to_string());
    } else if verdict == "MODERATE" {
        g.push("SCOPE: keep the change to one folder and \u{2264}3 files; stop and re-plan if the diff spreads further.".to_string());
        g.push("VERIFY CALLERS: check fan_in on every touched file (listed in hotspots) before merging.".to_string());
    } else {
        g.push("PROCEED: complexity is low \u{2014} prefer the straightforward change, but keep new functions under complexity 10.".to_string());
    }
    for x in warnings.iter().filter(|x| x.severity == "HIGH").take(5) {
        g.push(format!("CAUTION {}: {}.", x.r#where, x.msg));
    }
    if let Some(f) = focus_rep {
        g.push(format!("FOCUS: {}", f.assessment));
    }
    g.push("READ-ONLY NOTE: this audit did not modify the project; re-run `cxcap audit` after the change to confirm complexity did not grow disproportionately.".to_string());
    g
}

// ---------------------------------------------------------------------------
// Full report assembly + rendering
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct Phases {
    pub discovery_s: f64,
    pub read_parse_s: f64,
    pub relationships_s: f64,
    pub reporting_s: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Report {
    pub cxcap_version: &'static str,
    pub root: String,
    pub elapsed_s: f64,
    pub phases: Phases,
    pub files_total: usize,
    pub bytes_total: u64,
    pub code_files: usize,
    pub code_loc: u64,
    pub functions: usize,
    pub complexity: u32,
    pub prod_files: usize,
    pub prod_complexity: u32,
    pub test_files: usize,
    pub test_complexity: u32,
    pub test_hotspots: Vec<FileRec>,
    pub cycles: Vec<(Vec<String>, bool)>,
    pub clones: Vec<(String, String, usize)>,
    pub doc_files: usize,
    pub doc_loc: u64,
    pub config_files: usize,
    pub config_loc: u64,
    pub skipped_oversize: usize,
    pub skipped_unreadable: usize,
    pub skipped_symlinks: u64,
    pub parse_errors: Vec<String>,
    pub ext_top: Vec<(String, usize)>,
    pub unscored_top: Vec<(String, usize)>,
    pub loc_p50: u32,
    pub loc_p90: u32,
    pub hotspots: Vec<FileRec>,
    pub folders: Vec<FolderRow>,
    pub warnings: Vec<Warning>,
    pub verdict: String,
    pub verdict_why: String,
    pub guidance: Vec<String>,
    pub focus: Option<crate::focus::FocusRep>,
    pub intent: Option<crate::intent::IntentRep>,
    pub notes: Vec<String>,
}

pub struct AssembleArgs {
    pub version: &'static str,
    pub root: String,
    pub elapsed_s: f64,
    pub discovery_s: f64,
    pub read_parse_s: f64,
    pub relationships_s: f64,
    pub reporting_s: f64,
    pub files: Vec<FileRec>,
    pub bytes_total: u64,
    pub code_loc: u64,
    pub doc_files: usize,
    pub doc_loc: u64,
    pub cfg_files: usize,
    pub cfg_loc: u64,
    pub skipped_oversize: usize,
    pub skipped_unreadable: usize,
    pub symlinks: u64,
    pub ext_counts: HashMap<String, usize>,
    pub edges: Vec<(String, String, bool)>,
    pub cycles: Vec<(Vec<String>, bool)>,
    pub clone_pairs: Vec<(String, String, usize)>,
    pub top_n: usize,
    pub focus: Option<String>,
    pub intent: Option<String>,
    pub skip_dirs_sorted: Vec<String>,
}

pub fn assemble(mut a: AssembleArgs) -> Report {
    // Scored kinds: production code plus the verification surface.
    // (Production-only checks share scan::is_prod_kind; this four-way
    // split is used once, inline.)
    let code: Vec<usize> = a
        .files
        .iter()
        .enumerate()
        .filter(|(_, f)| matches!(f.kind, "python" | "js" | "rust" | "test"))
        .map(|(i, _)| i)
        .collect();
    let prod: Vec<usize> = a
        .files
        .iter()
        .enumerate()
        .filter(|(_, f)| crate::scan::is_prod_kind(f.kind))
        .map(|(i, _)| i)
        .collect();
    let test: Vec<usize> = code
        .iter()
        .filter(|&&i| a.files[i].kind == "test")
        .copied()
        .collect();
    let n_code = code.len();
    let cx_total: u32 = code.iter().map(|&i| a.files[i].complexity).sum();
    let cx_prod: u32 = prod.iter().map(|&i| a.files[i].complexity).sum();
    let func_total: usize = code.iter().map(|&i| a.files[i].funcs).sum();

    let (hot, test_hot) = score_hotspots(&mut a.files, a.top_n);

    let folders = aggregate(&code, &a.files, cx_total);
    let prod_folders = aggregate(&prod, &a.files, cx_prod);

    let mut locs: Vec<u32> = code.iter().map(|&i| a.files[i].loc).collect();
    locs.sort();
    let pct = |p: u32| -> u32 {
        if locs.is_empty() {
            return 0;
        }
        let i = ((p as f64 / 100.0 * locs.len() as f64) as usize).min(locs.len() - 1);
        locs[i]
    };
    let parse_errors: Vec<String> = code
        .iter()
        .filter(|&&i| a.files[i].parse_error.is_some())
        .map(|&i| a.files[i].path.clone())
        .collect();

    let pair_warns = crate::verdict::clone_pair_warnings(&a.clone_pairs, 5, 5);
    let warnings = crate::verdict::build_warnings(&prod, &a.files, &prod_folders, &parse_errors, &a.cycles, pair_warns);
    // Dominant unscored language (for the coverage-honesty verdict rule).
    let unscored_dominant: Option<(&str, usize)> = a
        .ext_counts
        .iter()
        .filter(|(e, _)| crate::verdict::UNSCORED_LANGS.contains(&e.as_str()))
        .max_by(|x, y| x.1.cmp(y.1).then_with(|| y.0.cmp(x.0)))
        .map(|(e, n)| (e.as_str(), *n));
    // Verdict floor follows production-involving cycles only:
    // verification-only SCCs constrain test/example reasoning, not the
    // production change (same rule as the warning severity in build_warnings).
    let prod_paths = crate::scan::prod_path_set(&a.files);
    let has_prod_cycle = a.cycles.iter().any(|(members, _)| {
        members.iter().any(|m| prod_paths.contains(m.as_str()))
    });
    let (verdict, verdict_why) = crate::verdict::build_verdict(
        prod.len(),
        n_code,
        cx_prod,
        &warnings,
        &prod_folders,
        has_prod_cycle,
        unscored_dominant,
    );

    let hot_paths: Vec<String> = hot.iter().map(|&i| a.files[i].path.clone()).collect();
    let focus_rep = crate::focus::build_focus(&code, &a.files, cx_total, n_code, a.focus.as_deref(), &a.edges, &a.cycles, &hot_paths);
    let intent_rep = crate::intent::build_intent(a.intent.as_deref(), &a.files, &a.edges, &a.cycles, &hot_paths, &a.root);
    let guidance = build_guidance(&verdict, &warnings, focus_rep.as_ref());

    let scored = |e: &str| {
        crate::dispatch::is_code_ext(e)
            || crate::dispatch::is_doc_ext(e)
            || crate::dispatch::is_cfg_ext(e)
    };
    let mut ext_top: Vec<(String, usize)> = a.ext_counts.iter().map(|(k, v)| (k.clone(), *v)).collect();
    ext_top.sort_by(|x, y| y.1.cmp(&x.1).then_with(|| x.0.cmp(&y.0)));
    let mut unscored: Vec<(String, usize)> = ext_top
        .iter()
        .filter(|(e, _)| !scored(e))
        .cloned()
        .collect();
    // NOTE: the reference ranks the FULL counter (walk-order ties) here;
    // this port ranks the same full set canonically (count desc, ext asc),
    // so boundary ties may name different extensions. Counts are exact.
    unscored.truncate(5);
    ext_top.truncate(12);

    Report {
        cxcap_version: a.version,
        root: a.root,
        elapsed_s: a.elapsed_s,
        phases: Phases {
            discovery_s: a.discovery_s,
            read_parse_s: a.read_parse_s,
            relationships_s: a.relationships_s,
            reporting_s: a.reporting_s,
        },
        files_total: a.files.len(),
        bytes_total: a.bytes_total,
        code_files: n_code,
        code_loc: a.code_loc,
        functions: func_total,
        complexity: cx_total,
        prod_files: prod.len(),
        prod_complexity: cx_prod,
        test_files: test.len(),
        test_complexity: cx_total - cx_prod,
        test_hotspots: test_hot.iter().map(|&i| a.files[i].clone()).collect(),
        cycles: a.cycles,
        clones: a.clone_pairs.into_iter().take(5).collect(),
        doc_files: a.doc_files,
        doc_loc: a.doc_loc,
        config_files: a.cfg_files,
        config_loc: a.cfg_loc,
        skipped_oversize: a.skipped_oversize,
        skipped_unreadable: a.skipped_unreadable,
        skipped_symlinks: a.symlinks,
        parse_errors,
        ext_top,
        unscored_top: unscored,
        loc_p50: pct(50),
        loc_p90: pct(90),
        hotspots: hot.iter().map(|&i| a.files[i].clone()).collect(),
        folders,
        warnings,
        verdict,
        verdict_why,
        guidance,
        focus: focus_rep,
        intent: intent_rep,
        notes: vec![
            "Python, Rust, and JS/TS metrics are AST-based (exact). .astro/.vue/.svelte score embedded script only (frontmatter + <script>); markup/CSS excluded. Rust `use` edges resolve onto the module tree (`crate`/`super`/`self` + workspace members by dir name; bare extern heads outside the workspace stay external); `#[cfg(test)]` subtrees are unscored.".to_string(),
            "fan_in counts runtime references only (type-only references draw no ripple claim, like cycles and focus); it resolves internal imports against full repo-relative paths (importer-aware for relative imports, package dirs to `__init__`, JS folder imports to `index.*`), falling back to stem matching only when ambiguous — still a heuristic, not a full resolver.".to_string(),
            "Workspace package names declared in package.json files count as internal: cross-package edges and cycles within the repo are analyzed. External bare specifiers stay external.".to_string(),
            format!("Skipped dirs (not scanned): {}.", a.skip_dirs_sorted.join(", ")),
            "Read-only: CXCAP only opens target files for reading; it never writes to the target.".to_string(),
        ],
    }
}

// ---------------------------------------------------------------------------
// Text rendering (byte-faithful to the reference)
// ---------------------------------------------------------------------------

/// Python repr() for small non-negative floats: shortest round-trip.
fn py_float(x: f64) -> String {
    format!("{x:?}")
}

pub fn render_text(rep: &Report, top_n: usize) -> String {
    let mut l: Vec<String> = Vec::new();
    l.push(format!(
        "CXCAP v{} \u{2014} complexity audit: {}",
        rep.cxcap_version, rep.root
    ));
    l.push(format!(
        "scanned {} in {}s | code: {}, {} LOC, complexity {}, functions {} | docs: {}, {} LOC | config: {}, {} LOC",
        n1(rep.files_total, "file"),
        py_float(rep.elapsed_s),
        n1(rep.code_files, "file"),
        rep.code_loc,
        rep.complexity,
        rep.functions,
        n1(rep.doc_files, "file"),
        rep.doc_loc,
        n1(rep.config_files, "file"),
        rep.config_loc
    ));
    if rep.test_files > 0 {
        let share = if rep.complexity > 0 {
            py_fmt1(py_round1(100.0 * rep.test_complexity as f64 / rep.complexity as f64))
        } else {
            "0".to_string()
        };
        l.push(format!(
            "production: {} cx={} | tests+examples+generated+types: {} cx={} ({}% of scored complexity)",
            n1(rep.prod_files, "file"),
            rep.prod_complexity,
            n1(rep.test_files, "file"),
            rep.test_complexity,
            share
        ));
    }
    l.push(format!(
        "file LOC p50={} p90={} | oversize-skipped={} parse_errors={}",
        rep.loc_p50, rep.loc_p90, rep.skipped_oversize, rep.parse_errors.len()
    ));
    if !rep.unscored_top.is_empty() {
        l.push(format!(
            "unscored (not analyzed): {}",
            rep.unscored_top
                .iter()
                .map(|(e, n)| {
                    // JSON keeps the "(none)" key; text says what it means.
                    let e = if e == "(none)" { "(no extension)" } else { e.as_str() };
                    format!("{n}x {e}")
                })
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if rep.skipped_symlinks > 0 {
        l.push(format!(
            "skipped {} symlinked path(s) (not followed \u{2014} counts may understate)",
            rep.skipped_symlinks
        ));
    }
    if rep.skipped_unreadable > 0 {
        l.push(format!(
            "skipped {} unreadable file(s) (permission errors \u{2014} counts may understate)",
            rep.skipped_unreadable
        ));
    }
    l.push(format!("VERDICT: {} \u{2014} {}", rep.verdict, rep.verdict_why));
    l.push(String::new());
    l.push(format!(
        "TOP {} HOTSPOTS (hotspot 0-100 = 35% LOC + 35% complexity + 20% coupling + 10% funcs):",
        top_n.min(rep.hotspots.len())
    ));
    for h in &rep.hotspots {
        // Hotspot suffix: only non-zero signals print (single use).
        let mut extra = String::new();
        if h.max_func_cx.unwrap_or(0) > 0 {
            extra.push_str(&format!(" maxFuncCx={}", h.max_func_cx.unwrap_or(0)));
        }
        if h.dup_lines.unwrap_or(0) > 0 {
            extra.push_str(&format!(" dup={}", h.dup_lines.unwrap_or(0)));
        }
        l.push(format!(
            "  {:5.1}  {}  LOC={} cx={} fn={} nest={} coup={}{}",
            py_round1(h.hotspot.unwrap_or(0.0)),
            h.path,
            h.loc,
            h.complexity,
            h.funcs,
            h.max_nesting,
            h.coupling.unwrap_or(0),
            extra
        ));
    }
    if !rep.test_hotspots.is_empty() {
        l.push(format!(
            "HEAVIEST TESTS+EXAMPLES (verification surface, not change targets): {}",
            rep.test_hotspots
                .iter()
                .map(|t| format!("{} cx={}", t.path, t.complexity))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if !rep.clones.is_empty() {
        l.push(format!(
            "CLONES (cross-file copy-paste, fix in all places or extract): {}",
            rep.clones
                .iter()
                .map(|(a, b, n)| format!("{a} \u{2194} {b} ({})", n1(*n, "block")))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    l.push(String::new());
    l.push("FOLDERS by complexity:".to_string());
    for fo in &rep.folders {
        l.push(format!(
            "  {:5.1}%  {}/  cx={} LOC={}",
            py_round1(fo.share),
            fo.dir,
            fo.complexity,
            fo.loc
        ));
    }
    l.push(String::new());
    l.push(format!(
        "WARNINGS ({} total, showing {}, HIGH first):",
        rep.warnings.len(),
        20.min(rep.warnings.len())
    ));
    if rep.warnings.is_empty() {
        l.push("  (none)".to_string());
    }
    for x in rep.warnings.iter().take(20) {
        l.push(format!("  [{}] {}: {}", x.severity, x.r#where, x.msg));
    }
    if let Some(f) = &rep.focus {
        l.push(String::new());
        l.push(format!(
            "FOCUS '{}': {}, complexity {} ({}% of repo)",
            f.pattern,
            n1(f.files, "file"),
            f.complexity,
            py_fmt1(f.share_pct)
        ));
        l.push(format!("  {}", f.assessment));
        if f.n_dependents > 0 {
            let deps: Vec<String> = f
                .dependents
                .iter()
                .map(|d| {
                    format!("{}{}", d.path, if d.kind == "test" { "(test)" } else { "" })
                })
                .collect();
            l.push(format!("  DEPENDENTS ({} outside): {}", f.n_dependents, deps.join(", ")));
        }
        if f.n_transitive > 0 {
            let trans: Vec<String> = f
                .transitive_dependents
                .iter()
                .map(|d| {
                    format!("{}{}", d.path, if d.kind == "test" { "(test)" } else { "" })
                })
                .collect();
            l.push(format!(
                "  TRANSITIVE ({} beyond direct): {}",
                f.n_transitive,
                trans.join(", ")
            ));
        }
        if !f.cycles_through.is_empty() {
            l.push(format!(
                "  CYCLES: {}",
                f.cycles_through
                    .iter()
                    .take(2)
                    .map(|c| c.summary())
                    .collect::<Vec<_>>()
                    .join("; ")
            ));
        }
        for t in &f.top {
            l.push(format!(
                "    {:5.1}  {}  LOC={} cx={} coup={}",
                py_round1(t.hotspot.unwrap_or(0.0)),
                t.path,
                t.loc,
                t.complexity,
                t.coupling.unwrap_or(0)
            ));
        }
    }
    if let Some(it) = &rep.intent {
        l.push(String::new());
        l.push(format!("CHANGE EXPOSURE (intent: '{}'):", it.query));
        l.push(format!("  {}", it.assessment));
        if !it.seeds.is_empty() {
            let tops: Vec<String> =
                it.seeds.iter().take(5).map(|s| format!("{} ({})", s.path, s.why)).collect();
            l.push(format!("  LIKELY TOUCHPOINTS: {}", tops.join("; ")));
        }
        if !it.expanded.is_empty() {
            let cap = if it.capped {
                format!(" (capped at {} files; more may be in reach)", it.expanded.len())
            } else {
                String::new()
            };
            l.push(format!(
                "  CONTEXT SURFACE{cap}: {}; files: {}",
                it.context_lines.join("; "),
                it.expanded.join(", ")
            ));
        }
        if !it.component_tangles.is_empty() {
            l.push(format!("  COMPONENT TANGLES: {}", it.component_tangles.join("; ")));
        }
        if !it.uncertainty.is_empty() {
            let us: Vec<String> = it
                .uncertainty
                .iter()
                .take(5)
                .map(|u| format!("{} [{}]: {}", u.path, u.kind, u.detail))
                .collect();
            l.push(format!("  UNCERTAINTY: {}", us.join("; ")));
        }
    }
    l.push(String::new());
    l.push("CONSTRAINTS FOR YOUR NEXT DECISION:".to_string());
    for (i, x) in rep.guidance.iter().enumerate() {
        l.push(format!("  {}. {}", i + 1, x));
    }
    l.push(String::new());
    l.push(format!("Notes: {}", rep.notes.join(" | ")));
    l.join("\n")
}
