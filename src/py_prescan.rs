//! Python lexical pre-scan: hostile-input parity with CPython.
//!
//! Refuses what CPython refuses before tree-sitter runs: NUL bytes,
//! bracket nesting past the cap, and malformed numeric literals.
//! Failures carry offsets; messages are composed at the call site.

pub(crate) const BRACKET_CAP: usize = 200;

/// Lexical pre-scan, strings and comments excluded, serving two CPython
/// parity tripwires: >200 nested brackets is refused at parse time, and so
/// is any malformed numeric token (`1foo`, `0755`, `0x1G`) which tree-sitter
/// would otherwise split into valid pieces.
/// Pre-scan failure: bracket-cap trips record the offending byte offset;
/// number trips record the literal's start offset (CPython blames that line).
pub(crate) enum PreFail {
    Bracket(usize),
    Num(NumFail, usize),
}

/// Numeric literal rejection classes, mapped 1:1 onto CPython's SyntaxError
/// messages (probed against the local interpreter — these are the
/// interpreter's stable catalog, not repo-specific rules).
pub(crate) enum NumFail {
    Hex,
    Oct { digit: Option<char> },
    Bin { digit: Option<char> },
    Dec,
    Imag,
    LeadingZero,
}

impl NumFail {
    pub(crate) fn message(&self) -> String {
        match self {
            NumFail::Hex => "invalid hexadecimal literal".to_string(),
            NumFail::Oct { digit: None } => "invalid octal literal".to_string(),
            NumFail::Oct { digit: Some(d) } => format!("invalid digit '{d}' in octal literal"),
            NumFail::Bin { digit: None } => "invalid binary literal".to_string(),
            NumFail::Bin { digit: Some(d) } => format!("invalid digit '{d}' in binary literal"),
            NumFail::Dec => "invalid decimal literal".to_string(),
            NumFail::Imag => "invalid imaginary literal".to_string(),
            NumFail::LeadingZero => "leading zeros in decimal integer literals are not permitted; use an 0o prefix for octal integers".to_string(),
        }
    }
}

pub(crate) fn pre_scan(text: &str) -> Result<(), PreFail> {
    let b = text.as_bytes();
    let mut i = 0;
    let mut depth = 0usize;
    let mut prev = 0u8;
    while i < b.len() {
        let c = b[i];
        // String prefixes ([rRbBuUfFtT]*) then a quote.
        if c.is_ascii_alphabetic() {
            let mut j = i;
            while j < b.len() && b"rRbBuUfFtT".contains(&b[j]) {
                j += 1;
            }
            if j < b.len() && (b[j] == b'\'' || b[j] == b'"') && j > i {
                let is_f = text[i..j].bytes().any(|p| p == b'f' || p == b'F');
                let q = j;
                i = skip_string(b, j, is_f, &mut depth);
                if depth > BRACKET_CAP {
                    return Err(PreFail::Bracket(q));
                }
                prev = b' ';
                continue;
            }
            // Identifier tail: skipD, digits can never start a token here.
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                i += 1;
            }
            prev = b'a';
            continue;
        }
        if c == b'\'' || c == b'"' {
            let q = i;
            i = skip_string(b, i, false, &mut depth);
            if depth > BRACKET_CAP {
                return Err(PreFail::Bracket(q));
            }
            prev = b' ';
            continue;
        }
        if c == b'#' {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if c.is_ascii_digit()
            && !(prev.is_ascii_alphanumeric() || prev == b'_')
        {
            // Digit starting a fresh token (identifier tails like `foo1`
            // are skipped by the alpha arm above).
            let start = i;
            if let Err(f) = scan_number(b, &mut i) {
                return Err(PreFail::Num(f, start));
            }
            prev = b'0';
            if depth > BRACKET_CAP {
                return Err(PreFail::Bracket(start));
            }
            continue;
        }
        if c == b'.' && b.get(i + 1).is_some_and(|d| d.is_ascii_digit()) {
            // Fraction-number start (`.5`, `x.5`, `.5foo`); a bare `.`
            // falls through as an operator.
            let start = i;
            if let Err(f) = scan_number(b, &mut i) {
                return Err(PreFail::Num(f, start));
            }
            prev = b'0';
            continue;
        }
        match c {
            b'(' | b'[' | b'{' => {
                depth += 1;
                if depth > BRACKET_CAP {
                    return Err(PreFail::Bracket(i));
                }
            }
            b')' | b']' | b'}' => {
                depth = depth.saturating_sub(1);
            }
            _ => {}
        }
        prev = c;
        i += 1;
    }
    Ok(())
}

