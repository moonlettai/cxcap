//! Proposed-change focus analysis: assess one path area against the repo baseline.
//!
//! Reports the area's complexity share, outside dependents, transitive reach,
//! hotspots inside, and cycles passing through it, plus a planning assessment.
//! Verification-only areas are reported as such instead of alarming.

use crate::scan::FileRec;
use serde::Serialize;
use std::collections::HashMap;

// Focus
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct Dependent {
    pub path: String,
    pub kind: String,
    pub coupling: u32,
}

/// An import cycle passing through a focus area, with its actionability:
/// eager cycles tangle initialization (break before extending), lazy ones
/// (function-level imports) are entangled reasoning only. Carried
/// explicitly because truncated member lists alone collide (two different
/// cycles can share a display prefix).
#[derive(Debug, Clone, Serialize)]
pub struct CycleRef {
    pub members: Vec<String>,
    pub eager: bool,
}

impl CycleRef {
    /// `31-module cycle (a ↔ b ↔ c…)` / `142-module lazy tangle (…)`.
    pub fn summary(&self) -> String {
        let shown: String = self
            .members
            .iter()
            .take(3)
            .cloned()
            .collect::<Vec<_>>()
            .join(" \u{2194} ");
        let more = if self.members.len() > 3 { "\u{2026}" } else { "" };
        if self.eager {
            format!(
                "{}-module cycle ({}{})",
                self.members.len(),
                shown,
                more
            )
        } else {
            format!(
                "{}-module lazy tangle ({}{})",
                self.members.len(),
                shown,
                more
            )
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct FocusRep {
    pub pattern: String,
    pub files: usize,
    pub complexity: u32,
    pub share_pct: f64,
    pub top: Vec<FileRec>,
    pub dependents: Vec<Dependent>,
    pub n_dependents: usize,
    pub transitive_dependents: Vec<Dependent>,
    pub n_transitive: usize,
    pub hotspots_inside: Vec<String>,
    pub cycles_through: Vec<CycleRef>,
    pub assessment: String,
}

fn assess_focus(
    ff: &[usize],
    files: &[FileRec],
    fcx: u32,
    cx_total: u32,
    n_code: usize,
    dependents: &[Dependent],
    n_dependents: usize,
    transitive: &[Dependent],
    n_transitive: usize,
    hotspots_inside: &[String],
    cycles_through: &[CycleRef],
) -> String {
    if ff.is_empty() {
        return "Focus pattern matched no code files \u{2014} the proposed area may be new. New code still inherits repo conventions; keep it isolated and small.".to_string();
    }
    let share = if cx_total > 0 {
        100.0 * fcx as f64 / cx_total as f64
    } else {
        0.0
    };
    if ff.len() == n_code && n_code > 0 {
        return "Focus covers essentially the whole repo \u{2014} no baseline contrast available. ".to_string()
            + "Treat the repo verdict as the focus verdict.";
    }
    if ff.iter().all(|&i| files[i].kind == "test") {
        return "Focus area is entirely verification surface (tests, examples, "
            .to_string()
            + "generated files, type declarations): changes here do not raise "
            + "production complexity. Keep them hermetic and fast, coupled to "
            + "the API they verify \u{2014} if production code must change to satisfy "
            + "them, assess that area separately.";
    }
    let avg_all = if n_code > 0 {
        cx_total as f64 / n_code as f64
    } else {
        0.0
    };
    let avg_f = fcx as f64 / ff.len() as f64;
    let mut base = if share > 30.0 || avg_f > avg_all * 1.5 {
        format!(
            "Focus area holds {}% of repo complexity across {} (avg {} vs repo avg {}). A 'simple' change here interacts with expensive code \u{2014} constrain scope, add/extend tests first, avoid signature widening.",
            crate::fmt::py_fmt1(share),
            crate::fmt::n1(ff.len(), "file"),
            crate::fmt::py_fmt1(avg_f),
            crate::fmt::py_fmt1(avg_all)
        )
    } else if avg_f > avg_all {
        format!(
            "Focus area is above-average complexity (avg {} vs {}). Proceed with a tight diff and verify callers.",
            crate::fmt::py_fmt1(avg_f),
            crate::fmt::py_fmt1(avg_all)
        )
    } else {
        format!(
            "Focus area is below-average complexity (avg {} vs {}). Cheapest place to change \u{2014} still keep the diff local.",
            crate::fmt::py_fmt1(avg_f),
            crate::fmt::py_fmt1(avg_all)
        )
    };
    let mut extra: Vec<String> = Vec::new();
    if n_dependents > 0 {
        let names: Vec<&str> = dependents.iter().take(3).map(|d| d.path.as_str()).collect();
        let more = if n_dependents > dependents.len() {
            format!(" (+{} more)", n_dependents - dependents.len())
        } else {
            String::new()
        };
        extra.push(format!(
            "Imported by {} ({names}{more}) \u{2014} keep interfaces stable or update them in the same diff.",
            crate::fmt::n1(n_dependents, "outside file"),
            names = names.join(", "),
            more = more
        ));
    }
    if n_transitive > 0 {
        let names: Vec<&str> = transitive.iter().take(3).map(|d| d.path.as_str()).collect();
        let more = if n_transitive > transitive.len() {
            format!(" (+{} more)", n_transitive - transitive.len())
        } else {
            String::new()
        };
        extra.push(format!(
            "Reaches {} transitively ({names}{more}) \u{2014} interface changes propagate beyond direct importers.",
            crate::fmt::n_txt(n_transitive, "further file", None),
            names = names.join(", "),
            more = more
        ));
    }
    if !hotspots_inside.is_empty() {
        // The test-cover imperative is earned only by actually-expensive
        // overlap. In tiny repos top-N covers every file, so a trivial
        // file vacuously "touches a hotspot" — demanding test cover for
        // a 1-line constant is noise of the same family fixed in #41.
        // Gate the imperative on an overlapped file exceeding the repo
        // average (the same baseline contrast the lead sentence uses):
        // no new threshold, no formula change. The neutral listing stays
        // either way — overlap is real evidence, scores beside it in
        // focus.top give context.
        let earned = hotspots_inside.iter().any(|p| {
            files
                .iter()
                .find(|f| f.path.as_str() == p.as_str())
                .map(|f| f.complexity as f64 > avg_all)
                .unwrap_or(false)
        });
        if earned {
            extra.push(format!(
                "Touches {}: {} \u{2014} cover with tests before editing.",
                crate::fmt::n1(hotspots_inside.len(), "repo hotspot"),
                hotspots_inside.iter().take(4).cloned().collect::<Vec<_>>().join(", ")
            ));
        } else {
            extra.push(format!(
                "Touches {}: {}.",
                crate::fmt::n1(hotspots_inside.len(), "repo hotspot"),
                hotspots_inside.iter().take(4).cloned().collect::<Vec<_>>().join(", ")
            ));
        }
    }
    if !cycles_through.is_empty() {
        let shown: Vec<String> = cycles_through.iter().take(2).map(|c| c.summary()).collect();
        extra.push(format!(
            "Touches {} ({}) \u{2014} understand all members before changing one.",
            crate::fmt::n1(cycles_through.len(), "import cycle"),
            shown.join("; ")
        ));
    }
    if !extra.is_empty() {
        base.push(' ');
        base.push_str(&extra.join(" "));
    }
    base
}

#[allow(clippy::too_many_arguments)]
pub fn build_focus(
    idxs: &[usize],
    files: &[FileRec],
    cx_total: u32,
    n_code: usize,
    focus: Option<&str>,
    edges: &[(String, String, bool)],
    cycles: &[(Vec<String>, bool)],
    hot_paths: &[String],
) -> Option<FocusRep> {
    let focus = focus?;
    // Segment-boundary match: the pattern must start the path or follow a
    // separator. `foo.py` matches `lib/foo.py` but not `lib/test_foo.py`.
    // (Single use; kept inline beside its only filter.)
    let ff: Vec<usize> = idxs
        .iter()
        .filter(|&&i| {
            let path = &files[i].path;
            path == focus || path.starts_with(focus) || path.contains(&format!("/{focus}"))
        })
        .copied()
        .collect();
    let fcx: u32 = ff.iter().map(|&i| files[i].complexity).sum();
    let inset: std::collections::HashSet<&str> =
        ff.iter().map(|&i| files[i].path.as_str()).collect();
    let mut dep_count: HashMap<&str, usize> = HashMap::new();
    for (importer, target, _) in edges {
        if inset.contains(target.as_str()) && !inset.contains(importer.as_str()) {
            *dep_count.entry(importer.as_str()).or_insert(0) += 1;
        }
    }
    let mut dependents: Vec<Dependent> = dep_count
        .keys()
        .map(|p| {
            let (kind, coupling) = files
                .iter()
                .find(|f| f.path.as_str() == (*p))
                .map(|f| (f.kind.to_string(), f.coupling.unwrap_or(0)))
                .unwrap_or(("?".to_string(), 0));
            Dependent {
                path: p.to_string(),
                kind,
                coupling,
            }
        })
        .collect();
    dependents.sort_by(|a, b| {
        // Total order (coupling desc, path asc): dep_count is a hash map,
        // so ties must not inherit its random iteration order. Same
        // canonical-tie policy as everywhere else in this port.
        b.coupling.cmp(&a.coupling).then_with(|| a.path.cmp(&b.path))
    });
    dependents.truncate(5);
    let n_dependents = dep_count.len();
    // Transitive blast radius: importers of dependents, excluding the area
    // and its direct dependents. Direct-only radius hides signature-change
    // breakage down import chains (each link reads LOW in isolation).
    let mut importers_of: HashMap<&str, Vec<&str>> = HashMap::new();
    for (importer, target, _) in edges {
        importers_of
            .entry(target.as_str())
            .or_default()
            .push(importer.as_str());
    }
    let direct: std::collections::HashSet<&str> = dep_count.keys().copied().collect();
    let mut seen: std::collections::HashSet<&str> =
        inset.iter().copied().chain(direct.iter().copied()).collect();
    let mut frontier: Vec<&str> = direct.iter().copied().collect();
    let mut transitive: Vec<Dependent> = Vec::new();
    while let Some(node) = frontier.pop() {
        if let Some(importers) = importers_of.get(node) {
            for imp in importers {
                if seen.insert(*imp) {
                    let (kind, coupling) = files
                        .iter()
                        .find(|f| f.path.as_str() == (*imp))
                        .map(|f| (f.kind.to_string(), f.coupling.unwrap_or(0)))
                        .unwrap_or(("?".to_string(), 0));
                    transitive.push(Dependent {
                        path: imp.to_string(),
                        kind,
                        coupling,
                    });
                    frontier.push(*imp);
                }
            }
        }
    }
    transitive.sort_by(|a, b| {
        b.coupling.cmp(&a.coupling).then_with(|| a.path.cmp(&b.path))
    });
    transitive.truncate(5);
    // Total reachable beyond direct (visited minus area minus direct).
    let n_transitive = seen.len() - inset.len() - direct.len();
    let hotspots_inside: Vec<String> = hot_paths
        .iter()
        .filter(|p| inset.contains(p.as_str()))
        .cloned()
        .collect();
    let mut cycles_through: Vec<CycleRef> = Vec::new();
    for (c, eager) in cycles {
        if c.iter().any(|m| inset.contains(m.as_str())) {
            // Rotate so a focus member leads: the reader sees WHY this
            // cycle is listed under their area, not a foreign file list.
            let i = c.iter().position(|m| inset.contains(m.as_str())).unwrap();
            cycles_through.push(CycleRef {
                members: c[i..].iter().chain(c[..i].iter()).cloned().collect(),
                eager: *eager,
            });
        }
    }
    let mut top: Vec<FileRec> = ff.iter().map(|&i| files[i].clone()).collect();
    top.sort_by(|a, b| {
        b.hotspot
            .unwrap_or(0.0)
            .partial_cmp(&a.hotspot.unwrap_or(0.0))
            .unwrap()
    });
    top.truncate(5);
    let assessment = assess_focus(
        &ff,
        files,
        fcx,
        cx_total,
        n_code,
        &dependents,
        n_dependents,
        &transitive,
        n_transitive,
        &hotspots_inside,
        &cycles_through,
    );
    Some(FocusRep {
        pattern: focus.to_string(),
        files: ff.len(),
        complexity: fcx,
        share_pct: if cx_total > 0 {
            crate::fmt::py_round1(100.0 * fcx as f64 / cx_total as f64)
        } else {
            0.0
        },
        top,
        dependents,
        n_dependents,
        transitive_dependents: transitive,
        n_transitive,
        hotspots_inside,
        cycles_through,
        assessment,
    })
}

pub fn normalize_focus(focus: Option<String>) -> Option<String> {
    let mut f = focus?;
    // `./x` and `x/` name the same area as `x`; match on the stripped form.
    f = f.trim().to_string();
    if let Some(s) = f.strip_prefix("./") {
        f = s.to_string();
    }
    f = f.trim_end_matches(['/', '\\']).to_string();
    if f.is_empty() {
        None
    } else {
        Some(f)
    }
}
