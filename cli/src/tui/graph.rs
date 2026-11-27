//! `/graph`: the knowledge graph of the memory as an overlay.
//!
//! The list on the left follows the search as you type. The right side shows the selected node: its text, its
//! neighbours (in and out) and a compact picture with the node in the middle and up to 8 neighbours around it. Enter on
//! the list opens a node. In the walk, Up and Down choose a neighbour, Enter goes there and Backspace goes one step
//! back. The graph is built once when the overlay opens (`crate::graph`); nothing here writes or calls a model.

use super::art::{ASH, CLAY, DIM, GOLD, INK, NIGHT, PAPER, RAISED, SEA};
use super::text::{clip, wrap_plain};
use super::view::{bold, dialog, fill, on, put};
use crate::config::Ctx;
use crate::graph::{now, Graph, Kind, Neighbour, Node, Stats};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::Frame;

const DAY: u64 = 86_400;

pub struct GraphView {
    graph: Graph,
    stats: Stats,
    now: u64,
    pub query: String,
    /// The ids that match the query, in list order.
    rows: Vec<String>,
    pub sel: usize,
    /// True while walking: `trail` holds the nodes visited, the last one is shown.
    pub walking: bool,
    pub trail: Vec<String>,
    pub nb: usize,
}

impl GraphView {
    pub fn open(ctx: &Ctx) -> GraphView {
        GraphView::from_graph(Graph::build(ctx), now())
    }

    pub fn from_graph(graph: Graph, now: u64) -> GraphView {
        let stats = graph.stats(now);
        let mut v = GraphView { graph, stats, now, query: String::new(), rows: Vec::new(), sel: 0, walking: false, trail: Vec::new(), nb: 0 };
        v.refresh();
        v
    }

    fn refresh(&mut self) {
        let (hits, _) = self.graph.page(&self.query, 0, usize::MAX);
        self.rows = hits.iter().map(|n| n.id.clone()).collect();
        self.sel = self.sel.min(self.rows.len().saturating_sub(1));
    }

    /// The node on show: the last one of the walk, or the one under the cursor of the list.
    pub fn current(&self) -> Option<&Node> {
        if self.walking {
            self.trail.last().and_then(|id| self.graph.get(id))
        } else {
            self.rows.get(self.sel).and_then(|id| self.graph.get(id))
        }
    }

    pub fn neighbours(&self) -> Vec<Neighbour> {
        self.current().map(|n| self.graph.neighbours(&n.id)).unwrap_or_default()
    }

    /// True when the overlay should close.
    pub fn key(&mut self, key: KeyEvent) -> bool {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc if self.walking => {
                self.walking = false;
                self.trail.clear();
            }
            KeyCode::Esc if !self.query.is_empty() => {
                self.query.clear();
                self.refresh();
            }
            KeyCode::Esc => return true,
            KeyCode::Enter if self.walking => {
                if let Some(next) = self.neighbours().get(self.nb) {
                    self.trail.push(next.id.clone());
                    self.nb = 0;
                }
            }
            KeyCode::Enter => {
                if let Some(id) = self.rows.get(self.sel) {
                    self.trail = vec![id.clone()];
                    self.walking = true;
                    self.nb = 0;
                }
            }
            KeyCode::Backspace if self.walking => {
                self.trail.pop();
                self.nb = 0;
                if self.trail.is_empty() {
                    self.walking = false;
                }
            }
            KeyCode::Backspace => {
                self.query.pop();
                self.refresh();
            }
            KeyCode::Up if self.walking => self.nb = self.nb.saturating_sub(1),
            KeyCode::Down if self.walking => self.nb = (self.nb + 1).min(self.neighbours().len().saturating_sub(1)),
            KeyCode::Up => self.sel = self.sel.saturating_sub(1),
            KeyCode::Down => self.sel = (self.sel + 1).min(self.rows.len().saturating_sub(1)),
            KeyCode::PageUp if !self.walking => self.sel = self.sel.saturating_sub(10),
            KeyCode::PageDown if !self.walking => self.sel = (self.sel + 10).min(self.rows.len().saturating_sub(1)),
            KeyCode::Char(c) if !ctrl && !key.modifiers.contains(KeyModifiers::ALT) => {
                // typing in the walk goes back to the list and starts a search
                self.walking = false;
                self.trail.clear();
                self.query.push(c);
                self.sel = 0;
                self.refresh();
            }
            _ => {}
        }
        false
    }
}

