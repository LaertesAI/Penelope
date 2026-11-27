//! Clarify before creating: the fields a cluster, a workflow or a scheduled task needs, the rules that tell which are
//! missing, the questions that fill them and the final yes.
//!
//! Everything a front end needs sits here: `Field` lists the fields and their questions, `Spec::from_request` reads
//! what the request already says, `missing` names what is left, `clarify_with` asks until nothing is missing and then
//! asks once for the yes. The asker is a function, so Penelope Desktop can bring its own dialog. `clarify` uses the
//! question dialog of `ask.rs` and asks nobody when nobody can answer (a timer, a script, an eval run).
//!
//! The filled spec goes into the created object: a factory profile carries it under `spec`, a cluster gets
//! `clusters/ID/spec.json`, and the goal text the workers read lists the agreed limits. A workflow calls `clarify` the
//! same way as soon as `/workflow` exists.

use crate::ask::{Answer, Question, Reply};
use crate::config::Ctx;
use regex::Regex;
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Cluster,
    Factory,
    Workflow,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Cluster => "cluster",
            Kind::Factory => "scheduled task",
            Kind::Workflow => "workflow",
        }
    }
}

/// One field of a spec. `opts` are the answers offered, a label and what it means.
pub struct Field {
    pub key: &'static str,
    pub header: &'static str,
    pub question: &'static str,
    pub opts: &'static [(&'static str, &'static str)],
    pub multi: bool,
    pub kinds: &'static [Kind],
}

use Kind::{Cluster as C, Factory as F, Workflow as W};

/// The fields in the order they are asked.
pub const FIELDS: &[Field] = &[
    Field { key: "goal", header: "Goal", question: "What should it do? Describe the goal in one sentence.",
            opts: &[("Research and report", "read sources and write a report"), ("Change code in this project", "edit files and run the tests"), ("Watch and notify", "check something and tell me when it changes")],
            multi: false, kinds: &[C, F, W] },
    Field { key: "done", header: "Done when", question: "How does it know it is done?",
            opts: &[("Tests pass", "the test command exits 0"), ("A report exists", "a written result is stored"), ("The validator accepts it", "the validator part checks the result"), ("I check it", "a person looks at the result")],
            multi: false, kinds: &[C, F, W] },
    Field { key: "abort", header: "Abort rule", question: "When should it give up?",
            opts: &[("Stuck for 3 reports", "stop after 3 reports without progress"), ("First failed check", "stop at the first failing check"), ("Budget is spent", "stop when the budget is used"), ("Never on its own", "only I stop it")],
            multi: false, kinds: &[C, F, W] },
    Field { key: "budget", header: "Budget", question: "How much may it spend? Type tokens or euro for another value.",
            opts: &[("1 EUR", "small job"), ("5 EUR", "normal job"), ("20 EUR", "large job"), ("unlimited", "only count the cost")],
            multi: false, kinds: &[C, F, W] },
    Field { key: "max_iterations", header: "Max rounds", question: "How many iterations or runs at most?",
            opts: &[("3 iterations", "a short loop"), ("10 iterations", "a normal loop"), ("30 iterations", "a long loop"), ("no limit", "until done or aborted")],
            multi: false, kinds: &[C, F, W] },
    Field { key: "schedule", header: "Schedule", question: "When does it run?",
            opts: &[("every 1h", "once an hour"), ("every 6h", "four times a day"), ("every 24h", "once a day"), ("at 08:00", "every day at 08:00")],
            multi: false, kinds: &[F] },
    Field { key: "rights", header: "Rights", question: "What may it do? Choose all that apply.",
            opts: &[("read-only", "read files and sources, change nothing"), ("write files", "edit files in the project"), ("network", "fetch pages and call APIs"), ("install packages", "run package installs")],
            multi: true, kinds: &[C, F, W] },
    Field { key: "models", header: "Models", question: "Which models or roles should work on it?",
            opts: &[("default profile", "the models of the profile"), ("cheap models only", "the weak role everywhere"), ("main model only", "one strong model"), ("strong planner, cheap workers", "mixed")],
            multi: false, kinds: &[C, F, W] },
    Field { key: "results_to", header: "Results", question: "Where do the results go?",
            opts: &[("show in the chat", "the transcript"), ("a file in the project", "a report file"), ("a pull request", "a branch with a PR"), ("only keep in .penelope", "stored, not shown")],
            multi: false, kinds: &[C, F, W] },
    Field { key: "approve_each_run", header: "Approval", question: "Must a person approve each run before it starts?",
            opts: &[("yes", "ask me before every run"), ("no", "run on its own")],
            multi: false, kinds: &[F, W] },
];

