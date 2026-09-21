//! Component/package architecture signals.
//!
//! Derives components generically as top-level directory segments
//! (root files group as "(root)"). No manifests, no repo-specific rules.
//! Pure function over already-resolved file edges: no new I/O.
//! Only flags coordination cost across boundaries (bidirectional links,
//! multi-component cycles) — never punishes boundary count or volume.

use std::collections::{HashMap, HashSet};

/// Component of a repo-relative path: first segment, or "(root)".
pub fn component_of(path: &str) -> String {
    match path.split_once('/') {
        Some((head, _)) => head.to_string(),
        None => "(root)".to_string(),
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ComponentStat {
    pub name: String,
    pub files: usize,
    pub internal_edges: usize,
    pub out_cross: usize,
    pub in_cross: usize,
    /// Components with edges in BOTH directions (coordination cost).
    pub bidirectional: Vec<String>,
    /// True only when bidirectional or in a multi-component cycle.
    pub tangle: bool,
}

/// Summarize cross-boundary structure. `files`: repo-relative paths.
/// `edges`: (importer, target) internal edges. `cycles`: file cycles from
/// relate::find_cycles over the same edges.
pub fn summarize(
    files: &[String],
    edges: &[(String, String)],
    cycles: &[Vec<String>],
) -> Vec<ComponentStat> {
    let mut members: HashMap<String, usize> = HashMap::new();
    for f in files {
        *members.entry(component_of(f)).or_insert(0) += 1;
    }
    let mut internal: HashMap<String, usize> = HashMap::new();
    let mut out_map: HashMap<(String, String), usize> = HashMap::new();
    for (a, b) in edges {
        let ca = component_of(a);
        let cb = component_of(b);
        if ca == cb {
            *internal.entry(ca).or_insert(0) += 1;
        } else {
            *out_map.entry((ca, cb)).or_insert(0) += 1;
        }
    }
    // Bidirectional pairs.
    let mut bidir: HashMap<String, HashSet<String>> = HashMap::new();
    let pairs: HashSet<(String, String)> = out_map.keys().cloned().collect();
    for (a, b) in pairs.iter() {
        if pairs.contains(&(b.clone(), a.clone())) {
            bidir.entry(a.clone()).or_default().insert(b.clone());
        }
    }
    // Components sharing a multi-component file cycle.
    let mut in_cycle: HashSet<String> = HashSet::new();
    for c in cycles {
        let comps: HashSet<String> = c.iter().map(|f| component_of(f)).collect();
        if comps.len() > 1 {
            for k in comps {
                in_cycle.insert(k);
            }
        }
    }
    let mut names: Vec<String> = members.keys().cloned().collect();
    names.sort();
    names
        .into_iter()
        .map(|n| {
            let out_cross: usize = out_map.iter().filter(|((a, _), _)| a == &n).map(|(_, c)| *c).sum();
            let in_cross: usize = out_map.iter().filter(|((_, b), _)| b == &n).map(|(_, c)| *c).sum();
            let mut bd: Vec<String> = bidir.get(&n).map(|s| s.iter().cloned().collect()).unwrap_or_default();
            bd.sort();
            let tangle = !bd.is_empty() || in_cycle.contains(&n);
            ComponentStat {
                files: members[&n],
                internal_edges: internal.get(&n).copied().unwrap_or(0),
                out_cross,
                in_cross,
                bidirectional: bd,
                tangle,
                name: n,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::relate;

    fn files(ps: &[&str]) -> Vec<String> {
        ps.iter().map(|s| s.to_string()).collect()
    }

    fn edges(ps: &[(&str, &str)]) -> Vec<(String, String)> {
        ps.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect()
    }

    fn cycles_for(e: &[(String, String)]) -> Vec<Vec<String>> {
        relate::find_cycles(e)
    }

    #[test]
    fn independent_modules_not_punished() {
        let f = files(&["a/x.ts", "b/y.ts", "c/z.ts", "d/w.ts"]);
        let e = edges(&[]);
        let s = summarize(&f, &e, &cycles_for(&e));
        assert!(s.iter().all(|c| !c.tangle));
        assert_eq!(s.len(), 4);
    }

    #[test]
    fn clean_layered_not_flagged() {
        let f = files(&["api/a.ts", "platform/b.ts", "auth/c.ts"]);
        let e = edges(&[("api/a.ts", "platform/b.ts"), ("platform/b.ts", "auth/c.ts")]);
        let s = summarize(&f, &e, &cycles_for(&e));
        assert!(s.iter().all(|c| !c.tangle));
    }

    #[test]
    fn bidirectional_pair_flagged() {
        let f = files(&["a/x.ts", "b/y.ts"]);
        let e = edges(&[("a/x.ts", "b/y.ts"), ("b/y.ts", "a/x.ts")]);
        let s = summarize(&f, &e, &cycles_for(&e));
        let get = |n: &str| s.iter().find(|c| c.name == n).unwrap().clone();
        assert!(get("a").tangle && get("b").tangle);
        assert_eq!(get("a").bidirectional, vec!["b".to_string()]);
    }

    #[test]
    fn ring_cycle_flagged_chain_clean() {
        // Ring a->b->c->a: file cycle spans 3 components.
        let f = files(&["a/x.ts", "b/y.ts", "c/z.ts"]);
        let ring = edges(&[("a/x.ts", "b/y.ts"), ("b/y.ts", "c/z.ts"), ("c/z.ts", "a/x.ts")]);
        let s = summarize(&f, &ring, &cycles_for(&ring));
        assert!(s.iter().all(|c| c.tangle));
        // Chain a->b->c: no cycle, one direction.
        let chain = edges(&[("a/x.ts", "b/y.ts"), ("b/y.ts", "c/z.ts")]);
        let s2 = summarize(&f, &chain, &cycles_for(&chain));
        assert!(s2.iter().all(|c| !c.tangle));
    }

    #[test]
    fn star_and_diamond_clean() {
        let f = files(&["hub/h.ts", "l1/a.ts", "l2/b.ts", "l3/c.ts"]);
        let star = edges(&[
            ("l1/a.ts", "hub/h.ts"),
            ("l2/b.ts", "hub/h.ts"),
            ("l3/c.ts", "hub/h.ts"),
        ]);
        assert!(summarize(&f, &star, &cycles_for(&star)).iter().all(|c| !c.tangle));
        let f2 = files(&["top/a.ts", "l/b.ts", "r/c.ts", "bot/d.ts"]);
        let diamond = edges(&[
            ("top/a.ts", "l/b.ts"),
            ("top/a.ts", "r/c.ts"),
            ("l/b.ts", "bot/d.ts"),
            ("r/c.ts", "bot/d.ts"),
        ]);
        assert!(summarize(&f2, &diamond, &cycles_for(&diamond)).iter().all(|c| !c.tangle));
    }

    #[test]
    fn stable_core_not_tangle_unstable_not_cycle() {
        // Many leaves depend on core: high inbound, no bidir -> not a tangle.
        let f = files(&["core/c.ts", "l1/a.ts", "l2/b.ts", "l3/d.ts"]);
        let e = edges(&[
            ("l1/a.ts", "core/c.ts"),
            ("l2/b.ts", "core/c.ts"),
            ("l3/d.ts", "core/c.ts"),
        ]);
        let s = summarize(&f, &e, &cycles_for(&e));
        let core = s.iter().find(|c| c.name == "core").unwrap();
        assert!(!core.tangle);
        assert_eq!(core.in_cross, 3);
        // Unstable: one component depends on everything, no return edges.
        let f2 = files(&["u/x.ts", "a/a.ts", "b/b.ts"]);
        let e2 = edges(&[("u/x.ts", "a/a.ts"), ("u/x.ts", "b/b.ts")]);
        let s2 = summarize(&f2, &e2, &cycles_for(&e2));
        let u = s2.iter().find(|c| c.name == "u").unwrap();
        assert!(!u.tangle);
        assert_eq!(u.out_cross, 2);
    }

    #[test]
    fn internal_complexity_clean_contract() {
        // Dense inside one component, single clean outbound edge.
        let f = files(&["pkg/a.ts", "pkg/b.ts", "pkg/c.ts", "api/x.ts"]);
        let e = edges(&[
            ("pkg/a.ts", "pkg/b.ts"),
            ("pkg/b.ts", "pkg/c.ts"),
            ("pkg/c.ts", "pkg/a.ts"),
            ("api/x.ts", "pkg/a.ts"),
        ]);
        let s = summarize(&f, &e, &cycles_for(&e));
        // Internal file cycle stays inside pkg: no cross-component tangle.
        assert!(s.iter().all(|c| !c.tangle));
        assert_eq!(s.iter().find(|c| c.name == "pkg").unwrap().internal_edges, 3);
    }
}
