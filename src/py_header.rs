//! Python PEP 695/696 header extraction: type-parameter defaults.
//!
//! tree-sitter-python models type parameters without defaults in reach,
//! so defaults are extracted first (positions refer to the original text)
//! and the analyzer works on a blanked copy plus standalone parses.

// --- PEP 695/696 type-parameter defaults ---
//
// tree-sitter-python 0.25 models `type_parameter` as `[` types `]` with no
// bound (`:`) or default (`=`) support, so headers using them parse with
// ERROR nodes (probed: `class C[T: B = D]`, `type X[T = D] = T`).
//
// Strategy: extract each default expression, blank it (spaces, newlines kept
// so every row number is preserved), parse the blanked file normally, and
// visit each default — parsed standalone as `x = (<expr>)` — in the scope
// its header opened. Branch counting inside defaults then matches the
// reference exactly; plain-name defaults (the realistic case) contribute
// nothing and cost one extra tiny parse.

/// One header's defaults: expression text + the row its first token sits on.
type Defaults = Vec<(String, u32)>;

#[derive(Debug, Default)]
pub(crate) struct Extracted {
    pub(crate) blanks: Vec<(usize, usize)>,
    /// `type`-statement defaults by (alias, statement row); visited in place.
    pub(crate) stmts: Vec<((String, u32), Defaults)>,
    /// def/class defaults by (is_class, name, def-keyword row).
    pub(crate) scopes: Vec<((bool, String, u32), Defaults)>,
    /// Empty-default terminator row (SyntaxError: invalid syntax).
    pub(crate) empty_err: Option<u32>,
}

/// Skip a quoted string starting at ch[i] (the quote). Triple-aware and
/// backslash-aware; unterminated short strings end at the newline, mirroring
/// the pre-scan's treatment. F-string fields are skipped blindly: for header
/// scanning only `=`/`,`/`]` outside strings matter, and those never live
/// inside a replacement field except as operators of the field expression...
/// which a blind skip treats as string content. A `]` or `,` inside a field
/// (`f"{d['k']}"`) is therefore invisible here — the correct outcome, since
/// it is not a header separator either.
fn skip_quoted(ch: &[char], i: usize) -> usize {
    let q = ch[i];
    let triple = ch.get(i + 1) == Some(&q) && ch.get(i + 2) == Some(&q);
    let mut j = i + if triple { 3 } else { 1 };
    while j < ch.len() {
        let c = ch[j];
        if c == '\\' {
            j += 2;
            continue;
        }
        if c == q {
            if triple {
                if ch.get(j + 1) == Some(&q) && ch.get(j + 2) == Some(&q) {
                    return j + 3;
                }
                j += 1;
                continue;
            }
            return j + 1;
        }
        if !triple && c == '\n' {
            return j;
        }
        j += 1;
    }
    j
}

fn is_ident_start(c: char) -> bool {
    c == '_' || c.is_alphabetic()
}
fn is_ident_char(c: char) -> bool {
    c == '_' || c.is_alphanumeric()
}

struct HScan<'a> {
    pub(crate) text: &'a str,
    ch: Vec<char>,
    /// Byte offset of each char index (plus a sentinel at the end).
    bo: Vec<usize>,
    i: usize,
    /// Last consumed non-whitespace, non-comment char (strings count as
    /// their quote). Only used for `type`-statement-start detection.
    prev_sig: Option<char>,
}

impl<'a> HScan<'a> {
    fn peek(&self) -> Option<char> {
        self.ch.get(self.i).copied()
    }
    fn prev_raw(&self) -> Option<char> {
        if self.i == 0 {
            None
        } else {
            Some(self.ch[self.i - 1])
        }
    }
    fn bump(&mut self) {
        if let Some(c) = self.ch.get(self.i) {
            if !c.is_whitespace() {
                self.prev_sig = Some(*c);
            }
            self.i += 1;
        }
    }
    /// Skip whitespace and comments (neither touches prev_sig).
    fn ws(&mut self) {
        while self.i < self.ch.len() {
            let c = self.ch[self.i];
            if c == '#' {
                while self.i < self.ch.len() && self.ch[self.i] != '\n' {
                    self.i += 1;
                }
                continue;
            }
            if c.is_whitespace() {
                self.i += 1;
                continue;
            }
            break;
        }
    }
    fn skip_string(&mut self) {
        let q = self.ch[self.i];
        self.i = skip_quoted(&self.ch, self.i);
        self.prev_sig = Some(q);
    }
    fn row_at(&self, idx: usize) -> u32 {
        crate::py::row_of(self.text, self.bo[idx.min(self.ch.len())])
    }

