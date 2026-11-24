//! Draws the interface: the start screen, the transcript with the input box, the side panel and the dialogs.

use super::art::{self, col, Mood, AMBER, ASH, DIM, INK, NIGHT, PAPER, RAISED, RED, TEAL};
use super::text::{clip, input_rows, markdown, width, wrap_plain};
use super::{App, Item, Overlay, ToolState};
use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use ratatui::Frame;

const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const SIDE_W: u16 = 36;

fn fg(c: art::Rgb) -> Style {
    Style::default().fg(col(c))
}

fn on(c: art::Rgb, bg: art::Rgb) -> Style {
    Style::default().fg(col(c)).bg(col(bg))
}

fn bold(c: art::Rgb) -> Style {
    fg(c).add_modifier(Modifier::BOLD)
}

fn num(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// Writes text at (x, y), cut at the right edge of `area`.
fn put(f: &mut Frame, area: Rect, x: u16, y: u16, text: &str, style: Style) {
    if y < area.y || y >= area.bottom() || x >= area.right() {
        return;
    }
    f.buffer_mut().set_stringn(x, y, text, (area.right() - x) as usize, style);
}

fn fill(f: &mut Frame, area: Rect, bg: art::Rgb) {
    let area = area.intersection(f.area());
    f.buffer_mut().set_style(area, Style::default().bg(col(bg)));
}

fn spinner(app: &App) -> &'static str {
    SPINNER[(app.tick as usize) % SPINNER.len()]
}

fn model_line(app: &App, weak: bool) -> (String, String) {
    let m = &app.ctx.cfg[if weak { "weak" } else { "main" }];
    (m["model"].as_str().unwrap_or("?").to_string(), m["kind"].as_str().unwrap_or("?").to_string())
}

pub fn draw(f: &mut Frame, app: &mut App) {
    let area = f.area();
    fill(f, area, INK);
    f.buffer_mut().set_style(area, on(PAPER, INK));
    if area.width < 30 || area.height < 8 {
        put(f, area, 0, 0, "Strix needs 30 columns and 8 rows.", fg(ASH));
        return;
    }
    let body = Rect::new(area.x, area.y, area.width, area.height - 1);
    footer(f, app, Rect::new(area.x, area.bottom() - 1, area.width, 1));
    if app.items.is_empty() && app.busy.is_none() {
        home(f, app, body);
    } else {
        chat(f, app, body);
    }
    overlay(f, app, body);
}

fn footer(f: &mut Frame, app: &App, area: Rect) {
    let home = std::env::var("HOME").unwrap_or_default();
    let cwd = std::env::current_dir().map(|p| p.display().to_string()).unwrap_or_default();
    let cwd = if !home.is_empty() && cwd.starts_with(&home) { format!("~{}", &cwd[home.len()..]) } else { cwd };
    let right = format!("strix forgery {}", env!("CARGO_PKG_VERSION"));
    put(f, area, area.x + 2, area.y, &clip(&cwd, (area.width as usize).saturating_sub(width(&right) + 6)), fg(DIM));
    put(f, area, area.right().saturating_sub(width(&right) as u16 + 2), area.y, &right, fg(DIM));
    let _ = app;
}

/// The input box with its lower edge on `bottom`. Returns the row above the box.
fn input_box(f: &mut Frame, app: &App, x: u16, bottom: u16, w: u16, top_limit: u16, face: bool) -> u16 {
    let inner = (w as usize).saturating_sub(4).max(1);
    let (rows, at) = input_rows(&app.input, app.cursor, inner);
    let room = (bottom.saturating_sub(top_limit) as usize).saturating_sub(4).max(1);
    let shown = rows.len().min(8).min(room);
    let first = (at.0 + 1).saturating_sub(shown);
    let h = shown as u16 + 3;
    let top = bottom.saturating_sub(h);
    let rect = Rect::new(x, top, w, h);
    fill(f, rect, NIGHT);
    for y in top..bottom {
        put(f, rect, x, y, "┃", on(AMBER, NIGHT));
    }
    if app.input.is_empty() {
        put(f, rect, x + 2, top + 1, "Ask Strix anything", on(DIM, NIGHT));
    }
    for (i, row) in rows.iter().skip(first).take(shown).enumerate() {
        put(f, rect, x + 2, top + 1 + i as u16, row, on(PAPER, NIGHT));
    }
    let (main, kind) = model_line(app, false);
    let (weak, _) = model_line(app, true);
    let meta = bottom - 1;
    let mut cx = x + 2;
    for (text, style) in [("Strix".to_string(), bold(AMBER).bg(col(NIGHT))), (format!("  {}", main), on(PAPER, NIGHT)),
                          (format!(" {}", kind), on(DIM, NIGHT)), (format!("  ·  weaker {}", weak), on(DIM, NIGHT))] {
        put(f, rect, cx, meta, &text, style);
        cx += width(&text) as u16;
    }
    if face {
        let mood = app.mood();
        let label = format!("{} {}", mood.face(), mood.name());
        if cx + width(&label) as u16 + 4 < rect.right() {
            put(f, rect, rect.right() - width(&label) as u16 - 2, meta, &label, on(mood_colour(mood), NIGHT));
        }
    }
    if matches!(app.overlay, Overlay::None) && at.0 >= first && at.0 < first + shown {
        f.set_cursor_position(Position::new(x + 2 + at.1 as u16, top + 1 + (at.0 - first) as u16));
    }
    top
}

