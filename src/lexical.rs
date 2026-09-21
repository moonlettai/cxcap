//! Ephemeral lexical intent retrieval.
//!
//! Maps natural-language change intent -> likely files using only data CXCAP
//! already collects (repo-relative paths + symbol names from FileRec).
//! No new I/O, no persistent index: the inverted index is built per
//! invocation in memory and discarded. Only production code kinds
//! (python/js/rust) are ranked; test/generated surfaces are excluded by
//! construction (they carry kind == "test").

use crate::scan::FileRec;
use std::collections::HashMap;

/// Split identifier into lowercase tokens: snake_case, kebab, dots, slashes,
/// plus camelCase / PascalCase boundaries. Numbers kept as tokens.
pub fn tokenize(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    // First split on non-alphanumeric boundaries (paths, extensions, etc.)
    for raw in text.split(|c: char| !(c.is_alphanumeric())) {
        if raw.is_empty() {
            continue;
        }
        // Then split camelCase boundaries: lower->Upper or acronym->Word.
        let chars: Vec<char> = raw.chars().collect();
        let mut start = 0;
        for i in 1..chars.len() {
            let prev = chars[i - 1];
            let cur = chars[i];
            let boundary = (prev.is_lowercase() && cur.is_uppercase())
                || (prev.is_uppercase()
                    && cur.is_uppercase()
                    && i + 1 < chars.len()
                    && chars[i + 1].is_lowercase())
                || (prev.is_alphabetic() && cur.is_numeric())
                || (prev.is_numeric() && cur.is_alphabetic());
            if boundary {
                let tok: String = chars[start..i].iter().collect();
                if !tok.is_empty() {
                    out.push(stem(tok.to_lowercase()));
                }
                start = i;
            }
        }
        let tok: String = chars[start..].iter().collect();
        if !tok.is_empty() {
            out.push(stem(tok.to_lowercase()));
        }
    }
    out
}

/// Minimal generic singular normalization: strip a single trailing 's'
/// (cycles->cycle) so plural/singular intent wording still localizes.
/// 'ss' endings (class) and short tokens are left alone. Applied to both
/// documents and queries, so it cannot create asymmetric matches.
fn stem(tok: String) -> String {
    if tok.len() > 3 && tok.ends_with('s') && !tok.ends_with("ss") {
        tok[..tok.len() - 1].to_string()
    } else {
        tok
    }
}

/// Document text for one file: path segments + symbol names.
/// Deliberately cheap: no file re-read, no comments/docstrings yet.
/// `func_names` is populated exactly when the file defines functions
/// (all analyzers push it alongside func details), so no fallback is
/// needed: function-less files rank on path tokens alone.
pub fn doc_tokens(rec: &FileRec) -> Vec<String> {
    let mut toks = tokenize(&rec.path);
    if let Some(names) = &rec.func_names {
        for n in names {
            toks.extend(tokenize(n));
        }
    }
    toks
}

const K1: f64 = 1.2;
const B: f64 = 0.75;

/// BM25 score of one document's tokens against the query terms.
fn score_doc(qtoks: &[String], toks: &[String], df: &HashMap<&str, usize>, n: f64, avgdl: f64) -> f64 {
    let dl = toks.len() as f64;
    let mut tf: HashMap<&str, usize> = HashMap::new();
    for t in toks {
        *tf.entry(t.as_str()).or_insert(0) += 1;
    }
    let mut score = 0.0;
    for term in qtoks {
        let f = *tf.get(term.as_str()).unwrap_or(&0) as f64;
        if f == 0.0 {
            continue;
        }
        let doc_freq = *df.get(term.as_str()).unwrap_or(&0) as f64;
        // Standard BM25 IDF, floored at 0 to avoid negative generic-word weight.
        let idf = ((n - doc_freq + 0.5) / (doc_freq + 0.5) + 1.0).ln();
        let denom = f + K1 * (1.0 - B + B * dl / avgdl.max(1.0));
        score += idf * f * (K1 + 1.0) / denom;
    }
    score
}

