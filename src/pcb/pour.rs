//! Pouring copper: each pour fills its outline (or, for a plane, the whole
//! board) with copper, less a clearance round everything of other nets, the
//! board's edge, and pours poured before it. Pads of its own net are joined
//! solid or by thermal relief spokes; necks too narrow and pieces that touch
//! nothing of the net are taken out.
//!
//! Every cut is drawn just outside the true round shape it stands for, so
//! poured copper is never closer than the clearance, by the rule check's
//! own measure.

use crate::pcb::copper::{self, Item};
use crate::pcb::geom::*;
use crate::pcb::model::*;
use i_overlay::core::fill_rule::FillRule;
use i_overlay::core::overlay_rule::OverlayRule;
use i_overlay::float::single::SingleFloatOverlay;
use i_overlay::float::slice::FloatSlice;
use i_overlay::mesh::float::outline::offset::OutlineOffset;
use i_overlay::mesh::float::style::{LineJoin, OutlineStyle};

type Pt = [f64; 2];
type Ring = Vec<Pt>;
type Shape = Vec<Ring>;

/// How much further out than the rule every cut goes, against rounding.
const MARGIN: f64 = 0.0005;
/// The size of the tiles a pour is cut into for checking.
const TILE: f64 = 2.0;

fn to(ring: &[P]) -> Ring {
    ring.iter().map(|q| [q.x, q.y]).collect()
}

fn from(ring: &[Pt]) -> Vec<P> {
    ring.iter().map(|q| p(q[0], q[1])).collect()
}

fn ccw(mut ring: Vec<P>) -> Vec<P> {
    if ring_area(&ring) < 0.0 {
        ring.reverse();
    }
    ring
}

/// A blob grown by `by`, its round parts drawn round the outside of the
/// true circle so nothing inside the ring is nearer than `by`.
fn cut(b: &Blob, by: f64) -> Ring {
    let r = b.r + by;
    if r <= 0.0 {
        return to(&ccw(b.core.clone()));
    }
    let steps = circle_steps(r) as f64;
    let r_out = r / (std::f64::consts::PI / steps).cos() + MARGIN;
    to(&ccw(grown_ring(&b.core, r_out)))
}

fn rings_of(shapes: &[Shape]) -> Vec<Ring> {
    shapes.iter().flatten().cloned().collect()
}

fn overlay(a: &[Ring], b: &[Ring], rule: OverlayRule) -> Vec<Shape> {
    if a.is_empty() {
        return Vec::new();
    }
    if b.is_empty() && rule != OverlayRule::Intersect {
        return a.to_vec().overlay(&Vec::<Ring>::new(), OverlayRule::Subject, FillRule::NonZero);
    }
    a.to_vec().overlay(&b.to_vec(), rule, FillRule::NonZero)
}

/// Grows (or with a negative `by`, shrinks) shapes, with round corners.
fn offset(shapes: &[Shape], by: f64) -> Vec<Shape> {
    if shapes.is_empty() || by == 0.0 {
        return shapes.to_vec();
    }
    let style = OutlineStyle::new(by).line_join(LineJoin::Round(0.1));
    shapes.to_vec().outline(&style)
}

fn shape_area(s: &Shape) -> f64 {
    s.iter().map(|r| ring_area(&from(r))).sum()
}

fn in_shape(q: P, s: &[Vec<P>]) -> bool {
    !s.is_empty() && point_in_ring(q, &s[0]) && !s[1..].iter().any(|h| point_in_ring(q, h))
}

/// Everything about the board a pour on `layer` needs to keep clear of or join.
struct Around {
    /// Fixed copper (no pours), with its holes.
    items: Vec<(copper::Cu, f64)>,
    bare_holes: Vec<Blob>,
    /// Keepouts that keep pours out, board's and parts', each as convex pieces.
    keepouts: Vec<(PlacedKeepout, Vec<Vec<P>>)>,
}

fn around(board: &Board) -> Around {
    let drill = |item: Item| -> f64 {
        match item {
            Item::Via(id) => board.via(id).map(|v| v.drill).unwrap_or(0.0),
            Item::Pad(f, i) => board.fp(f).and_then(|f| f.pads.get(i as usize)).map(|p| if p.through() { p.drill } else { 0.0 }).unwrap_or(0.0),
            _ => 0.0,
        }
    };
    let items = copper::collect_fixed(board).into_iter().map(|c| {
        let d = drill(c.item);
        (c, d)
    }).collect();
    let mut bare_holes = Vec::new();
    for f in &board.footprints {
        for (i, pad) in f.pads.iter().enumerate() {
            if pad.bare_hole() {
                bare_holes.push(Blob::dot(f.pad_center(i), pad.drill));
            }
        }
    }
    let keepouts = board.placed_keepouts().into_iter().filter(|k| k.restrict.pours).map(|k| {
        let pieces = k.pieces();
        (k, pieces)
    }).collect();
    Around { items, bare_holes, keepouts }
}

