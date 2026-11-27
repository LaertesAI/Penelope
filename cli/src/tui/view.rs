//! Draws the interface: the start screen, the transcript with the input box, the side panel and the dialogs.

use super::art::{self, col, Mood, CLAY, ASH, DIM, GOLD, INK, NIGHT, PAPER, RAISED, RED, SEA};
use super::text::{clip, input_rows, markdown, width, wrap_plain};
use super::picture;
use super::weave::{self, Look, Tone};
use super::{App, Item, Overlay, ToolState};
use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use ratatui::Frame;

const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

fn fg(c: art::Rgb) -> Style {
    Style::default().fg(col(c))
}

pub(super) fn on(c: art::Rgb, bg: art::Rgb) -> Style {
    Style::default().fg(col(c)).bg(col(bg))
}

pub(super) fn bold(c: art::Rgb) -> Style {
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
pub(super) fn put(f: &mut Frame, area: Rect, x: u16, y: u16, text: &str, style: Style) {
    if y < area.y || y >= area.bottom() || x >= area.right() {
        return;
    }
    f.buffer_mut().set_stringn(x, y, text, (area.right() - x) as usize, style);
}

pub(super) fn fill(f: &mut Frame, area: Rect, bg: art::Rgb) {
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
    art::set_palette(app.palette);
    let area = f.area();
    picture::cancel();
    fill(f, area, INK);
    f.buffer_mut().set_style(area, on(PAPER, INK));
    if area.width < 30 || area.height < 8 {
        put(f, area, 0, 0, "Penelope needs 30 columns and 8 rows.", fg(ASH));
        return;
    }
    let bar = if !app.tabs.is_empty() && area.height >= 14 { 1 } else { 0 };
    let body = Rect::new(area.x, area.y + bar, area.width, area.height - 1 - bar);
    if bar == 1 {
        tabs(f, app, Rect::new(area.x, area.y, area.width, 1));
    }
    footer(f, app, Rect::new(area.x, area.bottom() - 1, area.width, 1));
    if app.items.is_empty() && app.busy.is_none() {
        home(f, app, body);
    } else {
        chat(f, app, body);
    }
    overlay(f, app, body);
    if !matches!(app.overlay, Overlay::None) {
        picture::cancel();                       // a dialog may lie over the picture, so the picture leaves for that time
    }
}

/// A path with the home folder written as `~`, so that no screen picture shows the name of the account.
fn tilde(path: &str) -> String {
    let home = crate::platform::home_str();
    if !home.is_empty() && path.starts_with(&home) { format!("~{}", &path[home.len()..]) } else { path.to_string() }
}

/// The stored sessions as tabs, oldest on the left. The open one is marked. A session without an answer yet shows as "new".
/// If the row is too narrow, the tabs around the open one stay.
fn tabs(f: &mut Frame, app: &App, area: Rect) {
    fill(f, area, NIGHT);
    let open = app.session_id.as_ref().and_then(|id| app.tabs.iter().position(|(t, _)| t == id));
    let mut labels: Vec<(String, bool)> = app.tabs.iter().enumerate().map(|(i, (_, title))| (format!(" {} {} ", i + 1, clip(title, 20)), Some(i) == open)).collect();
    if open.is_none() {
        labels.push((" + new ".to_string(), true));
    }
    let hint = "f2 list · f3 f4 change ";
    let room = (area.width as usize).saturating_sub(hint.len() + 2);
    let at = labels.iter().position(|(_, on)| *on).unwrap_or(0);
    let (mut first, mut last, mut used) = (at, at, width(&labels[at].0));
    loop {                                                   // grow the window to both sides while it fits
        let before = first;
        let after = last;
        if last + 1 < labels.len() && used + width(&labels[last + 1].0) <= room {
            last += 1;
            used += width(&labels[last].0);
        }
        if first > 0 && used + width(&labels[first - 1].0) <= room {
            first -= 1;
            used += width(&labels[first].0);
        }
        if first == before && last == after {
            break;
        }
    }
    let mut x = area.x + 1;
    for (label, on_tab) in &labels[first..=last] {
        put(f, area, x, area.y, label, if *on_tab { on(INK, CLAY).add_modifier(Modifier::BOLD) } else { on(ASH, NIGHT) });
        x += width(label) as u16;
    }
    put(f, area, area.right().saturating_sub(hint.len() as u16), area.y, hint, on(DIM, NIGHT));
}

/// The same for a text that holds a path somewhere inside.
fn tilde_all(text: &str) -> String {
    let home = crate::platform::home_str();
    if home.len() > 1 { text.replace(&home, "~") } else { text.to_string() }
}

fn footer(f: &mut Frame, app: &App, area: Rect) {
    let cwd = tilde(&std::env::current_dir().map(|p| p.display().to_string()).unwrap_or_default());
    let right = format!("penelope {}", env!("CARGO_PKG_VERSION"));
    put(f, area, area.x + 2, area.y, &clip(&cwd, (area.width as usize).saturating_sub(width(&right) + 6)), fg(DIM));
    put(f, area, area.right().saturating_sub(width(&right) as u16 + 2), area.y, &right, fg(DIM));
    // The model and its effort sit left of the version, so an effort change shows at once.
    let model = app.footer_label();
    let at = area.right().saturating_sub((width(&right) + width(&model) + 6) as u16);
    if at > area.x + 2 + width(&cwd).min(12) as u16 {
        put(f, area, at, area.y, &model, fg(SEA));
    }
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
        put(f, rect, x, y, "┃", on(CLAY, NIGHT));
    }
    if app.input.is_empty() {
        put(f, rect, x + 2, top + 1, "Ask Penelope anything", on(DIM, NIGHT));
    }
    for (i, row) in rows.iter().skip(first).take(shown).enumerate() {
        put(f, rect, x + 2, top + 1 + i as u16, row, on(PAPER, NIGHT));
    }
    let (main, kind) = model_line(app, false);
    let (weak, _) = model_line(app, true);
    let meta = bottom - 1;
    let mut cx = x + 2;
    for (text, style) in [("Penelope".to_string(), bold(CLAY).bg(col(NIGHT))), (format!("  {}", main), on(PAPER, NIGHT)),
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
        Mood::Awake | Mood::Learned => CLAY,
        Mood::Thinking | Mood::Working => SEA,
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
        let text = format!("{} {} {}s", spinner(app), if working { "a tool runs" } else { "Penelope thinks" }, since.elapsed().as_secs());
        put(f, area, area.x + 2, area.y, &text, fg(SEA));
        put(f, area, area.x + 4 + width(&text) as u16, area.y, "esc stops", fg(DIM));
        used = width(&text) + 15;
    } else if app.learning {
        let text = format!("{} the weaker model looks for rules to keep", spinner(app));
        put(f, area, area.x + 2, area.y, &text, fg(DIM));
        used = width(&text) + 4;
    }
    let parts = [("enter", "send"), ("alt+enter", "new line"), ("/", "commands"), ("ctrl+p", "palette"), ("f5", "new"), ("f6", "delete"), ("f1", "help")];
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
        fill(f, rect, if chosen { CLAY } else { RAISED });
        let (name, help) = if chosen { (on(INK, CLAY).add_modifier(Modifier::BOLD), on(INK, CLAY)) } else { (on(CLAY, RAISED), on(ASH, RAISED)) };
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
    let show_side = area.height >= 22 && w >= 44;
    // The painted picture stands at the right where the terminal shows pictures: 24 columns and 12 rows,
    // or 32 columns and 16 rows on a tall screen. No terminal gets a picture made of large pixels.
    let (pw, ph): (u16, u16) = if !(show_side && picture::on()) { (0, 0) } else if area.height >= 40 { (32, 16) } else { (24, 12) };
    let block = if show_side { ph.max(5) } else { 1 };
    let stack = 3 + block + 4 + 2;
    let mut y = area.y + area.height.saturating_sub(stack) / 2;
    let name = if w >= 44 { "P E N E L O P E" } else { "penelope" };
    put(f, area, x + w.saturating_sub(width(name) as u16) / 2, y, name, bold(CLAY));
    y += 1;
    let tagline = "A command line harness that remembers the way.";
    put(f, area, x + w.saturating_sub(width(tagline) as u16) / 2, y, tagline, fg(ASH));
    y += 2;
    if show_side {
        if ph > 0 {
            picture::want(Rect::new(x + w - pw - 4, y, pw, ph));
        }
        let side = [format!("{} waypoints in the memory map", app.side.waypoints), format!("{} stored walks", app.side.walks),
                    if app.side.screencodec { "ScreenCodec connected".to_string() } else { "ScreenCodec not installed".to_string() },
                    format!("{} plugins", app.side.plugins)];
        let lw = w.saturating_sub(pw + 6);
        for (i, line) in side.iter().enumerate() {
            let ly = y + block - 5 + i as u16;
            put(f, Rect::new(x, ly, lw, 1), x + 2, ly, "◆", fg(SEA));
            put(f, Rect::new(x, ly, lw, 1), x + 4, ly, line, fg(DIM));
        }
        y += block;
    } else {
        y += 1;
    }
    let bottom = (y + 4 + (input_rows(&app.input, app.cursor, (w as usize).saturating_sub(4)).0.len().min(8) as u16 - 1)).min(area.bottom() - 1);
    let top = input_box(f, app, x, bottom, w, area.y, !show_side);
    hints(f, app, Rect::new(x, bottom, w, 1));
    slash_popup(f, app, x, top, w, area.y);
}

