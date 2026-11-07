//! A schematic sheet as it's edited: parts placed from symbols, wires, net
//! labels and power ports, with what they join worked out from where things
//! lie. Everything is in millimetres, symbol pins on a 2.54 mm (100 mil)
//! grid. Parts carry a copy of their symbol, as boards carry their footprints,
//! so a sheet stands on its own.

use crate::pcb::altium::schdoc::{self, Sheet};
use crate::pcb::altium::schlib::{Drawing, Pin, Symbol};
use crate::pcb::geom::*;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

/// A part on the sheet: a symbol, where it sits and how it's turned.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct Placed {
    pub id: u32,
    pub symbol: Symbol,
    pub designator: String,
    pub value: String,
    /// The footprint it will use on the board.
    pub footprint: String,
    pub at: P,
    /// Quarter turns anticlockwise, 0 to 3.
    pub turns: u8,
    /// Mirrored left to right before it's turned.
    pub mirror: bool,
    /// Which part of a many-part symbol this is (from 1).
    pub part: u32,
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct Wire {
    pub id: u32,
    pub pts: Vec<P>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum LabelKind {
    /// A name on a wire: wires with the same name are one net.
    Net,
    /// A supply symbol (GND, VCC…): a name that joins everywhere.
    Power,
    /// A connection to another sheet.
    Port,
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct Label {
    pub id: u32,
    pub at: P,
    pub text: String,
    pub kind: LabelKind,
    /// Which way a power symbol or port points, in quarter turns.
    #[serde(default)]
    pub turns: u8,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Sel {
    Part(u32),
    Wire(u32),
    Label(u32),
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct Schematic {
    pub name: String,
    pub parts: Vec<Placed>,
    pub wires: Vec<Wire>,
    pub labels: Vec<Label>,
    /// Where wires join other than at their ends: worked out, kept so a file
    /// shows them.
    #[serde(default)]
    pub junctions: Vec<P>,
    next_id: u32,
}

impl Default for Schematic {
    fn default() -> Self {
        Schematic { name: "Sheet1".into(), parts: Vec::new(), wires: Vec::new(), labels: Vec::new(), junctions: Vec::new(), next_id: 1 }
    }
}

/// A pin as it lies on the sheet.
#[derive(Clone, Debug, PartialEq)]
pub struct SheetPin {
    pub number: String,
    pub name: String,
    /// Where it meets the body.
    pub at: P,
    /// Where a wire joins.
    pub tip: P,
    pub angle: f64,
    pub hidden: bool,
}

/// What an electrical check found.
#[derive(Clone, Debug, PartialEq)]
pub struct Problem {
    pub at: Option<P>,
    pub text: String,
}

/// A point of a symbol on the sheet: mirrored, turned, then moved.
pub fn transform(q: P, at: P, turns: u8, mirror: bool) -> P {
    let q = if mirror { p(-q.x, q.y) } else { q };
    let q = match turns & 3 {
        0 => q,
        1 => p(-q.y, q.x),
        2 => p(-q.x, -q.y),
        _ => p(q.y, -q.x),
    };
    q.add(at).rounded()
}

/// A direction in degrees, mirrored and turned the same way.
pub fn transform_angle(angle: f64, turns: u8, mirror: bool) -> f64 {
    let a = if mirror { 180.0 - angle } else { angle };
    (a + 90.0 * (turns & 3) as f64).rem_euclid(360.0)
}

impl Placed {
    pub fn pin_on_sheet(&self, pin: &Pin) -> SheetPin {
        let at = transform(pin.at, self.at, self.turns, self.mirror);
        let angle = transform_angle(pin.angle, self.turns, self.mirror);
        let t = angle.to_radians();
        let tip = at.add(p(t.cos(), t.sin()).scale(pin.length)).rounded();
        SheetPin { number: pin.number.clone(), name: pin.name.clone(), at, tip, angle, hidden: pin.hidden }
    }

    /// The pins of the part shown (a many-part symbol shows one part).
    pub fn pins(&self) -> Vec<SheetPin> {
        self.symbol.pins.iter().filter(|p| p.part == 0 || p.part == self.part).map(|p| self.pin_on_sheet(p)).collect()
    }

    /// The drawings of the part shown.
    pub fn drawings(&self) -> impl Iterator<Item = &Drawing> {
        let part = self.part;
        self.symbol.drawings.iter().filter(move |(k, _)| *k == 0 || *k == part).map(|(_, d)| d)
    }

    pub fn bbox(&self) -> BBox {
        let mut b = BBox::default();
        for pin in self.pins() {
            b.add(pin.at);
            b.add(pin.tip);
        }
        for d in self.drawings() {
            for q in drawing_points(d) {
                b.add(transform(q, self.at, self.turns, self.mirror));
            }
        }
        if b.is_empty() {
            b.add(self.at);
        }
        b
    }
}

/// The points that bound a drawing, in the symbol's own coordinates.
pub fn drawing_points(d: &Drawing) -> Vec<P> {
    match d {
        Drawing::Line(a, b) | Drawing::Rect(a, b) => vec![*a, *b],
        Drawing::Polyline(v) | Drawing::Polygon(v) => v.clone(),
        Drawing::Arc { c, r, .. } => vec![c.add(p(*r, *r)), c.add(p(-r, -r))],
        Drawing::Ellipse { c, rx, ry } => vec![c.add(p(*rx, *ry)), c.add(p(-rx, -ry))],
        Drawing::Text { at, .. } => vec![*at],
    }
}

/// A wire from `a` to `b` in right angles: along x first or along y first.
pub fn elbow(a: P, b: P, x_first: bool) -> Vec<P> {
    if a.x == b.x || a.y == b.y {
        return vec![a, b];
    }
    let corner = if x_first { p(b.x, a.y) } else { p(a.x, b.y) };
    vec![a, corner, b]
}

impl Schematic {
    pub fn new_id(&mut self) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// The next free designator for a letter, like R4 when R1 to R3 are used.
    pub fn next_designator(&self, prefix: &str) -> String {
        let used: HashSet<u32> = self.parts.iter().filter_map(|p| p.designator.strip_prefix(prefix).and_then(|n| n.parse().ok())).collect();
        let n = (1..).find(|n| !used.contains(n)).unwrap();
        format!("{prefix}{n}")
    }

    /// Puts a part down; its pins are brought onto the grid with it.
    pub fn place(&mut self, symbol: &Symbol, at: P) -> u32 {
        let id = self.new_id();
        let designator = self.next_designator(&symbol.prefix);
        self.parts.push(Placed {
            id,
            symbol: symbol.clone(),
            designator,
            value: String::new(),
            footprint: symbol.footprints.first().cloned().unwrap_or_default(),
            at: at.rounded(),
            turns: 0,
            mirror: false,
            part: 1,
        });
        id
    }

    pub fn part(&self, id: u32) -> Option<&Placed> {
        self.parts.iter().find(|p| p.id == id)
    }

    pub fn part_mut(&mut self, id: u32) -> Option<&mut Placed> {
        self.parts.iter_mut().find(|p| p.id == id)
    }

    pub fn add_wire(&mut self, pts: Vec<P>) -> Option<u32> {
        let mut pts = pts;
        pts.dedup_by(|a, b| a.same(*b));
        if pts.len() < 2 {
            return None;
        }
        let id = self.new_id();
        self.wires.push(Wire { id, pts });
        self.fix_junctions();
        Some(id)
    }

    /// Adds a label; one put on a pin points away from it, a power symbol
    /// so it doesn't sit on the part's body.
    pub fn add_label(&mut self, at: P, text: &str, kind: LabelKind) -> u32 {
        let id = self.new_id();
        let at = at.rounded();
        let turns = self.pin_direction_at(at).map(|a| (((a - 90.0) / 90.0).round() as i32).rem_euclid(4) as u8).unwrap_or(0);
        self.labels.push(Label { id, at, text: text.to_string(), kind, turns });
        id
    }

    /// The way a pin whose end is at `q` points out from its part, in degrees.
    pub fn pin_direction_at(&self, q: P) -> Option<f64> {
        self.parts.iter().flat_map(|p| p.pins()).find(|s| s.tip.same(q)).map(|s| s.angle)
    }

    pub fn delete(&mut self, sel: Sel) {
        match sel {
            Sel::Part(id) => self.parts.retain(|p| p.id != id),
            Sel::Wire(id) => self.wires.retain(|w| w.id != id),
            Sel::Label(id) => self.labels.retain(|l| l.id != id),
        }
        self.fix_junctions();
    }

    /// Moves a thing; a part's wires don't follow (they stay where they were).
    pub fn translate(&mut self, sel: Sel, d: P) {
        match sel {
            Sel::Part(id) => {
                if let Some(p) = self.part_mut(id) {
                    p.at = p.at.add(d).rounded();
                }
            }
            Sel::Wire(id) => {
                if let Some(w) = self.wires.iter_mut().find(|w| w.id == id) {
                    w.pts.iter_mut().for_each(|q| *q = q.add(d).rounded());
                }
            }
            Sel::Label(id) => {
                if let Some(l) = self.labels.iter_mut().find(|l| l.id == id) {
                    l.at = l.at.add(d).rounded();
                }
            }
        }
        self.fix_junctions();
    }

    /// Moves a part and drags the wires joined to its pins with it, keeping
    /// them square by adding a corner where a segment would go crooked.
    pub fn move_part_with_wires(&mut self, id: u32, d: P) {
        let Some(part) = self.part(id) else { return };
        let tips: Vec<P> = part.pins().into_iter().map(|s| s.tip).collect();
        self.translate_only(Sel::Part(id), d);
        for w in &mut self.wires {
            // Either end of a wire may be on a pin of this part.
            for at_start in [true, false] {
                let pts = &mut w.pts;
                let (end, next) = if at_start { (pts[0], pts[1]) } else { (pts[pts.len() - 1], pts[pts.len() - 2]) };
                if !tips.iter().any(|t| t.same(end)) {
                    continue;
                }
                let moved = end.add(d).rounded();
                let was_horizontal = (end.y - next.y).abs() < 1e-9;
                let was_vertical = (end.x - next.x).abs() < 1e-9;
                let corner = if was_horizontal && (moved.y - next.y).abs() > 1e-9 {
                    Some(p(next.x, moved.y))
                } else if was_vertical && (moved.x - next.x).abs() > 1e-9 {
                    Some(p(moved.x, next.y))
                } else {
                    None
                };
                // From the pin inwards: the pin's new place, then the corner.
                let mut inwards = vec![moved];
                inwards.extend(corner);
                if at_start {
                    pts.splice(0..1, inwards);
                } else {
                    inwards.reverse();
                    let n = pts.len();
                    pts.splice(n - 1..n, inwards);
                }
                pts.dedup_by(|a, b| a.same(*b));
            }
        }
        // Wires that have shrunk to nothing go.
        self.wires.retain(|w| w.pts.len() >= 2);
        self.fix_junctions();
    }

    fn translate_only(&mut self, sel: Sel, d: P) {
        if let Sel::Part(id) = sel
            && let Some(p) = self.part_mut(id)
        {
            p.at = p.at.add(d).rounded();
        }
    }

    /// Turns a part or a label a quarter anticlockwise about where it is.
    pub fn rotate(&mut self, sel: Sel) {
        match sel {
            Sel::Part(id) => {
                if let Some(p) = self.part_mut(id) {
                    p.turns = (p.turns + 1) & 3;
                }
            }
            Sel::Label(id) => {
                if let Some(l) = self.labels.iter_mut().find(|l| l.id == id) {
                    l.turns = (l.turns + 1) & 3;
                }
            }
            Sel::Wire(_) => {}
        }
        self.fix_junctions();
    }

    pub fn mirror(&mut self, id: u32) {
        if let Some(p) = self.part_mut(id) {
            p.mirror = !p.mirror;
        }
        self.fix_junctions();
    }

    pub fn bbox(&self) -> BBox {
        let mut b = BBox::default();
        for p in &self.parts {
            b.union(&p.bbox());
        }
        for w in &self.wires {
            w.pts.iter().for_each(|q| b.add(*q));
        }
        for l in &self.labels {
            b.add(l.at);
        }
        b
    }

    /// The thing at a point, to within `tol`: labels, then parts, then wires.
    pub fn pick(&self, q: P, tol: f64) -> Option<Sel> {
        if let Some(l) = self.labels.iter().rev().find(|l| l.at.dist(q) <= tol * 3.0 + 0.8) {
            return Some(Sel::Label(l.id));
        }
        for part in self.parts.iter().rev() {
            if part.bbox().grown(tol).contains(q) {
                return Some(Sel::Part(part.id));
            }
        }
        self.wires.iter().rev().find(|w| w.pts.windows(2).any(|s| dist_to_segment(q, s[0], s[1]).0 <= tol)).map(|w| Sel::Wire(w.id))
    }

    // -- What joins ----------------------------------------------------------------

    /// Every point a wire can join at: pin tips, wire ends, labels.
    fn joint_points(&self) -> Vec<P> {
        let mut v: Vec<P> = self.parts.iter().flat_map(|p| p.pins().into_iter().map(|q| q.tip)).collect();
        v.extend(self.labels.iter().map(|l| l.at));
        v
    }

    /// Dots where a wire meets another partway along it, or three or more
    /// wire ends meet, so it shows they're joined. Kept in step by every edit.
    pub fn fix_junctions(&mut self) {
        let mut dots: Vec<P> = Vec::new();
        let ends: Vec<P> = self.wires.iter().flat_map(|w| [w.pts[0], *w.pts.last().unwrap()]).collect();
        let points = self.joint_points();
        for (wi, w) in self.wires.iter().enumerate() {
            for s in w.pts.windows(2) {
                // Something else touching this segment away from its ends.
                for q in ends.iter().chain(points.iter()) {
                    if !q.same(s[0]) && !q.same(s[1]) && dist_to_segment(*q, s[0], s[1]).0 < 1e-3 {
                        dots.push(*q);
                    }
                }
                // Another wire's corner lying on it.
                for (wj, o) in self.wires.iter().enumerate() {
                    if wi != wj {
                        for q in &o.pts[1..o.pts.len() - 1] {
                            if !q.same(s[0]) && !q.same(s[1]) && dist_to_segment(*q, s[0], s[1]).0 < 1e-3 {
                                dots.push(*q);
                            }
                        }
                    }
                }
            }
        }
        // Three or more wires ending at one point.
        let mut count: HashMap<(i64, i64), usize> = HashMap::new();
        for q in &ends {
            *count.entry(((q.x * 1000.0).round() as i64, (q.y * 1000.0).round() as i64)).or_default() += 1;
        }
        for (k, n) in count {
            if n >= 3 {
                dots.push(p(k.0 as f64 / 1000.0, k.1 as f64 / 1000.0));
            }
        }
        dots.sort_by(|a, b| a.x.total_cmp(&b.x).then(a.y.total_cmp(&b.y)));
        dots.dedup_by(|a, b| a.same(*b));
        self.junctions = dots;
    }

    /// The sheet as the netlist reader takes it.
    pub fn to_sheet(&self) -> Sheet {
        let parts = self
            .parts
            .iter()
            .map(|p| {
                let pins = p
                    .pins()
                    .into_iter()
                    .map(|s| Pin {
                        part: p.part,
                        number: s.number,
                        name: s.name,
                        at: s.at,
                        length: s.tip.dist(s.at),
                        angle: s.angle,
                        kind: crate::pcb::altium::schlib::PinKind::Passive,
                        hidden: s.hidden,
                    })
                    .collect();
                schdoc::Part { designator: p.designator.clone(), lib_ref: p.symbol.name.clone(), value: p.value.clone(), footprint: p.footprint.clone(), at: p.at, part: p.part, pins }
            })
            .collect();
        Sheet {
            name: self.name.clone(),
            parts,
            wires: self.wires.iter().map(|w| w.pts.clone()).collect(),
            junctions: self.junctions.clone(),
            names: self.labels.iter().map(|l| (l.at, l.text.clone())).collect(),
            drawings: Vec::new(),
        }
    }

    /// The nets: a name and the pins on it, as "R1-2".
    pub fn netlist(&self) -> Vec<(String, Vec<String>)> {
        schdoc::netlist(&[self.to_sheet()])
    }

    /// Electrical checks: pins joined to nothing, designators used twice,
    /// parts with no designator or no footprint, labels that touch nothing.
    pub fn check(&self) -> Vec<Problem> {
        let mut out = Vec::new();
        let ends: Vec<P> = self.wires.iter().flat_map(|w| w.pts.iter().copied()).collect();
        let touches_wire = |q: P| self.wires.iter().any(|w| w.pts.windows(2).any(|s| dist_to_segment(q, s[0], s[1]).0 < 1e-3));
        let all_tips: Vec<(usize, P)> = self.parts.iter().enumerate().flat_map(|(i, p)| p.pins().into_iter().map(move |s| (i, s.tip))).collect();
        for part in &self.parts {
            for pin in part.pins() {
                if pin.hidden {
                    continue;
                }
                let joined = touches_wire(pin.tip) || self.labels.iter().any(|l| l.at.same(pin.tip)) || all_tips.iter().filter(|(_, t)| t.same(pin.tip)).count() > 1 || ends.iter().any(|e| e.same(pin.tip));
                if !joined {
                    out.push(Problem { at: Some(pin.tip), text: format!("{} pin {} ({}) isn't connected", part.designator, pin.number, pin.name) });
                }
            }
        }
        let mut seen: HashMap<&str, usize> = HashMap::new();
        for p in &self.parts {
            if p.designator.is_empty() || p.designator.ends_with('?') {
                out.push(Problem { at: Some(p.at), text: format!("{} has no designator", p.symbol.name) });
            } else {
                *seen.entry(p.designator.as_str()).or_default() += 1;
            }
            if p.footprint.is_empty() {
                out.push(Problem { at: Some(p.at), text: format!("{} has no footprint", p.designator) });
            }
        }
        for (d, n) in seen {
            if n > 1 {
                out.push(Problem { at: None, text: format!("{d} is used {n} times") });
            }
        }
        for l in &self.labels {
            let on_something = touches_wire(l.at) || self.joint_points().iter().filter(|q| q.same(l.at)).count() > 1;
            if !on_something {
                out.push(Problem { at: Some(l.at), text: format!("The {} “{}” isn't on a wire or pin", if l.kind == LabelKind::Power { "power port" } else { "label" }, l.text) });
            }
        }
        out.sort_by(|a, b| a.text.cmp(&b.text));
        out
    }

    // -- Files ---------------------------------------------------------------------

    pub fn save(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    pub fn load(text: &str) -> Result<Schematic, String> {
        serde_json::from_str(text).map_err(|e| format!("not a Coppersmith schematic ({e})"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pcb::altium::schlib::PinKind;

    /// A two-pin part along x: pin 1 at (-5.08, 0) pointing left, pin 2 at (5.08, 0) pointing right, 2.54 long.
    fn resistor() -> Symbol {
        let pin = |n: &str, x: f64, a: f64| Pin { part: 1, number: n.into(), name: String::new(), at: p(x, 0.0), length: 2.54, angle: a, kind: PinKind::Passive, hidden: false };
        Symbol {
            name: "Resistor".into(),
            description: String::new(),
            prefix: "R".into(),
            parts: 1,
            pins: vec![pin("1", -2.54, 180.0), pin("2", 2.54, 0.0)],
            drawings: vec![(0, Drawing::Rect(p(-2.54, -1.0), p(2.54, 1.0)))],
            footprints: vec!["R_0603".into()],
            library: String::new(),
        }
    }

    fn tip(s: &Schematic, id: u32, n: &str) -> P {
        s.part(id).unwrap().pins().into_iter().find(|p| p.number == n).unwrap().tip
    }

    #[test]
    fn a_part_is_placed_named_and_its_pins_found_on_the_sheet() {
        let mut s = Schematic::default();
        let a = s.place(&resistor(), p(10.16, 20.32));
        let b = s.place(&resistor(), p(30.48, 20.32));
        assert_eq!((s.part(a).unwrap().designator.as_str(), s.part(b).unwrap().designator.as_str()), ("R1", "R2"));
        assert_eq!(s.part(a).unwrap().footprint, "R_0603");
        assert_eq!(tip(&s, a, "1"), p(10.16 - 5.08, 20.32));
        assert_eq!(tip(&s, a, "2"), p(10.16 + 5.08, 20.32));
        s.delete(Sel::Part(a));
        assert_eq!(s.next_designator("R"), "R1", "a freed number is used again");
    }

    #[test]
    fn turning_and_mirroring_move_the_pins() {
        let mut s = Schematic::default();
        let a = s.place(&resistor(), p(0.0, 0.0));
        s.rotate(Sel::Part(a));
        assert_eq!(tip(&s, a, "2"), p(0.0, 5.08), "pin 2 went from right to up");
        assert_eq!(tip(&s, a, "1"), p(0.0, -5.08));
        s.rotate(Sel::Part(a));
        assert_eq!(tip(&s, a, "2"), p(-5.08, 0.0));
        s.rotate(Sel::Part(a));
        s.rotate(Sel::Part(a));
        s.mirror(a);
        assert_eq!(tip(&s, a, "2"), p(-5.08, 0.0), "mirrored left to right");
        assert_eq!(s.part(a).unwrap().pins()[1].angle, 180.0);
    }

    #[test]
    fn a_wire_between_pins_makes_a_net_and_a_label_names_it() {
        let mut s = Schematic::default();
        let a = s.place(&resistor(), p(0.0, 0.0));
        let b = s.place(&resistor(), p(15.24, 0.0));
        // R1 pin 2 at x=5.08 to R2 pin 1 at x=10.16.
        s.add_wire(vec![tip(&s, a, "2"), tip(&s, b, "1")]).unwrap();
        s.add_label(p(7.62, 0.0), "MID", LabelKind::Net);
        let nets = s.netlist();
        let mid = nets.iter().find(|(n, _)| n == "MID").expect("MID net");
        assert_eq!(mid.1, vec!["R1-2".to_string(), "R2-1".to_string()]);
    }

    #[test]
    fn a_power_port_joins_everywhere_it_is_named() {
        let mut s = Schematic::default();
        let a = s.place(&resistor(), p(0.0, 0.0));
        let b = s.place(&resistor(), p(0.0, -20.32));
        // Each part's pin 1 reaches its own GND port, not wired together.
        s.add_label(tip(&s, a, "1"), "GND", LabelKind::Power);
        s.add_label(tip(&s, b, "1"), "GND", LabelKind::Power);
        let gnd = s.netlist().into_iter().find(|(n, _)| n == "GND").expect("GND");
        assert_eq!(gnd.1, vec!["R1-1".to_string(), "R2-1".to_string()]);
    }

    #[test]
    fn a_wire_joining_partway_gets_a_dot_and_a_plain_crossing_does_not() {
        let mut s = Schematic::default();
        s.add_wire(vec![p(0.0, 0.0), p(10.16, 0.0)]).unwrap();
        // Another ending on the middle of it.
        s.add_wire(vec![p(5.08, 0.0), p(5.08, 5.08)]).unwrap();
        assert_eq!(s.junctions, vec![p(5.08, 0.0)]);
        // One crossing right through, with ends away from it: no dot, no join.
        s.add_wire(vec![p(2.54, -5.08), p(2.54, 5.08)]).unwrap();
        assert_eq!(s.junctions.len(), 1);
        // Remove the T and the dot goes with it.
        let t = s.wires[1].id;
        s.delete(Sel::Wire(t));
        assert!(s.junctions.is_empty());
    }

    #[test]
    fn the_check_finds_loose_pins_repeated_designators_and_floating_labels() {
        let mut s = Schematic::default();
        let a = s.place(&resistor(), p(0.0, 0.0));
        let b = s.place(&resistor(), p(15.24, 0.0));
        s.add_wire(vec![tip(&s, a, "2"), tip(&s, b, "1")]).unwrap();
        let texts = |s: &Schematic| s.check().into_iter().map(|p| p.text).collect::<Vec<_>>();
        let t = texts(&s);
        assert!(t.contains(&"R1 pin 1 () isn't connected".to_string()), "{t:?}");
        assert!(t.contains(&"R2 pin 2 () isn't connected".to_string()), "{t:?}");
        assert!(!t.iter().any(|x| x.contains("R1 pin 2")), "wired pins are fine: {t:?}");
        s.part_mut(b).unwrap().designator = "R1".into();
        s.part_mut(a).unwrap().footprint.clear();
        s.add_label(p(50.0, 50.0), "LOST", LabelKind::Net);
        let t = texts(&s);
        assert!(t.contains(&"R1 is used 2 times".to_string()), "{t:?}");
        assert!(t.contains(&"R1 has no footprint".to_string()), "{t:?}");
        assert!(t.iter().any(|x| x.contains("“LOST” isn't on a wire")), "{t:?}");
    }

    #[test]
    fn picking_finds_a_part_a_wire_and_a_label() {
        let mut s = Schematic::default();
        let a = s.place(&resistor(), p(0.0, 0.0));
        let w = s.add_wire(vec![p(10.16, 0.0), p(10.16, 10.16)]).unwrap();
        let l = s.add_label(p(20.32, 0.0), "X", LabelKind::Net);
        assert_eq!(s.pick(p(0.5, 0.2), 0.2), Some(Sel::Part(a)));
        assert_eq!(s.pick(p(10.2, 5.0), 0.2), Some(Sel::Wire(w)));
        assert_eq!(s.pick(p(20.4, 0.2), 0.2), Some(Sel::Label(l)));
        assert_eq!(s.pick(p(30.0, 30.0), 0.2), None);
    }

    #[test]
    fn moving_a_part_drags_its_wires_and_keeps_them_square() {
        let mut s = Schematic::default();
        let a = s.place(&resistor(), p(0.0, 0.0));
        let b = s.place(&resistor(), p(15.24, 0.0));
        // R1 pin 2 (5.08, 0) to R2 pin 1 (10.16, 0): one straight wire.
        s.add_wire(vec![tip(&s, a, "2"), tip(&s, b, "1")]).unwrap();
        // Move R1 up: the wire stays on its pin, with a corner to stay square.
        s.move_part_with_wires(a, p(0.0, 5.08));
        let w = &s.wires[0].pts;
        assert_eq!(w.first(), Some(&p(5.08, 5.08)), "starts at the moved pin");
        assert_eq!(w.last(), Some(&p(10.16, 0.0)), "still ends on R2");
        assert_eq!(w.len(), 3, "{w:?}");
        assert!(w.windows(2).all(|s| s[0].x == s[1].x || s[0].y == s[1].y), "square: {w:?}");
        // The join holds: the same net as before.
        let nets = s.netlist();
        assert!(nets.iter().any(|(_, pins)| pins == &vec!["R1-2".to_string(), "R2-1".to_string()]), "{nets:?}");
        // Moving R2 along the wire's own way keeps it straight (no corner).
        s.move_part_with_wires(b, p(2.54, 0.0));
        assert_eq!(s.wires[0].pts.last(), Some(&p(12.7, 0.0)));
        assert!(s.wires[0].pts.windows(2).all(|s| s[0].x == s[1].x || s[0].y == s[1].y));
    }

    #[test]
    fn a_label_on_a_pin_points_away_from_the_part() {
        let mut s = Schematic::default();
        let a = s.place(&resistor(), p(0.0, 0.0));
        // Pin 2 points right (0°), pin 1 left (180°); a free label points up.
        let right = s.add_label(tip(&s, a, "2"), "A", LabelKind::Power);
        let left = s.add_label(tip(&s, a, "1"), "B", LabelKind::Power);
        let free = s.add_label(p(0.0, 30.0), "C", LabelKind::Power);
        let turns = |id| s.labels.iter().find(|l| l.id == id).unwrap().turns;
        assert_eq!(turns(right), 3, "right is three quarter turns from up");
        assert_eq!(turns(left), 1, "left is one");
        assert_eq!(turns(free), 0);
    }

    #[test]
    fn elbows_are_square() {
        assert_eq!(elbow(p(0.0, 0.0), p(5.0, 0.0), true), vec![p(0.0, 0.0), p(5.0, 0.0)]);
        assert_eq!(elbow(p(0.0, 0.0), p(5.0, 3.0), true), vec![p(0.0, 0.0), p(5.0, 0.0), p(5.0, 3.0)]);
        assert_eq!(elbow(p(0.0, 0.0), p(5.0, 3.0), false), vec![p(0.0, 0.0), p(0.0, 3.0), p(5.0, 3.0)]);
    }

    #[test]
    fn a_sheet_is_saved_and_read_back() {
        let mut s = Schematic::default();
        let a = s.place(&resistor(), p(0.0, 0.0));
        s.add_wire(vec![tip(&s, a, "2"), p(10.16, 0.0), p(10.16, 5.08)]).unwrap();
        s.add_label(p(10.16, 5.08), "OUT", LabelKind::Port);
        let back = Schematic::load(&s.save()).unwrap();
        assert_eq!(back, s);
        assert!(Schematic::load("nonsense").is_err());
    }
}
