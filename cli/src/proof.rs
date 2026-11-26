//! Report lines with an evidence field.
//!
//! A report line looks like `S:done|P:2/2|T:12s|F:backend|E:-|V:cargo test 41/41`. The fields are S (state),
//! P (parts done/total), T (time), F (focus), E (error) and the optional V (evidence: a command and its result).
//! `accept` rejects `S:done` without a non-empty V. Every other state passes.
//!
//! Escape rule: a field is separated from the next by `|`. Inside a value, `\|` stands for `|`, `\\` for `\`
//! and `\n` for a line break. A `\` followed by any other character is an error. `render` always writes this form,
//! so `parse(render(r)) == r`.
//!
//! `clusters::report` writes every report line with `render`, and `clusters::run_cluster` calls `accept` before it
//! writes `S:done`, so a done line without evidence cannot leave the cluster.

#[derive(Debug, Clone, PartialEq)]
pub struct Report {
    pub s: String,
    pub p: String,
    pub t: String,
    pub f: String,
    pub e: String,
    pub v: Option<String>,
}

fn escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '|' => out.push_str("\\|"),
            '\n' => out.push_str("\\n"),
            _ => out.push(c),
        }
    }
    out
}

/// Splits a line at every `|` that is not escaped and resolves the escapes in each piece.
fn split_fields(line: &str) -> Result<Vec<String>, String> {
    let mut fields = Vec::new();
    let mut cur = String::new();
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => match chars.next() {
                Some('|') => cur.push('|'),
                Some('\\') => cur.push('\\'),
                Some('n') => cur.push('\n'),
                Some(o) => return Err(format!("unknown escape \\{}", o)),
                None => return Err("line ends with a single backslash".to_string()),
            },
            '|' => fields.push(std::mem::take(&mut cur)),
            _ => cur.push(c),
        }
    }
    fields.push(cur);
    Ok(fields)
}

/// Reads one report line. S, P, T, F and E are required, V is optional, each key may appear once.
pub fn parse(line: &str) -> Result<Report, String> {
    let line = line.trim_end_matches(['\n', '\r']);
    let (mut s, mut p, mut t, mut f, mut e, mut v): (Option<String>, Option<String>, Option<String>, Option<String>, Option<String>, Option<String>) =
        (None, None, None, None, None, None);
    for field in split_fields(line)? {
        let (key, value) = field.split_once(':').ok_or_else(|| format!("field without a key: {}", field))?;
        let slot = match key {
            "S" => &mut s,
            "P" => &mut p,
            "T" => &mut t,
            "F" => &mut f,
            "E" => &mut e,
            "V" => &mut v,
            other => return Err(format!("unknown key {}", other)),
        };
        if slot.is_some() {
            return Err(format!("key {} appears twice", key));
        }
        *slot = Some(value.to_string());
    }
    let need = |o: Option<String>, k: &str| o.ok_or_else(|| format!("missing field {}", k));
    Ok(Report { s: need(s, "S")?, p: need(p, "P")?, t: need(t, "T")?, f: need(f, "F")?, e: need(e, "E")?, v })
}

/// Writes the line in the form `parse` reads, without a line break at the end.
pub fn render(r: &Report) -> String {
    let mut out = format!("S:{}|P:{}|T:{}|F:{}|E:{}", escape(&r.s), escape(&r.p), escape(&r.t), escape(&r.f), escape(&r.e));
    if let Some(v) = &r.v {
        out.push_str(&format!("|V:{}", escape(v)));
    }
    out
}

/// A report with state `done` needs a V with at least one non-space character. Other states pass.
pub fn accept(r: &Report) -> Result<(), String> {
    if r.s == "done" && r.v.as_deref().map(|v| v.trim().is_empty()).unwrap_or(true) {
        return Err("S:done without evidence: the V field is missing or empty".to_string());
    }
    Ok(())
}


/// The Kerkis status line. A cluster reports one line: `KRK:1|C:c2|S:working|P:4/9|T:312|F:f0412:88|E:TLSTimeout|V:-`.
/// The harness sets C, P, T and V. The model chooses S, F and E. E comes from a fixed list of at most 256 codes, so a
/// free text error cannot pass the grammar. `model_bits` counts only the fields the model chooses.
pub const KRK_VERSION: &str = "1";

/// The fixed error codes. The owner approves this list before the field is used; a new code needs a new entry here.
/// At most 256 codes fit the 8-bit E field.
pub const E_CODES: [&str; 25] = [
    "Ok", "Scope", "Spec", "Tests", "Build", "Dependency", "Timeout", "Syntax",
    "Permission", "Budget", "ToolError", "NoTermination", "AskLoop", "StatusGrammar", "Stale", "Blocked",
    "MissingInput", "Conflict", "Network", "RateLimit", "NotFound", "InvalidContract", "Stuck", "Unknown", "TLSTimeout",
];

/// The states the model may write in S.
pub const S_STATES: [&str; 5] = ["working", "done", "blocked", "stuck", "waiting"];

#[derive(Debug, Clone, PartialEq)]
pub struct KerkisLine {
    pub c: String,
    pub s: String,
    pub p_done: usize,
    pub p_total: usize,
    pub t: u64,
    pub f: String,
    pub e: String,
    pub v: String,
}