/// Scan a numeric literal at b[*i]; advance past it. Err classifies the
/// rejection the way CPython's tokenizer does (trailing letters,
/// leading-zero decimals, bad base digits, doubled underscores, and the
/// imaginary-suffix follow rule `1j2`).
fn scan_number(b: &[u8], i: &mut usize) -> Result<(), NumFail> {
    // Leading-dot fractions (`.5`, `x.5` attribute included). A consumed
    // leading dot means there IS no integer part, so the leading-zero rule
    // below must not fire on the fraction (`.075` is valid; the old code
    // read `075` as a zero-led int and errored).
    let leading_dot = b[*i] == b'.';
    if leading_dot {
        *i += 1;
    }
    // Base-prefixed integers.
    if b[*i] == b'0'
        && matches!(b.get(*i + 1), Some(b'x') | Some(b'X') | Some(b'o') | Some(b'O') | Some(b'b') | Some(b'B'))
    {
        let (ok, plain): (fn(u8) -> bool, NumFail) = match b[*i + 1] {
            b'x' | b'X' => (|c: u8| c.is_ascii_hexdigit(), NumFail::Hex),
            b'o' | b'O' => (
                |c: u8| matches!(c, b'0'..=b'7'),
                NumFail::Oct { digit: None },
            ),
            _ => (|c: u8| c == b'0' || c == b'1', NumFail::Bin { digit: None }),
        };
        let digit_fail = |d: u8| -> NumFail {
            if d.is_ascii_digit() {
                match &plain {
                    NumFail::Oct { .. } => NumFail::Oct {
                        digit: Some(d as char),
                    },
                    NumFail::Bin { .. } => NumFail::Bin {
                        digit: Some(d as char),
                    },
                    _ => plain_name(&plain),
                }
            } else {
                plain_name(&plain)
            }
        };
        *i += 2;
        // First char after the prefix must be a base digit; a single `_`
        // is allowed when a base digit follows (`0x_1` parses, `0x__1` and
        // a trailing `_` do not).
        match b.get(*i) {
            Some(&d) if ok(d) => *i += 1,
            Some(&b'_') if matches!(b.get(*i + 1), Some(n) if ok(*n)) => *i += 1,
            _ => return Err(digit_fail(*b.get(*i).unwrap_or(&b'_'))),
        }
        let mut last_us = false;
        while *i < b.len() && (ok(b[*i]) || b[*i] == b'_') {
            if b[*i] == b'_' && last_us {
                return Err(plain_name(&plain));
            }
            last_us = b[*i] == b'_';
            *i += 1;
        }
        if last_us {
            return Err(plain_name(&plain)); // trailing underscore
        }
        return match b.get(*i) {
            Some(d) if d.is_ascii_alphanumeric() || *d == b'_' => Err(digit_fail(*d)),
            _ => Ok(()),
        };
    }
/// Strip a digit-interpolation back to the plain message (doubled/trailing
/// underscores and alpha tails never name the digit).
fn plain_name(f: &NumFail) -> NumFail {
    match f {
        NumFail::Oct { .. } => NumFail::Oct { digit: None },
        NumFail::Bin { .. } => NumFail::Bin { digit: None },
        NumFail::Hex => NumFail::Hex,
        NumFail::Dec => NumFail::Dec,
        NumFail::Imag => NumFail::Imag,
        NumFail::LeadingZero => NumFail::LeadingZero,
    }
}
    // Leading zeros are rejected in plain ints (`0755`, `0_7`) — but not
    // in all-zero literals (`0`, `00`) — and a fraction, exponent, or
    // imaginary suffix rescues the token (`001.5`, `0755e3`, `01j` parse).
    // Classify by lookahead. Skipped wholesale after a leading dot: the
    // fraction has no integer part to lead-zero (`leading_dot` above).
    if !leading_dot
        && b[*i] == b'0'
        && matches!(b.get(*i + 1), Some(d) if d.is_ascii_digit() || *d == b'_')
    {
        let mut j = *i + 1;
        let mut nonzero = false;
        while j < b.len() && (b[j].is_ascii_digit() || b[j] == b'_') {
            nonzero = nonzero || matches!(b[j], b'1'..=b'9');
            j += 1;
        }
        let rescued = nonzero
            && ((b.get(j) == Some(&b'.')
                && matches!(b.get(j + 1), Some(d) if d.is_ascii_digit()))
                || matches!(b.get(j), Some(b'e') | Some(b'E'))
                || matches!(b.get(j), Some(b'j') | Some(b'J')));
        if nonzero && !rescued {
            return Err(NumFail::LeadingZero);
        }
    }
    let mut last_us = false;
    let mut any_digit = false;
    while *i < b.len() && (b[*i].is_ascii_digit() || b[*i] == b'_') {
        if b[*i] == b'_' && (last_us || !any_digit) {
            return Err(NumFail::Dec);
        }
        last_us = b[*i] == b'_';
        any_digit = any_digit || b[*i].is_ascii_digit();
        *i += 1;
    }
    if last_us {
        return Err(NumFail::Dec);
    }
    // Fraction. The first char after `.` must be a digit (`1._5` is
    // rejected; underscores only separate digits).
    if b.get(*i) == Some(&b'.')
        && matches!(b.get(*i + 1), Some(d) if d.is_ascii_digit() || *d == b'_')
    {
        *i += 1;
        if !matches!(b.get(*i), Some(d) if d.is_ascii_digit()) {
            return Err(NumFail::Dec);
        }
        last_us = false;
        while *i < b.len() && (b[*i].is_ascii_digit() || b[*i] == b'_') {
            if b[*i] == b'_' && last_us {
                return Err(NumFail::Dec);
            }
            last_us = b[*i] == b'_';
            *i += 1;
        }
        if last_us {
            return Err(NumFail::Dec);
        }
    }
    // Exponent.
    if matches!(b.get(*i), Some(b'e') | Some(b'E')) {
        let mut j = *i + 1;
        if matches!(b.get(j), Some(b'+') | Some(b'-')) {
            j += 1;
        }
        if !matches!(b.get(j), Some(d) if d.is_ascii_digit()) {
            return Err(NumFail::Dec);
        }
        last_us = false;
        while j < b.len() && (b[j].is_ascii_digit() || b[j] == b'_') {
            if b[j] == b'_' && last_us {
                return Err(NumFail::Dec);
            }
            last_us = b[j] == b'_';
            j += 1;
        }
        if last_us {
            return Err(NumFail::Dec);
        }
        *i = j;
    }
    // Imaginary suffix. A digit (or letter/underscore) after the suffix is
    // a different token shape CPython rejects (`1j2` is invalid).
    if matches!(b.get(*i), Some(b'j') | Some(b'J')) {
        *i += 1;
        if matches!(b.get(*i), Some(d) if d.is_ascii_alphanumeric() || *d == b'_') {
            return Err(NumFail::Imag);
        }
        return Ok(());
    }
    if matches!(b.get(*i), Some(d) if d.is_ascii_alphabetic() || *d == b'_') {
        return Err(NumFail::Dec);
    }
    Ok(())
}

