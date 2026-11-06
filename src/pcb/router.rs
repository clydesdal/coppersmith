//! The interactive router's thinking: which way a track may go from where it
//! is to where the pointer is, given the corner style and what's in the way.
//!
//! Modelled on Altium's interactive router:
//! - Corner styles: 45°, 90° and any angle (arcs come later). Space flips
//!   which way the corner goes.
//! - Conflict modes: Ignore obstacles (lay it anyway, the rule check shows the
//!   problem), Walk around obstacles (find a clear path round them), Stop at
//!   first obstacle (lay it as far as it goes clear). Push comes later.
//!
//! Walking round is an A* search over a fine lattice anchored on the route's
//! start, with every lattice point kept clear of other nets' copper and the
//! board edge, followed by a pass that pulls the path straight wherever an
//! exact check says a straighter path is clear.

use crate::pcb::copper::{Copper, Item};
use crate::pcb::geom::*;
use crate::pcb::model::*;
use std::cmp::Ordering;
use std::collections::BinaryHeap;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Corner {
    D45,
    /// 45° routing with each corner rounded into a tangent arc.
    Arc45,
    D90,
    /// 90° routing with each corner rounded into a tangent arc.
    Arc90,
    Any,
}

impl Corner {
    pub fn name(self) -> &'static str {
        match self {
            Corner::D45 => "45°",
            Corner::Arc45 => "45° with arc",
            Corner::D90 => "90°",
            Corner::Arc90 => "90° with arc",
            Corner::Any => "Any angle",
        }
    }
    /// Altium's order for Shift+Space.
    pub fn next(self) -> Corner {
        match self {
            Corner::D45 => Corner::Arc45,
            Corner::Arc45 => Corner::D90,
            Corner::D90 => Corner::Arc90,
            Corner::Arc90 => Corner::Any,
            Corner::Any => Corner::D45,
        }
    }
    /// The straight style the path is planned in before corners are rounded.
    pub fn base(self) -> Corner {
        match self {
            Corner::Arc45 => Corner::D45,
            Corner::Arc90 => Corner::D90,
            c => c,
        }
    }
    pub fn arcs(self) -> bool {
        matches!(self, Corner::Arc45 | Corner::Arc90)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Conflict {
    Ignore,
    Walkaround,
    Push,
    HugPush,
    Stop,
}

impl Conflict {
    pub fn name(self) -> &'static str {
        match self {
            Conflict::Ignore => "Ignore obstacles",
            Conflict::Walkaround => "Walk around",
            Conflict::Push => "Push",
            Conflict::HugPush => "Hug and push",
            Conflict::Stop => "Stop at first obstacle",
        }
    }
    pub fn next(self) -> Conflict {
        match self {
            Conflict::Ignore => Conflict::Walkaround,
            Conflict::Walkaround => Conflict::Push,
            Conflict::Push => Conflict::HugPush,
            Conflict::HugPush => Conflict::Stop,
            Conflict::Stop => Conflict::Ignore,
        }
    }
}

/// Something a new track has to keep `gap` away from.
#[derive(Clone, Debug)]
pub struct Obstacle {
    pub blob: Blob,
    pub bbox: BBox,
    pub gap: f64,
    pub item: Option<Item>,
    /// Whose copper it is; the board edge belongs to no net (`EDGE`).
    pub net: u32,
    /// A track, which Push may move.
    pub movable: bool,
}

pub const EDGE: u32 = u32::MAX;

/// What the router would lay from the route's end towards the pointer.
#[derive(Clone, Debug, Default)]
pub struct Plan {
    /// The corners, starting where the route is now.
    pub pts: Vec<P>,
    /// For each piece between corners, the middle of its arc if it's one.
    /// Empty when every piece is straight.
    pub mids: Vec<Option<P>>,
    /// It couldn't get all the way to the pointer.
    pub blocked: bool,
    /// What it runs into, for Ignore mode to show.
    pub hits: Vec<Item>,
    /// Other nets' tracks pushed aside to make room.
    pub pushed: Vec<Moved>,
    /// Other nets' vias pushed aside: which, and where to.
    pub vias: Vec<(u32, P)>,
    /// Something worth saying, like a push that couldn't be done.
    pub note: Option<&'static str>,
}

impl Plan {
    pub fn end(&self) -> Option<P> {
        self.pts.last().copied()
    }
    /// The middle of piece `i`'s arc, if it's an arc.
    pub fn mid(&self, i: usize) -> Option<P> {
        self.mids.get(i).copied().flatten()
    }
    /// The pieces, as (from, to, arc middle).
    pub fn pieces(&self) -> Vec<(P, P, Option<P>)> {
        self.pts.windows(2).enumerate().map(|(i, s)| (s[0], s[1], self.mid(i))).collect()
    }
}

/// Everything on a layer a track must keep clear of, whatever its net:
/// copper, plain holes and the board edge.
pub fn obstacles_all(board: &Board, copper: &Copper, layer: u8) -> Vec<Obstacle> {
    let r = &board.rules;
    let mut out: Vec<Obstacle> = copper
        .items
        .iter()
        .filter(|c| c.on.has(layer))
        .flat_map(|c| {
            // An arc is several pieces; arcs aren't pushed (yet), only straight tracks.
            let movable = matches!(c.item, Item::Track(_)) && c.parts.len() == 1;
            c.parts.iter().map(move |part| Obstacle {
                blob: part.clone(),
                bbox: part.bbox(),
                gap: r.clearance,
                item: Some(c.item),
                net: c.net,
                movable,
            })
        })
        .collect();
    // Plain holes have no copper but still can't be routed over.
    for f in &board.footprints {
        for (i, pad) in f.pads.iter().enumerate() {
            if pad.bare_hole() {
                let blob = Blob::dot(f.pad_center(i), pad.drill);
                out.push(Obstacle {
                    bbox: blob.bbox(),
                    blob,
                    gap: r.clearance,
                    item: Some(Item::Pad(f.id, i as u16)),
                    net: EDGE,
                    movable: false,
                });
            }
        }
    }
    let o = &board.outline;
    for i in 0..o.len() {
        let blob = Blob::line(o[i], o[(i + 1) % o.len()], 0.0);
        out.push(Obstacle { bbox: blob.bbox(), blob, gap: r.edge_clearance, item: None, net: EDGE, movable: false });
    }
    out
}

/// What a track of `net` has to keep clear of. Copper with no net is in
/// everyone's way, its own included.
pub fn for_net(all: &[Obstacle], net: u32) -> Vec<Obstacle> {
    all.iter().filter(|o| o.net != net || net == 0).cloned().collect()
}

/// Other nets' copper on this layer, and the board edge.
#[cfg(test)]
pub fn obstacles(board: &Board, copper: &Copper, layer: u8, net: u32) -> Vec<Obstacle> {
    for_net(&obstacles_all(board, copper, layer), net)
}

/// The legs from `a` to `b` in a corner style. `flip` swaps which comes first.
pub fn legs(a: P, b: P, corner: Corner, flip: bool) -> Vec<P> {
    let corner = corner.base();
    let d = b.sub(a);
    let (ax, ay) = (d.x.abs(), d.y.abs());
    if a.same(b) {
        return vec![a];
    }
    let straight = ax < 1e-9 || ay < 1e-9;
    match corner {
        Corner::Any => vec![a, b],
        _ if straight => vec![a, b],
        Corner::Arc45 | Corner::Arc90 => unreachable!(),
        Corner::D90 => {
            let corner = if flip { p(a.x, b.y) } else { p(b.x, a.y) };
            vec![a, corner, b]
        }
        Corner::D45 => {
            if (ax - ay).abs() < 1e-9 {
                return vec![a, b];
            }
            let m = ax.min(ay);
            let diag = p(m * d.x.signum(), m * d.y.signum());
            let corner = if flip { a.add(diag) } else { b.sub(diag) };
            vec![a, corner.rounded(), b]
        }
    }
}

fn near<'a>(obs: &'a [Obstacle], area: BBox) -> impl Iterator<Item = &'a Obstacle> {
    obs.iter().filter(move |o| o.bbox.grown(o.gap).touches(&area))
}

/// How much tighter the router is than the rule check, so that rounding a
/// corner to the nanometre can never tip a clear track into a violation.
/// The rule check allows 1e-6 mm under the clearance; the router allows none.
const SURE: f64 = 1e-7;

/// The first obstacle a track from `a` to `b` comes too close to.
pub fn clash(obs: &[Obstacle], a: P, b: P, w: f64) -> Option<&Obstacle> {
    let track = Blob::line(a, b, w);
    near(obs, track.bbox()).find(|o| track.gap(&o.blob).0 < o.gap + SURE)
}

pub fn clear(obs: &[Obstacle], pts: &[P], w: f64) -> bool {
    pts.windows(2).all(|s| clash(obs, s[0], s[1], w).is_none())
}

/// An arc piece from `a` through `m` to `b`, `w` wide, is clear.
pub fn clear_arc(obs: &[Obstacle], a: P, m: P, b: P, w: f64) -> bool {
    arc_blobs(a, m, b, w).iter().all(|part| near(obs, part.bbox()).all(|o| part.gap(&o.blob).0 >= o.gap + SURE))
}

/// Every piece of a path, straight or arc, is clear.
#[cfg(test)]
pub fn clear_pieces(obs: &[Obstacle], pieces: &[(P, P, Option<P>)], w: f64) -> bool {
    pieces.iter().all(|(a, b, m)| match m {
        Some(m) => clear_arc(obs, *a, *m, *b, w),
        None => clash(obs, *a, *b, w).is_none(),
    })
}

