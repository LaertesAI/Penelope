//! `/tools`: search for a tool and install one after a typed yes. Kept apart from `mod.rs`.

use super::App;
use crate::toolsearch;

impl App {
    pub(super) fn tools_command(&mut self, rest: &str) {
        let (verb, arg) = rest.split_once(char::is_whitespace).map(|(a, b)| (a, b.trim())).unwrap_or((rest, ""));
        match verb {
            "search" if !arg.is_empty() => {
                let query = arg.to_string();
                self.start_bg(move |ctx| {
                    let report = toolsearch::search(&ctx, &query, 10);
                    toolsearch::save_last(&ctx, &report.candidates);
                    let hint = if report.candidates.is_empty() { "" } else { "\n/tools install N lists what would run; nothing installs before /tools yes." };
                    Ok(format!("{}{}", toolsearch::render(&report), hint))
                });
            }
            "install" if !arg.is_empty() => {
                let mut parts = arg.split_whitespace();
                let n = parts.next().and_then(|n| n.parse::<usize>().ok()).unwrap_or(0);
                let report = toolsearch::request(&self.ctx, n, parts.next() == Some("repo"));
                self.pager("Tools", report);
            }
            "yes" => {
                let report = toolsearch::confirm(&self.ctx);
                self.refresh_side();
                self.pager("Tools", report);
            }
            "no" => self.toast(toolsearch::cancel()),
            _ => self.toast("/tools search QUERY, install N [repo], yes, no".to_string()),
        }
    }
}