fn mood_colour(mood: Mood) -> art::Rgb {
    match mood {
        Mood::Awake | Mood::Learned => AMBER,
        Mood::Thinking | Mood::Working => TEAL,
        Mood::Asking => RED,
        Mood::Asleep => DIM,
    }
}

fn hints(f: &mut Frame, app: &App, area: Rect) {
    let toast = app.toast.as_ref().filter(|(_, t)| t.elapsed().as_secs() < 5).map(|(s, _)| s.as_str());
    let mut used = 0usize;
    if let Some(text) = toast {
        let text = clip(text, (area.width as usize).saturating_sub(4));
        put(f, area, area.x + 2, area.y, &text, fg(ASH));
        used = width(&text) + 4;
    } else if let Some(since) = app.busy {
        let working = app.mood() == Mood::Working;
        let text = format!("{} {} {}s", spinner(app), if working { "a tool runs" } else { "Strix thinks" }, since.elapsed().as_secs());
        put(f, area, area.x + 2, area.y, &text, fg(TEAL));
        put(f, area, area.x + 4 + width(&text) as u16, area.y, "esc stops", fg(DIM));
        used = width(&text) + 15;
    } else if app.learning {
        let text = format!("{} the weaker model looks for rules to keep", spinner(app));
        put(f, area, area.x + 2, area.y, &text, fg(DIM));
        used = width(&text) + 4;
    }
    let parts = [("enter", "send"), ("alt+enter", "new line"), ("/", "commands"), ("ctrl+p", "palette"), ("f1", "help")];
    let total: usize = parts.iter().map(|(k, v)| k.len() + v.len() + 4).sum::<usize>() - 2;
    if used + total + 2 > area.width as usize {
        return;
    }
    let mut x = area.right() - total as u16 - 1;
    for (k, v) in parts {
        put(f, area, x, area.y, k, fg(ASH));
        put(f, area, x + k.len() as u16 + 1, area.y, v, fg(DIM));
        x += (k.len() + v.len() + 4) as u16;
    }
}

/// The list of matching slash commands, drawn upwards from `bottom`.
fn slash_popup(f: &mut Frame, app: &App, x: u16, bottom: u16, w: u16, top_limit: u16) {
    let matches = app.slash_matches();
    if matches.is_empty() || !matches!(app.overlay, Overlay::None) {
        return;
    }
    let rows = matches.len().min(8).min(bottom.saturating_sub(top_limit) as usize);
    let sel = app.slash_sel.min(matches.len() - 1);
    let first = (sel + 1).saturating_sub(rows);
    for (i, cmd) in matches.iter().skip(first).take(rows).enumerate() {
        let y = bottom - rows as u16 + i as u16;
        let rect = Rect::new(x, y, w, 1);
        let chosen = first + i == sel;
        f.render_widget(Clear, rect);
        fill(f, rect, if chosen { AMBER } else { RAISED });
        let (name, help) = if chosen { (on(INK, AMBER).add_modifier(Modifier::BOLD), on(INK, AMBER)) } else { (on(AMBER, RAISED), on(ASH, RAISED)) };
        put(f, rect, x + 2, y, cmd.name, name);
        put(f, rect, x + 14, y, cmd.help, help);
        if !cmd.key.is_empty() && w as usize > 30 + cmd.help.len() + cmd.key.len() {
            put(f, rect, rect.right() - cmd.key.len() as u16 - 2, y, cmd.key, help);
        }
    }
}

