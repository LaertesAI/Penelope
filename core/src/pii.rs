//! Personal data in German business text. `spans` finds names, IBANs, postal addresses, phone numbers, e-mail
//! addresses, tax ids and birth dates with fixed rules, and `mask` replaces each span by `[masked:KIND]`.
//!
//! The trait `Detector` is the seam for a model. The rules implement it today. An Utis model implements it later by
//! returning masked text, and `privacy_bench` scores any implementation from that masked text alone, so the model
//! needs no span output and no change to the bench.

use regex::Regex;
use std::collections::HashSet;
use std::sync::OnceLock;

/// One personal item: byte range in the text and its kind (`name`, `iban`, `address`, `phone`, `email`, `tax_id`,
/// `birth_date`).
#[derive(Clone, Debug, PartialEq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
    pub kind: &'static str,
}

/// Anything that masks personal data. `mask` returns the text with each item replaced by `[masked:KIND]` and every
/// other character unchanged.
pub trait Detector {
    fn name(&self) -> String;
    fn mask(&self, text: &str) -> String;
}

/// The fixed rules of this file. The harness calls `mask` through `privacy::mask_pii`; this type is the same rules
/// behind the trait, for a caller that holds a `dyn Detector`.
#[allow(dead_code)]
pub struct Rules;

impl Detector for Rules {
    fn name(&self) -> String {
        "rules".to_string()
    }
    fn mask(&self, text: &str) -> String {
        mask(text).0
    }
}

/// The text with every span replaced by `[masked:KIND]`, and the number of spans.
pub fn mask(text: &str) -> (String, usize) {
    let found = spans(text);
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for s in &found {
        out.push_str(&text[last..s.start]);
        out.push_str("[masked:");
        out.push_str(s.kind);
        out.push(']');
        last = s.end;
    }
    out.push_str(&text[last..]);
    (out, found.len())
}

/// All personal items in the text, sorted, without overlaps. A kind found earlier in this list wins an overlap.
///
/// Text from the read and grep tools carries a prefix per line (`    5  ` or `src/a.rs:5: `), and Windows files end
/// lines with `\r\n`. Both break the rules that anchor at a line start or end. `spans` removes them first, runs the
/// rules on the plain text, and maps each span back to the bytes of the input.
pub fn spans(text: &str) -> Vec<Span> {
    match Plain::of(text) {
        None => plain_spans(text),
        Some(p) => plain_spans(&p.text).into_iter().map(|s| Span { start: p.start(s.start), end: p.end(s.end), kind: s.kind }).collect(),
    }
}

/// The text without line prefixes and without the `\r` of `\r\n`, with the map back to the input. `parts` holds
/// (start in `text`, start in the input, length) of each kept piece, in order.
struct Plain {
    text: String,
    parts: Vec<(usize, usize, usize)>,
}

/// The prefix of a line as the read tool writes it: the number right-aligned in five places, then two spaces. A line
/// number never starts with 0, so a phone number such as `0171  2345678` never looks like one.
fn read_prefix(line: &str) -> Option<usize> {
    let digits_at = line.len() - line.trim_start_matches(' ').len();
    let digits = line[digits_at..].bytes().take_while(u8::is_ascii_digit).count();
    let width = digits_at + digits;
    (digits > 0 && line.as_bytes()[digits_at] != b'0' && width == digits.max(5) && line[width..].starts_with("  ")).then_some(width + 2)
}

/// The prefix of a line as the grep tool writes it: `path:NUMBER: `.
fn grep_prefix(line: &str) -> Option<usize> {
    let mut from = 0;
    while let Some(k) = line[from..].find(':') {
        let at = from + k + 1;
        let digits = line[at..].bytes().take_while(u8::is_ascii_digit).count();
        if at > 1 && digits > 0 && line.as_bytes()[at] != b'0' && line[at + digits..].starts_with(": ") {
            return Some(at + digits + 2);
        }
        from = at;
    }
    None
}

impl Plain {
    fn of(text: &str) -> Option<Plain> {
        let crlf = text.contains("\r\n");
        let lines: Vec<&str> = text.split_inclusive('\n').collect();
        let bodies: Vec<&str> = lines.iter().map(|l| l.trim_end_matches(['\n', '\r'])).collect();
        let full: Vec<&&str> = bodies.iter().filter(|b| !b.is_empty()).collect();
        let pick = |f: fn(&str) -> Option<usize>| !full.is_empty() && full.iter().all(|b| f(b).is_some());
        let prefix: Option<fn(&str) -> Option<usize>> = if pick(read_prefix) { Some(read_prefix) } else if pick(grep_prefix) { Some(grep_prefix) } else { None };
        if !crlf && prefix.is_none() {
            return None;
        }
        let mut p = Plain { text: String::with_capacity(text.len()), parts: Vec::new() };
        let mut at = 0usize;
        for line in lines {
            let cut = prefix.and_then(|f| f(line.trim_end_matches(['\n', '\r']))).unwrap_or(0);
            let body_end = if line.ends_with("\r\n") { line.len() - 2 } else if line.ends_with('\n') { line.len() - 1 } else { line.len() };
            p.keep(text, at + cut, at + body_end);
            if line.ends_with('\n') {
                p.keep(text, at + line.len() - 1, at + line.len());
            }
            at += line.len();
        }
        Some(p)
    }
    fn keep(&mut self, input: &str, from: usize, to: usize) {
        if from >= to {
            return;
        }
        match self.parts.last_mut() {
            Some((ps, po, pl)) if *po + *pl == from && *ps + *pl == self.text.len() => *pl += to - from,
            _ => self.parts.push((self.text.len(), from, to - from)),
        }
        self.text.push_str(&input[from..to]);
    }
    fn part(&self, pos: usize) -> &(usize, usize, usize) {
        &self.parts[self.parts.partition_point(|p| p.0 <= pos).saturating_sub(1)]
    }
    fn start(&self, pos: usize) -> usize {
        let (ps, po, _) = self.part(pos);
        po + (pos - ps)
    }
    fn end(&self, pos: usize) -> usize {
        let (ps, po, _) = self.part(pos - 1);
        po + (pos - 1 - ps) + 1
    }
}

fn plain_spans(text: &str) -> Vec<Span> {
    let mut taken = Taken::default();
    emails(text, &mut taken);
    ibans(text, &mut taken);
    tax_ids(text, &mut taken);
    phones(text, &mut taken);
    birth_dates(text, &mut taken);
    tables(text, &mut taken);
    addresses(text, &mut taken);
    names(text, &mut taken);
    taken.0.into_iter().map(|(start, (end, kind))| Span { start, end, kind }).collect()
}