pub fn field(key: &str) -> Option<&'static Field> {
    FIELDS.iter().find(|f| f.key == key)
}

/// The filled values of one spec and the fields that the request stated twice with different values.
#[derive(Clone, Debug, PartialEq)]
pub struct Spec {
    pub kind: Kind,
    pub values: BTreeMap<String, String>,
    pub conflicts: BTreeMap<String, Vec<String>>,
    /// True after the person said yes to the whole spec.
    pub confirmed: bool,
}

fn rx(pattern: &str) -> Regex {
    Regex::new(pattern).expect("a fixed pattern")
}

fn clean(text: &str) -> String {
    text.trim().trim_matches(|c: char| c == '.' || c == ',' || c == ';' || c == '"').trim().to_string()
}

/// Values the request states by name: `Done when: tests pass` on its own line.
fn labelled(text: &str) -> Vec<(&'static str, String)> {
    let names: &[(&str, &[&str])] = &[
        ("goal", &["goal"]), ("done", &["done when", "done", "done criterion"]), ("abort", &["abort rule", "abort", "abort when"]),
        ("budget", &["budget"]), ("max_iterations", &["max iterations", "max runs", "iterations", "runs", "max rounds"]),
        ("schedule", &["schedule"]), ("rights", &["rights", "tools"]), ("models", &["models", "model", "roles"]),
        ("results_to", &["results", "results to", "output"]), ("approve_each_run", &["approval", "approve each run"]),
    ];
    let mut out = Vec::new();
    for line in text.lines() {
        let Some((head, value)) = line.split_once(':') else { continue };
        let head = head.trim().to_lowercase();
        let value = clean(value);
        if value.is_empty() { continue; }
        if let Some((key, _)) = names.iter().find(|(_, labels)| labels.contains(&head.as_str())) {
            out.push((*key, value));
        }
    }
    out
}

fn first(text: &str, pattern: &str) -> Option<String> {
    rx(pattern).captures(text).and_then(|c| c.get(1).or_else(|| c.get(0))).map(|m| clean(m.as_str())).filter(|s| !s.is_empty())
}

fn yes_no(text: &str) -> Option<&'static str> {
    if rx(r"(?i)(ask|confirm with|check with) me before (each|every) run|approve (each|every) run|(each|every) run needs (my )?approval|human approval").is_match(text) {
        Some("yes")
    } else if rx(r"(?i)\bunattended\b|without asking|fully automatic|no approval needed|run on its own").is_match(text) {
        Some("no")
    } else {
        None
    }
}

/// Normal form of a yes or no answer: "yes" or "no".
fn yn(text: &str) -> String {
    let t = text.trim().to_lowercase();
    if t.starts_with("yes") || t == "y" || t.starts_with("ask me") { "yes".to_string() } else { "no".to_string() }
}

impl Spec {
    pub fn new(kind: Kind) -> Spec {
        Spec { kind, values: BTreeMap::new(), conflicts: BTreeMap::new(), confirmed: false }
    }

    /// The first line of the goal, for a title.
    pub fn title(&self) -> String {
        self.get("goal").and_then(|g| g.lines().next()).map(|l| l.chars().take(80).collect()).unwrap_or_default()
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.values.get(key).map(String::as_str)
    }

    fn set(&mut self, key: &str, value: String) {
        if !value.is_empty() {
            self.values.insert(key.to_string(), value);
            self.conflicts.remove(key);
        }
    }

