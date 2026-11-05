//! Points in millimetres, and the few shape sums the editor needs.
//!
//! Every piece of copper is a `Blob`: a point, a line or a convex polygon,
//! grown outwards by a radius. A round pad is a grown point, a track is a grown
//! line, a rounded pad is a grown rectangle. The gap between any two blobs is
//! the gap between their cores less both radii, so one routine answers
//! clearance, clicking and connection for everything.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct P {
    pub x: f64,
    pub y: f64,
}

pub const fn p(x: f64, y: f64) -> P {
    P { x, y }
}

/// Lengths are kept to the nearest nanometre, so sums like 0.1 + 0.2 land on
/// the grid instead of a hair beside it.
pub fn nm(v: f64) -> f64 {
    (v * 1e6).round() / 1e6
}

impl P {
    pub fn add(self, o: P) -> P {
        p(self.x + o.x, self.y + o.y)
    }
    pub fn sub(self, o: P) -> P {
        p(self.x - o.x, self.y - o.y)
    }
    pub fn scale(self, k: f64) -> P {
        p(self.x * k, self.y * k)
    }
    pub fn len(self) -> f64 {
        self.x.hypot(self.y)
    }
    pub fn dist(self, o: P) -> f64 {
        self.sub(o).len()
    }
    pub fn dot(self, o: P) -> f64 {
        self.x * o.x + self.y * o.y
    }
    pub fn cross(self, o: P) -> f64 {
        self.x * o.y - self.y * o.x
    }
    pub fn rot(self, deg: f64) -> P {
        if deg == 0.0 {
            return self;
        }
        // Quarter turns are exact, which keeps pads on the grid.
        let quarter = (deg / 90.0).round();
        if (deg - quarter * 90.0).abs() < 1e-9 {
            return match (quarter as i64).rem_euclid(4) {
                0 => self,
                1 => p(-self.y, self.x),
                2 => p(-self.x, -self.y),
                _ => p(self.y, -self.x),
            };
        }
        let (s, c) = deg.to_radians().sin_cos();
        p(self.x * c - self.y * s, self.x * s + self.y * c)
    }
    pub fn snap(self, grid: f64) -> P {
        if grid <= 0.0 {
            return p(nm(self.x), nm(self.y));
        }
        p(nm((self.x / grid).round() * grid), nm((self.y / grid).round() * grid))
    }
    pub fn rounded(self) -> P {
        p(nm(self.x), nm(self.y))
    }
    pub fn lerp(self, o: P, t: f64) -> P {
        self.add(o.sub(self).scale(t))
    }
    pub fn same(self, o: P) -> bool {
        self.dist(o) < 1e-4
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BBox {
    pub min: P,
    pub max: P,
}

impl Default for BBox {
    fn default() -> Self {
        BBox { min: p(f64::MAX, f64::MAX), max: p(f64::MIN, f64::MIN) }
    }
}

impl BBox {
    pub fn is_empty(&self) -> bool {
        self.min.x > self.max.x
    }
    pub fn add(&mut self, q: P) {
        self.min.x = self.min.x.min(q.x);
        self.min.y = self.min.y.min(q.y);
        self.max.x = self.max.x.max(q.x);
        self.max.y = self.max.y.max(q.y);
    }
    pub fn union(&mut self, o: &BBox) {
        if !o.is_empty() {
            self.add(o.min);
            self.add(o.max);
        }
    }
    pub fn grown(&self, by: f64) -> BBox {
        BBox { min: p(self.min.x - by, self.min.y - by), max: p(self.max.x + by, self.max.y + by) }
    }
    pub fn w(&self) -> f64 {
        self.max.x - self.min.x
    }
    pub fn h(&self) -> f64 {
        self.max.y - self.min.y
    }
    pub fn center(&self) -> P {
        self.min.lerp(self.max, 0.5)
    }
    pub fn contains(&self, q: P) -> bool {
        q.x >= self.min.x && q.x <= self.max.x && q.y >= self.min.y && q.y <= self.max.y
    }
    pub fn touches(&self, o: &BBox) -> bool {
        self.min.x <= o.max.x && o.min.x <= self.max.x && self.min.y <= o.max.y && o.min.y <= self.max.y
    }
    pub fn of(pts: &[P]) -> BBox {
        let mut b = BBox::default();
        for q in pts {
            b.add(*q);
        }
        b
    }
}

/// A point, line or convex polygon (counter-clockwise), grown by `r`.
#[derive(Clone, Debug, PartialEq)]
pub struct Blob {
    pub core: Vec<P>,
    pub r: f64,
}

impl Blob {
    pub fn dot(c: P, d: f64) -> Blob {
        Blob { core: vec![c], r: d / 2.0 }
    }
    pub fn line(a: P, b: P, w: f64) -> Blob {
        Blob { core: if a.same(b) { vec![a] } else { vec![a, b] }, r: w / 2.0 }
    }
    pub fn bbox(&self) -> BBox {
        BBox::of(&self.core).grown(self.r)
    }
    pub fn center(&self) -> P {
        BBox::of(&self.core).center()
    }
    /// How far a point is from the edge; below zero is inside.
    pub fn dist_to(&self, q: P) -> f64 {
        core_dist(&self.core, &[q]).0 - self.r
    }
    /// The gap between two blobs, and the nearest points on each core.
    pub fn gap(&self, o: &Blob) -> (f64, P, P) {
        let (d, a, b) = core_dist(&self.core, &o.core);
        (d - self.r - o.r, a, b)
    }
    /// The outline as a ring, with rounded corners where it's grown.
    pub fn ring(&self) -> Vec<P> {
        grown_ring(&self.core, self.r)
    }
}

pub fn circle_steps(r: f64) -> usize {
    ((r * 40.0).sqrt() * 6.0).clamp(12.0, 72.0) as usize
}

/// The outline of a convex core grown by `r`, counter-clockwise.
pub fn grown_ring(core: &[P], r: f64) -> Vec<P> {
    match core.len() {
        0 => Vec::new(),
        1 => {
            let n = circle_steps(r);
            (0..n)
                .map(|i| {
                    let t = i as f64 / n as f64 * std::f64::consts::TAU;
                    core[0].add(p(t.cos(), t.sin()).scale(r))
                })
                .collect()
        }
        _ if r <= 0.0 => core.to_vec(),
        _ => {
            // Walk round the corners; a line is a polygon with two corners.
            let ring: Vec<P> = if core.len() == 2 { vec![core[0], core[1]] } else { core.to_vec() };
            let n = ring.len();
            let steps = circle_steps(r);
            let mut out = Vec::new();
            for i in 0..n {
                let prev = ring[(i + n - 1) % n];
                let here = ring[i];
                let next = ring[(i + 1) % n];
                let normal = |a: P, b: P| {
                    let d = b.sub(a);
                    let l = d.len().max(1e-12);
                    p(d.y / l, -d.x / l)
                };
                let a0 = {
                    let nrm = normal(prev, here);
                    nrm.y.atan2(nrm.x)
                };
                let mut a1 = {
                    let nrm = normal(here, next);
                    nrm.y.atan2(nrm.x)
                };
                while a1 < a0 {
                    a1 += std::f64::consts::TAU;
                }
                let span = a1 - a0;
                let k = ((span / std::f64::consts::TAU) * steps as f64).ceil().max(1.0) as usize;
                for j in 0..=k {
                    let t = a0 + span * j as f64 / k as f64;
                    out.push(here.add(p(t.cos(), t.sin()).scale(r)));
                }
            }
            out
        }
    }
}

pub fn dist_to_segment(q: P, a: P, b: P) -> (f64, P) {
    let ab = b.sub(a);
    let l2 = ab.dot(ab);
    let t = if l2 > 0.0 { (q.sub(a).dot(ab) / l2).clamp(0.0, 1.0) } else { 0.0 };
    let c = a.add(ab.scale(t));
    (q.dist(c), c)
}

fn segments_cross(a: P, b: P, c: P, d: P) -> bool {
    let o = |p1: P, p2: P, p3: P| p2.sub(p1).cross(p3.sub(p1));
    let (d1, d2, d3, d4) = (o(c, d, a), o(c, d, b), o(a, b, c), o(a, b, d));
    ((d1 > 0.0 && d2 < 0.0) || (d1 < 0.0 && d2 > 0.0)) && ((d3 > 0.0 && d4 < 0.0) || (d3 < 0.0 && d4 > 0.0))
}

/// Nearest points between two segments.
pub fn segment_gap(a: P, b: P, c: P, d: P) -> (f64, P, P) {
    if segments_cross(a, b, c, d) {
        // Where they cross.
        let r = b.sub(a);
        let s = d.sub(c);
        let t = c.sub(a).cross(s) / r.cross(s);
        let x = a.add(r.scale(t));
        return (0.0, x, x);
    }
    let mut best = (f64::MAX, a, c);
    for (q, s0, s1, flip) in [(a, c, d, false), (b, c, d, false), (c, a, b, true), (d, a, b, true)] {
        let (dist, on) = dist_to_segment(q, s0, s1);
        if dist < best.0 {
            best = if flip { (dist, on, q) } else { (dist, q, on) };
        }
    }
    best
}

fn inside_convex(q: P, poly: &[P]) -> bool {
    if poly.len() < 3 {
        return false;
    }
    let n = poly.len();
    (0..n).all(|i| poly[(i + 1) % n].sub(poly[i]).cross(q.sub(poly[i])) >= -1e-12)
}

fn edges(core: &[P]) -> Vec<(P, P)> {
    match core.len() {
        0 => Vec::new(),
        1 => vec![(core[0], core[0])],
        2 => vec![(core[0], core[1])],
        n => (0..n).map(|i| (core[i], core[(i + 1) % n])).collect(),
    }
}

/// The gap between two cores, zero when they overlap.
pub fn core_dist(a: &[P], b: &[P]) -> (f64, P, P) {
    if let Some(q) = b.iter().find(|q| inside_convex(**q, a)) {
        return (0.0, *q, *q);
    }
    if let Some(q) = a.iter().find(|q| inside_convex(**q, b)) {
        return (0.0, *q, *q);
    }
    let mut best = (f64::MAX, P::default(), P::default());
    for (a0, a1) in edges(a) {
        for (b0, b1) in edges(b) {
            let g = segment_gap(a0, a1, b0, b1);
            if g.0 < best.0 {
                best = g;
            }
        }
    }
    best
}

/// A rectangle's corners, counter-clockwise, turned and moved.
pub fn rect_core(c: P, w: f64, h: f64, rot: f64) -> Vec<P> {
    [p(-w / 2.0, -h / 2.0), p(w / 2.0, -h / 2.0), p(w / 2.0, h / 2.0), p(-w / 2.0, h / 2.0)]
        .iter()
        .map(|q| c.add(q.rot(rot)))
        .collect()
}

pub fn point_in_ring(q: P, ring: &[P]) -> bool {
    let mut inside = false;
    let n = ring.len();
    for i in 0..n {
        let (a, b) = (ring[i], ring[(i + n - 1) % n]);
        if (a.y > q.y) != (b.y > q.y) && q.x < (b.x - a.x) * (q.y - a.y) / (b.y - a.y) + a.x {
            inside = !inside;
        }
    }
    inside
}

pub fn ring_area(ring: &[P]) -> f64 {
    let n = ring.len();
    (0..n).map(|i| ring[i].cross(ring[(i + 1) % n])).sum::<f64>() / 2.0
}

/// A shape cut into triangles: the indices point into `pts`.
pub fn triangulate(ring: &[P]) -> Vec<u32> {
    let mut idx: Vec<u32> = Vec::new();
    if ring.len() >= 3 {
        let mut cutter = earcut::Earcut::<f64>::new();
        cutter.earcut(ring.iter().map(|q| [q.x, q.y]), &[], &mut idx);
    }
    idx
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_round_pads_gap() {
        let a = Blob::dot(p(0.0, 0.0), 1.0);
        let b = Blob::dot(p(3.0, 0.0), 1.0);
        assert!((a.gap(&b).0 - 2.0).abs() < 1e-9);
    }

    #[test]
    fn track_past_a_square_pad() {
        let pad = Blob { core: rect_core(p(0.0, 0.0), 2.0, 2.0, 0.0), r: 0.0 };
        let track = Blob::line(p(-5.0, 1.5), p(5.0, 1.5), 0.2);
        assert!((pad.gap(&track).0 - 0.4).abs() < 1e-9);
        let through = Blob::line(p(-5.0, 0.0), p(5.0, 0.0), 0.2);
        assert!(pad.gap(&through).0 < 0.0);
    }

    #[test]
    fn quarter_turns_are_exact() {
        assert_eq!(p(1.27, 0.0).rot(90.0), p(0.0, 1.27));
        assert_eq!(p(1.27, 2.0).rot(-90.0), p(2.0, -1.27));
    }

    #[test]
    fn grown_rect_is_rounded() {
        let b = Blob { core: rect_core(p(0.0, 0.0), 2.0, 1.0, 0.0), r: 0.25 };
        let ring = b.ring();
        let bb = BBox::of(&ring);
        assert!((bb.w() - 2.5).abs() < 1e-9 && (bb.h() - 1.5).abs() < 1e-9);
        assert!(ring_area(&ring) > 0.0);
    }

    #[test]
    fn grid_snap_has_no_float_dust() {
        assert_eq!(p(0.1 + 0.2, 0.0).snap(0.05).x, 0.3);
    }
}
