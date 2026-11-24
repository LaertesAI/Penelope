//! Text for the terminal interface: word wrap of styled runs, a small Markdown reader and the layout of the input box.

use super::art::{col, CLAY, ASH, NIGHT, PAPER, SEA};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

pub fn width(s: &str) -> usize {
    UnicodeWidthStr::width(s)
}

/// The text cut to `max` columns, with a dot row at the end when something was cut.
pub fn clip(s: &str, max: usize) -> String {
    if width(s) <= max {
        return s.to_string();
    }
    let mut out = String::new();
    let mut w = 0;
    for c in s.chars() {
        let cw = c.width().unwrap_or(0);
        if w + cw + 1 > max {
            break;
        }
        out.push(c);
        w += cw;
    }
    out.push('…');
    out
}

#[derive(Clone)]
pub struct Run {
    pub text: String,
    pub style: Style,
}

fn run(text: &str, style: Style) -> Run {
    Run { text: text.to_string(), style }
}

/// Greedy word wrap. A word longer than the line is cut at the line end.
pub fn wrap(runs: &[Run], max: usize) -> Vec<Vec<Run>> {
    let max = max.max(1);
    let mut lines: Vec<Vec<Run>> = vec![Vec::new()];
    let mut used = 0usize;
    for r in runs {
        let mut rest = r.text.as_str();
        while !rest.is_empty() {
            let space = rest.starts_with(' ');
            let end = rest.find(|c: char| (c == ' ') != space).unwrap_or(rest.len());
            let (token, tail) = rest.split_at(end);
            rest = tail;
            let w = width(token);
            if space {
                if used > 0 && used + w < max {
                    lines.last_mut().unwrap().push(run(token, r.style));
                    used += w;
                }
                continue;
            }
            if used > 0 && used + w > max {
                trim_end(lines.last_mut().unwrap());
                lines.push(Vec::new());
                used = 0;
            }
            if w <= max {
                lines.last_mut().unwrap().push(run(token, r.style));
                used += w;
                continue;
            }
            let mut piece = String::new();
            for c in token.chars() {
                let cw = c.width().unwrap_or(0);
                if used + cw > max {
                    lines.last_mut().unwrap().push(run(&piece, r.style));
                    piece.clear();
                    lines.push(Vec::new());
                    used = 0;
                }
                piece.push(c);
                used += cw;
            }
            if !piece.is_empty() {
                lines.last_mut().unwrap().push(run(&piece, r.style));
            }
        }
    }
    trim_end(lines.last_mut().unwrap());
    lines
}

fn trim_end(line: &mut Vec<Run>) {
    while line.last().map(|r| r.text.trim().is_empty()).unwrap_or(false) {
        line.pop();
    }
}

/// `code` in teal, **bold** in bold, the rest in the base style.
fn inline(text: &str, base: Style) -> Vec<Run> {
    let mut out = Vec::new();
    let mut plain = String::new();
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    let mut bold = false;
    let style = |bold: bool| if bold { base.add_modifier(Modifier::BOLD) } else { base };
    while i < chars.len() {
        if chars[i] == '`' {
            if let Some(len) = chars[i + 1..].iter().position(|c| *c == '`') {
                if !plain.is_empty() {
                    out.push(run(&plain, style(bold)));
                    plain.clear();
                }
                let code: String = chars[i + 1..i + 1 + len].iter().collect();
                out.push(run(&code, Style::default().fg(col(SEA))));
                i += len + 2;
                continue;
            }
        }
        if chars[i] == '*' && chars.get(i + 1) == Some(&'*') {
            if !plain.is_empty() {
                out.push(run(&plain, style(bold)));
                plain.clear();
            }
            bold = !bold;
            i += 2;
            continue;
        }
        plain.push(chars[i]);
        i += 1;
    }
    if !plain.is_empty() {
        out.push(run(&plain, style(bold)));
    }
    out
}

fn to_line(indent: &str, runs: Vec<Run>) -> Line<'static> {
    let mut spans = vec![Span::raw(indent.to_string())];
    spans.extend(runs.into_iter().map(|r| Span::styled(r.text, r.style)));
    Line::from(spans)
}