    /// Parse a def/class/type header whose keyword ends at the current
    /// position. `kind`: 0 = def, 1 = class, 2 = type statement.
    fn header(&mut self, kind: u8, kw_row: u32, out: &mut Extracted) {
        self.ws();
        // Header name (absent for `type X = ...` without params — nothing
        // to extract, but the plain statement needs no help either).
        let name_start = self.i;
        if !self.peek().is_some_and(is_ident_start) {
            return;
        }
        while self.i < self.ch.len() && is_ident_char(self.ch[self.i]) {
            self.i += 1;
        }
        let name: String = self.ch[name_start..self.i].iter().collect();
        self.prev_sig = Some(self.ch[self.i - 1]);
        self.ws();
        if self.peek() != Some('[') {
            return;
        }
        self.bump(); // consume '['
        let mut group: Defaults = Vec::new();
        let mut depth = 1u32;
        loop {
            self.ws();
            let c = match self.peek() {
                Some(c) => c,
                None => return, // unbalanced: file errors regardless
            };
            if c == '\'' || c == '"' {
                self.skip_string();
                continue;
            }
            if c == '[' || c == '(' || c == '{' {
                depth += 1;
                self.bump();
                continue;
            }
            if c == ']' || c == ')' || c == '}' {
                // Only `]` can end the list here (`,` has its own branch
                // below and never reaches this arm).
                if depth == 1 && c == ']' {
                    break;
                }
                if depth == 1 {
                    return; // mismatched closer: file errors regardless
                }
                depth -= 1;
                self.bump();
                continue;
            }
            if c == ',' {
                self.bump();
                continue;
            }
            if c == '='
                && depth == 1
                && !matches!(self.prev_raw(), Some('=' | '!' | '<' | '>' | ':'))
                && self.ch.get(self.i + 1) != Some(&'=')
            {
                self.bump(); // consume '='
                let eq_end = self.bo[self.i];
                self.ws();
                // Empty default (`T = ,` / `T = ]`): invalid syntax at the
                // terminator row (probed).
                if matches!(self.peek(), Some(',') | Some(']') | None) {
                    let t = self.row_at(self.i);
                    out.empty_err = Some(out.empty_err.map_or(t, |r: u32| r.min(t)));
                    if self.peek() == Some(',') {
                        self.bump();
                        continue;
                    }
                    return;
                }
                let expr_idx = self.i;
                // Scan to the `,`/`]` terminating the default at this depth.
                let mut d = 1u32;
                let term = loop {
                    self.ws();
                    let cc = match self.peek() {
                        Some(cc) => cc,
                        None => break None,
                    };
                    if cc == '\'' || cc == '"' {
                        self.skip_string();
                        continue;
                    }
                    if cc == '[' || cc == '(' || cc == '{' {
                        d += 1;
                        self.bump();
                        continue;
                    }
                    if cc == ',' || cc == ']' {
                        // Commas never change depth (a `,` inside brackets
                        // is a separator, not a closer); only `]` at depth
                        // 1 terminates the default.
                        if d == 1 {
                            break Some(self.i);
                        }
                        if cc == ']' {
                            d -= 1;
                        }
                        self.bump();
                        continue;
                    }
                    if (cc == ')' || cc == '}') && d > 1 {
                        d -= 1;
                        self.bump();
                        continue;
                    }
                    if cc == ')' || cc == '}' {
                        return; // mismatched: file errors regardless
                    }
                    self.bump();
                };
                let term_idx = match term {
                    Some(t) => t,
                    None => return,
                };
                let expr = strip_trailing_comment(
                    &self.ch[expr_idx..term_idx].iter().collect::<String>(),
                );
                if expr.is_empty() {
                    let t = self.row_at(term_idx);
                    out.empty_err = Some(out.empty_err.map_or(t, |r: u32| r.min(t)));
                } else {
                    let erow = self.row_at(expr_idx);
                    // Blank from the `=` itself: the separator must go too,
                    // or the grammar still chokes on it.
                    out.blanks.push((eq_end - 1, self.bo[term_idx]));
                    group.push((expr, erow));
                }
                if self.ch[term_idx] == ']' {
                    self.i = term_idx + 1;
                    self.prev_sig = Some(']');
                    break;
                }
                self.i = term_idx + 1; // consume ','
                self.prev_sig = Some(',');
                continue;
            }
            self.bump();
        }
        // Consume the closing ']' (we broke on it, except via terminator path
        // which already advanced past it).
        if self.peek() == Some(']') {
            self.bump();
        }
        if group.is_empty() {
            return;
        }
        if kind == 2 {
            out.stmts.push(((name, kw_row), group));
        } else {
            out.scopes.push(((kind == 1, name, kw_row), group));
        }
    }
}

