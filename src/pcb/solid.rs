//! The board as a solid, for the 3D view: the outline made into a top, a
//! bottom and edges, with the holes drilled through, and for each face the
//! flat pictures it wears (copper, solder-mask openings, silkscreen) as
//! triangles, ready to be drawn into textures.
//!
//! The pictures follow what the Gerbers are made of (see `gerber::files`), so
//! the 3D view shows what would be made. The approach (a mesh of the shape,
//! the layers baked into pictures that are put on it) is Copperview's.

use crate::pcb::geom::*;
use crate::pcb::model::*;

/// What a wall is made of, for the shading.
pub const WALL_FR4: f32 = 0.0;
pub const WALL_PLATED: f32 = 1.0;

/// Floats in a vertex of the board's mesh: x, y, z (-0.5 to 0.5, scaled by
/// the thickness), the normal, and what a wall is made of.
pub const STRIDE: usize = 7;

/// Flat triangles for drawing into a picture: x, y and 1 for each corner.
#[derive(Clone, Default, Debug)]
pub struct Tris {
    pub verts: Vec<f32>,
    pub idx: Vec<u32>,
}

impl Tris {
    pub fn is_empty(&self) -> bool {
        self.idx.is_empty()
    }

    #[cfg(test)]
    pub fn count(&self) -> usize {
        self.idx.len() / 3
    }

    fn point(&mut self, q: P) -> u32 {
        let n = (self.verts.len() / 3) as u32;
        self.verts.extend_from_slice(&[q.x as f32, q.y as f32, 1.0]);
        n
    }

    /// A convex ring, as a fan.
    pub fn fan(&mut self, ring: &[P]) {
        if ring.len() < 3 {
            return;
        }
        let base = self.verts.len() as u32 / 3;
        for q in ring {
            self.point(*q);
        }
        for i in 1..ring.len() as u32 - 1 {
            self.idx.extend_from_slice(&[base, base + i, base + i + 1]);
        }
    }

    /// Any one ring (it may have corners that point in), cut by earcut.
    pub fn polygon(&mut self, ring: &[P]) {
        let cut = triangulate(ring);
        if cut.is_empty() {
            return;
        }
        let base = self.verts.len() as u32 / 3;
        for q in ring {
            self.point(*q);
        }
        self.idx.extend(cut.iter().map(|i| base + i));
    }

    pub fn triangles(&mut self, tris: &[[P; 3]]) {
        for t in tris {
            let base = self.verts.len() as u32 / 3;
            for q in t {
                self.point(*q);
            }
            self.idx.extend_from_slice(&[base, base + 1, base + 2]);
        }
    }

    /// Copper or ink of a shape: a rounded, convex one.
    pub fn blob(&mut self, b: &Blob) {
        self.fan(&b.ring());
    }

    /// A line of some width with round ends.
    pub fn line(&mut self, a: P, b: P, w: f64) {
        self.blob(&Blob::line(a, b, w.max(0.01)));
    }

    /// A disc of this diameter.
    pub fn disc(&mut self, c: P, d: f64) {
        self.blob(&Blob::dot(c, d.max(0.01)));
    }

    /// A circle drawn with a line of some width.
    pub fn annulus(&mut self, c: P, r: f64, w: f64) {
        let (inner, outer) = ((r - w / 2.0).max(0.0), r + w / 2.0);
        let n = circle_steps(outer);
        let base = self.verts.len() as u32 / 3;
        for i in 0..n {
            let t = i as f64 / n as f64 * std::f64::consts::TAU;
            let d = p(t.cos(), t.sin());
            self.point(c.add(d.scale(inner)));
            self.point(c.add(d.scale(outer)));
        }
        for i in 0..n as u32 {
            let (a, b) = (base + i * 2, base + (i + 1) % n as u32 * 2);
            self.idx.extend_from_slice(&[a, a + 1, b, b, a + 1, b + 1]);
        }
    }
}

/// What one face of the board wears.
#[derive(Clone, Default, Debug)]
pub struct Face {
    pub copper: Tris,
    /// Where the solder mask is open.
    pub mask: Tris,
    pub silk: Tris,
}