fn chat(f: &mut Frame, app: &mut App, area: Rect) {
    // The layout of a mod: side keeps the 36 column panel, compact narrows it, top hides it.
    let side_w = app.layout.sidebar_width();
    let side = app.sidebar && app.layout.shows_sidebar() && side_w > 0 && area.width >= side_w + 64 && area.height >= 16;
    let main = if side { Rect::new(area.x, area.y, area.width - side_w, area.height) } else { area };
    if side {
        sidebar(f, app, Rect::new(main.right(), area.y, side_w, area.height));
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
        put(f, view, view.right().saturating_sub(width(&more) as u16), view.bottom() - 1, &more, on(INK, CLAY));
    }
    slash_popup(f, app, x, top, w, main.y + 1);
}

fn transcript(app: &App, w: usize) -> Vec<Line<'static>> {
    let mut out: Vec<Line<'static>> = Vec::new();
    let mut last_small = false;
    for (idx, item) in app.items.iter().enumerate() {
        let small = matches!(item, Item::Tool { .. } | Item::Note { .. });
        if !out.is_empty() && !(small && last_small) {
            out.push(Line::default());
        }
        last_small = small;
        match item {
            Item::User(text) => {
                let inner = w.saturating_sub(3).max(1);
                let bar = Span::styled("┃", on(CLAY, NIGHT));
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
                    ToolState::Running => (spinner(app), CLAY),
                    ToolState::Done => ("●", SEA),
                    ToolState::Failed => ("●", RED),
                };
                let room = w.saturating_sub(width(name) + 8);
                out.push(Line::from(vec![Span::raw("  "), Span::styled(icon, fg(colour)), Span::raw(" "), Span::styled(name.clone(), bold(PAPER)),
                                         Span::raw("  "), Span::styled(clip(&tilde_all(detail), room), fg(ASH))]));
                if !summary.is_empty() {
                    out.push(Line::from(vec![Span::styled("    ↳ ", fg(DIM)), Span::styled(clip(&tilde_all(summary), w.saturating_sub(8)), fg(DIM))]));
                }
            }
            Item::Note { icon, label, text, colour } => {
                let room = w.saturating_sub(width(label) + 8);
                out.push(Line::from(vec![Span::raw("  "), Span::styled(*icon, fg(*colour)), Span::raw(" "), Span::styled(*label, bold(*colour)),
                                         Span::raw("  "), Span::styled(clip(text, room), fg(ASH))]));
            }
            Item::Error(text) => {
                // the broken thread of a failed turn, once, under the last error
                if idx + 1 == app.items.len() && app.busy.is_none() {
                    let rows = weave::frame(&weave::Params { look: Look::Error, tick: 0, width: w.saturating_sub(2), steps: 0, reduced: true });
                    if let Some(row) = rows.get(1).or_else(|| rows.first()) {
                        out.push(Line::from(weave_line(row)));
                    }
                }
                for (i, row) in wrap_plain(text, w.saturating_sub(6).max(1)).into_iter().enumerate() {
                    out.push(Line::from(vec![Span::styled(if i == 0 { "  ✖ " } else { "    " }, fg(RED)), Span::styled(row, fg(RED))]));
                }
            }
        }
    }
    if let Some(since) = app.busy.filter(|_| !app.streaming) {
        if !out.is_empty() {
            out.push(Line::default());
        }
        out.extend(loom(app, w, since.elapsed().as_secs()));
    }
    out
}

/// What the loom says while the model works, one phase every three seconds.
const LOOM_PHASES: [&str; 4] = ["warping the loom", "casting the shuttle", "beating the weft", "reading the pattern"];

/// The look of the waiting block: what the turn is doing right now.
fn look_of(app: &App) -> Look {
    if matches!(app.overlay, Overlay::Approve { .. } | Overlay::Question { .. } | Overlay::Ask { .. }) {
        Look::Waiting
    } else if app.side.clusters > 0 {
        Look::Cluster(app.side.clusters)
    } else if app.mood() == Mood::Working {
        Look::Tool
    } else {
        Look::Thinking
    }
}

fn tone_style(t: Tone) -> Style {
    match t {
        Tone::Post => fg(ASH),
        Tone::Warp => fg(DIM),
        Tone::Weft => fg(CLAY),
        Tone::Shuttle => bold(SEA),
        Tone::Cloth => fg(CLAY),
        Tone::Idle => fg(DIM),
        Tone::Error => bold(RED),
    }
}

fn weave_line(row: &weave::Row) -> Vec<Span<'static>> {
    let mut out: Vec<Span<'static>> = Vec::new();
    let mut run = String::new();
    let mut tone = None;
    for &(c, t) in row {
        if tone.map(|x| x != t).unwrap_or(false) {
            out.push(Span::styled(std::mem::take(&mut run), tone_style(tone.unwrap())));
        }
        tone = Some(t);
        run.push(c);
    }
    if let Some(t) = tone {
        out.push(Span::styled(run, tone_style(t)));
    }
    out
}

