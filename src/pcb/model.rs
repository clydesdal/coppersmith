//! What a board is made of. This is also the saved file, written as JSON.

use crate::pcb::geom::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize, PartialOrd, Ord)]
pub enum Side {
    Top,
    Bottom,
}

impl Side {
    pub fn other(self) -> Side {
        match self {
            Side::Top => Side::Bottom,
            Side::Bottom => Side::Top,
        }
    }
}

/// Copper 0 is the top and 31 the bottom, whatever the layer count, so
/// changing from two layers to four leaves the outer tracks where they are.
pub const TOP: u8 = 0;
pub const BOTTOM: u8 = 31;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize, PartialOrd, Ord)]
pub enum Layer {
    Cu(u8),
    Silk(Side),
    Mask(Side),
    Paste(Side),
    Fab(Side),
    Court(Side),
    Edge,
}

impl Layer {
    pub fn side(self) -> Option<Side> {
        match self {
            Layer::Cu(TOP) => Some(Side::Top),
            Layer::Cu(BOTTOM) => Some(Side::Bottom),
            Layer::Cu(_) | Layer::Edge => None,
            Layer::Silk(s) | Layer::Mask(s) | Layer::Paste(s) | Layer::Fab(s) | Layer::Court(s) => Some(s),
        }
    }

    /// The same layer seen from the other side, for parts flipped underneath.
    pub fn flipped(self) -> Layer {
        match self {
            Layer::Cu(TOP) => Layer::Cu(BOTTOM),
            Layer::Cu(BOTTOM) => Layer::Cu(TOP),
            Layer::Silk(s) => Layer::Silk(s.other()),
            Layer::Mask(s) => Layer::Mask(s.other()),
            Layer::Paste(s) => Layer::Paste(s.other()),
            Layer::Fab(s) => Layer::Fab(s.other()),
            Layer::Court(s) => Layer::Court(s.other()),
            other => other,
        }
    }

    pub fn on(self, side: Side) -> Layer {
        if side == Side::Bottom { self.flipped() } else { self }
    }

    pub fn name(self) -> String {
        let s = |side: Side| if side == Side::Top { "Top" } else { "Bottom" };
        match self {
            Layer::Cu(TOP) => "Top copper".into(),
            Layer::Cu(BOTTOM) => "Bottom copper".into(),
            Layer::Cu(n) => format!("Inner {n}"),
            Layer::Silk(side) => format!("{} silkscreen", s(side)),
            Layer::Mask(side) => format!("{} solder mask", s(side)),
            Layer::Paste(side) => format!("{} paste", s(side)),
            Layer::Fab(side) => format!("{} assembly", s(side)),
            Layer::Court(side) => format!("{} courtyard", s(side)),
            Layer::Edge => "Board outline".into(),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Debug, Serialize, Deserialize)]
pub enum PadShape {
    Circle,
    Rect,
    /// Corner radius as a share of the shorter side, up to a half.
    RoundRect(f64),
    Oval,
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct Pad {
    pub num: String,
    pub at: P,
    pub size: P,
    #[serde(default)]
    pub rot: f64,
    pub shape: PadShape,
    /// Zero for a surface pad. A hole with no copper round it is a plain hole.
    #[serde(default)]
    pub drill: f64,
    #[serde(default)]
    pub net: u32,
}

impl Pad {
    pub fn through(&self) -> bool {
        self.drill > 0.0
    }
    /// A hole with no ring of copper, like a mounting hole.
    pub fn bare_hole(&self) -> bool {
        self.through() && self.size.x.min(self.size.y) <= self.drill + 1e-6
    }