/// What a poured pour keeps for those poured after it on its layer.
type Done = (u8, u32, f64, Vec<Shape>);

fn done_of(pour: &Pour, islands: &[Island]) -> Done {
    (pour.layer, pour.net, pour.clearance, islands.iter().map(|is| is.rings.iter().map(|r| to(r)).collect()).collect())
}

/// A fingerprint of everything a pour's shape is worked out from: its own
/// settings, the board's edge, the copper on its layer and the pours poured
/// before it there (through `before`, their fingerprints in turn).
fn signature(board: &Board, pour: &Pour, env: &Around, before: u64) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    let f = |h: &mut std::collections::hash_map::DefaultHasher, v: f64| v.to_bits().hash(h);
    let pts = |h: &mut std::collections::hash_map::DefaultHasher, ring: &[P]| {
        ring.len().hash(h);
        for q in ring {
            f(h, q.x);
            f(h, q.y);
        }
    };
    (pour.layer, pour.net, pour.plane, pour.connect as u8, pour.remove_islands, before).hash(&mut h);
    for v in [pour.clearance, pour.edge, pour.spoke, pour.gap, pour.min_area, pour.neck, board.rules.clearance] {
        f(&mut h, v);
    }
    for g in &board.rules.gaps {
        (g.a, g.b).hash(&mut h);
        f(&mut h, g.mm);
    }
    pts(&mut h, pour.area(board));
    pts(&mut h, &board.outline);
    for hole in &env.bare_holes {
        pts(&mut h, &hole.core);
        f(&mut h, hole.r);
    }
    for (k, _) in env.keepouts.iter().filter(|(k, _)| k.applies_to(pour.layer)) {
        pts(&mut h, &k.pts);
    }
    for (cu, drill) in &env.items {
        if !cu.on.has(pour.layer) {
            continue;
        }
        cu.item.hash(&mut h);
        cu.net.hash(&mut h);
        f(&mut h, *drill);
        pts(&mut h, &cu.anchors);
        pts(&mut h, &cu.blob.core);
        f(&mut h, cu.blob.r);
        for part in cu.parts.iter() {
            pts(&mut h, &part.core);
            f(&mut h, part.r);
        }
    }
    h.finish() | 1
}

/// Pours every pour on the board, in order. Pours on different layers don't
/// affect each other, so each layer's go on a thread of their own; a pour
/// whose surroundings are as they were when it last poured is left alone.
pub fn repour(board: &mut Board) {
    if board.pours.is_empty() {
        return;
    }
    let env = around(board);
    let mut order: Vec<usize> = (0..board.pours.len()).collect();
    order.sort_by_key(|&i| (board.pours[i].plane, board.pours[i].order, board.pours[i].id));
    let mut layers: Vec<(u8, Vec<usize>)> = Vec::new();
    for i in order {
        let layer = board.pours[i].layer;
        match layers.iter_mut().find(|(l, _)| *l == layer) {
            Some((_, group)) => group.push(i),
            None => layers.push((layer, vec![i])),
        }
    }

    // (pour, its new fill and fingerprint), or None where it stands as it was.
    type Poured = Vec<(usize, Option<(Vec<Island>, u64)>)>;
    let pour_layer = |group: &[usize]| -> Poured {
        let mut done: Vec<Done> = Vec::new();
        let mut before = 0u64;
        let mut out: Poured = Vec::new();
        for &i in group {
            let pour = &board.pours[i];
            let sig = signature(board, pour, &env, before);
            before = sig;
            if pour.sig == sig {
                done.push(done_of(pour, &pour.fill));
                out.push((i, None));
                continue;
            }
            let shapes = pour_one(board, pour, &env, &done);
            let islands = finish(board, pour, &env, &shapes);
            done.push(done_of(pour, &islands));
            out.push((i, Some((islands, sig))));
        }
        out
    };
    let results: Vec<Poured> = if layers.len() < 2 {
        layers.iter().map(|(_, g)| pour_layer(g)).collect()
    } else {
        std::thread::scope(|scope| {
            let jobs: Vec<_> = layers.iter().map(|(_, g)| scope.spawn(|| pour_layer(g))).collect();
            jobs.into_iter().map(|j| j.join().expect("a pour panicked")).collect()
        })
    };
    for (i, poured) in results.into_iter().flatten() {
        if let Some((islands, sig)) = poured {
            board.pours[i].fill = islands;
            board.pours[i].sig = sig;
        }
    }
}