/// Steps of this turn that are finished: the tool calls and the text blocks after the last request of the person.
fn finished_steps(app: &App) -> usize {
    let from = app.items.iter().rposition(|i| matches!(i, Item::User(_))).map(|i| i + 1).unwrap_or(0);
    app.items[from..].iter().filter(|i| matches!(i, Item::Tool { state: ToolState::Done | ToolState::Failed, .. } | Item::Reply(_))).count()
}

/// The waiting block under the transcript, a weaving: warp threads hang down, a shuttle crosses and lays the weft, and
/// each finished step adds a row of cloth (see `weave.rs`). A waiting person, running tools and running clusters each
/// look different. Below it the tail of the model's thinking, when the provider streams it.
pub fn loom(app: &App, w: usize, secs: u64) -> Vec<Line<'static>> {
    let look = look_of(app);
    let params = weave::Params { look, tick: app.tick, width: w.saturating_sub(2), steps: finished_steps(app), reduced: weave::reduced(&app.ctx.cfg) };
    let rows = weave::frame(&params);
    let phase = match look {
        Look::Waiting => "waiting for you".to_string(),
        Look::Tool => "a tool runs".to_string(),
        Look::Cluster(n) => format!("{} cluster{} weaving", n, if n == 1 { "" } else { "s" }),
        Look::Error => "a thread broke".to_string(),
        Look::Thinking => LOOM_PHASES[(secs / 3) as usize % LOOM_PHASES.len()].to_string(),
    };
    let mut out: Vec<Line<'static>> = Vec::new();
    let shed = if rows.len() == 1 { 0 } else { 1 };
    for (i, row) in rows.iter().enumerate() {
        let mut spans = weave_line(row);
        if i == shed {
            spans.push(Span::styled(format!("  {} · {}s", phase, secs), fg(ASH)));
        }
        out.push(Line::from(spans));
    }
    let thought: String = app.thinking.split_whitespace().collect::<Vec<_>>().join(" ");
    if !thought.is_empty() {
        let rows = wrap_plain(&thought, w.saturating_sub(8).max(10));
        let first = rows.len().saturating_sub(2);
        for (i, row) in rows.into_iter().skip(first).enumerate() {
            let lead = if i == 0 { "    ↳ " } else { "      " };
            out.push(Line::from(vec![Span::styled(lead, fg(DIM)), Span::styled(row, fg(DIM).add_modifier(Modifier::ITALIC))]));
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
    if area.height >= 34 && picture::on() {
        // the painted picture, 22 columns wide, where the terminal shows pictures
        let (pw, ph) = (22u16, 11u16);
        picture::want(Rect::new(area.x + (area.width - pw) / 2, y, pw, ph));
        y += ph + 1;
    }
    let state = if app.busy.is_some() { format!("{} {}", mood.name(), spinner(app)) } else { mood.name().to_string() };
    let label = format!("Penelope {} {}", mood.face(), state);
    put(f, inner, area.x + (area.width.saturating_sub(width(&label) as u16)) / 2, y, "Penelope", bold(PAPER).bg(col(NIGHT)));
    put(f, inner, area.x + (area.width.saturating_sub(width(&label) as u16)) / 2 + 9, y, &format!("{} {}", mood.face(), state), on(mood_colour(mood), NIGHT));
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
        ("cost".into(), app.side.cost.clone(), SEA),
        ("mode".into(), format!("{} · {}", app.mode, crate::mode::current(&app.ctx)), SEA),
    ]);
    let (main, _) = model_line(app, false);
    let privacy = match app.ctx.cfg["privacy"].as_str().unwrap_or("rules") {
        "off" => "off".to_string(),
        level => format!("{} · {} masked", level, app.side.masked),
    };
    // The crew: the lead, then the subagents of the last run under the names the user or the orchestrator gave them.
    // A model of a Laertes family is gold and says "in training"; it cannot answer yet.
    let role_row = |role: &str, key: &str, state: &str| -> (String, String, art::Rgb) {
        match crate::route::model_for(&app.ctx, role) {
            Some(m) => {
                let (shown, family) = crate::crew::describe(m);
                let value = match family {
                    Some(f) => format!("{} · in training", f),
                    None => format!("{} · {}", shown, state),
                };
                (key.to_string(), value, if family.is_some() { GOLD } else { PAPER })
            }
            None => (key.to_string(), "rules".to_string(), PAPER),
        }
    };
    let mut crew = Vec::new();
    let lead = match crate::crew::family_of(&app.ctx.cfg["main"]) {
        Some(f) => (format!("lead"), format!("{} · in training", f), GOLD),
        None => {
            let mood = app.mood();
            ("lead".to_string(), format!("{} · {} · {}", main, app.ctx.cfg["main"]["kind"].as_str().unwrap_or("claude-cli"),
                                        if app.busy.is_some() { mood.name() } else { "ready" }), PAPER)
        }
    };
    crew.push(lead);
    let members = crate::crew::list(&app.ctx);
    let mut any_gold = crew.iter().any(|(_, _, c)| *c == GOLD);
    if members.is_empty() {
        let scout_state = match app.ctx.cfg["delegate"].as_str().unwrap_or("auto") { "off" => "no scout", mode => mode };
        for (role, key, state) in [("scout", "nostos", scout_state), ("tool", "computer", "ScreenCodec"), ("router", "tiresias", "routing"),
                                   ("filter", "utis", "masking"), ("condenser", "aiolos", "condensing"), ("judge", "argos", "judging")] {
            let row = role_row(role, key, state);
            any_gold |= row.2 == GOLD;
            crew.push(row);
        }
        if let Some((_, value, colour)) = crew.iter_mut().find(|(k, _, _)| k == "utis") {
            *value = format!("{} · {}", value, privacy);
            let _ = colour;
        }
    } else {
        // A subagent gets two rows: its name as the user or the orchestrator gave it, then the plugged model and
        // its state. Two rows fit in a 40 row terminal and keep the full name readable.
        for m in members.iter().take(2) {
            any_gold |= m.family.is_some();
            let gold = if m.family.is_some() { GOLD } else { PAPER };
            crew.push((String::new(), format!("{} · {}", m.role, m.name), gold));
            crew.push((String::new(), format!("   {} · {}", m.model, m.state), if m.family.is_some() { GOLD } else { ASH }));
        }
        if members.len() > 2 {
            crew.push((String::new(), format!("   +{} more in the run", members.len() - 2), DIM));
        }
    }
    if any_gold {
        crew.push((String::new(), "gold · Laertes model, in training".to_string(), DIM));
    }
    section(f, &mut y, "CREW", crew);
    if !app.side.plan.is_empty() {
        let rows = app.side.plan.iter().take(8).map(|(text, state)| {
            let (mark, colour) = match state.as_str() {
                "done" => ("●", DIM),
                "doing" => ("◐", CLAY),
                _ => ("○", ASH),
            };
            (String::new(), format!("{} {}", mark, text), colour)
        }).collect();
        section(f, &mut y, "PLAN", rows);
    }
    let mut memory = Vec::new();
    let way = if app.trail.is_empty() { "start".to_string() } else { format!("start › {}", app.trail.last().unwrap().replace('/', " › ")) };
    for row in wrap_plain(&way, w as usize) {
        memory.push((String::new(), row, SEA));
    }
    memory.push((String::new(), format!("{} waypoints · {} stored walks", app.side.waypoints, app.side.walks), PAPER));
    if let Some(last) = app.context_refs.last() {
        memory.push(("context".into(), last.clone(), SEA));
    }
    for id in app.learned.iter().rev().take(3) {
        memory.push(("learned".into(), id.clone(), CLAY));
    }
    section(f, &mut y, "MEMORY", memory);
    let routed = |i: usize| {
        let (calls, kept) = app.side.routed[i];
        if calls == 0 { "no long output yet".to_string() } else { format!("{} calls · {} % kept away", calls, kept) }
    };
    let scout = if app.side.scout.0 == 0 { "no task yet".to_string() } else { format!("{} tasks · {} tokens", app.side.scout.0, num(app.side.scout.1)) };
    section(f, &mut y, "ROUTED TO THE WEAKER MODEL", vec![("run".into(), routed(0), PAPER), ("fetch".into(), routed(1), PAPER), ("scout".into(), scout, PAPER)]);
    section(f, &mut y, "EYES AND PLUGINS", vec![
        ("screen".into(), if app.side.screencodec { "● ScreenCodec".into() } else { "○ not installed".into() }, if app.side.screencodec { SEA } else { DIM }),
        ("cursor".into(), if app.side.cursor { "● own cursor".into() } else { "○ no cursor command".into() }, if app.side.cursor { SEA } else { DIM }),
        ("plugins".into(), format!("{}", app.side.plugins), PAPER),
        ("clusters".into(), if app.side.clusters > 0 { format!("{} working", app.side.clusters) } else { "none working".to_string() }, if app.side.clusters > 0 { CLAY } else { DIM }),
        ("policy".into(), app.ctx.cfg["run_policy"].as_str().unwrap_or("safe").to_string(), PAPER),
    ]);
}

