//! Python AST analysis via tree-sitter, mirroring the reference
//! implementation's visitor semantics exactly (branch set, nesting rules,
//! TYPE_CHECKING erasure, deferred imports, module-level accounting).
//!
//! Hostile-input parity with CPython: NUL bytes, >200 bracket nesting, and
//! >500 visitor depth all report parse errors, because CPython refuses the
//! first two at parse time and dies recursing on the third.

use std::fmt::Write as _;

const WALK_CAP: u32 = 500;

#[derive(Debug, Clone, serde::Serialize)]
pub struct PyImport {
    pub module: String,
    pub level: u32,
    pub type_only: bool,
    pub deferred: bool,
    /// Names bound by `from module import a, b` (pre-alias). A name may be
    /// a submodule (`from pkg import crud`); resolution checks that.
    #[serde(skip)]
    pub names: Vec<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct PyFunc {
    pub name: String,
    pub lineno: u32,
    pub end: u32,
    pub params: u32,
    pub complexity: u32,
    pub length: u32,
}

#[derive(Debug, Default)]
pub struct PyMetrics {
    pub funcs: usize,
    pub classes: usize,
    pub complexity: u32,
    pub max_func_cx: u32,
    pub max_params: u32,
    pub max_nesting: u32,
    pub imports: Vec<PyImport>,
    pub func_details: Vec<PyFunc>,
    /// Every function/symbol name in visit order (untruncated; feeds
    /// ephemeral lexical ranking while func_details stays capped for reports).
    pub func_names: Vec<String>,
}

/// Row (1-based) of a byte offset in \n-normalized text.
pub(crate) fn row_of(text: &str, pos: usize) -> u32 {
    text.as_bytes()[..pos.min(text.len())]
        .iter()
        .filter(|&&c| c == b'\n')
        .count() as u32
        + 1
}


struct Frame {
    name: String,
    lineno: u32,
    end: u32,
    params: u32,
    complexity: u32,
}

enum Exit {
    Func,
    Class,
    Block,
    TypeOnly,
    /// enclosing `if`: restores depth to the recorded entry level.
    If(u32),
    Node,
}

enum Item<'a> {
    Enter(tree_sitter::Node<'a>),
    Leave(Exit),
}

fn node_text<'a>(n: &tree_sitter::Node, src: &'a [u8]) -> &'a str {
    std::str::from_utf8(&src[n.start_byte()..n.end_byte()]).unwrap_or("")
}

fn count_params(params_node: tree_sitter::Node) -> u32 {
    // Mirrors len(args.args) + len(kwonlyargs): positional (incl. self and
    // annotated) plus keyword-only; *args/**kwargs excluded (bare or
    // annotated — a typed splat still lands in kwarg position); names before
    // the positional-only separator excluded.
    fn is_plain_param(n: &tree_sitter::Node) -> bool {
        match n.kind() {
            "identifier" | "parameter" => true,
            "default_parameter" | "typed_parameter" | "typed_default_parameter" => {
                let mut cursor = n.walk();
                let kids: Vec<_> = n.children(&mut cursor).collect();
                !kids.iter().any(|g| {
                    matches!(
                        g.kind(),
                        "list_splat_pattern"
                            | "dictionary_splat_pattern"
                            | "splat_pattern"
                            | "list_splat"
                            | "dictionary_splat"
                    )
                })
            }
            _ => false,
        }
    }
    let mut n = 0u32;
    let mut cursor = params_node.walk();
    for ch in params_node.children(&mut cursor) {
        if ch.kind() == "positional_separator" {
            n = 0;
        } else if is_plain_param(&ch) {
            n += 1;
        }
    }
    n
}

fn func_name(n: &tree_sitter::Node, src: &[u8]) -> String {
    if let Some(name) = n.child_by_field_name("name") {
        return node_text(&name, src).to_string();
    }
    let mut cursor = n.walk();
    for ch in n.children(&mut cursor) {
        if ch.kind() == "identifier" {
            return node_text(&ch, src).to_string();
        }
    }
    "<anonymous>".to_string()
}