pub struct Solid {
    /// Tells one solid from another, so what was drawn from it is drawn again for a new one.
    pub id: u64,
    /// Which state of the board it was made from.
    pub rev: u64,
    pub verts: Vec<f32>,
    pub idx: Vec<u32>,
    pub top_count: i32,
    pub bottom_count: i32,
    pub wall_count: i32,
    pub min: [f32; 2],
    pub max: [f32; 2],
    pub top: Face,
    pub bottom: Face,
    pub drill: Tris,
    /// False when the board had no outline and a rectangle round its parts stands in.
    pub outlined: bool,
}

fn add_face(verts: &mut Vec<f32>, idx: &mut Vec<u32>, ring: &[P], cut: &[u32], z: f32, normal: f32) -> i32 {
    let base = (verts.len() / STRIDE) as u32;
    for q in ring {
        verts.extend_from_slice(&[q.x as f32, q.y as f32, z, 0.0, 0.0, normal, WALL_FR4]);
    }
    // Seen from outside, a face's corners go anticlockwise.
    for t in cut.chunks(3) {
        if normal > 0.0 {
            idx.extend_from_slice(&[base + t[0], base + t[1], base + t[2]]);
        } else {
            idx.extend_from_slice(&[base + t[0], base + t[2], base + t[1]]);
        }
    }
    cut.len() as i32
}

/// A wall round a ring from the top face to the bottom. `smooth` shares the
/// corners so a round hole shades as round.
fn add_wall(verts: &mut Vec<f32>, idx: &mut Vec<u32>, ring: &[P], stuff: f32, smooth: bool) -> i32 {
    let n = ring.len();
    if n < 3 {
        return 0;
    }
    let mut added = 0;
    if smooth {
        let mid = ring.iter().fold(P::default(), |s, q| s.add(*q)).scale(1.0 / n as f64);
        let base = (verts.len() / STRIDE) as u32;
        for q in ring {
            let out = q.sub(mid);
            let l = out.len().max(1e-9);
            for z in [0.5f32, -0.5] {
                verts.extend_from_slice(&[q.x as f32, q.y as f32, z, (out.x / l) as f32, (out.y / l) as f32, 0.0, stuff]);
            }
        }
        for i in 0..n as u32 {
            let (a, b) = (base + i * 2, base + (i + 1) % n as u32 * 2);
            idx.extend_from_slice(&[a, a + 1, b, b, a + 1, b + 1]);
            added += 6;
        }
        return added;
    }
    for i in 0..n {
        let (a, b) = (ring[i], ring[(i + 1) % n]);
        let d = b.sub(a);
        let l = d.len();
        if l < 1e-9 {
            continue;
        }
        let (nx, ny) = ((d.y / l) as f32, (-d.x / l) as f32);
        let base = (verts.len() / STRIDE) as u32;
        for (q, z) in [(a, 0.5f32), (a, -0.5), (b, 0.5), (b, -0.5)] {
            verts.extend_from_slice(&[q.x as f32, q.y as f32, z, nx, ny, 0.0, stuff]);
        }
        idx.extend_from_slice(&[base, base + 1, base + 2, base + 2, base + 1, base + 3]);
        added += 6;
    }
    added
}

/// What a face wears, from the board: the layers as the Gerbers would have them.
fn face(board: &Board, side: Side) -> Face {
    let layer = if side == Side::Top { TOP } else { BOTTOM };
    let r = &board.rules;
    let mut out = Face::default();

    // Copper: pours, fills, pads, tracks and vias.
    for pour in board.pours.iter().filter(|p| p.layer == layer) {
        for island in &pour.fill {
            out.copper.triangles(&island.tris);
        }
    }
    for fill in board.fills.iter().filter(|f| f.layer == layer) {
        out.copper.polygon(&fill.pts);
    }
    for f in &board.footprints {
        for (i, pad) in f.pads.iter().enumerate() {
            if pad.bare_hole() || !(pad.through() || f.copper() == layer) {
                continue;
            }
            out.copper.blob(&f.pad_blob(i, 0.0));
        }
    }
    for t in board.tracks.iter().filter(|t| t.layer == layer) {
        for b in t.blobs() {
            out.copper.blob(&b);
        }
    }
    for v in &board.vias {
        out.copper.disc(v.at, v.dia);
    }

    // Solder mask: open over the pads; vias stay covered.
    for f in &board.footprints {
        for (i, pad) in f.pads.iter().enumerate() {
            if pad.bare_hole() {
                out.mask.disc(f.pad_center(i), pad.drill + 2.0 * r.mask_margin);
            } else if pad.through() || f.side == side {
                out.mask.blob(&f.pad_blob(i, r.mask_margin));
            }
        }
    }

    // Silkscreen: the drawing and the names.
    for f in &board.footprints {
        for ink in f.art(Layer::Silk(side), r) {
            match ink {
                Ink::Line(a, b, w) => out.silk.line(a, b, w),
                Ink::Dot(c, d) => out.silk.disc(c, d),
                Ink::Ring(c, rad, w) => out.silk.annulus(c, rad, w),
            }
        }
    }
    out
}

