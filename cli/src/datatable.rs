//! Tables from pages and data files, number matching across formats, and the filter-and-rank step of deep research.
//!
//! A leaderboard page is often a markdown table after reading, or it links a CSV or JSON file. `tables_of` parses
//! all three. `select` keeps the rows that meet the question's criteria (for example parameters under 2B), computes
//! the rank from the kept rows and returns a table, so a model never has to copy rows or ranks by hand. A parameter
//! count that the table lacks is read from the model name ("Qwen3-0.6B") and marked as derived.
//!
//! `num_tokens` normalises numbers so that 21.1, 21.10, "21,1 %" and 450M and 0.45B compare equal.

use regex::Regex;
use serde_json::Value;
use std::collections::HashSet;
use std::sync::OnceLock;

macro_rules! rx {
    ($p:expr) => {{
        static R: OnceLock<Regex> = OnceLock::new();
        R.get_or_init(|| Regex::new($p).unwrap())
    }};
}

const MAX_ROWS: usize = 5000;
const SHOWN_ROWS: usize = 30;

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Table {
    pub source: String,
    pub primary: bool,
    pub header: Vec<String>,
    pub rows: Vec<Vec<String>>,
}

// ---------- numbers ----------

/// One number in a text: where it stands, and every form it can be compared in.
#[derive(Clone, Debug, PartialEq)]
pub struct NumTok {
    pub pos: usize,
    pub raw: String,
    pub value: f64,
    /// The factor of a unit written next to it (B, M, K, billion, million), when there is one.
    pub scale: Option<f64>,
    pub forms: Vec<String>,
    pub digits: usize,
}

fn fmt(f: f64) -> String {
    if f.fract() == 0.0 && f.abs() < 1e15 { format!("{}", f as i64) } else { format!("{}", f) }
}

/// The value of a number written with thousands separators or a decimal comma: 1,024 and 21,1 and 1.234,5 and 21.10.
pub fn parse_number(raw: &str) -> Option<f64> {
    let t: String = raw.chars().filter(|c| !c.is_whitespace() && *c != '\u{a0}').collect();
    let t = t.trim_end_matches('%');
    let (commas, dots) = (t.matches(',').count(), t.matches('.').count());
    let cleaned = if commas > 0 && dots > 0 {
        if t.rfind('.') > t.rfind(',') { t.replace(',', "") } else { t.replace('.', "").replace(',', ".") }
    } else if commas > 0 {
        if rx!(r"^\d{1,3}(,\d{3})+$").is_match(t) { t.replace(',', "") } else { t.replace(',', ".") }
    } else {
        t.to_string()
    };
    cleaned.parse::<f64>().ok()
}

/// Every number of a text with its position (in characters) and its comparable forms.
pub fn num_tokens(text: &str) -> Vec<NumTok> {
    let re = rx!(r"(\d[\d.,]*\d|\d)(?:\s?(%|billion|million|thousand|bn)\b|([kKmMbB])\b)?");
    let mut out = Vec::new();
    let (mut at_byte, mut at_char) = (0usize, 0usize);
    for c in re.captures_iter(text) {
        let m = c.get(1).unwrap();
        at_char += text[at_byte..m.start()].chars().count();
        at_byte = m.start();
        let Some(value) = parse_number(m.as_str()) else { continue };
        let unit = c.get(2).or_else(|| c.get(3)).map(|u| u.as_str().to_lowercase());
        let scale = match unit.as_deref() {
            Some("b") | Some("bn") | Some("billion") => Some(1e9),
            Some("m") | Some("million") => Some(1e6),
            Some("k") | Some("thousand") => Some(1e3),
            _ => None,
        };
        let mut forms = vec![fmt(value)];
        if let Some(s) = scale {
            forms.push(format!("u{}", (value * s).round() as i64));
        }
        let digits = m.as_str().chars().filter(|c| c.is_ascii_digit()).count();
        out.push(NumTok { pos: at_char, raw: m.as_str().to_string(), value, scale, forms, digits });
    }
    out
}

/// The numbers of `claim` (two digits or more) that have no equal form in `page`. Equal means the same value, however
/// it is written: 21.1, 21.10, 21,1 %, 1,024 and 1024, 450M and 0.45B.
pub fn missing_numbers(claim: &str, page: &str) -> Vec<String> {
    let have: HashSet<String> = num_tokens(page).into_iter().flat_map(|t| t.forms).collect();
    num_tokens(claim).into_iter().filter(|t| t.digits >= 2 && !t.forms.iter().any(|f| have.contains(f))).map(|t| t.raw).collect()
}

