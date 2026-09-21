//! Import-cycle detection via iterative Kosaraju SCC.
//!
//! Sorted member lists (size>1 or self-loop), sorted overall — mirroring
//! the reference exactly. Small eager cycles are actionable; large and
//! lazy tangles are context.

use std::collections::{HashMap, HashSet};

/// Import cycles via iterative Kosaraju SCC. Sorted member lists (size>1 or
/// self-loop), sorted overall — mirroring the reference exactly.
pub fn find_cycles(edges: &[(String, String)]) -> Vec<Vec<String>> {
    fn sorted_neighbors(graph: &HashMap<String, HashSet<String>>, n: &str) -> Vec<String> {
        let mut v: Vec<String> = graph.get(n).map(|s| s.iter().cloned().collect()).unwrap_or_default();
        v.sort();
        v
    }
    // Iterative DFS mirroring the reference's explicit-stack walk.
    fn walk(
        start: &str,
        graph: &HashMap<String, HashSet<String>>,
        record: bool,
        seen: &mut HashSet<String>,
        order: &mut Vec<String>,
        added: &mut Vec<String>,
    ) {
        let mut stack: Vec<(String, Vec<String>, usize)> =
            vec![(start.to_string(), sorted_neighbors(graph, start), 0)];
        seen.insert(start.to_string());
        added.push(start.to_string());
        while let Some(top) = stack.last_mut() {
            let mut descended = false;
            while top.2 < top.1.len() {
                let nxt = top.1[top.2].clone();
                top.2 += 1;
                if !seen.contains(&nxt) {
                    seen.insert(nxt.clone());
                    added.push(nxt.clone());
                    let nn = sorted_neighbors(graph, &nxt);
                    stack.push((nxt, nn, 0));
                    descended = true;
                    break;
                }
            }
            if !descended {
                let (node, _, _) = stack.pop().unwrap();
                if record {
                    order.push(node);
                }
            }
        }
    }

    let mut adj: HashMap<String, HashSet<String>> = HashMap::new();
    let mut radj: HashMap<String, HashSet<String>> = HashMap::new();
    let mut nodes: HashSet<String> = HashSet::new();
    for (a, b) in edges {
        adj.entry(a.clone()).or_default().insert(b.clone());
        radj.entry(b.clone()).or_default().insert(a.clone());
        nodes.insert(a.clone());
        nodes.insert(b.clone());
    }
    let mut seen: HashSet<String> = HashSet::new();
    let mut order: Vec<String> = Vec::new();
    let mut added: Vec<String> = Vec::new();
    let mut snodes: Vec<String> = nodes.iter().cloned().collect();
    snodes.sort();
    for n in &snodes {
        if !seen.contains(n) {
            walk(n, &adj, true, &mut seen, &mut order, &mut added);
        }
    }
    seen.clear();
    let mut found: Vec<Vec<String>> = Vec::new();
    let mut claimed: HashSet<String> = HashSet::new();
    let mut dummy: Vec<String> = Vec::new();
    for n in order.iter().rev() {
        if seen.contains(n) {
            continue;
        }
        let mut members: Vec<String> = Vec::new();
        walk(n, &radj, false, &mut seen, &mut dummy, &mut members);
        members.sort();
        if members.len() > 1 {
            claimed.extend(members.iter().cloned());
            found.push(members);
        }
    }
    for n in &snodes {
        // self-loops
        if adj
            .get(n)
            .map(|s| s.contains(n))
            .unwrap_or(false)
            && !claimed.contains(n)
        {
            found.push(vec![n.clone()]);
        }
    }
    found.sort();
    found
}