fn pour_one(board: &Board, pour: &Pour, env: &Around, done: &[(u8, u32, f64, Vec<Shape>)]) -> Vec<Shape> {
    let layer = pour.layer;
    let area_ring = pour.area(board);
    if area_ring.len() < 3 || board.outline.len() < 3 {
        return Vec::new();
    }
    // Where it may go: its outline, on the board.
    let mut area = overlay(&[to(&ccw(area_ring.to_vec()))], &[to(&ccw(board.outline.clone()))], OverlayRule::Intersect);

    // Clear of other nets, holes and the edge.
    let near = if pour.plane { board.rules.clearance.max(0.0) } else { pour.clearance };
    let mut away: Vec<Ring> = Vec::new();
    let o = &board.outline;
    for k in 0..o.len() {
        away.push(cut(&Blob::line(o[k], o[(k + 1) % o.len()], 0.0), pour.edge));
    }
    for h in &env.bare_holes {
        away.push(cut(h, near));
    }
    let mut reliefs: Vec<Ring> = Vec::new();
    let mut spokes: Vec<Ring> = Vec::new();
    for (cu, drill) in &env.items {
        if !cu.on.has(layer) {
            continue;
        }
        let ours = pour.net != 0 && cu.net == pour.net;
        if !ours {
            // How far a pour keeps from this kind of copper: what the rules ask of the pair
            // (the general clearance where the table doesn't list it), if more than the pour's own.
            let near = near.max(board.rules.gap(Kind::Pour, cu.kind()));
            for part in cu.parts.iter() {
                away.push(cut(part, near));
            }
            if pour.plane && *drill > 0.0 {
                away.push(cut(&Blob::dot(cu.anchors[0], *drill), pour.clearance));
            }
            continue;
        }
        // A pad of its own net, joined by spokes.
        if pour.connect == Connect::Relief && matches!(cu.item, Item::Pad(..)) {
            let b = &cu.blob;
            reliefs.push(cut(b, pour.gap));
            let c = cu.anchors[0];
            let u = if b.core.len() >= 2 { b.core[1].sub(b.core[0]) } else { p(1.0, 0.0) };
            let u = u.scale(1.0 / u.len().max(1e-12));
            let v = p(-u.y, u.x);
            let ring = b.ring();
            for d in [u, v, u.scale(-1.0), v.scale(-1.0)] {
                let reach = ring.iter().map(|q| q.sub(c).dot(d)).fold(0.0, f64::max) + pour.gap + pour.spoke;
                let side = p(-d.y, d.x).scale(pour.spoke / 2.0);
                let end = c.add(d.scale(reach));
                spokes.push(to(&ccw(vec![c.add(side), end.add(side), end.sub(side), c.sub(side)])));
            }
        }
    }
    // Keepouts that keep pours out, on this layer or every one.
    for (k, pieces) in &env.keepouts {
        if k.applies_to(layer) {
            for piece in pieces {
                away.push(cut(&Blob { core: piece.clone(), r: 0.0 }, near));
            }
        }
    }
    // Pours poured before this one, of other nets.
    for (l, net, gap, shapes) in done {
        if *l == layer && (*net != pour.net || pour.net == 0) {
            // The offset's round corners are drawn a little inside the true curve, so go 1% further.
            let apart = near.max(*gap).max(board.rules.gap(Kind::Pour, Kind::Pour));
            let grown = offset(shapes, apart * 1.01 + MARGIN);
            away.extend(rings_of(&grown));
        }
    }
    area = overlay(&rings_of(&area), &away, OverlayRule::Difference);
    let base = area.clone();
    let mut fill = if reliefs.is_empty() { base.clone() } else { overlay(&rings_of(&base), &reliefs, OverlayRule::Difference) };
    if pour.neck > 0.0 {
        fill = offset(&offset(&fill, -pour.neck / 2.0), pour.neck / 2.0);
        // Opening can only take away; keep it inside what was poured.
        fill = overlay(&rings_of(&fill), &rings_of(&base), OverlayRule::Intersect);
    }
    if !spokes.is_empty() {
        let joined = overlay(&spokes, &rings_of(&base), OverlayRule::Intersect);
        fill = overlay(&rings_of(&fill), &rings_of(&joined), OverlayRule::Union);
    }
    fill
}