/// A dialog in the middle of `area`. Returns the inner rectangle below the title row.
pub(super) fn dialog(f: &mut Frame, area: Rect, title: &str, want_w: u16, want_h: u16) -> Rect {
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

fn provider_state(p: &crate::providers::Provider, claude: &str) -> (&'static str, art::Rgb) {
    if p.kind == "laertes" {
        return ("in training", GOLD);
    }
    if p.name == "claude-cli" {
        let signed = claude.contains("signed in");
        return (if signed { "signed in" } else { "no login" }, if signed { SEA } else { CLAY });
    }
    if p.key_optional {
        return ("no key needed", SEA);
    }
    if !p.key_env.is_empty() && std::env::var(p.key_env).ok().filter(|v| !v.trim().is_empty()).is_some() {
        return ("env set", SEA);
    }
    if crate::auth::has(p.name) {
        return ("stored", SEA);
    }
    ("no key", CLAY)
}

fn target_label(target: &str) -> String {
    match target {
        "main" => "Main model".to_string(),
        "weak" => "Weaker model".to_string(),
        "scout" => "Research + web search".to_string(),
        other => super::ROLE_MODELS.iter().find(|(r, _)| *r == other).map(|(_, label)| label.to_string()).unwrap_or_else(|| other.to_string()),
    }
}

fn target_value<'a>(app: &'a App, target: &str) -> &'a serde_json::Value {
    match target {
        "main" | "weak" | "scout" => &app.ctx.cfg[target],
        other => &app.ctx.cfg["roles"][other],
    }
}

fn providers_dialog(f: &mut Frame, app: &App, area: Rect, sel: usize, target: &str, scroll: usize) {
    let list = &crate::providers::TABLE;
    let rows = (area.height.saturating_sub(9) as usize).clamp(4, 24).min(list.len().max(1));
    let inner = dialog(f, area, "Providers · connections", 96, rows as u16 + 8);
    let tab = |active: bool| if active { on(INK, CLAY).add_modifier(Modifier::BOLD) } else { on(ASH, RAISED) };
    if target == "main" || target == "weak" {
        put(f, inner, inner.x, inner.y, " main model ", tab(target == "main"));
        put(f, inner, inner.x + 13, inner.y, " weaker model ", tab(target == "weak"));
    } else {
        put(f, inner, inner.x, inner.y, &format!(" {} ", target_label(target)), tab(true));
    }
    let current = crate::providers::of(target_value(app, target)).map(|p| p.name);
    let first = scroll.min(list.len().saturating_sub(rows));
    for (i, p) in list.iter().skip(first).take(rows).enumerate() {
        let y = inner.y + 2 + i as u16;
        let chosen = first + i == sel;
        let bg = if chosen { NIGHT } else { RAISED };
        fill(f, Rect::new(inner.x.saturating_sub(1), y, inner.width + 2, 1), bg);
        put(f, inner, inner.x, y, if current == Some(p.name) { "●" } else { "○" }, fg(if current == Some(p.name) { SEA } else { DIM }));
        put(f, inner, inner.x + 2, y, &clip(p.label, 28), on(if p.kind == "laertes" { GOLD } else { PAPER }, bg).add_modifier(if chosen { Modifier::BOLD } else { Modifier::empty() }));
        put(f, inner, inner.x + 32, y, &clip(p.kind, 10), on(DIM, bg));
        let (state, colour) = provider_state(p, &app.side.claude);
        put(f, inner, inner.x + 44, y, &clip(state, 14), on(colour, bg));
        put(f, inner, inner.x + 60, y, &clip(if p.key_env.is_empty() { "-" } else { p.key_env }, 28), on(DIM, bg));
    }
    let p = &list[sel.min(list.len().saturating_sub(1))];
    let base = if p.base.is_empty() { "no address, the claude command" } else { p.base };
    let detail = format!("{} · example {} · {}", base, p.model, p.note);
    put(f, inner, inner.x, inner.bottom().saturating_sub(2), &clip(&detail, inner.width as usize), on(DIM, RAISED));
    let hint = if p.kind == "claude-cli" { "enter signs in with the official claude command" }
        else if p.kind == "laertes" { "this family is in training" }
        else { "enter opens the masked API-key window" };
    put(f, inner, inner.x, inner.bottom().saturating_sub(1), &clip(hint, inner.width.saturating_sub(20) as usize), on(CLAY, RAISED));
    put(f, inner, inner.right().saturating_sub(19), inner.bottom().saturating_sub(1), "tab main/weak · esc", on(DIM, RAISED));
}

fn roles_dialog(f: &mut Frame, app: &App, area: Rect, sel: usize) {
    let inner = dialog(f, area, "Crew models", 78, super::ROLE_MODELS.len() as u16 + 8);
    put(f, inner, inner.x, inner.y, "Choose one model per role. Web search and research share the scout role.", on(DIM, RAISED));
    for (i, (role, label)) in super::ROLE_MODELS.iter().enumerate() {
        let y = inner.y + 2 + i as u16;
        let chosen = i == sel;
        let bg = if chosen { NIGHT } else { RAISED };
        fill(f, Rect::new(inner.x.saturating_sub(1), y, inner.width + 2, 1), bg);
        let m = if *role == "scout" { &app.ctx.cfg["scout"] } else { &app.ctx.cfg["roles"][role] };
        let model = m["model"].as_str().filter(|s| !s.is_empty()).unwrap_or("follows the weak model");
        let effort = m["effort"].as_str().unwrap_or("");
        put(f, inner, inner.x, y, if chosen { "●" } else { "○" }, on(if chosen { SEA } else { DIM }, bg));
        put(f, inner, inner.x + 2, y, &format!("{:<24}", clip(label, 24)), on(PAPER, bg).add_modifier(if chosen { Modifier::BOLD } else { Modifier::empty() }));
        put(f, inner, inner.x + 28, y, &clip(model, 34), on(DIM, bg));
        put(f, inner, inner.x + 64, y, effort, on(if effort.is_empty() { DIM } else { SEA }, bg));
    }
    put(f, inner, inner.x, inner.bottom().saturating_sub(1), "enter chooses the provider for this role · esc back", on(CLAY, RAISED));
}