/// The value of a cell as a quantity: a number with a unit counts in base units, a bare number is scaled by `header_scale`.
fn quantity(cell: &str, header_scale: f64) -> Option<f64> {
    let t = num_tokens(cell).into_iter().next()?;
    Some(match t.scale {
        Some(s) => t.value * s,
        None => t.value * header_scale,
    })
}

fn header_scale(h: &str) -> f64 {
    let l = h.to_lowercase();
    if l.contains("(b)") || l.contains("billion") || l.contains("(bn)") || l.contains("(b params)") { 1e9 } else if l.contains("(m)") || l.contains("million") { 1e6 } else { 1.0 }
}

/// The parameter count in a model name: Qwen3-0.6B, xLAM-2-1b-fc-r, LFM2.5-VL-450M. In base units.
pub fn params_from_name(name: &str) -> Option<f64> {
    let re = rx!(r"(?i)(?:^|[^a-z0-9.])(\d+(?:\.\d+)?)\s?([bm])(?:[^a-z0-9]|$)");
    re.captures_iter(name).filter_map(|c| {
        let v: f64 = c[1].parse().ok()?;
        Some(v * if c[2].eq_ignore_ascii_case("b") { 1e9 } else { 1e6 })
    }).next()
}

// ---------- parsing ----------

fn pad(rows: &mut [Vec<String>], n: usize) {
    for r in rows.iter_mut() {
        r.resize(n, String::new());
    }
}

fn table(source: &str, mut rows: Vec<Vec<String>>) -> Option<Table> {
    if rows.len() < 3 {
        return None;
    }
    let n = rows.iter().map(|r| r.len()).max().unwrap_or(0);
    if n < 2 {
        return None;
    }
    pad(&mut rows, n);
    let header = rows.remove(0);
    rows.truncate(MAX_ROWS);
    Some(Table { source: source.to_string(), primary: false, header, rows })
}

/// The markdown tables of a page: runs of lines that start with `|`.
pub fn from_markdown(source: &str, md: &str) -> Vec<Table> {
    let mut out = Vec::new();
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut flush = |rows: &mut Vec<Vec<String>>| {
        if let Some(t) = table(source, std::mem::take(rows)) {
            out.push(t);
        }
    };
    for line in md.lines() {
        let l = line.trim();
        if !l.starts_with('|') {
            flush(&mut rows);
            continue;
        }
        let cells: Vec<String> = l.trim_matches('|').split('|').map(|c| c.trim().to_string()).collect();
        if cells.iter().all(|c| c.chars().all(|ch| ch == '-' || ch == ':' || ch == ' ')) {
            continue;
        }
        rows.push(cells);
    }
    flush(&mut rows);
    out
}

fn csv_rows(text: &str, delim: char) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    let (mut row, mut cell, mut quoted) = (Vec::new(), String::new(), false);
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if quoted && chars.peek() == Some(&'"') => {
                cell.push('"');
                chars.next();
            }
            '"' => quoted = !quoted,
            c if c == delim && !quoted => row.push(std::mem::take(&mut cell).trim().to_string()),
            '\n' | '\r' if !quoted => {
                if c == '\r' && chars.peek() == Some(&'\n') {
                    chars.next();
                }
                row.push(std::mem::take(&mut cell).trim().to_string());
                if row.iter().any(|c| !c.is_empty()) {
                    rows.push(std::mem::take(&mut row));
                } else {
                    row.clear();
                }
                if rows.len() > MAX_ROWS {
                    break;
                }
            }
            c => cell.push(c),
        }
    }
    if !cell.is_empty() || !row.is_empty() {
        row.push(cell.trim().to_string());
        rows.push(row);
    }
    rows
}

/// A CSV or TSV file. The delimiter is the one that splits the first line into most columns.
pub fn from_csv(source: &str, text: &str) -> Option<Table> {
    let first = text.lines().next().unwrap_or("");
    if first.trim_start().starts_with('<') {
        return None;
    }
    let delim = [',', ';', '\t'].into_iter().max_by_key(|d| first.matches(*d).count()).unwrap();
    if first.matches(delim).count() < 1 {
        return None;
    }
    table(source, csv_rows(text, delim))
}

