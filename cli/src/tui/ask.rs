//! The question dialog of `ask_user`: a tab strip of headers, one question at a time, a summary page before sending.
//! The keys live in `crate::ask::Dialog`; this file only draws.

use super::art::{CLAY, ASH, DIM, GOLD, INK, PAPER, RAISED, RED, SEA};
use super::text::{clip, wrap_plain, width};
use super::view::{dialog, fill, on, put};
use crate::ask::Dialog;
use ratatui::layout::Rect;
use ratatui::Frame;

/// The tab strip: every header, the open one marked, answered ones with a tick, then "send".
pub fn tabs(d: &Dialog) -> String {
    let mut parts: Vec<String> = d.qs.iter().enumerate().map(|(i, q)| {
        let tick = if d.answered(i) { "✓" } else { " " };
        if i == d.tab { format!("[{} {}]", tick, q.header) } else { format!(" {} {} ", tick, q.header) }
    }).collect();
    parts.push(if d.on_summary() { "[send]".to_string() } else { " send ".to_string() });
    parts.join("")
}

/// What the footer says for the open page.
pub fn hint(d: &Dialog) -> &'static str {
    if d.on_summary() {
        "enter sends · left goes back · esc closes without an answer"
    } else if d.typing {
        "type your answer · enter confirms · tab next · esc closes"
    } else if d.qs[d.tab].multi_select {
        "up/down · 1-6 or space toggles · enter next · tab switches · esc closes"
    } else {
        "up/down · 1-6 or enter chooses · type for your own answer · tab switches · esc closes"
    }
}

pub fn remaining_text(secs: u64) -> String {
    format!("{}:{:02} left", secs / 60, secs % 60)
}

pub fn draw(f: &mut Frame, area: Rect, d: &Dialog, now: u64) {
    let w = 84u16.min(area.width.saturating_sub(4)).max(10);
    let inner_w = (w as usize).saturating_sub(6).max(1);
    let (rows, option_rows) = if d.on_summary() {
        (Vec::new(), d.qs.len() * 2)
    } else {
        let q = &d.qs[d.tab];
        (wrap_plain(&q.question, inner_w), q.options.len() * 2 + usize::from(q.allow_text))
    };
    // A long question text scrolls with PgUp and PgDn. The options never scroll away.
    let max_inner = area.height.saturating_sub(6) as usize;
    let avail = max_inner.saturating_sub(option_rows + 6).max(1);
    let shown = rows.len().min(avail);
    let start = d.scroll.min(rows.len().saturating_sub(shown));
    let inner = dialog(f, area, "The main model asks", w, (shown + option_rows) as u16 + 10);
    let strip = tabs(d);
    put(f, inner, inner.x, inner.y, &clip(&strip, inner.width as usize), on(CLAY, RAISED));
    let left = remaining_text(d.remaining(now));
    // on the title row, left of the "esc" mark of the dialog frame
    put(f, area, (inner.right() + 1).saturating_sub(left.len() as u16 + 7), inner.y.saturating_sub(2), &left, on(if d.remaining(now) < 60 { RED } else { DIM }, RAISED));
    let mut y = inner.y + 2;
    if d.on_summary() {
        put(f, inner, inner.x, y, "Your answers", on(PAPER, RAISED));
        y += 1;
        let answers = d.answers();
        for (i, q) in d.qs.iter().enumerate() {
            let line = if d.answered(i) { answers[i].line() } else { "no answer yet".to_string() };
            put(f, inner, inner.x, y, &clip(&format!("{}  {}", q.header, q.question), inner.width as usize), on(ASH, RAISED));
            put(f, inner, inner.x + 2, y + 1, &clip(&line, (inner.width as usize).saturating_sub(2)), on(if d.answered(i) { SEA } else { GOLD }, RAISED));
            y += 2;
        }
    } else {
        let q = &d.qs[d.tab];
        for row in rows.iter().skip(start).take(shown) {
            put(f, inner, inner.x, y, row, on(PAPER, RAISED));
            y += 1;
        }
        if rows.len() > shown {
            put(f, inner, inner.x, y, &format!("lines {} to {} of {} · pgup and pgdn scroll", start + 1, start + shown, rows.len()), on(DIM, RAISED));
            y += 1;
        }
        y += 1;
        for (k, o) in q.options.iter().enumerate() {
            let chosen = d.cursor[d.tab] == k && !d.typing;
            let on_pick = d.picked[d.tab][k];
            let (bg, colour) = if chosen { (CLAY, INK) } else { (RAISED, PAPER) };
            let row = Rect::new(inner.x.saturating_sub(1), y, inner.width + 2, 1);
            fill(f, row, bg);
            let mark = match (q.multi_select, on_pick) { (true, true) => "[x]", (true, false) => "[ ]", (false, true) => "(o)", (false, false) => "( )" };
            put(f, row, inner.x, y, &clip(&format!("{} {} {}", k + 1, mark, o.label), inner.width as usize), on(colour, bg));
            if !o.description.is_empty() {
                put(f, inner, inner.x + 6, y + 1, &clip(&o.description, (inner.width as usize).saturating_sub(6)), on(DIM, RAISED));
            }
            y += 2;
        }
        if q.allow_text {
            let at_row = d.cursor[d.tab] == q.options.len();
            let (bg, colour) = if at_row || d.typing { (CLAY, INK) } else { (RAISED, PAPER) };
            let row = Rect::new(inner.x.saturating_sub(1), y, inner.width + 2, 1);
            fill(f, row, bg);
            let shown = if d.text[d.tab].is_empty() && !d.typing { "or type your own answer".to_string() } else { format!("{}{}", d.text[d.tab], if d.typing { "_" } else { "" }) };
            let room = (inner.width as usize).saturating_sub(4);
            let tail: String = if width(&shown) > room { shown.chars().rev().take(room).collect::<String>().chars().rev().collect() } else { shown };
            put(f, row, inner.x, y, &format!("> {}", tail), on(colour, bg));
            y += 1;
        }
    }
    if !d.warn.is_empty() {
        put(f, inner, inner.x, y + 1, &clip(&d.warn, inner.width as usize), on(GOLD, RAISED));
    }
    put(f, inner, inner.x, inner.bottom().saturating_sub(1), &clip(hint(d), inner.width as usize), on(DIM, RAISED));
}
