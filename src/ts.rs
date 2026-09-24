//! TypeScript/JavaScript parse + metrics (tree-sitter AST).
//!
//! Replaces the former brace/regex heuristics with exact structure while
//! preserving every deliberately-validated behavior:
//! - branch set: `if` (+ chains), ternaries, `?.` per occurrence,
//!   `&&`/`||`/`??` (operands − 1), `case` (NOT `default`), `catch`,
//!   `for`/`for-in`/`while`/`do`;
//! - `switch` itself adds nothing (like Python `match`); `try`/`finally`
//!   bodies nest (JS control-scope representation) but add no branch;
//!   `else` nests without branching, but `else if` chain links are
//!   transparent (like Python `elif`); `throw`, bare blocks, static blocks
//!   neither count nor nest;
//! - overload/ambient signatures (no body) are skipped like bodyless
//!   declarations; expression-bodied arrows count as functions but don't
//!   nest (they had no span before either);
//! - JSX prose (`jsx_text`, entities) and regex bodies are skipped — the
//!   old keyword noise is gone structurally — while `{...}` expressions
//!   and `${...}` interpolations ARE traversed (previously blanked whole;
//!   now precise with a real parser);
//! - imports harvested from syntax (`import`/`export…from`/`require`), so
//!   matches inside comments/strings no longer draw edges; dynamic
//!   `import()` stays missed (as before — documented);
//! - files always score (the heuristic never errored): trees with error
//!   nodes are traversed, never flagged.
//!
//! File totals count each branch once (nest-once); per-function entries
//! keep whole-stack sharing — identical economics to Python and Rust.

use tree_sitter::{Node, Parser};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TsLang {
    JavaScript,
    TypeScript,
    Tsx,
}

#[derive(Debug, Default, Clone)]
pub struct TsFunc {
    pub name: String,
    pub lineno: u32,
    pub end: u32,
    pub params: u32,
    pub complexity: u32,
    pub length: u32,
}

#[derive(Debug, Default)]
pub struct TsMetrics {
    pub funcs: usize,
    pub classes: usize,
    pub complexity: u32,
    pub max_func_cx: u32,
    pub max_params: u32,
    pub max_nesting: u32,
    pub imports: Vec<String>,
    pub type_only_imports: Vec<String>,
    pub func_details: Vec<TsFunc>,
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
    /// Whether this frame opened a nesting scope (expression-bodied
    /// arrows don't — they had no span before either).
    nested: bool,
}

fn node_text(n: &Node, bytes: &[u8]) -> String {
    n.utf8_text(bytes).unwrap_or("").to_string()
}

fn func_name(n: Node, bytes: &[u8]) -> String {
    if let Some(name) = n.child_by_field_name("name") {
        return node_text(&name, bytes);
    }
    // Anonymous (default-exported `function()`): still a real function.
    String::new()
}

fn param_count(n: Node) -> u32 {
    // Top-level value parameters; rest (`...args`) excluded (Python parity).
    if let Some(p) = n.child_by_field_name("parameters") {
        let mut c = p.walk();
        let kids: Vec<_> = p.children(&mut c).collect();
        return kids
            .iter()
            .filter(|k| {
                k.is_named()
                    && matches!(k.kind(), "required_parameter" | "optional_parameter")
            })
            .count() as u32;
    }
    // Single-identifier arrow params (`x => …`): one param if an
    // identifier precedes the `=>`.
    if n.kind() == "arrow_function" {
        let mut c = n.walk();
        let kids: Vec<_> = n.children(&mut c).collect();
        for k in &kids {
            if !k.is_named() && k.kind() == "=>" {
                break;
            }
            if k.is_named() && k.kind() == "identifier" {
                return 1;
            }
        }
    }
    0
}

fn string_value(n: Node, bytes: &[u8]) -> Option<String> {
    // A `"..."` / `'...'` literal's inner text (no escapes processing:
    // module specifiers don't use them; a miss just misses an edge).
    let t = node_text(&n, bytes);
    let t = t.strip_prefix(['"', '\''])?;
    t.strip_suffix(['"', '\'']).map(|s| s.to_string())
}

fn has_body(n: Node) -> bool {
    let mut c = n.walk();
    let kids: Vec<_> = n.children(&mut c).collect();
    kids.iter().any(|k| k.kind() == "statement_block")
}