fn plugins_dialog(f: &mut Frame, app: &App, area: Rect, sel: usize, scroll: usize, filter: &str, mode: u8) {
    let entries = &app.plugin_rows;
    let matches = app.plugin_matches(filter, mode);
    let inner = dialog(f, area, "Plugins", 110, 31);
    let commands: usize = entries.iter().map(|e| e.commands.len()).sum();
    let skills: usize = entries.iter().map(|e| e.skills.len()).sum();
    let servers: usize = entries.iter().map(|e| e.servers.len()).sum();
    let allowed = entries.iter().filter(|e| e.allow).count();
    let summary = format!("{} plugins · {} commands · {} skills · {} MCP servers · {} tool permissions allowed",
                          entries.len(), commands, skills, servers, allowed);
    put(f, inner, inner.x, inner.y, &clip(&summary, inner.width as usize), on(DIM, RAISED));
    let mode_name = match mode { 1 => "with commands", 2 => "with MCP servers", _ => "all" };
    let filter_line = if filter.is_empty() { format!("{} · {} shown · type to filter · tab changes the view", mode_name, matches.len()) } else { format!("{} · filter: {} · {} shown · backspace clears", mode_name, filter, matches.len()) };
    put(f, inner, inner.x, inner.y + 1, &clip(&filter_line, inner.width as usize), on(if filter.is_empty() { DIM } else { SEA }, RAISED));
    if entries.is_empty() {
        put(f, inner, inner.x, inner.y + 2, "No plugin registered yet.", on(PAPER, RAISED).add_modifier(Modifier::BOLD));
        put(f, inner, inner.x, inner.y + 4, "/plugins install LINK     clone a plugin from git or unpack a ZIP", on(ASH, RAISED));
        put(f, inner, inner.x, inner.y + 5, "/plugins import           take over the plugins of other harnesses", on(ASH, RAISED));
        put(f, inner, inner.x, inner.y + 6, "/plugins allow NAME       allow the tools of its MCP servers", on(ASH, RAISED));
        put(f, inner, inner.x, inner.bottom().saturating_sub(1), "esc closes", on(DIM, RAISED));
        return;
    }
    let list_w = 56u16.min(inner.width.saturating_sub(18));
    let rows = (inner.height as usize).saturating_sub(7).min(matches.len().max(1)).max(1);
    let first = scroll.min(matches.len().saturating_sub(rows));
    if matches.is_empty() {
        put(f, inner, inner.x, inner.y + 3, &format!("No plugin or command matches \"{}\"", filter), on(CLAY, RAISED));
    }
    for (i, index) in matches.iter().skip(first).take(rows).enumerate() {
        let Some(entry) = entries.get(*index) else { continue };
        let y = inner.y + 3 + i as u16;
        let chosen = first + i == sel.min(matches.len().saturating_sub(1));
        let bg = if chosen { NIGHT } else { RAISED };
        fill(f, Rect::new(inner.x.saturating_sub(1), y, inner.width + 2, 1), bg);
        put(f, inner, inner.x, y, if entry.allow { "●" } else { "○" }, on(if entry.allow { SEA } else { CLAY }, bg));
        put(f, inner, inner.x + 2, y, &clip(&entry.name, 28), on(PAPER, bg).add_modifier(if chosen { Modifier::BOLD } else { Modifier::empty() }));
        let counts = format!("{}c {}s {}m", entry.commands.len(), entry.skills.len(), entry.servers.len());
        put(f, inner, inner.x + 32, y, &clip(&counts, list_w.saturating_sub(34) as usize), on(DIM, bg));
    }
    let Some(entry) = matches.get(sel.min(matches.len().saturating_sub(1))).and_then(|i| entries.get(*i)) else { return; };
    let x = inner.x + list_w + 2;
    let detail_w = inner.right().saturating_sub(x);
    if detail_w < 12 { return; }
    put(f, inner, x, inner.y + 3, &clip(&entry.name, detail_w as usize), on(PAPER, RAISED).add_modifier(Modifier::BOLD));
    put(f, inner, x, inner.y + 4, if entry.allow { "tools allowed" } else { "tools denied · A allows" }, on(if entry.allow { SEA } else { CLAY }, RAISED));
    put(f, inner, x, inner.y + 5, &clip(&crate::files::show(std::path::Path::new(&entry.dir)), detail_w as usize), on(DIM, RAISED));
    let mut y = inner.y + 7;
    let mut line = |y: &mut u16, text: &str, colour: art::Rgb| {
        if *y < inner.bottom().saturating_sub(2) {
            put(f, inner, x, *y, &clip(text, detail_w as usize), on(colour, RAISED));
            *y += 1;
        }
    };
    if entry.commands.is_empty() {
        line(&mut y, "commands: none (add commands/NAME.md to the plugin)", DIM);
    } else {
        line(&mut y, "commands · type /plugin:name", CLAY);
        for c in entry.commands.iter().take(5) {
            let hint = if c.argument_hint.is_empty() { String::new() } else { format!(" {}", c.argument_hint) };
            line(&mut y, &format!("/{}:{}{} · {}", entry.name, c.name, hint, c.description), ASH);
        }
        if entry.commands.len() > 5 { line(&mut y, &format!("… {} more · enter shows all", entry.commands.len() - 5), DIM); }
    }
    if !entry.skills.is_empty() {
        y = y.saturating_add(1);
        line(&mut y, "skills", CLAY);
        for s in entry.skills.iter().take(4) { line(&mut y, &format!("{} · {}", s.name, s.description), ASH); }
    }
    if !entry.servers.is_empty() {
        y = y.saturating_add(1);
        line(&mut y, "MCP servers", CLAY);
        for name in entry.servers.iter().take(4) { line(&mut y, name, ASH); }
    }
    put(f, inner, inner.x, inner.bottom().saturating_sub(1),
        "↑↓ · type filters · tab view · enter commands · A allow · U update · XX remove · esc", on(CLAY, RAISED));
}