    /// The pad's copper in the footprint's own coordinates.
    pub fn blob_local(&self, grow: f64) -> Blob {
        let (w, h) = (self.size.x + 2.0 * grow, self.size.y + 2.0 * grow);
        match self.shape {
            PadShape::Circle => Blob::dot(self.at, w.max(0.0)),
            PadShape::Rect => Blob { core: rect_core(self.at, w, h, self.rot), r: 0.0 },
            PadShape::RoundRect(k) => {
                let r = (self.size.x.min(self.size.y) * k.clamp(0.0, 0.5) + grow).max(0.0);
                Blob { core: rect_core(self.at, (w - 2.0 * r).max(1e-6), (h - 2.0 * r).max(1e-6), self.rot), r }
            }
            PadShape::Oval => {
                let r = w.min(h) / 2.0;
                let half = if w >= h { p(w / 2.0 - r, 0.0) } else { p(0.0, h / 2.0 - r) };
                let half = half.rot(self.rot);
                Blob::line(self.at.sub(half), self.at.add(half), 2.0 * r)
            }
        }
    }
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub enum Graphic {
    Line { a: P, b: P, w: f64, layer: Layer },
    Circle { c: P, r: f64, w: f64, layer: Layer, fill: bool },
}

impl Graphic {
    pub fn layer(&self) -> Layer {
        match self {
            Graphic::Line { layer, .. } | Graphic::Circle { layer, .. } => *layer,
        }
    }
}

/// A part on the board. Its pads and drawing are copied in from the library
/// when it's placed, so the board file stands on its own.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct Footprint {
    pub id: u32,
    pub def: String,
    pub refdes: String,
    #[serde(default)]
    pub value: String,
    pub at: P,
    #[serde(default)]
    pub rot: f64,
    pub side: Side,
    pub pads: Vec<Pad>,
    #[serde(default)]
    pub graphics: Vec<Graphic>,
    /// Where the name sits, in the footprint's own coordinates.
    #[serde(default)]
    pub label_at: P,
    #[serde(default)]
    pub locked: bool,
}

impl Footprint {
    /// From the footprint's own coordinates to the board's.
    pub fn place(&self, q: P) -> P {
        let q = if self.side == Side::Bottom { p(-q.x, q.y) } else { q };
        self.at.add(q.rot(self.rot)).rounded()
    }

    pub fn place_blob(&self, b: Blob) -> Blob {
        let mut core: Vec<P> = b.core.iter().map(|q| self.place(*q)).collect();
        // A mirror turns the corners round the other way.
        if self.side == Side::Bottom && core.len() > 2 {
            core.reverse();
        }
        Blob { core, r: b.r }
    }

    pub fn pad_blob(&self, i: usize, grow: f64) -> Blob {
        self.place_blob(self.pads[i].blob_local(grow))
    }

    pub fn pad_center(&self, i: usize) -> P {
        self.place(self.pads[i].at)
    }

    /// The copper layer a surface pad sits on.
    pub fn copper(&self) -> u8 {
        if self.side == Side::Top { TOP } else { BOTTOM }
    }

    /// The part's footprint area: the courtyard if it has one, else the pads.
    pub fn bbox(&self) -> BBox {
        let mut b = BBox::default();
        for g in &self.graphics {
            if let Graphic::Line { a, b: e, layer: Layer::Court(_), .. } = g {
                b.add(self.place(*a));
                b.add(self.place(*e));
            }
        }
        if b.is_empty() {
            for i in 0..self.pads.len() {
                b.union(&self.pad_blob(i, 0.0).bbox());
            }
            b = b.grown(0.25);
        }
        if b.is_empty() {
            b = BBox::of(&[self.at]).grown(0.5);
        }
        b
    }

    pub fn label_pos(&self) -> P {
        self.place(self.label_at)
    }
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct Track {
    /// For an arc: a point on the way from `a` to `b`. None for straight.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mid: Option<P>,
    pub id: u32,
    pub a: P,
    pub b: P,
    pub w: f64,
    pub layer: u8,
    #[serde(default)]
    pub net: u32,
}

impl Track {
    /// The straight track's copper, or the chord of an arc's.
    pub fn blob(&self) -> Blob {
        Blob::line(self.a, self.b, self.w)
    }

    /// The copper, as pieces: one for a straight track, many for an arc.
    pub fn blobs(&self) -> Vec<Blob> {
        match self.mid {
            Some(m) => arc_blobs(self.a, m, self.b, self.w),
            None => vec![self.blob()],
        }
    }