/// Cut a `#` comment starting outside strings; trim trailing whitespace.
fn strip_trailing_comment(s: &str) -> String {
    let ch: Vec<char> = s.chars().collect();
    let mut j = 0;
    while j < ch.len() {
        let c = ch[j];
        if c == '\'' || c == '"' {
            j = skip_quoted(&ch, j);
            continue;
        }
        if c == '#' {
            return ch[..j].iter().collect::<String>().trim_end().to_string();
        }
        j += 1;
    }
    s.trim_end().to_string()
}

pub(crate) fn extract_type_defaults(text: &str) -> Extracted {
    let mut out = Extracted::default();
    if !text.contains('[') {
        return out;
    }
    let ch: Vec<char> = text.chars().collect();
    let mut bo: Vec<usize> = Vec::with_capacity(ch.len() + 1);
    let mut b = 0;
    for c in &ch {
        bo.push(b);
        b += c.len_utf8();
    }
    bo.push(b);
    let mut s = HScan {
        text,
        ch,
        bo,
        i: 0,
        prev_sig: None,
    };
    while s.i < s.ch.len() {
        let c = s.ch[s.i];
        if c == '#' {
            while s.i < s.ch.len() && s.ch[s.i] != '\n' {
                s.i += 1;
            }
            continue;
        }
        if c == '\'' || c == '"' {
            s.skip_string();
            continue;
        }
        if is_ident_start(c) {
            let start = s.i;
            while s.i < s.ch.len() && is_ident_char(s.ch[s.i]) {
                s.i += 1;
            }
            let word: String = s.ch[start..s.i].iter().collect();
            let kw = if word == "def" {
                Some(0)
            } else if word == "class" {
                Some(1)
            } else if word == "type"
                && matches!(s.prev_sig, None | Some('\n') | Some(';') | Some(':'))
            {
                Some(2)
            } else {
                None
            };
            if let Some(kind) = kw {
                // `def`/`class`/`type` boundary on the right: the word
                // reader already stopped at a non-identifier char.
                let kw_row = crate::py::row_of(text, s.bo[start]);
                s.prev_sig = Some(s.ch[s.i - 1]);
                s.header(kind, kw_row, &mut out);
                continue;
            }
            s.prev_sig = Some(s.ch[s.i - 1]);
            continue;
        }
        if !c.is_whitespace() {
            s.prev_sig = Some(c);
        }
        s.i += 1;
    }
    out
}

/// Blank recorded spans (spaces, newlines preserved) so row numbers survive.
pub(crate) fn apply_blanks(text: &str, blanks: &[(usize, usize)]) -> String {
    let mut bytes = text.as_bytes().to_vec();
    for &(a, z) in blanks {
        for i in a.min(bytes.len())..z.min(bytes.len()) {
            if bytes[i] != b'\n' {
                bytes[i] = b' ';
            }
        }
    }
    String::from_utf8(bytes).expect("blanking ASCII spaces never breaks UTF-8")
}

/// Wrap a default expression so it parses standalone; the newline prefix
/// aligns wrapped rows with original rows exactly.
pub(crate) fn wrap_default(expr: &str, expr_row: u32) -> String {
    let mut w = "\n".repeat((expr_row.saturating_sub(1)) as usize);
    w.push_str("x = (");
    w.push_str(expr);
    w.push(')');
    w
}

/// First error/missing node row in pre-order, if any.
pub(crate) fn first_error_row(root: tree_sitter::Node) -> Option<u32> {
    let mut stack = vec![root];
    while let Some(nd) = stack.pop() {
        if nd.is_error() || nd.is_missing() {
            return Some(nd.start_position().row as u32 + 1);
        }
        let mut cursor = nd.walk();
        let kids: Vec<_> = nd.children(&mut cursor).collect();
        for k in kids.into_iter().rev() {
            stack.push(k);
        }
    }
    None
}