fn plugin_commands_dialog(f: &mut Frame, app: &App, area: Rect, plugin: &str, sel: usize, scroll: usize) {
    let entry = app.plugin_rows.iter().find(|e| e.name == plugin);
    let commands = entry.map(|e| e.commands.as_slice()).unwrap_or(&[]);
    let inner = dialog(f, area, &format!("{} · commands", plugin), 88, commands.len().min(16) as u16 + 9);
    if commands.is_empty() {
        put(f, inner, inner.x, inner.y, "This plugin has no commands.", on(PAPER, RAISED).add_modifier(Modifier::BOLD));
        put(f, inner, inner.x, inner.y + 2, "A command is a Markdown file under commands/ in the plugin folder.", on(ASH, RAISED));
        put(f, inner, inner.x, inner.y + 3, "Skills and MCP servers still work; /plugins shows them.", on(ASH, RAISED));
        put(f, inner, inner.x, inner.bottom().saturating_sub(1), "esc goes back to the plugin list", on(CLAY, RAISED));
        return;
    }
    put(f, inner, inner.x, inner.y, &format!("{} commands · enter puts one into the input line", commands.len()), on(DIM, RAISED));
    let rows = (inner.height as usize).saturating_sub(5).min(commands.len()).max(1);
    let first = scroll.min(commands.len().saturating_sub(rows));
    for (i, command) in commands.iter().skip(first).take(rows).enumerate() {
        let y = inner.y + 2 + i as u16;
        let chosen = first + i == sel.min(commands.len().saturating_sub(1));
        let bg = if chosen { NIGHT } else { RAISED };
        fill(f, Rect::new(inner.x.saturating_sub(1), y, inner.width + 2, 1), bg);
        let hint = if command.argument_hint.is_empty() { String::new() } else { format!(" {}", command.argument_hint) };
        let name = format!("/{}:{}{}", plugin, command.name, hint);
        put(f, inner, inner.x, y, &clip(&name, 34), on(if chosen { SEA } else { PAPER }, bg).add_modifier(Modifier::BOLD));
        put(f, inner, inner.x + 36, y, &clip(&command.description, inner.width.saturating_sub(38) as usize), on(if chosen { PAPER } else { ASH }, bg));
    }
    put(f, inner, inner.x, inner.bottom().saturating_sub(1), "↑↓ choose · enter inserts · esc back", on(CLAY, RAISED));
}

fn apps_dialog(f: &mut Frame, app: &App, area: Rect, sel: usize, scroll: usize) {
    let rows = &app.app_rows;
    let active = crate::apps::active(&app.ctx);
    let inner = dialog(f, area, "Apps", 108, if rows.is_empty() { 12 } else { rows.len().min(12) as u16 + 8 });
    let summary = if rows.is_empty() {
        "No app installed · the first app is the cluster view".to_string()
    } else {
        format!("{} app{} · {} · enter shows · o opens an HTML app in the browser", rows.len(),
                if rows.len() == 1 { "" } else { "s" },
                active.as_deref().map(|name| format!("active {}", name)).unwrap_or_else(|| "no active app".to_string()))
    };
    put(f, inner, inner.x, inner.y, &clip(&summary, inner.width as usize), on(DIM, RAISED));
    if rows.is_empty() {
        put(f, inner, inner.x, inner.y + 2, "/app install LINK|DIR|ZIP  check the scripts, show the rights, install after a yes", on(ASH, RAISED));
        put(f, inner, inner.x, inner.y + 3, "/app update NAME           pull a git app in a staging copy; replace it only after a yes", on(ASH, RAISED));
        put(f, inner, inner.x, inner.y + 4, "apps live in .penelope/apps/NAME/; the manifest is .penelope-app.json or app.json", on(ASH, RAISED));
        put(f, inner, inner.x, inner.y + 5, "the first app is the cluster view: \"view\": \"clusters\"", on(ASH, RAISED));
        put(f, inner, inner.x, inner.bottom().saturating_sub(1), "esc closes · f7 opens this list", on(DIM, RAISED));
        return;
    }
    let visible = (inner.height as usize).saturating_sub(4).min(rows.len()).max(1);
    let first = scroll.min(rows.len().saturating_sub(visible));
    for (i, entry) in rows.iter().skip(first).take(visible).enumerate() {
        let y = inner.y + 2 + i as u16;
        let chosen = first + i == sel.min(rows.len().saturating_sub(1));
        let bg = if chosen { NIGHT } else { RAISED };
        fill(f, Rect::new(inner.x.saturating_sub(1), y, inner.width + 2, 1), bg);
        let on_app = active.as_deref() == Some(entry.name.as_str());
        put(f, inner, inner.x, y, if on_app { "●" } else { "○" }, on(if on_app { SEA } else { CLAY }, bg));
        put(f, inner, inner.x + 2, y, &clip(&entry.name, 22), on(PAPER, bg).add_modifier(if chosen { Modifier::BOLD } else { Modifier::empty() }));
        put(f, inner, inner.x + 26, y, &clip(&entry.version, 9), on(DIM, bg));
        put(f, inner, inner.x + 37, y, &clip(&entry.view.name(), 22), on(ASH, bg));
        put(f, inner, inner.x + 61, y, &clip(&format!("rights {}", entry.rights.summary()), inner.width.saturating_sub(62) as usize), on(DIM, bg));
    }
    if let Some(entry) = rows.get(sel.min(rows.len().saturating_sub(1))) {
        let note = if entry.description.is_empty() { entry.source.clone() } else { entry.description.clone() };
        let note = if note.is_empty() { crate::files::show(&entry.dir) } else { note };
        put(f, inner, inner.x, inner.bottom().saturating_sub(2), &clip(&note, inner.width as usize), on(ASH, RAISED));
    }
    put(f, inner, inner.x, inner.bottom().saturating_sub(1),
        "↑↓ · enter shows · o browser · a on/off · xx remove · esc", on(CLAY, RAISED));
}

fn app_install_dialog(f: &mut Frame, area: Rect, prepared: &crate::apps::Prepared) {
    let update = prepared.update;
    let inner = dialog(f, area, if update { "Update app" } else { "Install app" }, 96, 24);
    let intro = if update {
        format!("The git app was pulled and checked in a staging copy. The installed folder .penelope/apps/{} stays untouched before your yes.", prepared.app.name)
    } else {
        format!("The local install check is done. Nothing is installed under .penelope/apps/{} before your yes.", prepared.app.name)
    };
    put(f, inner, inner.x, inner.y, &clip(&intro, inner.width as usize), on(PAPER, RAISED).add_modifier(Modifier::BOLD));
    let report = prepared.report();
    let lines: Vec<&str> = report.lines().collect();
    let room = inner.height.saturating_sub(6) as usize;
    for (i, line) in lines.iter().take(room).enumerate() {
        let colour = if line.starts_with("rights:") { CLAY } else if line.starts_with("check:") { SEA } else { ASH };
        put(f, inner, inner.x, inner.y + 2 + i as u16, &clip(line, inner.width as usize), on(colour, RAISED));
    }
    put(f, inner, inner.x, inner.bottom().saturating_sub(2), "A yes stores the checked copy and shows the rights in /app list.", on(DIM, RAISED));
    put(f, inner, inner.x, inner.bottom().saturating_sub(1),
        if update { "y or enter updates · n or esc cancels and removes the checked copy" }
        else { "y or enter installs · n or esc cancels and removes the checked copy" }, on(CLAY, RAISED));
}

fn api_key_dialog(f: &mut Frame, app: &App, area: Rect, label: &str, input: &[char], cursor: usize, error: &str) {
    let _ = app;
    let inner = dialog(f, area, "API key", 66, 11);
    put(f, inner, inner.x, inner.y, &format!("Paste the key for {}.", clip(label, 44)), on(PAPER, RAISED));
    let masked = if input.is_empty() {
        "the first 4 characters are shown, the rest stays masked".to_string()
    } else {
        let first: String = input.iter().take(4).collect();
        let rest = input.len().saturating_sub(4).min(32);
        format!("{}{}", first, "•".repeat(rest.max(1)))
    };
    put(f, inner, inner.x, inner.y + 2, &masked, on(if input.is_empty() { DIM } else { PAPER }, RAISED));
    let at = (cursor.min(input.len()).min(4)) as u16;
    f.set_cursor_position(Position::new(inner.x + at, inner.y + 2));
    if !error.is_empty() {
        put(f, inner, inner.x, inner.y + 4, &clip(error, inner.width as usize), on(RED, RAISED));
    }
    put(f, inner, inner.x, inner.bottom().saturating_sub(1), "enter store · ctrl+u clear · esc cancel", on(DIM, RAISED));
}

