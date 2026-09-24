//! Change exposure (--intent): lexical seeds plus graph expansion plus context.
//!
//! Pure analysis over already-collected scan data plus a bounded,
//! read-only re-read for static-confidence flags. No persistent index;
//! default output (no --intent) is untouched.

use crate::scan::FileRec;
use serde::Serialize;
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Change exposure (--intent): lexical seeds + graph expansion + context
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct IntentSeed {
    pub path: String,
    pub score: f64,
    pub why: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct OpacityNote {
    pub path: String,
    pub kind: String,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct IntentRep {
    pub query: String,
    pub seeds: Vec<IntentSeed>,
    pub expanded: Vec<String>,
    /// The context set hit INTENT_EXPAND_CAP; more files may be in reach.
    pub capped: bool,
    pub context_lines: Vec<String>,
    pub component_tangles: Vec<String>,
    pub uncertainty: Vec<OpacityNote>,
    pub assessment: String,
}

const INTENT_SEEDS: usize = 8;
const INTENT_EXPAND_CAP: usize = 25;

/// Ephemeral intent analysis: rank lexical seeds, expand along the existing
/// dependency graph (1 hop, capped), summarize the descriptive context
/// surface and cross-component tangles, and re-read at most INTENT_EXPAND_CAP
/// files for static-confidence flags. No persistent index; default output
/// (no --intent) is untouched.
pub fn build_intent(
    intent: Option<&str>,
    files: &[FileRec],
    edges: &[(String, String, bool)],
    cycles: &[(Vec<String>, bool)],
    hot_paths: &[String],
    root: &str,
) -> Option<IntentRep> {
    let q = intent?;
    if q.trim().is_empty() {
        return None;
    }
    let ranked = crate::lexical::rank(q, files, INTENT_SEEDS);
    let seed_paths: Vec<String> = ranked.iter().map(|(p, _)| p.clone()).collect();
    let edges2: Vec<(String, String)> =
        edges.iter().map(|(a, b, _)| (a.clone(), b.clone())).collect();
    let expanded = crate::lexical::expand_seeds(&seed_paths, &edges2, 1, INTENT_EXPAND_CAP);
    let capped = expanded.len() >= INTENT_EXPAND_CAP;
    let loc: HashMap<String, (u32, String, u32)> = files
        .iter()
        .map(|f| (f.path.clone(), (f.loc, f.kind.to_string(), f.complexity)))
        .collect();
    let cyc: Vec<Vec<String>> = cycles.iter().map(|(c, _)| c.clone()).collect();
    let ctx = crate::context::summarize(&expanded, &loc, &edges2, &cyc);
    // Tangles describe the context set, like the counts beside them: only
    // edges and cycle members inside the set may make a component tangled.
    let in_set: std::collections::HashSet<&str> = expanded.iter().map(|p| p.as_str()).collect();
    let set_edges: Vec<(String, String)> = edges2
        .iter()
        .filter(|(a, b)| in_set.contains(a.as_str()) && in_set.contains(b.as_str()))
        .cloned()
        .collect();
    let set_cycles: Vec<Vec<String>> = cyc
        .iter()
        .map(|c| c.iter().filter(|f| in_set.contains(f.as_str())).cloned().collect())
        .collect();
    let tangles: Vec<String> = crate::component::summarize(&expanded, &set_edges, &set_cycles)
        .into_iter()
        .filter(|c| c.tangle)
        .map(|c| {
            if c.bidirectional.is_empty() {
                format!("{} (multi-component cycle)", c.name)
            } else {
                format!("{} <-> {}", c.name, c.bidirectional.join(", "))
            }
        })
        .collect();
    let hot: std::collections::HashSet<&str> =
        hot_paths.iter().map(|s| s.as_str()).collect();
    let mut importers: HashMap<&str, usize> = HashMap::new();
    for (_, b) in &edges2 {
        *importers.entry(b.as_str()).or_insert(0) += 1;
    }
    let seeds: Vec<IntentSeed> = ranked
        .iter()
        .map(|(p, s)| {
            let mut why = format!("lexical intent match (score {:.2})", s);
            if hot.contains(p.as_str()) {
                why.push_str("; repo hotspot");
            }
            let n = importers.get(p.as_str()).copied().unwrap_or(0);
            if n > 0 {
                why.push_str(&format!("; {} direct {}", n, if n == 1 { "dependent" } else { "dependents" }));
            }
            IntentSeed { path: p.clone(), score: (s * 100.0).round() / 100.0, why }
        })
        .collect();
    // Static-confidence re-read: bounded, read-only, silent on unreadable.
    let mut uncertainty = Vec::new();
    let base = std::path::Path::new(root);
    for p in expanded.iter() {
        if uncertainty.len() >= INTENT_EXPAND_CAP {
            break;
        }
        let kind = files.iter().find(|f| f.path.as_str() == p.as_str()).map(|f| f.kind);
        let lang = match kind {
            Some("python") => "python",
            Some("js") => "js",
            _ => continue,
        };
        let Ok(bytes) = std::fs::read(base.join(p)) else {
            continue;
        };
        let text = crate::dispatch::decode_text(&bytes);
        for fl in crate::opacity::flags_for_text(&text, lang) {
            uncertainty.push(OpacityNote {
                path: p.clone(),
                kind: fl.kind.to_string(),
                detail: fl.detail,
            });
        }
    }
    let assessment = assess_intent(q, &seeds, &ctx, &tangles, uncertainty.len());
    Some(IntentRep {
        query: q.to_string(),
        seeds,
        expanded,
        capped,
        context_lines: ctx.lines,
        component_tangles: tangles,
        uncertainty,
        assessment,
    })
}

fn assess_intent(
    q: &str,
    seeds: &[IntentSeed],
    ctx: &crate::context::ContextSurface,
    tangles: &[String],
    n_uncertain: usize,
) -> String {
    if seeds.is_empty() {
        return format!(
            "No files matched the intent vocabulary in '{q}' \u{2014} the concept may be new, named differently, or live in unscored surfaces. Fall back to --focus once a candidate area is known."
        );
    }
    let lead: Vec<&str> = seeds.iter().take(3).map(|s| s.path.as_str()).collect();
    let mut s = format!(
        "Likely touchpoints for '{q}': {}. Reasoning surface: {} production {} in {} ({} cross-boundary edges, {} {}).",
        lead.join(", "),
        crate::fmt::n1(ctx.prod_files, "file"),
        format!("(~{} tokens est.)", ctx.prod_tokens_est),
        crate::fmt::n1(ctx.components, "component"),
        ctx.cross_boundaries,
        ctx.cycles_involved,
        if ctx.cycles_involved == 1 { "cycle" } else { "cycles" },
    );
    if tangles.is_empty() {
        s.push_str(" No cross-component tangles.");
    } else {
        s.push_str(&format!(" Cross-component tangle: {}.", tangles.join("; ")));
    }
    if n_uncertain > 0 {
        s.push_str(&format!(
            " {} {} with dynamic behavior \u{2014} static blast radius may be incomplete.",
            n_uncertain,
            if n_uncertain == 1 { "area" } else { "areas" }
        ));
    }
    s.push_str(" Keep the change narrow; verify direct dependents before editing.");
    s
}
