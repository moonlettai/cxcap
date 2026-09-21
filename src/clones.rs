//! Copy-paste (clone) detection over normalized content windows.
//!
//! Only cross-file sharing counts; same-stem language mirrors (ports, not
//! drift) are excluded. Window length must match the reference.

use crate::resolve::{slash, stem_of};
use std::collections::{HashMap, HashSet};

/// Copy-paste window length (must match the reference).
pub const CLONE_MIN_LINES: usize = 8;

#[derive(Debug, Default)]
pub struct CloneResult {
    /// Kept lines covered by cross-file-shared windows, per path.
    pub dup_lines: HashMap<String, usize>,
    /// (path_a, path_b, shared windows), count desc, ties by first-seen —
    /// mirroring Counter.most_common's stability. Same-stem mirrors
    /// (language ports, not drift) are excluded.
    pub pairs: Vec<(String, String, usize)>,
}

/// Only cross-file sharing counts. `input` maps path to (window hashes,
/// kept-line count); the window index is the kept-line start.
pub fn find_clones(input: &HashMap<String, (Vec<String>, usize)>) -> CloneResult {
    let mut by_hash: HashMap<&str, Vec<(&str, usize)>> = HashMap::new();
    let mut hash_order: Vec<String> = Vec::new();
    // Deterministic input order (sorted paths): the reference iterates an
    // insertion-ordered dict fed by walk order, so ITS order is arbitrary.
    // Sorted order keeps this implementation deterministic; the harness
    // compares tie groups as multisets.
    let mut paths: Vec<&String> = input.keys().collect();
    paths.sort();
    for path in paths {
        let (windows, _) = &input[path];
        for (wi, h) in windows.iter().enumerate() {
            by_hash
                .entry(h.as_str())
                .or_insert_with(|| {
                    hash_order.push(h.clone());
                    Vec::new()
                })
                .push((path.as_str(), wi));
        }
    }
    let mut covered: HashMap<&str, HashSet<usize>> = HashMap::new();
    // (count, first_seen_seq) per pair; most_common is stable over first
    // occurrence, which (count desc, seq asc) reproduces exactly.
    let mut pair_windows: HashMap<(String, String), (usize, usize)> = HashMap::new();
    let mut seq = 0usize;
    for h in &hash_order {
        let locs = &by_hash[h.as_str()];
        let mut ps: Vec<&str> = locs.iter().map(|(p, _)| *p).collect();
        ps.sort();
        ps.dedup();
        if ps.len() < 2 {
            continue;
        }
        for (p, i) in locs {
            covered.entry(p).or_default().extend(*i..*i + CLONE_MIN_LINES);
        }
        for x in 0..ps.len() {
            for y in x + 1..ps.len() {
                let key = (ps[x].to_string(), ps[y].to_string());
                let e = pair_windows.entry(key).or_insert_with(|| {
                    let s = seq;
                    seq += 1;
                    (0usize, s)
                });
                e.0 += 1;
            }
        }
    }
    let mut out = CloneResult::default();
    for (p, s) in &covered {
        out.dup_lines.insert(p.to_string(), s.len());
    }
    let mut ranked: Vec<((String, String), (usize, usize))> = pair_windows.into_iter().collect();
    ranked.sort_by(|a, b| b.1 .0.cmp(&a.1 .0).then_with(|| a.1 .1.cmp(&b.1 .1)));
    for ((a, b), (n, _)) in ranked {
        // Same-stem mirrors (mentions.jsx <-> mentions.tsx) are language
        // ports, not copy-paste drift - uninteresting by construction.
        let aa = slash(&a);
        let bb = slash(&b);
        let sa = aa.rsplit('/').next().unwrap_or(aa.as_str());
        let sb = bb.rsplit('/').next().unwrap_or(bb.as_str());
        if stem_of(sa) == stem_of(sb) {
            continue;
        }
        out.pairs.push((a, b, n));
    }
    out
}