/// BM25 rank of candidate files for an intent string.
/// Returns (path, score) descending, deterministic ties by path.
pub fn rank(intent: &str, files: &[FileRec], top_k: usize) -> Vec<(String, f64)> {
    let q = tokenize(intent);
    if q.is_empty() {
        return Vec::new();
    }
    // Build ephemeral corpus: production code only (test/generated
    // surfaces carry kind == "test" and are excluded by construction).
    let docs: Vec<(&FileRec, Vec<String>)> = files
        .iter()
        .filter(|r| crate::scan::is_prod_kind(r.kind))
        .map(|r| (r, doc_tokens(r)))
        .collect();
    if docs.is_empty() {
        return Vec::new();
    }
    let n = docs.len() as f64;
    let avgdl: f64 =
        docs.iter().map(|(_, t)| t.len() as f64).sum::<f64>() / n.max(1.0);
    // Document frequency per query term.
    let mut df: HashMap<&str, usize> = HashMap::new();
    for term in q.iter() {
        if df.contains_key(term.as_str()) {
            continue;
        }
        let count = docs
            .iter()
            .filter(|(_, t)| t.contains(term))
            .count();
        df.insert(term.as_str(), count);
    }
    let mut scored: Vec<(String, f64)> = Vec::new();
    for (rec, toks) in docs.iter() {
        let score = score_doc(&q, toks, &df, n, avgdl);
        if score > 0.0 {
            scored.push((rec.path.clone(), score));
        }
    }
    scored.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap()
            .then_with(|| a.0.cmp(&b.0))
    });
    scored.truncate(top_k);
    scored
}

