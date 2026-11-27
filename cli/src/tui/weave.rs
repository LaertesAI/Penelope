//! The weaving animation of the waiting block. A pure frame generator: it turns the state of the turn (look, tick,
//! width, finished steps) into rows of one-column characters with a tone each, and the view colours them. Warp threads
//! hang down, a shuttle crosses the shed and lays the weft over and under the threads, and every step of the turn
//! (a tool call, a finished block of text) adds one row of finished cloth below. Unicode only, no images.

/// What the turn is doing. Each look moves and sounds different.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Look {
    /// The model thinks: one shuttle, slow and even.
    Thinking,
    /// A tool runs: the shuttle runs fast, the warp stands taut.
    Tool,
    /// Clusters run: one shuttle for each cluster (at most four), spread over the shed.
    Cluster(usize),
    /// Penelope waits for the person: the shuttle rests at the post and blinks, the warp hangs loose.
    Waiting,
    /// The turn failed: a thread is broken and the shuttle stands still.
    Error,
}

/// The colour class of one character.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Post,
    Warp,
    Weft,
    Shuttle,
    Cloth,
    Idle,
    Error,
}

pub type Row = Vec<(char, Tone)>;

pub struct Params {
    pub look: Look,
    /// Counts 80 ms. Ignored when `reduced` is set.
    pub tick: u64,
    /// Columns the block may use, with the indent.
    pub width: usize,
    /// Finished steps of this turn. Each adds a row of cloth, at most three.
    pub steps: usize,
    /// Reduced motion: nothing moves and nothing blinks. The cloth rows still show the progress.
    pub reduced: bool,
}

/// The widest block, in warp threads.
const MAX_THREADS: usize = 28;
/// Under this width only the shed row is drawn.
const COMPACT: usize = 36;

/// True when the person asked for no motion: config `tui.reduced_motion` or one of the variables
/// `PENELOPE_REDUCED_MOTION`, `REDUCE_MOTION` (set and not 0).
pub fn reduced(cfg: &serde_json::Value) -> bool {
    if let Some(on) = cfg["tui"]["reduced_motion"].as_bool() {
        return on;
    }
    ["PENELOPE_REDUCED_MOTION", "REDUCE_MOTION", "PENELOPE_NO_ANIM"].iter()
        .any(|v| std::env::var(v).map(|s| !s.is_empty() && s != "0").unwrap_or(false))
}

fn threads(width: usize) -> usize {
    // one thread takes two columns; the two posts and the indent take six
    (width.saturating_sub(6) / 2).clamp(4, MAX_THREADS)
}

/// The shuttle positions (in half steps over the shed) for one tick. A shuttle goes right and comes back.
fn position(tick: u64, shed: usize, speed: u64) -> (usize, bool) {
    let cycle = 2 * shed.max(1);
    let step = ((tick / speed.max(1)) as usize) % cycle;
    if step < shed { (step, true) } else { (cycle - 1 - step, false) }
}

