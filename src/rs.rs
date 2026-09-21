//! Rust parse + metrics (tree-sitter AST, same economics as Python).
//!
//! Branch set mirrors Python deliberately: `if` (+ chains), `match` arms,
//! `for`/`while`/`loop`, `?` (like `?.`), `&&`/`||` (operands − 1). Guards
//! and or-patterns add no branch beyond their operators (Python parity,
//! documented). Closures count as functions with bodies (JS-arrow parity:
//! Rust's iterator/callback style makes them first-class dense units,
//! unlike Python lambdas which the reference never tracked). Bodyless
//! declarations (`extern` blocks, trait signatures) are skipped —
//! interface surface, not scored units. `macro_rules!` definitions are
//! quasi-quoted templates and skipped; macro CALL arguments are traversed
//! (they can hold real closures). File totals count each branch once
//! (nest-once); per-function entries keep whole-stack sharing.

use tree_sitter::{Node, Parser};

#[derive(Debug, Default, Clone)]
pub struct RsFunc {
    pub name: String,
    pub lineno: u32,
    pub end: u32,
    pub params: u32,
    pub complexity: u32,
    pub length: u32,
}

#[derive(Debug, Default)]
pub struct RsMetrics {
    pub funcs: usize,
    pub classes: usize,
    pub complexity: u32,
    pub max_func_cx: u32,
    pub max_params: u32,
    pub max_nesting: u32,
    pub imports: Vec<String>,
    pub func_details: Vec<RsFunc>,
    /// Every function/symbol name in visit order (untruncated; feeds
    /// ephemeral lexical ranking while func_details stays capped for reports).
    pub func_names: Vec<String>,
}

#[derive(Debug)]
struct Frame {
    name: String,
    lineno: u32,
    end: u32,
    params: u32,
    complexity: u32,
}

fn node_text(n: &Node, bytes: &[u8]) -> String {
    n.utf8_text(bytes).unwrap_or("").to_string()
}

fn func_name(n: Node, bytes: &[u8]) -> String {
    n.child_by_field_name("name")
        .map(|c| node_text(&c, bytes))
        .unwrap_or_default()
}

fn param_count(n: Node) -> u32 {
    // Top-level `parameter` children of the `parameters` field.
    // `self` counts (it is a bound coordination surface).
    n.child_by_field_name("parameters")
        .map(|p| {
            let mut c = p.walk();
            p.children(&mut c)
                .filter(|k| k.is_named() && k.kind() == "parameter")
                .count() as u32
        })
        .unwrap_or(0)
}

fn use_path(n: Node, bytes: &[u8]) -> String {
    // `use crate::a::{b, c};` — the argument subtree, whitespace-collapsed.
    // Brace lists stay inline (resolution happens downstream, if at all).
    let mut c = n.walk();
    let kids: Vec<_> = n.children(&mut c).collect();
    kids.into_iter()
        .find(|k| k.is_named() && k.kind() != "use" && k.kind() != ";")
        .map(|k| {
            node_text(&k, bytes)
                .chars()
                .filter(|ch| !ch.is_whitespace())
                .collect()
        })
        .unwrap_or_default()
}

fn is_bool_op(n: Node, bytes: &[u8]) -> bool {
    n.child_by_field_name("operator")
        .map(|o| {
            let t = node_text(&o, bytes);
            t == "&&" || t == "||"
        })
        .unwrap_or(false)
}