impl KerkisLine {
    /// Builds a line with P, T and V from the harness. The caller passes the model's S, F and E.
    #[allow(clippy::too_many_arguments)]
    pub fn new(c: &str, s: &str, done: usize, total: usize, t: u64, f: &str, e: &str, v: &str) -> Result<KerkisLine, String> {
        let line = KerkisLine { c: c.trim().to_string(), s: s.trim().to_string(), p_done: done, p_total: total,
                                t, f: f.trim().to_string(), e: e.trim().to_string(), v: v.trim().to_string() };
        check_kerkis(&line)?;
        Ok(line)
    }

    /// The bits the model chooses: S 2.32, F 30.29 at 10,000 files, E 8. P, T, C and V are harness fields.
    pub fn model_bits(&self) -> f64 { 2.32 + 30.29 + 8.0 }

    /// The bits of the protocol version in Paper 35: S + P + F + E. Kept for the comparison in the paper.
    pub fn model_bits_with_p(&self) -> f64 { 2.32 + 19.93 + 30.29 + 8.0 }
}

fn e_code(value: &str) -> Option<&'static str> {
    E_CODES.iter().copied().find(|code| *code == value)
}

fn digits(value: &str, max: usize) -> Option<usize> {
    if value.is_empty() || !value.chars().all(|c| c.is_ascii_digit()) { return None; }
    let n: usize = value.parse().ok()?;
    (n <= max).then_some(n)
}

fn file_id(value: &str) -> bool {
    let Some((file, line)) = value.split_once(':') else { return false };
    let Some(number) = file.strip_prefix('f') else { return false };
    digits(number, 99_999).is_some() && digits(line, 1 << 17).is_some()
}

fn check_kerkis(line: &KerkisLine) -> Result<(), String> {
    if line.c.is_empty() || line.c.contains(['|', '\n', '\r']) { return Err("C: a cluster id without | or a line break".to_string()); }
    if !S_STATES.contains(&line.s.as_str()) { return Err(format!("S: {} is not one of {}", line.s, S_STATES.join(", "))); }
    if line.p_done > line.p_total || line.p_total > 999 { return Err("P: done/total must satisfy done <= total <= 999".to_string()); }
    if !file_id(&line.f) { return Err("F: a file id f<number>:<line> with line <= 2^17".to_string()); }
    if e_code(&line.e).is_none() { return Err(format!("StatusGrammar: E:{} is not in the fixed list", line.e)); }
    if line.v.is_empty() { return Err("V: missing; the harness sets an event id or -".to_string()); }
    Ok(())
}

/// Reads one Kerkis line. The keys must be KRK, C, S, P, T, F, E, V in this order, each once. A free text in E is a
/// grammar error, not a value.
pub fn parse_kerkis(line: &str) -> Result<KerkisLine, String> {
    let line = line.trim_end_matches(['\n', '\r']);
    let parts: Vec<&str> = line.split('|').collect();
    if parts.len() != 8 { return Err(format!("StatusGrammar: expected 8 fields, got {}", parts.len())); }
    let keys = ["KRK", "C", "S", "P", "T", "F", "E", "V"];
    let mut values: Vec<&str> = Vec::new();
    for (part, key) in parts.iter().zip(keys) {
        let Some((k, v)) = part.split_once(':') else { return Err(format!("StatusGrammar: field {} has no key", part)); };
        if k != key { return Err(format!("StatusGrammar: expected {}, got {}", key, k)); }
        values.push(v);
    }
    if values[0] != KRK_VERSION { return Err(format!("StatusGrammar: KRK:{} is not supported", values[0])); }
    let (p_done, p_total) = values[3].split_once('/').ok_or_else(|| "StatusGrammar: P must be done/total".to_string())?;
    let p_done = digits(p_done, 999).ok_or_else(|| "StatusGrammar: P done is not a number <= 999".to_string())?;
    let p_total = digits(p_total, 999).ok_or_else(|| "StatusGrammar: P total is not a number <= 999".to_string())?;
    let t: u64 = values[4].parse().map_err(|_| "StatusGrammar: T is not a number of seconds".to_string())?;
    let out = KerkisLine { c: values[1].to_string(), s: values[2].to_string(), p_done, p_total, t,
                           f: values[5].to_string(), e: values[6].to_string(), v: values[7].to_string() };
    check_kerkis(&out)?;
    Ok(out)
}

pub fn render_kerkis(line: &KerkisLine) -> String {
    format!("KRK:{}|C:{}|S:{}|P:{}/{}|T:{}|F:{}|E:{}|V:{}", KRK_VERSION, line.c, line.s, line.p_done,
            line.p_total, line.t, line.f, line.e, line.v)
}