    pub fn bbox(&self) -> BBox {
        let mut b = BBox::default();
        for bl in self.blobs() {
            b.union(&bl.bbox());
        }
        b
    }

    pub fn dist_to(&self, q: P) -> f64 {
        self.blobs().iter().map(|b| b.dist_to(q)).fold(f64::MAX, f64::min)
    }

    /// The centre line, as points.
    pub fn path(&self) -> Vec<P> {
        match self.mid {
            Some(m) => arc_points(self.a, m, self.b),
            None => vec![self.a, self.b],
        }
    }

    pub fn length(&self) -> f64 {
        match self.mid {
            Some(m) => arc_length(self.a, m, self.b),
            None => self.a.dist(self.b),
        }
    }

    /// Moves the ends; an arc keeps its bulge.
    pub fn set_ends(&mut self, a: P, b: P) {
        if let Some(m) = self.mid {
            self.mid = Some(refit_mid(self.a, m, self.b, a, b));
        }
        self.a = a;
        self.b = b;
    }

    /// The outline, for drawing round a picked track.
    pub fn ring(&self) -> Vec<P> {
        let Some(m) = self.mid else { return self.blob().ring() };
        let Some((c, r, a0, sweep)) = arc_sweep(self.a, m, self.b) else { return self.blob().ring() };
        let h = self.w / 2.0;
        let n = 48usize;
        let at = |rad: f64, t: f64| c.add(p(t.cos(), t.sin()).scale(rad));
        let mut ring: Vec<P> = (0..=n).map(|i| at(r + h, a0 + sweep * i as f64 / n as f64)).collect();
        // Round the far end, back along the inside, round the near end.
        let end = a0 + sweep;
        let dir = sweep.signum();
        for k in 1..12 {
            let t = k as f64 / 12.0 * std::f64::consts::PI;
            let e = at(r, end);
            let out = p(end.cos(), end.sin());
            let along = p(-end.sin(), end.cos()).scale(dir);
            ring.push(e.add(out.scale(h * t.cos())).add(along.scale(h * t.sin())));
        }
        ring.extend((0..=n).rev().map(|i| at(r - h, a0 + sweep * i as f64 / n as f64)));
        for k in 1..12 {
            let t = k as f64 / 12.0 * std::f64::consts::PI;
            let s = at(r, a0);
            let out = p(a0.cos(), a0.sin());
            let back = p(a0.sin(), -a0.cos()).scale(dir);
            ring.push(s.sub(out.scale(h * t.cos())).add(back.scale(h * t.sin())));
        }
        ring
    }
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct Via {
    pub id: u32,
    pub at: P,
    pub dia: f64,
    pub drill: f64,
    #[serde(default)]
    pub net: u32,
}

impl Via {
    pub fn blob(&self) -> Blob {
        Blob::dot(self.at, self.dia)
    }
}

/// How a pour meets the pads of its own net.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize, Default)]
pub enum Connect {
    /// Thermal relief: a gap round the pad, crossed by four spokes, so the
    /// pad can be soldered without the pour soaking up the heat.
    #[default]
    Relief,
    /// Poured solid right up to the pad.
    Direct,
}

/// A copper pour: an area of copper on one layer that fills itself in round
/// everything of other nets, keeping its clearance. A plane is a pour that
/// covers a whole inner layer (or, split, a part of one).
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct Pour {
    pub id: u32,
    pub layer: u8,
    #[serde(default)]
    pub net: u32,
    /// Where it may pour, counter-clockwise. Empty: the whole board.
    #[serde(default)]
    pub outline: Vec<P>,
    /// A power plane rather than a pour: kept clear of other nets' holes by
    /// `clearance` measured from the hole, and never trimmed of islands.
    #[serde(default)]
    pub plane: bool,
    /// Poured in this order; each keeps clear of those poured before it.
    #[serde(default)]
    pub order: u32,
    /// How far it keeps from copper of other nets.
    pub clearance: f64,
    /// How far it keeps back from the board's edge.
    pub edge: f64,
    #[serde(default)]
    pub connect: Connect,
    /// Thermal relief spoke width and the gap round the pad.
    pub spoke: f64,
    pub gap: f64,
    /// Takes out pieces that don't touch anything of its net.
    #[serde(default = "yes")]
    pub remove_islands: bool,
    /// Pieces smaller than this (mm²) are taken out.
    #[serde(default)]
    pub min_area: f64,
    /// Necks narrower than this are taken out.
    #[serde(default)]
    pub neck: f64,
    /// What it poured into: worked out from the board, not saved.
    #[serde(skip)]
    pub fill: Vec<Island>,
    /// A fingerprint of everything `fill` was worked out from, so a pour
    /// whose surroundings haven't changed isn't poured again. Not saved.
    #[serde(skip)]
    pub sig: u64,
}

fn yes() -> bool {
    true
}

/// One piece of poured copper: its outline and holes (outline
/// counter-clockwise, holes clockwise) and the same cut into small
/// triangles for drawing and for checking.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct Island {
    pub rings: Vec<Vec<P>>,
    pub tris: Vec<[P; 3]>,
    pub area: f64,
}

impl Pour {
    pub fn new(id: u32, layer: u8, net: u32, outline: Vec<P>, rules: &Rules) -> Pour {
        Pour {
            id,
            layer,
            net,
            outline,
            plane: false,
            order: id,
            clearance: rules.clearance.max(0.25),
            edge: rules.edge_clearance,
            connect: Connect::Relief,
            spoke: rules.track.max(rules.min_track),
            gap: rules.clearance.max(0.25),
            remove_islands: true,
            min_area: 0.0,
            neck: 0.0,
            fill: Vec::new(),
            sig: 0,
        }
    }