pub fn analyze_ts(text: &str, lang: TsLang) -> TsMetrics {
    let bytes = text.as_bytes();
    let mut parser = Parser::new();
    let grammar = match lang {
        TsLang::JavaScript => &tree_sitter_javascript::LANGUAGE.into(),
        TsLang::TypeScript => &tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        TsLang::Tsx => &tree_sitter_typescript::LANGUAGE_TSX.into(),
    };
    let mut m = TsMetrics::default();
    if parser.set_language(grammar).is_err() {
        return m;
    }
    let tree = match parser.parse(text, None) {
        Some(t) => t,
        None => return m,
    };
    let root = tree.root_node();

    let mut frames: Vec<Frame> = Vec::new();
    let mut all_branches = 0u32;
    let mut depth = 0u32;
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
    macro_rules! enter_func {
        ($node:expr, $nest:expr) => {{
            let n = $node;
            let lineno = n.start_position().row as u32 + 1;
            let end = n.end_position().row as u32 + 1;
            let params = param_count(n);
            if $nest {
                depth += 1;
                m.max_nesting = m.max_nesting.max(depth);
            }
            m.funcs += 1;
            frames.push(Frame {
                name: func_name(n, bytes),
                lineno,
                end,
                params,
                complexity: 1,
                nested: $nest,
            });
            stack.push(Item::LeaveFunc);
            push_kids!(n);
        }};
    }
    // Open a nesting scope without a function frame (control blocks).
    macro_rules! nest {
        ($node:expr) => {{
            let entry = depth;
            depth += 1;
            m.max_nesting = m.max_nesting.max(depth);
            stack.push(Item::LeaveDepth(entry));
            push_kids!($node);
        }};
    }

    while let Some(item) = stack.pop() {
        match item {
            Item::LeaveFunc => {
                if let Some(f) = frames.pop() {
                    if f.nested {
                        depth -= 1;
                    }
                    m.max_func_cx = m.max_func_cx.max(f.complexity);
                    m.max_params = m.max_params.max(f.params);
                    let length = f.end.saturating_sub(f.lineno) + 1;
                    m.func_names.push(f.name.clone());
                    m.func_details.push(TsFunc {
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
            Item::Enter(n) => match n.kind() {
                "function_declaration" | "function_expression" => {
                    // Overloads/ambients have signatures, not bodies.
                    if has_body(n) {
                        enter_func!(n, true);
                    } else {
                        push_kids!(n);
                    }
                }
                "arrow_function" => {
                    // Block bodies nest; expression bodies don't (they had
                    // no span before either).
                    enter_func!(n, has_body(n));
                }
                "method_definition" => {
                    // Abstract/overload signatures lack bodies.
                    if has_body(n) {
                        enter_func!(n, true);
                    } else {
                        push_kids!(n);
                    }
                }
                "class_declaration" | "class_expression" => {
                    m.classes += 1;
                    nest!(n);
                }
                "if_statement" => {
                    // Chain link (`else if`), not a level: the link branches
                    // without deepening (same representation as Python
                    // `elif`); a standalone `if` nests normally.
                    if n.parent().map(|p| p.kind() == "else_clause").unwrap_or(false) {
                        branch!();
                        push_kids!(n);
                    } else {
                        branch!();
                        nest!(n);
                    }
                }
                "else_clause" => {
                    // `else if` is a chain link: transparent, so the inner
                    // `if` counts its branch at the chain's level instead of
                    // stacking +2 per link (the old code read a 10-chain as
                    // depth 21). A block `else` keeps its control scope:
                    // `else` nests without branching.
                    let mut cursor = n.walk();
                    let chained = n
                        .children(&mut cursor)
                        .any(|c| c.is_named() && c.kind() == "if_statement");
                    if chained {
                        push_kids!(n);
                    } else {
                        nest!(n);
                    }
                }
                "ternary_expression" => {
                    branch!();
                    push_kids!(n);
                }
                "switch_statement" => {
                    // Cases branch (below); the switch itself doesn't,
                    // mirroring Python `match`.
                    push_kids!(n);
                }
                "switch_case" => {
                    branch!();
                    nest!(n);
                }
                "switch_default" => {
                    // Deliberate: the old keyword set counted `case` only.
                    push_kids!(n);
                }
                "for_statement" | "for_in_statement" | "while_statement"
                | "do_statement" => {
                    branch!();
                    nest!(n);
                }
                "catch_clause" => {
                    branch!();
                    nest!(n);
                }
                "try_statement" | "finally_clause" => {
                    // No branch, no nesting (Python parity: only the
                    // `except`/`catch` handler nests). The old heuristic
                    // nested `try` bodies, but the locked parity test
                    // (deep.ts == deep.py == 5) decides.
                    push_kids!(n);
                }
                "optional_chain" => {
                    // One node per `?.` occurrence: exact parity.
                    branch!();
                    push_kids!(n);
                }
                "binary_expression" => {
                    let mut cursor = n.walk();
                    let kids: Vec<_> = n.children(&mut cursor).collect();
                    let mut op_and = false;
                    let mut op_or = false;
                    let mut op_null = false;
                    let mut operands = 0;
                    for k in &kids {
                        if !k.is_named() {
                            match k.kind() {
                                "&&" => op_and = true,
                                "||" => op_or = true,
                                "??" => op_null = true,
                                _ => {}
                            }
                        } else {
                            operands += 1;
                        }
                    }
                    if (op_and || op_or || op_null) && operands >= 2 {
                        for _ in 0..operands - 1 {
                            branch!();
                        }
                    }
                    push_kids!(n);
                }
                "template_substitution" | "jsx_expression" => {
                    // Real code inside markup/templates: traversed, not
                    // nested (expression context).
                    push_kids!(n);
                }
                "regex" | "jsx_text" | "html_character_reference" => {
                    // String/regex/prose contents are not code: the old
                    // keyword noise is gone structurally.
                }
                "import_statement" => {
                    // Statement-level `import type` is type-only. So is an
                    // import whose every named specifier is `type`-marked
                    // (`import { type A }`), which erases fully at runtime.
                    // A bare default (`import D`) or any unmarked specifier
                    // keeps a runtime edge; `verbatimModuleSyntax` (which
                    // preserves the side-effect import) stays out of scope —
                    // documented on the edge, not modeled here.
                    let mut cursor = n.walk();
                    let kids: Vec<_> = n.children(&mut cursor).collect();
                    let mut source = None;
                    let mut is_type = false;
                    let mut spec_total = 0usize;
                    let mut spec_typed = 0usize;
                    for k in &kids {
                        if k.kind() == "string" && source.is_none() {
                            source = string_value(*k, bytes);
                        }
                        if k.kind() == "type" {
                            is_type = true;
                        }
                        if matches!(k.kind(), "import_clause" | "named_imports") {
                            let mut stack = vec![*k];
                            while let Some(cur) = stack.pop() {
                                let mut cc = cur.walk();
                                for ch in cur.children(&mut cc) {
                                    if ch.kind() == "import_specifier" {
                                        spec_total += 1;
                                        let mut sc = ch.walk();
                                        if ch.children(&mut sc).any(|g| g.kind() == "type") {
                                            spec_typed += 1;
                                        }
                                    } else {
                                        stack.push(ch);
                                    }
                                }
                            }
                        }
                    }
                    // A lone `import './x'` (no clause, no specifiers) is a
                    // side-effect runtime edge, not a type-only one.
                    let all_typed = spec_total > 0 && spec_total == spec_typed;
                    if let Some(s) = source {
                        if is_type || all_typed {
                            m.type_only_imports.push(s);
                        } else {
                            m.imports.push(s);
                        }
                    }
                }
                "export_statement" => {
                    // `export … from 'x'` (re-export edge) and
                    // `export type … from 'x'` (type-only). Like imports, an
                    // export whose every specifier is `type`-marked erases
                    // fully; a bare value specifier keeps the runtime edge.
                    // Plain exports declare nothing outward.
                    let mut cursor = n.walk();
                    let kids: Vec<_> = n.children(&mut cursor).collect();
                    let has_from = kids
                        .iter()
                        .any(|k| !k.is_named() && k.kind() == "from");
                    let mut is_type = false;
                    let mut source = None;
                    let mut spec_total = 0usize;
                    let mut spec_typed = 0usize;
                    for k in &kids {
                        if k.kind() == "type" {
                            is_type = true;
                        }
                        if k.kind() == "string" && source.is_none() {
                            source = string_value(*k, bytes);
                        }
                        if matches!(k.kind(), "export_clause") {
                            let mut stack = vec![*k];
                            while let Some(cur) = stack.pop() {
                                let mut cc = cur.walk();
                                for ch in cur.children(&mut cc) {
                                    if ch.kind() == "export_specifier" {
                                        spec_total += 1;
                                        let mut sc = ch.walk();
                                        if ch.children(&mut sc).any(|g| g.kind() == "type") {
                                            spec_typed += 1;
                                        }
                                    } else {
                                        stack.push(ch);
                                    }
                                }
                            }
                        }
                    }
                    let all_typed = spec_total > 0 && spec_total == spec_typed;
                    if has_from {
                        if let Some(s) = source {
                            if is_type || all_typed {
                                m.type_only_imports.push(s);
                            } else {
                                m.imports.push(s);
                            }
                        }
                    }
                    push_kids!(n);
                }
                "call_expression" => {
                    // `require('x')` draws an edge (as before); dynamic
                    // `import(x)` stays missed (as before — documented).
                    let mut cursor = n.walk();
                    let kids: Vec<_> = n.children(&mut cursor).collect();
                    let mut callee = None;
                    let mut arg = None;
                    for k in &kids {
                        if k.kind() == "identifier" && callee.is_none() {
                            callee = Some(node_text(k, bytes));
                        }
                        if k.kind() == "arguments" && arg.is_none() {
                            let mut ac = k.walk();
                            let ak: Vec<_> = k.children(&mut ac).collect();
                            for a in &ak {
                                if a.kind() == "string" {
                                    arg = string_value(*a, bytes);
                                    break;
                                }
                            }
                        }
                    }
                    if callee.as_deref() == Some("require") {
                        if let Some(s) = arg {
                            m.imports.push(s);
                        }
                    }
                    push_kids!(n);
                }
                _ => {
                    push_kids!(n);
                }
            },
        }
    }

    m.complexity =
        all_branches + m.func_details.len() as u32 + u32::from(m.funcs > 0 || m.classes > 0);
    m.func_details
        .sort_by_key(|f| std::cmp::Reverse(f.complexity));
    m.func_details.truncate(5);
    m
}
