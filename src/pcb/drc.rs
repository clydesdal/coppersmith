//! Checking the board against its rules before it goes to the maker.

use crate::pcb::copper::{Copper, Cu, Item};
use std::collections::{HashMap, HashSet};
use crate::pcb::geom::*;
use crate::pcb::model::*;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Severity {
    Error,
    Warning,
}

#[derive(Clone, Debug)]
pub struct Problem {
    pub severity: Severity,
    pub title: String,
    pub detail: String,
    pub at: P,
    pub items: Vec<Item>,
}

pub fn describe(board: &Board, item: Item) -> String {
    match item {
        Item::Pad(f, i) => match board.fp(f) {
            Some(fp) => format!("{} pad {}", fp.refdes, fp.pads.get(i as usize).map(|p| p.num.as_str()).unwrap_or("?")),
            None => "a pad".into(),
        },
        Item::Track(_) => "track".into(),
        Item::Via(_) => "via".into(),
        Item::Pour(id, _) => match board.pour(id) {
            Some(p) if p.plane => "plane".into(),
            _ => "pour".into(),
        },
        Item::Fill(_) => "fill".into(),
    }
}

/// What the rule check leaves for the next one: the copper it looked at and
/// the two kinds of problem that cost the most to find, those of copper too
/// close to other copper and to the board's edge. Each is about one or two
/// items, so when only some items have changed the others' problems stand.
#[derive(Clone)]
pub struct Cache {
    key: Key,
    items: Vec<Cu>,
    index: HashMap<Item, usize>,
    heavy: Vec<Problem>,
}

/// What every problem's wording or meaning rests on beyond the items: when it
/// changes, everything is checked again.
#[derive(Clone, PartialEq)]
struct Key {
    rules: Rules,
    outline: Vec<P>,
    nets: Vec<Net>,
    parts: Vec<(u32, String, Vec<String>)>,
    planes: Vec<(u32, bool)>,
}

impl Key {
    fn of(board: &Board) -> Key {
        Key {
            rules: board.rules.clone(),
            outline: board.outline.clone(),
            nets: board.nets.clone(),
            parts: board.footprints.iter().map(|f| (f.id, f.refdes.clone(), f.pads.iter().map(|p| p.num.clone()).collect())).collect(),
            planes: board.pours.iter().map(|p| (p.id, p.plane)).collect(),
        }
    }
}

pub fn check(board: &Board, copper: &Copper) -> Vec<Problem> {
    check_again(board, copper, None).0
}

/// The same as `check`, given what the last one left: only copper that has
/// changed since is looked at again. The answer is what `check` would give.
pub fn check_again(board: &Board, copper: &Copper, last: Option<&Cache>) -> (Vec<Problem>, Cache) {
    let key = Key::of(board);
    let items = &copper.items;
    let last = last.filter(|l| l.key == key);

    // Which copper needs looking at: all of it, or what differs from last time.
    let dirty: Vec<bool> = match last {
        Some(l) => items.iter().map(|c| l.index.get(&c.item).is_none_or(|&k| !l.items[k].same_as(c))).collect(),
        None => vec![true; items.len()],
    };
    let mut heavy: Vec<Problem> = Vec::new();
    if let Some(l) = last {
        let mut stale: HashSet<Item> = l.items.iter().map(|c| c.item).filter(|i| !copper.index.contains_key(i)).collect();
        stale.extend(items.iter().zip(&dirty).filter(|(_, d)| **d).map(|(c, _)| c.item));
        heavy.extend(l.heavy.iter().filter(|p| !p.items.iter().any(|i| stale.contains(i))).cloned());
    }
    if dirty.iter().any(|d| *d) {
        clearance_problems(board, items, &dirty, &mut heavy);
        edge_problems(board, items, &dirty, &mut heavy);
    }

    let mut out = heavy.clone();
    small_problems(board, copper, &mut out);
    out.sort_by(|a, b| {
        (a.severity == Severity::Warning, &a.title, &a.items, &a.detail).cmp(&(b.severity == Severity::Warning, &b.title, &b.items, &b.detail))
    });
    (out, Cache { key, items: items.clone(), index: copper.index.clone(), heavy })
}

