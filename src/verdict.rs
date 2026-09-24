//! Verdicts and warnings: repository-level judgment from scored evidence.
//!
//! Decides the overall verdict (including coverage honesty for
//! unscored-language dominance) and builds file-level warnings with
//! concrete evidence. Thresholds are documented guards, not tuning knobs.

use crate::fmt::FolderRow;
use crate::scan::FileRec;
use serde::Serialize;

// ---------------------------------------------------------------------------
// Warnings, verdict
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct Warning {
    pub severity: &'static str,
    pub r#where: String,
    pub msg: String,
}

fn where_shown(members: &[String]) -> String {
    let shown = members.iter().take(4).cloned().collect::<Vec<_>>().join(" \u{2194} ");
    if members.len() <= 4 {
        shown
    } else {
        format!("{shown} (+{} more)", members.len() - 4)
    }
}

/// Cross-file copy-paste worth knowing before a change. Only pairs spanning
/// folders qualify (same-folder clones are a local extract).
pub fn clone_pair_warnings(
    pairs: &[(String, String, usize)],
    min_windows: usize,
    cap: usize,
) -> Vec<Warning> {
    let mut out = Vec::new();
    for (a, b, n) in pairs {
        if *n < min_windows {
            break; // pairs sorted desc
        }
        let fa = crate::fmt::folder_of(a);
        let fb = crate::fmt::folder_of(b);
        if fa == fb {
            continue;
        }
        out.push(Warning {
            severity: "WATCH",
            r#where: format!("{a} \u{2194} {b}"),
            msg: format!(
                "shares {n} matching 8-line blocks ({fa}/ \u{2194} {fb}/) \u{2014} a fix here must be applied in both places or extracted once"
            ),
        });
        if out.len() >= cap {
            break;
        }
    }
    out
}

fn god_candidate(folders: &[FolderRow]) -> Option<&FolderRow> {
    // The single locus of complexity, or None. Requires >40% share AND
    // more than double the runner-up: two near-equal loci co-dominate.
    let big: Vec<&FolderRow> = folders.iter().filter(|fo| fo.share >= 10.0).collect();
    if big.len() < 2 {
        return None;
    }
    let (top, second) = (big[0], big[1]);
    if top.share > 40.0 && top.share > 2.0 * second.share {
        return Some(top);
    }
    None
}