fn home(f: &mut Frame, app: &App, area: Rect) {
    let w = area.width.saturating_sub(8).min(84).max(26);
    let x = area.x + (area.width - w) / 2;
    let (word, owl) = (art::wordmark(), art::peek());
    let show_word = area.height >= 34 && w >= 62;
    let show_owl = area.height >= 22 && w >= 44;
    let stack = if show_word { word.rows() + 4 } else { 3 } + if show_owl { owl.rows() } else { 1 } + 4 + 2;
    let mut y = area.y + area.height.saturating_sub(stack) / 2;
    if show_word {
        word.draw(f.buffer_mut(), x + (w - word.w as u16) / 2, y, area);
        y += word.rows();
        put(f, area, x + (w.saturating_sub(13)) / 2, y, "f o r g e r y", bold(AMBER));
        y += 2;
    } else {
        let nx = x + (w.saturating_sub(13)) / 2;
        put(f, area, nx, y, "strix", bold(PAPER));
        put(f, area, nx + 6, y, "forgery", bold(AMBER));
        y += 1;
    }
    let tagline = "A command line harness that remembers the way.";
    put(f, area, x + w.saturating_sub(width(tagline) as u16) / 2, y, tagline, fg(ASH));
    y += 2;
    if show_owl {
        owl.draw(f.buffer_mut(), x + w - owl.w as u16 - 4, y, area);
        let side = [format!("{} waypoints in the memory map", app.side.waypoints), format!("{} stored walks", app.side.walks),
                    if app.side.screencodec { "ScreenCodec connected".to_string() } else { "ScreenCodec not installed".to_string() },
                    format!("{} plugins", app.side.plugins)];
        for (i, line) in side.iter().enumerate() {
            let ly = y + owl.rows() - 5 + i as u16;
            put(f, Rect::new(x, ly, w.saturating_sub(owl.w as u16 + 6), 1), x + 2, ly, "◆", fg(TEAL));
            put(f, Rect::new(x, ly, w.saturating_sub(owl.w as u16 + 6), 1), x + 4, ly, line, fg(DIM));
        }
        y += owl.rows();
    } else {
        y += 1;
    }
    let bottom = (y + 4 + (input_rows(&app.input, app.cursor, (w as usize).saturating_sub(4)).0.len().min(8) as u16 - 1)).min(area.bottom() - 1);
    let top = input_box(f, app, x, bottom, w, area.y, !show_owl);
    hints(f, app, Rect::new(x, bottom, w, 1));
    slash_popup(f, app, x, top, w, area.y);
}

fn chat(f: &mut Frame, app: &mut App, area: Rect) {
    let side = app.sidebar && area.width >= 100 && area.height >= 16;
    let main = if side { Rect::new(area.x, area.y, area.width - SIDE_W, area.height) } else { area };
    if side {
        sidebar(f, app, Rect::new(main.right(), area.y, SIDE_W, area.height));
    }
    let pad = if main.width >= 60 { 2 } else { 1 };
    let (x, w) = (main.x + pad, main.width - 2 * pad);
    let bottom = main.bottom() - 1;
    let top = input_box(f, app, x, bottom, w, main.y + 2, !side);
    hints(f, app, Rect::new(x, bottom, w, 1));
    let view = Rect::new(x, main.y + 1, w, top.saturating_sub(main.y + 2));
    let lines = transcript(app, w as usize);
    let h = view.height as usize;
    app.page = (h / 2).max(1);
    app.scroll = app.scroll.min(lines.len().saturating_sub(h));
    let end = lines.len() - app.scroll;
    let start = end.saturating_sub(h);
    let offset = if lines.len() < h { 0 } else { h - (end - start) };
    f.render_widget(Paragraph::new(lines[start..end].to_vec()), Rect::new(view.x, view.y + offset as u16, view.width, (end - start) as u16));
    if app.scroll > 0 && view.height > 0 {
        let more = format!(" {} more lines below · page down ", app.scroll);
        put(f, view, view.right().saturating_sub(width(&more) as u16), view.bottom() - 1, &more, on(INK, AMBER));
    }
    slash_popup(f, app, x, top, w, main.y + 1);
}

