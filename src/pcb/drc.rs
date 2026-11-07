//! Checking the board against its rules before it goes to the maker.

use crate::pcb::copper::{self, Copper, Cu, Item};
use std::collections::{HashMap, HashSet};
use crate::pcb::geom::*;
use crate::pcb::model::*;
use crate::pcb::rulebook::RuleId;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Severity {
    Error,
    Warning,
}

#[derive(Clone, Debug)]
pub struct Problem {
    /// The rule it breaks.
    pub rule: RuleId,
    pub severity: Severity,
    pub title: String,
    pub detail: String,
    pub at: P,
    pub items: Vec<Item>,
}

impl Problem {
    /// Whether it's marked on the board: an unrouted connection is drawn as a rat's nest line
    /// instead, and a problem with nothing of its own to point at isn't marked (parts, which
    /// have no copper item, are).
    pub fn marked(&self) -> bool {
        self.title != "Not connected" && (!self.items.is_empty() || self.title.starts_with("Parts"))
    }
}

/// How much short of its gap copper may be before it's too close. Altium's
/// coordinates are 2.5 nm apart and each rounds when it comes in, so a gap
/// that is exactly the rule's there can come out a few nanometres short here.
const TOLERANCE: f64 = 1e-5;

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
    let widest = items.iter().filter_map(|c| c.keeps.map(|k| k.0)).fold(r.widest(), f64::max);
    for (i, j) in crate::pcb::copper::near_pairs_where(items, widest, |i, j| dirty[i] || dirty[j]) {
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
        // The gap the rules ask of these two kinds of copper; a pour keeps its own too, if wider.
        let need = [a.keeps, b.keeps].iter().flatten().map(|k| k.0).fold(r.gap(a.kind(), b.kind()), f64::max);
        // Pads of one part with no nets are its own business.
        if let (Item::Pad(fa, _), Item::Pad(fb, _)) = (a.item, b.item)
            && fa == fb
            && a.net == 0
            && b.net == 0
        {
            continue;
        }
        let (gap, pa, pb) = a.gap_within(b, need);
        if gap < need - TOLERANCE {
            let (what, sev, rule) = if gap <= 1e-6 { ("Short circuit", Severity::Error, RuleId::ShortCircuit) } else { ("Too close", Severity::Error, RuleId::Clearance) };
            if !r.is_on(rule) {
                continue;
            }
            out.push(Problem {
                rule,
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
    if edge.len() < 3 || !r.is_on(RuleId::BoardOutlineClearance) {
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
            && (!inside || gap < need - TOLERANCE)
        {
            out.push(Problem {
                rule: RuleId::BoardOutlineClearance,
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

/// Copper in a keepout that keeps it out: tracks, vias and pads (pours are cut
/// round keepouts when they're poured). Only what overlaps counts; touching
/// the edge of a keepout is allowed.
fn keepout_problems(board: &Board, copper: &Copper, out: &mut Vec<Problem>) {
    if !board.rules.is_on(RuleId::Keepout) {
        return;
    }
    let keepouts: Vec<(PlacedKeepout, Vec<Blob>)> = board
        .placed_keepouts()
        .into_iter()
        .filter(|k| k.restrict.any())
        .map(|k| {
            let pieces = k.pieces().into_iter().map(|core| Blob { core, r: 0.0 }).collect();
            (k, pieces)
        })
        .collect();
    if keepouts.is_empty() {
        return;
    }
    for cu in &copper.items {
        // What kind of copper it is, for what the keepout restricts.
        let (kind, forbids): (&str, fn(&Restrict) -> bool) = match cu.item {
            Item::Pour(..) => continue,
            Item::Track(_) | Item::Fill(_) => ("track", |r| r.tracks),
            Item::Via(_) => ("via", |r| r.vias),
            Item::Pad(f, i) => match board.fp(f).and_then(|f| f.pads.get(i as usize)) {
                Some(pad) if pad.through() => ("through-hole pad", |r| r.th_pads),
                _ => ("pad", |r| r.smd_pads),
            },
        };
        for (k, pieces) in &keepouts {
            if !forbids(&k.restrict) {
                continue;
            }
            // Its layers: a via and a through-hole pad are on all of them.
            let on_layer = match cu.on {
                copper::On::One(l) => k.applies_to(l),
                copper::On::All => true,
            };
            // A part's own pads aren't held to the part's own keepouts.
            if !on_layer || matches!((cu.item, k.owner), (Item::Pad(f, _), Some(o)) if f == o) {
                continue;
            }
            let kb = k.bbox();
            if !kb.touches(&cu.bbox) {
                continue;
            }
            let mut worst: Option<(f64, P)> = None;
            for (part, pb) in cu.parts.iter().zip(cu.boxes.iter()) {
                if !kb.touches(pb) {
                    continue;
                }
                for piece in pieces {
                    let (gap, a, b) = part.gap(piece);
                    if gap < -1e-6 && worst.is_none_or(|w| gap < w.0) {
                        worst = Some((gap, a.lerp(b, 0.5)));
                    }
                }
            }
            if let Some((_, at)) = worst {
                out.push(Problem {
                    rule: RuleId::Keepout,
                    severity: Severity::Error,
                    title: "In a keepout".into(),
                    detail: format!("{} ({}) is in a keepout that keeps {kind}s out", describe(board, cu.item), board.net_name(cu.net)),
                    at,
                    items: vec![cu.item],
                });
            }
        }
    }
}

/// The problems that are quick to find, looked for afresh every time: thin
/// tracks, small holes, a missing outline, parts on top of each other, and
/// connections still to make.
fn small_problems(board: &Board, copper: &Copper, out: &mut Vec<Problem>) {
    let r = &board.rules;
    for t in &board.tracks {
        if r.is_on(RuleId::Width) && t.w < r.min_track - 1e-6 {
            out.push(Problem {
                rule: RuleId::Width,
                severity: Severity::Error,
                title: "Track too thin".into(),
                detail: format!("{:.3} mm, the least is {:.3}", t.w, r.min_track),
                at: t.a.lerp(t.b, 0.5),
                items: vec![Item::Track(t.id)],
            });
        }
        if r.is_on(RuleId::Width) && r.max_track > 0.0 && t.w > r.max_track + 1e-6 {
            out.push(Problem {
                rule: RuleId::Width,
                severity: Severity::Error,
                title: "Track too wide".into(),
                detail: format!("{:.3} mm, the most is {:.3}", t.w, r.max_track),
                at: t.a.lerp(t.b, 0.5),
                items: vec![Item::Track(t.id)],
            });
        }
        if t.layer != TOP && t.layer != BOTTOM && t.layer >= board.copper.saturating_sub(1) {
            out.push(Problem {
                rule: RuleId::RoutingLayers,
                severity: Severity::Error,
                title: "Track on a missing layer".into(),
                detail: format!("{} isn't in this {}-layer board", Layer::Cu(t.layer).name(), board.copper),
                at: t.a.lerp(t.b, 0.5),
                items: vec![Item::Track(t.id)],
            });
        }
    }

    let hole = |out: &mut Vec<Problem>, item: Item, at: P, drill: f64, ring: Option<f64>| {
        if r.is_on(RuleId::HoleSize) && drill < r.min_drill - 1e-6 {
            out.push(Problem {
                rule: RuleId::HoleSize,
                severity: Severity::Error,
                title: "Hole too small".into(),
                detail: format!("{:.3} mm, the least is {:.3}", drill, r.min_drill),
                at,
                items: vec![item],
            });
        }
        if r.is_on(RuleId::HoleSize) && r.max_drill > 0.0 && drill > r.max_drill + 1e-6 {
            out.push(Problem {
                rule: RuleId::HoleSize,
                severity: Severity::Error,
                title: "Hole too large".into(),
                detail: format!("{:.3} mm, the most is {:.3}", drill, r.max_drill),
                at,
                items: vec![item],
            });
        }
        if r.is_on(RuleId::MinimumAnnularRing)
            && let Some(ring) = ring
            && ring < r.min_ring - 1e-6
        {
            out.push(Problem {
                rule: RuleId::MinimumAnnularRing,
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

    keepout_problems(board, copper, out);
    via_problems(board, out);
    hole_gap_problems(board, out);
    acute_problems(board, out);
    length_problems(board, out);
    class_problems(board, out);
    unpoured_problems(board, out);

    if board.outline.len() < 3 && r.is_on(RuleId::BoardOutlineClearance) {
        out.push(Problem {
            rule: RuleId::BoardOutlineClearance,
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
            if !r.is_on(RuleId::ComponentClearance) {
                continue;
            }
            let (ba, bb) = (boxes[i], boxes[j]);
            let w = ba.max.x.min(bb.max.x) - ba.min.x.max(bb.min.x);
            let h = ba.max.y.min(bb.max.y) - ba.min.y.max(bb.min.y);
            if w > 0.01 && h > 0.01 {
                let at = p(ba.min.x.max(bb.min.x) + w / 2.0, ba.min.y.max(bb.min.y) + h / 2.0);
                out.push(Problem {
                    rule: RuleId::ComponentClearance,
                    severity: Severity::Warning,
                    title: "Parts overlap".into(),
                    detail: format!("{} and {} courtyards overlap", a.refdes, b.refdes),
                    at,
                    items: vec![],
                });
            } else if r.component_gap > 0.0 {
                // Not touching, but nearer than the rule allows (corner to corner, or side by side).
                let apart = (-w).max(0.0).hypot((-h).max(0.0));
                if apart < r.component_gap - TOLERANCE {
                    out.push(Problem {
                        rule: RuleId::ComponentClearance,
                        severity: Severity::Warning,
                        title: "Parts too close".into(),
                        detail: format!("{} and {} courtyards are {:.3} mm apart, needs {:.3}", a.refdes, b.refdes, apart, r.component_gap),
                        at: ba.center().lerp(bb.center(), 0.5),
                        items: vec![],
                    });
                }
            }
        }
    }

    for rat in copper.rats.iter().filter(|_| r.is_on(RuleId::UnRoutedNet)) {
        out.push(Problem {
            rule: RuleId::UnRoutedNet,
            severity: Severity::Warning,
            title: "Not connected".into(),
            detail: format!("{} still needs a track here", board.net_name(rat.net)),
            at: rat.a.lerp(rat.b, 0.5),
            items: vec![],
        });
    }
}

/// Vias with a pad or a hole outside what the via rule allows.
fn via_problems(board: &Board, out: &mut Vec<Problem>) {
    let r = &board.rules;
    if !r.is_on(RuleId::RoutingVias) {
        return;
    }
    for v in &board.vias {
        let mut say = |title: &str, detail: String| {
            out.push(Problem { rule: RuleId::RoutingVias, severity: Severity::Error, title: title.into(), detail, at: v.at, items: vec![Item::Via(v.id)] });
        };
        if r.via_min_dia > 0.0 && v.dia < r.via_min_dia - 1e-6 {
            say("Via too small", format!("pad {:.3} mm, the least is {:.3}", v.dia, r.via_min_dia));
        }
        if r.via_max_dia > 0.0 && v.dia > r.via_max_dia + 1e-6 {
            say("Via too large", format!("pad {:.3} mm, the most is {:.3}", v.dia, r.via_max_dia));
        }
        if r.via_min_drill > 0.0 && v.drill < r.via_min_drill - 1e-6 {
            say("Via hole too small", format!("{:.3} mm, the least is {:.3}", v.drill, r.via_min_drill));
        }
        if r.via_max_drill > 0.0 && v.drill > r.via_max_drill + 1e-6 {
            say("Via hole too large", format!("{:.3} mm, the most is {:.3}", v.drill, r.via_max_drill));
        }
    }
}

/// Drilled holes whose edges are nearer one another than the hole-to-hole rule allows.
fn hole_gap_problems(board: &Board, out: &mut Vec<Problem>) {
    let r = &board.rules;
    if !r.is_on(RuleId::HoleToHoleClearance) || r.hole_gap <= 0.0 {
        return;
    }
    let mut holes: Vec<(Item, P, f64)> = board.vias.iter().filter(|v| v.drill > 0.0).map(|v| (Item::Via(v.id), v.at, v.drill / 2.0)).collect();
    for f in &board.footprints {
        for (i, pad) in f.pads.iter().enumerate() {
            if pad.drill > 0.0 {
                holes.push((Item::Pad(f.id, i as u16), f.pad_center(i), pad.drill / 2.0));
            }
        }
    }
    holes.sort_by(|a, b| a.1.x.total_cmp(&b.1.x));
    let widest = holes.iter().map(|h| h.2).fold(0.0, f64::max);
    for i in 0..holes.len() {
        for j in i + 1..holes.len() {
            // Sorted by x: once the next is further than any pair could be near, so are the rest.
            if holes[j].1.x - holes[i].1.x > 2.0 * widest + r.hole_gap {
                break;
            }
            let (a, b) = (&holes[i], &holes[j]);
            let apart = a.1.dist(b.1) - a.2 - b.2;
            if apart < r.hole_gap - TOLERANCE {
                out.push(Problem {
                    rule: RuleId::HoleToHoleClearance,
                    severity: Severity::Error,
                    title: "Holes too close".into(),
                    detail: format!("{} and {}: {:.3} mm between the holes, needs {:.3}", describe(board, a.0), describe(board, b.0), apart.max(0.0), r.hole_gap),
                    at: a.1.lerp(b.1, 0.5),
                    items: vec![a.0, b.0],
                });
            }
        }
    }
}

/// Two straight tracks of a net meeting at a sharper angle than the rule allows, with
/// nothing else (a via, a pad, a third track) there to fill it in.
fn acute_problems(board: &Board, out: &mut Vec<Problem>) {
    let r = &board.rules;
    if !r.is_on(RuleId::AcuteAngle) || r.min_angle <= 0.0 || r.min_angle >= 180.0 {
        return;
    }
    let key = |q: P| ((q.x * 1000.0).round() as i64, (q.y * 1000.0).round() as i64);
    let mut filled: HashSet<(i64, i64)> = board.vias.iter().map(|v| key(v.at)).collect();
    for f in &board.footprints {
        for i in 0..f.pads.len() {
            filled.insert(key(f.pad_center(i)));
        }
    }
    let mut ends: HashMap<(u8, u32, i64, i64), Vec<(u32, P)>> = HashMap::new();
    for t in board.tracks.iter().filter(|t| t.mid.is_none() && t.net != 0 && !t.a.same(t.b)) {
        for (at, far) in [(t.a, t.b), (t.b, t.a)] {
            let (x, y) = key(at);
            ends.entry((t.layer, t.net, x, y)).or_default().push((t.id, far));
        }
    }
    let mut found: Vec<Problem> = Vec::new();
    for ((_, net, x, y), at) in &ends {
        if at.len() != 2 || filled.contains(&(*x, *y)) {
            continue;
        }
        let here = p(*x as f64 / 1000.0, *y as f64 / 1000.0);
        let (u, v) = (at[0].1.sub(here), at[1].1.sub(here));
        let cos = (u.dot(v) / (u.len() * v.len())).clamp(-1.0, 1.0);
        let angle = cos.acos().to_degrees();
        if angle < r.min_angle - 0.01 {
            found.push(Problem {
                rule: RuleId::AcuteAngle,
                severity: Severity::Warning,
                title: "Acute angle".into(),
                detail: format!("{}: two tracks meet at {:.0}°, the least is {:.0}°", board.net_name(*net), angle, r.min_angle),
                at: here,
                items: vec![Item::Track(at[0].0), Item::Track(at[1].0)],
            });
        }
    }
    // The map's order is arbitrary; the list must not be.
    found.sort_by(|a, b| (a.at.x, a.at.y).partial_cmp(&(b.at.x, b.at.y)).unwrap_or(std::cmp::Ordering::Equal));
    out.extend(found);
}

/// What each net's tracks add up to.
fn net_lengths(board: &Board) -> HashMap<u32, (f64, u32)> {
    let mut totals: HashMap<u32, (f64, u32)> = HashMap::new();
    for t in &board.tracks {
        let e = totals.entry(t.net).or_insert((0.0, t.id));
        e.0 += t.length();
    }
    totals
}

/// Nets with tracks adding up to more or less than the length rule allows.
fn length_problems(board: &Board, out: &mut Vec<Problem>) {
    let r = &board.rules;
    if !r.is_on(RuleId::Length) || (r.min_net_length <= 0.0 && r.max_net_length <= 0.0) {
        return;
    }
    let mut totals: Vec<(u32, (f64, u32))> = net_lengths(board).into_iter().filter(|(n, _)| *n != 0).collect();
    totals.sort_by_key(|(n, _)| *n);
    for (net, (total, first)) in totals {
        let at = board.tracks.iter().find(|t| t.id == first).map(|t| t.a.lerp(t.b, 0.5)).unwrap_or_default();
        let (title, detail) = if r.max_net_length > 0.0 && total > r.max_net_length + 1e-6 {
            ("Net too long", format!("{}: {:.3} mm of track, the most is {:.3}", board.net_name(net), total, r.max_net_length))
        } else if r.min_net_length > 0.0 && total < r.min_net_length - 1e-6 {
            ("Net too short", format!("{}: {:.3} mm of track, the least is {:.3}", board.net_name(net), total, r.min_net_length))
        } else {
            continue;
        };
        out.push(Problem { rule: RuleId::Length, severity: Severity::Error, title: title.into(), detail, at, items: vec![Item::Track(first)] });
    }
}

/// Tracks of a class of nets outside the widths its rule gives their layer, and groups of nets
/// that are further apart in length than a matched-lengths rule allows.
fn class_problems(board: &Board, out: &mut Vec<Problem>) {
    if board.class_rules.is_empty() {
        return;
    }
    if board.rules.is_on(RuleId::NetClassWidth) {
        for t in board.tracks.iter().filter(|t| t.net != 0) {
            for rule in board.class_rules_for(t.net).into_iter().filter(|r| matches!(r.kind, ClassRuleKind::Width | ClassRuleKind::PairRouting)) {
                let Some(l) = rule.on(t.layer) else { continue };
                let (lo, hi) = (l.min_width.unwrap_or(0.0), l.max_width.unwrap_or(f64::MAX));
                if t.w < lo - 1e-6 || t.w > hi + 1e-6 {
                    out.push(Problem {
                        rule: RuleId::NetClassWidth,
                        severity: Severity::Error,
                        title: "Width outside its class".into(),
                        detail: format!("{}: {:.3} mm on {}, {} allows {:.3} to {:.3}", board.net_name(t.net), t.w, Layer::Cu(t.layer).name(), rule.name, lo, hi.min(999.0)),
                        at: t.a.lerp(t.b, 0.5),
                        items: vec![Item::Track(t.id)],
                    });
                }
            }
        }
    }
    if !board.rules.is_on(RuleId::MatchedLengths) {
        return;
    }
    let totals = net_lengths(board);
    for rule in board.class_rules.iter().filter(|r| r.kind == ClassRuleKind::MatchedLengths) {
        let Some(tol) = rule.tolerance else { continue };
        // What is compared with what: within each pair, and across pairs if the rule asks.
        let groups: Vec<Vec<u32>> = match &rule.scope {
            ClassScope::PairClass(c) => {
                let pairs: Vec<&DiffPair> = board.pair_classes.iter().filter(|x| x.name.eq_ignore_ascii_case(c)).flat_map(|x| x.pairs.iter()).filter_map(|n| board.diff_pairs.iter().find(|d| d.name.eq_ignore_ascii_case(n))).collect();
                let mut g: Vec<Vec<u32>> = Vec::new();
                if rule.within_pairs {
                    g.extend(pairs.iter().map(|d| vec![d.pos, d.neg]));
                }
                if rule.between_pairs {
                    g.push(pairs.iter().flat_map(|d| [d.pos, d.neg]).collect());
                }
                g
            }
            ClassScope::NetClass(c) => board.net_classes.iter().filter(|x| x.name.eq_ignore_ascii_case(c)).map(|x| x.nets.clone()).collect(),
        };
        for g in groups {
            let lens: Vec<(u32, f64, u32)> = g.iter().filter_map(|n| totals.get(n).filter(|t| t.0 > 0.0).map(|t| (*n, t.0, t.1))).collect();
            let (Some(long), Some(short)) = (lens.iter().max_by(|a, b| a.1.total_cmp(&b.1)), lens.iter().min_by(|a, b| a.1.total_cmp(&b.1))) else { continue };
            let spread = long.1 - short.1;
            if lens.len() >= 2 && spread > tol + 1e-6 {
                let at = board.tracks.iter().find(|t| t.id == long.2).map(|t| t.a.lerp(t.b, 0.5)).unwrap_or_default();
                out.push(Problem {
                    rule: RuleId::MatchedLengths,
                    severity: Severity::Warning,
                    title: "Lengths not matched".into(),
                    detail: format!("{}: {} is {:.3} mm longer than {}, {} allows {:.3}", rule.name, board.net_name(long.0), spread, board.net_name(short.0), "the rule", tol),
                    at,
                    items: vec![Item::Track(long.2), Item::Track(short.2)],
                });
            }
        }
    }
}

/// Pours that have no copper, on a board whose other pours have.
fn unpoured_problems(board: &Board, out: &mut Vec<Problem>) {
    if !board.rules.is_on(RuleId::UnpouredPolygon) || !board.pours.iter().any(|p| !p.fill.is_empty()) {
        return;
    }
    for pour in board.pours.iter().filter(|p| p.fill.is_empty()) {
        let ring = if pour.outline.len() >= 3 { &pour.outline } else { &board.outline };
        let at = if ring.is_empty() { P::default() } else { BBox::of(ring).center() };
        out.push(Problem {
            rule: RuleId::UnpouredPolygon,
            severity: Severity::Warning,
            title: "Pour has no copper".into(),
            detail: format!("The {} pour on {} has nothing poured", board.net_name(pour.net), Layer::Cu(pour.layer).name()),
            at,
            items: vec![Item::Pour(pour.id, 0)],
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

    /// The problems on a real board, by kind:
    /// COPPERSMITH_BOARD=file cargo test dump_problems -- --ignored --nocapture
    #[test]
    #[ignore]
    fn dump_problems() {
        let Some(f) = std::env::var_os("COPPERSMITH_BOARD") else { return };
        let b = crate::pcb::altium::read_board(std::path::Path::new(&f)).unwrap().board;
        let mut by: std::collections::BTreeMap<(String, String), (usize, String)> = std::collections::BTreeMap::new();
        for p in check(&b, &analyse(&b)) {
            let e = by.entry((p.title.clone(), p.rule.info().name.to_string())).or_insert((0, p.detail.clone()));
            e.0 += 1;
        }
        for ((title, rule), (n, example)) in by {
            println!("{n:5} {title} [{rule}] e.g. {example}");
        }
        println!("off: {:?}", b.rules.off);
    }

    fn titles(b: &Board) -> Vec<String> {
        check(b, &analyse(b)).iter().map(|p| p.title.clone()).collect()
    }

    fn found(b: &Board, title: &str) -> Option<Problem> {
        check(b, &analyse(b)).into_iter().find(|p| p.title == title)
    }

    #[test]
    fn a_problem_says_which_rule_it_breaks_and_a_rule_switched_off_is_not_checked() {
        let mut b = Board::default();
        let (a, n) = (b.net("A"), b.net("B"));
        track(&mut b, (5.0, 10.0), (40.0, 10.0), TOP, a);
        track(&mut b, (5.0, 10.3), (40.0, 10.3), TOP, n);
        assert_eq!(found(&b, "Too close").expect("too close").rule, RuleId::Clearance);
        b.rules.set_on(RuleId::Clearance, false);
        assert!(!titles(&b).contains(&"Too close".to_string()), "off means unchecked");
        // Touching is a short circuit, a rule of its own.
        let mut s = Board::default();
        let (a, n) = (s.net("A"), s.net("B"));
        track(&mut s, (5.0, 10.0), (40.0, 10.0), TOP, a);
        track(&mut s, (5.0, 10.0), (40.0, 10.0), TOP, n);
        assert_eq!(found(&s, "Short circuit").expect("short").rule, RuleId::ShortCircuit);
        s.rules.set_on(RuleId::ShortCircuit, false);
        assert!(found(&s, "Short circuit").is_none());
        assert!(found(&s, "Too close").is_none(), "a short isn't also 'too close'");
    }

    #[test]
    fn vias_and_holes_outside_their_sizes_are_caught() {
        let mut b = Board::default();
        let n = b.net("A");
        let id = b.new_id();
        b.vias.push(Via { id, at: p(20.0, 20.0), dia: 0.6, drill: 0.3, net: n });
        assert!(titles(&b).iter().all(|t| !t.starts_with("Via")), "no limits, no problem");
        b.rules.via_max_dia = 0.5;
        b.rules.via_min_drill = 0.35;
        // The via style rule starts off (it's for through vias only), whatever the limits.
        assert!(found(&b, "Via too large").is_none() && !b.rules.is_on(RuleId::RoutingVias));
        b.rules.set_on(RuleId::RoutingVias, true);
        assert_eq!(found(&b, "Via too large").unwrap().rule, RuleId::RoutingVias);
        assert!(found(&b, "Via hole too small").is_some());
        b.rules.set_on(RuleId::RoutingVias, false);
        assert!(titles(&b).iter().all(|t| !t.starts_with("Via")));
        // A hole bigger than the drill limit.
        part(&mut b, p(10.0, 10.0), n, 1.2);
        assert!(found(&b, "Hole too large").is_none());
        b.rules.max_drill = 1.0;
        assert_eq!(found(&b, "Hole too large").unwrap().rule, RuleId::HoleSize);
    }

    #[test]
    fn holes_nearer_than_the_rule_allows_are_caught_when_the_rule_is_on() {
        let mut b = Board::default();
        let n = b.net("A");
        for (id, x) in [(1u32, 20.0), (2, 20.5)] {
            b.vias.push(Via { id, at: p(x, 20.0), dia: 0.6, drill: 0.3, net: n });
        }
        // Centres 0.5 apart, holes 0.3 across: 0.2 mm between their edges.
        assert!(found(&b, "Holes too close").is_none(), "off to start with");
        b.rules.set_on(RuleId::HoleToHoleClearance, true);
        let hit = found(&b, "Holes too close").expect("holes too close");
        assert_eq!(hit.rule, RuleId::HoleToHoleClearance);
        assert_eq!(hit.items.len(), 2);
        b.rules.hole_gap = 0.1;
        assert!(found(&b, "Holes too close").is_none(), "a smaller gap is allowed");
    }

    #[test]
    fn tracks_meeting_at_an_acute_angle_are_caught_unless_something_fills_the_corner() {
        let mut b = Board::default();
        let n = b.net("A");
        track(&mut b, (10.0, 10.0), (20.0, 10.0), TOP, n);
        track(&mut b, (10.0, 10.0), (20.0, 12.0), TOP, n);
        assert!(found(&b, "Acute angle").is_none(), "off to start with");
        b.rules.set_on(RuleId::AcuteAngle, true);
        let hit = found(&b, "Acute angle").expect("acute angle");
        assert_eq!(hit.rule, RuleId::AcuteAngle);
        // A right angle is fine, and a via at the corner fills it.
        b.tracks.last_mut().unwrap().b = p(10.0, 20.0);
        assert!(found(&b, "Acute angle").is_none());
        b.tracks.last_mut().unwrap().b = p(20.0, 12.0);
        let id = b.new_id();
        b.vias.push(Via { id, at: p(10.0, 10.0), dia: 0.6, drill: 0.3, net: n });
        assert!(found(&b, "Acute angle").is_none());
    }

    #[test]
    fn nets_too_long_and_parts_too_close_are_caught() {
        let mut b = Board::default();
        let n = b.net("A");
        track(&mut b, (5.0, 10.0), (15.0, 10.0), TOP, n);
        track(&mut b, (15.0, 10.0), (20.0, 10.0), TOP, n);
        b.rules.set_on(RuleId::Length, true);
        b.rules.max_net_length = 12.0;
        let hit = found(&b, "Net too long").expect("net too long");
        assert_eq!(hit.rule, RuleId::Length);
        b.rules.max_net_length = 0.0;
        b.rules.min_net_length = 20.0;
        assert!(found(&b, "Net too short").is_some());
        // Two parts side by side, a little apart.
        let mut c = Board::default();
        let m = c.net("A");
        let (x, y) = (part(&mut c, p(10.0, 10.0), m, 0.0), part(&mut c, p(12.0, 10.0), m, 0.0));
        let gap = {
            let (bx, by) = (c.fp(x).unwrap().bbox(), c.fp(y).unwrap().bbox());
            by.min.x - bx.max.x
        };
        assert!(gap > 0.0, "the two don't overlap: {gap}");
        assert!(found(&c, "Parts too close").is_none(), "no gap asked, none checked");
        c.rules.component_gap = gap + 0.2;
        assert_eq!(found(&c, "Parts too close").expect("too close").rule, RuleId::ComponentClearance);
        c.rules.component_gap = gap - 0.05;
        assert!(found(&c, "Parts too close").is_none());
    }

    #[test]
    fn a_net_class_has_its_widths_and_matched_lengths_held() {
        let mut b = Board::default();
        let (pos, neg) = (b.net("D_P"), b.net("D_N"));
        track(&mut b, (5.0, 10.0), (15.0, 10.0), TOP, pos);
        track(&mut b, (5.0, 12.0), (17.0, 12.0), TOP, neg);
        b.net_classes.push(NetClass { name: "RF".into(), nets: vec![pos] });
        b.class_rules.push(ClassRule {
            name: "Width_RF".into(),
            kind: ClassRuleKind::Width,
            scope: ClassScope::NetClass("RF".into()),
            layers: vec![LayerLimit { layer: TOP, min_width: Some(0.1), max_width: Some(0.2), ..LayerLimit::default() }],
            tolerance: None,
            max_uncoupled: None,
            within_pairs: true,
            between_pairs: false,
        });
        // The track is 0.25 wide; the class allows up to 0.2.
        let hit = found(&b, "Width outside its class").expect("width outside its class");
        assert_eq!(hit.rule, RuleId::NetClassWidth);
        b.rules.set_on(RuleId::NetClassWidth, false);
        assert!(found(&b, "Width outside its class").is_none());
        // A pair 2 mm apart in length, in a class asking for 0.5.
        b.diff_pairs.push(DiffPair { name: "D".into(), pos, neg });
        b.pair_classes.push(PairClass { name: "BUS".into(), pairs: vec!["D".into()] });
        b.class_rules.push(ClassRule {
            name: "Match".into(),
            kind: ClassRuleKind::MatchedLengths,
            scope: ClassScope::PairClass("BUS".into()),
            layers: vec![],
            tolerance: Some(0.5),
            max_uncoupled: None,
            within_pairs: true,
            between_pairs: false,
        });
        let hit = found(&b, "Lengths not matched").expect("lengths not matched");
        assert_eq!(hit.rule, RuleId::MatchedLengths);
        assert!(hit.detail.contains("2.000"), "{}", hit.detail);
        b.class_rules[1].tolerance = Some(2.5);
        assert!(found(&b, "Lengths not matched").is_none());
        // Within the pair is what's asked, so a second pair's length doesn't matter here.
        b.class_rules[1].tolerance = Some(0.5);
        b.class_rules[1].within_pairs = false;
        assert!(found(&b, "Lengths not matched").is_none(), "neither within nor between: nothing compared");
    }

    fn square(b: &mut Board, layer: Option<u8>, lo: (f64, f64), hi: (f64, f64), restrict: Restrict) -> u32 {
        let id = b.new_id();
        b.keepouts.push(Keepout { id, layer, pts: rect_outline(p(lo.0, lo.1), p(hi.0, hi.1)), restrict });
        id
    }

    fn part(b: &mut Board, at: P, net: u32, drill: f64) -> u32 {
        let id = b.new_id();
        b.footprints.push(Footprint {
            id,
            def: "TP".into(),
            refdes: format!("TP{id}"),
            value: String::new(),
            at,
            rot: 0.0,
            side: Side::Top,
            pads: vec![Pad { num: "1".into(), at: p(0.0, 0.0), size: p(1.0, 1.0), rot: 0.0, shape: PadShape::Circle, drill, net }],
            graphics: vec![],
            label_at: P::default(),
            label_height: None,
            locked: false,
            keepouts: vec![],
        });
        id
    }

    fn in_keepouts(b: &Board) -> Vec<Problem> {
        check(b, &analyse(b)).into_iter().filter(|p| p.title == "In a keepout").collect()
    }

    #[test]
    fn copper_in_a_keepout_is_caught_but_touching_its_edge_is_not() {
        let mut b = Board::default();
        let n = b.net("A");
        let t = track(&mut b, (5.0, 10.0), (20.0, 10.0), TOP, n);
        square(&mut b, Some(TOP), (10.0, 8.0), (15.0, 12.0), Restrict::default());
        let found = in_keepouts(&b);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].items, vec![Item::Track(t)]);
        assert_eq!(found[0].severity, Severity::Error);
        // Moved so its edge just touches the top of the keepout: allowed.
        b.tracks[0].a = p(5.0, 12.125);
        b.tracks[0].b = p(20.0, 12.125);
        assert!(in_keepouts(&b).is_empty());
        // And a little further in, it isn't.
        b.tracks[0].a = p(5.0, 12.0);
        b.tracks[0].b = p(20.0, 12.0);
        assert_eq!(in_keepouts(&b).len(), 1);
    }

    #[test]
    fn a_keepout_holds_on_its_own_layer_or_on_every_one() {
        let mut b = Board::default();
        let n = b.net("A");
        track(&mut b, (5.0, 10.0), (20.0, 10.0), BOTTOM, n);
        let k = square(&mut b, Some(TOP), (10.0, 8.0), (15.0, 12.0), Restrict::default());
        assert!(in_keepouts(&b).is_empty(), "a track underneath isn't in a keepout on the top");
        b.keepouts.iter_mut().find(|x| x.id == k).unwrap().layer = Some(BOTTOM);
        assert_eq!(in_keepouts(&b).len(), 1);
        b.keepouts.iter_mut().find(|x| x.id == k).unwrap().layer = None;
        assert_eq!(in_keepouts(&b).len(), 1, "every layer");
    }

    #[test]
    fn a_keepout_only_keeps_out_what_it_says() {
        let mut b = Board::default();
        let n = b.net("A");
        track(&mut b, (5.0, 10.0), (8.0, 10.0), TOP, n);
        let vid = b.new_id();
        b.vias.push(Via { id: vid, at: p(12.0, 10.0), dia: 0.6, drill: 0.3, net: n });
        part(&mut b, p(12.0, 15.0), n, 0.0);
        part(&mut b, p(12.0, 18.0), n, 0.5);
        let k = square(&mut b, Some(TOP), (7.0, 5.0), (14.0, 20.0), Restrict::default());
        let kinds = |b: &Board| in_keepouts(b).iter().map(|p| p.detail.clone()).collect::<Vec<_>>();
        let all = kinds(&b);
        assert_eq!(all.len(), 4, "a track, a via, a surface pad, a through-hole pad: {all:?}");
        assert!(all.iter().any(|d| d.contains("track")) && all.iter().any(|d| d.contains("via")));
        assert!(all.iter().any(|d| d.contains("through-hole pad")));
        for (what, restrict, left) in [
            ("tracks", Restrict { tracks: false, ..Restrict::default() }, 3),
            ("vias", Restrict { vias: false, ..Restrict::default() }, 3),
            ("surface pads", Restrict { smd_pads: false, ..Restrict::default() }, 3),
            ("through-hole pads", Restrict { th_pads: false, ..Restrict::default() }, 3),
            ("pours only", Restrict { vias: false, tracks: false, smd_pads: false, th_pads: false, pours: true }, 0),
        ] {
            b.keepouts.iter_mut().find(|x| x.id == k).unwrap().restrict = restrict;
            assert_eq!(kinds(&b).len(), left, "{what} allowed");
        }
    }

    #[test]
    fn a_part_is_not_held_to_its_own_keepout_but_others_are() {
        let mut b = Board::default();
        let n = b.net("A");
        let f = part(&mut b, p(20.0, 20.0), n, 0.0);
        let mine = Keepout { id: 0, layer: Some(TOP), pts: rect_outline(p(-2.0, -2.0), p(2.0, 2.0)), restrict: Restrict::default() };
        b.footprints[0].keepouts.push(mine.clone());
        assert!(in_keepouts(&b).is_empty(), "its own pad is inside its own keepout");
        // Another part's pad in it is caught, and the keepout goes with the part.
        part(&mut b, p(21.0, 20.0), n, 0.0);
        assert_eq!(in_keepouts(&b).len(), 1);
        b.fp_mut(f).unwrap().at = p(30.0, 30.0);
        assert!(in_keepouts(&b).is_empty(), "the keepout moved away with its part");
        // On a part underneath, the keepout's top is the board's bottom.
        let under = part(&mut b, p(40.0, 20.0), n, 0.0);
        b.fp_mut(under).unwrap().keepouts.push(mine);
        b.fp_mut(under).unwrap().side = Side::Bottom;
        let t = track(&mut b, (38.0, 20.0), (42.0, 20.0), BOTTOM, n);
        let found = in_keepouts(&b);
        assert!(found.iter().any(|p| p.items == vec![Item::Track(t)]), "{found:?}");
        b.tracks.last_mut().unwrap().layer = TOP;
        assert!(in_keepouts(&b).iter().all(|p| p.items != vec![Item::Track(t)]), "a track on the part's far side is clear");
    }

    #[test]
    fn checking_again_notices_a_keepout_come_and_go() {
        let mut b = Board::default();
        let n = b.net("A");
        track(&mut b, (5.0, 10.0), (20.0, 10.0), TOP, n);
        let c = analyse(&b);
        let (none, cache) = check_again(&b, &c, None);
        assert!(none.iter().all(|p| p.title != "In a keepout"));
        let k = square(&mut b, Some(TOP), (10.0, 8.0), (15.0, 12.0), Restrict::default());
        let c = analyse(&b);
        let (found, cache) = check_again(&b, &c, Some(&cache));
        assert_eq!(found.iter().filter(|p| p.title == "In a keepout").count(), 1);
        b.keepouts.retain(|x| x.id != k);
        let c = analyse(&b);
        let (gone, _) = check_again(&b, &c, Some(&cache));
        assert!(gone.iter().all(|p| p.title != "In a keepout"));
    }

    fn too_close(b: &Board) -> Vec<String> {
        check(b, &analyse(b)).into_iter().filter(|p| p.title == "Too close").map(|p| p.detail).collect()
    }

    #[test]
    fn the_gap_asked_depends_on_what_is_close() {
        let mut b = Board::default();
        let (a, n) = (b.net("A"), b.net("B"));
        b.rules.clearance = 0.1;
        b.rules.gaps = vec![Gap { a: Kind::Via, b: Kind::Track, mm: 0.3 }, Gap { a: Kind::Via, b: Kind::Via, mm: 0.4 }];
        // Two tracks 0.15 apart: the general gap is 0.1, so that's fine.
        track(&mut b, (5.0, 10.0), (20.0, 10.0), TOP, a);
        track(&mut b, (5.0, 10.4), (20.0, 10.4), TOP, n);
        assert!(too_close(&b).is_empty(), "{:?}", too_close(&b));
        // A via 0.2 from a track: the table asks 0.3.
        let v = b.new_id();
        b.vias.push(Via { id: v, at: p(30.0, 5.0), dia: 0.6, drill: 0.3, net: n });
        let t = track(&mut b, (25.0, 4.0), (35.0, 4.0), TOP, a);
        assert!(too_close(&b).is_empty(), "a track 0.575 from the via's edge is fine");
        b.tracks.iter_mut().find(|x| x.id == t).unwrap().a = p(25.0, 5.625);
        b.tracks.iter_mut().find(|x| x.id == t).unwrap().b = p(35.0, 5.625);
        let found = too_close(&b);
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].contains("needs 0.300") && found[0].contains("via"), "{found:?}");
        // Two vias 0.35 apart: the table asks 0.4 of two vias.
        b.tracks.retain(|x| x.id != t);
        let w = b.new_id();
        b.vias.push(Via { id: w, at: p(30.95, 5.0), dia: 0.6, drill: 0.3, net: a });
        let found = too_close(&b);
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].contains("needs 0.400"), "{found:?}");
    }

    #[test]
    fn a_gap_a_few_nanometres_short_is_not_too_close_but_a_few_microns_is() {
        let mut b = Board::default();
        let (a, n) = (b.net("A"), b.net("B"));
        b.rules.clearance = 0.1;
        track(&mut b, (5.0, 10.0), (20.0, 10.0), TOP, a);
        // Edges 0.1 - 4 nm apart: that's Altium's rounding, not a violation.
        let t = track(&mut b, (5.0, 10.35 - 4e-6), (20.0, 10.35 - 4e-6), TOP, n);
        assert!(too_close(&b).is_empty(), "{:?}", too_close(&b));
        b.tracks.iter_mut().find(|x| x.id == t).unwrap().a = p(5.0, 10.35 - 3e-5);
        b.tracks.iter_mut().find(|x| x.id == t).unwrap().b = p(20.0, 10.35 - 3e-5);
        assert_eq!(too_close(&b).len(), 1, "30 nm short is");
    }

    #[test]
    fn checking_again_notices_the_gap_table_change() {
        let mut b = Board::default();
        let (a, n) = (b.net("A"), b.net("B"));
        b.rules.clearance = 0.1;
        track(&mut b, (5.0, 10.0), (20.0, 10.0), TOP, a);
        track(&mut b, (5.0, 10.4), (20.0, 10.4), TOP, n);
        let c = analyse(&b);
        let (none, cache) = check_again(&b, &c, None);
        assert!(none.iter().all(|p| p.title != "Too close"));
        b.rules.gaps = vec![Gap { a: Kind::Track, b: Kind::Track, mm: 0.3 }];
        let c = analyse(&b);
        let (found, _) = check_again(&b, &c, Some(&cache));
        assert_eq!(found.iter().filter(|p| p.title == "Too close").count(), 1, "the table says 0.3 now, with nothing else changed");
        assert_eq!(said(&found), said(&check(&b, &c)));
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