fn overlay(f: &mut Frame, app: &App, area: Rect) {
    if !matches!(app.overlay, Overlay::None) {
        // A modal owns the screen. Clear removes the glyphs; fill paints the ground.
        f.render_widget(Clear, area);
        fill(f, area, INK);
    }
    match &app.overlay {
        Overlay::Providers { sel, target, scroll } => providers_dialog(f, app, area, *sel, target, *scroll),
        Overlay::Roles { sel } => roles_dialog(f, app, area, *sel),
        Overlay::Plugins { sel, scroll, filter, mode } => plugins_dialog(f, app, area, *sel, *scroll, filter, *mode),
        Overlay::PluginCommands { plugin, sel, scroll } => plugin_commands_dialog(f, app, area, plugin, *sel, *scroll),
        Overlay::Apps { sel, scroll } => apps_dialog(f, app, area, *sel, *scroll),
        Overlay::AppInstall { prepared } => app_install_dialog(f, area, prepared),
        Overlay::FolderTrust { path, lines, .. } => {
            let inner = dialog(f, area, "Enter folder", 96, 20);
            put(f, inner, inner.x, inner.y, &clip(&format!("{} has its own settings. They would apply:", tilde(&path.display().to_string())), inner.width as usize), on(PAPER, RAISED).add_modifier(Modifier::BOLD));
            let room = inner.height.saturating_sub(5) as usize;
            for (i, line) in lines.iter().take(room).enumerate() {
                put(f, inner, inner.x, inner.y + 2 + i as u16, &clip(line, inner.width as usize), on(ASH, RAISED));
            }
            put(f, inner, inner.x, inner.bottom().saturating_sub(1), "y or enter loads them and enters · n or esc stays", on(CLAY, RAISED));
        }
        Overlay::ApiKey { label, input, cursor, error, .. } => api_key_dialog(f, app, area, label, input, *cursor, error),
        Overlay::None => {}
        Overlay::Palette { query, sel } => {
            let matches = App::palette_matches(query);
            let inner = dialog(f, area, "Commands", 64, matches.len() as u16 + 7);
            put(f, inner, inner.x, inner.y, ">", on(CLAY, RAISED));
            put(f, inner, inner.x + 2, inner.y, if query.is_empty() { "type to search" } else { query }, on(if query.is_empty() { DIM } else { PAPER }, RAISED));
            f.set_cursor_position(Position::new((inner.x + 2 + width(query) as u16).min(inner.right().saturating_sub(1)), inner.y));
            let rows = inner.height.saturating_sub(2) as usize;
            let first = (sel + 1).saturating_sub(rows);
            for (i, cmd) in matches.iter().skip(first).take(rows).enumerate() {
                let y = inner.y + 2 + i as u16;
                let chosen = first + i == *sel;
                let (bg, name, help) = if chosen { (CLAY, INK, INK) } else { (RAISED, CLAY, ASH) };
                let row = Rect::new(inner.x.saturating_sub(1), y, inner.width + 2, 1);
                fill(f, row, bg);
                put(f, row, inner.x, y, cmd.name, on(name, bg).add_modifier(Modifier::BOLD));
                put(f, row, inner.x + 11, y, &clip(cmd.help, (inner.width as usize).saturating_sub(13 + cmd.key.len())), on(help, bg));
                put(f, row, inner.right().saturating_sub(cmd.key.len() as u16), y, cmd.key, on(if chosen { INK } else { DIM }, bg));
            }
        }
        Overlay::Models { sel, target, filter } => {
            let rows = app.model_rows_filtered(target, filter);
            // display lines: a header per provider group, then its rows; headers carry no row index
            let mut lines: Vec<(Option<usize>, String, usize)> = Vec::new();
            for (provider, first, n) in super::row_groups(&rows) {
                lines.push((None, provider, n));
                for i in first..first + n { lines.push((Some(i), String::new(), 0)); }
            }
            let shown = 20usize;
            let inner = dialog(f, area, &format!("Models · {}", target_label(target)), 84, lines.len().min(shown) as u16 + 10);
            let current = target_value(app, target)["model"].as_str().unwrap_or("?").to_string();
            let current_levels = app.model_efforts(target, &current);
            let current_effort = if current_levels.is_empty() { "no effort".to_string() }
                else { target_value(app, target)["effort"].as_str().unwrap_or("high").to_string() };
            let provider = app.provider_name(target);
            if !app.model_source.is_empty() {
                put(f, inner, inner.x, inner.y, &clip(&app.model_source, inner.width as usize), on(DIM, RAISED));
            }
            put(f, inner, inner.x, inner.y + 1, &clip(&format!("filter: {}▏", filter), inner.width as usize), on(if filter.is_empty() { DIM } else { PAPER }, RAISED));
            let at = (*sel).min(rows.len().saturating_sub(1));
            let disp_sel = lines.iter().position(|(i, _, _)| *i == Some(at)).unwrap_or(0);
            let first = if lines.len() > shown { disp_sel.saturating_sub(shown - 1) } else { 0 };
            if rows.is_empty() {
                put(f, inner, inner.x, inner.y + 2, "no model matches the filter", on(ASH, RAISED));
            }
            for (d, (index, group, count)) in lines.iter().enumerate().skip(first).take(shown) {
                let y = inner.y + 2 + (d - first) as u16;
                let Some(i) = index else {
                    fill(f, Rect::new(inner.x.saturating_sub(1), y, inner.width + 2, 1), RAISED);
                    let head = format!("{} · {} models", super::provider_label(group), count);
                    put(f, inner, inner.x, y, &clip(&head, inner.width as usize), bold(CLAY).bg(col(RAISED)));
                    continue;
                };
                let (row_provider, name) = &rows[*i];
                let chosen = *i == at;
                let bg = if chosen { NIGHT } else { RAISED };
                let is_current = *name == current && *row_provider == provider;
                fill(f, Rect::new(inner.x.saturating_sub(1), y, inner.width + 2, 1), bg);
                put(f, inner, inner.x + 1, y, if is_current { "●" } else { "○" }, on(if is_current { SEA } else { DIM }, bg));
                put(f, inner, inner.x + 3, y, &clip(name, 34), on(PAPER, bg));
                let effort = app.provider_efforts(row_provider, name).join(" ");
                put(f, inner, inner.x + 59, y, &clip(if effort.is_empty() { "no effort" } else { &effort }, 24), on(DIM, bg));
            }
            let (selected_provider, selected) = rows.get(*sel).cloned().unwrap_or_default();
            let selected_label = app.model_cache.get(&selected_provider).and_then(|list| list.iter().find(|m| m.id == selected)).map(|m| m.label.clone())
                .or_else(|| crate::providers::models(&selected_provider).iter().find(|m| m.id == selected).map(|m| m.label.to_string()))
                .unwrap_or_else(|| selected.clone());
            let note = format!("{} · current {} / effort {}", selected_label, current, current_effort);
            put(f, inner, inner.x, inner.bottom().saturating_sub(2), &clip(&note, inner.width as usize), on(CLAY, RAISED));
            put(f, inner, inner.x, inner.bottom().saturating_sub(1), "type filters · ↑↓ · ←→ provider · tab main/weak · ^r roles · ^e effort · esc", on(DIM, RAISED));
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
        Overlay::Question { question, options, sel, .. } => {
            let w = 76u16.min(area.width.saturating_sub(4)).max(10);
            let rows = wrap_plain(question, (w as usize).saturating_sub(6).max(1));
            let inner = dialog(f, area, "The main model asks", w, (rows.len() + options.len()) as u16 + 8);
            let mut y = inner.y;
            for row in &rows {
                put(f, inner, inner.x, y, row, on(PAPER, RAISED));
                y += 1;
            }
            y += 1;
            for (i, option) in options.iter().enumerate() {
                let chosen = i == *sel;
                let (bg, colour) = if chosen { (CLAY, INK) } else { (RAISED, PAPER) };
                let row = Rect::new(inner.x.saturating_sub(1), y, inner.width + 2, 1);
                fill(f, row, bg);
                put(f, row, inner.x, y, &format!("{}  {}", i + 1, clip(option, (inner.width as usize).saturating_sub(5))), on(colour, bg));
                y += 1;
            }
            put(f, inner, inner.x, y + 1, "enter or the number chooses · esc closes without an answer", on(DIM, RAISED));
        }
        Overlay::Ask { dialog, .. } => super::ask::draw(f, area, dialog, crate::ask::now_secs()),
        Overlay::Sessions { sel } => {
            let all = crate::sessions::all(&app.ctx);
            let inner = dialog(f, area, "Sessions", 92, all.len().min(16) as u16 + 7);
            put(f, inner, inner.x, inner.y, "enter opens · entf deletes · the newest is on top", on(ASH, RAISED));
            let rows = inner.height.saturating_sub(2) as usize;
            let first = (sel + 1).saturating_sub(rows);
            for (i, e) in all.iter().rev().skip(first).take(rows).enumerate() {
                let y = inner.y + 2 + i as u16;
                let chosen = first + i == *sel;
                let open = app.session_id.as_deref() == e["id"].as_str();
                let (bg, main, side) = if chosen { (CLAY, INK, INK) } else { (RAISED, PAPER, ASH) };
                let row = Rect::new(inner.x.saturating_sub(1), y, inner.width + 2, 1);
                fill(f, row, bg);
                let meta = format!("{}  {:>3} turns", crate::sessions::stamp(e["last"].as_u64().unwrap_or(0)), e["turns"].as_u64().unwrap_or(0));
                put(f, row, inner.x, y, &format!("{:>2}", all.len() - first - i), on(side, bg));
                put(f, row, inner.x + 4, y, &meta, on(side, bg));
                put(f, row, inner.x + 6 + width(&meta) as u16, y, &clip(e["title"].as_str().unwrap_or(""), (inner.width as usize).saturating_sub(width(&meta) + 14)),
                    on(main, bg).add_modifier(if open { Modifier::BOLD } else { Modifier::empty() }));
                if open {
                    put(f, row, inner.right().saturating_sub(4), y, "open", on(side, bg));
                }
            }
            if all.is_empty() {
                put(f, inner, inner.x, inner.y + 2, "no session yet. A session is stored after its first answer.", on(DIM, RAISED));
            }
        }
        Overlay::Graph(view) => super::graph::draw(f, area, view),
        Overlay::Brain { scroll } => {
            let body = crate::brain::ascii(&app.ctx);
            let lines: Vec<&str> = body.lines().collect();
            let widest = lines.iter().map(|l| width(l)).max().unwrap_or(40) as u16;
            let inner = dialog(f, area, "The brain · the memory map as a graph", (widest + 6).clamp(56, 104), lines.len() as u16 + 7);
            let h = inner.height as usize;
            let start = (*scroll).min(lines.len().saturating_sub(h));
            for (i, line) in lines.iter().skip(start).take(h).enumerate() {
                let colour = if line.contains('●') { CLAY } else if line.contains('○') { DIM }
                    else if line.contains("waypoints") { SEA } else if line.starts_with("The same") { SEA } else { ASH };
                put(f, inner, inner.x, inner.y + i as u16, &clip(line, inner.width as usize), on(colour, RAISED));
            }
            put(f, inner, inner.x, inner.bottom(), "w writes an Obsidian vault · up/down scroll · a full note: penelope memory go", on(DIM, RAISED));
        }
        Overlay::Approve { command, cwd, file, diff, .. } => {
            let widest = diff.iter().map(|l| width(l)).max().unwrap_or(0) as u16;
            let w = (widest + 6).clamp(76, 104).min(area.width.saturating_sub(4)).max(10);
            let command = &tilde_all(command);
            let rows = wrap_plain(command, (w as usize).saturating_sub(8).max(1));
            let inner = dialog(f, area, if *file { "Change this file?" } else { "Run this command?" }, w,
                               rows.len() as u16 + 10 + if diff.is_empty() { 0 } else { diff.len() as u16 + 1 });
            put(f, inner, inner.x, inner.y, if *file { "The main model wants to change a file. Penelope waits for an answer." }
                else { "The command is outside the safe list. Penelope waits for an answer." }, on(ASH, RAISED));
            let mut y = inner.y + 2;
            let shown = rows.len().min(inner.height.saturating_sub(5) as usize);
            for row in rows.iter().take(shown) {
                fill(f, Rect::new(inner.x, y, inner.width, 1), INK);
                put(f, inner, inner.x + 1, y, if *file { "±" } else { "$" }, on(CLAY, INK));
                put(f, inner, inner.x + 3, y, row, on(PAPER, INK));
                y += 1;
            }
            // The changed lines: removed in red, added in clay. The dialog keeps 5 rows for its fixed lines, the diff gets the rest.
            let room = (inner.height as usize).saturating_sub(6 + shown);
            if !diff.is_empty() && room > 0 {
                y += 1;
                let fits = diff.len() <= room;
                for (i, line) in diff.iter().take(room).enumerate() {
                    fill(f, Rect::new(inner.x, y, inner.width, 1), INK);
                    let (text, colour) = if !fits && i + 1 == room { (format!("        {} more changed lines", diff.len() - i), DIM) } else {
                        (tilde_all(line), match line.chars().next() { Some('-') => RED, Some('+') => CLAY, _ => DIM })
                    };
                    put(f, inner, inner.x + 1, y, &text, on(colour, INK));
                    y += 1;
                }
            }
            if !cwd.is_empty() {
                put(f, inner, inner.x, y, &format!("in {}", clip(&tilde(cwd), (inner.width as usize).saturating_sub(4))), on(DIM, RAISED));
            }
            y += 2;
            let mut x = inner.x;
            for (key, text) in [("y", if *file { "change it" } else { "run it once" }), ("a", "allow all in this session"), ("n", "refuse")] {
                put(f, inner, x, y, &format!(" {} ", key), on(INK, CLAY).add_modifier(Modifier::BOLD));
                put(f, inner, x + 4, y, text, on(PAPER, RAISED));
                x += 4 + text.len() as u16 + 3;
            }
        }
    }
}
