//! Reading a KiCad board (`.kicad_pcb`) and its footprints (`.kicad_mod`).
//!
//! KiCad's Y runs down the page and Coppersmith's up, so every point is flipped.
//! A part on the bottom keeps its pads and drawing in KiCad as they look
//! underneath; here they're kept in the part's own terms (as seen from its own
//! side) and mirrored when it's placed, so those are turned back.

use super::sexpr::{Sx, parse};
use crate::pcb::altium::pcbdoc::ImportedBoard;
use crate::pcb::geom::*;
use crate::pcb::model::*;
use std::collections::{BTreeMap, HashMap};

/// What was left out or approximated, counted, to be told in a line each.
#[derive(Default)]
pub struct Left(BTreeMap<&'static str, usize>);

impl Left {
    pub fn add(&mut self, what: &'static str) {
        *self.0.entry(what).or_default() += 1;
    }

    pub fn notes(&self) -> Vec<String> {
        self.0.iter().map(|(what, n)| format!("{n} {what}")).collect()
    }
}

/// KiCad's point to ours.
pub fn pt(x: f64, y: f64) -> P {
    p(x, -y)
}

fn xy(n: &Sx) -> Option<P> {
    Some(pt(n.num_arg(0)?, n.num_arg(1)?))
}

fn child_pt(n: &Sx, name: &str) -> Option<P> {
    n.get(name).and_then(xy)
}

/// The circle through three points: its centre and radius; none if they're in a line.
pub fn circumcircle(a: P, m: P, c: P) -> Option<(P, f64)> {
    let (d1, d2) = (m.sub(a), c.sub(m));
    if d1.cross(d2).abs() < 1e-12 {
        return None;
    }
    let (ma, mb) = (a.lerp(m, 0.5), m.lerp(c, 0.5));
    // Where the perpendicular bisectors of a–m and m–c meet.
    let (n1, n2) = (p(-d1.y, d1.x), p(-d2.y, d2.x));
    let den = n1.cross(n2);
    if den.abs() < 1e-12 {
        return None;
    }
    let t = mb.sub(ma).cross(n2) / den;
    let centre = ma.add(n1.scale(t));
    Some((centre, centre.dist(a)))
}

/// The points of an arc from its ends and a point on its way: along it, a few degrees apart.
pub fn arc_points(a: P, m: P, c: P) -> Vec<P> {
    let Some((centre, r)) = circumcircle(a, m, c) else { return vec![a, c] };
    let cross = m.sub(a).cross(c.sub(m));
    let ang = |q: P| (q.y - centre.y).atan2(q.x - centre.x);
    let tau = std::f64::consts::TAU;
    let sweep = if cross > 0.0 { (ang(c) - ang(a)).rem_euclid(tau) } else { -((ang(a) - ang(c)).rem_euclid(tau)) };
    let n = ((sweep.abs() / (5.0_f64).to_radians()).ceil() as usize).max(2);
    let mut out: Vec<P> = (0..=n).map(|k| {
        let th = ang(a) + sweep * k as f64 / n as f64;
        p(centre.x + r * th.cos(), centre.y + r * th.sin()).rounded()
    }).collect();
    // The ends are the file's own, not worked out again.
    out[0] = a;
    *out.last_mut().unwrap() = c;
    out
}

/// A shape drawn on a layer.
pub enum Shape {
    Line(P, P),
    /// Joined points, closed or not.
    Poly(Vec<P>, bool),
    Circle(P, f64),
}

/// The shape of a `fp_line`, `gr_arc`, `fp_rect`… node, its points taken through `at`.
pub fn shape_of(n: &Sx, at: &dyn Fn(f64, f64) -> P, left: &mut Left) -> Option<Shape> {
    let head = n.head()?;
    let kind = head.strip_prefix("fp_").or_else(|| head.strip_prefix("gr_"))?;
    let point = |name: &str| n.get(name).and_then(|c| Some(at(c.num_arg(0)?, c.num_arg(1)?)));
    Some(match kind {
        "line" => Shape::Line(point("start")?, point("end")?),
        "rect" => {
            let (a, b) = (point("start")?, point("end")?);
            Shape::Poly(vec![a, p(b.x, a.y), b, p(a.x, b.y)], true)
        }
        "circle" => {
            let c = point("center")?;
            Shape::Circle(c, c.dist(point("end")?))
        }
        "arc" => match n.get("mid") {
            Some(_) => Shape::Poly(arc_points(point("start")?, point("mid")?, point("end")?), false),
            // The old form: a centre, a point on the arc, and how far it sweeps.
            None => {
                let (c, s) = (point("start")?, point("end")?);
                let sweep = n.child_num("angle")?.to_radians();
                let (v, r) = (s.sub(c), s.dist(c));
                let a0 = v.y.atan2(v.x);
                // KiCad's Y runs down, so a positive sweep goes clockwise on screen: ours turns the other way.
                let k = ((sweep.abs() / (5.0_f64).to_radians()).ceil() as usize).max(2);
                Shape::Poly((0..=k).map(|i| {
                    let th = a0 - sweep * i as f64 / k as f64;
                    p(c.x + r * th.cos(), c.y + r * th.sin()).rounded()
                }).collect(), false)
            }
        },
        "poly" => {
            let pts: Vec<P> = n.get("pts")?.all("xy").filter_map(|q| Some(at(q.num_arg(0)?, q.num_arg(1)?))).collect();
            if pts.len() < 2 {
                return None;
            }
            Shape::Poly(pts, true)
        }
        "curve" => {
            left.add("curves (Bézier) left out");
            return None;
        }
        _ => return None,
    })
}

fn stroke_width(n: &Sx) -> f64 {
    let w = n.get("stroke").and_then(|s| s.child_num("width")).or_else(|| n.child_num("width")).unwrap_or(0.12);
    if w > 0.0 { w } else { 0.1 }
}

fn is_filled(n: &Sx) -> bool {
    match n.get("fill") {
        Some(f) => matches!(f.str_arg(0), Some("yes") | Some("solid") | Some("filled")) || f.child_str("type") == Some("solid"),
        None => false,
    }
}

/// The silkscreen, courtyard and assembly layers a part's drawing can be on.
fn graphic_layer(name: &str) -> Option<Layer> {
    Some(match name {
        "F.SilkS" | "F.Silkscreen" => Layer::Silk(Side::Top),
        "B.SilkS" | "B.Silkscreen" => Layer::Silk(Side::Bottom),
        "F.CrtYd" | "F.Courtyard" => Layer::Court(Side::Top),
        "B.CrtYd" | "B.Courtyard" => Layer::Court(Side::Bottom),
        "F.Fab" => Layer::Fab(Side::Top),
        "B.Fab" => Layer::Fab(Side::Bottom),
        _ => return None,
    })
}

/// A footprint as KiCad has it: on a board, or in a library.
pub struct Fp {
    pub name: String,
    pub library: String,
    pub description: String,
    pub refdes: String,
    pub value: String,
    pub bottom: bool,
    pub at: P,
    pub rot: f64,
    pub locked: bool,
    pub label_at: P,
    pub label_height: Option<f64>,
    pub pads: Vec<Pad>,
    /// Surface pads on the other side of the board from the part, as pads of a part on that side at the same place.
    pub opposite: Vec<Pad>,
    pub graphics: Vec<Graphic>,
    /// Shapes it draws on the board's edge layer, in its own coordinates.
    pub edge: Vec<Vec<P>>,
}

fn pad_of(n: &Sx, bottom: bool, fp_rot: f64, local: &dyn Fn(f64, f64) -> P, net: &mut dyn FnMut(&Sx) -> u32, left: &mut Left) -> Option<Pad> {
    let a = n.args();
    let num = a.first()?.text()?.to_string();
    let kind = a.get(1).and_then(|k| k.text()).unwrap_or("smd");
    let shape = a.get(2).and_then(|k| k.text()).unwrap_or("rect");
    let at = n.get("at")?;
    let centre = local(at.num_arg(0)?, at.num_arg(1)?);
    let size = n.get("size")?;
    let (w, h) = (size.num_arg(0)?, size.num_arg(1).unwrap_or_else(|| size.num_arg(0).unwrap_or(0.0)));
    // The slot of an oval hole is drilled as its narrower width.
    let drill = match n.get("drill") {
        Some(d) if d.str_arg(0) == Some("oval") => d.num_arg(1).unwrap_or(0.0).min(d.num_arg(2).unwrap_or(f64::MAX)),
        Some(d) => d.num_arg(0).unwrap_or(0.0),
        None => 0.0,
    };
    let shape = match shape {
        "circle" => PadShape::Circle,
        "oval" if (w - h).abs() < 1e-9 => PadShape::Circle,
        "oval" => PadShape::Oval,
        "roundrect" => PadShape::RoundRect(n.child_num("roundrect_rratio").unwrap_or(0.25).clamp(0.0, 0.5)),
        "trapezoid" => {
            left.add("trapezoid pads drawn as rectangles");
            PadShape::Rect
        }
        "custom" => {
            left.add("custom-shaped pads drawn as their anchor");
            if n.get("options").and_then(|o| o.child_str("anchor")) == Some("circle") { PadShape::Circle } else { PadShape::Rect }
        }
        _ => PadShape::Rect,
    };
    // A hole with no copper: a mounting hole.
    let (w, h) = if kind == "np_thru_hole" && drill > 0.0 { (w.max(drill), h.max(drill)) } else { (w, h) };
    // The angle in a board's file is the pad's on the board, the part's turn included; here it's the pad's own.
    let on_board = at.num_arg(2).unwrap_or(0.0);
    let rot = if matches!(shape, PadShape::Circle) {
        0.0
    } else if bottom {
        (fp_rot - on_board).rem_euclid(360.0)
    } else {
        (on_board - fp_rot).rem_euclid(360.0)
    };
    let net = if kind == "np_thru_hole" { 0 } else { net(n) };
    Some(Pad { num, at: centre, size: p(w, h), rot, shape, drill, net })
}

/// Reads a `(footprint …)` or the older `(module …)`. `net` finds the net a node names.
pub fn parse_footprint(n: &Sx, net: &mut dyn FnMut(&Sx) -> u32, left: &mut Left) -> Fp {
    let id = n.args().first().and_then(|a| a.text()).unwrap_or("Footprint");
    let (library, name) = match id.split_once(':') {
        Some((l, nm)) => (l.to_string(), nm.to_string()),
        None => (String::new(), id.to_string()),
    };
    let bottom = n.child_str("layer").is_some_and(|l| l.starts_with("B."));
    let at = n.get("at");
    let (ax, ay, rot) = (at.and_then(|a| a.num_arg(0)).unwrap_or(0.0), at.and_then(|a| a.num_arg(1)).unwrap_or(0.0), at.and_then(|a| a.num_arg(2)).unwrap_or(0.0));
    // Its own coordinates: not turned, and from underneath turned back.
    let local = move |x: f64, y: f64| if bottom { p(-x, -y) } else { p(x, -y) };
    let mut fp = Fp {
        name,
        library,
        description: n.child_str("descr").unwrap_or("").to_string(),
        refdes: String::new(),
        value: String::new(),
        bottom,
        at: pt(ax, ay),
        rot: rot.rem_euclid(360.0),
        locked: n.flag("locked"),
        label_at: P::default(),
        label_height: None,
        pads: Vec::new(),
        opposite: Vec::new(),
        graphics: Vec::new(),
        edge: Vec::new(),
    };
    // Reference and value: properties in newer files, `fp_text` in older ones.
    let mut texts: Vec<(&str, &str, &Sx)> = Vec::new();
    for c in n.args() {
        match c.head() {
            Some("property") => {
                if let (Some(k), Some(v)) = (c.str_arg(0), c.str_arg(1)) {
                    texts.push((k, v, c));
                }
            }
            Some("fp_text") => {
                if let (Some(k), Some(v)) = (c.str_arg(0), c.str_arg(1)) {
                    texts.push((if k == "reference" { "Reference" } else if k == "value" { "Value" } else { "" }, v, c));
                }
            }
            _ => {}
        }
    }
    for (k, v, c) in texts {
        match k {
            "Reference" => {
                fp.refdes = v.to_string();
                if let Some(a) = c.get("at") {
                    fp.label_at = local(a.num_arg(0).unwrap_or(0.0), a.num_arg(1).unwrap_or(0.0));
                }
                fp.label_height = c.get("effects").and_then(|e| e.get("font")).and_then(|f| f.get("size")).and_then(|s| s.num_arg(0));
            }
            "Value" => fp.value = v.to_string(),
            _ => {}
        }
    }
    for c in n.args() {
        match c.head() {
            Some("pad") => {
                if let Some(mut pad) = pad_of(c, bottom, fp.rot, &local, net, left) {
                    // A surface pad on the far side from its part: seen from that side it's mirrored.
                    let layers: Vec<&str> = c.get("layers").map(|l| l.args().iter().filter_map(|x| x.text()).collect()).unwrap_or_default();
                    let on_top = layers.contains(&"F.Cu");
                    let on_bottom = layers.contains(&"B.Cu");
                    let far = pad.drill == 0.0 && (on_top != on_bottom) && on_top == bottom;
                    if far {
                        pad.at = p(-pad.at.x, pad.at.y);
                        pad.rot = (360.0 - pad.rot).rem_euclid(360.0);
                        fp.opposite.push(pad);
                    } else {
                        fp.pads.push(pad);
                    }
                }
            }
            Some("fp_line" | "fp_rect" | "fp_circle" | "fp_arc" | "fp_poly" | "fp_curve") => {
                let layer = c.child_str("layer").unwrap_or("");
                let Some(shape) = shape_of(c, &local, left) else { continue };
                if layer == "Edge.Cuts" {
                    match shape {
                        Shape::Line(a, b) => fp.edge.push(vec![a, b]),
                        Shape::Poly(mut v, closed) => {
                            if closed {
                                v.push(v[0]);
                            }
                            fp.edge.push(v);
                        }
                        Shape::Circle(c, r) => fp.edge.push(circle_ring(c, r)),
                    }
                    continue;
                }
                // Stored in the part's own terms: a part underneath has its sides swapped.
                let Some(own) = graphic_layer(layer).map(|l| if bottom { l.flipped() } else { l }) else {
                    left.add("drawings on layers other than silkscreen, courtyard and assembly");
                    continue;
                };
                let w = stroke_width(c);
                match shape {
                    Shape::Line(a, b) => fp.graphics.push(Graphic::Line { a, b, w, layer: own }),
                    Shape::Poly(v, closed) => {
                        for k in 0..v.len().saturating_sub(1) {
                            fp.graphics.push(Graphic::Line { a: v[k], b: v[k + 1], w, layer: own });
                        }
                        if closed && v.len() > 2 {
                            fp.graphics.push(Graphic::Line { a: v[v.len() - 1], b: v[0], w, layer: own });
                        }
                    }
                    Shape::Circle(cc, r) => fp.graphics.push(Graphic::Circle { c: cc, r, w, layer: own, fill: is_filled(c) }),
                }
            }
            Some("zone") => left.add("keep-out zones inside footprints left out"),
            _ => {}
        }
    }
    fp
}

/// A circle as a ring of points.
pub fn circle_ring(c: P, r: f64) -> Vec<P> {
    let n = 48;
    let mut ring: Vec<P> = (0..n).map(|k| {
        let th = std::f64::consts::TAU * k as f64 / n as f64;
        p(c.x + r * th.cos(), c.y + r * th.sin()).rounded()
    }).collect();
    ring.push(ring[0]);
    ring
}

/// Pieces of a board's edge joined end to end into rings; open runs that don't close are dropped.
pub fn rings_of(pieces: Vec<Vec<P>>) -> Vec<Vec<P>> {
    let near = |a: P, b: P| a.dist(b) < 1e-3;
    let mut rings: Vec<Vec<P>> = Vec::new();
    let mut open: Vec<Vec<P>> = Vec::new();
    for v in pieces {
        if v.len() >= 3 && near(v[0], *v.last().unwrap()) {
            rings.push(v[..v.len() - 1].to_vec());
        } else if v.len() >= 2 {
            open.push(v);
        }
    }
    while let Some(mut run) = open.pop() {
        loop {
            if run.len() >= 3 && near(run[0], *run.last().unwrap()) {
                run.pop();
                rings.push(run);
                break;
            }
            let end = *run.last().unwrap();
            let Some(i) = open.iter().position(|o| near(o[0], end) || near(*o.last().unwrap(), end)) else { break };
            let mut next = open.remove(i);
            if !near(next[0], end) {
                next.reverse();
            }
            run.extend(next.into_iter().skip(1));
        }
    }
    rings
}

/// A board's nets: the table old files have, and the names pads, tracks and zones give.
#[derive(Default)]
struct Nets {
    table: HashMap<i64, String>,
    ids: HashMap<String, u32>,
    order: Vec<String>,
}

impl Nets {
    fn name_of(&self, n: &Sx) -> Option<String> {
        let net = n.get("net")?;
        let a = net.args();
        let name = match (a.first(), a.get(1)) {
            (Some(id), Some(name)) if id.num().is_some() => name.text().map(String::from),
            (Some(x), None) => match x.num() {
                Some(id) => self.table.get(&(id as i64)).cloned(),
                None => x.text().map(String::from),
            },
            _ => None,
        };
        name.filter(|s| !s.is_empty())
    }

    fn id(&mut self, name: &str) -> u32 {
        if let Some(i) = self.ids.get(name) {
            return *i;
        }
        let id = self.order.len() as u32 + 1;
        self.ids.insert(name.to_string(), id);
        self.order.push(name.to_string());
        id
    }

    fn of(&mut self, n: &Sx) -> u32 {
        match self.name_of(n) {
            Some(name) => self.id(&name),
            None => 0,
        }
    }
}

/// The copper layer a KiCad layer name is: the top is 0, the inner ones count from 1, the bottom is 31.
fn copper_index(name: &str) -> Option<u8> {
    match name {
        "F.Cu" => Some(TOP),
        "B.Cu" => Some(BOTTOM),
        _ => name.strip_prefix("In")?.strip_suffix(".Cu")?.parse::<u8>().ok().filter(|n| (1..30).contains(n)),
    }
}

/// A KiCad board. `project` is the text of its `.kicad_pro`, if it has one beside it.
pub fn read(text: &str, name: &str, project: Option<&str>) -> Result<ImportedBoard, String> {
    let root = parse(text)?;
    if root.head() != Some("kicad_pcb") {
        return Err("isn't a KiCad board (it should start with kicad_pcb)".into());
    }
    let mut left = Left::default();
    let mut nets = Nets::default();
    for n in root.all("net") {
        if let (Some(id), Some(name)) = (n.num_arg(0), n.str_arg(1)) {
            nets.table.insert(id as i64, name.to_string());
        }
    }

    // The copper layers.
    let mut inner = 0u8;
    if let Some(layers) = root.get("layers") {
        for l in layers.args() {
            if let Some(i) = l.str_arg(0).and_then(copper_index).filter(|i| *i != TOP && *i != BOTTOM) {
                inner = inner.max(i);
            }
        }
    }
    let mut b = Board { name: name.to_string(), footprints: Vec::new(), outline: Vec::new(), ..Board::default() };
    b.copper = (inner + 2).min(32);
    // KiCad's own checker allows a gap to come out a few tenths of a micron short.
    b.rules.tolerance = 1e-3;

    let mut edge: Vec<Vec<P>> = Vec::new();
    for c in root.args() {
        match c.head() {
            Some("footprint" | "module") => {
                let fp = parse_footprint(c, &mut |n| nets.of(n), &mut left);
                let id = b.new_id();
                let part = Footprint {
                    id,
                    def: fp.name.clone(),
                    refdes: fp.refdes.clone(),
                    value: fp.value.clone(),
                    at: fp.at,
                    rot: fp.rot,
                    side: if fp.bottom { Side::Bottom } else { Side::Top },
                    pads: fp.pads,
                    graphics: fp.graphics,
                    label_at: fp.label_at,
                    label_height: fp.label_height,
                    locked: fp.locked,
                    keepouts: Vec::new(),
                };
                for shape in &fp.edge {
                    edge.push(shape.iter().map(|q| part.place(*q)).collect());
                }
                if !fp.opposite.is_empty() {
                    left.add("parts with pads on the far side (made two parts of the same name, one on each side)");
                    let id = b.new_id();
                    b.footprints.push(Footprint {
                        id,
                        pads: fp.opposite,
                        graphics: Vec::new(),
                        side: if fp.bottom { Side::Top } else { Side::Bottom },
                        ..part.clone()
                    });
                }
                b.footprints.push(part);
            }
            Some("segment") => {
                let (Some(a), Some(e), Some(layer)) = (child_pt(c, "start"), child_pt(c, "end"), c.child_str("layer").and_then(copper_index)) else { continue };
                let id = b.new_id();
                b.tracks.push(Track { mid: None, id, a: a.rounded(), b: e.rounded(), w: c.child_num("width").unwrap_or(0.25), layer, net: nets.of(c) });
            }
            Some("arc") => {
                let (Some(a), Some(m), Some(e), Some(layer)) = (child_pt(c, "start"), child_pt(c, "mid"), child_pt(c, "end"), c.child_str("layer").and_then(copper_index)) else { continue };
                let id = b.new_id();
                b.tracks.push(Track { mid: Some(m.rounded()), id, a: a.rounded(), b: e.rounded(), w: c.child_num("width").unwrap_or(0.25), layer, net: nets.of(c) });
            }
            Some("via") => {
                let Some(at) = child_pt(c, "at") else { continue };
                let layers: Vec<&str> = c.get("layers").map(|l| l.args().iter().filter_map(|x| x.text()).collect()).unwrap_or_default();
                if layers.len() == 2 && !(layers.contains(&"F.Cu") && layers.contains(&"B.Cu")) {
                    left.add("blind or buried vias made through vias");
                }
                let id = b.new_id();
                b.vias.push(Via { id, at: at.rounded(), dia: c.child_num("size").unwrap_or(0.6), drill: c.child_num("drill").unwrap_or(0.3), net: nets.of(c) });
            }
            Some("gr_line" | "gr_rect" | "gr_circle" | "gr_arc" | "gr_poly" | "gr_curve") => {
                if c.child_str("layer") == Some("Edge.Cuts") {
                    match shape_of(c, &|x, y| pt(x, y), &mut left) {
                        Some(Shape::Line(a, e)) => edge.push(vec![a, e]),
                        Some(Shape::Poly(mut v, closed)) => {
                            if closed {
                                v.push(v[0]);
                            }
                            edge.push(v);
                        }
                        Some(Shape::Circle(cc, r)) => edge.push(circle_ring(cc, r)),
                        None => {}
                    }
                } else {
                    left.add("lines and shapes drawn on the board itself");
                }
            }
            Some("gr_text" | "gr_text_box" | "text") => left.add("texts on the board itself"),
            Some("dimension" | "image" | "target" | "table") => left.add("dimensions, images and tables"),
            Some("zone") => {}
            _ => {}
        }
    }

    // The outline: of the rings the edge makes, the biggest.
    let mut rings = rings_of(edge);
    rings.sort_by(|x, y| ring_area(y).abs().total_cmp(&ring_area(x).abs()));
    if let Some(mut ring) = rings.first().cloned() {
        if ring_area(&ring) < 0.0 {
            ring.reverse();
        }
        b.outline = ring;
    }
    if rings.len() > 1 {
        left.add("cut-outs and slots in the board's edge left out");
    }
    if b.outline.is_empty() {
        left.add("boards with no closed outline on Edge.Cuts");
    }

    for n in nets.order.iter() {
        let id = nets.ids[n];
        b.nets.push(Net { id, name: n.clone() });
    }
    // The nets the rules are in terms of must exist before the zones take them.
    if let Some(json) = project {
        apply_project(&mut b, json);
    }
    zones(&root, &mut b, &mut nets, &mut left);
    // Zones may have met nets that nothing else did.
    for n in nets.order.iter() {
        let id = nets.ids[n];
        if !b.nets.iter().any(|x| x.id == id) {
            b.nets.push(Net { id, name: n.clone() });
        }
    }
    let notes = left.notes();
    Ok(ImportedBoard { board: b, notes })
}

fn zones(root: &Sx, b: &mut Board, nets: &mut Nets, left: &mut Left) {
    let copper = b.copper_layers();
    for z in root.all("zone") {
        // Its layers: one, or several, or all of them.
        let mut names: Vec<String> = Vec::new();
        if let Some(l) = z.child_str("layer") {
            names.push(l.to_string());
        }
        if let Some(l) = z.get("layers") {
            names.extend(l.args().iter().filter_map(|a| a.text().map(String::from)));
        }
        let mut layers: Vec<u8> = Vec::new();
        for name in &names {
            match name.as_str() {
                "*.Cu" => layers.extend(copper.iter().copied()),
                "F&B.Cu" => layers.extend([TOP, BOTTOM]),
                other => layers.extend(copper_index(other)),
            }
        }
        layers.sort_unstable();
        layers.dedup();
        let rings: Vec<Vec<P>> = z
            .all("polygon")
            .map(|poly| {
                let mut ring: Vec<P> = poly.points().into_iter().map(|(x, y)| pt(x, y).rounded()).collect();
                if ring_area(&ring) < 0.0 {
                    ring.reverse();
                }
                ring
            })
            .filter(|r| r.len() >= 3)
            .collect();
        if layers.is_empty() || rings.is_empty() {
            continue;
        }
        if let Some(k) = z.get("keepout") {
            let not = |what: &str| k.child_str(what) == Some("not_allowed");
            let restrict = Restrict { vias: not("vias"), tracks: not("tracks"), pours: not("copperpour"), smd_pads: not("pads"), th_pads: not("pads") };
            if k.child_str("footprints") == Some("not_allowed") {
                left.add("keep-outs against footprints noted as copper keep-outs only");
            }
            let every = layers.len() == copper.len();
            for ring in &rings {
                for &layer in if every { &layers[..1] } else { &layers[..] } {
                    let id = b.new_id();
                    b.keepouts.push(Keepout { id, layer: if every { None } else { Some(layer) }, pts: ring.clone(), restrict });
                }
            }
            continue;
        }
        let net = nets.of(z);
        if net == 0 {
            left.add("zones with no net left out");
            continue;
        }
        if z.get("fill").and_then(|f| f.get("mode")).and_then(|m| m.str_arg(0)) == Some("hatch") {
            left.add("hatched zones poured solid");
        }
        let connect = z.get("connect_pads");
        if connect.and_then(|c| c.str_arg(0)) == Some("no") {
            left.add("zones that don't connect to pads (made to connect with thermal relief)");
        }
        let fill = z.get("fill");
        for ring in &rings {
            for &layer in &layers {
                let id = b.new_id();
                let mut pour = Pour::new(id, layer, net, ring.clone(), &b.rules);
                if let Some(c) = connect.and_then(|c| c.child_num("clearance")).or_else(|| z.child_num("clearance")) {
                    pour.clearance = c;
                }
                if let Some(w) = z.child_num("min_thickness") {
                    pour.neck = w;
                }
                if let Some(f) = fill {
                    if let Some(g) = f.child_num("thermal_gap") {
                        pour.gap = g;
                    }
                    if let Some(w) = f.child_num("thermal_bridge_width") {
                        pour.spoke = w;
                    }
                    // Mode 0 removes every island; 1 keeps them all; 2 removes those below an area.
                    match f.child_num("island_removal_mode") {
                        Some(1.0) => pour.remove_islands = false,
                        Some(2.0) => {
                            pour.remove_islands = false;
                            pour.min_area = f.child_num("island_area_min").unwrap_or(0.0);
                        }
                        _ => {}
                    }
                }
                if connect.and_then(|c| c.str_arg(0)) == Some("yes") {
                    pour.connect = Connect::Direct;
                }
                // Coppersmith marks a power plane by naming its zone so; KiCad has no such thing.
                pour.plane = z.child_str("name") == Some("Power plane");
                // KiCad fills the higher priority first; here the lower order does.
                let priority = z.child_num("priority").unwrap_or(0.0).clamp(0.0, 999.0) as u32;
                pour.order = (1000 - priority) * 1000 + id;
                b.pours.push(pour);
            }
        }
    }
}

/// A KiCad wildcard (`*` any run, `?` any one) match.
fn wildcard(pattern: &str, text: &str) -> bool {
    let (p, t): (Vec<char>, Vec<char>) = (pattern.chars().collect(), text.chars().collect());
    fn go(p: &[char], t: &[char]) -> bool {
        match p.first() {
            None => t.is_empty(),
            Some('*') => (0..=t.len()).any(|k| go(&p[1..], &t[k..])),
            Some('?') => !t.is_empty() && go(&p[1..], &t[1..]),
            Some(c) => t.first() == Some(c) && go(&p[1..], &t[1..]),
        }
    }
    go(&p, &t)
}

/// The rules and net classes in a KiCad project file (`.kicad_pro`, JSON).
fn apply_project(b: &mut Board, json: &str) {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(json) else { return };
    let num = |path: &[&str]| -> Option<f64> {
        let mut at = &v;
        for k in path {
            at = at.get(k)?;
        }
        at.as_f64()
    };
    let r = &mut b.rules;
    let ds = ["board", "design_settings", "rules"];
    let with = |k: &str| num(&[ds[0], ds[1], ds[2], k]);
    if let Some(x) = with("min_track_width").filter(|x| *x > 0.0) {
        r.min_track = x;
    }
    if let Some(x) = with("min_through_hole_diameter").filter(|x| *x > 0.0) {
        r.min_drill = x;
    }
    if let Some(x) = with("min_via_annular_width").filter(|x| *x > 0.0) {
        r.min_ring = x;
    }
    if let Some(x) = with("min_copper_edge_clearance").filter(|x| *x > 0.0) {
        r.edge_clearance = x;
    }
    if let Some(x) = with("min_via_diameter").filter(|x| *x > 0.0) {
        r.via_min_dia = x;
    }
    if let Some(x) = with("min_hole_to_hole").filter(|x| *x > 0.0) {
        r.hole_gap = x;
        r.set_on(crate::pcb::rulebook::RuleId::HoleToHoleClearance, true);
    }
    if let Some(x) = with("min_clearance").filter(|x| *x > 0.0) {
        r.clearance = x;
    }
    if let Some(x) = num(&["board", "design_settings", "defaults", "silk_line_width"]).filter(|x| *x > 0.0) {
        r.silk_width = x;
    }
    // The default net class says what a new track and via start as.
    let classes: Vec<&serde_json::Value> = v.get("net_settings").and_then(|n| n.get("classes")).and_then(|c| c.as_array()).map(|a| a.iter().collect()).unwrap_or_default();
    if let Some(d) = classes.iter().find(|c| c.get("name").and_then(|n| n.as_str()) == Some("Default")) {
        let f = |k: &str| d.get(k).and_then(|x| x.as_f64()).filter(|x| *x > 0.0);
        if let Some(x) = f("clearance") {
            r.clearance = r.clearance.max(x);
        }
        if let Some(x) = f("track_width") {
            r.track = x;
        }
        if let Some(x) = f("via_diameter") {
            r.via_dia = x;
        }
        if let Some(x) = f("via_drill") {
            r.via_drill = x;
        }
    }
    // The other classes, with the nets they hold: named outright, or by pattern.
    let names: Vec<(u32, String)> = b.nets.iter().map(|n| (n.id, n.name.clone())).collect();
    let assigned = |class: &str| -> Vec<u32> {
        let mut out: Vec<u32> = Vec::new();
        if let Some(map) = v.get("net_settings").and_then(|n| n.get("netclass_assignments")).and_then(|a| a.as_object()) {
            for (net, to) in map {
                let hit = match to {
                    serde_json::Value::String(s) => s == class,
                    serde_json::Value::Array(a) => a.iter().any(|x| x.as_str() == Some(class)),
                    _ => false,
                };
                if hit && let Some((id, _)) = names.iter().find(|(_, n)| n == net) {
                    out.push(*id);
                }
            }
        }
        if let Some(pats) = v.get("net_settings").and_then(|n| n.get("netclass_patterns")).and_then(|a| a.as_array()) {
            for p in pats.iter().filter(|p| p.get("netclass").and_then(|c| c.as_str()) == Some(class)) {
                if let Some(pat) = p.get("pattern").and_then(|s| s.as_str()) {
                    out.extend(names.iter().filter(|(_, n)| wildcard(pat, n)).map(|(id, _)| *id));
                }
            }
        }
        out.sort_unstable();
        out.dedup();
        out
    };
    for c in classes.iter().filter(|c| c.get("name").and_then(|n| n.as_str()).is_some_and(|n| n != "Default")) {
        let name = c.get("name").and_then(|n| n.as_str()).unwrap_or("").to_string();
        let members = assigned(&name);
        if members.is_empty() {
            continue;
        }
        if let Some(w) = c.get("track_width").and_then(|x| x.as_f64()).filter(|x| *x > 0.0) {
            b.class_rules.push(ClassRule {
                name: format!("Width_{name}"),
                kind: ClassRuleKind::Width,
                scope: ClassScope::NetClass(name.clone()),
                layers: vec![LayerLimit { layer: u8::MAX, pref_width: Some(w), ..LayerLimit::default() }],
                tolerance: None,
                max_uncoupled: None,
                within_pairs: true,
                between_pairs: false,
            });
        }
        b.net_classes.push(NetClass { name, nets: members });
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn an_arc_is_followed_the_way_it_goes() {
        // A quarter turn about the origin, from (1,0) over (0.707,0.707) to (0,1), counter-clockwise.
        let ccw = arc_points(p(1.0, 0.0), p(0.7071, 0.7071), p(0.0, 1.0));
        assert!(ccw.len() >= 4);
        assert!(ccw.iter().all(|q| (q.dist(P::default()) - 1.0).abs() < 1e-3), "all on the circle");
        assert!(ccw.iter().all(|q| q.x >= -1e-6 && q.y >= -1e-6), "stays in the first quarter");
        // The same ends the other way round, through (0.707, -0.707): the long way.
        let long = arc_points(p(1.0, 0.0), p(0.0, -1.0), p(0.0, 1.0));
        assert!(long.iter().any(|q| q.y < -0.9), "goes round the bottom: {long:?}");
        // Straight is a line.
        assert_eq!(arc_points(p(0.0, 0.0), p(1.0, 0.0), p(2.0, 0.0)).len(), 2);
    }

    #[test]
    fn pieces_of_an_edge_join_into_a_ring_whichever_way_they_are_drawn() {
        let pieces = vec![vec![p(0.0, 0.0), p(10.0, 0.0)], vec![p(10.0, 10.0), p(10.0, 0.0)], vec![p(10.0, 10.0), p(0.0, 10.0)], vec![p(0.0, 10.0), p(0.0, 0.0)]];
        let rings = rings_of(pieces);
        assert_eq!(rings.len(), 1);
        assert_eq!(rings[0].len(), 4);
        assert!((ring_area(&rings[0]).abs() - 100.0).abs() < 1e-9);
        // An open run is no ring.
        assert!(rings_of(vec![vec![p(0.0, 0.0), p(5.0, 0.0)], vec![p(5.0, 0.0), p(5.0, 5.0)]]).is_empty());
    }

    /// A small board in the older style (numbered nets), with a part turned on top, one underneath,
    /// an arc track, a via, an edge drawn in four lines, and a zone.
    pub(crate) const SMALL: &str = r#"(kicad_pcb (version 20221018) (generator "pcbnew")
 (layers (0 "F.Cu" signal) (31 "B.Cu" signal) (44 "Edge.Cuts" user) (37 "F.SilkS" user))
 (net 0 "") (net 1 "A") (net 2 "GND")
 (footprint "Lib:R" (layer "F.Cu") (at 10 20 90)
   (property "Reference" "R1" (at 0 -2 0) (layer "F.SilkS") (effects (font (size 1.2 1.2) (thickness 0.15))))
   (property "Value" "10k" (at 0 2 0) (layer "F.Fab") (effects (font (size 1 1))))
   (pad "1" smd rect (at -1 0 90) (size 1 1.2) (layers "F.Cu" "F.Paste" "F.Mask") (net 1 "A"))
   (pad "2" smd rect (at 1 0 90) (size 1 1.2) (layers "F.Cu" "F.Paste" "F.Mask") (net 2 "GND"))
   (fp_line (start -1 -1) (end 1 -1) (stroke (width 0.12) (type solid)) (layer "F.SilkS")))
 (footprint "Lib:R" (layer "B.Cu") (at 30 20 0)
   (property "Reference" "R2" (at 0 -2 0) (layer "B.SilkS") (effects (font (size 1 1))))
   (pad "1" smd rect (at -1 0 0) (size 1 1.2) (layers "B.Cu" "B.Paste" "B.Mask") (net 1 "A"))
   (pad "2" smd rect (at 1 0 0) (size 1 1.2) (layers "B.Cu" "B.Paste" "B.Mask") (net 2 "GND"))
   (fp_line (start -1 -1) (end 1 -1) (stroke (width 0.12) (type solid)) (layer "B.SilkS")))
 (segment (start 10 19) (end 10 10) (width 0.25) (layer "F.Cu") (net 1))
 (via (at 10 10) (size 0.6) (drill 0.3) (layers "F.Cu" "B.Cu") (net 1))
 (arc (start 10 10) (mid 15 5) (end 20 10) (width 0.25) (layer "B.Cu") (net 1))
 (gr_line (start 0 0) (end 40 0) (layer "Edge.Cuts") (stroke (width 0.1) (type solid)))
 (gr_line (start 40 0) (end 40 40) (layer "Edge.Cuts") (stroke (width 0.1) (type solid)))
 (gr_line (start 40 40) (end 0 40) (layer "Edge.Cuts") (stroke (width 0.1) (type solid)))
 (gr_line (start 0 40) (end 0 0) (layer "Edge.Cuts") (stroke (width 0.1) (type solid)))
 (zone (net 2) (net_name "GND") (layer "B.Cu") (priority 1) (connect_pads (clearance 0.5)) (min_thickness 0.25)
   (fill yes (thermal_gap 0.4) (thermal_bridge_width 0.6))
   (polygon (pts (xy 1 1) (xy 39 1) (xy 39 39) (xy 1 39)))))"#;

    fn small() -> Board {
        read(SMALL, "small", None).unwrap().board
    }

    #[test]
    fn a_board_reads_with_its_parts_where_kicad_put_them() {
        let b = small();
        assert_eq!((b.copper, b.footprints.len(), b.tracks.len(), b.vias.len()), (2, 2, 2, 1));
        let (r1, r2) = (&b.footprints[0], &b.footprints[1]);
        assert_eq!((r1.refdes.as_str(), r1.value.as_str(), r1.def.as_str(), r1.side), ("R1", "10k", "R", Side::Top));
        assert_eq!(r2.side, Side::Bottom);
        assert_eq!(r1.label_height, Some(1.2));
        // KiCad's Y runs down: the part at y = 20 is at -20.
        assert!(r1.at.same(p(10.0, -20.0)) && r1.rot == 90.0);
        // Pad 1 is 1 to the left of a part turned a quarter: in KiCad's picture it ends up 1 further down the page (10, 21).
        assert!(r1.pad_center(0).same(p(10.0, -21.0)), "{:?}", r1.pad_center(0));
        assert!(r1.pad_center(1).same(p(10.0, -19.0)), "{:?}", r1.pad_center(1));
        // A part underneath stores its pads as they look from below; pad 1 is at (29, 20) on the board.
        assert!(r2.pad_center(0).same(p(29.0, -20.0)), "{:?}", r2.pad_center(0));
        assert!(r2.pad_center(1).same(p(31.0, -20.0)), "{:?}", r2.pad_center(1));
        // Its silkscreen is on its own side, which from the board is the bottom.
        assert!(r2.graphics.iter().all(|g| g.layer().on(Side::Bottom) == Layer::Silk(Side::Bottom)));
        assert!(r1.graphics.iter().all(|g| g.layer().on(Side::Top) == Layer::Silk(Side::Top)));
    }

    #[test]
    fn a_pad_turned_with_its_part_keeps_its_own_turn_zero() {
        let b = small();
        // The file says 90° for pads of a part turned 90°: that's the part's turn, not the pad's.
        assert!(b.footprints[0].pads.iter().all(|p| p.rot.abs() < 1e-9));
        // The pad's copper is 1.2 tall on the page after the quarter turn, as KiCad draws it: so wide here.
        let bb = b.footprints[0].pad_blob(0, 0.0).bbox();
        assert!((bb.w() - 1.2).abs() < 1e-6 && (bb.h() - 1.0).abs() < 1e-6, "{:?}", bb);
    }

    #[test]
    fn nets_tracks_arcs_and_vias_come_in_with_their_nets() {
        let b = small();
        let (a, gnd) = (b.net_by_name("A").unwrap(), b.net_by_name("GND").unwrap());
        assert_eq!(b.nets.len(), 2, "net 0 is no net");
        assert_eq!((b.footprints[0].pads[0].net, b.footprints[0].pads[1].net), (a, gnd));
        let seg = &b.tracks[0];
        assert!(seg.a.same(p(10.0, -19.0)) && seg.b.same(p(10.0, -10.0)) && seg.net == a && seg.layer == TOP && seg.mid.is_none());
        let arc = &b.tracks[1];
        assert!(arc.mid.is_some_and(|m| m.same(p(15.0, -5.0))) && arc.layer == BOTTOM);
        assert!(b.vias[0].at.same(p(10.0, -10.0)) && b.vias[0].net == a);
    }

    #[test]
    fn the_edge_drawn_in_pieces_is_one_ring_and_the_zone_is_a_pour() {
        let b = small();
        assert_eq!(b.outline.len(), 4);
        assert!((ring_area(&b.outline) - 1600.0).abs() < 1e-6, "counter-clockwise, 40 by 40");
        assert_eq!(b.pours.len(), 1);
        let z = &b.pours[0];
        assert_eq!((z.layer, z.net), (BOTTOM, b.net_by_name("GND").unwrap()));
        assert_eq!((z.clearance, z.neck, z.gap, z.spoke), (0.5, 0.25, 0.4, 0.6));
        assert!(ring_area(&z.outline) > 0.0 && z.outline.len() == 4);
    }

    #[test]
    fn a_part_with_a_pad_on_the_far_side_becomes_two_parts() {
        // An edge connector: pad 1 on the top, pad B1 underneath, at the same place.
        let text = r#"(kicad_pcb (version 20240108) (layers (0 "F.Cu" signal) (2 "B.Cu" signal))
 (footprint "Conn:Edge" (layer "F.Cu") (at 5 5 0)
  (pad "A1" smd rect (at 0 0) (size 1 2) (layers "F.Cu" "F.Mask") (net "X"))
  (pad "B1" smd rect (at 0 0) (size 1 2) (layers "B.Cu" "B.Mask") (net "Y"))))"#;
        let got = read(text, "edge", None).unwrap();
        let b = &got.board;
        assert_eq!(b.footprints.len(), 2);
        assert_eq!((b.footprints[0].pads.len(), b.footprints[0].side), (1, Side::Bottom));
        assert_eq!((b.footprints[1].pads.len(), b.footprints[1].side), (1, Side::Top));
        assert!(b.footprints[0].pad_center(0).same(b.footprints[1].pad_center(0)), "the same spot, one each side");
        assert!(got.notes.iter().any(|n| n.contains("far side")), "{:?}", got.notes);
        // New-style nets, named in place.
        assert!(b.net_by_name("X").is_some() && b.net_by_name("Y").is_some());
    }

    #[test]
    fn the_project_gives_the_rules_and_the_net_classes() {
        let project = r#"{"board":{"design_settings":{"rules":{"min_track_width":0.15,"min_through_hole_diameter":0.3,"min_via_annular_width":0.1,"min_copper_edge_clearance":0.25,"min_clearance":0.1}}},
          "net_settings":{"classes":[{"name":"Default","clearance":0.2,"track_width":0.3,"via_diameter":0.7,"via_drill":0.35},{"name":"pwr","track_width":0.6}],
          "netclass_assignments":{"A":"pwr"},"netclass_patterns":[{"netclass":"pwr","pattern":"G*"}]}}"#;
        let b = read(SMALL, "small", Some(project)).unwrap().board;
        let r = &b.rules;
        assert_eq!((r.min_track, r.min_drill, r.min_ring, r.edge_clearance), (0.15, 0.3, 0.1, 0.25));
        assert_eq!((r.clearance, r.track, r.via_dia, r.via_drill), (0.2, 0.3, 0.7, 0.35), "the default class says what a new track starts as");
        assert_eq!(b.net_classes.len(), 1);
        assert_eq!(b.net_classes[0].name, "pwr");
        assert_eq!(b.net_classes[0].nets.len(), 2, "A by name and GND by pattern");
        assert!(b.class_rules.iter().any(|c| c.name == "Width_pwr" && c.layers[0].pref_width == Some(0.6)));
    }

    #[test]
    fn something_that_is_not_a_board_is_said_so() {
        let Err(e) = read("(kicad_sch (version 1))", "x", None) else { panic!("a schematic isn't a board") };
        assert!(e.contains("isn't a KiCad board"), "{e}");
        assert!(read("garbage", "x", None).is_err());
    }

    #[test]
    fn wildcards_match_as_kicad_does() {
        assert!(wildcard("USB*", "USB_D+") && wildcard("/CLK?", "/CLK1") && wildcard("*", "x"));
        assert!(!wildcard("USB*", "XUSB") && !wildcard("A?", "A") && !wildcard("A?", "ABC"));
    }
}