/// `S:done` needs a V that points to a real evidence event, not `-`. Other states pass with `-`.
pub fn accept_kerkis(line: &KerkisLine) -> Result<(), String> {
    if line.s == "done" && (line.v.trim().is_empty() || line.v == "-") {
        return Err("S:done without evidence: V must name an event, not -".to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_line_without_evidence() {
        let r = parse("S:working|P:1/2|T:12s|F:backend|E:-").unwrap();
        assert_eq!(r.s, "working");
        assert_eq!(r.p, "1/2");
        assert_eq!(r.t, "12s");
        assert_eq!(r.f, "backend");
        assert_eq!(r.e, "-");
        assert_eq!(r.v, None);
    }

    #[test]
    fn parses_evidence() {
        let r = parse("S:done|P:2/2|T:12s|F:backend|E:-|V:cargo test 41/41\n").unwrap();
        assert_eq!(r.v.as_deref(), Some("cargo test 41/41"));
    }

    #[test]
    fn done_without_evidence_is_rejected() {
        let r = parse("S:done|P:2/2|T:12s|F:-|E:-").unwrap();
        assert!(accept(&r).is_err());
        let r = parse("S:done|P:2/2|T:12s|F:-|E:-|V:   ").unwrap();
        assert!(accept(&r).is_err());
    }

    #[test]
    fn done_with_evidence_passes() {
        let r = parse("S:done|P:2/2|T:12s|F:-|E:-|V:cat hello.txt ok").unwrap();
        assert!(accept(&r).is_ok());
    }

    #[test]
    fn other_states_pass_without_evidence() {
        for s in ["queued", "working", "validating", "failed", "killing"] {
            let r = parse(&format!("S:{}|P:0/2|T:1s|F:-|E:-", s)).unwrap();
            assert!(accept(&r).is_ok(), "{}", s);
        }
    }

    #[test]
    fn pipe_inside_a_value_survives_the_round_trip() {
        let r = Report {
            s: "done".into(), p: "1/1".into(), t: "3s".into(), f: "a|b".into(), e: "-".into(),
            v: Some("cargo test | tail: 41/41 \\ ok\nline two".into()),
        };
        let line = render(&r);
        assert!(line.contains("a\\|b"));
        assert!(!line.contains('\n'));
        assert_eq!(parse(&line).unwrap(), r);
    }

    #[test]
    fn escaped_pipe_does_not_split() {
        let r = parse("S:failed|P:0/1|T:1s|F:-|E:a\\|b\\|c").unwrap();
        assert_eq!(r.e, "a|b|c");
    }

    #[test]
    fn malformed_lines_are_errors() {
        assert!(parse("").is_err());
        assert!(parse("S:done|P:1/1").is_err());
        assert!(parse("S:done|P:1/1|T:1s|F:-|E:-|X:1").is_err());
        assert!(parse("S:done|S:done|P:1/1|T:1s|F:-|E:-").is_err());
        assert!(parse("S:done|P:1/1|T:1s|F:-|E:bad\\q").is_err());
        assert!(parse("S:done|P:1/1|T:1s|F:-|E:end\\").is_err());
    }

    #[test]
    fn round_trip_of_the_existing_line_format() {
        let line = "S:working|P:1/2|T:12s|F:backend|E:-";
        assert_eq!(render(&parse(line).unwrap()), line);
    }

    fn kerkis() -> KerkisLine {
        KerkisLine::new("c2", "working", 4, 9, 312, "f0412:88", "TLSTimeout", "-").unwrap()
    }

    #[test]
    fn kerkis_round_trip_and_bits() {
        let line = kerkis();
        let text = render_kerkis(&line);
        assert_eq!(text, "KRK:1|C:c2|S:working|P:4/9|T:312|F:f0412:88|E:TLSTimeout|V:-");
        assert_eq!(parse_kerkis(&text).unwrap(), line);
        assert!((line.model_bits() - 40.61).abs() < 0.001);
        assert!((line.model_bits_with_p() - 60.54).abs() < 0.001);
        assert!(line.model_bits() <= 77.2);
    }

    #[test]
    fn kerkis_done_needs_evidence() {
        let mut line = kerkis();
        line.s = "done".to_string();
        assert!(accept_kerkis(&line).is_err());
        line.v = "e7".to_string();
        assert!(accept_kerkis(&line).is_ok());
    }

    #[test]
    fn kerkis_rejects_free_text_in_e() {
        let text = "KRK:1|C:c2|S:working|P:4/9|T:312|F:f0412:88|E:the TLS handshake timed out|V:-";
        let err = parse_kerkis(text).unwrap_err();
        assert!(err.contains("StatusGrammar") && err.contains("fixed list"), "{}", err);
    }

    #[test]
    fn kerkis_rejects_bad_f_and_p() {
        assert!(parse_kerkis("KRK:1|C:c2|S:working|P:4/9|T:312|F:notafile|E:Timeout|V:-").is_err());
        assert!(parse_kerkis("KRK:1|C:c2|S:working|P:10/9|T:312|F:f0412:88|E:Timeout|V:-").is_err());
        assert!(parse_kerkis("KRK:2|C:c2|S:working|P:4/9|T:312|F:f0412:88|E:Timeout|V:-").is_err());
    }

    #[test]
    fn kerkis_code_list_fits_one_byte() {
        assert!(E_CODES.len() <= 256);
        assert!(E_CODES.iter().all(|c| !c.is_empty() && !c.contains('|')));
    }
}