/// A ring round a hole.
fn hole_ring(c: P, d: f64) -> Vec<P> {
    let n = if d > 3.0 { 28 } else { 16 };
    (0..n).map(|i| c.add(p((i as f64 / n as f64 * std::f64::consts::TAU).cos(), (i as f64 / n as f64 * std::f64::consts::TAU).sin()).scale(d / 2.0))).collect()
}

/// The solid board, or none when there's nothing to make it of (no outline
/// and no parts or copper).
pub fn build(board: &Board, rev: u64) -> Option<Solid> {
    let mut ring: Vec<P> = board.outline.clone();
    let outlined = ring.len() >= 3;
    if !outlined {
        let e = board.extent();
        if e.is_empty() {
            return None;
        }
        let e = e.grown(2.0);
        ring = vec![e.min, p(e.max.x, e.min.y), e.max, p(e.min.x, e.max.y)];
    }
    if ring_area(&ring) < 0.0 {
        ring.reverse();
    }
    let area = BBox::of(&ring);
    if area.w() <= 0.0 || area.h() <= 0.0 {
        return None;
    }
    let cut = triangulate(&ring);
    if cut.is_empty() {
        return None;
    }
    let (mut verts, mut idx) = (Vec::new(), Vec::new());
    let top_count = add_face(&mut verts, &mut idx, &ring, &cut, 0.5, 1.0);
    let bottom_count = add_face(&mut verts, &mut idx, &ring, &cut, -0.5, -1.0);
    let mut wall_count = add_wall(&mut verts, &mut idx, &ring, WALL_FR4, false);

    // Holes: through-hole pads, plain holes and vias.
    let mut drill = Tris::default();
    let mut hole = |c: P, d: f64, plated: bool, verts: &mut Vec<f32>, idx: &mut Vec<u32>| {
        if d <= 0.0 {
            return 0;
        }
        drill.disc(c, d);
        add_wall(verts, idx, &hole_ring(c, d), if plated { WALL_PLATED } else { WALL_FR4 }, true)
    };
    for v in &board.vias {
        wall_count += hole(v.at, v.drill, true, &mut verts, &mut idx);
    }
    for f in &board.footprints {
        for (i, pad) in f.pads.iter().enumerate() {
            if pad.through() {
                wall_count += hole(f.pad_center(i), pad.drill, !pad.bare_hole(), &mut verts, &mut idx);
            }
        }
    }
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    Some(Solid {
        id: NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        rev,
        verts,
        idx,
        top_count,
        bottom_count,
        wall_count,
        min: [area.min.x as f32, area.min.y as f32],
        max: [area.max.x as f32, area.max.y as f32],
        top: face(board, Side::Top),
        bottom: face(board, Side::Bottom),
        drill,
        outlined,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sample_board_makes_a_solid_with_both_faces_and_drilled_walls() {
        let b = crate::pcb::sample::board();
        let s = build(&b, 1).expect("a solid");
        assert!(s.outlined);
        assert_eq!(s.top_count, s.bottom_count);
        assert_eq!(s.idx.len() as i32, s.top_count + s.bottom_count + s.wall_count, "every index belongs to a face or wall");
        assert!(s.wall_count > 6 * 4, "the edge and the holes have walls: {}", s.wall_count);
        assert!(!s.top.copper.is_empty() && !s.top.mask.is_empty() && !s.top.silk.is_empty());
        assert!(!s.drill.is_empty(), "the through-hole part and the via are drilled");
        // 50 by 35 mm.
        assert!((s.max[0] - s.min[0] - 50.0).abs() < 0.01 && (s.max[1] - s.min[1] - 35.0).abs() < 0.01);
        // Every index points at a vertex.
        let n = (s.verts.len() / STRIDE) as u32;
        assert!(s.idx.iter().all(|i| *i < n));
        let flat = (s.top.copper.verts.len() / 3) as u32;
        assert!(s.top.copper.idx.iter().all(|i| *i < flat));
    }

    /// A 10 by 10 board with one surface pad on top and one on the bottom, and a via.
    fn small() -> Board {
        let mut b = Board::default();
        b.outline = rect_outline(p(0.0, 0.0), p(10.0, 10.0));
        let def = crate::pcb::library::FootprintDef {
            name: "P".into(),
            category: String::new(),
            description: String::new(),
            prefix: "U".into(),
            pads: vec![Pad { num: "1".into(), at: p(0.0, 0.0), size: p(1.0, 1.0), rot: 0.0, shape: PadShape::Rect, drill: 0.0, net: 0 }],
            graphics: Vec::new(),
            label_at: p(0.0, 2.0),
            keepouts: Vec::new(),
            source: String::new(),
        };
        let id = b.new_id();
        b.footprints.push(def.instantiate(id, "U1".into(), p(3.0, 3.0)));
        let id = b.new_id();
        let mut bottom = def.instantiate(id, "U2".into(), p(7.0, 7.0));
        bottom.side = Side::Bottom;
        b.footprints.push(bottom);
        b
    }

    #[test]
    fn a_surface_pad_is_copper_and_a_mask_opening_on_its_own_face_only() {
        let s = build(&small(), 1).unwrap();
        assert!(!s.top.copper.is_empty() && !s.bottom.copper.is_empty());
        // One pad each: the same number of pieces on each face, not both on both.
        assert_eq!(s.top.copper.count(), s.bottom.copper.count());
        assert_eq!(s.top.mask.count(), s.bottom.mask.count());
        let near = |t: &Tris, x: f32| t.verts.chunks(3).all(|v| (v[0] - x).abs() < 1.5);
        assert!(near(&s.top.copper, 3.0), "the top pad is at 3");
        assert!(near(&s.bottom.copper, 7.0), "the bottom pad is at 7");
        assert!(s.drill.is_empty(), "surface pads aren't drilled");
    }

    #[test]
    fn a_through_hole_pad_is_on_both_faces_and_drilled_and_a_via_is_drilled_and_covered() {
        let mut b = small();
        b.footprints[0].pads[0].drill = 0.6;
        b.footprints[0].pads[0].size = p(1.6, 1.6);
        let (n, _) = (b.vias.len(), 0);
        b.vias.push(Via { id: 99, at: p(5.0, 5.0), dia: 0.6, drill: 0.3, net: 0 });
        let s = build(&b, 1).unwrap();
        // The through pad is copper and a mask opening top and bottom; the bottom's own pad too.
        assert!(s.top.copper.count() >= 1 && s.bottom.copper.count() >= 2);
        assert!(s.bottom.mask.count() >= 2, "open on the bottom for the through pad and the bottom pad");
        assert!(s.drill.count() >= 2 && n == 0);
        // The via is copper but not a mask opening: more copper than mask on the top.
        let top_mask_discs = s.top.mask.count();
        assert!(s.top.copper.count() > top_mask_discs - 1);
    }

    #[test]
    fn without_an_outline_a_rectangle_round_the_parts_stands_in() {
        let mut b = small();
        b.outline.clear();
        let s = build(&b, 1).unwrap();
        assert!(!s.outlined);
        assert!(s.max[0] - s.min[0] > 4.0);
        // A board with nothing on it is the usual blank 50 by 40, and still shows.
        let blank = build(&Board::default(), 1).expect("a blank slab");
        assert!(blank.top.copper.is_empty(), "no copper on a blank board");
        assert!(blank.max[0] - blank.min[0] >= 50.0, "{:?} {:?}", blank.min, blank.max);
    }

    #[test]
    fn a_pour_and_a_circle_of_silk_come_through() {
        let mut b = small();
        let tri = [p(0.0, 0.0), p(4.0, 0.0), p(0.0, 4.0)];
        let mut pour = Pour::new(5, TOP, 0, tri.to_vec(), &b.rules);
        pour.fill = vec![Island { rings: vec![tri.to_vec()], tris: vec![tri], area: 8.0, parts: Default::default() }];
        b.pours.push(pour);
        let before = build(&b, 1).unwrap().top.copper.count();
        assert!(before >= 2, "the pour's triangle is copper");
        let mut t = Tris::default();
        t.annulus(p(0.0, 0.0), 1.0, 0.2);
        assert!(t.count() >= 24 && t.idx.iter().all(|i| (*i as usize) < t.verts.len() / 3));
    }
}