/// Exp 2: expand lexical seeds along existing dependency edges.
/// `edges` are (importer, target) pairs from relate::attribute_coupling.
/// Walks both directions (dependencies for context, dependents for exposure),
/// BFS up to `hops`, deterministic order, hard cap prevents generic-word
/// blowup from swallowing the repo. Seeds first, then by (depth, path).
pub fn expand_seeds(
    seeds: &[String],
    edges: &[(String, String)],
    hops: usize,
    cap: usize,
) -> Vec<String> {
    use std::collections::{HashMap, HashSet, VecDeque};
    let mut fwd: HashMap<&str, Vec<&str>> = HashMap::new();
    let mut rev: HashMap<&str, Vec<&str>> = HashMap::new();
    for (a, b) in edges {
        fwd.entry(a.as_str()).or_default().push(b.as_str());
        rev.entry(b.as_str()).or_default().push(a.as_str());
    }
    let mut seen: HashSet<String> = HashSet::new();
    let mut depth: HashMap<String, usize> = HashMap::new();
    let mut q: VecDeque<String> = VecDeque::new();
    for s in seeds {
        if seen.insert(s.clone()) {
            depth.insert(s.clone(), 0);
            q.push_back(s.clone());
        }
    }
    while let Some(cur) = q.pop_front() {
        let d = *depth.get(&cur).unwrap_or(&0);
        if d >= hops {
            continue;
        }
        let mut next: Vec<String> = Vec::new();
        for n in fwd.get(cur.as_str()).into_iter().flatten() {
            next.push(n.to_string());
        }
        for n in rev.get(cur.as_str()).into_iter().flatten() {
            next.push(n.to_string());
        }
        next.sort();
        next.dedup();
        for n in next {
            if seen.insert(n.clone()) {
                depth.insert(n.clone(), d + 1);
                q.push_back(n);
                if seen.len() >= cap {
                    break;
                }
            }
        }
        if seen.len() >= cap {
            break;
        }
    }
    let mut out: Vec<String> = seen.into_iter().collect();
    out.sort_by(|a, b| {
        depth
            .get(a)
            .cmp(&depth.get(b))
            .then_with(|| a.cmp(b))
    });
    out.truncate(cap);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::py::PyFunc;

    fn rec(path: &str, kind: &str, funcs: &[&str]) -> FileRec {
        FileRec {
            path: path.to_string(),
            bytes: 100,
            loc: 50,
            kind: Box::leak(kind.to_string().into_boxed_str()),
            skipped: None,
            funcs: funcs.len(),
            classes: 0,
            complexity: 10,
            max_nesting: 2,
            fan_out_in: 0,
            fan_out_ex: 0,
            max_func_cx: None,
            max_params: None,
            imports: None,
            type_only_imports: None,
            top_funcs: Some(
                funcs
                    .iter()
                    .map(|n| PyFunc {
                        name: n.to_string(),
                        lineno: 1,
                        end: 5,
                        params: 1,
                        complexity: 2,
                        length: 5,
                    })
                    .collect(),
            ),
            func_names: Some(funcs.iter().map(|n| n.to_string()).collect()),
            parse_error: None,
            fan_in: None,
            coupling: None,
            dup_lines: None,
            hotspot: None,
        }
    }

    #[test]
    fn tokenize_splits_cases() {
        assert!(tokenize("SessionConfig").contains(&"session".to_string()));
        assert!(tokenize("SessionConfig").contains(&"config".to_string()));
        assert!(tokenize("auth_session").contains(&"auth".to_string()));
        assert!(tokenize("auth/session.ts").contains(&"auth".to_string()));
    }

    #[test]
    fn intent_in_symbol_found() {
        let files = vec![
            rec("auth/session.ts", "js", &["createSession"]),
            rec("util/format.ts", "js", &["formatDate"]),
        ];
        let r = rank("session", &files, 5);
        assert_eq!(r[0].0, "auth/session.ts");
    }

    #[test]
    fn generic_word_does_not_dominate() {
        // "util" appears everywhere-ish; specific term should still win.
        let files = vec![
            rec("util/a.ts", "js", &["helper"]),
            rec("util/b.ts", "js", &["helper"]),
            rec("auth/session.ts", "js", &["createSession"]),
        ];
        let r = rank("session", &files, 5);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].0, "auth/session.ts");
    }

    #[test]
    fn test_surface_excluded() {
        let files = vec![
            rec("auth/session.test.ts", "test", &["sessionWorks"]),
            rec("auth/session.ts", "js", &["createSession"]),
        ];
        let r = rank("session", &files, 5);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].0, "auth/session.ts");
    }

    #[test]
    fn multiple_areas_sharing_token_all_returned() {
        let files = vec![
            rec("auth/session.ts", "js", &["openSession"]),
            rec("db/session.ts", "js", &["dbSession"]),
            rec("util/x.ts", "js", &["other"]),
        ];
        let r = rank("session", &files, 5);
        assert_eq!(r.len(), 2);
    }

    #[test]
    fn expand_pulls_dependent_but_not_unrelated() {
        // service imports session; util is isolated.
        let edges = vec![("auth/service.ts".to_string(), "auth/session.ts".to_string())];
        let out = expand_seeds(&["auth/session.ts".to_string()], &edges, 1, 20);
        assert!(out.contains(&"auth/session.ts".to_string()));
        assert!(out.contains(&"auth/service.ts".to_string()));
        assert!(!out.contains(&"util/string.ts".to_string()));
    }

    #[test]
    fn expand_respects_hops_and_cap() {
        // chain a->b->c; 1 hop must not reach c; cap bounds blowup.
        let edges = vec![
            ("a".to_string(), "b".to_string()),
            ("b".to_string(), "c".to_string()),
        ];
        let one = expand_seeds(&["a".to_string()], &edges, 1, 20);
        assert!(one.contains(&"b".to_string()));
        assert!(!one.contains(&"c".to_string()));
        let two = expand_seeds(&["a".to_string()], &edges, 2, 20);
        assert!(two.contains(&"c".to_string()));
        let capped = expand_seeds(&["a".to_string()], &edges, 2, 2);
        assert_eq!(capped.len(), 2);
    }

    #[test]
    fn expand_is_deterministic() {
        let edges = vec![
            ("b".to_string(), "a".to_string()),
            ("c".to_string(), "a".to_string()),
        ];
        let x = expand_seeds(&["a".to_string()], &edges, 1, 20);
        let y = expand_seeds(&["a".to_string()], &edges, 1, 20);
        assert_eq!(x, y);
    }
}