fn cell_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        _ => String::new(),
    }
}

type Items<'a> = Vec<(Option<String>, &'a serde_json::Map<String, Value>)>;

fn objects_table(source: &str, items: Items) -> Option<Table> {
    let mut header: Vec<String> = Vec::new();
    if items.iter().any(|(k, _)| k.is_some()) {
        header.push("name".to_string());
    }
    for (_, o) in items.iter().take(200) {
        for k in o.keys() {
            if !header.contains(k) {
                header.push(k.clone());
            }
        }
    }
    let mut rows = vec![header.clone()];
    for (key, o) in items.iter().take(MAX_ROWS) {
        rows.push(header.iter().map(|h| if h == "name" && key.is_some() && !o.contains_key("name") { key.clone().unwrap_or_default() } else { o.get(h).map(cell_text).unwrap_or_default() }).collect());
    }
    table(source, rows)
}

/// A JSON file that holds a list of objects, an object with such a list under one key, or an object of objects.
pub fn from_json(source: &str, text: &str) -> Option<Table> {
    let v: Value = serde_json::from_str(text.trim()).ok()?;
    fn list(v: &Value) -> Option<Items<'_>> {
        let a = v.as_array()?;
        let items: Vec<_> = a.iter().filter_map(|x| x.as_object().map(|o| (None, o))).collect();
        if items.len() >= 3 { Some(items) } else { None }
    }
    if let Some(items) = list(&v) {
        return objects_table(source, items);
    }
    let obj = v.as_object()?;
    for child in obj.values() {
        if let Some(items) = list(child) {
            return objects_table(source, items);
        }
    }
    let items: Vec<_> = obj.iter().filter_map(|(k, x)| x.as_object().map(|o| (Some(k.clone()), o))).collect();
    if items.len() >= 3 { objects_table(source, items) } else { None }
}

/// Every table in a fetched page or data file, by content type: JSON, CSV or TSV, and markdown tables of an HTML page.
pub fn tables_of(url: &str, content_type: &str, text: &str) -> Vec<Table> {
    let lower = content_type.to_lowercase();
    let start = text.trim_start();
    let mut out = Vec::new();
    if lower.contains("json") || start.starts_with('{') || start.starts_with('[') {
        out.extend(from_json(url, text));
    }
    let path = url.split(['?', '#']).next().unwrap_or("").to_lowercase();
    if lower.contains("csv") || lower.contains("tab-separated") || path.ends_with(".csv") || path.ends_with(".tsv") {
        out.extend(from_csv(url, text));
    }
    out.extend(from_markdown(url, text));
    out
}

/// Links of a page that point at a data file, the likely leaderboard files first.
pub fn data_links(links: &[String]) -> Vec<String> {
    let mut found: Vec<&String> = links.iter().filter(|l| {
        let p = l.split(['?', '#']).next().unwrap_or("").to_lowercase();
        p.ends_with(".csv") || p.ends_with(".json") || p.ends_with(".tsv")
    }).collect();
    let hint = |l: &str| ["overall", "score", "leaderboard", "data", "result"].iter().filter(|k| l.to_lowercase().contains(*k)).count();
    found.sort_by_key(|l| std::cmp::Reverse(hint(l)));
    found.into_iter().take(3).cloned().collect()
}

/// Where a leaderboard usually publishes its data when the page only holds a script: next to the page or at the site root.
pub fn guess_data_files(url: &str) -> Vec<String> {
    let Some((scheme, rest)) = url.split_once("://") else { return Vec::new() };
    let host = rest.split('/').next().unwrap_or("");
    let path = rest[host.len()..].split(['?', '#']).next().unwrap_or("");
    let dir = path.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
    let mut out = Vec::new();
    for base in [dir, ""] {
        for name in ["data_overall.csv", "data.csv", "leaderboard.csv", "results.json"] {
            let u = format!("{}://{}{}/{}", scheme, host, base, name);
            if !out.contains(&u) {
                out.push(u);
            }
        }
    }
    out.truncate(6);
    out
}