/// The poured shapes as islands: those too small or touching nothing of
/// the net taken out, each cut into small triangles.
fn finish(board: &Board, pour: &Pour, env: &Around, shapes: &[Shape]) -> Vec<Island> {
    // Points that show an island touches its net.
    let mut marks: Vec<P> = Vec::new();
    for (cu, _) in &env.items {
        if cu.net == pour.net && cu.on.has(pour.layer) {
            marks.extend(cu.anchors.iter().copied());
            if let Item::Track(_) = cu.item
                && cu.anchors.len() == 2
            {
                marks.push(cu.anchors[0].lerp(cu.anchors[1], 0.5));
            }
            if matches!(cu.item, Item::Fill(_)) {
                marks.extend(cu.parts.iter().map(|b| b.center()));
            }
        }
    }
    let _ = board;
    let mut out = Vec::new();
    for s in shapes {
        let rings: Vec<Vec<P>> = s.iter().map(|r| from(r)).collect();
        if rings.is_empty() || rings[0].len() < 3 {
            continue;
        }
        let area = shape_area(s);
        if area <= 1e-9 || area < pour.min_area {
            continue;
        }
        if !pour.plane && pour.remove_islands && pour.net != 0 && !marks.iter().any(|q| in_shape(*q, &rings)) {
            continue;
        }
        let tris = triangles(s);
        let parts = Parts::new(tris.iter().map(|t| Blob { core: t.to_vec(), r: 0.0 }).collect());
        out.push(Island { rings, tris, area, parts });
    }
    out
}

/// Lines across a shape: one through each hole, and with `tile`, a grid.
fn cuts(s: &Shape, tile: Option<f64>) -> Vec<Ring> {
    let all: Vec<P> = s.iter().flat_map(|r| from(r)).collect();
    let bb = BBox::of(&all);
    let (y0, y1) = (bb.min.y - 1.0, bb.max.y + 1.0);
    let mut lines: Vec<Ring> = Vec::new();
    for h in &s[1..] {
        let x = BBox::of(&from(h)).center().x;
        lines.push(vec![[x, y0], [x, y1]]);
    }
    if let Some(t) = tile {
        let (x0, x1) = (bb.min.x - 1.0, bb.max.x + 1.0);
        let mut x = (bb.min.x / t).floor() * t + t;
        while x < bb.max.x {
            lines.push(vec![[x, y0], [x, y1]]);
            x += t;
        }
        let mut y = (bb.min.y / t).floor() * t + t;
        while y < bb.max.y {
            lines.push(vec![[x0, y], [x1, y]]);
            y += t;
        }
    }
    lines
}

/// A shape with holes as one ring: each hole joined to what's left of it
/// by a cut-in, a line out and back along the same path, as Gerber regions
/// are written. Holes go in from the leftmost, each joined by a line run
/// left from its leftmost corner to the nearest edge already in the ring.
pub fn fracture(rings: &[Vec<P>]) -> Vec<P> {
    let Some(outer) = rings.first() else { return Vec::new() };
    let mut ring = outer.clone();
    let mut holes: Vec<&Vec<P>> = rings[1..].iter().filter(|h| h.len() >= 3).collect();
    let left = |h: &Vec<P>| (0..h.len()).min_by(|a, b| h[*a].x.total_cmp(&h[*b].x).then(h[*a].y.total_cmp(&h[*b].y))).unwrap_or(0);
    holes.sort_by(|a, b| a[left(a)].x.total_cmp(&b[left(b)].x));
    for h in holes {
        let k = left(h);
        let q = h[k];
        // The nearest edge crossing the line run left from q.
        let n = ring.len();
        let mut best: Option<(f64, usize)> = None;
        for i in 0..n {
            let (a, b) = (ring[i], ring[(i + 1) % n]);
            if (a.y > q.y) == (b.y > q.y) {
                continue;
            }
            let x = a.x + (q.y - a.y) / (b.y - a.y) * (b.x - a.x);
            if x <= q.x + 1e-12 && best.is_none_or(|(bx, _)| x > bx) {
                best = Some((x, i));
            }
        }
        let Some((x, i)) = best else { continue };
        let at = p(x, q.y);
        let mut joined = Vec::with_capacity(n + h.len() + 4);
        joined.extend_from_slice(&ring[..=i]);
        joined.push(at);
        for j in 0..=h.len() {
            joined.push(h[(k + j) % h.len()]);
        }
        joined.push(at);
        joined.extend_from_slice(&ring[i + 1..]);
        ring = joined;
    }
    ring
}

