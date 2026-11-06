//! Checking the board against its rules before it goes to the maker.

use crate::pcb::copper::{Copper, Item};
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

pub fn check(board: &Board, copper: &Copper) -> Vec<Problem> {
    let r = &board.rules;
    let mut out = Vec::new();
    let items = &copper.items;

    // Copper of different nets too close together. Pours may keep a wider gap.
    let widest = items.iter().filter_map(|c| c.keeps.map(|k| k.0)).fold(r.clearance, f64::max);
    for (i, j) in crate::pcb::copper::near_pairs(items, widest) {
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
        hole(&mut out, Item::Via(v.id), v.at, v.drill, Some((v.dia - v.drill) / 2.0));
    }
    for f in &board.footprints {
        for (i, pad) in f.pads.iter().enumerate() {
            if pad.through() {
                let ring = (!pad.bare_hole()).then(|| (pad.size.x.min(pad.size.y) - pad.drill) / 2.0);
                hole(&mut out, Item::Pad(f.id, i as u16), f.pad_center(i), pad.drill, ring);
            }
        }
    }

    // Copper near or past the board's edge.
    let edge = &board.outline;
    if edge.len() >= 3 {
        let n = edge.len();
        for cu in items {
            let need = cu.keeps.map(|k| k.1).unwrap_or(r.edge_clearance);
            let mut worst: Option<(f64, P)> = None;
            for k in 0..n {
                let (a, b) = (edge[k], edge[(k + 1) % n]);
                let seg = BBox::of(&[a, b]);
                for (part, bb) in cu.parts.iter().zip(&cu.boxes) {
                    // Far pieces of a big pour needn't be measured.
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
    } else {
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
    for i in 0..fps.len() {
        for j in i + 1..fps.len() {
            let (a, b) = (&fps[i], &fps[j]);
            let both_through = a.pads.iter().all(|p| p.through()) && b.pads.iter().all(|p| p.through());
            if a.side != b.side && !both_through {
                continue;
            }
            let (ba, bb) = (a.bbox(), b.bbox());
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

    out.sort_by_key(|p| (p.severity == Severity::Warning, p.title.clone()));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pcb::copper::analyse;

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