    /// Reads what the request already says. `preset` holds values the caller knows: tool arguments or command flags,
    /// by field key. A preset wins over a labelled line, a labelled line over a phrase in the text.
    pub fn from_request(kind: Kind, request: &str, preset: &Map<String, Value>) -> Spec {
        let mut spec = Spec::new(kind);
        let text = request.trim();
        spec.set("goal", text.to_string());   // the whole request is the goal; its first line is only the title
        // phrases
        if let Some(v) = first(text, r"(?i)\b(?:done|finished|complete|successful|success)\s+(?:when|if|once)\s+([^.;\n]+)").or_else(|| first(text, r"(?i)\buntil\s+([^.;\n]+)")) {
            spec.set("done", v);
        }
        if let Some(v) = first(text, r"(?i)\b(?:stop|abort|give up|halt)\s+(?:if|when|after|on)\s+[^.;\n]+") {
            spec.set("abort", v);
        }
        if let Some(m) = rx(r"(?i)(\d+(?:[.,]\d+)?)\s*(?:eur|euros?|€)").captures(text) {
            spec.set("budget", format!("{} EUR", m[1].replace(',', ".")));
        } else if let Some(m) = rx(r"(?i)\b(\d+(?:[.,]\d+)?)\s*([km])?\s*tokens").captures(text) {
            spec.set("budget", format!("{}{} tokens", m[1].replace(',', "."), m.get(2).map(|x| x.as_str().to_lowercase()).unwrap_or_default()));
        } else if rx(r"(?i)unlimited budget|no budget limit").is_match(text) {
            spec.set("budget", "unlimited".to_string());
        }
        if let Some(m) = rx(r"(?i)\b(\d+)\s*(iterations?|rounds?|runs?|attempts?|retries)\b").captures(text) {
            spec.set("max_iterations", format!("{} {}", &m[1], m[2].to_lowercase()));
        }
        let (every, at) = (first(text, r"(?i)\bevery\s+(\d+\s*(?:seconds?|sec|s|minutes?|min|m|hours?|h|days?|d))\b"), first(text, r"(?i)\bat\s+(\d{1,2}:\d{2})\b"));
        let every = every.map(|e| {
            // 3 hours, 30 min, 45 seconds: the first letter of the unit is what the timer takes
            let digits: String = e.chars().filter(|c| c.is_ascii_digit()).collect();
            let unit = e.chars().find(|c| c.is_ascii_alphabetic()).unwrap_or('h').to_ascii_lowercase();
            let short = format!("{}{}", digits, unit);
            format!("every {}", crate::factories::normalize_every(&short).unwrap_or(short))
        });
        let at = at.map(|a| format!("at {}", crate::factories::normalize_at(&a).unwrap_or(a)));
        match (every, at) {
            (Some(e), Some(a)) => { spec.conflicts.insert("schedule".to_string(), vec![e, a]); }
            (Some(v), None) | (None, Some(v)) => spec.set("schedule", v),
            (None, None) => {
                if rx(r"(?i)\bhourly\b").is_match(text) { spec.set("schedule", "every 1h".to_string()); }
                else if rx(r"(?i)\bdaily\b").is_match(text) { spec.set("schedule", "every 24h".to_string()); }
            }
        }
        let mut rights: Vec<&str> = Vec::new();
        let read_only = rx(r"(?i)read[- ]only").is_match(text);
        if read_only { rights.push("read-only"); }
        if rx(r"(?i)(?:may|can|allowed to|with)\s+(?:write|edit|modify)\b|write access").is_match(text) { rights.push("write files"); }
        if rx(r"(?i)(?:with|has|may use|allowed)\s+(?:the\s+)?(?:network|internet)|network access").is_match(text) { rights.push("network"); }
        if rx(r"(?i)(?:may|can|allowed to)\s+install|install(?:ing)?\s+(?:packages|dependencies)").is_match(text) { rights.push("install packages"); }
        if rights.len() > 1 && read_only {
            spec.conflicts.insert("rights".to_string(), rights.iter().map(|r| r.to_string()).collect());
        } else if !rights.is_empty() {
            spec.set("rights", rights.join(", "));
        }
        if let Some(v) = first(text, r"(?i)\b(?:with|using)\s+(?:the\s+)?([\w.-]+)\s+(?:model|profile)s?\b").or_else(|| first(text, r"(?i)\bmodels?\s*[:=]\s*([^.;\n]+)")) {
            spec.set("models", v);
        }
        if let Some(v) = first(text, r"(?i)\b(?:write|save|put|send|post|report)\s+(?:the\s+|a\s+|its\s+)?(?:results?|report|summary|output|answer)s?\s+(?:to|in|into)\s+([^.;\n]+)")
            .or_else(|| first(text, r"(?i)\bresults?\s+(?:go|goes)\s+to\s+([^.;\n]+)")) {
            spec.set("results_to", v);
        }
        if let Some(v) = yes_no(text) { spec.set("approve_each_run", v.to_string()); }
        // labelled lines, then the preset
        for (key, value) in labelled(text) {
            if key != "goal" || spec.get("goal").is_none() { spec.set(key, value); }
        }
        for (key, value) in preset {
            let text = match value { Value::String(s) => s.trim().to_string(), Value::Null => String::new(), Value::Bool(b) => (if *b { "yes" } else { "no" }).to_string(), other => other.to_string() };
            if field(key).is_some() { spec.set(key, text); }
        }
        spec
    }