fn kind_colour(kind: Kind) -> super::art::Rgb {
    match kind {
        Kind::Note => PAPER,
        Kind::Rule => CLAY,
        Kind::ClusterEntry => SEA,
        Kind::File => GOLD,
        Kind::Link => ASH,
    }
}

fn ago(now: u64, t: u64) -> String {
    match now.saturating_sub(t) / DAY {
        0 => "today".to_string(),
        1 => "yesterday".to_string(),
        d => format!("{} days ago", d),
    }
}

fn label(graph: &Graph, id: &str, max: usize) -> String {
    clip(&graph.get(id).map(|n| n.title.clone()).unwrap_or_else(|| id.to_string()), max)
}

/// The node in the middle and up to 8 neighbours around it: three above, one left, one right, three below. Arrows
/// show the direction of the edge: `out` is true when the edge leaves the middle node.
pub fn star(center: &str, around: &[(String, bool)], cell: usize) -> Vec<String> {
    let c = cell.max(8);
    let mid = |s: &str| -> String {
        let s = clip(s, c);
        let n = c.saturating_sub(super::text::width(&s));
        format!("{}{}{}", " ".repeat(n / 2), s, " ".repeat(n - n / 2))
    };
    let row = |idx: [usize; 3], arrow: &dyn Fn(bool) -> &'static str, labels: bool| -> String {
        idx.iter().map(|&i| match around.get(i) {
            Some((t, out)) => mid(if labels { t } else { arrow(*out) }),
            None => mid(""),
        }).collect::<Vec<_>>().join(" ").trim_end().to_string()
    };
    let (top, bottom) = ([0usize, 1, 2], [5usize, 6, 7]);
    let mut out = Vec::new();
    if top.iter().any(|&i| around.get(i).is_some()) {
        out.push(row(top, &|_| "", true));
        out.push(row(top, &|o| if o { "▲" } else { "▼" }, false));
    }
    let left = around.get(3).map(|(t, o)| format!("{} {}", clip(t, c), if *o { "◀──" } else { "──▶" })).unwrap_or_default();
    let right = around.get(4).map(|(t, o)| format!("{} {}", if *o { "──▶" } else { "◀──" }, clip(t, c))).unwrap_or_default();
    out.push(format!("{:>w$} [ {} ] {}", left, clip(center, c), right, w = c + 5).trim_end().to_string());
    if bottom.iter().any(|&i| around.get(i).is_some()) {
        out.push(row(bottom, &|o| if o { "▼" } else { "▲" }, false));
        out.push(row(bottom, &|_| "", true));
    }
    out
}