/// Full warning list: verdicts and JSON need true totals; text caps display.
pub fn build_warnings(
    idxs: &[usize],
    files: &[FileRec],
    folders: &[FolderRow],
    parse_errors: &[String],
    cycles: &[(Vec<String>, bool)],
    extra: Vec<Warning>,
) -> Vec<Warning> {
    let mut w = extra;
    // Production membership per cycle member: verification-only SCCs
    // (tests/examples/generated/type-declarations) are still listed, but
    // must not read as production constraints (#80/90: DT yuka/athenajs
    // test-example cycles, TS fixture mirrors).
    let prod_paths = crate::scan::prod_path_set(files);
    // Small cycles first: they fit in a human head. Large tangles and lazy
    // cycles are context, not action items, but still precede other warnings.
    let mut ordered: Vec<&(Vec<String>, bool)> = cycles.iter().collect();
    ordered.sort_by_key(|(c, eager)| {
        if *eager && c.len() <= 8 {
            0
        } else {
            1
        }
    });
    for (members, eager) in ordered.iter().take(6) {
        let prod_members = members
            .iter()
            .filter(|m| prod_paths.contains(m.as_str()))
            .count();
        if *eager && members.len() <= 8 && prod_members == 0 {
            w.push(Warning {
                severity: "WATCH",
                r#where: where_shown(members),
                msg: format!(
                    "import cycle across {} (verification-only surface) \u{2014} no production member; entangled test/example reasoning, not a production constraint",
                    crate::fmt::n1(members.len(), "module")
                ),
            });
        } else if *eager && members.len() <= 8 {
            w.push(Warning {
                severity: "HIGH",
                r#where: where_shown(members),
                msg: format!(
                    "import cycle across {} \u{2014} tangled initialization and reuse; understand all members before changing one, break the cycle before extending it",
                    crate::fmt::n1(members.len(), "module")
                ),
            });
        } else if *eager {
            w.push(Warning {
                severity: "WATCH",
                r#where: where_shown(members),
                msg: format!(
                    "large dependency tangle across {} \u{2014} too big to reason about as units; treat the area as one, avoid adding members or eager cross-imports",
                    crate::fmt::n1(members.len(), "module")
                ),
            });
        } else {
            w.push(Warning {
                severity: "WATCH",
                r#where: where_shown(members),
                msg: format!(
                    "static dependency cycle across {} (lazy imports) \u{2014} entangled reasoning; do not make cross-imports eager",
                    crate::fmt::n1(members.len(), "module")
                ),
            });
        }
    }
    for &i in idxs {
        let f = &files[i];
        let p = f.path.clone();
        if f.loc > 800 {
            // Flat data-like files break the "many reasons to change" claim:
            // >800 LOC with near-zero branching and no dependents means
            // single-line, contained edits. Say so honestly at WATCH and
            // point at the generated-vs-hand-maintained question instead.
            // Flat-but-coupled files keep the HIGH: every edit still reaches
            // all dependents (the coupling warning names the ripple).
            // Bounds (cx<=5, coup<10) are the documented empirical separator,
            // not tuning: LOC>800 coincides with cx>5 or coup>=10 almost
            // everywhere else.
            if f.complexity <= 5 && f.coupling.unwrap_or(0) < 10 {
                w.push(Warning {
                    severity: "WATCH",
                    r#where: p.clone(),
                    msg: format!(
                        "large flat file ({} LOC, complexity {}) \u{2014} data-like: cheap per-line edits, low coordination cost; confirm hand-maintained, not generated output",
                        f.loc, f.complexity
                    ),
                });
            } else {
                w.push(Warning {
                    severity: "HIGH",
                    r#where: p.clone(),
                    msg: format!(
                        "large file ({} LOC) \u{2014} many reasons to change, high coordination cost",
                        f.loc
                    ),
                });
            }
        } else if f.loc > 500 {
            w.push(Warning {
                severity: "WATCH",
                r#where: p.clone(),
                msg: format!(
                    "big file ({} LOC) \u{2014} check if change can stay local",
                    f.loc
                ),
            });
        }
        let mcx = f.max_func_cx.unwrap_or(0);
        if mcx > 15 {
            w.push(Warning {
                severity: "HIGH",
                r#where: p.clone(),
                msg: format!(
                    "function complexity {mcx} \u{2014} dense branching, test before touching"
                ),
            });
        } else if mcx > 10 {
            w.push(Warning {
                severity: "WATCH",
                r#where: p.clone(),
                msg: format!(
                    "function complexity {mcx} \u{2014} follow existing branch structure exactly"
                ),
            });
        }
        if f.max_nesting > 4 {
            w.push(Warning {
                severity: "WATCH",
                r#where: p.clone(),
                msg: format!(
                    "nesting depth {} \u{2014} deep context, extract rather than nest further",
                    f.max_nesting
                ),
            });
        }
        if f.max_params.unwrap_or(0) > 5 {
            w.push(Warning {
                severity: "WATCH",
                r#where: p.clone(),
                msg: format!(
                    "function with {} params \u{2014} wire carefully, avoid widening signatures",
                    f.max_params.unwrap_or(0)
                ),
            });
        }
        let coupling = f.coupling.unwrap_or(0);
        let fan_in = f.fan_in.unwrap_or(0);
        if coupling >= 20 {
            // Direction matters: pure fan-out (hubs, CLIs, barrels) breaks
            // when its imports change, not when edited — "ripples" would
            // point the reader's caution backwards.
            let tail = if fan_in == 0 {
                "outgoing only: editing is locally contained, upstream changes break here first"
            } else {
                "change ripples"
            };
            w.push(Warning {
                severity: "HIGH",
                r#where: p.clone(),
                msg: format!(
                    "coupling {coupling} (fan_in {fan_in} + internal fan_out {}) \u{2014} {tail}",
                    f.fan_out_in
                ),
            });
        } else if coupling >= 10 {
            let tail = if fan_in == 0 {
                "outgoing only: check its imports before editing"
            } else {
                "check callers/importers before editing"
            };
            w.push(Warning {
                severity: "WATCH",
                r#where: p.clone(),
                msg: format!("coupling {coupling} \u{2014} {tail}"),
            });
        }
    }
    // God-folder only meaningful when complexity could live in several
    // substantial places: single-package layouts concentrate trivially, and
    // two near-equal loci are co-dominance, not a god.
    if idxs.len() >= 10 {
        if let Some(god) = god_candidate(folders) {
            w.push(Warning {
                severity: "HIGH",
                r#where: format!("{}/", god.dir),
                msg: format!(
                    "god folder: {}% of total complexity lives here",
                    crate::fmt::py_fmt1(god.share)
                ),
            });
        }
    }
    for p in parse_errors.iter().take(10) {
        w.push(Warning {
            severity: "WATCH",
            r#where: p.clone(),
            msg: "parse error \u{2014} CXCAP fell back to line counts here".to_string(),
        });
    }
    w.sort_by(|a, b| {
        let oa = if a.severity == "HIGH" { 0 } else { 1 };
        let ob = if b.severity == "HIGH" { 0 } else { 1 };
        (oa, &a.r#where).cmp(&(ob, &b.r#where))
    });
    w
}

/// General-purpose languages outside the analyzer's scope. When these
/// dominate a repo, a verdict over the scored minority would mislead, so
/// the verdict is withheld (local signals still report). Scripts (sh),
/// IDL (proto), and query files (sql) are deliberately excluded: they are
/// disclosed as unscored without forcing N/A.
pub(crate) const UNSCORED_LANGS: &[&str] = &[
    ".go", ".java", ".c", ".h", ".hh", ".hpp", ".hxx", ".cc", ".cpp", ".cxx", ".cs", ".rb", ".php",
    ".swift", ".kt", ".kts", ".scala", ".lua", ".pl", ".pm", ".r", ".jl", ".dart", ".elm", ".erl",
    ".ex", ".exs", ".hs", ".ml", ".mli", ".zig", ".nim", ".m", ".mm", ".vb",
];

pub fn build_verdict(
    n_code: usize,
    n_scored_total: usize,
    cx_total: u32,
    warnings: &[Warning],
    folders: &[FolderRow],
    has_cycle: bool,
    unscored_top: Option<(&str, usize)>,
) -> (String, String) {
    if n_code == 0 {
        return (
            "N/A".to_string(),
            "no Python/JS/TS/SFC/Rust code files found \u{2014} nothing was scored".to_string(),
        );
    }
    // Coverage honesty: a verdict over a scored sliver misleads when an
    // unscored language dominates (10x structural ratio against ALL scored
    // files — production and verification surface alike — same family as
    // the god-candidate rule: a guard, not a sensitivity knob).
    if let Some((ext, n)) = unscored_top {
        if UNSCORED_LANGS.contains(&ext) && n > 10 * n_scored_total {
            return (
                "N/A".to_string(),
                format!(
                    "dominated by unscored {ext} ({n} files); scored {n_scored_total} \u{2014} audit says nothing about the unscored majority"
                ),
            );
        }
    }
    let highs = warnings.iter().filter(|x| x.severity == "HIGH").count();
    let avg_cx = cx_total as f64 / n_code as f64;
    let god = if n_code >= 10 {
        god_candidate(folders)
    } else {
        None
    };
    let top_share = god.map(|g| g.share).unwrap_or(0.0);
    // Name the concentration only when a folder actually qualifies; SEVERE
    // reached through HIGH warnings or average complexity has none to name.
    let concentration = god
        .map(|g| format!("; concentration in {}/", g.dir))
        .unwrap_or_default();
    if highs >= 8 || avg_cx > 60.0 || top_share > 60.0 {
        (
            "SEVERE".to_string(),
            format!(
                "{}; avg complexity/file {}{concentration}",
                crate::fmt::n1(highs, "HIGH warning"),
                crate::fmt::py_fmt1(avg_cx)
            ),
        )
    } else if highs >= 3 || avg_cx > 30.0 || top_share > 40.0 {
        (
            "HIGH".to_string(),
            format!("{}; avg complexity/file {}", crate::fmt::n1(highs, "HIGH warning"), crate::fmt::py_fmt1(avg_cx)),
        )
    // Any HIGH warning floors the verdict: LOW guidance says PROCEED, which
    // contradicts telling the reader a specific file needs caution. The same
    // holds for import-cycle warnings of any severity: a lazy cycle ("do
    // not make cross-imports eager") or a large tangle ("treat the area as
    // one") both contradict PROCEED's "prefer the straightforward change".
    } else if highs >= 1 || has_cycle || warnings.len() >= 5 || avg_cx > 12.0 {
        (
            "MODERATE".to_string(),
            format!("{}; avg complexity/file {}", crate::fmt::n1(warnings.len(), "warning"), crate::fmt::py_fmt1(avg_cx)),
        )
    } else {
        (
            "LOW".to_string(),
            format!("avg complexity/file {}; {}", crate::fmt::py_fmt1(avg_cx), crate::fmt::n1(warnings.len(), "warning")),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(dir: &str, share: f64) -> FolderRow {
        FolderRow { dir: dir.to_string(), complexity: 0, loc: 0, share }
    }

    fn high(r#where: &str) -> Warning {
        Warning { severity: "HIGH", r#where: r#where.to_string(), msg: String::new() }
    }

    #[test]
    fn severe_without_concentration_names_no_folder() {
        let warnings: Vec<Warning> = (0..8).map(|i| high(&format!("src/f{i}.rs"))).collect();
        let (verdict, why) = build_verdict(27, 33, 1603, &warnings, &[folder("src", 100.0)], false, None);
        assert_eq!(verdict, "SEVERE");
        assert!(!why.contains("concentration"), "{why}");
        assert!(!why.contains('\u{2014}'), "{why}");
    }

    #[test]
    fn severe_with_concentration_names_the_folder() {
        let folders = [folder("src/core", 70.0), folder("src/cli", 20.0)];
        let (verdict, why) = build_verdict(12, 12, 100, &[], &folders, false, None);
        assert_eq!(verdict, "SEVERE");
        assert!(why.ends_with("; concentration in src/core/"), "{why}");
    }
}