/// The frame as rows. The first row is the warp above the shed, the second the shed with the shuttle, then the cloth.
/// A compact frame is the shed row alone.
pub fn frame(p: &Params) -> Vec<Row> {
    let n = threads(p.width);
    let shed = 2 * n - 1;                       // columns between the posts: thread, gap, thread ...
    let tick = if p.reduced { 0 } else { p.tick };
    let broken = n / 2;
    let warp_char = match p.look {
        Look::Tool => '┃',
        Look::Waiting => '╎',
        _ => '│',
    };
    let warp_tone = if p.look == Look::Waiting { Tone::Idle } else { Tone::Warp };

    // the shuttles of this look
    let shuttles: Vec<(usize, bool)> = match p.look {
        Look::Thinking => vec![position(tick, shed, 2)],
        Look::Tool => vec![position(tick, shed, 1)],
        Look::Cluster(k) => {
            let k = k.clamp(1, 4);
            let (base, right) = position(tick, shed, 2);
            (0..k).map(|i| ((base + i * shed / k) % shed, right)).collect()
        }
        Look::Waiting => vec![(0, true)],
        Look::Error => vec![(broken * 2, true)],
    };
    let blink_off = p.look == Look::Waiting && !p.reduced && (tick / 6) % 2 == 1;

    let mut warp: Row = vec![(' ', Tone::Idle), (' ', Tone::Idle), (' ', Tone::Idle)];
    let mut shed_row: Row = vec![(' ', Tone::Idle), (' ', Tone::Idle), ('╪', Tone::Post)];
    for c in 0..shed {
        let thread = c % 2 == 0;
        // warp row
        warp.push(if thread {
            if p.look == Look::Error && c / 2 == broken { ('╵', Tone::Error) } else { (warp_char, warp_tone) }
        } else { (' ', Tone::Idle) });
        // shed row: behind a shuttle the weft is laid, ahead the warp stands open
        let mut cell = if thread { (warp_char, warp_tone) } else { (' ', Tone::Idle) };
        let behind = shuttles.iter().any(|&(at, right)| if right { c < at } else { c > at });
        if behind && p.look != Look::Waiting && p.look != Look::Error {
            // over, under: the weft passes over every second thread
            cell = if thread { if (c / 2) % 2 == 0 { ('━', Tone::Weft) } else { (warp_char, warp_tone) } } else { ('─', Tone::Weft) };
        }
        if p.look == Look::Error {
            cell = if thread && c / 2 == broken { ('╳', Tone::Error) } else if c < broken * 2 && !thread { ('─', Tone::Weft) } else if c < broken * 2 { ('━', Tone::Weft) } else { cell };
        }
        if shuttles.iter().any(|&(at, _)| at == c) {
            cell = match p.look {
                Look::Error => ('✖', Tone::Error),
                Look::Waiting => (if blink_off { '◇' } else { '◆' }, Tone::Shuttle),
                _ => ('◆', Tone::Shuttle),
            };
        }
        shed_row.push(cell);
    }
    warp.extend([(' ', Tone::Idle), (' ', Tone::Idle)]);
    shed_row.push(('╪', Tone::Post));

    if p.width < COMPACT {
        return vec![shed_row];
    }
    let mut rows = vec![warp, shed_row];
    for i in 0..p.steps.min(3) {
        let mut row: Row = vec![(' ', Tone::Idle), (' ', Tone::Idle), (' ', Tone::Idle)];
        for c in 0..shed {
            // a twill that shifts by one thread in every row, so each step leaves its own line in the cloth
            let on = (c / 2 + i) % 4 < 2;
            row.push(if c % 2 == 1 { ('▒', Tone::Cloth) } else if on { ('▓', Tone::Cloth) } else { ('░', Tone::Cloth) });
        }
        rows.push(row);
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(look: Look, tick: u64, width: usize, steps: usize) -> Params {
        Params { look, tick, width, steps, reduced: false }
    }

    fn text(row: &Row) -> String {
        row.iter().map(|(c, _)| *c).collect()
    }

    fn shuttle_at(rows: &[Row]) -> Vec<usize> {
        rows.iter().flat_map(|r| r.iter().enumerate().filter(|(_, (c, t))| *t == Tone::Shuttle && matches!(*c, '◆' | '◇')).map(|(i, _)| i)).collect()
    }

    #[test]
    fn every_row_fits_the_width_and_the_shed_row_holds_one_shuttle() {
        for width in [12usize, 24, 36, 60, 120] {
            for look in [Look::Thinking, Look::Tool, Look::Cluster(3), Look::Waiting, Look::Error] {
                for tick in [0u64, 7, 23, 99] {
                    let rows = frame(&p(look, tick, width, 3));
                    assert!(!rows.is_empty());
                    for r in &rows {
                        assert!(r.len() <= width.max(14), "{:?} at {} wide: {} columns", look, width, r.len());
                    }
                    if look == Look::Thinking || look == Look::Tool {
                        assert_eq!(shuttle_at(&rows).len(), 1, "{:?}", look);
                    }
                }
            }
        }
    }

    #[test]
    fn the_shuttle_crosses_the_shed_and_comes_back() {
        let at = |tick| shuttle_at(&frame(&p(Look::Thinking, tick, 60, 0)))[0];
        let seen: Vec<usize> = (0..120).map(at).collect();
        let (min, max) = (*seen.iter().min().unwrap(), *seen.iter().max().unwrap());
        assert!(max - min >= 40, "{} to {}", min, max);
        assert!(seen.windows(2).any(|w| w[1] < w[0]), "it turns around");
        // the tool look is faster: more distance in the same ticks
        let travel = |look| (0..20u64).map(|t| shuttle_at(&frame(&p(look, t, 60, 0)))[0] as i64).collect::<Vec<_>>().windows(2).map(|w| (w[1] - w[0]).abs()).sum::<i64>();
        assert!(travel(Look::Tool) > travel(Look::Thinking));
    }

    #[test]
    fn weft_is_laid_behind_the_shuttle_and_the_warp_stays_open_ahead() {
        let rows = frame(&p(Look::Thinking, 20, 60, 0));
        let shed = &rows[1];
        let at = shuttle_at(&rows)[0];
        assert!(shed[3..at].iter().any(|(_, t)| *t == Tone::Weft) || shed[at + 1..].iter().any(|(_, t)| *t == Tone::Weft));
        let ahead: Vec<_> = if shed[at - 1].1 == Tone::Weft { shed[at + 1..shed.len() - 1].to_vec() } else { shed[3..at].to_vec() };
        assert!(ahead.iter().all(|(_, t)| *t != Tone::Weft));
    }

    #[test]
    fn each_step_adds_a_row_of_cloth_up_to_three() {
        let cloth = |steps| frame(&p(Look::Thinking, 0, 60, steps)).iter().filter(|r| r.iter().any(|(_, t)| *t == Tone::Cloth)).count();
        assert_eq!((cloth(0), cloth(1), cloth(2), cloth(3), cloth(9)), (0, 1, 2, 3, 3));
        let rows = frame(&p(Look::Thinking, 0, 60, 3));
        assert_ne!(text(&rows[2]), text(&rows[3]), "every row shows a different line");
    }

    #[test]
    fn a_cluster_look_shows_one_shuttle_for_each_cluster() {
        for k in 1..=4usize {
            assert_eq!(shuttle_at(&frame(&p(Look::Cluster(k), 5, 80, 0))).len(), k, "{} clusters", k);
        }
        assert_eq!(shuttle_at(&frame(&p(Look::Cluster(9), 5, 80, 0))).len(), 4, "at most four");
    }

    #[test]
    fn waiting_blinks_and_error_stands_still_with_a_broken_thread() {
        let a = frame(&p(Look::Waiting, 0, 60, 0));
        let b = frame(&p(Look::Waiting, 6, 60, 0));
        assert_ne!(text(&a[1]), text(&b[1]), "the resting shuttle blinks");
        let e1 = frame(&p(Look::Error, 0, 60, 1));
        let e2 = frame(&p(Look::Error, 50, 60, 1));
        assert_eq!(e1, e2, "an error does not move");
        assert!(e1[1].iter().any(|(c, t)| *c == '✖' && *t == Tone::Error) && e1[0].iter().any(|(c, _)| *c == '╵'));
    }

    #[test]
    fn reduced_motion_freezes_every_look() {
        for look in [Look::Thinking, Look::Tool, Look::Cluster(2), Look::Waiting] {
            let at = |tick| frame(&Params { look, tick, width: 60, steps: 2, reduced: true });
            assert_eq!(at(0), at(31), "{:?}", look);
        }
    }

    #[test]
    fn a_narrow_terminal_gets_the_shed_row_alone() {
        let rows = frame(&p(Look::Thinking, 3, 24, 3));
        assert_eq!(rows.len(), 1);
        assert!(rows[0].len() <= 24 && rows[0].iter().any(|(c, _)| *c == '◆'));
    }

    #[test]
    fn the_setting_and_the_variables_switch_the_motion_off() {
        assert!(reduced(&serde_json::json!({"tui": {"reduced_motion": true}})));
        assert!(!reduced(&serde_json::json!({"tui": {"reduced_motion": false}})));
    }
}
