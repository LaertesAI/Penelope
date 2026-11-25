//! A score per part and task type, and suggestions for the profile.
//!
//! The table counts how often the validator accepted or rejected the result of a part (for example `coding-3`) on a
//! task type (for example `rust-fix`). `advise` prints one sentence per pair that has at least 5 results and a
//! rejection share of 60 % or more, worst share first. The table is stored as JSON.
//!
//! Not wired into a command yet: nothing outside the tests calls this module.

use serde_json::{json, Value};
use std::fs;
use std::path::Path;

pub const MIN_RESULTS: u64 = 5;
/// Rejection share in percent from which a part gets a suggestion.
pub const MIN_REJECTED_PERCENT: u64 = 60;

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

/// One sentence per (part, task type) with at least 5 results and a rejection share of 60 % or more, worst share
/// first. Equal shares are ordered by part and task type.
pub fn advise(table: &Value) -> Vec<String> {
    let mut found: Vec<(f64, String, String, u64, u64)> = Vec::new();
    for (part, types) in table.as_object().into_iter().flatten() {
        for (task_type, cell) in types.as_object().into_iter().flatten() {
            let accepted = cell["accepted"].as_u64().unwrap_or(0);
            let rejected = cell["rejected"].as_u64().unwrap_or(0);
            let total = accepted + rejected;
            if total >= MIN_RESULTS && rejected * 100 >= MIN_REJECTED_PERCENT * total {
                found.push((rejected as f64 / total as f64, part.clone(), task_type.clone(), rejected, total));
            }
        }
    }
    found.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal).then_with(|| a.1.cmp(&b.1)).then_with(|| a.2.cmp(&b.2)));
    found
        .into_iter()
        .map(|(_, part, task_type, rejected, total)| {
            format!("{} was rejected {} of {} times on {}: change its model.", part, rejected, total, task_type)
        })
        .collect()
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
}
