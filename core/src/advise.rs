//! A score per part and task type, and suggestions for the profile.
//!
//! The table counts how often the validator accepted or rejected the result of a part (for example `coding-3`) on a
//! task type (for example `rust-fix`). `advise` prints one sentence per pair that has at least 5 results and a
//! rejection share of 60 % or more, worst share first. The table is stored as JSON.
//!
//! A cluster validator writes every verdict with `record` into `.penelope/clusters/scores.json`; `penelope cluster
//! advise` reads the same table. `suggestions` returns the pairs with their numbers, so a caller can filter by part.

use serde_json::{json, Value};
use std::fs;
use std::path::Path;

pub const MIN_RESULTS: u64 = 5;
/// Rejection share in percent from which a part gets a suggestion.
pub const MIN_REJECTED_PERCENT: u64 = 60;
/// A model needs at least this many results and at least 60 % accepted to be suggested by the score table.
pub const MODEL_MIN_RESULTS: u64 = 3;
pub const MODEL_MIN_ACCEPTED_PERCENT: u64 = 60;

/// JSON object: `{"<part>": {"<task type>": {"accepted": n, "rejected": n}}}`.
pub fn new_table() -> Value {
    json!({})
}

/// Adds one result to the table.
pub fn record(table: &mut Value, part: &str, task_type: &str, accepted: bool) {
    if !table.is_object() {
        *table = json!({});
    }
    let key = if accepted { "accepted" } else { "rejected" };
    let cell = &mut table[part][task_type];
    if !cell.is_object() {
        *cell = json!({"accepted": 0, "rejected": 0});
    }
    let n = cell[key].as_u64().unwrap_or(0) + 1;
    cell[key] = json!(n);
}

/// Adds one result and the model setting that produced it. The model counts live under
/// `table[part][task_type]["models"][setting]`, so the old accepted/rejected numbers stay unchanged and a reader from
/// before this field keeps working.
pub fn record_model(table: &mut Value, part: &str, task_type: &str, model: &str, accepted: bool) {
    record(table, part, task_type, accepted);
    let model = model.trim();
    if model.is_empty() {
        return;
    }
    let cell = &mut table[part][task_type]["models"][model];
    if !cell.is_object() {
        *cell = json!({"accepted": 0, "rejected": 0});
    }
    let key = if accepted { "accepted" } else { "rejected" };
    let n = cell[key].as_u64().unwrap_or(0) + 1;
    cell[key] = json!(n);
}

/// Adds one Kerkis event for a part: a gate forbids a call, the loop guard kills a call, or a part reads a source
/// again. The counts live under `table[part]["events"][kind]` and do not change the validator scores.
pub fn record_event(table: &mut Value, part: &str, kind: &str) {
    let kind = kind.trim();
    if kind.is_empty() { return; }
    if !table.is_object() { *table = json!({}); }
    let cell = &mut table[part]["events"][kind];
    let n = cell.as_u64().unwrap_or(0) + 1;
    *cell = json!(n);
}

/// The event counts of one part, for a terminal or a test.
pub fn event_counts(table: &Value, part: &str) -> Value {
    table[part]["events"].clone()
}

/// A union of more than 8 tools costs context and choice. The suggestion names the least used tool when usage is
/// known, otherwise the first tool in sorted order.
pub fn tool_advice(tools: &[String], usage: &std::collections::BTreeMap<String, u64>) -> Option<String> {
    if tools.len() <= 8 { return None; }
    let mut sorted = tools.to_vec();
    sorted.sort();
    let least = sorted.iter().min_by_key(|t| usage.get(*t).copied().unwrap_or(0)).cloned().unwrap_or_default();
    Some(format!("the union has {} tools, more than 8; remove {} or split the cluster", tools.len(), least))
}

/// Reads the table from a file. A missing file is an empty table. A file that is not a JSON object is an error.
pub fn load(path: &Path) -> Result<Value, String> {
    if !path.exists() {
        return Ok(new_table());
    }
    let text = fs::read_to_string(path).map_err(|e| format!("cannot read {}: {}", path.display(), e))?;
    let v: Value = serde_json::from_str(&text).map_err(|e| format!("{} is not valid JSON: {}", path.display(), e))?;
    if !v.is_object() {
        return Err(format!("{} does not hold a JSON object", path.display()));
    }
    Ok(v)
}

/// Writes the table as JSON and creates the parent folder when needed.
pub fn save(path: &Path, table: &Value) -> Result<(), String> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent).map_err(|e| format!("cannot create {}: {}", parent.display(), e))?;
    }
    let text = serde_json::to_string_pretty(table).map_err(|e| e.to_string())?;
    fs::write(path, text).map_err(|e| format!("cannot write {}: {}", path.display(), e))
}