    /// True if the kind of this spec uses the field.
    fn needs(&self, f: &Field) -> bool {
        f.kinds.contains(&self.kind)
    }

    /// The fields that are empty or stated with two different values, in the order they are asked.
    pub fn missing(&self) -> Vec<&'static Field> {
        FIELDS.iter().filter(|f| self.needs(f) && (self.get(f.key).is_none() || self.conflicts.contains_key(f.key))).collect()
    }

    /// Every field of the kind, rights and budget first, each cut at 200 characters so none hides another.
    pub fn summary(&self) -> String {
        let mut order: Vec<&Field> = FIELDS.iter().filter(|f| self.needs(f)).collect();
        order.sort_by_key(|f| match f.key { "rights" => 0, "budget" => 1, "goal" => 9, _ => 2 });
        order.iter().map(|f| {
            let v = self.get(f.key).unwrap_or("not set").replace('\n', " ");
            format!("{}: {}", f.header, if v.chars().count() > 200 { format!("{}...", v.chars().take(200).collect::<String>()) } else { v })
        }).collect::<Vec<_>>().join("\n")
    }

    /// The euro budget as a number when the budget says euro.
    pub fn budget_eur(&self) -> Option<f64> {
        let b = self.get("budget")?;
        let m = rx(r"(?i)^(\d+(?:\.\d+)?)\s*(?:eur|euros?|€)$").captures(b.trim())?;
        m[1].parse().ok()
    }

    /// The token budget as a number when the budget says tokens: `200k tokens`, `1m tokens`, `5000 tokens`.
    #[allow(dead_code)]          // for front ends that meter tokens; the tests call it
    pub fn budget_tokens(&self) -> Option<u64> {
        let m = rx(r"(?i)^(\d+(?:\.\d+)?)\s*([km])?\s*tokens$").captures(self.get("budget")?.trim())?;
        let n: f64 = m[1].parse().ok()?;
        let unit = match m.get(2).map(|x| x.as_str().to_lowercase()).as_deref() { Some("k") => 1e3, Some("m") => 1e6, _ => 1.0 };
        Some((n * unit) as u64)
    }

    /// The goal followed by the agreed limits, for the workers. Without a yes it is the goal alone.
    pub fn goal_text(&self) -> String {
        let goal = self.get("goal").unwrap_or("").to_string();
        if !self.confirmed {
            return goal;
        }
        let lines: Vec<String> = FIELDS.iter().filter(|f| f.key != "goal" && self.needs(f)).filter_map(|f| self.get(f.key).map(|v| format!("- {}: {}", f.header, v))).collect();
        if lines.is_empty() { goal } else { format!("{}\n\nAgreed with the user before the start:\n{}", goal, lines.join("\n")) }
    }

    pub fn to_value(&self) -> Value {
        let mut v = json!({"kind": self.kind.name(), "confirmed": self.confirmed, "created": crate::ask::now_secs()});
        for (k, val) in &self.values { v[k] = json!(val); }
        v
    }
}

/// The question for one field. A field that was stated twice offers the two values.
pub fn question_for(spec: &Spec, f: &Field) -> Question {
    let candidates = spec.conflicts.get(f.key);
    let (text, opts): (String, Vec<crate::ask::Opt>) = match candidates {
        Some(c) => (format!("{} The request names more than one value.", f.question),
                    c.iter().map(|v| crate::ask::Opt { label: v.clone(), description: "named in the request".to_string() }).collect()),
        None => (f.question.to_string(), f.opts.iter().map(|(l, d)| crate::ask::Opt { label: l.to_string(), description: d.to_string() }).collect()),
    };
    let goal = spec.get("goal").map(|g| format!("{}\n\n", g.chars().take(200).collect::<String>())).unwrap_or_default();
    Question { question: format!("{}{}", if f.key == "goal" { String::new() } else { goal }, text), header: f.header.to_string(), options: opts, multi_select: f.multi, allow_text: true }
}