pub fn draw(f: &mut Frame, area: Rect, v: &GraphView) {
    let inner = dialog(f, area, "Graph · the knowledge graph of the memory", 120, 36);
    let s = &v.stats;
    let kinds: Vec<String> = s.by_kind.iter().map(|(k, n)| format!("{} {}", n, k)).collect();
    let cap = if s.truncated { format!(" · capped, {} exist", v.graph.total) } else { String::new() };
    put(f, inner, inner.x, inner.y, &clip(&format!("{} nodes · {} edges · {} · +{} in 7 days{}", s.nodes, s.edges, kinds.join(" · "), s.added_7d, cap), inner.width as usize), on(SEA, RAISED));
    put(f, inner, inner.x, inner.y + 1, &clip(&format!("search: {}▏  {} of {}", v.query, v.rows.len(), s.nodes), inner.width as usize), on(if v.query.is_empty() { DIM } else { PAPER }, RAISED));
    let top = inner.y + 3;
    let rows = inner.bottom().saturating_sub(top + 2) as usize;
    let lw = 38u16.min(inner.width / 2);
    // the list
    let first = if v.sel >= rows { v.sel + 1 - rows } else { 0 };
    for (i, id) in v.rows.iter().skip(first).take(rows).enumerate() {
        let Some(n) = v.graph.get(id) else { continue };
        let y = top + i as u16;
        let chosen = first + i == v.sel;
        let on_trail = v.walking && v.trail.last() == Some(id);
        let bg = if chosen && !v.walking { CLAY } else if on_trail { NIGHT } else { RAISED };
        let fg = if chosen && !v.walking { INK } else { kind_colour(n.kind) };
        fill(f, Rect::new(inner.x.saturating_sub(1), y, lw + 1, 1), bg);
        put(f, inner, inner.x, y, &clip(&format!("{} {}", &n.kind.name()[..1], n.title), lw as usize - 1), on(fg, bg));
    }
    if v.rows.is_empty() {
        put(f, inner, inner.x, top, "no node matches", on(DIM, RAISED));
    }
    // the node
    let x = inner.x + lw + 2;
    let w = inner.right().saturating_sub(x) as usize;
    let mut y = top;
    let Some(n) = v.current() else {
        put(f, inner, x, y, "the memory holds no node yet. /note stores the first one.", on(DIM, RAISED));
        footer(f, inner);
        return;
    };
    put(f, inner, x, y, &clip(&n.title, w), bold(kind_colour(n.kind)).bg(super::art::col(RAISED)));
    y += 1;
    let meta = format!("{} · {} · L{} · used {}x · changed {} · {}", n.kind.name(), n.scope.name(), n.level, n.uses, ago(v.now, n.changed), n.path);
    put(f, inner, x, y, &clip(&meta, w), on(DIM, RAISED));
    y += 1;
    for line in wrap_plain(if n.text.is_empty() { "(no text, a signpost or a reference)" } else { &n.text }, w.max(10)).into_iter().take(5) {
        y += 1;
        put(f, inner, x, y, &clip(&line, w), on(PAPER, RAISED));
    }
    y += 2;
    let nbs = v.neighbours();
    put(f, inner, x, y, &format!("neighbours: {} ({} out, {} in)", nbs.len(), nbs.iter().filter(|n| n.outgoing).count(), nbs.iter().filter(|n| !n.outgoing).count()), on(SEA, RAISED));
    y += 1;
    let room = (inner.bottom().saturating_sub(y + 8) as usize).clamp(3, 10);
    let start = if v.nb >= room { v.nb + 1 - room } else { 0 };
    for (i, nb) in nbs.iter().skip(start).take(room).enumerate() {
        let chosen = v.walking && start + i == v.nb;
        let bg = if chosen { CLAY } else { RAISED };
        let colour = if chosen { INK } else { v.graph.get(&nb.id).map(|n| kind_colour(n.kind)).unwrap_or(ASH) };
        let row = Rect::new(x.saturating_sub(1), y, w as u16 + 1, 1);
        fill(f, row, bg);
        put(f, inner, x, y, &clip(&format!("{} {:<8} {}", if nb.outgoing { "→" } else { "←" }, nb.kind.name(), label(&v.graph, &nb.id, w.saturating_sub(12))), w), on(colour, bg));
        y += 1;
    }
    if nbs.len() > room {
        put(f, inner, x, y, &format!("... {} more", nbs.len() - room), on(DIM, RAISED));
        y += 1;
    }
    y += 1;
    let around: Vec<(String, bool)> = nbs.iter().take(8).map(|nb| (label(&v.graph, &nb.id, 12), nb.outgoing)).collect();
    for line in star(&clip(&n.title, 14), &around, 14) {
        put(f, inner, x, y, &clip(&line, w), on(ASH, RAISED));
        y += 1;
    }
    footer(f, inner);
}