/// Copper of different nets too close together, for pairs with at least one
/// item that `dirty` marks. Pours may keep a wider gap.
fn clearance_problems(board: &Board, items: &[Cu], dirty: &[bool], out: &mut Vec<Problem>) {
    let r = &board.rules;
    let widest = items.iter().filter_map(|c| c.keeps.map(|k| k.0)).fold(r.clearance, f64::max);
    for (i, j) in crate::pcb::copper::near_pairs(items, widest) {
        if !dirty[i] && !dirty[j] {
            continue;
        }
        let (a, b) = (&items[i], &items[j]);
        if !a.on.shares(b.on) || (a.net == b.net && a.net != 0) {
            continue;
        }
        // Two pieces of one pour never clash with each other.
        if let (Item::Pour(x, _), Item::Pour(y, _)) = (a.item, b.item)
            && x == y
        {
            continue;
        }
        let need = [a.keeps, b.keeps].iter().flatten().map(|k| k.0).fold(r.clearance, f64::max);
        // Pads of one part with no nets are its own business.
        if let (Item::Pad(fa, _), Item::Pad(fb, _)) = (a.item, b.item)
            && fa == fb
            && a.net == 0
            && b.net == 0
        {
            continue;
        }
        let (gap, pa, pb) = a.gap_within(b, need);
        if gap < need - 1e-6 {
            let (what, sev) = if gap <= 1e-6 { ("Short circuit", Severity::Error) } else { ("Too close", Severity::Error) };
            out.push(Problem {
                severity: sev,
                title: what.into(),
                detail: format!(
                    "{} ({}) and {} ({}): {:.3} mm, needs {:.3}",
                    describe(board, a.item),
                    board.net_name(a.net),
                    describe(board, b.item),
                    board.net_name(b.net),
                    gap.max(0.0),
                    need
                ),
                at: pa.lerp(pb, 0.5),
                items: vec![a.item, b.item],
            });
        }
    }
}

/// Copper near or past the board's edge, for the items `dirty` marks.
fn edge_problems(board: &Board, items: &[Cu], dirty: &[bool], out: &mut Vec<Problem>) {
    let r = &board.rules;
    let edge = &board.outline;
    if edge.len() < 3 {
        return;
    }
    let n = edge.len();
    let mut cand: Vec<u32> = Vec::new();
    for (cu, _) in items.iter().zip(dirty).filter(|(_, d)| **d) {
        let need = cu.keeps.map(|k| k.1).unwrap_or(r.edge_clearance);
        let mut worst: Option<(f64, P)> = None;
        for k in 0..n {
            let (a, b) = (edge[k], edge[(k + 1) % n]);
            let seg = BBox::of(&[a, b]);
            // A pour's pieces far from this stretch of edge needn't be
            // measured; the first is always taken so there's a reading.
            match &cu.grid {
                Some(grid) => {
                    grid.near(&seg.grown(need + 0.01), &mut cand);
                    if worst.is_none() {
                        cand.push(0);
                        cand.sort_unstable();
                        cand.dedup();
                    }
                }
                None => {
                    cand.clear();
                    cand.extend(0..cu.parts.len() as u32);
                }
            }
            for &k in &cand {
                let (part, bb) = (&cu.parts[k as usize], &cu.boxes[k as usize]);
                if cu.parts.len() > 1 && box_gap(bb, &seg) > need + 0.01 && worst.is_some() {
                    continue;
                }
                let (d, pa, _) = core_dist(&part.core, &[a, b]);
                let gap = d - part.r;
                if worst.is_none_or(|w| gap < w.0) {
                    worst = Some((gap, pa));
                }
            }
        }
        // Each piece's middle (its corners' mean, inside it as it's convex).
        let mid = |b: &Blob| b.core.iter().fold(P::default(), |s, q| s.add(*q)).scale(1.0 / b.core.len().max(1) as f64);
        let inside = cu.parts.iter().all(|part| point_in_ring(mid(part), edge));
        if let Some((gap, at)) = worst
            && (!inside || gap < need - 1e-6)
        {
            out.push(Problem {
                severity: Severity::Error,
                title: if inside { "Too close to the edge".into() } else { "Off the board".into() },
                detail: if inside {
                    format!("{}: {:.3} mm from the edge, needs {:.3}", describe(board, cu.item), gap.max(0.0), need)
                } else {
                    format!("{} is outside the board outline", describe(board, cu.item))
                },
                at,
                items: vec![cu.item],
            });
        }
    }
}