fn transcript(app: &App, w: usize) -> Vec<Line<'static>> {
    let mut out: Vec<Line<'static>> = Vec::new();
    let mut last_small = false;
    for item in &app.items {
        let small = matches!(item, Item::Tool { .. } | Item::Note { .. });
        if !out.is_empty() && !(small && last_small) {
            out.push(Line::default());
        }
        last_small = small;
        match item {
            Item::User(text) => {
                let inner = w.saturating_sub(3).max(1);
                let bar = Span::styled("┃", on(AMBER, NIGHT));
                let blank = Line::from(vec![bar.clone(), Span::styled(" ".repeat(w.saturating_sub(1)), on(PAPER, NIGHT))]);
                out.push(blank.clone());
                for row in wrap_plain(text, inner) {
                    let pad = inner.saturating_sub(width(&row));
                    out.push(Line::from(vec![bar.clone(), Span::styled(format!(" {}{} ", row, " ".repeat(pad)), on(PAPER, NIGHT))]));
                }
                out.push(blank);
            }
            Item::Reply(text) => {
                for mut line in markdown(text, w.saturating_sub(4).max(1)) {
                    line.spans.insert(0, Span::raw("  "));
                    out.push(line);
                }
            }
            Item::Tool { name, detail, state, summary, .. } => {
                let (icon, colour) = match state {
                    ToolState::Running => (spinner(app), AMBER),
                    ToolState::Done => ("●", TEAL),
                    ToolState::Failed => ("●", RED),
                };
                let room = w.saturating_sub(width(name) + 8);
                out.push(Line::from(vec![Span::raw("  "), Span::styled(icon, fg(colour)), Span::raw(" "), Span::styled(name.clone(), bold(PAPER)),
                                         Span::raw("  "), Span::styled(clip(detail, room), fg(ASH))]));
                if !summary.is_empty() {
                    out.push(Line::from(vec![Span::styled("    ↳ ", fg(DIM)), Span::styled(clip(summary, w.saturating_sub(8)), fg(DIM))]));
                }
            }
            Item::Note { icon, label, text, colour } => {
                let room = w.saturating_sub(width(label) + 8);
                out.push(Line::from(vec![Span::raw("  "), Span::styled(*icon, fg(*colour)), Span::raw(" "), Span::styled(*label, bold(*colour)),
                                         Span::raw("  "), Span::styled(clip(text, room), fg(ASH))]));
            }
            Item::Error(text) => {
                for (i, row) in wrap_plain(text, w.saturating_sub(6).max(1)).into_iter().enumerate() {
                    out.push(Line::from(vec![Span::styled(if i == 0 { "  ✖ " } else { "    " }, fg(RED)), Span::styled(row, fg(RED))]));
                }
            }
        }
    }
    out
}