/// Writes the answer to a field into the spec. A closed answer leaves it empty.
pub fn apply_answer(spec: &mut Spec, f: &Field, a: &Answer) {
    if a.closed {
        return;
    }
    let mut parts: Vec<String> = a.labels.clone();
    if let Some(t) = a.text.as_ref().filter(|t| !t.trim().is_empty()) {
        if !f.multi { parts.clear(); }
        parts.push(t.trim().to_string());
    }
    if f.multi && parts.len() > 1 { parts.retain(|p| p != "read-only"); }
    let value = if f.key == "approve_each_run" { yn(&parts.join(" ")) } else { parts.join(", ") };
    spec.set(f.key, value);
}

/// The result of clarifying: the spec and how many questions the person answered.
#[derive(Debug)]
pub struct Clarified {
    pub spec: Spec,
    pub asked: usize,
}

/// Asks until no field is missing, then once for the yes. `ask` shows a dialog and returns its reply. `interactive`
/// false asks nobody: the spec stays as the request made it and `confirmed` stays false.
pub fn clarify_with(kind: Kind, request: &str, preset: &Map<String, Value>, interactive: bool, ask: &dyn Fn(&[Question]) -> Reply,
                    confirm: &dyn Fn(&[Question]) -> Reply) -> Result<Clarified, String> {
    let mut spec = Spec::from_request(kind, request, preset);
    if !interactive {
        return Ok(Clarified { spec, asked: 0 });
    }
    let mut asked = 0;
    let mut tried: Vec<&str> = Vec::new();
    loop {
        let todo: Vec<&'static Field> = spec.missing().into_iter().filter(|f| !tried.contains(&f.key)).take(crate::ask::MAX_QUESTIONS).collect();
        if todo.is_empty() { break; }
        let qs: Vec<Question> = todo.iter().map(|f| question_for(&spec, f)).collect();
        tried.extend(todo.iter().map(|f| f.key));
        match ask(&qs) {
            Reply::Answers(answers) => {
                for (f, a) in todo.iter().zip(&answers) {
                    apply_answer(&mut spec, f, a);
                    asked += usize::from(!a.closed);
                }
            }
            other => return Err(format!("NOT created. {}", crate::ask::reply_text(&qs, &other).split(". ").next().unwrap_or("no answer"))),
        }
    }
    if let Some(f) = spec.missing().first() {
        return Err(format!("NOT created. The field {} has no answer.", f.header));
    }
    let ask_yes = Question {
        question: format!("Create this {}: {}?\n{}", kind.name(), spec.title(), spec.summary()),
        header: "Create?".to_string(),
        // the safe answer comes first, so the cursor opens on it
        options: vec![crate::ask::Opt { label: "No, do not create".to_string(), description: "nothing is created".to_string() },
                      crate::ask::Opt { label: "Yes, create it".to_string(), description: "write the spec into it and start".to_string() }],
        multi_select: false,
        allow_text: false,
    };
    match confirm(std::slice::from_ref(&ask_yes)) {
        Reply::Answers(a) if a.first().map(|a| a.labels.first().map(|l| l.starts_with("Yes")).unwrap_or(false)).unwrap_or(false) => {
            spec.confirmed = true;
            Ok(Clarified { spec, asked })
        }
        Reply::Answers(_) => Err(format!("NOT created. You did not confirm the {}. Nothing was written.", kind.name())),
        other => Err(format!("NOT created. {}", crate::ask::reply_text(std::slice::from_ref(&ask_yes), &other).split(". ").next().unwrap_or("no answer"))),
    }
}

/// True when this process may put a dialog in front of a person: a dialog folder exists and the caller is no scout and
/// no worker of a cluster.
pub fn can_ask() -> bool {
    crate::ask::available() && crate::ask::main_turn_here()
}

/// `clarify_with` and the question dialog of the interface.
pub fn clarify(ctx: &Ctx, kind: Kind, request: &str, preset: &Map<String, Value>) -> Result<Clarified, String> {
    let timeout = crate::ask::timeout_secs(ctx);
    clarify_with(kind, request, preset, can_ask(), &|qs| crate::ask::ask_questions(qs, timeout), &|qs| crate::ask::ask_confirm(qs, timeout))
}

/// The hook of the coming `/workflow`: the same questions as for a cluster, plus the approval of each run. The caller
/// stores `clarified.spec.to_value()` in the workflow it creates and starts nothing when this returns Err.
#[allow(dead_code)]          // the hook of the coming /workflow
pub fn clarify_workflow(ctx: &Ctx, request: &str, preset: &Map<String, Value>) -> Result<Clarified, String> {
    clarify(ctx, Kind::Workflow, request, preset)
}