// ---------- filter and rank ----------

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Op {
    Lt,
    Le,
    Gt,
    Ge,
    Eq,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Filter {
    pub column: String,
    pub op: Op,
    pub value: f64,
    /// B, M, K or empty.
    pub unit: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Spec {
    pub filters: Vec<Filter>,
    pub rank_by: Option<String>,
    pub descending: bool,
}

/// Reads `{"filters":[{"column":"parameters","op":"<","value":2,"unit":"B"}],"rank_by":"overall","descending":true}`.
pub fn parse_spec(v: &Value) -> Spec {
    let filters = v["filters"].as_array().map(|a| a.iter().filter_map(|f| {
        let column = f["column"].as_str()?.trim().to_string();
        let op = match f["op"].as_str().unwrap_or("") {
            "<" => Op::Lt,
            "<=" => Op::Le,
            ">" => Op::Gt,
            ">=" => Op::Ge,
            "=" | "==" => Op::Eq,
            _ => return None,
        };
        let value = f["value"].as_f64().or_else(|| f["value"].as_str().and_then(parse_number))?;
        let unit = f["unit"].as_str().unwrap_or("").trim().to_uppercase();
        if column.is_empty() { None } else { Some(Filter { column, op, value, unit }) }
    }).collect()).unwrap_or_default();
    Spec { filters, rank_by: v["rank_by"].as_str().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()), descending: v["descending"].as_bool().unwrap_or(true) }
}

fn norm(h: &str) -> String {
    h.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_lowercase()
}

fn param_like(hint: &str) -> bool {
    let n = norm(hint);
    n.contains("param") || n.contains("size")
}

fn find_col(header: &[String], hint: &str) -> Option<usize> {
    let h = norm(hint);
    if h.is_empty() {
        return None;
    }
    let by = |f: &dyn Fn(&str) -> bool| header.iter().position(|c| f(&norm(c)) && !norm(c).contains("link") && !norm(c).contains("url"));
    by(&|c| c == h).or_else(|| by(&|c| c.contains(&h) || (c.len() >= 4 && h.contains(c)))).or_else(|| if param_like(hint) { by(&|c| c.contains("param")) } else { None })
}

fn name_col(header: &[String]) -> usize {
    header.iter().position(|c| { let n = norm(c); (n.contains("model") || n == "name") && !n.contains("link") && !n.contains("url") }).unwrap_or(0)
}

fn default_rank_col(t: &Table) -> Option<usize> {
    ["overall", "score", "accuracy", "acc"].iter().find_map(|k| find_col(&t.header, k))
}

/// The rows of one table that meet the criteria, ranked.
#[derive(Clone, Debug, PartialEq)]
pub struct Selection {
    pub source: String,
    pub primary: bool,
    pub header: Vec<String>,
    pub rows: Vec<Vec<String>>,
    /// Rows of the table.
    pub considered: usize,
    /// Rows left out because a filtered value or the rank value could not be read.
    pub unknown: usize,
    pub kept: usize,
    pub rank_col: String,
    pub criteria: String,
}

fn pass(op: Op, a: f64, b: f64) -> bool {
    match op {
        Op::Lt => a < b,
        Op::Le => a <= b,
        Op::Gt => a > b,
        Op::Ge => a >= b,
        Op::Eq => (a - b).abs() < 1e-9 * b.abs().max(1.0),
    }
}

fn unit_scale(u: &str) -> f64 {
    match u {
        "B" => 1e9,
        "M" => 1e6,
        "K" => 1e3,
        _ => 1.0,
    }
}

fn shown_size(v: f64) -> String {
    if v >= 1e9 { format!("{}B", fmt((v / 1e7).round() / 100.0)) } else if v >= 1e6 { format!("{}M", fmt((v / 1e4).round() / 100.0)) } else { fmt(v) }
}

/// Keeps the rows of `t` that meet the filters and ranks them by the rank column. The rank counts the kept rows only
/// (ties share a rank), never the rank the source prints. `None` when the table has no model column, no rank column
/// or none of its rows passes.
pub fn select(t: &Table, spec: &Spec) -> Option<Selection> {
    let name_i = name_col(&t.header);
    let rank_i = match &spec.rank_by {
        Some(h) => find_col(&t.header, h)?,
        None => default_rank_col(t)?,
    };
    if rank_i == name_i {
        return None;
    }
    let cols: Vec<Option<usize>> = spec.filters.iter().map(|f| find_col(&t.header, &f.column)).collect();
    // A filter column the table lacks is read from the model name when it is a size.
    if spec.filters.iter().zip(&cols).any(|(f, c)| c.is_none() && !param_like(&f.column)) {
        return None;
    }
    let mut kept: Vec<(f64, String, Vec<String>)> = Vec::new();
    let mut unknown = 0;
    for r in &t.rows {
        let name = r.get(name_i).cloned().unwrap_or_default();
        if name.is_empty() {
            continue;
        }
        let Some(score) = r.get(rank_i).and_then(|c| quantity(c, 1.0)) else { unknown += 1; continue };
        let mut ok = true;
        let mut shown: Vec<String> = Vec::new();
        for (f, c) in spec.filters.iter().zip(&cols) {
            let (value, text) = match c {
                Some(i) => {
                    let cell = r.get(*i).cloned().unwrap_or_default();
                    (quantity(&cell, header_scale(&t.header[*i])), cell)
                }
                None => match params_from_name(&name) {
                    Some(v) => (Some(v), format!("{} (from name)", shown_size(v))),
                    None => (None, String::new()),
                },
            };
            let Some(v) = value else { ok = false; unknown += 1; break };
            if !pass(f.op, v, f.value * unit_scale(&f.unit)) {
                ok = false;
                break;
            }
            shown.push(text);
        }
        if ok {
            let mut row = vec![name.clone()];
            row.extend(shown);
            row.push(r.get(rank_i).cloned().unwrap_or_default());
            kept.push((score, name.to_lowercase(), row));
        }
    }
    // One row per model: the best score.
    kept.sort_by(|a, b| if spec.descending { b.0.partial_cmp(&a.0) } else { a.0.partial_cmp(&b.0) }.unwrap_or(std::cmp::Ordering::Equal));
    let mut seen = HashSet::new();
    kept.retain(|(_, n, _)| seen.insert(n.clone()));
    if kept.is_empty() {
        return None;
    }
    let mut header = vec!["Rank".to_string(), t.header[name_i].clone()];
    for (f, c) in spec.filters.iter().zip(&cols) {
        header.push(match c { Some(i) => t.header[*i].clone(), None => format!("{} (from the model name)", f.column) });
    }
    header.push(t.header[rank_i].clone());
    let mut rows = Vec::new();
    let mut rank = 0usize;
    let mut last: Option<f64> = None;
    for (i, (score, _, cells)) in kept.iter().enumerate() {
        if last != Some(*score) {
            rank = i + 1;
            last = Some(*score);
        }
        let mut row = vec![rank.to_string()];
        row.extend(cells.clone());
        rows.push(row);
    }
    let criteria = spec.filters.iter().map(|f| format!("{} {} {}{}", f.column, match f.op { Op::Lt => "<", Op::Le => "<=", Op::Gt => ">", Op::Ge => ">=", Op::Eq => "=" }, fmt(f.value), f.unit)).collect::<Vec<_>>().join(" and ");
    let kept_n = rows.len();
    rows.truncate(SHOWN_ROWS);
    Some(Selection { source: t.source.clone(), primary: t.primary, header, rows, considered: t.rows.len(), unknown, kept: kept_n, rank_col: t.header[rank_i].clone(), criteria })
}

/// The best selection over several tables: a primary source before a secondary one, then the one with most rows.
pub fn best(tables: &[Table], spec: &Spec) -> Option<Selection> {
    tables.iter().filter_map(|t| select(t, spec)).max_by_key(|s| (s.primary, s.kept))
}

impl Selection {
    /// The caption and the table, as it stands at the top of the answer.
    pub fn markdown(&self) -> String {
        let mut out = format!("Computed by Penelope from the rows of {} ({}): {} of {} rows{}, ranked by {}. The rank counts the kept rows, not the rank the source prints.{}\n\n",
            self.source, if self.primary { "primary source" } else { "secondary source, third party" }, self.kept, self.considered,
            if self.criteria.is_empty() { String::new() } else { format!(" with {}", self.criteria) }, self.rank_col,
            if self.unknown > 0 { format!(" {} rows were left out because a value could not be read.", self.unknown) } else { String::new() });
        out.push_str(&format!("| {} |\n|{}|\n", self.header.join(" | "), vec!["---"; self.header.len()].join("|")));
        for r in &self.rows {
            out.push_str(&format!("| {} |\n", r.iter().map(|c| c.replace('|', "/")).collect::<Vec<_>>().join(" | ")));
        }
        if self.kept > self.rows.len() {
            out.push_str(&format!("\nThe first {} of {} rows are shown.\n", self.rows.len(), self.kept));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const HTML_TABLE_MD: &str = "Intro\n\n| Rank | Model | Overall Acc | Total Cost |\n| 1 | GPT-5 (FC) | 72.10 | 10.2 |\n| 2 | Qwen3-0.6B (FC) | 21,1 % | 0.1 |\n| 3 | Llama-3.1-70B-Instruct | 60.5 | 3 |\n| 4 | xLAM-2-1b-fc-r | 33.4 | 0.2 |\n| 5 | Mystery Model | 40.0 | 1 |\n| 6 | MiniCPM5-1B | 25.1 | 0.1 |\n\nAfter";

    const CSV: &str = "Rank,Model,Model Link,Overall Acc,Params (B)\n1,GPT-5,https://x.example/a,72.1,\n2,\"Tiny, Model\",https://x.example/b,\"21,10\",0.45\n3,Big Model,https://x.example/c,60.5,70\n4,Small One,https://x.example/d,30.2,1.5\n";

    fn spec() -> Spec {
        parse_spec(&json!({"filters": [{"column": "parameters", "op": "<", "value": 2, "unit": "B"}], "rank_by": "overall", "descending": true}))
    }

    #[test]
    fn numbers_compare_across_formats_and_units() {
        for page in ["scored 21.1", "scored 21.10", "scored 21,1 %", "scored 21.1%", "| LFM | 21.10 |"] {
            assert!(missing_numbers("overall score 21.1%", page).is_empty(), "{}", page);
        }
        assert!(missing_numbers("it has 1,024 units", "1024 units").is_empty() && missing_numbers("it has 1024 units", "1,024 units").is_empty());
        assert!(missing_numbers("1.234,5 euro", "1,234.5 euro").is_empty());
        assert!(missing_numbers("a 450M model", "a 0.45B model").is_empty() && missing_numbers("a 0.6B model", "600M parameters").is_empty());
        assert_eq!(missing_numbers("scored 21.2", "scored 21.1 and 21.10"), vec!["21.2".to_string()]);
        assert_eq!(missing_numbers("scored 77", "scored 7 and 770"), vec!["77".to_string()]);
        assert_eq!(parse_number("21,1 %"), Some(21.1));
        assert_eq!(parse_number("1,024"), Some(1024.0));
        assert_eq!(params_from_name("Qwen3-0.6B (FC)"), Some(6e8));
        assert_eq!(params_from_name("LFM2.5-VL-450M"), Some(4.5e8));
        assert_eq!(params_from_name("Llama-3.1-70B-Instruct"), Some(7e10));
        assert_eq!(params_from_name("GPT-4o-mini"), None);
        let toks = num_tokens("a 12 ms b 3,5");
        assert_eq!(toks[0].pos, 2);
        assert_eq!(toks[1].forms, vec!["3.5".to_string()]);
    }

    #[test]
    fn a_markdown_table_a_csv_and_a_json_file_become_tables() {
        let md = from_markdown("https://lb.example/", HTML_TABLE_MD);
        assert_eq!(md.len(), 1);
        assert_eq!(md[0].header, vec!["Rank", "Model", "Overall Acc", "Total Cost"]);
        assert_eq!(md[0].rows.len(), 6);
        let csv = from_csv("https://lb.example/data.csv", CSV).unwrap();
        assert_eq!(csv.header.len(), 5);
        assert_eq!(csv.rows[1][1], "Tiny, Model", "a quoted comma stays in the cell");
        assert_eq!(csv.rows[1][3], "21,10");
        let json = from_json("https://lb.example/d.json", &json!({"updated": "2026", "models": [
            {"model": "A-1B", "score": 10.5, "params": 1.0e9}, {"model": "B-7B", "score": 20.5, "params": 7.0e9}, {"model": "C-0.5B", "score": 5, "params": 5.0e8}]}).to_string()).unwrap();
        assert_eq!(json.header, vec!["model", "params", "score"], "serde_json sorts the keys");
        assert_eq!(json.rows.len(), 3);
        let by_key = from_json("u", &json!({"a": {"score": 1}, "b": {"score": 2}, "c": {"score": 3}}).to_string()).unwrap();
        assert_eq!(by_key.header, vec!["name", "score"]);
        assert!(from_json("u", "[1,2,3]").is_none() && from_csv("u", "<html><body>x</body></html>").is_none());
        // By content type and address.
        assert_eq!(tables_of("https://lb.example/x.csv", "text/csv", CSV).len(), 1);
        assert_eq!(tables_of("https://lb.example/api", "application/json", "[{\"a\":1,\"b\":2},{\"a\":3,\"b\":4},{\"a\":5,\"b\":6}]").len(), 1);
        assert_eq!(tables_of("https://lb.example/", "text/html", HTML_TABLE_MD).len(), 1);
        assert_eq!(guess_data_files("https://gorilla.cs.berkeley.edu/leaderboard.html")[0], "https://gorilla.cs.berkeley.edu/data_overall.csv");
        assert!(guess_data_files("https://x.example/a/b/lb.html").contains(&"https://x.example/a/b/data.csv".to_string()));
        assert_eq!(data_links(&["https://a.example/x.png".into(), "https://a.example/other.csv".into(), "https://a.example/data_overall.csv".into()])[0], "https://a.example/data_overall.csv");
    }

    #[test]
    fn rows_are_filtered_by_size_and_ranked_from_the_kept_rows() {
        let t = &from_markdown("https://lb.example/", HTML_TABLE_MD)[0];
        let s = select(t, &spec()).unwrap();
        // Under 2B by name: Qwen3-0.6B (21.1), xLAM-2-1b (33.4), MiniCPM5-1B (25.1). Mystery Model has no size in its name.
        assert_eq!(s.kept, 3);
        assert_eq!(s.unknown, 2, "GPT-5 and Mystery Model have no size in their names: {:?}", s);
        let models: Vec<&str> = s.rows.iter().map(|r| r[1].as_str()).collect();
        assert_eq!(models, ["xLAM-2-1b-fc-r", "MiniCPM5-1B", "Qwen3-0.6B (FC)"]);
        let ranks: Vec<&str> = s.rows.iter().map(|r| r[0].as_str()).collect();
        assert_eq!(ranks, ["1", "2", "3"], "the rank counts the kept rows; the source printed 4, 6 and 2");
        assert!(s.rows[0][2].contains("from name") && s.header[2].contains("from the model name"));
        let md = s.markdown();
        assert!(md.contains("| Rank | Model |") && md.contains("| 1 | xLAM-2-1b-fc-r |") && md.contains("secondary source") && md.contains("not the rank the source prints"), "{}", md);
    }

    #[test]
    fn a_size_column_is_read_with_its_unit_and_ties_share_a_rank() {
        let mut csv = from_csv("https://lb.example/data.csv", CSV).unwrap();
        csv.primary = true;
        let s = select(&csv, &spec()).unwrap();
        // 0.45 (B) and 1.5 (B) pass; 70 and the empty size do not.
        let models: Vec<&str> = s.rows.iter().map(|r| r[1].as_str()).collect();
        assert_eq!(models, ["Small One", "Tiny, Model"]);
        assert_eq!(s.unknown, 1);
        assert!(s.markdown().contains("primary source"));
        let tie = Table { source: "u".into(), primary: false, header: vec!["Model".into(), "Score".into()],
                          rows: vec![vec!["A-1B".into(), "50".into()], vec!["B-1B".into(), "50".into()], vec!["C-1B".into(), "40".into()]] };
        let by_score = parse_spec(&json!({"filters": [{"column": "parameters", "op": "<", "value": 2, "unit": "B"}], "rank_by": "score"}));
        let s = select(&tie, &by_score).unwrap();
        assert_eq!(s.rows.iter().map(|r| r[0].clone()).collect::<Vec<_>>(), ["1", "1", "3"]);
        assert!(select(&tie, &parse_spec(&json!({"filters": [{"column": "license", "op": "=", "value": 1}]}))).is_none());
        // A primary table beats a bigger secondary one.
        let mut big = tie.clone();
        big.rows.extend((0..5).map(|i| vec![format!("D{}-1B", i), format!("{}", i)]));
        assert_eq!(best(&[big, csv.clone()], &spec()).unwrap().source, "https://lb.example/data.csv");
    }
}