/// The problems that are quick to find, looked for afresh every time: thin
/// tracks, small holes, a missing outline, parts on top of each other, and
/// connections still to make.
fn small_problems(board: &Board, copper: &Copper, out: &mut Vec<Problem>) {
    let r = &board.rules;
    for t in &board.tracks {
        if t.w < r.min_track - 1e-6 {
            out.push(Problem {
                severity: Severity::Error,
                title: "Track too thin".into(),
                detail: format!("{:.3} mm, the least is {:.3}", t.w, r.min_track),
                at: t.a.lerp(t.b, 0.5),
                items: vec![Item::Track(t.id)],
            });
        }
        if t.layer != TOP && t.layer != BOTTOM && t.layer >= board.copper.saturating_sub(1) {
            out.push(Problem {
                severity: Severity::Error,
                title: "Track on a missing layer".into(),
                detail: format!("{} isn't in this {}-layer board", Layer::Cu(t.layer).name(), board.copper),
                at: t.a.lerp(t.b, 0.5),
                items: vec![Item::Track(t.id)],
            });
        }
    }

    let hole = |out: &mut Vec<Problem>, item: Item, at: P, drill: f64, ring: Option<f64>| {
        if drill < r.min_drill - 1e-6 {
            out.push(Problem {
                severity: Severity::Error,
                title: "Hole too small".into(),
                detail: format!("{:.3} mm, the least is {:.3}", drill, r.min_drill),
                at,
                items: vec![item],
            });
        }
        if let Some(ring) = ring
            && ring < r.min_ring - 1e-6
        {
            out.push(Problem {
                severity: Severity::Error,
                title: "Copper ring too thin".into(),
                detail: format!("{:.3} mm round the hole, the least is {:.3}", ring.max(0.0), r.min_ring),
                at,
                items: vec![item],
            });
        }
    };
    for v in &board.vias {
        hole(out, Item::Via(v.id), v.at, v.drill, Some((v.dia - v.drill) / 2.0));
    }
    for f in &board.footprints {
        for (i, pad) in f.pads.iter().enumerate() {
            if pad.through() {
                let ring = (!pad.bare_hole()).then(|| (pad.size.x.min(pad.size.y) - pad.drill) / 2.0);
                hole(out, Item::Pad(f.id, i as u16), f.pad_center(i), pad.drill, ring);
            }
        }
    }

    if board.outline.len() < 3 {
        out.push(Problem {
            severity: Severity::Warning,
            title: "No board outline".into(),
            detail: "Draw one with the Outline tool".into(),
            at: P::default(),
            items: vec![],
        });
    }

    // Parts that sit on top of each other.
    let fps = &board.footprints;
    let boxes: Vec<BBox> = fps.iter().map(|f| f.bbox()).collect();
    let all_through: Vec<bool> = fps.iter().map(|f| f.pads.iter().all(|p| p.through())).collect();
    for i in 0..fps.len() {
        for j in i + 1..fps.len() {
            let (a, b) = (&fps[i], &fps[j]);
            if a.side != b.side && !(all_through[i] && all_through[j]) {
                continue;
            }
            let (ba, bb) = (boxes[i], boxes[j]);
            let w = ba.max.x.min(bb.max.x) - ba.min.x.max(bb.min.x);
            let h = ba.max.y.min(bb.max.y) - ba.min.y.max(bb.min.y);
            if w > 0.01 && h > 0.01 {
                let at = p(ba.min.x.max(bb.min.x) + w / 2.0, ba.min.y.max(bb.min.y) + h / 2.0);
                out.push(Problem {
                    severity: Severity::Warning,
                    title: "Parts overlap".into(),
                    detail: format!("{} and {} courtyards overlap", a.refdes, b.refdes),
                    at,
                    items: vec![],
                });
            }
        }
    }

    for rat in &copper.rats {
        out.push(Problem {
            severity: Severity::Warning,
            title: "Not connected".into(),
            detail: format!("{} still needs a track here", board.net_name(rat.net)),
            at: rat.a.lerp(rat.b, 0.5),
            items: vec![],
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pcb::copper::analyse;

    fn track(b: &mut Board, a: (f64, f64), e: (f64, f64), layer: u8, net: u32) -> u32 {
        let id = b.new_id();
        b.tracks.push(Track { mid: None, id, a: p(a.0, a.1), b: p(e.0, e.1), w: 0.25, layer, net });
        id
    }

    /// What the check says, as text, in the order it gives.
    fn said(found: &[Problem]) -> Vec<String> {
        found.iter().map(|p| format!("{:?} {} | {} | {:?} | {:?}", p.severity, p.title, p.detail, p.items, p.at)).collect()
    }

    /// Checks again after each change, and the answer must be what a check from
    /// scratch gives.
    #[test]
    fn checking_again_gives_what_checking_from_scratch_gives() {
        let mut b = Board::default();
        let (a, n, g) = (b.net("A"), b.net("B"), b.net("GND"));
        let first = track(&mut b, (5.0, 10.0), (40.0, 10.0), TOP, a);
        track(&mut b, (5.0, 20.0), (40.0, 20.0), TOP, n);
        track(&mut b, (5.0, 10.4), (40.0, 10.4), BOTTOM, n);
        let id = b.new_id();
        b.pours.push(Pour::new(id, TOP, g, Vec::new(), &b.rules));
        b.repour();

        let mut cache: Option<Cache> = None;
        let mut step = 0;
        let mut go = |b: &mut Board, what: &str| {
            b.repour();
            let copper = analyse(b);
            let (again, left) = check_again(b, &copper, cache.as_ref());
            let scratch = check(b, &copper);
            assert_eq!(said(&again), said(&scratch), "step {step}: {what}");
            cache = Some(left);
            step += 1;
            scratch.len()
        };
        let start = go(&mut b, "the first check");
        assert_eq!(start, go(&mut b, "nothing changed"));

        // A track of another net laid too close, on the same layer as one.
        let close = track(&mut b, (5.0, 10.4), (40.0, 10.4), TOP, n);
        assert!(go(&mut b, "a close track") > start);
        // Moved away again.
        b.tracks.iter_mut().find(|t| t.id == close).unwrap().a = p(5.0, 14.0);
        b.tracks.iter_mut().find(|t| t.id == close).unwrap().b = p(40.0, 14.0);
        go(&mut b, "moved away");
        // Dropped on the board's edge.
        let edge = track(&mut b, (0.0, 30.0), (30.0, 30.0), TOP, a);
        assert!(go(&mut b, "a track at the edge") > start);
        // A via too near a track.
        let vid = b.new_id();
        b.vias.push(Via { id: vid, at: p(20.0, 10.5), dia: 0.6, drill: 0.3, net: n });
        go(&mut b, "a via");
        // Its drill made too small (the copper doesn't change, the problem does).
        b.vias.iter_mut().find(|v| v.id == vid).unwrap().drill = 0.05;
        go(&mut b, "a small drill");
        // A track taken away.
        b.tracks.retain(|t| t.id != first && t.id != edge);
        go(&mut b, "tracks removed");
        // The pour keeping further away, which moves its edges.
        b.pours[0].clearance = 1.5;
        go(&mut b, "a pour with a wider clearance");
        b.pours[0].net = n;
        go(&mut b, "a pour of another net");
        // Things everything's worded or judged by.
        b.rules.clearance = 2.0;
        go(&mut b, "a wider clearance rule");
        b.nets[0].name = "RENAMED".into();
        go(&mut b, "a net renamed");
        b.outline = rect_outline(p(2.0, 2.0), p(30.0, 25.0));
        go(&mut b, "a smaller board");
        b.outline.clear();
        go(&mut b, "no outline");
    }

    #[test]
    fn close_tracks_of_two_nets_are_caught() {
        let mut b = Board::default();
        let (n1, n2) = (b.net("A"), b.net("B"));
        b.tracks.push(Track { mid: None, id: 1, a: p(5.0, 10.0), b: p(20.0, 10.0), w: 0.25, layer: TOP, net: n1 });
        b.tracks.push(Track { mid: None, id: 2, a: p(5.0, 10.4), b: p(20.0, 10.4), w: 0.25, layer: TOP, net: n2 });
        let found = check(&b, &analyse(&b));
        assert!(found.iter().any(|p| p.title == "Too close"), "{found:?}");
        b.tracks[1].b = p(20.0, 10.0);
        b.tracks[1].a = p(5.0, 10.0);
        let found = check(&b, &analyse(&b));
        assert!(found.iter().any(|p| p.title == "Short circuit"));
        b.tracks[1].layer = BOTTOM;
        assert!(check(&b, &analyse(&b)).iter().all(|p| p.severity != Severity::Error));
    }
}