/// Creates a cluster after the questions: the orchestrator gets the goal with the agreed limits, and the new top-level
/// clusters keep the spec as `clusters/ID/spec.json`. `extra` holds field values the caller knows.
pub fn start_cluster(ctx: &Ctx, goal: &str, profile: Option<&str>, parallel: bool, extra: &Map<String, Value>) -> Result<String, String> {
    if goal.trim().is_empty() {
        return Err("NOT started. The cluster goal is empty.".to_string());
    }
    let mut preset = extra.clone();
    if let Some(p) = profile.map(str::trim).filter(|p| !p.is_empty()) {
        preset.entry("models".to_string()).or_insert(json!(format!("{} profile", p)));
    }
    let done = clarify(ctx, Kind::Cluster, goal, &preset)?;
    let spec = done.spec.to_value();
    let mut out = if done.spec.confirmed {
        crate::clusters::start_with_spec(ctx, &done.spec.goal_text(), profile, parallel, &spec)
    } else {
        crate::clusters::start(ctx, &done.spec.goal_text(), profile, parallel)
    };
    if out.starts_with("NOT started") {
        return Err(out);
    }
    if done.spec.confirmed {
        out.push_str(&format!("\nspec confirmed ({} answers) and applied before the first model call.", done.asked));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ask::Opt;
    use std::cell::RefCell;

    fn no_preset() -> Map<String, Value> {
        Map::new()
    }

    /// An asker that picks the first option of every question and records what it was asked.
    fn first_option(log: &RefCell<Vec<Vec<String>>>) -> impl Fn(&[Question]) -> Reply + '_ {
        move |qs| {
            log.borrow_mut().push(qs.iter().map(|q| q.header.clone()).collect());
            Reply::Answers(qs.iter().map(|q| Answer::chosen(&q.options[if q.header == "Create?" { 1 } else { 0 }].label)).collect())
        }
    }

    const COMPLETE: &str = "Fix the parser bug in src/parse.rs. Done when all tests pass. Stop if the tests fail twice. Budget 3 EUR, at most 5 rounds. \
                            Read-only access. Results go to the chat. Using the solo profile.";

    #[test]
    fn a_complete_request_asks_only_for_the_final_yes() {
        let spec = Spec::from_request(Kind::Cluster, COMPLETE, &no_preset());
        assert!(spec.missing().is_empty(), "{:?}", spec.missing().iter().map(|f| f.key).collect::<Vec<_>>());
        let log = RefCell::new(Vec::new());
        let done = clarify_with(Kind::Cluster, COMPLETE, &no_preset(), true, &first_option(&log), &first_option(&log)).unwrap();
        assert_eq!(*log.borrow(), vec![vec!["Create?".to_string()]]);
        assert!(done.spec.confirmed && done.asked == 0);
    }

    #[test]
    fn a_sparse_request_asks_for_every_missing_field_in_rounds_of_six() {
        let log = RefCell::new(Vec::new());
        let done = clarify_with(Kind::Cluster, "Refactor the code", &no_preset(), true, &first_option(&log), &first_option(&log)).unwrap();
        let rounds = log.borrow().clone();
        assert_eq!(rounds[0], vec!["Done when", "Abort rule", "Budget", "Max rounds", "Rights", "Models"]);
        assert_eq!(rounds[1], vec!["Results"]);
        assert_eq!(rounds[2], vec!["Create?"]);
        assert_eq!(done.asked, 7);
        assert_eq!(done.spec.get("budget"), Some("1 EUR"));
        assert_eq!(done.spec.budget_eur(), Some(1.0));
    }

    #[test]
    fn only_missing_fields_are_asked() {
        let log = RefCell::new(Vec::new());
        let text = "Refactor the code. Done when the tests pass. Budget 2 EUR.";
        clarify_with(Kind::Cluster, text, &no_preset(), true, &first_option(&log), &first_option(&log)).unwrap();
        let first = &log.borrow()[0];
        assert!(!first.contains(&"Done when".to_string()) && !first.contains(&"Budget".to_string()), "{:?}", first);
        assert!(first.contains(&"Abort rule".to_string()));
        let mut preset = Map::new();
        preset.insert("abort".to_string(), json!("stop after 3 reports"));
        let spec = Spec::from_request(Kind::Cluster, text, &preset);
        assert_eq!(spec.get("abort"), Some("stop after 3 reports"));
        assert!(!spec.missing().iter().any(|f| f.key == "abort"));
    }

    #[test]
    fn a_factory_also_needs_a_schedule_and_the_approval_and_a_cluster_does_not() {
        let factory = Spec::from_request(Kind::Factory, "Check the news", &no_preset());
        let keys: Vec<&str> = factory.missing().iter().map(|f| f.key).collect();
        assert!(keys.contains(&"schedule") && keys.contains(&"approve_each_run"), "{:?}", keys);
        let cluster = Spec::from_request(Kind::Cluster, "Check the news", &no_preset());
        assert!(!cluster.missing().iter().any(|f| f.key == "schedule" || f.key == "approve_each_run"));
        let spec = Spec::from_request(Kind::Factory, "Check the news every 3h. Ask me before each run.", &no_preset());
        assert_eq!(spec.get("schedule"), Some("every 3h"));
        assert_eq!(spec.get("approve_each_run"), Some("yes"));
        assert_eq!(Spec::from_request(Kind::Factory, "Check it daily", &no_preset()).get("schedule"), Some("every 24h"));
    }

    #[test]
    fn two_different_values_for_one_field_are_asked_with_both_as_options() {
        let spec = Spec::from_request(Kind::Factory, "Check mail every 3h and at 08:00", &no_preset());
        assert_eq!(spec.conflicts["schedule"], vec!["every 3h".to_string(), "at 08:00".to_string()]);
        let f = field("schedule").unwrap();
        assert!(spec.missing().iter().any(|m| m.key == "schedule"));
        let q = question_for(&spec, f);
        assert_eq!(q.options.iter().map(|o| o.label.as_str()).collect::<Vec<_>>(), vec!["every 3h", "at 08:00"]);
        let rights = Spec::from_request(Kind::Cluster, "Read-only. It may write to the project and can install packages.", &no_preset());
        assert!(rights.conflicts.contains_key("rights"), "{:?}", rights);
    }

    #[test]
    fn the_final_yes_is_required() {
        let say_no = |qs: &[Question]| Reply::Answers(qs.iter().map(|q| Answer::chosen(q.options[0].label.as_str())).collect());
        let e = clarify_with(Kind::Cluster, COMPLETE, &no_preset(), true, &say_no, &say_no).unwrap_err();
        assert!(e.starts_with("NOT created") && e.contains("did not confirm"), "{}", e);
        let closed = |_: &[Question]| Reply::Closed;
        assert!(clarify_with(Kind::Cluster, COMPLETE, &no_preset(), true, &closed, &closed).unwrap_err().contains("closed the dialog"));
        let timed_out = |_: &[Question]| Reply::TimedOut(900);
        assert!(clarify_with(Kind::Cluster, "Refactor", &no_preset(), true, &timed_out, &timed_out).unwrap_err().contains("did not answer within 900"));
    }

    #[test]
    fn nobody_is_asked_when_nobody_can_answer() {
        let boom = |_: &[Question]| -> Reply { panic!("must not ask") };
        let done = clarify_with(Kind::Cluster, "Refactor the code", &no_preset(), false, &boom, &boom).unwrap();
        assert!(!done.spec.confirmed);
        assert_eq!(done.spec.goal_text(), "Refactor the code", "an unconfirmed spec leaves the goal alone");
    }

    #[test]
    fn typed_answers_and_several_choices_fill_the_fields() {
        let ask = |qs: &[Question]| Reply::Answers(qs.iter().map(|q| match q.header.as_str() {
            "Budget" => Answer::typed("200k tokens"),
            "Rights" => Answer { labels: vec!["read-only".into(), "network".into()], text: None, closed: false },
            "Create?" => Answer::chosen("Yes, create it"),
            _ => Answer::chosen(&q.options[0].label),
        }).collect());
        let done = clarify_with(Kind::Cluster, "Refactor the code", &no_preset(), true, &ask, &ask).unwrap();
        assert_eq!(done.spec.get("budget"), Some("200k tokens"));
        assert_eq!(done.spec.budget_tokens(), Some(200_000));
        assert_eq!(done.spec.get("rights"), Some("network"), "read-only drops out when something else is chosen");
        let text = done.spec.goal_text();
        assert!(text.starts_with("Refactor the code\n\nAgreed with the user") && text.contains("- Budget: 200k tokens"), "{}", text);
        assert_eq!(done.spec.to_value()["confirmed"], true);
        let o = Opt { label: "x".into(), description: String::new() };
        assert_eq!(o.label, "x");
    }

    #[test]
    fn the_whole_request_is_the_goal_and_the_first_line_is_the_title() {
        let text = "Port the parser\nto the new grammar\nand keep the old tests.";
        let spec = Spec::from_request(Kind::Cluster, text, &no_preset());
        assert_eq!(spec.get("goal"), Some(text));
        assert_eq!(spec.title(), "Port the parser");
        assert_eq!(spec.goal_text(), text, "without a yes the goal is still the whole text");
        let done = clarify_with(Kind::Cluster, &format!("{} Done when tests pass. Stop if stuck. 2 EUR, 3 rounds. Read-only. Using solo profile. Results go to the chat.", text),
                                &no_preset(), true, &first_option(&RefCell::new(Vec::new())), &first_option(&RefCell::new(Vec::new()))).unwrap();
        assert!(done.spec.goal_text().starts_with("Port the parser\nto the new grammar\nand keep the old tests."), "{}", done.spec.goal_text());
    }

    #[test]
    fn the_summary_shows_every_field_with_rights_and_budget_first_even_after_a_long_text() {
        let long = format!("Done when {}", "the tests pass and the report is written ".repeat(40));
        assert!(long.len() > 1600);
        let request = format!("Refactor it. {}. Budget 3 EUR, at most 5 rounds. Read-only access. Stop if stuck. Using solo profile. Results go to the chat.", long.trim());
        let spec = Spec::from_request(Kind::Workflow, &request, &no_preset());
        let summary = spec.summary();
        assert!(summary.starts_with("Rights: read-only\nBudget: 3 EUR"), "{}", summary);
        for h in ["Max rounds", "Approval", "Done when", "Abort rule", "Models", "Results", "Goal"] {
            assert!(summary.contains(&format!("{}:", h)), "{} missing in {}", h, summary);
        }
        assert!(summary.lines().all(|l| l.chars().count() <= 215), "a field is cut on its own");
        // the question the dialog gets parses and starts with the safe answer
        let seen = RefCell::new(Vec::new());
        let confirm_q = |qs: &[Question]| { seen.borrow_mut().push(qs[0].clone()); Reply::Closed };
        let _ = clarify_with(Kind::Workflow, &format!("{}\nApproval: no", request), &no_preset(), true, &first_option(&RefCell::new(Vec::new())), &confirm_q);
        let q = seen.borrow().last().unwrap().clone();
        assert_eq!(q.header, "Create?");
        assert_eq!(q.options[0].label, "No, do not create");
        assert!(q.question.contains("Rights: read-only") && q.question.contains("Budget: 3 EUR"), "{}", q.question);
        assert!(crate::ask::parse_questions(&crate::ask::questions_to_value(&[q])).is_ok());
    }

    #[test]
    fn the_questions_pass_the_schema_of_ask_user() {
        let spec = Spec::new(Kind::Factory);
        let qs: Vec<Question> = FIELDS.iter().map(|f| question_for(&spec, f)).collect();
        assert!(qs.iter().all(|q| q.options.len() >= 2 && q.options.len() <= 6 && !q.header.is_empty() && q.header.chars().count() <= 16));
        let as_json = crate::ask::questions_to_value(&qs[..6]);
        assert_eq!(crate::ask::parse_questions(&as_json).unwrap().len(), 6);
        assert!(FIELDS.iter().map(|f| f.key).collect::<std::collections::HashSet<_>>().len() == FIELDS.len());
    }

    #[test]
    fn labelled_lines_and_presets_beat_phrases() {
        let text = "Build the report\nDone when: the file exists\nBudget: 4 EUR\nMax runs: 2 runs";
        let spec = Spec::from_request(Kind::Workflow, text, &no_preset());
        assert_eq!(spec.title(), "Build the report");
        assert_eq!(spec.get("done"), Some("the file exists"));
        assert_eq!(spec.get("budget"), Some("4 EUR"));
        assert_eq!(spec.get("max_iterations"), Some("2 runs"));
        let mut preset = Map::new();
        preset.insert("budget".to_string(), json!("9 EUR"));
        preset.insert("approve_each_run".to_string(), json!(false));
        let spec = Spec::from_request(Kind::Workflow, text, &preset);
        assert_eq!((spec.get("budget"), spec.get("approve_each_run")), (Some("9 EUR"), Some("no")));
    }
}