/// End row of a function body: the last real statement. Trailing comments
/// attach inside tree-sitter's container ranges (block, for_statement,
/// ...) but CPython's end_lineno stops at code. A leaf's end is always a
/// real token, so only leaves count — comments never do.
fn func_end_row(n: &tree_sitter::Node) -> u32 {
    let mut best = n.start_position().row;
    let mut stack = vec![*n];
    while let Some(nd) = stack.pop() {
        let mut cursor = nd.walk();
        let kids: Vec<_> = nd.children(&mut cursor).collect();
        if kids.is_empty() {
            if nd.kind() != "comment" {
                best = best.max(nd.end_position().row);
            }
        } else {
            stack.extend(kids);
        }
    }
    best as u32 + 1
}

/// (alias name, statement row) for a type_alias_statement node. The alias is
/// the `type` child just before the `=` child (the first `type` child is the
/// `type` keyword itself — never the alias, even when the alias is literally
/// named `type`).
fn alias_key(n: &tree_sitter::Node, src: &[u8]) -> Option<(String, u32)> {
    let mut cursor = n.walk();
    let kids: Vec<_> = n.children(&mut cursor).collect();
    let eq = kids.iter().position(|c| c.kind() == "=")?;
    let alias = kids[..eq].iter().rev().find(|c| c.kind() == "type")?;
    let mut st = vec![*alias];
    while let Some(nd) = st.pop() {
        if nd.kind() == "identifier" {
            return Some((
                node_text(&nd, src).to_string(),
                n.start_position().row as u32 + 1,
            ));
        }
        let mut cc = nd.walk();
        let kk: Vec<_> = nd.children(&mut cc).collect();
        for k in kk.into_iter().rev() {
            st.push(k);
        }
    }
    None
}