    /// Where it may pour: its own outline, or for an outline-less plane the board's.
    pub fn area<'a>(&'a self, board: &'a Board) -> &'a [P] {
        if self.outline.len() >= 3 { &self.outline } else { &board.outline }
    }

    pub fn copper_area(&self) -> f64 {
        self.fill.iter().map(|i| i.area).sum()
    }
}

/// Solid copper of a fixed shape: an Altium fill or copper region.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct Fill {
    pub id: u32,
    pub layer: u8,
    #[serde(default)]
    pub net: u32,
    /// Counter-clockwise.
    pub pts: Vec<P>,
}

impl Fill {
    /// The shape as convex pieces: itself if it's convex, or triangles.
    pub fn pieces(&self) -> Vec<Vec<P>> {
        if is_convex(&self.pts) {
            return vec![self.pts.clone()];
        }
        let idx = triangulate(&self.pts);
        idx.chunks(3).map(|t| {
            let mut tri = vec![self.pts[t[0] as usize], self.pts[t[1] as usize], self.pts[t[2] as usize]];
            if ring_area(&tri) < 0.0 {
                tri.reverse();
            }
            tri
        }).collect()
    }
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct Net {
    pub id: u32,
    pub name: String,
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct Rules {
    pub clearance: f64,
    pub track: f64,
    pub via_dia: f64,
    pub via_drill: f64,
    pub min_track: f64,
    pub min_drill: f64,
    pub min_ring: f64,
    pub edge_clearance: f64,
    pub mask_margin: f64,
    pub silk_width: f64,
    pub text_height: f64,
    /// The shortest a 45° step may be where a 45° track turns, so a corner
    /// never closes up into a right angle.
    #[serde(default = "default_min_diag")]
    pub min_diag: f64,
}

fn default_min_diag() -> f64 {
    0.5
}

impl Default for Rules {
    /// What most low-cost board makers will build without asking.
    fn default() -> Self {
        Rules {
            clearance: 0.2,
            track: 0.25,
            via_dia: 0.6,
            via_drill: 0.3,
            min_track: 0.15,
            min_drill: 0.3,
            min_ring: 0.13,
            edge_clearance: 0.3,
            mask_margin: 0.05,
            silk_width: 0.15,
            text_height: 1.0,
            min_diag: default_min_diag(),
        }
    }
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct Board {
    pub format: u32,
    pub name: String,
    /// How many copper layers: 2, 4 or 6.
    pub copper: u8,
    /// The edge of the board, a closed ring.
    pub outline: Vec<P>,
    pub footprints: Vec<Footprint>,
    pub tracks: Vec<Track>,
    pub vias: Vec<Via>,
    #[serde(default)]
    pub pours: Vec<Pour>,
    #[serde(default)]
    pub fills: Vec<Fill>,
    /// Net 0 is "not connected"; it's never stored here.
    pub nets: Vec<Net>,
    pub rules: Rules,
    pub next_id: u32,
}

impl Default for Board {
    fn default() -> Self {
        Board {
            format: 1,
            name: "Untitled board".into(),
            copper: 2,
            outline: rect_outline(p(0.0, 0.0), p(50.0, 40.0)),
            footprints: Vec::new(),
            tracks: Vec::new(),
            vias: Vec::new(),
            pours: Vec::new(),
            fills: Vec::new(),
            nets: Vec::new(),
            rules: Rules::default(),
            next_id: 1,
        }
    }
}

pub fn rect_outline(a: P, b: P) -> Vec<P> {
    vec![p(a.x, a.y), p(b.x, a.y), p(b.x, b.y), p(a.x, b.y)]
}

impl Board {
    pub fn new_id(&mut self) -> u32 {
        self.next_id += 1;
        self.next_id - 1
    }

    /// The copper layers from top to bottom.
    pub fn copper_layers(&self) -> Vec<u8> {
        let mut v = vec![TOP];
        v.extend(1..self.copper.saturating_sub(1));
        v.push(BOTTOM);
        v
    }

    pub fn net_name(&self, id: u32) -> &str {
        if id == 0 {
            return "No net";
        }
        self.nets.iter().find(|n| n.id == id).map(|n| n.name.as_str()).unwrap_or("?")
    }

    pub fn net_by_name(&self, name: &str) -> Option<u32> {
        self.nets.iter().find(|n| n.name.eq_ignore_ascii_case(name)).map(|n| n.id)
    }

    /// The net with this name, made if it isn't there yet.
    pub fn net(&mut self, name: &str) -> u32 {
        if let Some(id) = self.net_by_name(name) {
            return id;
        }
        let id = self.nets.iter().map(|n| n.id).max().unwrap_or(0) + 1;
        self.nets.push(Net { id, name: name.to_string() });
        id
    }

    pub fn fp(&self, id: u32) -> Option<&Footprint> {
        self.footprints.iter().find(|f| f.id == id)
    }
    pub fn fp_mut(&mut self, id: u32) -> Option<&mut Footprint> {
        self.footprints.iter_mut().find(|f| f.id == id)
    }
    pub fn track(&self, id: u32) -> Option<&Track> {
        self.tracks.iter().find(|t| t.id == id)
    }
    pub fn via(&self, id: u32) -> Option<&Via> {
        self.vias.iter().find(|v| v.id == id)
    }
    pub fn pour(&self, id: u32) -> Option<&Pour> {
        self.pours.iter().find(|v| v.id == id)
    }
    pub fn fill(&self, id: u32) -> Option<&Fill> {
        self.fills.iter().find(|v| v.id == id)
    }

    /// Pours every pour afresh from the board as it is.
    pub fn repour(&mut self) {
        crate::pcb::pour::repour(self);
    }

    pub fn outline_bbox(&self) -> BBox {
        BBox::of(&self.outline)
    }

    /// Everything on the board, for fitting it to the window.
    pub fn extent(&self) -> BBox {
        let mut b = self.outline_bbox();
        for f in &self.footprints {
            b.union(&f.bbox());
        }
        for t in &self.tracks {
            b.union(&t.bbox());
        }
        for f in &self.fills {
            b.union(&BBox::of(&f.pts));
        }
        if b.is_empty() {
            b = BBox::of(&[p(0.0, 0.0), p(50.0, 40.0)]);
        }
        b
    }

    /// The next free name with this prefix, like R7.
    pub fn next_refdes(&self, prefix: &str) -> String {
        let used: std::collections::HashSet<&str> = self.footprints.iter().map(|f| f.refdes.as_str()).collect();
        (1..).map(|n| format!("{prefix}{n}")).find(|r| !used.contains(r.as_str())).unwrap()
    }

    pub fn save(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    pub fn load(text: &str) -> Result<Board, String> {
        let mut b: Board = serde_json::from_str(text).map_err(|e| format!("This isn't a board file I can read ({e})"))?;
        let top = b
            .footprints
            .iter()
            .map(|f| f.id)
            .chain(b.tracks.iter().map(|t| t.id))
            .chain(b.vias.iter().map(|v| v.id))
            .chain(b.pours.iter().map(|v| v.id))
            .chain(b.fills.iter().map(|v| v.id))
            .max()
            .unwrap_or(0);
        b.next_id = b.next_id.max(top + 1);
        b.repour();
        Ok(b)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bottom_parts_are_mirrored() {
        let mut f = Footprint {
            id: 1,
            def: "x".into(),
            refdes: "R1".into(),
            value: String::new(),
            at: p(10.0, 10.0),
            rot: 0.0,
            side: Side::Top,
            pads: vec![Pad {
                num: "1".into(),
                at: p(-1.0, 0.0),
                size: p(1.0, 1.0),
                rot: 0.0,
                shape: PadShape::Rect,
                drill: 0.0,
                net: 0,
            }],
            graphics: vec![],
            label_at: p(0.0, 2.0),
            locked: false,
        };
        assert_eq!(f.pad_center(0), p(9.0, 10.0));
        f.side = Side::Bottom;
        assert_eq!(f.pad_center(0), p(11.0, 10.0));
        assert!(ring_area(&f.pad_blob(0, 0.0).core) > 0.0, "corners stay counter-clockwise");
    }

    #[test]
    fn saves_and_loads() {
        let mut b = Board::default();
        let n = b.net("GND");
        b.tracks.push(Track { mid: None, id: 7, a: p(0.0, 0.0), b: p(1.0, 0.0), w: 0.25, layer: TOP, net: n });
        let again = Board::load(&b.save()).unwrap();
        assert_eq!(again.tracks, b.tracks);
        assert!(again.next_id > 7);
    }
}

/// One stroke of drawing on a non-copper layer, already on the board.
#[derive(Clone, Copy, Debug)]
pub enum Ink {
    Line(P, P, f64),
    Dot(P, f64),
    Ring(P, f64, f64),
}

impl Footprint {
    /// The name's turn, kept readable: never upside down.
    pub fn label_rot(&self) -> f64 {
        let r = self.rot.rem_euclid(360.0);
        if r > 90.0 + 1e-6 && r <= 270.0 + 1e-6 { r - 180.0 } else { r }
    }

    /// What this part draws on a board layer, with its name on the silkscreen.
    pub fn art(&self, layer: Layer, rules: &Rules) -> Vec<Ink> {
        let mut out = Vec::new();
        let mirror = self.side == Side::Bottom;
        for g in &self.graphics {
            if g.layer().on(self.side) != layer {
                continue;
            }
            match *g {
                Graphic::Line { a, b, w, .. } => out.push(Ink::Line(self.place(a), self.place(b), w)),
                Graphic::Circle { c, r, w, fill, .. } => {
                    if fill {
                        out.push(Ink::Dot(self.place(c), 2.0 * r));
                    } else {
                        out.push(Ink::Ring(self.place(c), r, w));
                    }
                }
            }
        }
        let label_layer = Layer::Silk(self.side);
        if layer == label_layer && !self.refdes.is_empty() {
            let h = rules.text_height;
            let strokes = crate::pcb::font::strokes(&self.refdes, self.label_pos(), h, self.label_rot(), mirror);
            for s in strokes {
                if s.len() == 1 || (s.len() == 2 && s[0].same(s[1])) {
                    out.push(Ink::Dot(s[0], rules.silk_width));
                }
                for w in s.windows(2) {
                    out.push(Ink::Line(w[0], w[1], rules.silk_width));
                }
            }
        }
        out
    }
}