fn sidebar(f: &mut Frame, app: &App, area: Rect) {
    fill(f, area, NIGHT);
    let (x, w) = (area.x + 2, area.width.saturating_sub(4));
    let inner = Rect::new(x, area.y, w, area.height);
    let mood = app.mood();
    let mut y = area.y + 1;
    if area.height >= 34 {
        let head = art::head(mood, app.tick % 50 < 2);
        head.draw(f.buffer_mut(), area.x + (area.width - head.w as u16) / 2, y, area);
        y += head.rows() + 1;
    }
    let state = if app.busy.is_some() { format!("{} {}", mood.name(), spinner(app)) } else { mood.name().to_string() };
    let label = format!("Strix {} {}", mood.face(), state);
    put(f, inner, area.x + (area.width.saturating_sub(width(&label) as u16)) / 2, y, "Strix", bold(PAPER).bg(col(NIGHT)));
    put(f, inner, area.x + (area.width.saturating_sub(width(&label) as u16)) / 2 + 6, y, &format!("{} {}", mood.face(), state), on(mood_colour(mood), NIGHT));
    y += 2;

    let section = |f: &mut Frame, y: &mut u16, title: &str, rows: Vec<(String, String, art::Rgb)>| {
        if *y + 1 + rows.len() as u16 > area.bottom() {
            return;
        }
        put(f, inner, x, *y, title, bold(DIM).bg(col(NIGHT)));
        *y += 1;
        for (key, value, colour) in rows {
            put(f, inner, x, *y, &key, on(ASH, NIGHT));
            let vx = x + if key.is_empty() { 0 } else { 9 };
            put(f, inner, vx, *y, &clip(&value, (inner.right().saturating_sub(vx)) as usize), on(colour, NIGHT));
            *y += 1;
        }
        *y += 1;
    };

    section(f, &mut y, "SESSION", vec![
        ("tokens".into(), format!("{} in · {} out", num(app.tokens.0), num(app.tokens.1)), PAPER),
        ("turns".into(), format!("{} · {} tool calls", app.turns, app.tool_calls), PAPER),
    ]);
    let (main, mkind) = model_line(app, false);
    let (weak, wkind) = model_line(app, true);
    // The formation: the lead model answers, the model behind it reads tool output, the filter guards secrets.
    let privacy = if app.ctx.cfg["privacy"].as_str() == Some("off") { "off".to_string() } else { format!("rules · {} masked", app.side.masked) };
    section(f, &mut y, "FORMATION", vec![
        ("lead".into(), format!("{} · {}", main, mkind), PAPER),
        ("tools".into(), format!("{} · {}", weak, wkind), PAPER),
        ("privacy".into(), privacy, PAPER),
    ]);
    let mut memory = Vec::new();
    let way = if app.trail.is_empty() { "start".to_string() } else { format!("start › {}", app.trail.last().unwrap().replace('/', " › ")) };
    for row in wrap_plain(&way, w as usize) {
        memory.push((String::new(), row, TEAL));
    }
    memory.push((String::new(), format!("{} waypoints · {} stored walks", app.side.waypoints, app.side.walks), PAPER));
    for id in app.learned.iter().rev().take(3) {
        memory.push(("learned".into(), id.clone(), AMBER));
    }
    section(f, &mut y, "MEMORY", memory);
    let routed = |i: usize| {
        let (calls, kept) = app.side.routed[i];
        if calls == 0 { "no long output yet".to_string() } else { format!("{} calls · {} % kept away", calls, kept) }
    };
    section(f, &mut y, "ROUTED OUTPUT", vec![("run".into(), routed(0), PAPER), ("fetch".into(), routed(1), PAPER)]);
    section(f, &mut y, "EYES AND PLUGINS", vec![
        ("screen".into(), if app.side.screencodec { "● ScreenCodec".into() } else { "○ not installed".into() }, if app.side.screencodec { TEAL } else { DIM }),
        ("plugins".into(), format!("{}", app.side.plugins), PAPER),
        ("policy".into(), app.ctx.cfg["run_policy"].as_str().unwrap_or("safe").to_string(), PAPER),
    ]);
}

/// A dialog in the middle of `area`. Returns the inner rectangle below the title row.
fn dialog(f: &mut Frame, area: Rect, title: &str, want_w: u16, want_h: u16) -> Rect {
    let w = want_w.min(area.width.saturating_sub(4)).max(10).min(area.width);
    let h = want_h.min(area.height.saturating_sub(2)).max(3).min(area.height);
    let rect = Rect::new(area.x + (area.width - w) / 2, area.y + (area.height - h) / 2, w, h);
    f.render_widget(Clear, rect);
    fill(f, rect, RAISED);
    f.buffer_mut().set_style(rect, on(PAPER, RAISED));
    put(f, rect, rect.x + 2, rect.y + 1, title, bold(PAPER).bg(col(RAISED)));
    put(f, rect, rect.right().saturating_sub(5), rect.y + 1, "esc", on(DIM, RAISED));
    Rect::new(rect.x + 2, rect.y + 3, rect.width.saturating_sub(4), rect.height.saturating_sub(4))
}