pub fn analyze_rust(text: &str) -> Result<RsMetrics, String> {
    let bytes = text.as_bytes();
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_rust::LANGUAGE.into())
        .map_err(|e| format!("grammar load failed: {e}"))?;
    let tree = parser
        .parse(text, None)
        .ok_or_else(|| "SyntaxError: parse failed".to_string())?;
    let root = tree.root_node();
    if root.has_error() {
        // Location-only fallback (rustc wording unknowable statically).
        // First-error walk is shared with the Python analyzer.
        return Err(format!(
            "SyntaxError: invalid syntax @ line {}",
            crate::py_header::first_error_row(root).unwrap_or(1)
        ));
    }

    let mut m = RsMetrics::default();
    let mut frames: Vec<Frame> = Vec::new();
    let mut all_branches = 0u32;
    let mut depth = 0u32;
    // A `#[cfg(test)]` outer attribute gates the NEXT sibling item (the
    // grammar keeps them adjacent, not nested).
    let mut pending_test_gate = false;
    // Explicit stack (no recursion: hostile-input safe).
    enum Item<'a> {
        Enter(tree_sitter::Node<'a>),
        LeaveFunc,
        LeaveDepth(u32),
    }
    let mut stack = vec![Item::Enter(root)];

    macro_rules! branch {
        () => {{
            all_branches += 1;
            for f in frames.iter_mut() {
                f.complexity += 1;
            }
        }};
    }
    macro_rules! push_kids {
        ($node:expr) => {{
            let mut cursor = $node.walk();
            let kids: Vec<_> = $node.children(&mut cursor).collect();
            for ch in kids.into_iter().rev() {
                stack.push(Item::Enter(ch));
            }
        }};
    }

    while let Some(item) = stack.pop() {
        match item {
            Item::LeaveFunc => {
                depth -= 1;
                if let Some(f) = frames.pop() {
                    m.max_func_cx = m.max_func_cx.max(f.complexity);
                    m.max_params = m.max_params.max(f.params);
                    let length = f.end.saturating_sub(f.lineno) + 1;
                    m.func_names.push(f.name.clone());
                    m.func_details.push(RsFunc {
                        name: f.name,
                        lineno: f.lineno,
                        end: f.end,
                        params: f.params,
                        complexity: f.complexity,
                        length,
                    });
                }
            }
            Item::LeaveDepth(entry) => {
                depth = entry;
            }
            Item::Enter(n) => {
                // Item-level nodes consume a pending #[cfg(test)] gate:
                // gated subtrees are verification surface (imports and fns
                // inside stay out of coupling and totals). Anything else
                // clears a stale gate and processes normally.
                const ITEMS: &[&str] = &[
                    "function_item",
                    "closure_expression",
                    "struct_item",
                    "enum_item",
                    "trait_item",
                    "union_item",
                    "impl_item",
                    "mod_item",
                    "macro_definition",
                    "use_declaration",
                    "const_item",
                    "static_item",
                    "type_item",
                ];
                if ITEMS.contains(&n.kind()) {
                    if pending_test_gate {
                        pending_test_gate = false;
                        continue;
                    }
                } else if n.kind() != "attribute_item" {
                    pending_test_gate = false;
                }
                match n.kind() {
                "function_item" => {
                    // Bodyless = declaration (extern/trait sig): skipped.
                    if n.child_by_field_name("body").is_none() {
                        continue;
                    }
                    let lineno = n.start_position().row as u32 + 1;
                    let end = n.end_position().row as u32 + 1;
                    let params = param_count(n);
                    depth += 1;
                    m.max_nesting = m.max_nesting.max(depth);
                    m.funcs += 1;
                    frames.push(Frame {
                        name: func_name(n, bytes),
                        lineno,
                        end,
                        params,
                        complexity: 1,
                    });
                    stack.push(Item::LeaveFunc);
                    push_kids!(n);
                }
                "closure_expression" => {
                    let lineno = n.start_position().row as u32 + 1;
                    let end = n.end_position().row as u32 + 1;
                    let params: u32 = n
                        .child_by_field_name("parameters")
                        .map(|p| {
                            let mut c = p.walk();
                            p.children(&mut c)
                                .filter(|k| k.is_named())
                                .count() as u32
                        })
                        .unwrap_or(0);
                    depth += 1;
                    m.max_nesting = m.max_nesting.max(depth);
                    m.funcs += 1;
                    frames.push(Frame {
                        name: String::new(),
                        lineno,
                        end,
                        params,
                        complexity: 1,
                    });
                    stack.push(Item::LeaveFunc);
                    push_kids!(n);
                }
                "struct_item" | "enum_item" | "trait_item" | "union_item" => {
                    m.classes += 1;
                    let entry = depth;
                    depth += 1;
                    m.max_nesting = m.max_nesting.max(depth);
                    stack.push(Item::LeaveDepth(entry));
                    push_kids!(n);
                }
                "impl_item" | "mod_item" => {
                    let entry = depth;
                    depth += 1;
                    m.max_nesting = m.max_nesting.max(depth);
                    stack.push(Item::LeaveDepth(entry));
                    push_kids!(n);
                }
                "macro_definition" => {
                    // Quasi-quoted template: skip the whole subtree.
                }
                "if_expression" => {
                    // Chain link (`else if`), not a level: the link branches
                    // without deepening (same representation as Python
                    // `elif`); a standalone `if` nests normally.
                    if n.parent().map(|p| p.kind() == "else_clause").unwrap_or(false) {
                        branch!();
                        push_kids!(n);
                    } else {
                        branch!();
                        let entry = depth;
                        depth += 1;
                        m.max_nesting = m.max_nesting.max(depth);
                        stack.push(Item::LeaveDepth(entry));
                        push_kids!(n);
                    }
                }
                "for_expression" | "while_expression"
                | "loop_expression" | "match_arm" => {
                    branch!();
                    let entry = depth;
                    depth += 1;
                    m.max_nesting = m.max_nesting.max(depth);
                    stack.push(Item::LeaveDepth(entry));
                    push_kids!(n);
                }
                "try_expression" => {
                    // `expr?`: short-circuit exactly like `?.`.
                    branch!();
                    push_kids!(n);
                }
                "binary_expression" => {
                    if is_bool_op(n, bytes) {
                        let mut cursor = n.walk();
                        let operands = n
                            .children(&mut cursor)
                            .filter(|c| {
                                c.is_named()
                                    && c.kind() != "comment"
                                    && c.kind() != "&&"
                                    && c.kind() != "||"
                            })
                            .count();
                        for _ in 0..operands.saturating_sub(1) {
                            branch!();
                        }
                    }
                    push_kids!(n);
                }
                "use_declaration" => {
                    let p = use_path(n, bytes);
                    if !p.is_empty() {
                        m.imports.push(p);
                    }
                }
                "attribute_item" => {
                    // Outer attributes precede their item as siblings.
                    // Precisely the standard test idioms — `cfg(test)` and
                    // `cfg_attr(test, ...)` — so `cfg(not(test))` and exotic
                    // predicates keep scoring (documented boundary).
                    // Doc comments are attribute_items too; they never
                    // match the gate and leave it unchanged.
                    let mut c = n.walk();
                    let kids: Vec<_> = n.children(&mut c).collect();
                    let gated = kids.iter().any(|k| {
                        k.is_named() && k.kind() == "attribute" && {
                            let t: String = node_text(k, bytes)
                                .chars()
                                .filter(|ch| !ch.is_whitespace())
                                .collect();
                            t == "cfg(test)" || t.starts_with("cfg_attr(test,")
                        }
                    });
                    pending_test_gate |= gated;
                }
                _ => {
                    push_kids!(n);
                }
                } // end match n.kind()
            } // end Item::Enter
        }
    }

    m.complexity =
        all_branches + m.func_details.len() as u32 + u32::from(m.funcs > 0 || m.classes > 0);
    m.func_details
        .sort_by_key(|f| std::cmp::Reverse(f.complexity));
    m.func_details.truncate(5);
    Ok(m)
}