/// A path's pieces as short straight steps, for pushing with: arcs are cut
/// up finely enough that `ARC_ERROR` more width covers them.
pub fn as_steps(pieces: &[(P, P, Option<P>)]) -> Vec<(P, P)> {
    let mut out = Vec::new();
    for (a, b, m) in pieces {
        match m {
            Some(m) => out.extend(arc_points(*a, *m, *b).windows(2).map(|s| (s[0], s[1]))),
            None => out.push((*a, *b)),
        }
    }
    out
}

/// Rounds a path's corners into tangent arcs of `radius`. Each corner may
/// use up to half of a segment it shares with another corner, or all of an
/// end segment. An arc that would come too close to `obs` tries half the
/// radius, and so on; below 0.05 mm the corner stays sharp.
pub fn fillet(obs: &[Obstacle], pts: &[P], w: f64, radius: f64) -> (Vec<P>, Vec<Option<P>>) {
    let n = pts.len();
    if n < 3 || radius <= 0.0 {
        return (pts.to_vec(), vec![None; n.saturating_sub(1)]);
    }
    let mut out = vec![pts[0]];
    let mut mids: Vec<Option<P>> = Vec::new();
    for i in 1..n - 1 {
        let (p0, c, p1) = (pts[i - 1], pts[i], pts[i + 1]);
        let (din, dout) = (c.sub(p0), p1.sub(c));
        let (lin, lout) = (din.len(), dout.len());
        let mut rounded = false;
        if lin > 1e-9 && lout > 1e-9 {
            let (u, v) = (din.scale(1.0 / lin), dout.scale(1.0 / lout));
            let theta = u.dot(v).clamp(-1.0, 1.0).acos();
            if theta > 1e-6 && theta < std::f64::consts::PI - 1e-3 {
                let half = (theta / 2.0).tan();
                let avail_in = if i == 1 { lin } else { lin / 2.0 };
                let avail_out = if i + 1 == n - 1 { lout } else { lout / 2.0 };
                let mut r = radius.min(avail_in.min(avail_out) / half);
                let turn = u.cross(v).signum();
                while r >= 0.05 {
                    let t = r * half;
                    let (t1, t2) = (c.sub(u.scale(t)).rounded(), c.add(v.scale(t)).rounded());
                    let centre = c.sub(u.scale(t)).add(p(-u.y, u.x).scale(turn * r));
                    let towards = c.sub(centre);
                    let mid = centre.add(towards.scale(r / towards.len())).rounded();
                    if clear_arc(obs, t1, mid, t2, w) {
                        out.push(t1);
                        mids.push(None);
                        out.push(t2);
                        mids.push(Some(mid));
                        rounded = true;
                        break;
                    }
                    r /= 2.0;
                }
            }
        }
        if !rounded {
            out.push(c);
            mids.push(None);
        }
    }
    out.push(pts[n - 1]);
    mids.push(None);
    // Drop straight pieces with no length, where an arc fills a whole segment.
    let mut i = 0;
    while i + 1 < out.len() {
        if mids[i].is_none() && out[i].same(out[i + 1]) && out.len() > 2 {
            out.remove(i + 1);
            mids.remove(i);
        } else {
            i += 1;
        }
    }
    (out, mids)
}

/// Cuts a path at its first clash: straight pieces as far as they're clear,
/// arcs not at all. True when it was cut.
pub fn cut_at_clash(obs: &[Obstacle], pieces: &[(P, P, Option<P>)], w: f64) -> (Vec<P>, Vec<Option<P>>, bool) {
    let mut pts = vec![pieces.first().map(|p| p.0).unwrap_or_default()];
    let mut mids = Vec::new();
    for (a, b, m) in pieces {
        match m {
            Some(m) if clear_arc(obs, *a, *m, *b, w) => {
                pts.push(*b);
                mids.push(Some(*m));
            }
            Some(_) => return (pts, mids, true),
            None => {
                let (part, cut) = stop_short(obs, &[*a, *b], w);
                if part.len() == 2 {
                    pts.push(part[1]);
                    mids.push(None);
                }
                if cut {
                    return (pts, mids, true);
                }
            }
        }
    }
    (pts, mids, false)
}