fn footer(f: &mut Frame, inner: Rect) {
    put(f, inner, inner.x, inner.bottom().saturating_sub(1), "type searches · up/down · enter opens, then walks to a neighbour · backspace back · esc", on(DIM, RAISED));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::tests::filled;
    use std::fs;

    fn press(v: &mut GraphView, code: KeyCode) -> bool {
        v.key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn typed(v: &mut GraphView, text: &str) {
        for c in text.chars() {
            press(v, KeyCode::Char(c));
        }
    }

    #[test]
    fn search_opens_a_node_and_enter_and_backspace_walk_the_graph() {
        let (dir, ctx) = filled("tui-walk");
        let mut v = GraphView::open(&ctx);
        let all = v.rows.len();
        assert!(all > 8);
        typed(&mut v, "anyhow");
        assert_eq!(v.rows, vec!["p:rust/errors".to_string()], "search as you type narrows the list");
        assert_eq!(v.current().unwrap().title, "errors");
        press(&mut v, KeyCode::Enter);
        assert!(v.walking && v.trail == ["p:rust/errors"]);
        let nbs = v.neighbours();
        assert!(nbs.len() >= 4, "{:?}", nbs);
        // down to the first neighbour that is the build waypoint, then walk there
        let at = nbs.iter().position(|n| n.id == "p:rust/build").unwrap();
        for _ in 0..at {
            press(&mut v, KeyCode::Down);
        }
        press(&mut v, KeyCode::Enter);
        assert_eq!(v.current().unwrap().id, "p:rust/build");
        assert_eq!(v.trail.len(), 2);
        // from build to the shared file node, and back with backspace
        let file = v.neighbours().iter().position(|n| n.id == "f:src/main.rs").unwrap();
        for _ in 0..file {
            press(&mut v, KeyCode::Down);
        }
        press(&mut v, KeyCode::Enter);
        assert_eq!(v.current().unwrap().kind, Kind::File);
        press(&mut v, KeyCode::Backspace);
        assert_eq!(v.current().unwrap().id, "p:rust/build");
        press(&mut v, KeyCode::Backspace);
        assert_eq!(v.current().unwrap().id, "p:rust/errors");
        press(&mut v, KeyCode::Backspace);
        assert!(!v.walking, "the last step back returns to the list");
        assert_eq!(v.query, "anyhow");
        // backspace edits the search, esc clears it, a second esc closes
        press(&mut v, KeyCode::Backspace);
        assert_eq!(v.query, "anyho");
        assert!(!press(&mut v, KeyCode::Esc) && v.query.is_empty() && v.rows.len() == all);
        assert!(press(&mut v, KeyCode::Esc));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_list_moves_pages_and_stays_inside() {
        let (dir, ctx) = filled("tui-list");
        let mut v = GraphView::open(&ctx);
        press(&mut v, KeyCode::Up);
        assert_eq!(v.sel, 0);
        press(&mut v, KeyCode::PageDown);
        assert_eq!(v.sel, 10.min(v.rows.len() - 1));
        for _ in 0..100 {
            press(&mut v, KeyCode::Down);
        }
        assert_eq!(v.sel, v.rows.len() - 1);
        typed(&mut v, "zzzzzz");
        assert!(v.rows.is_empty() && v.current().is_none());
        assert!(!press(&mut v, KeyCode::Enter) && !v.walking, "enter on an empty list does nothing");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_star_shows_the_node_in_the_middle_and_at_most_eight_neighbours() {
        let around: Vec<(String, bool)> = (0..8).map(|i| (format!("n{}", i), i % 2 == 0)).collect();
        let lines = star("centre", &around, 10);
        let text = lines.join("\n");
        assert!(text.contains("[ centre ]"), "{}", text);
        for i in 0..8 {
            assert!(text.contains(&format!("n{}", i)), "n{} missing in\n{}", i, text);
        }
        assert!(text.contains("◀──") || text.contains("──▶"));
        assert_eq!(lines.len(), 5);
        let lone = star("solo", &[], 10);
        assert_eq!(lone.len(), 1);
        assert!(lone[0].contains("[ solo ]"));
        assert!(star("x", &[("a".into(), true)], 10).iter().any(|l| l.contains('a')));
    }

    #[test]
    fn the_overlay_draws_counts_growth_the_text_and_the_neighbours() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;
        let (dir, ctx) = filled("tui-draw");
        let mut app = super::super::App::new(ctx.clone());
        let mut v = GraphView::open(&ctx);
        typed(&mut v, "anyhow");
        press(&mut v, KeyCode::Enter);
        app.overlay = super::super::Overlay::Graph(Box::new(v));
        let mut t = Terminal::new(TestBackend::new(130, 40)).unwrap();
        let frame = t.draw(|f| super::super::view::draw(f, &mut app)).unwrap();
        let buf = frame.buffer;
        let screen: String = (0..40).map(|y| (0..130).map(|x| buf[(x, y)].symbol().to_string()).collect::<String>()).collect::<Vec<_>>().join("\n");
        for needle in ["knowledge graph", "nodes", "edges", "in 7 days", "search: anyhow", "Use anyhow in the binary", "neighbours:", "mention", "[ errors ]"] {
            assert!(screen.contains(needle), "{} missing in\n{}", needle, screen);
        }
        // a small terminal still draws
        let mut small = Terminal::new(TestBackend::new(60, 16)).unwrap();
        small.draw(|f| super::super::view::draw(f, &mut app)).unwrap();
        let _ = fs::remove_dir_all(&dir);
    }
}