/// Accepted spans by start. They never overlap, so a new range overlaps one only if the span starting last before
/// its end reaches into it. A new span that overlaps is dropped.
#[derive(Default)]
struct Taken(std::collections::BTreeMap<usize, (usize, &'static str)>);

impl Taken {
    fn free(&self, start: usize, end: usize) -> bool {
        self.0.range(..end).next_back().map(|(_, (e, _))| *e <= start).unwrap_or(true)
    }
    fn add(&mut self, start: usize, end: usize, kind: &'static str) -> bool {
        if start >= end || !self.free(start, end) {
            return false;
        }
        self.0.insert(start, (end, kind));
        true
    }
    /// Is exactly this range already one span of this kind?
    fn is(&self, start: usize, end: usize, kind: &str) -> bool {
        self.0.get(&start).map(|(e, k)| *e == end && *k == kind).unwrap_or(false)
    }
    /// Adds a span that may cover spans of the same kind inside it: "O. Rehberg" over the "Rehberg" a rule found first.
    fn grow(&mut self, start: usize, end: usize, kind: &'static str) -> bool {
        let inner: Vec<usize> = self.0.range(start..end).filter(|(_, (e, k))| *e <= end && *k == kind).map(|(s, _)| *s).collect();
        let removed: Vec<(usize, (usize, &'static str))> = inner.iter().map(|s| (*s, self.0.remove(s).unwrap())).collect();
        if self.add(start, end, kind) {
            return true;
        }
        self.0.extend(removed);
        false
    }
}

/// The token that holds byte `pos`, found by binary search: tokens are sorted and do not overlap.
fn tok_at(toks: &[Tok], pos: usize) -> Option<usize> {
    let k = toks.partition_point(|t| t.s <= pos).checked_sub(1)?;
    (pos < toks[k].e).then_some(k)
}

fn re(cell: &'static OnceLock<Regex>, pattern: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pattern).unwrap())
}

fn is_delim(c: char) -> bool {
    c.is_whitespace() || matches!(c, '"' | '\'' | ',' | ';' | '(' | ')' | '<' | '>' | '`' | '[' | ']' | '{' | '}' | '„' | '“' | '”' | '‚' | '‘' | '’')
}

/// Whitespace-separated words with the outer punctuation cut off: (start, end) of the core.
fn cores(text: &str) -> Vec<(usize, usize)> {
    static WORD: OnceLock<Regex> = OnceLock::new();
    let mut out = Vec::new();
    for m in re(&WORD, r"\S+").find_iter(text) {
        let w = m.as_str();
        let lead = w.len() - w.trim_start_matches(is_delim).len();
        let core = w.trim_start_matches(is_delim).trim_end_matches(|c: char| is_delim(c) || matches!(c, '.' | ':' | '!' | '?'));
        if !core.is_empty() {
            out.push((m.start() + lead, m.start() + lead + core.len()));
        }
    }
    out
}

// ---------------------------------------------------------------- e-mail

fn is_email(w: &str) -> bool {
    let Some((local, domain)) = w.split_once('@') else { return false };
    !local.is_empty() && !domain.contains('@') && domain.contains('.') && !domain.starts_with('.') && !domain.ends_with('.')
        && local.chars().all(|c| c.is_alphanumeric() || matches!(c, '.' | '_' | '-' | '+'))
        && domain.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-'))
}

fn emails(text: &str, taken: &mut Taken) {
    for (s, e) in cores(text) {
        if is_email(&text[s..e]) {
            taken.add(s, e, "email");
        }
    }
}

// ---------------------------------------------------------------- IBAN

/// Length of an IBAN by country, for the countries a German business text names most.
fn iban_length(country: &str) -> Option<usize> {
    Some(match country {
        "DE" | "GB" | "IE" => 22,
        "AT" | "LU" | "LT" | "EE" | "KZ" => 20,
        "CH" | "LI" | "LV" => 21,
        "NL" => 18,
        "BE" => 16,
        "DK" | "FI" | "FO" | "GL" => 18,
        "NO" => 15,
        "SE" | "ES" | "CZ" | "SK" | "RO" | "TN" => 24,
        "FR" | "IT" | "MC" | "SM" => 27,
        "PL" | "HU" | "CY" => 28,
        "PT" => 25,
        "GR" => 27,
        "SI" => 19,
        "HR" => 21,
        "TR" => 26,
        _ => return None,
    })
}

/// ISO 13616: the first four characters move to the end, letters become 10 to 35, the number mod 97 must be 1.
pub fn iban_checksum_ok(iban: &str) -> bool {
    let b = iban.as_bytes();
    if b.len() < 15 || b.len() > 34 || !b[..2].iter().all(u8::is_ascii_uppercase) || !b[2..4].iter().all(u8::is_ascii_digit)
        || !b.iter().all(|c| c.is_ascii_digit() || c.is_ascii_uppercase()) {
        return false;
    }
    let mut rest: u32 = 0;
    for &c in b[4..].iter().chain(b[..4].iter()) {
        if c.is_ascii_digit() {
            rest = (rest * 10 + (c - b'0') as u32) % 97;
        } else {
            rest = (rest * 100 + (c - b'A' + 10) as u32) % 97;
        }
    }
    rest == 1
}

fn iban_ok(compact: &str) -> bool {
    match iban_length(&compact[..2]) {
        Some(n) => compact.len() == n && iban_checksum_ok(compact),
        None => iban_checksum_ok(compact),
    }
}

/// The word "IBAN" or "Konto" shortly before: then an IBAN-shaped string counts even with a wrong check sum.
fn iban_context(text: &str, start: usize) -> bool {
    let from = floor(text, start.saturating_sub(24));
    let before = text[from..start].to_lowercase();
    before.contains("iban") || before.contains("konto")
}

fn ibans(text: &str, taken: &mut Taken) {
    static ONE: OnceLock<Regex> = OnceLock::new();
    static GROUPS: OnceLock<Regex> = OnceLock::new();
    for (s, e) in cores(text) {
        let w = &text[s..e];
        if re(&ONE, r"^[A-Z]{2}\d{2}[A-Z0-9]{11,30}$").is_match(w) && (iban_ok(w) || iban_context(text, s)) {
            taken.add(s, e, "iban");
        }
    }
    // in groups of four: DE89 3704 0044 0532 0130 00
    for m in re(&GROUPS, r"\b[A-Z]{2}\d{2}(?: [A-Z0-9]{1,4}){2,8}").find_iter(text) {
        let parts: Vec<(usize, &str)> = {
            let mut v = Vec::new();
            let mut at = m.start();
            for p in m.as_str().split(' ') {
                v.push((at, p));
                at += p.len() + 1;
            }
            v
        };
        let mut compact = String::new();
        let mut best: Option<usize> = None;
        let mut loose: Option<usize> = None;
        let want = iban_length(&m.as_str()[..2]);
        for (k, (at, p)) in parts.iter().enumerate() {
            compact.push_str(p);
            let end = at + p.len();
            let boundary = text[end..].chars().next().map(|c| !c.is_ascii_alphanumeric()).unwrap_or(true);
            if !boundary || k == 0 || compact.len() > 34 {
                continue;
            }
            if iban_ok(&compact) {
                best = Some(end);
            }
            let fits = match want { Some(n) => compact.len() == n, None => compact.len() >= 15 };
            if fits {
                loose = Some(end);
            }
        }
        let end = best.or_else(|| if iban_context(text, m.start()) { loose } else { None });
        if let Some(end) = end {
            taken.add(m.start(), end, "iban");
        }
    }
}

fn floor(text: &str, mut i: usize) -> usize {
    while i > 0 && !text.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// The text of the same line before `at`, at most `max` bytes.
fn line_before(text: &str, at: usize, max: usize) -> &str {
    let from = floor(text, at.saturating_sub(max));
    let from = text[from..at].rfind('\n').map(|i| from + i + 1).unwrap_or(from);
    &text[from..at]
}

// ---------------------------------------------------------------- phone

fn phones(text: &str, taken: &mut Taken) {
    static PHONE: OnceLock<Regex> = OnceLock::new();
    static CONTEXT: OnceLock<Regex> = OnceLock::new();
    static NUMBER: OnceLock<Regex> = OnceLock::new();
    // area code, then groups split by a space, a slash or a hyphen, also with spaces around them: "0861 / 20 55 88",
    // "(0 63 32) 8 91 45", "+49 (0)40 123456-0"
    let phone = re(&PHONE, r"(?:(?:\+|\b00)\d{2} ?(?:\(0\) ?)?\d|\(0[\d ]{1,7}\d\)|\b0 ?\d)(?:\d| ?[ /-] ?\d| \d){5,22}");
    let context = re(&CONTEXT, r"(?i)(?:tel|telefon|fon|phone|mobil|handy|fax|rufnummer|erreichbar|anrufen|durchwahl|whatsapp)");
    let number = re(&NUMBER, r"(?i)(?:nr|nummer|art|bestell|kunden|rechnung|az|aktenzeichen|vertrag|personal|auftrag|beleg|ticket|vorgang|order|id)\.?\s*:?\s*$");
    for m in phone.find_iter(text) {
        let digits = m.as_str().chars().filter(|c| c.is_ascii_digit()).count();
        if !(9..=15).contains(&digits) {
            continue;
        }
        if text[..m.start()].chars().next_back().map(|c| c.is_alphanumeric() || c == '/' || c == '.').unwrap_or(false) {
            continue;
        }
        if text[m.end()..].chars().next().map(|c| c.is_alphanumeric()).unwrap_or(false) {
            continue;
        }
        let before = line_before(text, m.start(), 30);
        let has_context = context.is_match(before);
        let separated = m.as_str().contains([' ', '/', '-', '(']) || m.as_str().starts_with('+');
        if !separated && !has_context {
            continue;
        }
        if number.is_match(before) && !has_context {
            continue;
        }
        taken.add(m.start(), m.end(), "phone");
    }
}

// ---------------------------------------------------------------- tax id

/// The German tax id (Steuer-Identifikationsnummer): 11 digits, the first not 0, the last a check digit by ISO 7064
/// MOD 11,10.
pub fn tax_id_ok(digits: &str) -> bool {
    let d: Vec<u32> = digits.chars().filter_map(|c| c.to_digit(10)).collect();
    if d.len() != 11 || d[0] == 0 || digits.chars().any(|c| !c.is_ascii_digit()) {
        return false;
    }
    let mut product = 10;
    for &x in &d[..10] {
        let mut sum = (x + product) % 10;
        if sum == 0 {
            sum = 10;
        }
        product = (sum * 2) % 11;
    }
    let check = (11 - product) % 10;
    check == d[10]
}

fn tax_ids(text: &str, taken: &mut Taken) {
    static TAX: OnceLock<Regex> = OnceLock::new();
    static CONTEXT: OnceLock<Regex> = OnceLock::new();
    let context = re(&CONTEXT, r"(?i)(?:steuer|idnr|id-nr|identifikations|tin\b)");
    for m in re(&TAX, r"\b[1-9]\d(?: ?\d{3}){3}\b").find_iter(text) {
        let compact: String = m.as_str().chars().filter(|c| c.is_ascii_digit()).collect();
        if !tax_id_ok(&compact) {
            continue;
        }
        // "+49 30 123 456 789" is a phone number, not a tax id
        let before = line_before(text, m.start(), 8);
        if before.ends_with(|c: char| c.is_ascii_digit() || c == '+' || c == '/' || c == '-' || c == ')')
            || before.trim_end().ends_with(|c: char| c.is_ascii_digit() || c == ')') {
            continue;
        }
        let grouped = m.as_str().contains(' ');
        if grouped || context.is_match(line_before(text, m.start(), 60)) {
            taken.add(m.start(), m.end(), "tax_id");
        }
    }
}

// ---------------------------------------------------------------- birth date

fn birth_dates(text: &str, taken: &mut Taken) {
    static DATE: OnceLock<Regex> = OnceLock::new();
    static CONTEXT: OnceLock<Regex> = OnceLock::new();
    let date = re(&DATE, r"\b(?:\d{1,2}\. ?\d{1,2}\. ?(?:19|20)\d{2}|\d{1,2}\. ?(?:Januar|Februar|März|Maerz|April|Mai|Juni|Juli|August|September|Oktober|November|Dezember|Jan\.|Feb\.|Mär\.|Apr\.|Jun\.|Jul\.|Aug\.|Sep\.|Sept\.|Okt\.|Nov\.|Dez\.) (?:19|20)\d{2}|(?:19|20)\d{2}-\d{2}-\d{2})\b");
    let context = re(&CONTEXT, r"(?i)(?:(?:geb\.|geboren|geburtsdatum|geburtstag|geb\.-datum|\bgeb\b)[^\n]{0,14}|(?:^|[\s(,;])\* ?)$");
    for m in date.find_iter(text) {
        // "Ich bin am 02.02.1998 geboren": the word may also follow the date
        let after = text[m.end()..].trim_start_matches([' ', '\t']);
        if context.is_match(line_before(text, m.start(), 40)) || after.starts_with("geboren") {
            taken.add(m.start(), m.end(), "birth_date");
        }
    }
}

// ---------------------------------------------------------------- tables

fn column_kind_known(h: &str) -> bool {
    matches!(h, "steuer-id (idnr)")
}

/// The kind of a table column by its header: "Nachname" holds names, "PLZ" addresses, "Geburtsdatum" birth dates.
fn column_kind(header: &str) -> Option<&'static str> {
    let h = header.trim().trim_matches(['"', '*', '\'']).to_lowercase().replace('_', "-");
    let h = match h.find(" (") { Some(k) if !column_kind_known(&h) => h[..k].to_string(), _ => h };
    let h = h.as_str();
    Some(match h {
        "name" | "vorname" | "nachname" | "familienname" | "rufname" | "mitarbeiter" | "mitarbeiterin" | "mitarbeitende" | "kunde" | "kundin"
        | "ansprechpartner" | "ansprechpartnerin" | "teilnehmer" | "teilnehmerin" | "empfänger" | "kontoinhaber" | "person" | "vor- und nachname"
        | "name, vorname" | "nachname, vorname" | "bewerber" | "bewerberin" | "mieter" | "mieterin" | "patient" | "patientin" | "teilnahme"
        | "teilnehmende" | "verantwortlich" | "zuständig" | "ansprechperson" | "kontakt" | "mitarbeiter/in" | "mitarbeiter:in" | "beschäftigte"
        | "unterschrift" | "wer" => "name",
        "straße" | "strasse" | "str." | "straße und hausnummer" | "straße, hausnummer" | "anschrift" | "adresse" | "plz" | "postleitzahl" | "ort"
        | "wohnort" | "stadt" | "plz ort" | "plz/ort" | "plz, ort" | "plz-ort" | "hausnummer" | "hausnr." | "nr." => "address",
        "telefon" | "tel" | "tel." | "mobil" | "handy" | "fax" | "telefonnummer" | "rufnummer" | "mobilnummer" | "telefon privat" => "phone",
        "e-mail" | "email" | "mail" | "e-mail-adresse" | "e-mail privat" => "email",
        "geburtsdatum" | "geb." | "geboren" | "geburtstag" | "geb.-datum" | "geb.datum" => "birth_date",
        "iban" | "bankverbindung" | "konto" | "kontonummer" => "iban",
        "steuer-id" | "steuerid" | "steuer-idnr" | "steuer-idnr." | "steuer-identifikationsnummer" | "idnr" | "steuerliche id" | "steuer-id (idnr)"
        | "idnr." | "tin" => "tax_id",
        _ => return None,
    })
}

/// Cells of a CSV, TSV or Markdown table under a personal column. A header row with two or more known columns starts a
/// table; it ends at a blank line or a row with another number of cells.
fn tables(text: &str, taken: &mut Taken) {
    let mut kinds: Vec<Option<&'static str>> = Vec::new();
    // a plain "Name" as the only personal column also heads tables of tools and providers (`| Name | Kind |`): then a
    // cell counts only when its words look like a person's name, so `claude-cli` and `parser` stay
    let mut strict = false;
    let mut sep = ';';
    let mut at = 0usize;
    for line in text.split_inclusive('\n') {
        let start = at;
        at += line.len();
        let body = line.trim_end_matches(['\n', '\r']);
        if !kinds.is_empty() {
            let cells: Vec<&str> = body.split(sep).collect();
            if body.trim().is_empty() || cells.len() != kinds.len() {
                kinds.clear();
            } else if body.chars().all(|c| matches!(c, '|' | '-' | ':' | ' ' | '+')) {
                continue; // the rule line of a Markdown table
            } else {
                let mut off = start;
                for (cell, kind) in cells.iter().zip(kinds.iter()) {
                    if let Some(kind) = kind {
                        let lead = cell.len() - cell.trim_start().len();
                        let core = cell.trim().trim_matches('"');
                        let s = off + cell.find(core).unwrap_or(lead);
                        let person = !strict || *kind != "name" || {
                            let words: Vec<&str> = core.split_whitespace().collect();
                            words.iter().any(|w| name_shape(w)) && words.iter().all(|w| name_shape(w.trim_end_matches(',')) || PARTICLES.contains(w))
                        };
                        if person && core.chars().any(|c| c.is_alphanumeric()) {
                            taken.add(s, s + core.len(), kind);
                        }
                    }
                    off += cell.len() + sep.len_utf8();
                }
                continue;
            }
        }
        if body.contains(['{', '}', '=', '<', '>', '"', '[', ']', '!']) || body.contains("::") || body.contains("fn ") {
            continue; // a line of code, not a table header
        }
        for candidate in [';', '\t', '|', ','] {
            if !body.contains(candidate) {
                continue;
            }
            let cells: Vec<&str> = body.split(candidate).collect();
            if cells.len() < 3 {
                continue;
            }
            let found: Vec<Option<&'static str>> = cells.iter().map(|c| column_kind(c)).collect();
            let short = cells.iter().all(|c| c.trim().chars().count() <= 30);
            let mapped = found.iter().filter(|k| k.is_some()).count();
            if mapped >= 2 || (mapped == 1 && short && found.contains(&Some("name"))) {
                strict = mapped == 1 && cells.iter().zip(&found).any(|(c, k)| *k == Some("name") && c.trim().trim_matches(['"', '*', '\'']).eq_ignore_ascii_case("name"));
                kinds = found;
                sep = candidate;
                break;
            }
        }
    }
}

// ---------------------------------------------------------------- postal address

fn addresses(text: &str, taken: &mut Taken) {
    static SUFFIX: OnceLock<Regex> = OnceLock::new();
    static WORDS: OnceLock<Regex> = OnceLock::new();
    static PREFIX: OnceLock<Regex> = OnceLock::new();
    static PLZ: OnceLock<Regex> = OnceLock::new();
    const NO: &str = r"\d{1,3} ?[a-zA-Z]?(?: ?[-/] ?\d{1,3} ?[a-zA-Z]?)?";
    let suffix = re(&SUFFIX, &format!(r"\b(?:[A-ZÄÖÜ][a-zäöüß]+[ -]){{0,2}}[A-ZÄÖÜ][a-zäöüß]*(?:straße|strasse|str\.|weg|gasse|platz|allee|ring|damm|ufer|chaussee|steig|pfad|markt|wall|graben|stieg|twiete|kamp|hof|park|garten|feld|berg|tor|brücke|grund|anger|winkel|höhe) {}", NO));
    let words = re(&WORDS, &format!(r"\b(?:[A-ZÄÖÜ][a-zäöüß]+[ -]){{1,3}}(?:Straße|Strasse|Str\.|Weg|Gasse|Platz|Allee|Ring|Damm|Ufer|Chaussee|Markt|Steig|Pfad|Wall|Graben) {}", NO));
    let prefix = re(&PREFIX, &format!(r"\b(?:Am|An der|An den|An dem|Im|In der|In den|Zum|Zur|Auf der|Auf dem|Auf den|Unter den|Unter der|Unter dem|Hinter der|Hinter dem|Hinter den|Vor dem|Vor der|Vor den|Bei der|Alte|Alter|Altes|Neue|Neuer|Neues|Großer|Kleiner|Obere|Untere) (?:[a-zäöüß]+ )?(?:[A-ZÄÖÜ][a-zäöüß]+ ){{0,2}}[A-ZÄÖÜ][a-zäöüß]+ {}", NO));
    let mut found: Vec<(usize, usize)> = Vec::new();
    for (pattern, strict) in [(suffix, false), (words, false), (prefix, true)] {
        for m in pattern.find_iter(text) {
            let mut end = m.end();
            // a house number never runs into the next word: "Lindenweg 4 in" keeps "4"
            while end > m.start() && text[..end].ends_with(|c: char| c.is_ascii_alphabetic())
                && text[end..].chars().next().map(|c| c.is_alphabetic()).unwrap_or(false) {
                end -= 1;
            }
            let end = text[..end].trim_end().len();
            let after = text[end..].chars().next();
            if text[end..].starts_with(|c: char| c.is_ascii_digit()) {
                continue;
            }
            let next = text[end..].trim_start_matches([' ', '\t']).chars().next();
            if strict && !matches!(next, None | Some(',' | ';' | '\n' | ')' | '\r' | '·' | '|' | '0'..='9')) {
                continue;
            }
            let _ = after;
            found.push((m.start(), end));
        }
    }
    // "Auf dem Weinberg 10" holds the match "Weinberg 10": overlapping matches become one span
    found.sort();
    let mut merged: Vec<(usize, usize)> = Vec::new();
    for (s, e) in found {
        match merged.last_mut() {
            Some(last) if s < last.1 => last.1 = last.1.max(e),
            _ => merged.push((s, e)),
        }
    }
    for (s, e) in merged {
        taken.add(s, e, "address");
    }
    // a street without a known ending right before a postcode: "Markt 5, 07545 Gera", "Feldrain 9\n60389 Frankfurt"
    static BEFORE_PLZ: OnceLock<Regex> = OnceLock::new();
    let before_plz = re(&BEFORE_PLZ, r"(?:^|[,:;·|(]\s*|\n\s*|wohnhaft (?:in )?(?:der )?)((?:[A-ZÄÖÜ][a-zäöüß.]+[ -]){0,2}[A-ZÄÖÜ][a-zäöüß]+ \d{1,3} ?[a-zA-Z]?(?: ?[-/] ?\d{1,3}[a-zA-Z]?)?)(?:, ?| · | \| |[ \t]*\n[ \t]*)(?:D-)?\d{5} \p{Lu}");
    static CONTEXT_STREET: OnceLock<Regex> = OnceLock::new();
    let context_street = re(&CONTEXT_STREET, r"(?:wohnhaft (?:in )?(?:der )?|Anschrift: ?|Adresse: ?|Wohnort: ?|Schadenort: ?|Lieferadresse: ?|Rechnungsadresse: ?|Objekt: ?|Baustelle: ?|Einsatzort: ?)((?:[A-ZÄÖÜ][a-zäöüß.]+[ -]){0,2}[A-ZÄÖÜ][a-zäöüß]+ \d{1,3} ?[a-zA-Z]?(?: ?[-/] ?\d{1,3}[a-zA-Z]?)?)\b");
    let mut extra: Vec<(usize, usize)> = Vec::new();
    for c in before_plz.captures_iter(text).chain(context_street.captures_iter(text)) {
        let g = c.get(1).unwrap();
        let first = g.as_str().split([' ', '-']).next().unwrap_or("");
        if lists().stop.contains(first) && !matches!(first, "Am" | "Im" | "An" | "Auf" | "Zum" | "Zur" | "In") {
            continue;
        }
        extra.push((g.start(), text[..g.end()].trim_end().len()));
    }
    for (s, e) in extra {
        taken.add(s, e, "address");
    }
    static POSTBOX: OnceLock<Regex> = OnceLock::new();
    for m in re(&POSTBOX, r"\bPostfach \d[\d ]{0,9}\d\b").find_iter(text) {
        taken.add(m.start(), m.end(), "address");
    }
    let plz = re(&PLZ, r"(?m)(?:^|[,:;·|(]|D-|\s{2}|\bin ) *(\d{5}) ((?:Bad |Sankt |St\. )?[A-ZÄÖÜ][a-zäöüß]+(?:-[A-ZÄÖÜ][a-zäöüß]+)*(?: (?:am|an der|im|in der|ob der|vor der|bei) [A-ZÄÖÜ][a-zäöüß]+)?(?: \([A-ZÄÖÜ][a-zäöüß]+\))?)( [A-ZÄÖÜ][a-zäöüß]+)?");
    const NOT_CITY: &[&str] = &["Euro", "Stück", "Mal", "Tage", "Kunden", "Mitarbeiter", "Einwohner", "Personen", "Seiten", "Zeichen", "Punkte", "Stunden", "Minuten", "Jahre", "Monate", "Bestellungen",
                              "Fällen", "Exemplaren", "Metern", "Kilometern", "Litern", "Kunden", "Filialen", "Haushalten"];
    for c in plz.captures_iter(text) {
        let (code, city) = (c.get(1).unwrap(), c.get(2).unwrap());
        if NOT_CITY.contains(&city.as_str()) || code.as_str() < "01001" {
            continue;
        }
        // a second word joins the city only at the end of the line or before punctuation: "74523 Schwäbisch Hall"
        let mut end = city.end();
        if let Some(second) = c.get(3) {
            let next = text[second.end()..].chars().next();
            if matches!(next, None | Some('\n' | '\r' | ',' | ';' | '·' | '|' | ')' | '.')) && !lists().stop.contains(second.as_str().trim()) {
                end = second.end();
            }
        }
        taken.add(code.start(), end, "address");
    }
}

// ---------------------------------------------------------------- names

/// Common German first names, also those of families from Turkey, Poland and Italy who live in Germany.
const FIRST: &[&str] = &[
    "Anna", "Maria", "Emma", "Mia", "Sofia", "Sophia", "Sophie", "Hannah", "Hanna", "Emilia", "Lina", "Lea", "Leonie", "Marie", "Lena", "Laura", "Julia",
    "Sarah", "Sara", "Lisa", "Katharina", "Johanna", "Clara", "Klara", "Charlotte", "Amelie", "Luisa", "Louisa", "Ella", "Ida", "Greta", "Frieda", "Mila",
    "Nele", "Lara", "Paula", "Alina", "Jana", "Jasmin", "Vanessa", "Nina", "Sandra", "Sabine", "Susanne", "Petra", "Andrea", "Claudia", "Nicole",
    "Stefanie", "Stephanie", "Kerstin", "Birgit", "Monika", "Ursula", "Renate", "Gabriele", "Karin", "Elke", "Heike", "Martina", "Christina", "Christine",
    "Kristina", "Daniela", "Anja", "Silke", "Tanja", "Melanie", "Simone", "Manuela", "Doris", "Ingrid", "Helga", "Brigitte", "Gisela", "Erika",
    "Elisabeth", "Barbara", "Angelika", "Cornelia", "Beate", "Bettina", "Carina", "Franziska", "Theresa", "Teresa", "Miriam", "Nadine", "Svenja",
    "Antonia", "Maja", "Pia", "Jule", "Merle", "Annika", "Anke", "Ines", "Iris", "Judith", "Kathrin", "Katrin", "Marion", "Natalie", "Patricia",
    "Ramona", "Regina", "Rita", "Ruth", "Sonja", "Ulrike", "Verena", "Viktoria", "Yvonne", "Zoe", "Elena", "Selina", "Isabel", "Helena", "Magdalena",
    "Emily", "Jessica", "Jennifer", "Anne", "Eva", "Carla", "Carolin", "Caroline", "Denise", "Diana", "Esther", "Gudrun", "Hildegard", "Irene",
    "Jacqueline", "Lieselotte", "Marlene", "Michaela", "Nora", "Olga", "Rebecca", "Saskia", "Tamara", "Tatjana", "Vera", "Wiebke",
    "Ben", "Paul", "Leon", "Finn", "Elias", "Jonas", "Luis", "Louis", "Noah", "Felix", "Lukas", "Lucas", "Maximilian", "Henry", "Emil", "Theo",
    "Anton", "Jakob", "Jacob", "David", "Moritz", "Niklas", "Nico", "Tim", "Tom", "Jan", "Julian", "Fabian", "Florian", "Philipp", "Simon", "Tobias",
    "Sebastian", "Alexander", "Daniel", "Michael", "Thomas", "Andreas", "Stefan", "Stephan", "Christian", "Markus", "Marcus", "Matthias", "Martin",
    "Jürgen", "Klaus", "Peter", "Wolfgang", "Uwe", "Dieter", "Bernd", "Ralf", "Rolf", "Werner", "Manfred", "Helmut", "Gerhard", "Günter", "Günther",
    "Horst", "Heinz", "Karl", "Carl", "Hans", "Joachim", "Jörg", "Dirk", "Holger", "Sven", "Torsten", "Thorsten", "Oliver", "Patrick", "Dennis",
    "Marco", "Mario", "Kai", "Lars", "Björn", "Kevin", "Marcel", "Benjamin", "Johannes", "Jonathan", "Leonard", "Konstantin", "Vincent", "Oskar",
    "Oscar", "Mats", "Matteo", "Erik", "Eric", "Robin", "Robert", "Richard", "Rainer", "Reinhard", "Norbert", "Hubert", "Georg", "Gregor", "Hendrik",
    "Henrik", "Malte", "Ole", "Hannes", "Till", "Timo", "Lennart", "Linus", "Luca", "Samuel", "Benedikt", "Dominik", "Christoph", "Jens", "Volker",
    "Axel", "Udo", "Achim", "Armin", "Detlef", "Egon", "Friedrich", "Fritz", "Gustav", "Herbert", "Hermann", "Heinrich", "Ingo", "Kurt", "Lothar",
    "Ludwig", "Otto", "Siegfried", "Walter", "Wilhelm", "Willi", "Mehmet", "Ahmet", "Ali", "Mustafa", "Murat", "Ayşe", "Fatma", "Elif", "Zeynep",
    "Emine", "Hasan", "Hüseyin", "Ibrahim", "Yusuf", "Emir", "Can", "Piotr", "Tomasz", "Katarzyna", "Anastasia", "Natalia", "Alessandro", "Giuseppe",
    "Francesca", "Sergej", "Andrej", "Igor", "Ivan", "Nikolai", "Fatima", "Ahmed", "Mohammed", "Mohamed", "Omar", "Youssef",
    "Yasmin", "Amina", "Aminata", "Leyla", "Layla", "Rania", "Hassan", "Khaled", "Tariq", "Bilal", "Jamal", "Malik", "Nadia", "Salma", "Kofi",
    "Kwame", "Ama", "Fatou", "Ibrahima", "Moussa", "Marek", "Jakub", "Kacper", "Zofia", "Svetlana", "Tatiana", "Dmitri", "Wladimir", "Oleg",
    "Lan", "Minh", "Huong", "Trang", "Tuan", "Quang", "Paolo", "Giovanni", "Francesco", "Antonio", "Alessandra", "Federica", "Valentina",
    "Carlos", "Juan", "José", "Ana", "Pablo", "Diego", "Javier", "Lucia", "Krystyna", "Tadeusz", "Michail", "Ercan", "Kemal", "Serkan", "Ngozi", "Chidi", "Adebayo", "Oluwaseun", "Amara", "Deniz", "Gül", "Hatice", "Max", "Frank", "Ernst", "Rosa", "Mark", "August", "Wolf",
];

/// First names that are also German words. They count only next to a listed surname or in a name context.
const AMBIGUOUS: &[&str] = &["Max", "Frank", "Ernst", "Rosa", "Mark", "August", "Wolf", "Can", "Ali", "Will", "Henry", "Emil"];

/// Short first names that are also words or abbreviations ("Jan." for January, "am Kai"). They never count alone.
const NEVER_ALONE: &[&str] = &["Jan", "Kai", "Tom", "Ben", "Ole", "Ida", "Mia", "Eva", "Tim", "Max"];

/// Common German surnames and a few common surnames of families from other countries.
const SURNAMES: &[&str] = &[
    "Müller", "Schmidt", "Schneider", "Fischer", "Weber", "Meyer", "Wagner", "Becker", "Schulz", "Hoffmann", "Schäfer", "Koch", "Bauer", "Richter",
    "Klein", "Wolf", "Schröder", "Neumann", "Schwarz", "Zimmermann", "Braun", "Krüger", "Hofmann", "Hartmann", "Lange", "Schmitt", "Werner",
    "Schmitz", "Krause", "Meier", "Lehmann", "Schmid", "Schulze", "Maier", "Köhler", "Herrmann", "König", "Walter", "Mayer", "Huber", "Kaiser",
    "Fuchs", "Peters", "Lang", "Scholz", "Möller", "Weiß", "Jung", "Hahn", "Schubert", "Vogel", "Friedrich", "Keller", "Günther", "Frank", "Berger",
    "Winkler", "Roth", "Beck", "Lorenz", "Baumann", "Franke", "Albrecht", "Schuster", "Simon", "Ludwig", "Böhm", "Winter", "Kraus", "Martin",
    "Schumacher", "Krämer", "Vogt", "Stein", "Jäger", "Otto", "Sommer", "Groß", "Seidel", "Heinrich", "Brandt", "Haas", "Schreiber", "Graf",
    "Schulte", "Dietrich", "Ziegler", "Kuhn", "Kühn", "Pohl", "Engel", "Horn", "Busch", "Bergmann", "Thomas", "Voigt", "Sauer", "Arnold", "Wolff",
    "Pfeiffer", "Berg", "Lindner", "Brinkmann", "Ernst", "Franz", "Hansen", "Jansen", "Kremer", "Marx", "Nagel", "Petersen", "Ritter",
    "Schütz", "Stahl", "Thiel", "Ulrich", "Wendt", "Wirth", "Yilmaz", "Kaya", "Demir", "Şahin", "Çelik", "Yıldız", "Öztürk", "Nowak", "Kowalski",
    "Wiśniewski", "Lewandowski", "Nguyen", "Tran", "Popescu", "Ivanov", "Petrović", "Rossi", "Russo", "Esposito", "Bianchi",
    "Kowalczyk", "Wójcik", "Kamiński", "Zieliński", "Szymański", "Woźniak", "Dąbrowski", "Kozłowski", "Mazur", "Krawczyk", "Pham", "Hoang",
    "Huynh", "Diallo", "Mensah", "Traoré", "Ivanova", "Petrov", "Smirnov", "Kuznetsov", "Popov", "Koslow", "Iwanow", "Arslan", "Doğan",
    "Aydın", "Özdemir", "Koç", "Polat", "Aksoy", "Ferrari", "Romano", "Colombo", "Ricci", "Marino", "Greco", "Gallo", "Conti", "Moretti",
];

/// Words that start with a capital letter in German text but are no names.
const STOP: &[&str] = &[
    "Der", "Die", "Das", "Den", "Dem", "Des", "Ein", "Eine", "Einen", "Einem", "Einer", "Sie", "Ihr", "Ihre", "Ihren", "Ihrem", "Ihrer", "Ihnen",
    "Wir", "Uns", "Unser", "Unsere", "Unseren", "Ich", "Mein", "Meine", "Er", "Es", "Man", "Wer", "Was", "Wie", "Wo", "Wann", "Warum", "Bitte",
    "Danke", "Vielen", "Dank", "Mit", "Und", "Oder", "Aber", "Auch", "Nur", "Noch", "Schon", "Sehr", "Hallo", "Liebe", "Lieber", "Herr", "Herrn",
    "Frau", "Damen", "Herren", "Team", "Kollegen", "Kolleginnen", "Kollege", "Kollegin", "Alle", "Zusammen", "Grüße", "Gruß", "Grüßen", "GmbH",
    "AG", "KG", "SE", "UG", "OHG", "Co", "Ltd", "Inc", "Straße", "Weg", "Platz", "Rechnung", "Vertrag", "Betreff", "Datum", "Seite", "Anlage",
    "Anlagen", "Tel", "Telefon", "Fax", "Mobil", "E-Mail", "Email", "Mail", "IBAN", "BIC", "Steuer", "Kundennummer", "Rechnungsnummer",
    "Personalnummer", "Abteilung", "Personalabteilung", "Buchhaltung", "Geschäftsführung", "Montag", "Dienstag", "Mittwoch", "Donnerstag",
    "Freitag", "Samstag", "Sonntag", "Januar", "Februar", "März", "April", "Mai", "Juni", "Juli", "September", "Oktober", "November",
    "Dezember", "Euro", "EUR", "Im", "In", "Am", "An", "Auf", "Für", "Von", "Vom", "Zum", "Zur", "Bei", "Nach", "Aus", "Über", "Unter", "Vor",
    "Hinter", "Neben", "Zwischen", "Gestern", "Heute", "Morgen", "Anbei", "Leider", "Gerne", "Gern", "Hiermit", "Wenn", "Falls", "Da", "Dass",
    "Damit", "Bis", "Ab", "Seit", "Nachfolgend", "Arbeitgeber", "Arbeitnehmer", "Arbeitnehmerin", "Mieter", "Vermieter", "Kunde", "Kundin",
    "Geschäftsführer", "Geschäftsführerin", "Mitarbeiter", "Mitarbeiterin", "Ansprechpartner", "Ansprechpartnerin", "Name", "Vorname",
    "Nachname", "Unterschrift", "Ort", "Anschrift", "Adresse", "Hinweis", "Hinweise", "Freundliche", "Freundlichen", "Beste", "Besten",
    "Viele", "Herzliche", "Herzlichen", "Schöne", "Guten", "Tag", "Morgen", "Abend", "Hi", "Hey", "Moin", "Servus", "Personal", "Zeugnis",
    "Kündigung", "Gehalt", "Termin", "Gespräch", "Einladung", "Bestätigung", "Angebot", "Auftrag", "Lieferung", "Zahlung", "Summe", "Gesamt",
    "Netto", "Brutto", "MwSt", "USt", "Pos", "Menge", "Preis", "Beschreibung", "Position", "Konto", "Bank", "Sparkasse", "Volksbank", "Zahlbar",
    "Fällig", "Bezahlt", "Offen", "Wichtig", "Info", "Information", "Support", "Service", "Vertrieb", "Einkauf", "Verkauf", "Leitung",
    "Abteilungsleiter", "Abteilungsleiterin", "Teamleiter", "Teamleiterin", "Dr", "Prof", "Dipl", "Ihres", "Ihrem", "Diese", "Dieser",
    "Dieses", "Jeder", "Jede", "Keine", "Kein", "Weitere", "Weiterer", "Neue", "Neuer", "Alte", "Alter", "Sachbearbeiter", "Sachbearbeiterin",
    "Vertreten", "Vertreter", "Inhaber", "Inhaberin", "Kontoinhaber", "Kontoinhaberin", "Absender", "Empfänger", "Kopie", "Cc", "Bcc",
    "Ehefrau", "Ehemann", "Sohn", "Tochter", "Kind", "Kinder", "Mutter", "Vater", "Bruder", "Schwester", "Partner", "Partnerin", "Nachbar",
    "Nachbarin", "Beruf", "Fa", "Firma", "Baustelle", "Personalbüro", "Wohnung", "Haus", "Gebäude", "Halle", "Lager", "Annahme", "Entschuldigt",
    "Vorgesetzte", "Vorgesetzter", "Versicherungsnehmer", "Versicherungsnehmerin", "Teilnehmende", "Anwesend", "Protokoll", "Familie",
    "Rechtsanwalt", "Rechtsanwältin", "Referent", "Referentin", "Kanzlei", "Praxis", "Objekt", "Schadenort", "Betreff", "Genehmigt",
    "PS", "Vorstand", "Aufsichtsrat", "Amtsgericht", "Registergericht", "Handelsregister", "Sitz", "Zuständig", "Ansprechperson", "Gez",
];

/// Job titles and departments that follow a name in a signature: "Lorenz Haverkamp\nProkurist", "Rania Khalil,
/// Kundenservice", "Omar Haddad (Lagerlogistik)".
const ROLE_WORDS: &str = "Geschäftsführer(?:in)?|Geschäftsführung|Prokurist(?:in)?|Inhaber(?:in)?|Vorstand|Vorsitzende[r]?|Personalreferent(?:in)?|\
Sachbearbeiter(?:in)?|Assistenz|Assistent(?:in)?|Sekretariat|Kundenservice|Kundenbetreuung|Vertrieb|Einkauf|Buchhaltung|Controlling|Marketing|IT|\
Personal|Personalabteilung|Personalwesen|Empfang|Lager|Logistik|Verwaltung|Hausverwaltung|Technik|Service|Support|Recht|Datenschutz|\
[A-ZÄÖÜ][a-zäöüß]*(?:leitung|leiter|leiterin|abteilung|logistik|management|manager|managerin|referent|referentin|beauftragte[r]?|berater|beraterin|assistent|assistentin|technik|planung|entwicklung|wesen)|\
Rechtsanwalt|Rechtsanwältin|Steuerberater(?:in)?|Notar(?:in)?|Arzt|Ärztin|Meister(?:in)?|Ehemann|Ehefrau|Mutter|Vater|Sohn|Tochter|\
Fahrer(?:in)?|Disponent(?:in)?|Monteur(?:in)?|Techniker(?:in)?|Geselle|Gesellin|Auszubildende[r]?|Praktikant(?:in)?|Werkstudent(?:in)?|Aushilfe|\
[A-ZÄÖÜ][a-zäöüß]*(?:geber|geberin|nehmer|nehmerin|ärztin|arzt|medizin|dienst|therapeut|therapeutin|pfleger|pflegerin|kauffrau|kaufmann)";

/// After one of these a capitalised word is a noun: "der Vorgesetzte Ilka Schröder".
const ARTICLES: &[&str] = &["der", "die", "das", "den", "dem", "des", "vom", "zum", "zur", "beim", "im", "am", "ein", "eine", "einen", "einem", "einer", "unser", "unsere", "unseren",
                            "unserem", "unserer", "ihr", "ihre", "ihren", "ihrem", "ihrer", "sein", "seine", "seinen", "seinem", "seiner"];

const PARTICLES: &[&str] = &["von", "van", "de", "der", "zu", "vom", "ten", "ter", "da", "di", "del", "du", "le", "la", "el", "al", "bin"];

const LEGAL: &[&str] = &["GmbH", "AG", "KG", "SE", "UG", "OHG", "mbH", "Co", "Ltd", "Inc", "e.V", "eG", "GbR", "PartG", "KGaA", "Holding"];

const NOUN_ENDINGS: &[&str] = &["ung", "heit", "keit", "schaft", "nummer", "tion", "ment", "ität", "nis", "tum", "datum", "vertrag", "rechnung",
                                "adresse", "straße", "abteilung", "leitung", "gespräch", "termin", "zeugnis", "bescheinigung", "büro", "team",
                                "service", "verwaltung", "kanzlei", "zentrale", "stelle", "zeiten", "beauftragte", "beauftragten", "ungen", "heiten",
                                "keiten", "therapie", "praxis", "studio", "zentrum", "dienst"];

struct Lists {
    first: HashSet<&'static str>,
    ambiguous: HashSet<&'static str>,
    surnames: HashSet<&'static str>,
    stop: HashSet<&'static str>,
}

fn lists() -> &'static Lists {
    static LISTS: OnceLock<Lists> = OnceLock::new();
    LISTS.get_or_init(|| Lists {
        first: FIRST.iter().copied().collect(),
        ambiguous: AMBIGUOUS.iter().copied().collect(),
        surnames: SURNAMES.iter().copied().collect(),
        stop: STOP.iter().copied().collect(),
    })
}

/// A listed first name, also as the first part of a double first name: "Marie-Theres", "Hans-Jürgen".
fn first_name(w: &str) -> bool {
    let l = lists();
    l.first.contains(w) || (w.contains('-') && w.split('-').next().map(|p| l.first.contains(p)).unwrap_or(false) && name_shape(w))
}

/// Is this first name in the detector list? The factory uses it to split names into listed and unlisted.
pub fn listed_first_name(w: &str) -> bool {
    lists().first.contains(w)
}

pub fn listed_surname(w: &str) -> bool {
    lists().surnames.contains(w)
}

pub fn stop_word(w: &str) -> bool {
    lists().stop.contains(w)
}

#[derive(Debug)]
struct Tok<'a> {
    s: usize,
    e: usize,
    w: &'a str,
    line: usize,
    /// The token ends a group: `,`, `;`, `:`, `!`, `?`, `.` or a closing bracket follows the core.
    stop_after: bool,
    /// An opening bracket or quote stands right before the core.
    open_before: bool,
    /// One capital letter and a full stop: the "F." of "Jan F. Rehberg".
    initial: bool,
}

fn tokens(text: &str) -> Vec<Tok<'_>> {
    static WORD: OnceLock<Regex> = OnceLock::new();
    let mut out = Vec::new();
    let (mut line, mut last) = (0usize, 0usize);
    for m in re(&WORD, r"\S+").find_iter(text) {
        line += text[last..m.start()].matches('\n').count();
        last = m.start();
        let w = m.as_str();
        let lead = w.len() - w.trim_start_matches(is_delim).len();
        let core = w.trim_start_matches(is_delim).trim_end_matches(|c: char| is_delim(c) || matches!(c, '.' | ':' | '!' | '?'));
        if core.is_empty() {
            continue;
        }
        let tail = &w[lead + core.len()..];
        out.push(Tok { s: m.start() + lead, e: m.start() + lead + core.len(), w: core, line,
                       stop_after: tail.contains([',', ';', ':', '!', '?', '.', ')', ']', '>', '"', '“', '”']),
                       open_before: lead > 0, initial: core.chars().count() == 1 && core.chars().all(char::is_uppercase) && tail.starts_with('.') });
    }
    out
}

/// A word that can be part of a name: capital first letter, then letters, hyphens or an apostrophe, some lowercase.
fn name_shape(w: &str) -> bool {
    let mut chars = w.chars();
    let Some(first) = chars.next() else { return false };
    if !first.is_uppercase() || w.chars().count() < 2 {
        return false;
    }
    if !w.chars().all(|c| c.is_alphabetic() || matches!(c, '-' | '\'' | '’')) || !w.chars().any(|c| c.is_lowercase()) {
        return false;
    }
    w.split('-').all(|p| p.chars().next().map(|c| c.is_uppercase()).unwrap_or(false))
}

fn nameish(w: &str) -> bool {
    static ROLE: OnceLock<Regex> = OnceLock::new();
    let role = ROLE.get_or_init(|| Regex::new(&format!(r"^(?:{})$", ROLE_WORDS)).unwrap());
    name_shape(w) && !lists().stop.contains(w) && !LEGAL.contains(&w) && !role.is_match(w)
}

fn noun_like(w: &str) -> bool {
    let l = w.to_lowercase();
    NOUN_ENDINGS.iter().any(|e| l.ends_with(e)) && !lists().surnames.contains(w)
}

/// From token `i`, the name words on the same line: up to `max` words, particles between them allowed. Empty when the
/// group is a company (a legal form follows) or a taken span stands in the way.
fn group(toks: &[Tok], i: usize, max: usize, taken: &Taken) -> Vec<usize> {
    let mut out: Vec<usize> = Vec::new();
    let mut j = i;
    let mut words = 0;
    while j < toks.len() && toks[j].line == toks[i].line && words < max {
        let t = &toks[j];
        if !taken.free(t.s, t.e) || (words > 0 && t.open_before) {
            break;
        }
        let joined = t.w == "und" && j > 0 && PARTICLES.contains(&toks[j - 1].w) && toks.get(j + 1).map(|n| PARTICLES.contains(&n.w)).unwrap_or(false);
        if PARTICLES.contains(&t.w) || joined {
            if words == 0 && !matches!(t.w, "von" | "van" | "zu" | "de") {
                break;
            }
            if j + 1 < toks.len() && toks[j + 1].line == t.line && (nameish(toks[j + 1].w) || PARTICLES.contains(&toks[j + 1].w)) && !t.stop_after {
                out.push(j);
                j += 1;
                continue;
            }
            break;
        }
        if t.initial && words > 0 && toks.get(j + 1).map(|n| n.line == t.line && nameish(n.w)).unwrap_or(false) {
            out.push(j);
            j += 1;
            continue;
        }
        if !nameish(t.w) || noun_like(t.w) {
            break;
        }
        out.push(j);
        words += 1;
        if t.stop_after {
            break;
        }
        j += 1;
    }
    while out.last().map(|&k| PARTICLES.contains(&toks[k].w)).unwrap_or(false) {
        out.pop();
    }
    if out.is_empty() {
        return out;
    }
    // a legal form among the next three words of the line makes the group a company: "Nordline Service KG"
    let mut company = out.iter().any(|&k| LEGAL.contains(&toks[k].w));
    let last = *out.last().unwrap();
    if !toks[last].stop_after {
        for t in toks.iter().skip(last + 1).take(3).take_while(|t| t.line == toks[i].line) {
            if LEGAL.iter().any(|l| t.w.starts_with(l)) || t.w == "&" {
                company = true;
            }
            if t.stop_after {
                break;
            }
        }
    }
    if company {
        return Vec::new();
    }
    out
}

/// The word belongs to a company name: a legal form or "&" follows within two words, or a business word comes right
/// before: "Kanzlei Lorenz & Collegen", "Bürobedarf Priem GmbH".
fn in_company(toks: &[Tok], k: usize) -> bool {
    let line = toks[k].line;
    let follows = toks.iter().skip(k + 1).take(2).take_while(|t| t.line == line)
        .any(|t| t.w == "&" || LEGAL.iter().any(|l| t.w.starts_with(l)) || t.w == "Partner" || t.w == "PartG");
    let business = k > 0 && toks[k - 1].line == line && {
        let w = toks[k - 1].w.to_lowercase();
        ["kanzlei", "verwaltung", "praxis", "bäckerei", "autohaus", "bürobedarf", "steuerkanzlei", "agentur", "atelier", "studio",
         "apotheke", "fahrschule", "malerbetrieb", "metzgerei", "gärtnerei", "druckerei", "werkstatt", "elektro", "sanitär", "büro", "studio",
         "praxis", "salon", "laden", "markt", "hof"].iter().any(|b| w.ends_with(b))
    };
    follows || business
}

/// A listed surname, or a double name with a listed part: "Schmidt-Prellmöller".
fn listed_surname_part(w: &str) -> bool {
    let l = lists();
    l.surnames.contains(w) || (w.contains('-') && w.split('-').any(|p| l.surnames.contains(p)))
}

/// The token starts a sentence: only spaces since the line start, or a full stop, `!` or `?` before it.
fn first_on_line_after_stop(text: &str, at: usize) -> bool {
    let before = text[..at].trim_end_matches([' ', '\t']);
    before.is_empty() || before.ends_with(['\n', '.', '!', '?', ':'])
}

fn names(text: &str, taken: &mut Taken) {
    static TITLE: OnceLock<Regex> = OnceLock::new();
    static LABEL: OnceLock<Regex> = OnceLock::new();
    static ROLE: OnceLock<Regex> = OnceLock::new();
    static GREETING: OnceLock<Regex> = OnceLock::new();
    static CLOSING: OnceLock<Regex> = OnceLock::new();
    static LONE_TITLE: OnceLock<Regex> = OnceLock::new();
    let academic = r"(?:Dr\.|Prof\.|Dipl\.-[A-Za-zäöü]+\.?|Dr\. ?med\.|Dr\.-Ing\.|Mag\.|M\.\s?Sc\.|B\.\s?Sc\.)";
    let title = re(&TITLE, &format!(r"(?:\b(?:[Hh]errn?|[Ff]rau|Hr\.|Fr\.|Familie|Fam\.)(?: {a})*|\b{a}(?: {a})*)(?: (?:von|van|zu|de|vom)(?: der| dem| den)?)? $", a = academic));
    // labels are capitalised in German forms; a lower-case "name:" is a field in source code
    let label = re(&LABEL, &format!(r"(?:\b(?:Name(?: (?:des|der) [A-ZÄÖÜ][a-zäöüß]+)?|Vorname|Nachname|Ansprechpartner(?:in)?|Kontoinhaber(?:in)?|Mitarbeiter(?:in)?|Arbeitnehmer(?:in)?|Mieter(?:in)?|Vermieter(?:in)?|Bearbeiter(?:in)?|Sachbearbeiter(?:in)?|Empfänger(?:in)?|Absender(?:in)?|[Vv]on|[Aa]n|Cc|CC|Bcc|BCC|Kopie|Kontakt|Inhaber(?:in)?|Geschäftsführer(?:in)?|Geschäftsführung|Vorstand|Unterschrift|Antragsteller(?:in)?|Bewerber(?:in)?|Teilnehmer(?:in)?|Kunde|Kundin|Annahme|Verfasser(?:in)?|Zuständig|Ansprechperson|Vertreter(?:in)?|Beschäftigte[r]?|Auszubildende[r]?|Praktikant(?:in)?|Vorgesetzte[r]?|Unterzeichner(?:in)?|Gesprächspartner(?:in)?|Auftragnehmer(?:in)?|Auftraggeber(?:in)?|Notfallkontakt|Kontaktperson|Ehepartner(?:in)?|Erstellt von|Geprüft von|Freigegeben von|Teamleitung|Leitung|Projektleitung|Verantwortlich|Für|Anwesend|Abwesend|Entschuldigt|Verteiler|Gäste|Teilnehmende|Teilnehmer(?:in|innen)?|Protokoll|Protokollführer(?:in)?|Genehmigt|Lieferadresse|Rechnungsadresse|Rechnungsempfänger(?:in)?|Anrufer(?:in)?|Patient(?:in)?|Versicherte[r]?|Fahrer(?:in)?|Mitglied|Schuldner(?:in)?|Gläubiger(?:in)?|Zeuge|Zeugin|Bürge|Bürgin|Erbe|Erbin|Melder(?:in)?|Gast|Gäste|Bearbeitet von|Erfasst von|Aufgenommen von|Unterschrieben von|Bevollmächtigte[r]?|Vollmachtgeber(?:in)?|Referent(?:in)?|Moderation|Gesprächsführung|Dozent(?:in)?|Trainer(?:in)?|Lehrer(?:in)?|Mentor(?:in)?|Betreuer(?:in)?|Berater(?:in)?|Pate|Patin|Monteur(?:in)?|Techniker(?:in)?|Handwerker(?:in)?|Geselle|Gesellin|Fahrer(?:in)?|Kurier|Disponent(?:in)?|Disposition|[A-ZÄÖÜ][a-zäöüß]*(?:nehmer|nehmerin|empfänger|empfängerin|beauftragte[r]?|leiter|leiterin|leitung|führung|führer|führerin|partner|partnerin|steller|stellerin|halter|halterin|inhaber|inhaberin|annahme|kontakt|person|vertretung|verantwortliche[r]?|zuständige[r]?))\s*:|\b(?:Genehmigt|Geprüft|Bearbeitet|Erstellt|Aufgenommen|Unterschrieben|Freigegeben|Erfasst|Dokumentiert|Angenommen|Ausgestellt|Abgezeichnet) (?:durch|von)|\bInh\.|\bi\. ?[AV]\.|\bz\. ?Hd\.?|\bgez\.|\b[Vv]ertreten durch(?: den| die| ihre[n]?| seine[n]?)?(?: Geschäftsführer(?:in)?| Vorstand| Prokuristen| Prokuristin| Inhaber(?:in)?)?|\bper Adresse|\bc/o|\b(?:{r})\s*:|\b(?:[Gg]eprüft|[Ff]reigegeben|[Gg]enehmigt|[Ee]rstellt|[Bb]earbeitet|[Uu]nterschrieben|[Ee]rfasst|[Vv]ermerkt|[Aa]ufgenommen)(?: am [\d.]+)?(?: (?:durch|von))?\s*:?|\bUnterschriften\s*:) *(?:(?:von|van|zu|de)(?: der| dem| den)? )?$", r = ROLE_WORDS));
    let role = re(&ROLE, r"(?:\b(?:Mitarbeiter(?:in)?|Kollege|Kollegin|Geschäftsführer(?:in)?|Sachbearbeiter(?:in)?|Ansprechpartner(?:in)?|Teamleiter(?:in)?|Abteilungsleiter(?:in)?|Personalleiter(?:in)?|Vorgesetzte[rn]?|Bewerber(?:in)?|Arbeitnehmer(?:in)?|Mieter(?:in)?|Vermieter(?:in)?|Kunde|Kunden|Kundin|Prokurist(?:in)?|Inhaber(?:in)?|Auszubildende[rn]?|Praktikant(?:in)?|Nachfolger(?:in)?|Vertretung|Kontoinhaber(?:in)?|Kontaktperson|Rechtsanwalt|Rechtsanwältin|Notar(?:in)?|Steuerberater(?:in)?|Referent(?:in)?|Dozent(?:in)?|Trainer(?:in)?|Berater(?:in)?|Betreuer(?:in)?|Hausmeister(?:in)?|Monteur(?:in)?|Techniker(?:in)?|Fahrer(?:in)?|Arzt|Ärztin|Patient(?:in)?|Student(?:in)?|Werkstudent(?:in)?|Chef(?:in)?|Leiter(?:in)?|Ehemann|Ehefrau|Sohn|Tochter|Nachbar(?:in)?|[A-ZÄÖÜ][a-zäöüß]*(?:beauftragte[nr]?|leiter(?:in)?|referent(?:in)?|berater(?:in)?|meister(?:in)?|assistent(?:in)?|manager(?:in)?))(?: ist| sind| war| wird|,)?) $");
    let greeting = re(&GREETING, r"(?:^|[\s,])(?:Hallo|Hi|Hey|Moin|Liebe|Lieber|Servus|Guten Tag|Guten Morgen|Sehr geehrte|Sehr geehrter|Danke|Danke,|Dank an|Grüße an|Gruß an|Dank gilt|bedanken uns bei|danken) $");
    let closing = re(&CLOSING, r"(?i)^\s*(?:mit )?(?:freundlichen|besten|vielen|herzlichen|lieben|schönen|freundliche|beste|viele|herzliche|liebe|schöne|sonnige|kollegiale|kollegialen)? ?(?:grüßen|grüße|gruß|gruss|grüssen)\b[^\n]*$|^\s*(?:i\. ?[AV]\.|gez\.|ppa\.|Ihr|Ihre|Dein|Deine|Euer|Eure)\s*$|^\s*_{5,}[\s_]*$");
    let lone_title = re(&LONE_TITLE, &format!(r"^\s*(?:Herrn?|Frau)(?: {a})*\s*$", a = academic));
    static PAREN_ROLE: OnceLock<Regex> = OnceLock::new();
    static AFTER_BIRTH: OnceLock<Regex> = OnceLock::new();
    let after_birth = re(&AFTER_BIRTH, &format!(r"^(?:,? ?(?:geb\.|geboren|wohnhaft|\(geb\.|\(\*|\* ?\d)|(?:,| -) (?:{r})\b|(?: \()(?:{r})\))", r = ROLE_WORDS));
    static CHAT: OnceLock<Regex> = OnceLock::new();
    let chat = re(&CHAT, r"^\s*(?:\[?\d{1,2}:\d{2}(?::\d{2})?\]?|\d{1,2}\.\d{1,2}\.(?:\d{2,4})?,? \d{1,2}:\d{2})\s*-?\s*$");
    let paren_role = re(&PAREN_ROLE, r"^ *\((?:Geschäftsführung|Geschäftsführer(?:in)?|Arbeitgeber(?:in)?|Arbeitnehmer(?:in)?|Vermieter(?:in)?|Mieter(?:in)?|Auftragnehmer(?:in)?|Auftraggeber(?:in)?|Inhaber(?:in)?|Vorstand|Prokurist(?:in)?|Ehemann|Ehefrau|Mutter|Vater|Sohn|Tochter|Partner(?:in)?|Lebenspartner(?:in)?|Bevollmächtigte[r]?|Zeuge|Zeugin)\)");
    let l = lists();
    let toks = tokens(text);
    let line_starts: Vec<usize> = std::iter::once(0).chain(text.match_indices('\n').map(|(i, _)| i + 1)).collect();
    let line_text = |n: usize| -> &str {
        let s = line_starts[n];
        let e = line_starts.get(n + 1).map(|x| x - 1).unwrap_or(text.len());
        &text[s..e]
    };
    // the last non-empty line before each line, found once so long texts stay linear
    let mut last_full: Vec<Option<usize>> = Vec::with_capacity(line_starts.len());
    let mut seen: Option<usize> = None;
    for n in 0..line_starts.len() {
        last_full.push(seen);
        if !line_text(n).trim().is_empty() {
            seen = Some(n);
        }
    }
    let mut next_full: Vec<Option<usize>> = vec![None; line_starts.len()];
    let mut ahead: Option<usize> = None;
    for n in (0..line_starts.len()).rev() {
        next_full[n] = ahead;
        if !line_text(n).trim().is_empty() {
            ahead = Some(n);
        }
    }
    let next_line = |n: usize| -> Option<&str> { next_full[n].map(line_text) };
    static ROLE_WORD: OnceLock<Regex> = OnceLock::new();
    let role_word = re(&ROLE_WORD, &format!(r"^(?:{})$", ROLE_WORDS));
    static ROLE_LINE: OnceLock<Regex> = OnceLock::new();
    let role_line = re(&ROLE_LINE, &format!(r"^\s*\(?(?:{r})(?: (?:{r}|\p{{Lu}}[\p{{Ll}}ß]+))?\)?\s*$", r = ROLE_WORDS));
    let prev_line = |n: usize| -> Option<&str> {
        last_full[n].map(|k| { let t = line_text(k); &t[floor(t, t.len().saturating_sub(200))..] })
    };
    let line_end = |n: usize| -> usize { line_starts.get(n + 1).map(|x| x - 1).unwrap_or(text.len()) };
    let mut hits: Vec<usize> = Vec::new();
    // per line: the line before holds no name; computed once per line so a line of many tokens stays linear
    let mut middle_memo: Vec<Option<bool>> = vec![None; line_starts.len()];
    let mut i = 0;
    while i < toks.len() {
        let t = &toks[i];
        if !nameish(t.w) || !taken.free(t.s, t.e) {
            i += 1;
            continue;
        }
        let before = line_before(text, t.s, 120);
        let first_on_line = (0..i).rev().take_while(|&k| toks[k].line == t.line).all(|k| PARTICLES.contains(&toks[k].w));
        let mut found: Vec<usize> = Vec::new();
        // no rule on the text before a token matches an empty text: skipping them keeps a line of one word cheap
        let blank = before.trim().is_empty();
        let label_here = !blank && label.is_match(before);
        if !blank && (label_here || title.is_match(before)) {
            found = group(&toks, i, 4, taken);
        } else if first_on_line && t.line > 0 && prev_line(t.line).map(|p| lone_title.is_match(p)).unwrap_or(false) {
            found = group(&toks, i, 4, taken);
        } else if first_on_line && t.line > 0 && prev_line(t.line).map(|p| p.trim_end().ends_with(':') && label.is_match(p.trim_end())).unwrap_or(false)
            && !text[t.s..line_end(t.line).max(t.s)].chars().any(|c| c.is_ascii_digit()) {
            // "Lieferadresse:" on its own line, the name on the next
            found = group(&toks, i, 4, taken);
        } else if blank {
        } else if chat.is_match(before) {
            // "08:45 Silvia Conti: Ich hab ..."
            let g = group(&toks, i, 3, taken);
            if let Some(&last) = g.last() {
                if text[toks[last].e..].starts_with(':') || text[toks[last].e..].starts_with(".:") {
                    found = g;
                }
            }
        } else if role.is_match(before) {
            let g = group(&toks, i, 3, taken);
            // after "Fahrer, " the name must close with a comma or start with a listed first name: "Ihr Fahrer,
            // Branimir Iliev, hat" holds a name, "Fahrer, Fuhrpark Lübeck" a department
            let comma = before.ends_with(", ");
            let closed = g.last().map(|&k| text[toks[k].e..].starts_with(',')).unwrap_or(false);
            let starts_first = g.first().map(|&k| first_name(toks[k].w)).unwrap_or(false);
            if (!comma || closed || starts_first)
                && (g.iter().filter(|&&k| !PARTICLES.contains(&toks[k].w)).count() >= 2 || g.iter().any(|&k| first_name(toks[k].w))) {
                found = g;
            }
        } else if greeting.is_match(before) {
            let g = group(&toks, i, 3, taken);
            let starts_ok = g.first().map(|&k| first_name(toks[k].w) || l.surnames.contains(toks[k].w)).unwrap_or(false);
            let two = g.iter().filter(|&&k| !PARTICLES.contains(&toks[k].w)).count() >= 2 && g.iter().all(|&k| !noun_like(toks[k].w));
            let thanks = ["Dank an ", "Danke an ", "Grüße an ", "Gruß an ", "danke "].iter().any(|p| before.ends_with(p));
            if starts_ok || two || thanks || before.trim_end().ends_with(|c: char| c.is_alphabetic() || c == ',') && matches!(before.trim().rsplit(' ').next(), Some("Hallo" | "Hi" | "Hey" | "Moin" | "Liebe" | "Lieber" | "Servus" | "Tag" | "Morgen" | "Danke" | "Danke,")) {
                found = g;
            }
        }
        // a signature starts on the line after the closing, or one line later when that line holds no name
        // ("Mit freundlichen Grüßen\nPersonalabteilung\nGundula Franke", but not "...\nSwantje Ruge\nAußendienst")
        // the line checks of a signature run only for the first name word of a line that no rule took yet
        let (mut before_address, mut signature) = (false, false);
        if found.is_empty() && first_on_line && !role_word.is_match(t.w) {
            let middle_free = *middle_memo[t.line].get_or_insert_with(|| last_full[t.line].map(|k| {
                let s = line_starts[k];
                let e = s + line_text(k).len();
                taken.0.range(s..e).all(|(_, (_, kind))| *kind != "name")
            }).unwrap_or(true));
            let after_closing = t.line > 0 && (prev_line(t.line).map(|p| closing.is_match(p)).unwrap_or(false)
                || (middle_free && last_full[t.line].and_then(|k| prev_line(k)).map(|p| closing.is_match(p)).unwrap_or(false)));
            before_address = next_full[t.line].map(|n| {
                let s = line_starts[n];
                let s = s + (line_text(n).len() - line_text(n).trim_start().len());
                taken.0.get(&s).map(|(_, k)| *k == "address").unwrap_or(false)
            }).unwrap_or(false);
            let next_is_role = || next_line(t.line).map(|n| role_line.is_match(n)).unwrap_or(false);
            signature = (after_closing || before_address || next_is_role()) && !role_line.is_match(line_text(t.line));
        }
        if signature {
            // a signature or the name above a street: the line starts with name words and ends after them or at a
            // comma; side-by-side signatures ("Gotthilf Renner   Ngozi Adaeze Eze") continue across the line
            let g = group(&toks, i, 4, taken);
            if let Some(&last) = g.last() {
                let rest = &text[toks[last].e..line_end(t.line).max(toks[last].e)];
                let rest_names = rest.split_whitespace().all(|w| {
                    let w = w.trim_matches(|c: char| !c.is_alphanumeric() && c != '-');
                    w.is_empty() || matches!(w, "und") || PARTICLES.contains(&w) || (nameish(w) && !noun_like(w) && !role_word.is_match(w))
                });
                if (rest.trim().is_empty() || rest.starts_with(',') || rest.starts_with("  ") || rest.starts_with(" ·") || rest.starts_with(" |") || rest_names)
                    && g.iter().all(|&k| !noun_like(toks[k].w)) {
                    found = g;
                    let mut k = last + 1;
                    while k < toks.len() && toks[k].line == t.line && !before_address {
                        let w = toks[k].w;
                        if matches!(w, "·" | "|" | "/" | "&" | "und") {
                            k += 1;
                            continue;
                        }
                        if !(nameish(w) || PARTICLES.contains(&w)) || noun_like(w) || role_word.is_match(w) || !taken.free(toks[k].s, toks[k].e) {
                            break;
                        }
                        found.push(k);
                        k += 1;
                    }
                }
            }
        }
        if found.is_empty() {
            // "Ronja Prellsen (Geschäftsführung)": a role in brackets right after the name words
            let g = group(&toks, i, 4, taken);
            if let Some(&last) = g.last() {
                let after = &text[toks[last].e..];
                if (paren_role.is_match(after) || after_birth.is_match(after)) && g.iter().all(|&k| !noun_like(toks[k].w)) {
                    found = g;
                }
            }
        }
        if found.is_empty() && !first_name(t.w) && !noun_like(t.w) && !t.stop_after && !first_on_line_after_stop(text, t.s) {
            // an unlisted first name before a listed surname: "Gerrit König", "Ignaz Schmidt-Prellmöller"
            if let Some(n) = toks.get(i + 1) {
                if n.line == t.line && !n.open_before && listed_surname_part(n.w) && taken.free(n.s, n.e) {
                    found = group(&toks, i, 3, taken);
                }
            }
        }
        if found.is_empty() && first_name(t.w) {
            let g = group(&toks, i, 4, taken);
            let ambiguous = l.ambiguous.contains(t.w);
            if g.len() >= 2 && (!ambiguous || l.surnames.contains(toks[g[1]].w)) {
                found = g;
            } else if !ambiguous && !NEVER_ALONE.contains(&t.w) && t.w.chars().count() >= 3 {
                found = vec![i];
            }
        }
        if found.is_empty() && l.surnames.contains(t.w) && text[t.e..].starts_with(", ") {
            // "Berg, Anna" in forms and lists
            if let Some(n) = toks.get(i + 1) {
                let ends = text[n.e..].chars().next().map(|c| matches!(c, '\n' | '\r' | '.' | ')' | ';' | ':')).unwrap_or(true);
                if n.line == t.line && (first_name(n.w) || (nameish(n.w) && !noun_like(n.w) && ends)) {
                    found = vec![i, i + 1];
                }
            }
        }
        if found.is_empty() {
            i += 1;
            continue;
        }
        // "Nachname, Vorname" after a label
        let last = *found.last().unwrap();
        if found.len() == 1 && text[toks[last].e..].starts_with(", ") && (label.is_match(before) || title.is_match(before)) {
            if let Some(n) = toks.get(last + 1) {
                if n.line == toks[last].line && nameish(n.w) && !noun_like(n.w) {
                    found.push(last + 1);
                }
            }
        }
        // "Teilnehmende: Piotr Kowalski, Aisha Toure, Hermann Vogt": a label covers the whole list on its line
        if label_here {
            loop {
                let last = *found.last().unwrap();
                let mut rest = &text[toks[last].e..];
                let mut last = last;
                if rest.starts_with(" (") {
                    // "Omar Haddad (Lagerlogistik), Henrike Voss-Kirsch": skip the bracket
                    let Some(close) = rest[..floor(rest, rest.len().min(60))].find(')') else { break };
                    if rest[..close].contains('\n') {
                        break;
                    }
                    let end = toks[last].e + close + 1;
                    while last + 1 < toks.len() && toks[last + 1].s < end {
                        last += 1;
                    }
                    rest = &text[end..];
                }
                let skip = if rest.starts_with(", ") || rest.starts_with("; ") { 1 } else if rest.starts_with(" und ") || rest.starts_with(" sowie ") { 2 } else { 0 };
                let k = last + skip;
                if skip == 0 || k >= toks.len() || toks[k].line != toks[last].line {
                    break;
                }
                let g = group(&toks, k, 3, taken);
                let words = g.iter().filter(|&&x| !PARTICLES.contains(&toks[x].w)).count();
                if g.is_empty() || g.iter().any(|&x| noun_like(toks[x].w)) || (words < 2 && !first_name(toks[g[0]].w)) {
                    break;
                }
                found.extend(g);
            }
        }
        // "Frau von Quistorp": the particles right before the first name word belong to the name
        // only after a title or at the start of a line: "Reisekosten von Nowak" keeps its "von"
        let mut first = found[0];
        while first > 0 && toks[first - 1].line == toks[first].line && PARTICLES.contains(&toks[first - 1].w) && taken.free(toks[first - 1].s, toks[first - 1].e) {
            let p = first - 1;
            let lead = line_before(text, toks[p].s, 120);
            if !(title.is_match(lead) || lead.trim().is_empty() || (p > 0 && toks[p - 1].line == toks[p].line && first_name(toks[p - 1].w))) {
                break;
            }
            first = p;
            found.insert(0, first);
        }
        for &k in &found {
            if taken.add(toks[k].s, toks[k].e, "name") && !PARTICLES.contains(&toks[k].w) {
                hits.push(k);
            }
        }
        i = found.last().unwrap() + 1;
    }
    // one person per line in a list: "Wójcik, Marek (Produktion)", "- Omar Haddad (Lagerlogistik)"
    // surnames under 3 letters ("Vo, Duc") spread only when a list line showed them in the form "Surname, First"
    let mut short: HashSet<&str> = HashSet::new();
    static LIST_LINE: OnceLock<Regex> = OnceLock::new();
    let list_line = re(&LIST_LINE, r"(?m)^[ \t]*(?:[-–•*][ \t]*)?(\p{Lu}[\p{L}'’-]+)(, | )(\p{Lu}[\p{L}'’-]+)( \([^)\n]{1,40}\))?[ \t]*$");
    for c in list_line.captures_iter(text) {
        let (a, sep, b, paren) = (c.get(1).unwrap(), c.get(2).unwrap().as_str(), c.get(3).unwrap(), c.get(4));
        if !(nameish(a.as_str()) && nameish(b.as_str())) || noun_like(a.as_str()) || noun_like(b.as_str()) {
            continue;
        }
        if sep == " " && paren.is_none() {
            continue; // two capitalised words alone on a line are often a heading
        }
        // a word an earlier rule already masked as a name by exactly its own span does not block the line:
        // "Amrani, Omar (Vertrieb)" where "Omar" was found alone as a listed first name
        let open = |g: &regex::Match| taken.free(g.start(), g.end()) || taken.is(g.start(), g.end(), "name");
        if open(&a) && open(&b) {
            taken.add(a.start(), a.end(), "name");
            taken.add(b.start(), b.end(), "name");
            for g in [a, b] {
                if let Some(k) = tok_at(&toks, g.start()).filter(|&k| toks[k].s == g.start()) {
                    hits.push(k);
                }
            }
            if sep == ", " && a.as_str().chars().count() < 3 {
                short.insert(a.as_str());
            }
        }
    }
    // initials: "H.-J. Wilkening, Geschäftsführung", "Frau A. Berg"
    static INITIALS: OnceLock<Regex> = OnceLock::new();
    for c in re(&INITIALS, r"(?:^|[\s(,:])((?:\p{Lu}\.-?){1,3} ?(\p{Lu}[\p{Ll}ß'’]+(?:-\p{Lu}[\p{Ll}ß]+)?))").captures_iter(text) {
        let (all, surname) = (c.get(1).unwrap(), c.get(2).unwrap());
        let after = &text[all.end()..];
        let lead = line_before(text, all.start(), 120);
        if lead.ends_with("i. ") || lead.ends_with("i.") {
            continue; // "i. A. Martin Keller": the "A." of "im Auftrag" is no initial
        }
        let ok = listed_surname_part(surname.as_str()) || after_birth.is_match(after) || title.is_match(lead) || label.is_match(lead);
        // an earlier rule may have masked the surname alone ("Rehberg (Vollmachtgeberin)"): the initials join it
        if ok && nameish(surname.as_str()) && !noun_like(surname.as_str()) && taken.grow(all.start(), all.end(), "name") {
            if let Some(k) = tok_at(&toks, surname.start()) {
                hits.push(k);
            }
        }
    }
    // names found by the table rule spread like the others
    let mut in_hits: HashSet<usize> = hits.iter().copied().collect();
    for (k, t) in toks.iter().enumerate() {
        if let Some((_, (end, kind))) = taken.0.range(..=t.s).next_back() {
            if *kind == "name" && *end >= t.e && name_shape(t.w) && in_hits.insert(k) {
                hits.push(k);
            }
        }
    }
    // the speaker of a chat line, also a handle: "[08:01] hakan.sahin: verstanden"
    static SPEAKER: OnceLock<Regex> = OnceLock::new();
    // "[08:01] hakan.sahin: ...", "ayse.yilmaz (09:12): ...", "Lena Hartmann [08:02]: ..."
    let speaker = re(&SPEAKER, r"(?m)^[ \t]*\**(?:\[?\d{1,2}:\d{2}(?::\d{2})?\]?[ \t]*[-–—]?[ \t]*(\p{L}[\p{L}\d._-]{1,30}(?: \p{L}[\p{L}.'’-]{0,30}){0,2})(?: \([^)\n]{1,20}\))?:|(\p{L}[\p{L}\d._-]{1,30}(?: \p{L}[\p{L}.'’-]{0,30}){0,2}) [(\[]\d{1,2}:\d{2}(?::\d{2})?[)\]]:?)(?:\*\*)?(?:[ \t]|$)");
    let mut seeds: HashSet<String> = HashSet::new();
    for c in speaker.captures_iter(text) {
        let Some(g) = c.get(1).or_else(|| c.get(2)) else { continue };
        let first = g.as_str().split(' ').next().unwrap_or("");
        if lists().stop.contains(first) {
            continue;
        }
        taken.add(g.start(), g.end(), "name");
        seeds.extend(g.as_str().split(['.', '_', '-', ' ']).map(str::to_string));
    }
    // "anna.berg@example.de" names Anna Berg: the parts of an e-mail address seed the spreading below
    for (s, e) in cores(text) {
        let w = &text[s..e];
        if let Some((local, _)) = w.split_once('@') {
            if is_email(w) && local.contains(['.', '_']) {
                seeds.extend(local.split(['.', '_']).map(str::to_string));
            }
        }
    }
    // each distinct seed word once, then one pass over the tokens: a text of 80,000 addresses stays linear
    let mut seed_words: HashSet<String> = HashSet::new();
    for part in seeds {
        let letters: String = part.chars().filter(|c| c.is_alphabetic()).collect();
        if letters.chars().count() < 3 || letters.len() != part.len() {
            continue;
        }
        let mut cap = letters.chars();
        let word: String = cap.next().map(|c| c.to_uppercase().collect::<String>()).unwrap_or_default() + cap.as_str();
        if !nameish(&word) || noun_like(&word) || PARTICLES.contains(&word.to_lowercase().as_str()) {
            continue;
        }
        seed_words.insert(word);
    }
    if !seed_words.is_empty() {
        for (k, t) in toks.iter().enumerate() {
            if seed_words.contains(t.w) && !in_company(&toks, k) && taken.add(t.s, t.e, "name") {
                hits.push(k);
            }
        }
    }
    // "@Silvia Conti" in a chat
    static MENTION: OnceLock<Regex> = OnceLock::new();
    static HANDLE: OnceLock<Regex> = OnceLock::new();
    const FILE_ENDINGS: &[&str] = &["log", "txt", "json", "md", "rs", "py", "js", "ts", "toml", "yaml", "yml", "csv", "pdf", "png", "zip", "sh", "html"];
    for m in re(&HANDLE, r"(?:^|[\s(])@(\p{Ll}[\p{L}\d]*[._][\p{L}\d._]*\p{L})").captures_iter(text) {
        let g = m.get(1).unwrap();
        let ending = g.as_str().rsplit('.').next().unwrap_or("");
        if g.as_str().contains('.') && FILE_ENDINGS.contains(&ending) {
            continue;
        }
        taken.add(g.start(), g.end(), "name");
    }
    for c in re(&MENTION, r"(?:^|[\s(])@(\p{Lu}[\p{L}'’-]+)(?: (\p{Lu}[\p{L}'’-]+))?").captures_iter(text) {
        let first = c.get(1).unwrap();
        if !nameish(first.as_str()) {
            continue;
        }
        if taken.add(first.start(), first.end(), "name") {
            if let Some(k) = tok_at(&toks, first.start()).filter(|&k| toks[k].s == first.start()) {
                hits.push(k);
            }
        }
        if let Some(second) = c.get(2) {
            if nameish(second.as_str()) && !noun_like(second.as_str()) && taken.add(second.start(), second.end(), "name") {
                if let Some(k) = tok_at(&toks, second.start()).filter(|&k| toks[k].s == second.start()) {
                    hits.push(k);
                }
            }
        }
    }
    // A name word found once is a name everywhere in the same text: "Frau Berg" in the address makes "Berg" in the
    // body a name too. A capitalised word right before a found name is its first name: "Yara Lindqvist". Both steps
    // repeat, so a first name found next to a spread surname spreads too.
    let mut fresh = hits;
    let mut known: HashSet<&str> = HashSet::new();
    for _ in 0..3 {
        let words: Vec<&str> = fresh.iter().map(|&k| toks[k].w)
            .filter(|w| (w.chars().count() >= 3 || short.contains(w)) && !PARTICLES.contains(w) && !known.contains(w)).collect();
        if words.is_empty() && fresh.is_empty() {
            break;
        }
        known.extend(words);
        let mut next: Vec<usize> = Vec::new();
        for (k, t) in toks.iter().enumerate() {
            if known.contains(t.w) && !in_company(&toks, k) && taken.add(t.s, t.e, "name") {
                next.push(k);
            }
        }
        let mut grown: Vec<usize> = Vec::new();
        for &k in fresh.iter().chain(next.iter()) {
            let mut k = k;
            while k > 0 {
                let p = &toks[k - 1];
                let article = k >= 2 && toks[k - 2].line == p.line && ARTICLES.contains(&toks[k - 2].w.to_lowercase().as_str());
                let role_word = role.is_match(&format!("{} ", p.w));
                if p.line != toks[k].line || p.stop_after || !nameish(p.w) || noun_like(p.w) || article || role_word || in_company(&toks, k - 1) {
                    break;
                }
                let first = k - 1 == 0 || toks[k - 2].line != p.line;
                if first && !first_name(p.w) {
                    break;
                }
                if !taken.add(p.s, p.e, "name") {
                    break;
                }
                grown.push(k - 1);
                k -= 1;
            }
        }
        fresh = next.into_iter().chain(grown).collect();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(text: &str) -> Vec<(String, &'static str)> {
        spans(text).into_iter().map(|s| (text[s.start..s.end].to_string(), s.kind)).collect()
    }

    #[test]
    fn checksums_follow_the_standards() {
        assert!(iban_checksum_ok("DE89370400440532013000"));
        assert!(!iban_checksum_ok("DE89370400440532013001"));
        assert!(iban_checksum_ok("NL91ABNA0417164300"));
        assert!(iban_checksum_ok("AT611904300234573201"));
        assert!(iban_checksum_ok("CH9300762011623852957"));
        // the example id of the Bundeszentralamt fuer Steuern
        assert!(tax_id_ok("86095742719"));
        assert!(!tax_id_ok("86095742718"));
        assert!(!tax_id_ok("06095742719"));
    }

    #[test]
    fn finds_each_kind_in_a_letter() {
        let text = "Herrn\nJonas Weber\nLindenweg 4\n20457 Hamburg\n\nSehr geehrter Herr Weber,\nIhre IBAN DE89 3704 0044 0532 0130 00.\n\
Steuer-ID: 86 095 742 719, geboren am 03.07.1990.\nTel. 040 1234567-12, jonas.weber@example.org\n\nMit freundlichen Grüßen\nYara Lindqvist\n";
        let got = kinds(text);
        let want = [("Jonas", "name"), ("Weber", "name"), ("Lindenweg 4", "address"), ("20457 Hamburg", "address"), ("Weber", "name"),
                    ("DE89 3704 0044 0532 0130 00", "iban"), ("86 095 742 719", "tax_id"), ("03.07.1990", "birth_date"),
                    ("040 1234567-12", "phone"), ("jonas.weber@example.org", "email"), ("Yara", "name"), ("Lindqvist", "name")];
        let want: Vec<(String, &str)> = want.iter().map(|(a, b)| (a.to_string(), *b)).collect();
        assert_eq!(got, want);
    }

    #[test]
    fn line_prefixes_and_windows_line_ends_give_the_same_items() {
        let text = "Teilnehmende:\nAmrani, Omar (Vertrieb Messen)\nVo, Duc (Lizenzen)\n\nVo berichtet. Ihr Fahrer, Branimir Iliev, hat notiert.\n\
Kontakt: Lindenweg 4\n20457 Hamburg\n\nMit freundlichen Grüßen\nYara Lindqvist\n[08:01] hakan.sahin: verstanden\n";
        let want = kinds(text);
        assert!(want.contains(&("Amrani".to_string(), "name")) && want.contains(&("Yara".to_string(), "name")), "{:?}", want);
        assert_eq!(kinds(&text.replace('\n', "\r\n")), want);
        let read: Vec<String> = text.lines().enumerate().map(|(i, l)| format!("{:>5}  {}", i + 1, l)).collect();
        assert_eq!(kinds(&read.join("\n")), want);
        assert_eq!(kinds(&read.join("\r\n")), want);
        let grep: Vec<String> = text.lines().enumerate().map(|(i, l)| format!("C:\\notes\\a.txt:{}: {}", i + 1, l)).collect();
        assert_eq!(kinds(&grep.join("\n")), want);
        // a phone number at a line start is no line number: it never starts with 0
        assert_eq!(kinds("0171  2345678 ist meine Nummer"), vec![("0171  2345678".to_string(), "phone")]);
    }

    #[test]
    fn list_lines_short_surnames_roles_with_a_comma_and_initials() {
        let text = "Teilnehmende (alphabetisch):\nAmrani, Omar (Vertrieb Messen)\nHinrichs, Fentje (Sekretariat)\nVo, Duc (Lizenzen)\n\n\
Amrani koordiniert. Vo berichtet. Ihr Fahrer, Branimir Iliev, hat den Mangel notiert.\n\ngez. O. Rehberg (Vollmachtgeberin)\n\
gez. Jan F. Rehberg\nName des Empfängers: T. Fleischer-Zorn.\n";
        let names: Vec<String> = kinds(text).into_iter().filter(|(_, k)| *k == "name").map(|(t, _)| t).collect();
        for want in ["Amrani", "Omar", "Hinrichs", "Fentje", "Vo", "Duc", "Branimir", "Iliev", "O. Rehberg", "F", "T. Fleischer-Zorn"] {
            assert!(names.iter().any(|n| n == want), "{} not in {:?}", want, names);
        }
        assert_eq!(names.iter().filter(|n| *n == "Amrani").count(), 2);
        assert_eq!(names.iter().filter(|n| *n == "Vo").count(), 2);
        // "Fahrer, Fuhrpark Lübeck" is a department, "i. A." no initial
        assert!(kinds("Viele Grüße\nGerhard Wuttke\nFahrer, Fuhrpark Lübeck\n").iter().all(|(t, _)| t != "Fuhrpark" && t != "Lübeck"));
        assert!(kinds("Freundliche Grüße\ni. A. Martin Keller\n").iter().all(|(t, _)| !t.contains("A.")));
    }

    #[test]
    fn a_table_of_tools_under_a_plain_name_header_stays() {
        let doc = "| Name | Kind | Base address |\n|------|------|------|\n| claude-cli | cli | - |\n| parser | lib | - |\n";
        assert_eq!(kinds(doc), Vec::<(String, &str)>::new());
        let people = "| Name | Abteilung | Raum |\n|---|---|---|\n| Anna Berg | Einkauf | 2.14 |\n";
        assert_eq!(kinds(people), vec![("Anna Berg".to_string(), "name")]);
    }

    /// Crafted worst cases: e-mail locals, initials, list lines, mentions and titles, each repeated. They used to
    /// cost quadratic time (6 s for 1 MB of e-mail locals in release). Four times the text may cost at most 7 times
    /// the time; linear gives 4, quadratic 16. Nine times plus 0.1 s keeps the check stable on a loaded Windows runner.
    #[test]
    fn crafted_inputs_stay_linear() {
        for unit in ["kaj.ole@x.de ", "A. Müller\n", "Vo, Duc (Lizenzen)\n", "@Silvia Conti ok\n", "Herr Otto Berg und Frau Anna Berg. "] {
            let time = |kb: usize| {
                let text = unit.repeat(kb * 1024 / unit.len());
                let started = std::time::Instant::now();
                assert!(!spans(&text).is_empty(), "{}", unit);
                started.elapsed().as_secs_f64()
            };
            let small = time(32).min(time(32));
            let large = time(128);
            assert!(large < 9.0 * small + 0.1, "{:?}: 32 KB {:.3} s, 128 KB {:.3} s", unit, small, large);
        }
    }

    #[test]
    fn leaves_business_numbers_and_companies_alone() {
        for plain in ["Rechnung Nr. RE-2026-004711 vom 12.03.2026", "Bestellnummer 4500123456", "Art.-Nr. 0815-4711-22",
                      "USt-IdNr.: DE123456789", "Amtsgericht Hamburg HRB 12345", "Nordwerk Logistik GmbH", "Summe: 1.234,56 EUR",
                      "Sehr geehrte Damen und Herren,", "Liebe Kolleginnen und Kollegen,", "Kundennummer 0045812734",
                      "Das Arbeitsverhältnis beginnt am 01.04.2026.", "10000 Stück geliefert", "Im Jahr 2025 stieg der Umsatz",
                      "max_tokens=4096", "fn password_hash(input: &str) -> String", "git@github.com:owner/repo.git"] {
            assert_eq!(kinds(plain), Vec::<(String, &str)>::new(), "{}", plain);
        }
    }
}