/// Skip a quoted string starting at the opening quote in b[q]. Brackets
/// inside f-string replacement fields are real code and counted; plain
/// strings contribute nothing. Returns the index past the closing quote.
fn skip_string(b: &[u8], q: usize, is_f: bool, depth: &mut usize) -> usize {
    let quote = b[q];
    let triple = b.get(q + 1) == Some(&quote) && b.get(q + 2) == Some(&quote);
    let mut i = q + if triple { 3 } else { 1 };
    let mut field_depth = 0u32;
    while i < b.len() {
        let c = b[i];
        if c == b'\\' {
            i += 2;
            continue;
        }
        if is_f && c == b'{' && field_depth == 0 {
            if b.get(i + 1) == Some(&b'{') {
                i += 2; // escaped {{
                continue;
            }
            field_depth += 1;
            i += 1;
            continue;
        }
        if field_depth > 0 {
            if c == b'}' {
                field_depth -= 1;
                i += 1;
                continue;
            }
            if c == b'\'' || c == b'"' {
                // Nested string inside a replacement field.
                i = skip_string(b, i, false, depth);
                continue;
            }
            match c {
                b'(' | b'[' | b'{' => *depth += 1,
                b')' | b']' => *depth = depth.saturating_sub(1),
                _ => {}
            }
            i += 1;
            continue;
        }
        if c == quote {
            if triple {
                if b.get(i + 1) == Some(&quote) && b.get(i + 2) == Some(&quote) {
                    return i + 3;
                }
                i += 1;
                continue;
            }
            return i + 1;
        }
        if !triple && c == b'\n' {
            return i; // unterminated short string: resume code scanning
        }
        i += 1;
    }
    i
}