/// Markdown of an answer as lines of at most `max` columns: headings, lists, quotes, code blocks, inline code and bold.
pub fn markdown(text: &str, max: usize) -> Vec<Line<'static>> {
    let base = Style::default().fg(col(PAPER));
    let mut out: Vec<Line<'static>> = Vec::new();
    let mut code = false;
    for raw in text.lines() {
        let line = raw.trim_end();
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") {
            code = !code;
            continue;
        }
        if code {
            let style = Style::default().fg(col(PAPER)).bg(col(NIGHT));
            let inner = max.saturating_sub(2).max(1);
            let mut piece = String::new();
            let mut w = 0;
            let mut rows: Vec<String> = Vec::new();
            for c in line.replace('\t', "    ").chars() {
                let cw = c.width().unwrap_or(0);
                if w + cw > inner {
                    rows.push(std::mem::take(&mut piece));
                    w = 0;
                }
                piece.push(c);
                w += cw;
            }
            rows.push(piece);
            for r in rows {
                let pad = inner.saturating_sub(width(&r));
                out.push(Line::from(Span::styled(format!(" {}{} ", r, " ".repeat(pad)), style)));
            }
            continue;
        }
        if trimmed.is_empty() {
            out.push(Line::default());
            continue;
        }
        let lead = line.len() - trimmed.len();
        let hashes = trimmed.chars().take_while(|c| *c == '#').count();
        if (1..=6).contains(&hashes) && trimmed[hashes..].starts_with(' ') {
            let style = Style::default().fg(col(CLAY)).add_modifier(Modifier::BOLD);
            for l in wrap(&inline(trimmed[hashes..].trim(), style), max) {
                out.push(to_line("", l));
            }
            continue;
        }
        let (first, body) = if let Some(rest) = trimmed.strip_prefix("- ").or_else(|| trimmed.strip_prefix("* ")).or_else(|| trimmed.strip_prefix("+ ")) {
            (format!("{}• ", " ".repeat(lead.min(8))), rest)
        } else if let Some(rest) = trimmed.strip_prefix("> ") {
            ("▎ ".to_string(), rest)
        } else {
            let digits = trimmed.chars().take_while(|c| c.is_ascii_digit()).count();
            let after = &trimmed[digits..];
            if digits > 0 && digits <= 3 && (after.starts_with(". ") || after.starts_with(") ")) {
                (format!("{}{} ", " ".repeat(lead.min(8)), &trimmed[..digits + 1]), &after[2..])
            } else {
                (String::new(), trimmed)
            }
        };
        let quote = first.starts_with('▎');
        let body_style = if quote { Style::default().fg(col(ASH)) } else { base };
        let indent = " ".repeat(width(&first));
        for (i, l) in wrap(&inline(body, body_style), max.saturating_sub(width(&first))).into_iter().enumerate() {
            if i == 0 || quote {
                let mut spans = vec![Span::styled(first.clone(), Style::default().fg(col(CLAY)))];
                spans.extend(l.into_iter().map(|r| Span::styled(r.text, r.style)));
                out.push(Line::from(spans));
            } else {
                out.push(to_line(&indent, l));
            }
        }
    }
    while out.last().map(|l| l.spans.is_empty()).unwrap_or(false) {
        out.pop();
    }
    out
}

/// Plain text wrapped to `max` columns, one String per line. Line breaks of the text are kept.
pub fn wrap_plain(text: &str, max: usize) -> Vec<String> {
    let mut out = Vec::new();
    for line in text.lines() {
        if line.trim().is_empty() {
            out.push(String::new());
            continue;
        }
        for l in wrap(&[run(line, Style::default())], max) {
            out.push(l.into_iter().map(|r| r.text).collect());
        }
    }
    if out.is_empty() {
        out.push(String::new());
    }
    out
}

/// The input box: the characters broken into rows of `max` columns and the row and column of the cursor.
pub fn input_rows(chars: &[char], cursor: usize, max: usize) -> (Vec<String>, (usize, usize)) {
    let max = max.max(1);
    let mut rows = vec![String::new()];
    let mut w = 0usize;
    let mut at = (0usize, 0usize);
    for (i, c) in chars.iter().enumerate() {
        if i == cursor {
            at = (rows.len() - 1, w);
        }
        if *c == '\n' {
            rows.push(String::new());
            w = 0;
            continue;
        }
        let cw = c.width().unwrap_or(0);
        if w + cw > max {
            rows.push(String::new());
            w = 0;
            if i == cursor {
                at = (rows.len() - 1, 0);
            }
        }
        rows.last_mut().unwrap().push(*c);
        w += cw;
    }
    if cursor >= chars.len() {
        at = (rows.len() - 1, w);
        if w >= max {
            rows.push(String::new());
            at = (rows.len() - 1, 0);
        }
    }
    (rows, at)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat(lines: &[Vec<Run>]) -> Vec<String> {
        lines.iter().map(|l| l.iter().map(|r| r.text.as_str()).collect()).collect()
    }

    #[test]
    fn wraps_at_words() {
        let lines = wrap(&[run("one two three four", Style::default())], 9);
        assert_eq!(flat(&lines), vec!["one two", "three", "four"]);
    }

    #[test]
    fn cuts_a_long_word() {
        let lines = wrap(&[run("abcdefghij", Style::default())], 4);
        assert_eq!(flat(&lines), vec!["abcd", "efgh", "ij"]);
    }

    #[test]
    fn markdown_keeps_code_and_lists() {
        let lines = markdown("# Title\n\n- first item\n- second\n\n```\nlet a = 1;\n```\ntext with `code` and **bold**", 30);
        let text: Vec<String> = lines.iter().map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect()).collect();
        assert_eq!(text[0], "Title");
        assert_eq!(text[2], "• first item");
        assert!(text[5].starts_with(" let a = 1;"));
        assert_eq!(text[5].chars().count(), 30);
        assert_eq!(text[6], "text with code and bold");
    }

    #[test]
    fn no_line_is_wider_than_the_limit() {
        let lines = markdown("1. a numbered entry that is long enough to wrap twice over the limit\n> a quote that also runs long enough to wrap", 20);
        for l in &lines {
            let s: String = l.spans.iter().map(|s| s.content.as_ref()).collect();
            assert!(width(&s) <= 20, "{:?}", s);
        }
        assert!(lines.len() >= 6);
    }

    #[test]
    fn input_cursor_follows_rows() {
        let chars: Vec<char> = "abcdef\ngh".chars().collect();
        let (rows, at) = input_rows(&chars, 5, 4);
        assert_eq!(rows, vec!["abcd", "ef", "gh"]);
        assert_eq!(at, (1, 1));
        let (_, end) = input_rows(&chars, chars.len(), 4);
        assert_eq!(end, (2, 2));
        let (rows, at) = input_rows(&"abcd".chars().collect::<Vec<_>>(), 4, 4);
        assert_eq!((rows.len(), at), (2, (1, 0)));
    }

    #[test]
    fn clip_marks_the_cut() {
        assert_eq!(clip("abcdef", 4), "abc…");
        assert_eq!(clip("abc", 4), "abc");
    }
}