/// How far along a path it can go before coming too close to something.
pub fn stop_short(obs: &[Obstacle], pts: &[P], w: f64) -> (Vec<P>, bool) {
    let mut out = vec![pts[0]];
    for s in pts.windows(2) {
        if clash(obs, s[0], s[1], w).is_none() {
            out.push(s[1]);
            continue;
        }
        // Halve the way in to the furthest clear point.
        let (mut lo, mut hi) = (0.0, 1.0);
        for _ in 0..24 {
            let mid = (lo + hi) / 2.0;
            if clash(obs, s[0], s[0].lerp(s[1], mid), w).is_none() {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        // A micron short of the limit, so rounding can't cross it.
        let len = s[0].dist(s[1]).max(1e-9);
        let end = s[0].lerp(s[1], (lo - 0.001 / len).max(0.0)).rounded();
        if !end.same(s[0]) {
            out.push(end);
        }
        return (out, true);
    }
    (out, false)
}

#[derive(Clone, Copy, PartialEq)]
struct Open {
    f: f64,
    cell: usize,
}
impl Eq for Open {}
impl Ord for Open {
    fn cmp(&self, o: &Self) -> Ordering {
        o.f.total_cmp(&self.f)
    }
}
impl PartialOrd for Open {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}

const DIRS: [(i32, i32); 8] = [(1, 0), (1, 1), (0, 1), (-1, 1), (-1, 0), (-1, -1), (0, -1), (1, -1)];

/// Finds a clear path from `a` towards `b`, round whatever is in the way.
pub fn walkaround(obs: &[Obstacle], a: P, b: P, w: f64, corner: Corner, flip: bool, area: BBox) -> Plan {
    // The quick answer, when it's clear.
    for f in [flip, !flip] {
        let direct = legs(a, b, corner, f);
        if clear(obs, &direct, w) {
            return Plan { pts: direct, blocked: false, hits: Vec::new(), ..Default::default() };
        }
    }
    lattice(obs, a, b, w, corner, flip, area, None, false)
}

/// How much a pushed track pays per millimetre for every millimetre it
/// strays from where it was, so it bends as little as it can.
const STAY: f64 = 2.0;

fn polyline_dist(q: P, line: &[P]) -> f64 {
    match line.len() {
        0 => 0.0,
        1 => q.dist(line[0]),
        _ => line.windows(2).map(|s| dist_to_segment(q, s[0], s[1]).0).fold(f64::MAX, f64::min),
    }
}

/// The lattice search behind walking round and pushing. With a `guide` it
/// keeps close to that path; with `pinned_end` it must finish exactly on `b`.
#[allow(clippy::too_many_arguments)]
fn lattice(obs: &[Obstacle], a: P, b: P, w: f64, corner: Corner, flip: bool, area: BBox, guide: Option<&[P]>, pinned_end: bool) -> Plan {
    let gap_max = obs.iter().map(|o| o.gap).fold(0.0, f64::max);
    let h = ((w + 2.0 * gap_max) / 4.0).clamp(0.05, 0.25);
    let h = (h / 0.025).round().max(2.0) * 0.025;
    // The lattice: anchored on `a`, covering `area`.
    let (i0, j0) = (((area.min.x - a.x) / h).floor() as i64, ((area.min.y - a.y) / h).floor() as i64);
    let (i1, j1) = (((area.max.x - a.x) / h).ceil() as i64, ((area.max.y - a.y) / h).ceil() as i64);
    let (nx, ny) = ((i1 - i0 + 1).clamp(1, 900) as usize, (j1 - j0 + 1).clamp(1, 900) as usize);
    let at = |c: usize| -> P { p(a.x + (i0 + (c % nx) as i64) as f64 * h, a.y + (j0 + (c / nx) as i64) as f64 * h) };
    let cell_of = |q: P| -> Option<usize> {
        let i = ((q.x - a.x) / h).round() as i64 - i0;
        let j = ((q.y - a.y) / h).round() as i64 - j0;
        (i >= 0 && j >= 0 && (i as usize) < nx && (j as usize) < ny).then(|| j as usize * nx + i as usize)
    };

    // Stamp out every lattice point too close to something. The extra 0.75 h
    // keeps the straight step between two clear points clear as well.
    let mut blocked = vec![false; nx * ny];
    let margin = 0.75 * h;
    for o in obs {
        let reach = o.gap + w / 2.0 + margin;
        let bb = o.bbox.grown(reach);
        let ci0 = (((bb.min.x - a.x) / h).floor() as i64 - i0).max(0);
        let cj0 = (((bb.min.y - a.y) / h).floor() as i64 - j0).max(0);
        let ci1 = (((bb.max.x - a.x) / h).ceil() as i64 - i0).min(nx as i64 - 1);
        let cj1 = (((bb.max.y - a.y) / h).ceil() as i64 - j0).min(ny as i64 - 1);
        for j in cj0..=cj1 {
            for i in ci0..=ci1 {
                let c = j as usize * nx + i as usize;
                if !blocked[c] && o.blob.dist_to(at(c)) < reach {
                    blocked[c] = true;
                }
            }
        }
    }
    let Some(start) = cell_of(a) else { return blocked_plan(obs, a, b, w, corner, flip) };
    blocked[start] = false;
    // Aim for the lattice point nearest the pointer, or the nearest clear one.
    let Some(mut goal) = cell_of(b) else { return blocked_plan(obs, a, b, w, corner, flip) };
    let mut reached_goal = true;
    if pinned_end {
        blocked[goal] = false;
    }
    if blocked[goal] {
        reached_goal = false;
        let (gi, gj) = ((goal % nx) as i64, (goal / nx) as i64);
        let mut best: Option<(f64, usize)> = None;
        for ring in 1..40i64 {
            for dj in -ring..=ring {
                for di in -ring..=ring {
                    if di.abs() != ring && dj.abs() != ring {
                        continue;
                    }
                    let (i, j) = (gi + di, gj + dj);
                    if i < 0 || j < 0 || i >= nx as i64 || j >= ny as i64 {
                        continue;
                    }
                    let c = j as usize * nx + i as usize;
                    if !blocked[c] {
                        let d = at(c).dist(b);
                        if best.is_none_or(|bst| d < bst.0) {
                            best = Some((d, c));
                        }
                    }
                }
            }
            if best.is_some() {
                break;
            }
        }
        match best {
            Some((_, c)) => goal = c,
            None => return blocked_plan(obs, a, b, w, corner, flip),
        }
    }

    // A*, with a small charge for every turn so the path keeps straight.
    let dirs: Vec<usize> = if corner == Corner::D90 { vec![0, 2, 4, 6] } else { (0..8).collect() };
    let mut g = vec![f64::INFINITY; nx * ny];
    let mut from = vec![usize::MAX; nx * ny];
    let mut dir_in = vec![u8::MAX; nx * ny];
    let goal_p = at(goal);
    let heuristic = |c: usize| {
        let d = at(c).sub(goal_p);
        let (dx, dy) = (d.x.abs(), d.y.abs());
        if corner == Corner::D90 { dx + dy } else { dx.max(dy) + (std::f64::consts::SQRT_2 - 1.0) * dx.min(dy) }
    };
    let mut open = BinaryHeap::new();
    g[start] = 0.0;
    open.push(Open { f: heuristic(start), cell: start });
    let mut expanded = 0usize;
    let mut found = false;
    while let Some(Open { cell, .. }) = open.pop() {
        if cell == goal {
            found = true;
            break;
        }
        expanded += 1;
        if expanded > 250_000 {
            break;
        }
        let (ci, cj) = ((cell % nx) as i64, (cell / nx) as i64);
        for &k in &dirs {
            let (di, dj) = DIRS[k];
            let (ni, nj) = (ci + di as i64, cj + dj as i64);
            if ni < 0 || nj < 0 || ni >= nx as i64 || nj >= ny as i64 {
                continue;
            }
            let n = nj as usize * nx + ni as usize;
            if blocked[n] {
                continue;
            }
            let step = if di != 0 && dj != 0 { std::f64::consts::SQRT_2 * h } else { h };
            let turn = match dir_in[cell] {
                u8::MAX => 0.0,
                d => {
                    let t = (k as i32 - d as i32).rem_euclid(8);
                    let t = t.min(8 - t);
                    [0.0, 0.6, 1.6, 6.0, 20.0][t as usize] * h
                }
            };
            let bend = guide.map(|g| polyline_dist(at(n), g) * STAY * step).unwrap_or(0.0);
            let cost = g[cell] + step + turn + bend;
            if cost < g[n] {
                g[n] = cost;
                from[n] = cell;
                dir_in[n] = k as u8;
                open.push(Open { f: cost + heuristic(n), cell: n });
            }
        }
    }
    if !found {
        return blocked_plan(obs, a, b, w, corner, flip);
    }
    let mut cells = vec![goal];
    while let Some(&c) = cells.last() {
        if c == start {
            break;
        }
        cells.push(from[c]);
    }
    cells.reverse();
    let mut pts: Vec<P> = cells.iter().map(|&c| at(c).rounded()).collect();
    pts[0] = a;
    let mut pts = merge_collinear(pts);
    // Finish exactly on the pointer when the last little step is clear.
    if reached_goal {
        let last = *pts.last().unwrap();
        let tail = legs(last, b, corner, flip);
        if clear(obs, &tail, w) {
            pts.extend_from_slice(&tail[1..]);
        } else {
            reached_goal = false;
        }
    }
    let pts = straighten(obs, merge_collinear(pts), w, corner);
    Plan { blocked: !reached_goal, pts, ..Default::default() }
}

/// When there's no way through: as far as it goes clear on the direct legs.
fn blocked_plan(obs: &[Obstacle], a: P, b: P, w: f64, corner: Corner, flip: bool) -> Plan {
    let (pts, _) = stop_short(obs, &legs(a, b, corner, flip), w);
    Plan { pts, blocked: true, hits: Vec::new(), ..Default::default() }
}

pub fn merge_collinear(pts: Vec<P>) -> Vec<P> {
    let mut out: Vec<P> = Vec::with_capacity(pts.len());
    for q in pts {
        if out.last().is_some_and(|l| l.same(q)) {
            continue;
        }
        if out.len() >= 2 {
            let (a, b) = (out[out.len() - 2], out[out.len() - 1]);
            let (d1, d2) = (b.sub(a), q.sub(b));
            if d1.cross(d2).abs() < 1e-9 * (d1.len() * d2.len()).max(1e-12) && d1.dot(d2) > 0.0 {
                out.pop();
            }
        }
        out.push(q);
    }
    out
}

/// Pulls a path straight: from each corner, jump to the furthest later
/// corner the legs can reach clear.
pub fn straighten(obs: &[Obstacle], pts: Vec<P>, w: f64, corner: Corner) -> Vec<P> {
    if pts.len() < 3 {
        return pts;
    }
    let mut out = vec![pts[0]];
    let mut i = 0;
    while i < pts.len() - 1 {
        let mut jumped = false;
        for j in (i + 2..pts.len()).rev() {
            for flip in [false, true] {
                let l = legs(pts[i], pts[j], corner, flip);
                if clear(obs, &l, w) {
                    out.extend_from_slice(&l[1..]);
                    i = j;
                    jumped = true;
                    break;
                }
            }
            if jumped {
                break;
            }
        }
        if !jumped {
            out.push(pts[i + 1]);
            i += 1;
        }
    }
    merge_collinear(out)
}

/// What the router knows about one layer while routing one net.
#[derive(Clone, Default)]
pub struct Scene {
    /// Everything on the layer, every net.
    pub all: Vec<Obstacle>,
    /// What the route has to keep clear of.
    pub own: Vec<Obstacle>,
    /// Other nets' tracks, as runs that can be pushed.
    pub lines: Vec<Line>,
    pub net: u32,
    pub layer: u8,
    pub clearance: f64,
    /// The other copper layers, for vias that are pushed: everything on
    /// each, and its tracks as runs (which follow a via that moves).
    pub others: Vec<(u8, Vec<Obstacle>, Vec<Line>)>,
    /// Other nets' vias: id, centre, pad size, net.
    pub vias: Vec<(u32, P, f64, u32)>,
    /// Whether Push may move vias too (Altium's "Allow via pushing").
    pub via_push: bool,
    /// The radius corners are rounded to in the arc corner styles.
    pub arc_radius: f64,
}

impl Scene {
    pub fn new(board: &Board, copper: &Copper, layer: u8, net: u32) -> Scene {
        let all = obstacles_all(board, copper, layer);
        let others = board
            .copper_layers()
            .into_iter()
            .filter(|l| *l != layer)
            .map(|l| (l, obstacles_all(board, copper, l), lines(board, copper, l, 0)))
            .collect();
        Scene {
            own: for_net(&all, net),
            lines: lines(board, copper, layer, net),
            all,
            net,
            layer,
            clearance: board.rules.clearance,
            others,
            vias: board.vias.iter().filter(|v| v.net != net || net == 0).map(|v| (v.id, v.at, v.dia, v.net)).collect(),
            via_push: true,
            arc_radius: 1.0,
        }
    }

    /// Can this be moved by a push?
    fn movable(&self, o: &Obstacle) -> bool {
        o.movable || (self.via_push && matches!(o.item, Some(Item::Via(_))))
    }
}

/// Works out the next piece of route in the chosen mode. `held` is route
/// already placed but not yet on the board, which pushed tracks keep clear of.
#[allow(clippy::too_many_arguments)]
pub fn plan(scene: &Scene, a: P, b: P, w: f64, corner: Corner, flip: bool, mode: Conflict, board: BBox, held: &[(P, P)]) -> Plan {
    let mut plan = plan_straight(scene, a, b, w, corner, flip, mode, board, held);
    // Pushing modes round their corners before pushing (see push_plan).
    if corner.arcs() && plan.mids.is_empty() {
        let obs: &[Obstacle] = if mode == Conflict::Ignore { &[] } else { &scene.own };
        let (pts, mids) = fillet(obs, &plan.pts, w, scene.arc_radius);
        plan.pts = pts;
        plan.mids = mids;
    }
    plan
}

#[allow(clippy::too_many_arguments)]
fn plan_straight(scene: &Scene, a: P, b: P, w: f64, corner: Corner, flip: bool, mode: Conflict, board: BBox, held: &[(P, P)]) -> Plan {
    let arcs = corner.arcs();
    let corner = corner.base();
    let obs = &scene.own;
    let span = a.dist(b);
    let mut area = BBox::of(&[a, b]).grown((span * 0.6).clamp(3.0, 25.0));
    if !board.is_empty() {
        let edge = board.grown(1.0);
        area = BBox {
            min: p(area.min.x.max(edge.min.x), area.min.y.max(edge.min.y)),
            max: p(area.max.x.min(edge.max.x), area.max.y.min(edge.max.y)),
        };
        area.add(a);
        area.add(b);
    }
    match mode {
        Conflict::Ignore => {
            let pts = legs(a, b, corner, flip);
            let mut hits: Vec<Item> = Vec::new();
            for s in pts.windows(2) {
                let track = Blob::line(s[0], s[1], w);
                for o in near(obs, track.bbox()) {
                    if let Some(item) = o.item
                        && track.gap(&o.blob).0 < o.gap - 1e-6
                        && !hits.contains(&item)
                    {
                        hits.push(item);
                    }
                }
            }
            Plan { pts, blocked: false, hits, ..Default::default() }
        }
        Conflict::Stop => {
            let (pts, blocked) = stop_short(obs, &legs(a, b, corner, flip), w);
            Plan { pts, blocked, ..Default::default() }
        }
        Conflict::Walkaround => walkaround(obs, a, b, w, corner, flip, area),
        Conflict::Push => push_plan(scene, a, b, w, corner, flip, area, held, arcs),
        Conflict::HugPush => {
            // Keep close round what's in the way while that's a fair path;
            // push only when going round would be the long way or no way.
            let round = walkaround(obs, a, b, w, corner, flip, area);
            let direct = length(&legs(a, b, corner, flip));
            if !round.blocked && length(&round.pts) <= direct * 1.3 + 1.0 {
                return round;
            }
            let pushed = push_plan(scene, a, b, w, corner, flip, area, held, arcs);
            if pushed.note.is_some() && !round.blocked { round } else { pushed }
        }
    }
}

fn length(pts: &[P]) -> f64 {
    pts.windows(2).map(|s| s[0].dist(s[1])).sum()
}

/// Round what can't move; through what can, pushing it aside.
#[allow(clippy::too_many_arguments)]
fn push_plan(scene: &Scene, a: P, b: P, w: f64, corner: Corner, flip: bool, area: BBox, held: &[(P, P)], arcs: bool) -> Plan {
    let fixed: Vec<Obstacle> = scene.own.iter().filter(|o| !scene.movable(o)).cloned().collect();
    let mut head = walkaround(&fixed, a, b, w, corner, flip, area);
    // Round the corners first, so what's pushed keeps clear of the arcs.
    let mut width = w;
    if arcs {
        let (pts, mids) = fillet(&fixed, &head.pts, w, scene.arc_radius);
        head.pts = pts;
        head.mids = mids;
        width += 2.0 * ARC_ERROR;
    }
    let mut segs: Vec<(P, P)> = held.to_vec();
    segs.extend(as_steps(&head.pieces()));
    match shove(scene, &segs, width) {
        Some((pushed, vias)) => Plan { pushed, vias, ..head },
        None => {
            let mut round = walkaround(&scene.own, a, b, w, corner, flip, area);
            if arcs {
                let (pts, mids) = fillet(&scene.own, &round.pts, w, scene.arc_radius);
                round.pts = pts;
                round.mids = mids;
            }
            round.note = Some("can't push that aside");
            round
        }
    }
}

// ---------------------------------------------------------------------------
// Pushing.

/// A run of one net's track between two fixed ends: pads, vias, or where
/// three or more tracks meet. Pushing bends the run; its ends stay put.
#[derive(Clone, Debug)]
pub struct Line {
    pub ids: Vec<u32>,
    pub pts: Vec<P>,
    pub w: f64,
    pub net: u32,
}

/// A run after it's been pushed: its old tracks go, these corners replace them.
#[derive(Clone, Debug)]
pub struct Moved {
    pub ids: Vec<u32>,
    pub pts: Vec<P>,
    pub w: f64,
    pub net: u32,
    pub layer: u8,
}

fn key(q: P) -> (i64, i64) {
    ((q.x * 1e4).round() as i64, (q.y * 1e4).round() as i64)
}

/// Other nets' tracks on a layer, joined into runs.
pub fn lines(board: &Board, copper: &Copper, layer: u8, skip_net: u32) -> Vec<Line> {
    use std::collections::{HashMap, HashSet};
    // Arcs aren't pushed (yet); where one meets a straight track it pins it.
    let tracks: Vec<&Track> =
        board.tracks.iter().filter(|t| t.layer == layer && t.mid.is_none() && (t.net != skip_net || skip_net == 0)).collect();
    let mut ends: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
    for (i, t) in tracks.iter().enumerate() {
        ends.entry(key(t.a)).or_default().push(i);
        ends.entry(key(t.b)).or_default().push(i);
    }
    // Pads and vias pin a run's end, whatever the net.
    let mut pinned: HashSet<(i64, i64)> = HashSet::new();
    for c in &copper.items {
        if !matches!(c.item, Item::Track(_)) {
            pinned.insert(key(c.anchors[0]));
        }
    }
    let fixed = |q: P| pinned.contains(&key(q)) || ends.get(&key(q)).is_none_or(|v| v.len() != 2);
    let mut used = vec![false; tracks.len()];
    let mut out = Vec::new();
    for start in 0..tracks.len() {
        if used[start] {
            continue;
        }
        used[start] = true;
        let t0 = tracks[start];
        let mut ids = vec![t0.id];
        let mut pts = std::collections::VecDeque::from([t0.a, t0.b]);
        // Grow forwards from b, then backwards from a.
        for forward in [true, false] {
            loop {
                let here = if forward { *pts.back().unwrap() } else { *pts.front().unwrap() };
                if fixed(here) {
                    break;
                }
                let next = ends[&key(here)].iter().copied().find(|&j| {
                    !used[j] && tracks[j].net == t0.net && (tracks[j].w - t0.w).abs() < 1e-9
                });
                let Some(j) = next else { break };
                used[j] = true;
                let t = tracks[j];
                let far = if t.a.same(here) { t.b } else { t.a };
                if forward {
                    pts.push_back(far);
                    ids.push(t.id);
                } else {
                    pts.push_front(far);
                    ids.insert(0, t.id);
                }
            }
        }
        out.push(Line { ids, pts: pts.into_iter().collect(), w: t0.w, net: t0.net });
    }
    out
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Owner {
    Route,
    Run(usize),
    Via(usize),
}

struct Pusher {
    blob: Blob,
    bbox: BBox,
    net: u32,
    owner: Owner,
}

fn pusher(blob: Blob, net: u32, owner: Owner) -> Pusher {
    Pusher { bbox: blob.bbox(), blob, net, owner }
}

fn crowded(blob: &Blob, net: u32, who: Owner, pushers: &[Pusher], clearance: f64) -> bool {
    let bb = blob.bbox().grown(clearance);
    pushers
        .iter()
        .any(|p| p.owner != who && (p.net != net || net == 0) && p.bbox.touches(&bb) && blob.gap(&p.blob).0 < clearance + SURE)
}

fn crowds(pts: &[P], w: f64, net: u32, who: Owner, pushers: &[Pusher], clearance: f64) -> bool {
    pts.windows(2).any(|s| crowded(&Blob::line(s[0], s[1], w), net, who, pushers, clearance))
}

fn as_obstacle(p: &Pusher, gap: f64) -> Obstacle {
    Obstacle { blob: p.blob.clone(), bbox: p.bbox, gap, item: None, net: p.net, movable: false }
}

/// Distance along a path to each corner.
fn along(pts: &[P]) -> Vec<f64> {
    let mut d = vec![0.0];
    for s in pts.windows(2) {
        d.push(d.last().unwrap() + s[0].dist(s[1]));
    }
    d
}

/// The point at distance `s` along a path.
fn point_at(pts: &[P], s: f64) -> P {
    let d = along(pts);
    for i in 0..pts.len() - 1 {
        if s <= d[i + 1] + 1e-12 {
            let span = (d[i + 1] - d[i]).max(1e-12);
            return pts[i].lerp(pts[i + 1], ((s - d[i]) / span).clamp(0.0, 1.0)).rounded();
        }
    }
    *pts.last().unwrap()
}

/// The piece of a path from `s0` to `s1` along it.
fn piece(pts: &[P], s0: f64, s1: f64) -> Vec<P> {
    let d = along(pts);
    let mut out = vec![point_at(pts, s0)];
    for i in 0..pts.len() {
        if d[i] > s0 + 1e-9 && d[i] < s1 - 1e-9 {
            out.push(pts[i]);
        }
    }
    out.push(point_at(pts, s1));
    out
}

/// The path with the piece from `s0` to `s1` swapped for `mid`.
fn splice(pts: &[P], s0: f64, s1: f64, mid: &[P]) -> Vec<P> {
    let d = along(pts);
    let mut out: Vec<P> = (0..pts.len()).filter(|&i| d[i] < s0 - 1e-9).map(|i| pts[i]).collect();
    out.extend_from_slice(mid);
    out.extend((0..pts.len()).filter(|&i| d[i] > s1 + 1e-9).map(|i| pts[i]));
    merge_collinear(out)
}

/// Re-bends the stretch of `run` from `s0` to `s1` along it to keep clear of
/// `obs`, leaving the rest exactly where it was. Widens the stretch if it has
/// to. None when it can't be done, or only the long way round.
fn rebend(obs: &[Obstacle], run: &[P], w: f64, s0: f64, s1: f64, new_end: Option<P>) -> Option<Vec<P>> {
    let total = *along(run).last().unwrap();
    for pad in [1.0, 2.5, f64::INFINITY] {
        let (a0, a1) = ((s0 - pad).max(0.0), (s1 + pad).min(total));
        let guide = piece(run, a0, a1);
        let a = guide[0];
        let b = match new_end {
            Some(to) if a1 >= total - 1e-9 => to,
            _ => *guide.last().unwrap(),
        };
        let mut area = BBox::of(&guide).grown(2.0 + w);
        area.add(b);
        for o in obs {
            if o.bbox.touches(&area) {
                area.union(&o.bbox);
            }
        }
        let area = area.grown(1.0);
        let bent = lattice(obs, a, b, w, Corner::D45, false, area, Some(&guide), true);
        if bent.blocked || !bent.end().is_some_and(|e| e.same(b)) || !bent.pts[0].same(a) {
            continue;
        }
        let mut whole = splice(run, a0, a1, &bent.pts);
        if new_end.is_some() && a1 >= total - 1e-9 {
            *whole.last_mut().unwrap() = b;
        }
        if !clear(obs, &whole, w) {
            continue;
        }
        // A push bends a track; it doesn't send it the long way round.
        if length(&whole) > total.max(run[0].dist(b)) * 1.5 + 3.0 {
            return None;
        }
        return Some(whole);
    }
    None
}

/// Where along a run the pushers crowd it: the first and last distance.
fn crowded_stretch(run: &[P], w: f64, net: u32, who: Owner, pushers: &[Pusher], clearance: f64) -> Option<(f64, f64)> {
    let d = along(run);
    let reach = clearance + w / 2.0 + 0.05;
    let mut span: Option<(f64, f64)> = None;
    for i in 0..run.len() - 1 {
        let n = ((d[i + 1] - d[i]) / 0.05).ceil().max(1.0) as usize;
        for k in 0..=n {
            let s = d[i] + (d[i + 1] - d[i]) * k as f64 / n as f64;
            let q = run[i].lerp(run[i + 1], k as f64 / n as f64);
            let hit = pushers.iter().any(|p| p.owner != who && (p.net != net || net == 0) && p.blob.dist_to(q) < reach);
            if hit {
                span = Some(span.map_or((s, s), |(a, b)| (a.min(s), b.max(s))));
            }
        }
    }
    span
}

/// Re-bends a run so the end that was at `from` finishes at `to`, changing
/// only the stretch near that end.
fn follow(obs: &[Obstacle], run: &[P], from: P, to: P, w: f64) -> Option<Vec<P>> {
    let at_start = run[0].same(from);
    let mut path: Vec<P> = run.to_vec();
    if at_start {
        path.reverse();
    }
    let total = *along(&path).last().unwrap();
    let near = (to.dist(from) + 0.5).min(total);
    let mut pts = rebend(obs, &path, w, total - near, total, Some(to))?;
    if at_start {
        pts.reverse();
    }
    Some(pts)
}

/// Pushes other nets' runs, and their vias if allowed, out of the way of
/// `segs` (the route, `w` wide). Each pushed run is bent as little as it can
/// be, keeping its ends; a pushed via moves the least it can to somewhere
/// clear on every layer, and its tracks follow it. Whatever is pushed then
/// pushes what it runs into. None when something can't give way.
pub fn shove(scene: &Scene, segs: &[(P, P)], w: f64) -> Option<(Vec<Moved>, Vec<(u32, P)>)> {
    let c = scene.clearance;
    let mut pushers: Vec<Pusher> = segs.iter().map(|(a, b)| pusher(Blob::line(*a, *b, w), scene.net, Owner::Route)).collect();
    let mut runs: Vec<Vec<P>> = scene.lines.iter().map(|l| l.pts.clone()).collect();
    let mut moved: Vec<bool> = vec![false; runs.len()];
    // Runs on other layers that followed a via: (layer slot, run) -> corners.
    let mut followed: Vec<((usize, usize), Vec<P>)> = Vec::new();
    let vias: Vec<(u32, P, f64, u32)> = if scene.via_push { scene.vias.clone() } else { Vec::new() };
    let mut via_at: Vec<P> = vias.iter().map(|v| v.1).collect();
    let mut via_moved = vec![false; vias.len()];
    let fixed_here = |net: u32| -> Vec<Obstacle> {
        scene.all.iter().filter(|o| !scene.movable(o) && (o.net != net || net == 0)).cloned().collect()
    };
    let mut rounds = 0;
    loop {
        rounds += 1;
        if rounds > 60 {
            return None;
        }
        // A via the route or a pushed run crowds: move it.
        if let Some(k) = (0..vias.len()).find(|&k| crowded(&Blob::dot(via_at[k], vias[k].2), vias[k].3, Owner::Via(k), &pushers, c)) {
            let (id, _, dia, net) = vias[k];
            let from = via_at[k];
            let found = move_via(scene, &pushers, k, id, from, dia, net, &runs, &followed)?;
            let (to, here, there) = found;
            via_at[k] = to;
            via_moved[k] = true;
            pushers.retain(|p| p.owner != Owner::Via(k));
            pushers.push(pusher(Blob::dot(to, dia), net, Owner::Via(k)));
            for (i, pts) in here {
                pushers.retain(|p| p.owner != Owner::Run(i));
                for s in pts.windows(2) {
                    pushers.push(pusher(Blob::line(s[0], s[1], scene.lines[i].w), net, Owner::Run(i)));
                }
                runs[i] = pts;
                moved[i] = true;
            }
            for (key, pts) in there {
                followed.retain(|f| f.0 != key);
                followed.push((key, pts));
            }
            continue;
        }
        // A run something crowds: bend it out of the way.
        let Some(i) = (0..runs.len()).find(|&i| crowds(&runs[i], scene.lines[i].w, scene.lines[i].net, Owner::Run(i), &pushers, c))
        else {
            break;
        };
        let l = &scene.lines[i];
        // Keeps clear of what can't move, and of everything pushing it. Vias
        // are kept clear of as they stand: only the route pushes vias.
        let mut obs = fixed_here(l.net);
        for (k, v) in vias.iter().enumerate() {
            if v.3 != l.net || l.net == 0 {
                let blob = Blob::dot(via_at[k], v.2);
                obs.push(Obstacle { bbox: blob.bbox(), blob, gap: c, item: None, net: v.3, movable: false });
            }
        }
        for p in pushers.iter().filter(|p| p.owner != Owner::Run(i) && (p.net != l.net || l.net == 0)) {
            if !matches!(p.owner, Owner::Via(_)) {
                obs.push(as_obstacle(p, c));
            }
        }
        let Some((s0, s1)) = crowded_stretch(&runs[i], l.w, l.net, Owner::Run(i), &pushers, c) else { continue };
        let pts = rebend(&obs, &runs[i], l.w, s0, s1, None)?;
        pushers.retain(|p| p.owner != Owner::Run(i));
        for s in pts.windows(2) {
            pushers.push(pusher(Blob::line(s[0], s[1], l.w), l.net, Owner::Run(i)));
        }
        runs[i] = pts;
        moved[i] = true;
    }
    let mut out: Vec<Moved> = (0..runs.len())
        .filter(|&i| moved[i])
        .map(|i| Moved {
            ids: scene.lines[i].ids.clone(),
            pts: runs[i].clone(),
            w: scene.lines[i].w,
            net: scene.lines[i].net,
            layer: scene.layer,
        })
        .collect();
    for ((slot, j), pts) in followed {
        let (layer, _, lines) = &scene.others[slot];
        let l = &lines[j];
        out.push(Moved { ids: l.ids.clone(), pts, w: l.w, net: l.net, layer: *layer });
    }
    let moved_vias = (0..vias.len()).filter(|&k| via_moved[k]).map(|k| (vias[k].0, via_at[k])).collect();
    Some((out, moved_vias))
}

type ViaMove = (P, Vec<(usize, Vec<P>)>, Vec<((usize, usize), Vec<P>)>);

/// Finds the nearest spot a via can move to: clear of the route and every
/// pusher on this layer, clear of everything on the other layers, with each
/// track on it able to follow. Returns the spot and the re-bent tracks.
#[allow(clippy::too_many_arguments)]
fn move_via(
    scene: &Scene,
    pushers: &[Pusher],
    k: usize,
    id: u32,
    from: P,
    dia: f64,
    net: u32,
    runs: &[Vec<P>],
    followed: &[((usize, usize), Vec<P>)],
) -> Option<ViaMove> {
    let c = scene.clearance;
    let me = Some(Item::Via(id));
    let other_net = |o_net: u32| o_net != net || net == 0;
    // What the via itself must keep clear of, on every layer.
    let mut walls: Vec<Obstacle> = scene.all.iter().filter(|o| o.item != me && other_net(o.net) && !o.movable).cloned().collect();
    walls.extend(pushers.iter().filter(|p| p.owner != Owner::Via(k) && other_net(p.net)).map(|p| as_obstacle(p, c)));
    for (_, all, _) in &scene.others {
        walls.extend(all.iter().filter(|o| o.item != me && other_net(o.net)).cloned());
    }
    let ok = |q: P| {
        let blob = Blob::dot(q, dia);
        let bb = blob.bbox();
        walls.iter().all(|o| !o.bbox.grown(o.gap).touches(&bb) || blob.gap(&o.blob).0 >= o.gap + SURE)
    };
    // Spots nearest first, out to a few millimetres.
    let step = 0.05;
    let reach = 60i64;
    let mut spots: Vec<(f64, P)> = Vec::new();
    for j in -reach..=reach {
        for i in -reach..=reach {
            let d = ((i * i + j * j) as f64).sqrt() * step;
            if d > 0.0 && d <= reach as f64 * step {
                spots.push((d, from.add(p(i as f64 * step, j as f64 * step)).rounded()));
            }
        }
    }
    spots.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut tries = 0;
    for (_, to) in spots {
        if !ok(to) {
            continue;
        }
        tries += 1;
        if tries > 8 {
            return None;
        }
        // Its tracks on this layer follow it, bending round what pushes them.
        let mut here = Vec::new();
        let mut fine = true;
        for (i, l) in scene.lines.iter().enumerate() {
            let run = &runs[i];
            if l.net != net || !(run[0].same(from) || run.last().unwrap().same(from)) {
                continue;
            }
            let mut obs: Vec<Obstacle> = scene.all.iter().filter(|o| !scene.movable(o) && other_net(o.net)).cloned().collect();
            obs.extend(pushers.iter().filter(|p| p.owner != Owner::Run(i) && p.owner != Owner::Via(k) && other_net(p.net)).map(|p| as_obstacle(p, c)));
            match follow(&obs, run, from, to, l.w) {
                Some(pts) => here.push((i, pts)),
                None => {
                    fine = false;
                    break;
                }
            }
        }
        // And on the other layers, bending round what's there.
        let mut there = Vec::new();
        for (slot, (_, all, lines)) in scene.others.iter().enumerate() {
            if !fine {
                break;
            }
            for (j, l) in lines.iter().enumerate() {
                let run = followed.iter().find(|f| f.0 == (slot, j)).map(|f| f.1.clone()).unwrap_or_else(|| l.pts.clone());
                if l.net != net || !(run[0].same(from) || run.last().unwrap().same(from)) {
                    continue;
                }
                let obs: Vec<Obstacle> = all.iter().filter(|o| other_net(o.net)).cloned().collect();
                match follow(&obs, &run, from, to, l.w) {
                    Some(pts) => there.push(((slot, j), pts)),
                    None => {
                        fine = false;
                        break;
                    }
                }
            }
        }
        if fine {
            return Some((to, here, there));
        }
    }
    None
}

/// Puts pushed runs and vias on the board in place of the old ones. Returns
/// the new tracks' ids.
pub fn apply_pushed(board: &mut Board, pushed: &[Moved], vias: &[(u32, P)]) -> Vec<u32> {
    let mut made = Vec::new();
    for m in pushed {
        board.tracks.retain(|t| !m.ids.contains(&t.id));
        for s in m.pts.windows(2) {
            if !s[0].same(s[1]) {
                let id = board.new_id();
                board.tracks.push(Track { mid: None, id, a: s[0], b: s[1], w: m.w, layer: m.layer, net: m.net });
                made.push(id);
            }
        }
    }
    for (id, at) in vias {
        if let Some(v) = board.vias.iter_mut().find(|v| v.id == *id) {
            v.at = *at;
        }
    }
    made
}

/// Tracks of one net that can go without parting anything that was joined:
/// loops and dead-end stubs. Vias count as ends worth keeping.
pub fn spare_tracks(board: &Board, net: u32) -> Vec<u32> {
    let mut spare = Vec::new();
    let ids: Vec<u32> = board.tracks.iter().filter(|t| t.net == net).map(|t| t.id).collect();
    let whole = net_groups(board, net, None);
    for id in ids {
        if net_groups(board, net, Some(id)) == whole {
            spare.push(id);
        }
    }
    spare
}

/// Which pads and vias of a net are joined, as a canonical grouping, with
/// one track left out.
pub fn net_groups(board: &Board, net: u32, without: Option<u32>) -> Vec<usize> {
    let mut b = Board { tracks: Vec::new(), ..board.clone() };
    b.tracks = board.tracks.iter().filter(|t| t.net == net && Some(t.id) != without).cloned().collect();
    b.vias.retain(|v| v.net == net);
    for f in &mut b.footprints {
        for pad in &mut f.pads {
            if pad.net != net {
                pad.net = 0;
            }
        }
    }
    let c = crate::pcb::copper::analyse(&b);
    // The pads and vias, in a fixed order, each named by the first of its group.
    let ends: Vec<usize> = c
        .items
        .iter()
        .enumerate()
        .filter(|(_, cu)| cu.net == net && !matches!(cu.item, Item::Track(_)))
        .map(|(i, _)| i)
        .collect();
    ends.iter()
        .map(|&i| ends.iter().position(|&j| c.group[j] == c.group[i]).unwrap_or(0))
        .collect()
}

/// After a route is laid: takes out the old path it made redundant, and any
/// stub left hanging, but never a loop or stub that was there before.
/// `spare_before` is `spare_tracks` from before the route went down.
pub fn remove_loops(board: &mut Board, net: u32, spare_before: &[u32], new: &[u32]) {
    if net == 0 {
        return;
    }
    loop {
        let spare = spare_tracks(board, net);
        let Some(id) = spare.into_iter().find(|id| !spare_before.contains(id) && !new.contains(id)) else { break };
        board.tracks.retain(|t| t.id != id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scene(obs: Vec<Obstacle>) -> Scene {
        Scene { all: obs.clone(), own: obs, net: 1, layer: TOP, clearance: 0.2, ..Default::default() }
    }

    fn wall() -> Vec<Obstacle> {
        // A wall of copper across the way from (5, -3) to (5, 3).
        let blob = Blob::line(p(5.0, -3.0), p(5.0, 3.0), 0.5);
        vec![Obstacle { bbox: blob.bbox(), blob, gap: 0.2, item: None, net: 7, movable: false }]
    }

    #[test]
    fn straight_when_clear() {
        let plan = plan(&scene(vec![]), p(0.0, 0.0), p(10.0, 0.0), 0.25, Corner::D45, false, Conflict::Walkaround, BBox::default(), &[]);
        assert_eq!(plan.pts, vec![p(0.0, 0.0), p(10.0, 0.0)]);
        assert!(!plan.blocked);
    }

    #[test]
    fn walks_round_a_wall_at_45_degrees() {
        let obs = wall();
        let plan = plan(&scene(obs.clone()), p(0.0, 0.0), p(10.0, 0.0), 0.25, Corner::D45, false, Conflict::Walkaround, BBox::default(), &[]);
        assert!(!plan.blocked, "{plan:?}");
        assert!(plan.pts.last().unwrap().same(p(10.0, 0.0)));
        assert!(clear(&obs, &plan.pts, 0.25));
        for s in plan.pts.windows(2) {
            let d = s[1].sub(s[0]);
            let ok = d.x.abs() < 1e-6 || d.y.abs() < 1e-6 || (d.x.abs() - d.y.abs()).abs() < 1e-6;
            assert!(ok, "not 45°: {s:?}");
        }
        assert!(plan.pts.len() <= 8, "too many corners: {:?}", plan.pts);
    }

    #[test]
    fn stops_before_the_wall() {
        let obs = wall();
        let plan = plan(&scene(obs.clone()), p(0.0, 0.0), p(10.0, 0.0), 0.25, Corner::D45, false, Conflict::Stop, BBox::default(), &[]);
        assert!(plan.blocked);
        let end = plan.end().unwrap();
        assert!(end.x < 5.0 - 0.25 - 0.2 - 0.125 + 1e-3 && end.x > 4.0, "{end:?}");
    }

    #[test]
    fn ninety_degree_legs() {
        assert_eq!(legs(p(0.0, 0.0), p(3.0, 2.0), Corner::D90, false), vec![p(0.0, 0.0), p(3.0, 0.0), p(3.0, 2.0)]);
        assert_eq!(legs(p(0.0, 0.0), p(3.0, 2.0), Corner::D90, true), vec![p(0.0, 0.0), p(0.0, 2.0), p(3.0, 2.0)]);
    }

    #[test]
    fn rerouting_removes_the_old_path() {
        let mut b = crate::pcb::sample::board();
        // The LED net already has R3.2 to D1.2 straight down; route it again
        // the long way round and the old track should go.
        let net = b.net("LED");
        let spare_before = spare_tracks(&b, net);
        assert!(spare_before.is_empty());
        let mut new = Vec::new();
        for (a, e) in [((16.9125, 22.0), (18.5, 22.0)), ((18.5, 22.0), (18.5, 27.0)), ((18.5, 27.0), (16.9125, 27.0))] {
            let id = b.new_id();
            b.tracks.push(Track { mid: None, id, a: p(a.0, a.1), b: p(e.0, e.1), w: 0.25, layer: TOP, net });
            new.push(id);
        }
        let old_led: Vec<u32> = b.tracks.iter().filter(|t| t.net == net && !new.contains(&t.id)).map(|t| t.id).collect();
        remove_loops(&mut b, net, &spare_before, &new);
        assert!(b.tracks.iter().all(|t| !old_led.contains(&t.id)), "old path kept");
        assert!(new.iter().all(|id| b.track(*id).is_some()), "new path lost");
    }
}

#[cfg(test)]
mod speed {
    use super::*;

    #[test]
    fn a_long_walk_round_is_quick() {
        let b = crate::pcb::sample::board();
        let c = crate::pcb::copper::analyse(&b);
        let vcc = b.net_by_name("VCC").unwrap();
        let obs = obstacles(&b, &c, TOP, vcc);
        let t = std::time::Instant::now();
        let mut worst = 0.0f64;
        for (a, e) in [((6.0, 18.77), (27.475, 19.905)), ((6.0, 18.77), (34.9125, 12.0)), ((44.0, 30.0), (6.0, 5.0))] {
            let s = std::time::Instant::now();
            let scene = Scene::new(&b, &c, TOP, vcc);
            let plan = plan(&scene, p(a.0, a.1), p(e.0, e.1), 0.25, Corner::D45, false, Conflict::Walkaround, b.outline_bbox(), &[]);
            worst = worst.max(s.elapsed().as_secs_f64());
            assert!(clear(&obs, &plan.pts, 0.25));
        }
        eprintln!("three walk-rounds in {:?}, worst {:.1} ms", t.elapsed(), worst * 1e3);
        assert!(worst < 0.25, "too slow to follow the pointer: {worst}s");
    }
}

#[cfg(test)]
mod pushing {
    use super::*;
    use crate::pcb::copper::analyse;

    fn pad(b: &mut Board, at: P, net: u32) {
        let id = b.new_id();
        b.footprints.push(Footprint {
            id,
            def: "TP".into(),
            refdes: format!("TP{id}"),
            value: String::new(),
            at,
            rot: 0.0,
            side: Side::Top,
            pads: vec![Pad { num: "1".into(), at: p(0.0, 0.0), size: p(1.0, 1.0), rot: 0.0, shape: PadShape::Circle, drill: 0.0, net }],
            graphics: vec![],
            label_at: P::default(),
            locked: false,
        });
    }

    fn run(b: &mut Board, y: f64, net: u32) {
        pad(b, p(0.0, y), net);
        pad(b, p(20.0, y), net);
        let id = b.new_id();
        b.tracks.push(Track { mid: None, id, a: p(0.0, y), b: p(20.0, y), w: 0.25, layer: TOP, net });
    }

    fn board() -> (Board, u32) {
        let mut b = Board { outline: rect_outline(p(-5.0, -10.0), p(25.0, 10.0)), ..Board::default() };
        let route = b.net("A");
        let other = b.net("B");
        run(&mut b, 0.0, other);
        (b, route)
    }

    #[test]
    fn a_route_pushes_a_track_over_its_tip() {
        let (b, a) = board();
        let scene = Scene::new(&b, &analyse(&b), TOP, a);
        assert_eq!(scene.lines.len(), 1);
        let head = [(p(10.0, -5.0), p(10.0, 1.0))];
        let (moved, _) = shove(&scene, &head, 0.25).expect("it can give way");
        assert_eq!(moved.len(), 1);
        let m = &moved[0];
        assert!(m.pts[0].same(p(0.0, 0.0)) && m.pts.last().unwrap().same(p(20.0, 0.0)), "ends stay put");
        let route = Blob::line(head[0].0, head[0].1, 0.25);
        for s in m.pts.windows(2) {
            let gap = route.gap(&Blob::line(s[0], s[1], 0.25)).0;
            assert!(gap >= 0.2 - 1e-6, "pushed track only {gap} from the route");
        }
        assert!(m.pts.iter().any(|q| q.y > 1.3), "goes over the tip: {:?}", m.pts);
        assert!(m.pts.len() <= 8, "bends too much: {:?}", m.pts);
    }

    #[test]
    fn pushes_carry_on_to_the_next_track() {
        let (mut b, a) = board();
        let c = b.net("C");
        // Its pads keep clear of the first track's pads.
        run(&mut b, 1.3, c);
        let scene = Scene::new(&b, &analyse(&b), TOP, a);
        let (moved, _) = shove(&scene, &[(p(10.0, -5.0), p(10.0, 1.0))], 0.25).expect("both give way");
        assert_eq!(moved.len(), 2, "the second track is pushed too");
        let (m0, m1) = (&moved[0], &moved[1]);
        for s in m0.pts.windows(2) {
            for t in m1.pts.windows(2) {
                let gap = Blob::line(s[0], s[1], 0.25).gap(&Blob::line(t[0], t[1], 0.25)).0;
                assert!(gap >= 0.2 - 1e-6, "pushed tracks {gap} apart");
            }
        }
    }

    #[test]
    fn a_pinned_track_cant_be_pushed() {
        let (mut b, a) = board();
        // A wall of pads just above the track, all the way along.
        let wall = b.net("W");
        for i in -4..=24 {
            pad(&mut b, p(i as f64, 1.05), wall);
        }
        let scene = Scene::new(&b, &analyse(&b), TOP, a);
        // The route comes up from the board's edge, so there's no way round
        // its other end either.
        let got = shove(&scene, &[(p(10.0, -9.6), p(10.0, 1.0))], 0.25);
        assert!(got.is_none(), "{:?}", got.map(|m| m.0[0].pts.clone()));
        // Push mode then walks round instead, and says so.
        let plan = plan(&scene, p(10.0, -9.6), p(10.0, 1.0), 0.25, Corner::D45, false, Conflict::Push, b.outline_bbox(), &[]);
        assert!(plan.pushed.is_empty());
        assert!(plan.note.is_some());
    }

    #[test]
    fn pushing_is_quick() {
        let (mut b, a) = board();
        for (i, y) in [1.3, 2.6, 3.9].iter().enumerate() {
            let n = b.net(&format!("N{i}"));
            run(&mut b, *y, n);
        }
        let scene = Scene::new(&b, &analyse(&b), TOP, a);
        let t = std::time::Instant::now();
        let moved = shove(&scene, &[(p(10.0, -5.0), p(10.0, 1.0))], 0.25);
        eprintln!("four tracks pushed in {:?}", t.elapsed());
        let n = moved.map(|m| m.0.len()).unwrap_or(0);
        assert!(n >= 2, "only {n} pushed");
        assert!(t.elapsed().as_secs_f64() < 0.25);
    }
}

#[cfg(test)]
mod clearance_always_holds {
    use super::*;
    use crate::pcb::copper::analyse;
    use crate::pcb::drc;

    const CLEARANCE: [&str; 4] = ["Too close", "Short circuit", "Too close to the edge", "Off the board"];

    /// Puts a plan on a copy of the board and asks the rule check about it.
    fn problems(b: &Board, net: u32, plan: &Plan, w: f64) -> Vec<String> {
        let mut b = b.clone();
        let mut fresh: Vec<Item> = apply_pushed(&mut b, &plan.pushed, &plan.vias).into_iter().map(Item::Track).collect();
        fresh.extend(plan.vias.iter().map(|v| Item::Via(v.0)));
        for (a, e, mid) in plan.pieces() {
            if !a.same(e) {
                let id = b.new_id();
                b.tracks.push(Track { mid, id, a, b: e, w, layer: TOP, net });
                fresh.push(Item::Track(id));
            }
        }
        drc::check(&b, &analyse(&b))
            .into_iter()
            .filter(|p| CLEARANCE.contains(&p.title.as_str()) && p.items.iter().any(|i| fresh.contains(i)))
            .map(|p| format!("{}: {}", p.title, p.detail))
            .collect()
    }

    #[test]
    fn no_mode_but_ignore_ever_plans_too_close() {
        let b = crate::pcb::sample::board();
        let c = analyse(&b);
        let starts: Vec<(P, u32)> = c
            .items
            .iter()
            .filter(|i| matches!(i.item, Item::Pad(..)) && i.net != 0 && i.on.has(TOP))
            .map(|i| (i.anchors[0], i.net))
            .collect();
        // A fixed spread of places to aim at: every pad, and spots across the board.
        let mut targets: Vec<P> = c.items.iter().filter(|i| matches!(i.item, Item::Pad(..))).map(|i| i.anchors[0]).collect();
        let mut seed = 12345u64;
        for _ in 0..20 {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            let x = 2.0 + (seed >> 33) as f64 % 46.0;
            let y = 2.0 + (seed >> 13) as f64 % 31.0;
            targets.push(p(x, y).snap(0.25));
        }
        let mut checked = 0;
        let mut arcs = 0;
        for (mode, corner) in [
            (Conflict::Walkaround, Corner::D45),
            (Conflict::Push, Corner::D45),
            (Conflict::HugPush, Corner::D45),
            (Conflict::Stop, Corner::D45),
            (Conflict::Walkaround, Corner::Arc45),
            (Conflict::Walkaround, Corner::Arc90),
            (Conflict::Push, Corner::Arc45),
            (Conflict::Stop, Corner::Arc90),
        ] {
            for (k, (a, net)) in starts.iter().enumerate() {
                let mut scene = Scene::new(&b, &c, TOP, *net);
                scene.via_push = true;
                scene.arc_radius = [0.5, 1.0, 3.0][k % 3];
                for to in targets.iter().skip(k % 3).step_by(3) {
                    let plan = plan(&scene, *a, *to, 0.25, corner, k % 2 == 0, mode, b.outline_bbox(), &[]);
                    arcs += plan.mids.iter().filter(|m| m.is_some()).count();
                    let found = problems(&b, *net, &plan, 0.25);
                    assert!(found.is_empty(), "{mode:?} {corner:?} from {a:?} to {to:?}: {found:?}\n{plan:?}");
                    checked += 1;
                }
            }
        }
        assert!(checked > 600, "only {checked} routes tried");
        assert!(arcs > 100, "only {arcs} arcs made");
    }
}

#[cfg(test)]
mod via_pushing {
    use super::*;
    use crate::pcb::copper::analyse;

    fn tp(b: &mut Board, at: P, net: u32) {
        let id = b.new_id();
        b.footprints.push(Footprint {
            id,
            def: "TP".into(),
            refdes: format!("TP{id}"),
            value: String::new(),
            at,
            rot: 0.0,
            side: Side::Top,
            pads: vec![Pad { num: "1".into(), at: p(0.0, 0.0), size: p(1.0, 1.0), rot: 0.0, shape: PadShape::Circle, drill: 0.8, net }],
            graphics: vec![],
            label_at: P::default(),
            locked: false,
        });
    }

    /// Net B goes from a pad along the top to a via, then along the bottom to
    /// another pad. The route (net A) comes up at the via from below.
    fn board() -> (Board, u32, u32) {
        let mut b = Board { outline: rect_outline(p(-5.0, -10.0), p(25.0, 10.0)), ..Board::default() };
        let a = b.net("A");
        let n = b.net("B");
        tp(&mut b, p(0.0, 0.0), n);
        tp(&mut b, p(20.0, 0.0), n);
        let v = b.new_id();
        b.vias.push(Via { id: v, at: p(10.0, 0.0), dia: 0.6, drill: 0.3, net: n });
        let t1 = b.new_id();
        b.tracks.push(Track { mid: None, id: t1, a: p(0.0, 0.0), b: p(10.0, 0.0), w: 0.25, layer: TOP, net: n });
        let t2 = b.new_id();
        b.tracks.push(Track { mid: None, id: t2, a: p(10.0, 0.0), b: p(20.0, 0.0), w: 0.25, layer: BOTTOM, net: n });
        (b, a, n)
    }

    #[test]
    fn a_via_moves_and_its_tracks_follow_on_both_layers() {
        let (b, a, _) = board();
        let mut scene = Scene::new(&b, &analyse(&b), TOP, a);
        scene.via_push = true;
        let head = [(p(10.0, -5.0), p(10.0, -0.2))];
        let (runs, vias) = shove(&scene, &head, 0.25).expect("the via can move");
        assert_eq!(vias.len(), 1, "the via moves");
        let to = vias[0].1;
        let route = Blob::line(head[0].0, head[0].1, 0.25);
        assert!(route.gap(&Blob::dot(to, 0.6)).0 >= 0.2 - 1e-6, "via still too close at {to:?}");
        assert!(to.dist(p(10.0, 0.0)) < 1.0, "moved further than needed: {to:?}");
        for layer in [TOP, BOTTOM] {
            let m = runs.iter().find(|m| m.layer == layer).unwrap_or_else(|| panic!("track on layer {layer} didn't follow"));
            assert!(m.pts[0].same(to) || m.pts.last().unwrap().same(to), "{layer}: {:?}", m.pts);
        }
    }

    #[test]
    fn without_via_pushing_it_walks_round() {
        let (b, a, _) = board();
        let mut scene = Scene::new(&b, &analyse(&b), TOP, a);
        scene.via_push = false;
        let plan = plan(&scene, p(10.0, -5.0), p(10.0, 3.0), 0.25, Corner::D45, false, Conflict::Push, b.outline_bbox(), &[]);
        assert!(plan.vias.is_empty());
    }

    #[test]
    fn hug_and_push_goes_round_when_it_fairly_can() {
        // A short stub of another net in the way: going round is cheap.
        let mut b = Board { outline: rect_outline(p(-5.0, -10.0), p(25.0, 10.0)), ..Board::default() };
        let a = b.net("A");
        let n = b.net("B");
        tp(&mut b, p(10.0, 0.0), n);
        let t = b.new_id();
        b.tracks.push(Track { mid: None, id: t, a: p(10.0, 0.0), b: p(11.0, 0.0), w: 0.25, layer: TOP, net: n });
        let scene = Scene::new(&b, &analyse(&b), TOP, a);
        let plan = plan(&scene, p(5.0, 0.0), p(15.0, 0.0), 0.25, Corner::D45, false, Conflict::HugPush, b.outline_bbox(), &[]);
        assert!(plan.pushed.is_empty() && !plan.blocked, "{plan:?}");
        // A long track across the board: going round is the long way, so it pushes.
        let (b, a, _) = super::pushing_board();
        let scene = Scene::new(&b, &analyse(&b), TOP, a);
        let pushing = super::plan(&scene, p(10.0, -5.0), p(10.0, 1.0), 0.25, Corner::D45, false, Conflict::HugPush, b.outline_bbox(), &[]);
        assert_eq!(pushing.pushed.len(), 1, "{pushing:?}");
    }
}

/// A route's net and another net's long track across its way, for tests.
#[cfg(test)]
fn pushing_board() -> (Board, u32, u32) {
    let mut b = Board { outline: rect_outline(p(-5.0, -10.0), p(25.0, 10.0)), ..Board::default() };
    let a = b.net("A");
    let n = b.net("B");
    for x in [0.0, 20.0] {
        let id = b.new_id();
        b.footprints.push(Footprint {
            id,
            def: "TP".into(),
            refdes: format!("TP{id}"),
            value: String::new(),
            at: p(x, 0.0),
            rot: 0.0,
            side: Side::Top,
            pads: vec![Pad { num: "1".into(), at: p(0.0, 0.0), size: p(1.0, 1.0), rot: 0.0, shape: PadShape::Circle, drill: 0.0, net: n }],
            graphics: vec![],
            label_at: P::default(),
            locked: false,
        });
    }
    let id = b.new_id();
    b.tracks.push(Track { mid: None, id, a: p(0.0, 0.0), b: p(20.0, 0.0), w: 0.25, layer: TOP, net: n });
    (b, a, n)
}


#[cfg(test)]
mod arcs {
    use super::*;

    #[test]
    fn a_square_corner_becomes_a_tangent_arc() {
        let pts = [p(0.0, 0.0), p(10.0, 0.0), p(10.0, 10.0)];
        let (out, mids) = fillet(&[], &pts, 0.25, 2.0);
        assert_eq!(out, vec![p(0.0, 0.0), p(8.0, 0.0), p(10.0, 2.0), p(10.0, 10.0)]);
        let m = mids[1].expect("the corner is an arc");
        let (c, r) = arc_center(out[1], m, out[2]).unwrap();
        // The middle is kept to the nanometre, so the centre is within a few.
        assert!(c.dist(p(8.0, 2.0)) < 1e-5 && (r - 2.0).abs() < 1e-5, "{c:?} {r}");
        assert!(mids[0].is_none() && mids[2].is_none());
    }

    #[test]
    fn a_45_degree_corner_uses_a_shorter_tangent() {
        let pts = [p(0.0, 0.0), p(10.0, 0.0), p(20.0, 10.0)];
        let (out, mids) = fillet(&[], &pts, 0.25, 2.0);
        let t = 2.0 * (std::f64::consts::FRAC_PI_8).tan();
        assert!(out[1].dist(p(10.0 - t, 0.0)) < 1e-6, "{out:?}");
        assert!(mids[1].is_some());
    }

    #[test]
    fn an_arc_shrinks_rather_than_come_too_close() {
        // A pad tucked inside the corner, where a big arc would cut across it.
        // Clear of the sharp corner, but right where a 3 mm arc would pass.
        let blob = Blob::dot(p(9.0, 1.0), 0.6);
        let obs = vec![Obstacle { bbox: blob.bbox(), blob, gap: 0.2, item: None, net: 9, movable: false }];
        let pts = [p(0.0, 0.0), p(10.0, 0.0), p(10.0, 10.0)];
        assert!(clear(&obs, &pts, 0.25), "the sharp path is clear");
        let (out, mids) = fillet(&obs, &pts, 0.25, 3.0);
        let pieces: Vec<(P, P, Option<P>)> = out.windows(2).enumerate().map(|(i, s)| (s[0], s[1], mids[i])).collect();
        assert!(clear_pieces(&obs, &pieces, 0.25), "{pieces:?}");
        let m = mids.iter().flatten().next().expect("a smaller arc still fits");
        let (_, r) = arc_center(out[1], *m, out[2]).unwrap();
        assert!(r < 2.9, "it should have shrunk: {r}");
    }

    #[test]
    fn arcs_go_to_the_gerber_as_arcs() {
        let mut b = Board::default();
        let n = b.net("A");
        b.tracks.push(Track { mid: Some(p(8.0 + 2.0f64.sqrt(), 2.0 - 2.0f64.sqrt()).rounded()), id: 1, a: p(8.0, 0.0), b: p(10.0, 2.0), w: 0.25, layer: TOP, net: n });
        let files = crate::pcb::gerber::files(&b);
        let top = &files.iter().find(|f| f.0.ends_with(".GTL")).unwrap().1;
        assert!(top.contains("G75*"));
        // Anticlockwise to (10, 2), centre (8, 2) to within a couple of nanometres.
        let line = top.lines().find(|l| l.starts_with("G03X10000000Y2000000I")).expect("an anticlockwise arc");
        let ij: Vec<i64> = line.trim_start_matches("G03X10000000Y2000000I").trim_end_matches("D01*").split('J').map(|v| v.parse().unwrap()).collect();
        assert!(ij[0].abs() <= 2 && (ij[1] - 2_000_000).abs() <= 2, "{line}");
    }
}
