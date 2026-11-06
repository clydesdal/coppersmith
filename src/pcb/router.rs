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
    D90,
    Any,
}

impl Corner {
    pub fn name(self) -> &'static str {
        match self {
            Corner::D45 => "45°",
            Corner::D90 => "90°",
            Corner::Any => "Any angle",
        }
    }
    pub fn next(self) -> Corner {
        match self {
            Corner::D45 => Corner::D90,
            Corner::D90 => Corner::Any,
            Corner::Any => Corner::D45,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Conflict {
    Ignore,
    Walkaround,
    Push,
    Stop,
}

impl Conflict {
    pub fn name(self) -> &'static str {
        match self {
            Conflict::Ignore => "Ignore obstacles",
            Conflict::Walkaround => "Walk around",
            Conflict::Push => "Push",
            Conflict::Stop => "Stop at first obstacle",
        }
    }
    pub fn next(self) -> Conflict {
        match self {
            Conflict::Ignore => Conflict::Walkaround,
            Conflict::Walkaround => Conflict::Push,
            Conflict::Push => Conflict::Stop,
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
    /// It couldn't get all the way to the pointer.
    pub blocked: bool,
    /// What it runs into, for Ignore mode to show.
    pub hits: Vec<Item>,
    /// Other nets' tracks pushed aside to make room.
    pub pushed: Vec<Moved>,
    /// Something worth saying, like a push that couldn't be done.
    pub note: Option<&'static str>,
}

impl Plan {
    pub fn end(&self) -> Option<P> {
        self.pts.last().copied()
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
        .map(|c| Obstacle {
            blob: c.blob.clone(),
            bbox: c.bbox,
            gap: r.clearance,
            item: Some(c.item),
            net: c.net,
            movable: matches!(c.item, Item::Track(_)),
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
    let d = b.sub(a);
    let (ax, ay) = (d.x.abs(), d.y.abs());
    if a.same(b) {
        return vec![a];
    }
    let straight = ax < 1e-9 || ay < 1e-9;
    match corner {
        Corner::Any => vec![a, b],
        _ if straight => vec![a, b],
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

/// The first obstacle a track from `a` to `b` comes too close to.
pub fn clash(obs: &[Obstacle], a: P, b: P, w: f64) -> Option<&Obstacle> {
    let track = Blob::line(a, b, w);
    near(obs, track.bbox()).find(|o| track.gap(&o.blob).0 < o.gap - 1e-6)
}

pub fn clear(obs: &[Obstacle], pts: &[P], w: f64) -> bool {
    pts.windows(2).all(|s| clash(obs, s[0], s[1], w).is_none())
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
        let end = s[0].lerp(s[1], lo).rounded();
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
}

impl Scene {
    pub fn new(board: &Board, copper: &Copper, layer: u8, net: u32) -> Scene {
        let all = obstacles_all(board, copper, layer);
        Scene {
            own: for_net(&all, net),
            lines: lines(board, copper, layer, net),
            all,
            net,
            layer,
            clearance: board.rules.clearance,
        }
    }
}

/// Works out the next piece of route in the chosen mode. `held` is route
/// already placed but not yet on the board, which pushed tracks keep clear of.
#[allow(clippy::too_many_arguments)]
pub fn plan(scene: &Scene, a: P, b: P, w: f64, corner: Corner, flip: bool, mode: Conflict, board: BBox, held: &[(P, P)]) -> Plan {
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
        Conflict::Push => {
            // Round what can't move; through what can, pushing it aside.
            let fixed: Vec<Obstacle> = obs.iter().filter(|o| !o.movable).cloned().collect();
            let head = walkaround(&fixed, a, b, w, corner, flip, area);
            let mut segs: Vec<(P, P)> = held.to_vec();
            segs.extend(head.pts.windows(2).map(|s| (s[0], s[1])));
            match shove(scene, &segs, w) {
                Some(pushed) => Plan { pushed, ..head },
                None => {
                    let mut round = walkaround(obs, a, b, w, corner, flip, area);
                    round.note = Some("can't push that aside");
                    round
                }
            }
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
    let tracks: Vec<&Track> = board.tracks.iter().filter(|t| t.layer == layer && (t.net != skip_net || skip_net == 0)).collect();
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

struct Pusher {
    blob: Blob,
    bbox: BBox,
    net: u32,
    /// The run this came from, once a pushed run starts pushing others.
    owner: Option<usize>,
}

fn crowds(pts: &[P], w: f64, net: u32, who: Option<usize>, pushers: &[Pusher], clearance: f64) -> bool {
    pts.windows(2).any(|s| {
        let seg = Blob::line(s[0], s[1], w);
        let bb = seg.bbox().grown(clearance);
        pushers.iter().any(|p| {
            p.owner != who && (p.net != net || net == 0) && p.bbox.touches(&bb) && seg.gap(&p.blob).0 < clearance - 1e-6
        })
    })
}

/// Pushes other nets' runs out of the way of `segs` (the route, `w` wide).
/// Each pushed run is bent as little as it can be, keeping its ends, and then
/// pushes whatever it runs into in turn. None when something can't give way.
pub fn shove(scene: &Scene, segs: &[(P, P)], w: f64) -> Option<Vec<Moved>> {
    let c = scene.clearance;
    let mut pushers: Vec<Pusher> = segs
        .iter()
        .map(|(a, b)| {
            let blob = Blob::line(*a, *b, w);
            Pusher { bbox: blob.bbox(), blob, net: scene.net, owner: None }
        })
        .collect();
    let mut runs: Vec<Vec<P>> = scene.lines.iter().map(|l| l.pts.clone()).collect();
    let mut moved: Vec<bool> = vec![false; runs.len()];
    let mut rounds = 0;
    loop {
        let Some(i) = (0..runs.len()).find(|&i| crowds(&runs[i], scene.lines[i].w, scene.lines[i].net, Some(i), &pushers, c))
        else {
            break;
        };
        rounds += 1;
        if rounds > 40 {
            return None;
        }
        let l = &scene.lines[i];
        // Keeps clear of what can't move, and of everything pushing it.
        let mut obs: Vec<Obstacle> = scene.all.iter().filter(|o| !o.movable && (o.net != l.net || l.net == 0)).cloned().collect();
        for p in pushers.iter().filter(|p| p.owner != Some(i) && (p.net != l.net || l.net == 0)) {
            obs.push(Obstacle { blob: p.blob.clone(), bbox: p.bbox, gap: c, item: None, net: p.net, movable: false });
        }
        let mut area = BBox::of(&runs[i]);
        for p in &pushers {
            if p.owner != Some(i) {
                area.union(&p.bbox);
            }
        }
        let area = area.grown(2.0 + l.w + c);
        let (a, b) = (runs[i][0], *runs[i].last().unwrap());
        let bent = lattice(&obs, a, b, l.w, Corner::D45, false, area, Some(&scene.lines[i].pts), true);
        if bent.blocked || !bent.end().is_some_and(|e| e.same(b)) || !clear(&obs, &bent.pts, l.w) {
            return None;
        }
        // A push bends a track; it doesn't send it the long way round.
        let length = |pts: &[P]| pts.windows(2).map(|s| s[0].dist(s[1])).sum::<f64>();
        if length(&bent.pts) > length(&scene.lines[i].pts) * 1.5 + 3.0 {
            return None;
        }
        pushers.retain(|p| p.owner != Some(i));
        for s in bent.pts.windows(2) {
            let blob = Blob::line(s[0], s[1], l.w);
            pushers.push(Pusher { bbox: blob.bbox(), blob, net: l.net, owner: Some(i) });
        }
        runs[i] = bent.pts;
        moved[i] = true;
    }
    Some(
        (0..runs.len())
            .filter(|&i| moved[i])
            .map(|i| Moved {
                ids: scene.lines[i].ids.clone(),
                pts: runs[i].clone(),
                w: scene.lines[i].w,
                net: scene.lines[i].net,
                layer: scene.layer,
            })
            .collect(),
    )
}

/// Puts pushed runs on the board in place of their old tracks.
pub fn apply_pushed(board: &mut Board, pushed: &[Moved]) {
    for m in pushed {
        board.tracks.retain(|t| !m.ids.contains(&t.id));
        for s in m.pts.windows(2) {
            if !s[0].same(s[1]) {
                let id = board.new_id();
                board.tracks.push(Track { id, a: s[0], b: s[1], w: m.w, layer: m.layer, net: m.net });
            }
        }
    }
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
        Scene { all: obs.clone(), own: obs, lines: vec![], net: 1, layer: TOP, clearance: 0.2 }
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
            b.tracks.push(Track { id, a: p(a.0, a.1), b: p(e.0, e.1), w: 0.25, layer: TOP, net });
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
        b.tracks.push(Track { id, a: p(0.0, y), b: p(20.0, y), w: 0.25, layer: TOP, net });
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
        let moved = shove(&scene, &head, 0.25).expect("it can give way");
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
        let moved = shove(&scene, &[(p(10.0, -5.0), p(10.0, 1.0))], 0.25).expect("both give way");
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
        assert!(got.is_none(), "{:?}", got.map(|m| m[0].pts.clone()));
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
        let n = moved.map(|m| m.len()).unwrap_or(0);
        assert!(n >= 2, "only {n} pushed");
        assert!(t.elapsed().as_secs_f64() < 0.25);
    }
}