/// One (part, task type) pair that needs a change.
#[derive(Clone, Debug, PartialEq)]
pub struct Suggestion {
    pub part: String,
    pub task_type: String,
    pub rejected: u64,
    pub total: u64,
}

impl Suggestion {
    pub fn sentence(&self) -> String {
        format!("{} was rejected {} of {} times on {}: change its model.",
                self.part, self.rejected, self.total, self.task_type)
    }
}

/// One entry per (part, task type) with at least 5 results and a rejection share of 60 % or more, worst share first.
/// Equal shares are ordered by part and task type.
pub fn suggestions(table: &Value) -> Vec<Suggestion> {
    let mut found: Vec<(f64, Suggestion)> = Vec::new();
    for (part, types) in table.as_object().into_iter().flatten() {
        for (task_type, cell) in types.as_object().into_iter().flatten() {
            if task_type == "events" { continue; }
            let accepted = cell["accepted"].as_u64().unwrap_or(0);
            let rejected = cell["rejected"].as_u64().unwrap_or(0);
            let total = accepted + rejected;
            if total >= MIN_RESULTS && rejected * 100 >= MIN_REJECTED_PERCENT * total {
                found.push((rejected as f64 / total as f64, Suggestion {
                    part: part.clone(), task_type: task_type.clone(), rejected, total,
                }));
            }
        }
    }
    found.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal)
        .then_with(|| a.1.part.cmp(&b.1.part)).then_with(|| a.1.task_type.cmp(&b.1.task_type)));
    found.into_iter().map(|(_, suggestion)| suggestion).collect()
}

/// One sentence per suggestion, for a terminal or a test.
pub fn advise(table: &Value) -> Vec<String> {
    suggestions(table).iter().map(Suggestion::sentence).collect()
}

/// The acceptance of one model on one (part, task type) pair: `(accepted, total)`.
pub fn model_acceptance(table: &Value, part: &str, task_type: &str, model: &str) -> Option<(u64, u64)> {
    let cell = table[part][task_type]["models"].get(model)?;
    let accepted = cell["accepted"].as_u64().unwrap_or(0);
    let rejected = cell["rejected"].as_u64().unwrap_or(0);
    Some((accepted, accepted + rejected))
}