fn overlay(f: &mut Frame, app: &App, area: Rect) {
    match &app.overlay {
        Overlay::None => {}
        Overlay::Palette { query, sel } => {
            let matches = App::palette_matches(query);
            let inner = dialog(f, area, "Commands", 64, matches.len() as u16 + 7);
            put(f, inner, inner.x, inner.y, ">", on(AMBER, RAISED));
            put(f, inner, inner.x + 2, inner.y, if query.is_empty() { "type to search" } else { query }, on(if query.is_empty() { DIM } else { PAPER }, RAISED));
            f.set_cursor_position(Position::new((inner.x + 2 + width(query) as u16).min(inner.right().saturating_sub(1)), inner.y));
            let rows = inner.height.saturating_sub(2) as usize;
            let first = (sel + 1).saturating_sub(rows);
            for (i, cmd) in matches.iter().skip(first).take(rows).enumerate() {
                let y = inner.y + 2 + i as u16;
                let chosen = first + i == *sel;
                let (bg, name, help) = if chosen { (AMBER, INK, INK) } else { (RAISED, AMBER, ASH) };
                let row = Rect::new(inner.x.saturating_sub(1), y, inner.width + 2, 1);
                fill(f, row, bg);
                put(f, row, inner.x, y, cmd.name, on(name, bg).add_modifier(Modifier::BOLD));
                put(f, row, inner.x + 11, y, &clip(cmd.help, (inner.width as usize).saturating_sub(13 + cmd.key.len())), on(help, bg));
                put(f, row, inner.right().saturating_sub(cmd.key.len() as u16), y, cmd.key, on(if chosen { INK } else { DIM }, bg));
            }
        }
        Overlay::Models { sel, weak } => {
            let choices = app.model_choices(*weak);
            let inner = dialog(f, area, "Models", 64, choices.len() as u16 + 9);
            let tab = |active: bool| if active { on(INK, AMBER).add_modifier(Modifier::BOLD) } else { on(ASH, RAISED) };
            put(f, inner, inner.x, inner.y, " main model ", tab(!*weak));
            put(f, inner, inner.x + 13, inner.y, " weaker model ", tab(*weak));
            let (current, kind) = model_line(app, *weak);
            for (i, name) in choices.iter().enumerate() {
                let y = inner.y + 2 + i as u16;
                let chosen = i == (*sel).min(choices.len().saturating_sub(1));
                let bg = if chosen { NIGHT } else { RAISED };
                fill(f, Rect::new(inner.x.saturating_sub(1), y, inner.width + 2, 1), bg);
                put(f, inner, inner.x, y, if *name == current { "●" } else { "○" }, on(if *name == current { TEAL } else { DIM }, bg));
                put(f, inner, inner.x + 2, y, name, on(PAPER, bg));
                put(f, inner, inner.x + 2 + width(name) as u16 + 2, y, &kind, on(DIM, bg));
            }
            let note = if kind == "openai" { "Other endpoints and models: .strix/config.json" } else { "Models of the claude command, with the login of the user" };
            put(f, inner, inner.x, inner.y + 3 + choices.len() as u16, note, on(DIM, RAISED));
            put(f, inner, inner.x, inner.y + 4 + choices.len() as u16, "tab switches · enter chooses", on(DIM, RAISED));
        }
        Overlay::Pager { title, lines, scroll } => {
            let widest = lines.iter().map(|l| width(l)).max().unwrap_or(20) as u16;
            let inner = dialog(f, area, title, (widest + 4).clamp(44, 100), lines.len() as u16 + 5);
            let mut rows: Vec<String> = Vec::new();
            for l in lines {
                rows.extend(wrap_plain(l, inner.width as usize));
            }
            let h = inner.height as usize;
            let start = (*scroll).min(rows.len().saturating_sub(h));
            for (i, row) in rows.iter().skip(start).take(h).enumerate() {
                put(f, inner, inner.x, inner.y + i as u16, row, on(PAPER, RAISED));
            }
            if rows.len() > h {
                let more = format!("{} to {} of {} · up and down scroll", start + 1, (start + h).min(rows.len()), rows.len());
                put(f, inner, inner.x, inner.bottom(), &more, on(DIM, RAISED));
            }
        }
        Overlay::Approve { command, cwd, .. } => {
            let w = 76u16.min(area.width.saturating_sub(4)).max(10);
            let rows = wrap_plain(command, (w as usize).saturating_sub(8).max(1));
            let inner = dialog(f, area, "Run this command?", w, rows.len() as u16 + 10);
            put(f, inner, inner.x, inner.y, "The command is outside the safe list. Strix waits for an answer.", on(ASH, RAISED));
            let mut y = inner.y + 2;
            for row in rows.iter().take(inner.height.saturating_sub(5) as usize) {
                fill(f, Rect::new(inner.x, y, inner.width, 1), INK);
                put(f, inner, inner.x + 1, y, "$", on(AMBER, INK));
                put(f, inner, inner.x + 3, y, row, on(PAPER, INK));
                y += 1;
            }
            if !cwd.is_empty() {
                put(f, inner, inner.x, y, &format!("in {}", clip(cwd, (inner.width as usize).saturating_sub(4))), on(DIM, RAISED));
            }
            y += 2;
            let mut x = inner.x;
            for (key, text) in [("y", "run it once"), ("a", "allow all in this session"), ("n", "refuse")] {
                put(f, inner, x, y, &format!(" {} ", key), on(INK, AMBER).add_modifier(Modifier::BOLD));
                put(f, inner, x + 4, y, text, on(PAPER, RAISED));
                x += 4 + text.len() as u16 + 3;
            }
        }
    }
}