pub fn analyze_python(text: &str) -> Result<PyMetrics, String> {
    // Universal newlines (mirrors text-mode file reads): \r\n and lone \r
    // become \n before row numbers are assigned.
    let owned;
    let text = if text.contains('\r') {
        owned = text.replace("\r\n", "\n").replace('\r', "\n");
        &owned
    } else {
        text
    };
    if text.contains('\0') {
        // CPython raises SyntaxError (lineno None) for null bytes.
        return Err("source code string cannot contain null bytes @ line None".to_string());
    }
    // PEP 696 defaults live outside the grammar's reach: extract them first
    // (positions refer to the original text), then work on the blanked copy.
    let ext = crate::py_header::extract_type_defaults(text);
    // Competing failures resolve by source row: the parser reports the first
    // error it reaches. (Multiple simultaneous error classes in one file are
    // pathological; row order is the principled tie-break.)
    let mut cand: Option<(u32, String)> = None;
    let mut consider = |row: u32, msg: String| {
        cand = Some(match cand.take() {
            Some((r, m)) if r <= row => (r, m),
            _ => (row, msg),
        });
    };
    if let Some(r) = ext.empty_err {
        consider(r, "invalid syntax".to_string());
    }
    match crate::py_prescan::pre_scan(text) {
        Ok(()) => {}
        Err(crate::py_prescan::PreFail::Bracket(pos)) => {
            consider(row_of(text, pos), "too many nested parentheses".to_string());
        }
        Err(crate::py_prescan::PreFail::Num(f, start)) => {
            consider(row_of(text, start), f.message());
        }
    }
    if let Some((row, msg)) = cand {
        return Err(format!("{msg} @ line {row}"));
    }
    let blanked = crate::py_header::apply_blanks(text, &ext.blanks);
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&tree_sitter_python::LANGUAGE.into())
        .map_err(|e| format!("grammar error: {e}"))?;
    // Standalone parses of the extracted defaults, visited in the scope
    // their header opened. A broken default is a file-level syntax error at
    // its (row-aligned) position.
    let mut wtrees: Vec<tree_sitter::Tree> = Vec::new();
    let mut stmt_q: std::collections::HashMap<(String, u32), Vec<usize>> =
        std::collections::HashMap::new();
    let mut scope_q: std::collections::HashMap<(bool, String, u32), Vec<usize>> =
        std::collections::HashMap::new();
    for ((name, row), defs) in &ext.stmts {
        let mut idxs = Vec::new();
        for (expr, erow) in defs {
            let w = parser
                .parse(crate::py_header::wrap_default(expr, *erow).as_bytes(), None)
                .ok_or_else(|| "SyntaxError: parse failed".to_string())?;
            if let Some(r) = crate::py_header::first_error_row(w.root_node()) {
                return Err(format!("invalid syntax @ line {r}"));
            }
            idxs.push(wtrees.len());
            wtrees.push(w);
        }
        stmt_q.insert((name.clone(), *row), idxs);
    }
    for ((is_class, name, row), defs) in &ext.scopes {
        let mut idxs = Vec::new();
        for (expr, erow) in defs {
            let w = parser
                .parse(crate::py_header::wrap_default(expr, *erow).as_bytes(), None)
                .ok_or_else(|| "SyntaxError: parse failed".to_string())?;
            if let Some(r) = crate::py_header::first_error_row(w.root_node()) {
                return Err(format!("invalid syntax @ line {r}"));
            }
            idxs.push(wtrees.len());
            wtrees.push(w);
        }
        scope_q.insert((*is_class, name.clone(), *row), idxs);
    }
    let bytes = blanked.as_bytes();
    let tree = parser
        .parse(bytes, None)
        .ok_or_else(|| "SyntaxError: parse failed".to_string())?;
    let root = tree.root_node();

    let mut m = PyMetrics::default();
    let mut func_stack: Vec<Frame> = Vec::new();
    let mut class_depth = 0u32;
    let mut type_only = 0u32;
    let mut depth = 0u32; // scope depth (functions, classes, blocks)
    // Every branch exactly once, regardless of nesting: the FILE total
    // must not multiply branches by their def depth (a nested branch is
    // one decision, not N). Per-function entries keep whole-stack sharing
    // (established semantics: nested spans nest), but the aggregate below
    // uses this counter instead of summing shared entries.
    let mut all_branches = 0u32;
    let mut node_depth = 0u32; // raw tree depth: hostile-input tripwire
    // Node id of a defined scope whose frame was pre-pushed by an enclosing
    // decorated_definition (see below); its own Enter skips re-pushing.
    let mut pre_pushed: Option<usize> = None;
    let mut stack = vec![Item::Enter(root)];

    macro_rules! branch {
        () => {
            // A branch counts in EVERY enclosing function (nested defs
            // share their branches upward), else at module level — but
            // exactly ONCE in the file aggregate (all_branches).
            all_branches += 1;
            if !func_stack.is_empty() {
                for f in func_stack.iter_mut() {
                    f.complexity += 1;
                }
            }
        };
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
    macro_rules! visit_defaults {
        ($map:expr, $key:expr) => {
            // Drain: each header's defaults are visited exactly once, in the
            // scope the header opened (reference: type_params are part of the
            // definition node's own visit).
            if let Some(idxs) = $map.remove(&$key) {
                for idx in idxs.into_iter().rev() {
                    stack.push(Item::Enter(wtrees[idx].root_node()));
                }
            }
        };
    }

    while let Some(item) = stack.pop() {
        match item {
            Item::Leave(Exit::Node) => {
                node_depth -= 1;
            }
            Item::Leave(Exit::Func) => {
                depth -= 1;
                if let Some(f) = func_stack.pop() {
                    m.max_func_cx = m.max_func_cx.max(f.complexity);
                    m.max_params = m.max_params.max(f.params);
                    let length = f.end.saturating_sub(f.lineno) + 1;
                    m.func_names.push(f.name.clone());
                    m.func_details.push(PyFunc {
                        name: f.name,
                        lineno: f.lineno,
                        end: f.end,
                        params: f.params,
                        complexity: f.complexity,
                        length,
                    });
                }
            }
            Item::Leave(Exit::Class) => {
                depth -= 1;
                class_depth -= 1;
            }
            Item::Leave(Exit::Block) => {
                depth -= 1;
            }
            Item::Leave(Exit::TypeOnly) => {
                // Depth is restored wholly by the paired If marker.
                type_only -= 1;
            }
            Item::Leave(Exit::If(entry)) => {
                depth = entry;
            }
            Item::Enter(n) => {
                node_depth += 1;
                if node_depth > WALK_CAP {
                    return Err("RecursionError: maximum recursion depth exceeded".to_string());
                }
                if n.is_error() || n.is_missing() {
                    // Tree-sitter cannot reproduce CPython's full message
                    // catalog; "invalid syntax" is the common case and the
                    // format matches the reference exactly.
                    let mut msg = "invalid syntax".to_string();
                    let _ = write!(msg, " @ line {}", n.start_position().row + 1);
                    return Err(msg);
                }
                // Every Enter gets a matching Node leave for the tripwire.
                stack.push(Item::Leave(Exit::Node));
                match n.kind() {
                    "decorated_definition" => {
                        // The reference pushes the defined scope's frame and
                        // THEN visits decorators, so decorator branches count
                        // into the scope. Mirror by pre-pushing here: the
                        // definition's own Enter skips re-pushing below.
                        let mut cursor = n.walk();
                        let kids: Vec<_> = n.children(&mut cursor).collect();
                        let target = kids.iter().find(|c| {
                            c.kind() == "function_definition"
                                || c.kind() == "class_definition"
                        });
                        if let Some(t) = target {
                            pre_pushed = Some(t.id());
                            if t.kind() == "function_definition" {
                                let params = t
                                    .child_by_field_name("parameters")
                                    .map(count_params)
                                    .unwrap_or(0);
                                func_stack.push(Frame {
                                    name: func_name(t, bytes),
                                    lineno: t.start_position().row as u32 + 1,
                                    end: func_end_row(t),
                                    params,
                                    complexity: 1,
                                });
                                m.funcs += 1;
                                depth += 1;
                                m.max_nesting = m.max_nesting.max(depth);
                                stack.push(Item::Leave(Exit::Func));
                                visit_defaults!(
                                    scope_q,
                                    (
                                        false,
                                        func_name(t, bytes),
                                        t.start_position().row as u32 + 1
                                    )
                                );
                            } else {
                                m.classes += 1;
                                depth += 1;
                                class_depth += 1;
                                m.max_nesting = m.max_nesting.max(depth);
                                stack.push(Item::Leave(Exit::Class));
                                visit_defaults!(
                                    scope_q,
                                    (
                                        true,
                                        func_name(t, bytes),
                                        t.start_position().row as u32 + 1
                                    )
                                );
                            }
                        }
                        push_kids!(n);
                    }
                    "function_definition" => {
                        if pre_pushed.take() == Some(n.id()) {
                            // Frame already opened by decorated_definition.
                            push_kids!(n);
                        } else {
                            let params = n
                                .child_by_field_name("parameters")
                                .map(count_params)
                                .unwrap_or(0);
                            // Decorators/defaults walk inside the new frame —
                            // the reference visitor pushes before visiting them.
                            func_stack.push(Frame {
                                name: func_name(&n, bytes),
                                lineno: n.start_position().row as u32 + 1,
                                end: func_end_row(&n),
                                params,
                                complexity: 1,
                            });
                            m.funcs += 1;
                            depth += 1;
                            m.max_nesting = m.max_nesting.max(depth);
                            stack.push(Item::Leave(Exit::Func));
                            visit_defaults!(
                                scope_q,
                                (
                                    false,
                                    func_name(&n, bytes),
                                    n.start_position().row as u32 + 1
                                )
                            );
                            push_kids!(n);
                        }
                    }
                    "class_definition" => {
                        if pre_pushed.take() == Some(n.id()) {
                            push_kids!(n);
                        } else {
                            m.classes += 1;
                            depth += 1;
                            class_depth += 1;
                            m.max_nesting = m.max_nesting.max(depth);
                            stack.push(Item::Leave(Exit::Class));
                            visit_defaults!(
                                scope_q,
                                (
                                    true,
                                    func_name(&n, bytes),
                                    n.start_position().row as u32 + 1
                                )
                            );
                            push_kids!(n);
                        }
                    }
                    "if_statement" => {
                        branch!();
                        let is_tc = n
                            .child_by_field_name("condition")
                            .map(|c| {
                                c.kind() == "identifier"
                                    && node_text(&c, bytes) == "TYPE_CHECKING"
                            })
                            .unwrap_or(false);
                        let entry = depth;
                        depth += 1;
                        m.max_nesting = m.max_nesting.max(depth);
                        if is_tc {
                            // Erased at runtime: imports inside are
                            // type-only, but still reasoning coupling.
                            // Depth is restored wholly by the If marker.
                            type_only += 1;
                            stack.push(Item::Leave(Exit::If(entry)));
                            stack.push(Item::Leave(Exit::TypeOnly));
                        } else {
                            stack.push(Item::Leave(Exit::If(entry)));
                        }
                        push_kids!(n);
                    }
                    "elif_clause" => {
                        // Chain link, not a level: an if/elif/else chain is
                        // one decision (wide dispatch, not deep context), so
                        // the link branches without deepening. Statements
                        // inside the body still nest normally. (The old code
                        // mirrored ast's If-in-orelse nesting literally and
                        // read a 31-elif dispatch as depth 34.)
                        branch!();
                        push_kids!(n);
                    }
                    "for_statement" | "while_statement" | "except_clause"
                    | "case_clause" => {
                        branch!();
                        depth += 1;
                        m.max_nesting = m.max_nesting.max(depth);
                        stack.push(Item::Leave(Exit::Block));
                        push_kids!(n);
                    }
                    "conditional_expression" | "assert_statement" => {
                        branch!();
                        push_kids!(n);
                    }
                    "boolean_operator" => {
                        let mut cursor = n.walk();
                        // `comment` nodes are named in this grammar; only
                        // real operands count (a comment between operands
                        // must not read as an extra branch).
                        let operands = n
                            .children(&mut cursor)
                            .filter(|c| c.is_named() && c.kind() != "comment")
                            .count();
                        for _ in 0..operands.saturating_sub(1) {
                            branch!();
                        }
                        push_kids!(n);
                    }
                    "if_clause" => {
                        // Comprehension filter: counts, never nests. But the
                        // same node kind guards match cases (`case X if c`),
                        // where the reference sees a plain expression —
                        // only the guard's own operators count there.
                        let is_guard = n
                            .parent()
                            .map(|p| p.kind() == "case_clause")
                            .unwrap_or(false);
                        if !is_guard {
                            branch!();
                        }
                        push_kids!(n);
                    }
                    "import_statement" => {
                        let deferred =
                            !func_stack.is_empty() || class_depth > 0;
                        let t = type_only > 0;
                        let mut cursor = n.walk();
                        for ch in n.children(&mut cursor) {
                            let module = match ch.kind() {
                                "dotted_name" => node_text(&ch, bytes)
                                    .chars()
                                    .filter(|c| !c.is_whitespace())
                                    .collect(),
                                "aliased_import" => {
                                    let mut cc = ch.walk();
                                    let kids: Vec<_> =
                                        ch.children(&mut cc).collect();
                                    kids.iter()
                                        .find(|g| g.kind() == "dotted_name")
                                        .map(|g| {
                                            node_text(g, bytes)
                                                .chars()
                                                .filter(|c| !c.is_whitespace())
                                                .collect()
                                        })
                                        .unwrap_or_default()
                                }
                                _ => continue,
                            };
                            m.imports.push(PyImport {
                                module,
                                level: 0,
                                type_only: t,
                                deferred,
                                names: Vec::new(),
                            });
                        }
                        // No children need walking (names carry no branches).
                    }
                    "import_from_statement" => {
                        let deferred =
                            !func_stack.is_empty() || class_depth > 0;
                        let t = type_only > 0;
                        // The module is the dotted name BEFORE the `import`
                        // keyword, or nested in the relative prefix
                        // (`from .pkg import y`); names AFTER `import` are
                        // bindings, never modules.
                        let mut module = String::new();
                        let mut level = 0u32;
                        let mut seen_import_kw = false;
                        let mut names: Vec<String> = Vec::new();
                        let mut cursor = n.walk();
                        for ch in n.children(&mut cursor) {
                            match ch.kind() {
                                "import" => seen_import_kw = true,
                                "dotted_name" => {
                                    if !seen_import_kw && module.is_empty() {
                                        module = node_text(&ch, bytes)
                                            .chars()
                                            .filter(|c| !c.is_whitespace())
                                            .collect();
                                    } else if seen_import_kw {
                                        names.push(node_text(&ch, bytes).trim().to_string());
                                    }
                                }
                                "aliased_import" if seen_import_kw => {
                                    if let Some(nm) = ch.child_by_field_name("name") {
                                        names.push(node_text(&nm, bytes).trim().to_string());
                                    }
                                }
                                "relative_import" => {
                                    let mut cc = ch.walk();
                                    let kids: Vec<_> =
                                        ch.children(&mut cc).collect();
                                    for g in &kids {
                                        if g.kind() == "import_prefix" {
                                            level += node_text(g, bytes)
                                                .chars()
                                                .filter(|&c| c == '.')
                                                .count()
                                                as u32;
                                        } else if g.kind() == "dotted_name"
                                            && module.is_empty()
                                        {
                                            module = node_text(g, bytes)
                                                .chars()
                                                .filter(|c| !c.is_whitespace())
                                                .collect();
                                        }
                                    }
                                }
                                _ => {}
                            }
                        }
                        m.imports.push(PyImport {
                            module,
                            level,
                            type_only: t,
                            deferred,
                            names,
                        });
                    }
                    "future_import_statement" => {
                        // `from __future__ import ...`: module is literal.
                        let deferred =
                            !func_stack.is_empty() || class_depth > 0;
                        m.imports.push(PyImport {
                            module: "__future__".to_string(),
                            level: 0,
                            type_only: type_only > 0,
                            deferred,
                            names: Vec::new(),
                        });
                    }
                    "type_alias_statement" => {
                        // PEP 696 defaults on `type X[T = D] = ...` were
                        // extracted and blanked above; visit them here, in
                        // the statement's own scope (module level, or inside
                        // the enclosing function when nested).
                        if let Some(key) = alias_key(&n, bytes) {
                            visit_defaults!(stmt_q, key);
                        }
                        push_kids!(n);
                    }
                    _ => {
                        push_kids!(n);
                    }
                }
            }
        }
    }

    // File total counts each branch once (all_branches) plus one base per
    // defined function: identical to the shared sum for flat files, immune
    // to nesting-depth multiplication. (Upward sharing is retained inside
    // per-function entries for max_func_cx / top_funcs whole-stack
    // semantics.)
    m.complexity = all_branches
        + m.func_details.len() as u32
        + u32::from(m.funcs > 0 || m.classes > 0);
    // Sort by complexity desc, ties by completion order: the reference
    // appends on scope exit and stable-sorts, so decorate-and-sort keeps
    // ties in visit-completion order deterministically.
    let mut order: Vec<usize> = (0..m.func_details.len()).collect();
    order.sort_by_key(|&i| std::cmp::Reverse(m.func_details[i].complexity));
    m.func_details = order.into_iter().map(|i| m.func_details[i].clone()).collect();
    m.func_details.truncate(5);
    Ok(m)
}