/// The model with the best acceptance on one pair. Only a model with at least `MODEL_MIN_RESULTS` results and at
/// least 60 % accepted can be suggested; the table never recommends a weak model just because it is the only one.
pub fn best_model(table: &Value, part: &str, task_type: &str) -> Option<String> {
    let models = table[part][task_type]["models"].as_object()?;
    let mut found: Vec<(f64, u64, String)> = Vec::new();
    for (name, cell) in models {
        let accepted = cell["accepted"].as_u64().unwrap_or(0);
        let rejected = cell["rejected"].as_u64().unwrap_or(0);
        let total = accepted + rejected;
        if total < MODEL_MIN_RESULTS || accepted * 100 < MODEL_MIN_ACCEPTED_PERCENT * total {
            continue;
        }
        found.push((accepted as f64 / total as f64, total, name.clone()));
    }
    found.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal)
        .then_with(|| b.1.cmp(&a.1)).then_with(|| a.2.cmp(&b.2)));
    found.into_iter().next().map(|(_, _, name)| name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fill(table: &mut Value, part: &str, task: &str, accepted: u32, rejected: u32) {
        for _ in 0..accepted {
            record(table, part, task, true);
        }
        for _ in 0..rejected {
            record(table, part, task, false);
        }
    }

    #[test]
    fn kerkis_events_are_counted_per_part_and_do_not_change_scores() {
        let mut t = new_table();
        record_event(&mut t, "coding-3", "gate_forbid");
        record_event(&mut t, "coding-3", "gate_forbid");
        record_event(&mut t, "coding-3", "loop_kill");
        assert_eq!(event_counts(&t, "coding-3")["gate_forbid"], 2);
        assert_eq!(event_counts(&t, "coding-3")["loop_kill"], 1);
        fill(&mut t, "coding-3", "rust-fix", 1, 4);
        assert_eq!(suggestions(&t).len(), 1, "the events key must not become a task type");
    }

    #[test]
    fn a_union_above_eight_tools_gets_one_removal() {
        let tools: Vec<String> = (1..=10).map(|i| format!("tool{}", i)).collect();
        assert!(tool_advice(&tools[..8], &std::collections::BTreeMap::new()).is_none());
        let mut usage = std::collections::BTreeMap::new();
        usage.insert("tool7".to_string(), 9);
        assert!(tool_advice(&tools, &usage).unwrap().contains("remove tool1"), "{}", tool_advice(&tools, &usage).unwrap());
    }

    #[test]
    fn record_counts_both_outcomes() {
        let mut t = new_table();
        fill(&mut t, "coding-3", "rust-fix", 2, 3);
        assert_eq!(t["coding-3"]["rust-fix"]["accepted"], 2);
        assert_eq!(t["coding-3"]["rust-fix"]["rejected"], 3);
    }

    #[test]
    fn four_of_five_gives_a_suggestion_naming_the_part() {
        let mut t = new_table();
        fill(&mut t, "coding-3", "rust-fix", 1, 4);
        assert_eq!(advise(&t), vec!["coding-3 was rejected 4 of 5 times on rust-fix: change its model."]);
    }

    #[test]
    fn one_of_five_gives_none() {
        let mut t = new_table();
        fill(&mut t, "coding-3", "rust-fix", 4, 1);
        assert!(advise(&t).is_empty());
    }

    #[test]
    fn three_of_four_is_too_few_results() {
        let mut t = new_table();
        fill(&mut t, "coding-3", "rust-fix", 1, 3);
        assert!(advise(&t).is_empty());
    }

    #[test]
    fn three_of_five_is_exactly_sixty_percent() {
        let mut t = new_table();
        fill(&mut t, "a", "x", 2, 3);
        assert_eq!(advise(&t).len(), 1);
        fill(&mut t, "b", "x", 3, 2);
        assert_eq!(advise(&t).len(), 1);
    }

    #[test]
    fn sorted_by_rejection_share_then_name() {
        let mut t = new_table();
        fill(&mut t, "a", "x", 2, 3); // 60 %
        fill(&mut t, "z", "x", 0, 5); // 100 %
        fill(&mut t, "b", "x", 1, 4); // 80 %
        fill(&mut t, "c", "x", 1, 4); // 80 %
        let out = advise(&t);
        let names: Vec<&str> = out.iter().map(|s| s.split(' ').next().unwrap()).collect();
        assert_eq!(names, vec!["z", "b", "c", "a"]);
    }

    #[test]
    fn task_types_of_one_part_are_scored_apart() {
        let mut t = new_table();
        fill(&mut t, "coding-3", "rust-fix", 0, 5);
        fill(&mut t, "coding-3", "docs", 5, 0);
        let out = advise(&t);
        assert_eq!(out.len(), 1);
        assert!(out[0].contains("rust-fix"));
    }

    #[test]
    fn save_and_load_round_trip() {
        let dir = std::env::temp_dir().join(format!("penelope-advise-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let path = dir.join("sub").join("scores.json");
        assert_eq!(load(&path).unwrap(), new_table());
        let mut t = new_table();
        fill(&mut t, "coding-3", "rust-fix", 1, 4);
        save(&path, &t).unwrap();
        let back = load(&path).unwrap();
        assert_eq!(back, t);
        fs::write(&path, "[1,2]").unwrap();
        assert!(load(&path).is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_model_of_a_result_is_counted_beside_the_old_totals() {
        let mut t = new_table();
        for _ in 0..4 { record_model(&mut t, "coding-3", "rust-fix", "weak", false); }
        record_model(&mut t, "coding-3", "rust-fix", "weak", true);
        for _ in 0..5 { record_model(&mut t, "coding-3", "rust-fix", "main", true); }
        assert_eq!((t["coding-3"]["rust-fix"]["accepted"].as_u64(), t["coding-3"]["rust-fix"]["rejected"].as_u64()),
                   (Some(6), Some(4)));
        assert_eq!(model_acceptance(&t, "coding-3", "rust-fix", "weak"), Some((1, 5)));
        assert_eq!(model_acceptance(&t, "coding-3", "rust-fix", "main"), Some((5, 5)));
        assert_eq!(best_model(&t, "coding-3", "rust-fix").as_deref(), Some("main"));
    }

    #[test]
    fn a_single_lucky_run_is_not_enough_to_recommend_a_model() {
        let mut t = new_table();
        for _ in 0..5 { record_model(&mut t, "coding-3", "rust-fix", "weak", false); }
        record_model(&mut t, "coding-3", "rust-fix", "main", true);
        assert_eq!(best_model(&t, "coding-3", "rust-fix"), None, "one accepted run is below the model threshold");
        for _ in 0..2 { record_model(&mut t, "coding-3", "rust-fix", "main", true); }
        assert_eq!(best_model(&t, "coding-3", "rust-fix").as_deref(), Some("main"));
    }

    #[test]
    fn a_model_below_sixty_percent_is_never_suggested() {
        let mut t = new_table();
        for _ in 0..3 { record_model(&mut t, "p", "t", "weak", false); }
        for _ in 0..2 { record_model(&mut t, "p", "t", "weak", true); }
        assert_eq!(best_model(&t, "p", "t"), None);
    }
}
