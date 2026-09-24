//! Descriptive context/attention burden.
//!
//! Pure summary over already-collected data: how much distinct relevant
//! information must be understood together before a change. Deliberately
//! descriptive (counts + estimates), never a scalar "context score".
//! Token estimates use LOC*8 and are labeled estimates, not measurements.

use crate::component::component_of;
use std::collections::{HashMap, HashSet};

/// Approx source tokens per line for estimates. Documented heuristic.
pub const TOKENS_PER_LOC: u64 = 8;

#[derive(Debug, Clone, PartialEq)]
pub struct ContextSurface {
    pub prod_files: usize,
    pub prod_tokens_est: u64,
    pub test_files: usize,
    pub test_tokens_est: u64,
    pub components: usize,
    pub cross_boundaries: usize,
    pub cycles_involved: usize,
    /// Share of ranked complexity in the top-4 files (0-100),if computable.
    pub top4_share_pct: Option<f64>,
    /// Human + agent readable lines, e.g. "9 production files".
    pub lines: Vec<String>,
}

/// Build the surface for an expanded candidate set.
/// `loc`: path -> (loc, kind, complexity). `edges`: (importer, target).
/// `cycles`: file cycles; count those touching the set.
pub fn summarize(
    set: &[String],
    loc: &HashMap<String, (u32, String, u32)>,
    edges: &[(String, String)],
    cycles: &[Vec<String>],
) -> ContextSurface {
    let in_set: HashSet<&str> = set.iter().map(|s| s.as_str()).collect();
    let mut prod = 0usize;
    let mut prod_loc = 0u64;
    let mut test = 0usize;
    let mut test_loc = 0u64;
    let mut cx: Vec<u32> = Vec::new();
    for p in set {
        if let Some((l, k, c)) = loc.get(p) {
            cx.push(*c);
            if k == "test" {
                test += 1;
                test_loc += *l as u64;
            } else {
                prod += 1;
                prod_loc += *l as u64;
            }
        } else {
            prod += 1;
        }
    }
    let comps: HashSet<String> = set.iter().map(|p| component_of(p)).collect();
    let cross = edges
        .iter()
        .filter(|(a, b)| in_set.contains(a.as_str()) && in_set.contains(b.as_str()))
        .filter(|(a, b)| component_of(a) != component_of(b))
        .count();
    let cyc = cycles
        .iter()
        .filter(|c| c.iter().any(|f| in_set.contains(f.as_str())))
        .count();
    cx.sort_by(|a, b| b.cmp(a));
    let top4_share_pct = if cx.len() >= 2 && cx.iter().sum::<u32>() > 0 {
        let tot = cx.iter().sum::<u32>() as f64;
        let top: u32 = cx.iter().take(4).sum();
        Some((top as f64 * 100.0 / tot * 10.0).round() / 10.0)
    } else {
        None
    };
    let mut lines = vec![
        format!("{prod} production files (~{} source tokens est.)", prod_loc * TOKENS_PER_LOC),
        format!("{test} verification files (~{} tokens est.)", test_loc * TOKENS_PER_LOC),
        format!(
            "{}, {}, {}",
            crate::fmt::n1(comps.len(), "component"),
            crate::fmt::n1(cross, "cross-boundary edge"),
            crate::fmt::n1(cyc, "cycle")
        ),
    ];
    if let Some(pct) = top4_share_pct {
        lines.push(format!("top 4 files hold {pct}% of ranked complexity"));
    }
    ContextSurface {
        prod_files: prod,
        prod_tokens_est: prod_loc * TOKENS_PER_LOC,
        test_files: test,
        test_tokens_est: test_loc * TOKENS_PER_LOC,
        components: comps.len(),
        cross_boundaries: cross,
        cycles_involved: cyc,
        top4_share_pct,
        lines,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn loc(entries: &[(&str, u32, &str, u32)]) -> HashMap<String, (u32, String, u32)> {
        entries.iter().map(|(p, l, k, c)| (p.to_string(), (*l, k.to_string(), *c))).collect()
    }

    #[test]
    fn localized_vs_spanning_distinguished() {
        let small = vec!["auth/a.ts".to_string(), "auth/b.ts".to_string()];
        let l = loc(&[("auth/a.ts", 100, "js", 20), ("auth/b.ts", 50, "js", 10)]);
        let s = summarize(&small, &l, &[], &[]);
        assert_eq!(s.prod_files, 2);
        assert_eq!(s.components, 1);
        assert_eq!(s.cross_boundaries, 0);
        assert_eq!(s.cycles_involved, 0);
        // No scalar score field exists: descriptive only.
        assert!(s.lines.len() >= 3);

        let big: Vec<String> = (0..9).map(|i| format!("p{i}/f.ts")).collect();
        let entries: Vec<(String, u32, String, u32)> =
            big.iter().map(|p| (p.clone(), 100, "js".to_string(), 10)).collect();
        let l2: HashMap<String, (u32, String, u32)> = entries
            .iter()
            .map(|(p, a, b, c)| (p.clone(), (*a, b.clone(), *c)))
            .collect();
        let e: Vec<(String, String)> = vec![("p0/f.ts".into(), "p1/f.ts".into())];
        let cyc = vec![vec!["p0/f.ts".to_string(), "p1/f.ts".to_string()]];
        let s2 = summarize(&big, &l2, &e, &cyc);
        assert_eq!(s2.prod_files, 9);
        assert_eq!(s2.components, 9);
        assert_eq!(s2.cross_boundaries, 1);
        assert_eq!(s2.cycles_involved, 1);
        assert!(s2.prod_tokens_est > s.prod_tokens_est);
    }

    #[test]
    fn verification_split_and_top_share() {
        let set = vec!["a.ts".to_string(), "a.test.ts".to_string()];
        let l = loc(&[("a.ts", 100, "js", 80), ("a.test.ts", 200, "test", 20)]);
        let s = summarize(&set, &l, &[], &[]);
        assert_eq!(s.prod_files, 1);
        assert_eq!(s.test_files, 1);
        assert_eq!(s.top4_share_pct, Some(100.0));
    }

    #[test]
    fn deterministic_lines() {
        let set = vec!["b.ts".to_string(), "a.ts".to_string()];
        let l = loc(&[("a.ts", 10, "js", 5), ("b.ts", 10, "js", 5)]);
        let a = summarize(&set, &l, &[], &[]);
        let b = summarize(&set, &l, &[], &[]);
        assert_eq!(a, b);
    }
}