/// Small triangles covering a shape, for drawing and checking.
fn triangles(s: &Shape) -> Vec<[P; 3]> {
    let lines = cuts(s, Some(TILE));
    let pieces = if lines.is_empty() { vec![s.clone()] } else { s.slice_by(&lines, FillRule::NonZero) };
    let mut out = Vec::new();
    for piece in &pieces {
        let mut flat: Vec<[f64; 2]> = Vec::new();
        let mut holes: Vec<usize> = Vec::new();
        for (k, r) in piece.iter().enumerate() {
            if k > 0 {
                holes.push(flat.len());
            }
            flat.extend(r.iter().copied());
        }
        let mut idx: Vec<usize> = Vec::new();
        earcut::Earcut::<f64>::new().earcut(flat.iter().copied(), &holes, &mut idx);
        for t in idx.chunks(3) {
            let q = |i: usize| p(flat[i][0], flat[i][1]);
            let (a, b, c) = (q(t[0]), q(t[1]), q(t[2]));
            let tri = if b.sub(a).cross(c.sub(a)) < 0.0 { [a, c, b] } else { [a, b, c] };
            out.push(tri);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pad(b: &mut Board, at: P, net: u32, drill: f64) -> u32 {
        let id = b.new_id();
        b.footprints.push(Footprint {
            id,
            def: "TP".into(),
            refdes: format!("TP{id}"),
            value: String::new(),
            at,
            rot: 0.0,
            side: Side::Top,
            pads: vec![Pad { num: "1".into(), at: p(0.0, 0.0), size: p(1.5, 1.5), rot: 0.0, shape: PadShape::Circle, drill, net }],
            graphics: vec![],
            label_at: P::default(),
            label_height: None,
            locked: false,
            keepouts: vec![],
        });
        id
    }

    #[test]
    fn a_pour_keeps_clear_and_joins_its_pads() {
        let mut b = Board::default();
        let (gnd, sig) = (b.net("GND"), b.net("SIG"));
        pad(&mut b, p(10.0, 10.0), gnd, 0.0);
        pad(&mut b, p(20.0, 10.0), sig, 0.0);
        b.tracks.push(Track { mid: None, id: 900, a: p(20.0, 10.0), b: p(30.0, 30.0), w: 0.25, layer: TOP, net: sig });
        let id = b.new_id();
        let mut pour = Pour::new(id, TOP, gnd, rect_outline(p(0.0, 0.0), p(50.0, 40.0)), &b.rules);
        pour.connect = Connect::Relief;
        b.pours.push(pour);
        b.repour();
        let fill = &b.pours[0].fill;
        assert_eq!(fill.len(), 1, "one piece");
        assert!(fill[0].area > 1500.0, "{}", fill[0].area);
        let c = copper::analyse(&b);
        let found = crate::pcb::drc::check(&b, &c);
        let bad: Vec<_> = found.iter().filter(|p| p.severity == crate::pcb::drc::Severity::Error).collect();
        assert!(bad.is_empty(), "{bad:?}");
        // The GND pad is joined to the pour, so nothing's left to connect.
        assert!(c.rats.is_empty(), "{:?}", c.rats);
    }

    /// A board with a GND pour on the top and another on the bottom.
    fn two_pours() -> Board {
        let mut b = Board::default();
        let (gnd, sig) = (b.net("GND"), b.net("SIG"));
        // A through-hole pad, so GND is there on both layers for the pours to join.
        pad(&mut b, p(10.0, 10.0), gnd, 0.8);
        b.tracks.push(Track { mid: None, id: 900, a: p(20.0, 10.0), b: p(30.0, 30.0), w: 0.25, layer: TOP, net: sig });
        for layer in [TOP, BOTTOM] {
            let id = b.new_id();
            b.pours.push(Pour::new(id, layer, gnd, Vec::new(), &b.rules));
        }
        b
    }

    #[test]
    fn a_pour_is_left_alone_while_its_surroundings_stand() {
        let mut b = two_pours();
        b.repour();
        let (areas, sigs) = (b.pours.iter().map(|p| p.copper_area()).collect::<Vec<_>>(), b.pours.iter().map(|p| p.sig).collect::<Vec<_>>());
        assert!(areas.iter().all(|a| *a > 100.0) && sigs.iter().all(|s| *s != 0), "{areas:?} {sigs:?}");
        // Poured again with nothing changed: nothing is redone.
        b.repour();
        assert_eq!(sigs, b.pours.iter().map(|p| p.sig).collect::<Vec<_>>());
        assert_eq!(areas, b.pours.iter().map(|p| p.copper_area()).collect::<Vec<_>>());
        // Copper of another net on the top pours it again there, and only there.
        let sig = b.net("SIG");
        b.tracks.push(Track { mid: None, id: 901, a: p(5.0, 30.0), b: p(40.0, 30.0), w: 1.0, layer: TOP, net: sig });
        b.repour();
        assert_ne!(sigs[0], b.pours[0].sig);
        assert!(b.pours[0].copper_area() < areas[0] - 10.0, "the top pour gave way");
        assert_eq!(sigs[1], b.pours[1].sig);
        assert_eq!(areas[1], b.pours[1].copper_area());
    }

    fn keepout(b: &mut Board, layer: Option<u8>, restrict: Restrict) -> u32 {
        let id = b.new_id();
        b.keepouts.push(Keepout { id, layer, pts: rect_outline(p(15.0, 12.0), p(25.0, 22.0)), restrict });
        id
    }

    fn poured_at(b: &Board, pour: usize, q: P) -> bool {
        b.pours[pour].fill.iter().any(|i| in_shape(q, &i.rings))
    }

    #[test]
    fn a_pour_keeps_out_of_a_keepout_and_clear_of_its_edge() {
        let mut b = two_pours();
        b.repour();
        let before: Vec<f64> = b.pours.iter().map(|p| p.copper_area()).collect();
        assert!(poured_at(&b, 0, p(20.0, 17.0)) && poured_at(&b, 1, p(20.0, 17.0)));
        keepout(&mut b, Some(TOP), Restrict::default());
        b.repour();
        // The top pour has a hole where the keepout is; the bottom pour doesn't.
        assert!(!poured_at(&b, 0, p(20.0, 17.0)), "nothing poured in the keepout");
        assert!(b.pours[0].copper_area() < before[0] - 90.0, "{} -> {}", before[0], b.pours[0].copper_area());
        assert!(poured_at(&b, 1, p(20.0, 17.0)) && (b.pours[1].copper_area() - before[1]).abs() < 1e-6);
        // Held off by the pour's clearance (0.25) from its edge, and no further.
        assert!(!poured_at(&b, 0, p(14.9, 17.0)), "0.1 from the edge is too near");
        assert!(poured_at(&b, 0, p(14.5, 17.0)), "0.5 from the edge is fine");
        // The pour is clear of it by the rule check's own measure too. (The
        // board's SIG track crosses the keepout, and is rightly reported.)
        let found = crate::pcb::drc::check(&b, &copper::analyse(&b));
        let in_it: Vec<_> = found.iter().filter(|p| p.title == "In a keepout").collect();
        assert!(in_it.iter().all(|p| p.items.iter().all(|i| matches!(i, Item::Track(_)))), "{in_it:?}");
        assert!(found.iter().all(|p| !(p.title == "Too close" && p.items.iter().any(|i| matches!(i, Item::Pour(..))))), "{found:?}");
    }

    #[test]
    fn a_keepout_on_every_layer_holds_every_pour_off_and_one_that_allows_pours_holds_none() {
        let mut b = two_pours();
        b.repour();
        let k = keepout(&mut b, None, Restrict::default());
        b.repour();
        assert!(!poured_at(&b, 0, p(20.0, 17.0)) && !poured_at(&b, 1, p(20.0, 17.0)));
        b.keepouts.iter_mut().find(|x| x.id == k).unwrap().restrict = Restrict { pours: false, ..Restrict::default() };
        b.repour();
        assert!(poured_at(&b, 0, p(20.0, 17.0)) && poured_at(&b, 1, p(20.0, 17.0)), "a keepout that lets pours in isn't a hole");
    }

    #[test]
    fn a_pour_is_poured_again_when_a_keepout_moves_and_not_when_nothing_changes() {
        let mut b = two_pours();
        let k = keepout(&mut b, Some(TOP), Restrict::default());
        b.repour();
        let sig = b.pours[0].sig;
        b.repour();
        assert_eq!(sig, b.pours[0].sig, "nothing changed, nothing redone");
        b.keepouts.iter_mut().find(|x| x.id == k).unwrap().pts = rect_outline(p(30.0, 5.0), p(35.0, 9.0));
        b.repour();
        assert_ne!(sig, b.pours[0].sig);
        assert!(poured_at(&b, 0, p(20.0, 17.0)) && !poured_at(&b, 0, p(32.0, 7.0)));
        // A keepout on another layer doesn't touch the bottom pour's fingerprint.
        let bottom = b.pours[1].sig;
        b.keepouts.iter_mut().find(|x| x.id == k).unwrap().pts = rect_outline(p(30.0, 5.0), p(36.0, 9.0));
        b.repour();
        assert_eq!(bottom, b.pours[1].sig);
    }

    #[test]
    fn a_keepout_on_a_part_keeps_pours_out_and_goes_with_the_part() {
        let mut b = two_pours();
        let f = pad(&mut b, p(20.0, 30.0), 0, 0.0);
        b.fp_mut(f).unwrap().keepouts.push(Keepout { id: 0, layer: Some(TOP), pts: rect_outline(p(-3.0, -3.0), p(3.0, 3.0)), restrict: Restrict::default() });
        b.repour();
        assert!(!poured_at(&b, 0, p(20.0, 30.0)) && !poured_at(&b, 0, p(22.0, 32.0)));
        b.fp_mut(f).unwrap().at = p(40.0, 10.0);
        b.repour();
        assert!(poured_at(&b, 0, p(20.0, 30.0)), "where it was, poured again");
        assert!(!poured_at(&b, 0, p(41.0, 11.0)), "where it is");
    }

    #[test]
    fn a_pour_keeps_the_gap_the_table_gives_for_each_kind_of_copper() {
        let mut b = Board::default();
        let (gnd, sig) = (b.net("GND"), b.net("SIG"));
        b.rules.clearance = 0.1;
        b.rules.gaps = vec![Gap { a: Kind::Pour, b: Kind::Track, mm: 0.5 }, Gap { a: Kind::Pour, b: Kind::SmdPad, mm: 0.8 }];
        b.tracks.push(Track { mid: None, id: 900, a: p(10.0, 20.0), b: p(40.0, 20.0), w: 0.25, layer: TOP, net: sig });
        pad(&mut b, p(25.0, 30.0), sig, 0.0);
        let id = b.new_id();
        let mut pour = Pour::new(id, TOP, gnd, Vec::new(), &b.rules);
        pour.clearance = 0.2;
        pour.remove_islands = false;
        b.pours.push(pour);
        b.repour();
        // The track's edge is at y = 20.125: held off by 0.5, not by the pour's own 0.2.
        assert!(!poured_at(&b, 0, p(25.0, 20.125 + 0.45)), "0.45 from the track is too near");
        assert!(poured_at(&b, 0, p(25.0, 20.125 + 0.55)), "0.55 is not");
        // A surface pad (radius 0.75): the table asks 0.8.
        assert!(!poured_at(&b, 0, p(25.0, 30.0 + 0.75 + 0.75)), "0.75 from the pad is too near");
        assert!(poured_at(&b, 0, p(25.0, 30.0 + 0.75 + 0.85)), "0.85 is not");
        // And the rule check, asking the same, finds the pour clear of both.
        let found = crate::pcb::drc::check(&b, &copper::analyse(&b));
        assert!(found.iter().all(|p| p.title != "Too close"), "{found:?}");
    }

    #[test]
    fn two_pours_keep_the_gap_the_table_gives_between_pours() {
        let mut b = Board::default();
        let (gnd, vbat) = (b.net("GND"), b.net("VBAT"));
        b.rules.gaps = vec![Gap { a: Kind::Pour, b: Kind::Pour, mm: 0.889 }];
        // GND's corners at (30, 5) and (30, 35) are inside VBAT's area, where
        // VBAT is cut back from them by a rounded offset.
        for (net, lo, hi) in [(gnd, (5.0, 5.0), (30.0, 35.0)), (vbat, (20.0, 0.0), (50.0, 40.0))] {
            let id = b.new_id();
            let mut pour = Pour::new(id, TOP, net, rect_outline(p(lo.0, lo.1), p(hi.0, hi.1)), &b.rules);
            pour.remove_islands = false;
            b.pours.push(pour);
        }
        b.repour();
        assert!(b.pours.iter().all(|p| p.copper_area() > 100.0), "both poured");
        let found = crate::pcb::drc::check(&b, &copper::analyse(&b));
        assert!(found.iter().all(|p| p.title != "Too close"), "{:?}", found.iter().filter(|p| p.title == "Too close").collect::<Vec<_>>());
        // And not further than it had to be, by much: the first pour reaches to within a hair of the gap.
        let edge = b.pours[0].fill.iter().flat_map(|i| i.rings[0].iter().map(|q| q.x)).fold(f64::MIN, f64::max);
        let first = b.pours[1].fill.iter().flat_map(|i| i.rings[0].iter().map(|q| q.x)).fold(f64::MAX, f64::min);
        assert!(first - edge < 0.889 + 0.05, "gap {}", first - edge);
    }

    #[test]
    fn emptied_pours_keep_their_areas_and_pour_again_the_same() {
        let mut b = two_pours();
        b.repour();
        let poured: Vec<_> = b.pours.iter().map(|p| p.fill.clone()).collect();
        let settings: Vec<_> = b.pours.iter().map(|p| (p.id, p.layer, p.net, p.outline.clone(), p.clearance)).collect();
        assert!(b.clear_fills());
        assert!(!b.clear_fills(), "nothing left to take out");
        assert!(b.pours.iter().all(|p| p.fill.is_empty()));
        assert_eq!(settings, b.pours.iter().map(|p| (p.id, p.layer, p.net, p.outline.clone(), p.clearance)).collect::<Vec<_>>());
        // With no copper they're not in the way of a track laid right through them…
        let sig = b.net("SIG");
        b.tracks.push(Track { mid: None, id: 950, a: p(2.0, 12.0), b: p(40.0, 12.0), w: 0.25, layer: TOP, net: sig });
        let c = copper::analyse(&b);
        assert!(c.items.iter().all(|cu| !matches!(cu.item, Item::Pour(..))));
        // …and poured again they work round it.
        b.repour();
        assert!(b.pours[0].copper_area() < poured[0].iter().map(|i| i.area).sum::<f64>());
        b.tracks.pop();
        b.repour();
        assert_eq!(poured, b.pours.iter().map(|p| p.fill.clone()).collect::<Vec<_>>());
    }

    #[test]
    fn pouring_layers_side_by_side_gives_what_pouring_each_alone_does() {
        let mut both = two_pours();
        both.repour();
        for k in 0..2 {
            let mut alone = two_pours();
            alone.pours.remove(1 - k);
            alone.repour();
            assert_eq!(alone.pours[0].fill, both.pours[k].fill, "layer {k}");
        }
    }

    #[test]
    fn islands_with_nothing_of_the_net_go() {
        let mut b = Board::default();
        let (gnd, sig) = (b.net("GND"), b.net("SIG"));
        pad(&mut b, p(5.0, 5.0), gnd, 0.0);
        // A wall of SIG across the board cuts the pour in two.
        b.tracks.push(Track { mid: None, id: 900, a: p(25.0, -1.0), b: p(25.0, 41.0), w: 1.0, layer: TOP, net: sig });
        let id = b.new_id();
        b.pours.push(Pour::new(id, TOP, gnd, Vec::new(), &b.rules));
        b.repour();
        assert_eq!(b.pours[0].fill.len(), 1);
        b.pours[0].remove_islands = false;
        b.repour();
        assert_eq!(b.pours[0].fill.len(), 2);
    }

    #[test]
    fn shrinking_takes_out_necks() {
        let square: Vec<Shape> = vec![vec![vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]]]];
        let small = offset(&square, -1.0);
        assert!((shape_area(&small[0]) - 64.0).abs() < 0.1, "{}", shape_area(&small[0]));
    }

    #[test]
    fn holes_join_the_outline_by_cut_ins() {
        let hole = |a: P, b: P| {
            let mut h = rect_outline(a, b);
            h.reverse();
            h
        };
        let rings = vec![rect_outline(p(0.0, 0.0), p(10.0, 10.0)), hole(p(4.0, 4.0), p(6.0, 6.0)), hole(p(7.0, 1.0), p(8.0, 2.0))];
        let one = fracture(&rings);
        // The cut-ins add no area: what's left is the square less its holes.
        assert!((ring_area(&one) - 95.0).abs() < 1e-9, "{}", ring_area(&one));
        assert!(!point_in_ring(p(5.0, 5.0), &one) && point_in_ring(p(2.0, 5.0), &one));
    }
}
