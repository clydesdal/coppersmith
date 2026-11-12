//! Reading a PADS ASCII board (see `pads` for the format).

use crate::pcb::altium::pcbdoc::ImportedBoard;
use crate::pcb::geom::*;
use crate::pcb::model::*;
use std::collections::{HashMap, HashSet};

/// The versions of the format that differ from one another (see the notes in `mod.rs`).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Era {
    V5,
    V2005,
    V2007,
    /// PADS 9 and 10 (and, taken to be alike, anything newer).
    V9,
}

impl Era {
    fn of(major: u32) -> Era {
        match major {
            0..=5 => Era::V5,
            6..=2005 if major >= 2000 => Era::V2005,
            2006..=2007 => Era::V2007,
            _ => Era::V9,
        }
    }
    /// A line for the font after each piece of text.
    fn font_line(self) -> bool {
        self != Era::V5
    }
    /// A line style in pieces, and a corner on square and rectangular pads.
    fn corner(self) -> bool {
        self == Era::V9
    }
    /// Pins are named on the terminal lines.
    fn pin_names(self) -> bool {
        self >= Era::V2007
    }
    /// Flags in the header of a line item.
    fn line_flags(self) -> bool {
        self >= Era::V2007
    }
    /// A units field in the part-type header.
    fn type_units(self) -> bool {
        self <= Era::V2005
    }
}

fn toks(l: &str) -> Vec<&str> {
    l.split_whitespace().collect()
}

fn num(t: &str) -> Option<f64> {
    t.parse::<f64>().ok()
}

fn n(toks: &[&str], i: usize) -> f64 {
    toks.get(i).and_then(|t| num(t)).unwrap_or(0.0)
}

/// The name of the section a line begins, if it begins one.
fn section_name(l: &str) -> Option<&str> {
    let t = l.split_whitespace().next()?;
    if t.len() > 2 && t.starts_with('*') && t.ends_with('*') {
        let name = &t[1..t.len() - 1];
        if name.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_') && name != "SIGNAL" && name != "REMARK" {
            return Some(name);
        }
    }
    None
}

/// A cursor over the file's lines. Blank lines and remarks mean nothing.
struct Lines<'a> {
    v: Vec<&'a str>,
    i: usize,
}

impl<'a> Lines<'a> {
    fn skip(&mut self) {
        while self.i < self.v.len() {
            let t = self.v[self.i].trim();
            if t.is_empty() || t.starts_with("*REMARK*") {
                self.i += 1;
            } else {
                break;
            }
        }
    }
    fn peek(&mut self) -> Option<&'a str> {
        self.skip();
        self.v.get(self.i).map(|l| l.trim())
    }
    fn next(&mut self) -> Option<&'a str> {
        let l = self.peek()?;
        self.i += 1;
        Some(l)
    }
    /// The next line as it is, even if blank: the text of a label or a piece of text.
    fn raw(&mut self) -> &'a str {
        let l = self.v.get(self.i).copied().unwrap_or("");
        self.i += 1;
        l.trim()
    }
    fn at_section(&mut self) -> bool {
        self.peek().is_none_or(|l| section_name(l).is_some())
    }
}

/// A line of drawing: its kind, width, layer, and corners (millimetres, arcs already cut into lines).
#[derive(Clone, Debug)]
struct Piece {
    kind: String,
    width: f64,
    level: i32,
    restrict: String,
    pts: Vec<P>,
}

#[derive(Clone, Debug)]
struct Term {
    at: P,
    name: String,
}

#[derive(Clone, Debug, Default)]
struct Row {
    level: i32,
    size: f64,
    shape: String,
    /// Finger orientation, length and offset.
    fin: (f64, f64, f64),
    corner: f64,
    drill: f64,
    plated: bool,
    slot: bool,
}

#[derive(Clone, Debug)]
struct Label {
    name: String,
    visible: bool,
    at: P,
    height: f64,
}

#[derive(Clone, Debug, Default)]
struct Decal {
    pieces: Vec<Piece>,
    terms: Vec<Term>,
    /// Stack 0 is the default; stack k replaces it for the k-th terminal.
    stacks: HashMap<usize, Vec<Row>>,
    labels: Vec<Label>,
}

#[derive(Clone, Debug)]
struct PartType {
    decals: Vec<String>,
    /// The pins the gates give numbers to, and the names for the pins they don't (in terminal order).
    gate_pins: HashSet<String>,
    unused: Vec<String>,
}

#[derive(Clone, Debug)]
struct Part {
    refdes: String,
    ptype: String,
    at: P,
    rot: f64,
    bottom: bool,
    glued: bool,
    alt: usize,
}

#[derive(Clone, Debug)]
struct RPoint {
    at: P,
    layer: i32,
    width: f64,
    via: Option<String>,
    /// For a point that is the centre of an arc: whether it runs clockwise.
    arc: Option<bool>,
}

#[derive(Clone, Debug, Default)]
struct Signal {
    name: String,
    pins: Vec<(String, String)>,
    conns: Vec<Vec<RPoint>>,
}

#[derive(Clone, Debug, Default)]
struct LayerInfo {
    name: String,
    kind: String,
    plane: String,
    nets: Vec<String>,
}

#[derive(Clone, Debug)]
struct ViaDef {
    drill: f64,
    dia: f64,
    spans: bool,
}

#[derive(Clone, Debug)]
struct PourItem {
    net: String,
    level: i32,
    order: i32,
    outline: Vec<P>,
}

#[derive(Default)]
struct Counts {
    decal_copper: usize,
    free_graphics: usize,
    free_text: usize,
    cutouts: usize,
    copper_cutouts: usize,
    unnamed_copper: usize,
    layer_shapes: usize,
    blind_vias: usize,
    slots: usize,
    no_decal: usize,
    empty_pads: usize,
    dimensions: usize,
    route_arcs: usize,
}

struct Reader<'a> {
    l: Lines<'a>,
    era: Era,
    basic: bool,
    /// Millimetres in a unit of the file, when it isn't in BASIC units.
    unit_mm: f64,
    copper: usize,
    layers: HashMap<i32, LayerInfo>,
    decals: HashMap<String, Decal>,
    types: HashMap<String, PartType>,
    parts: Vec<Part>,
    signals: Vec<Signal>,
    vias: HashMap<String, ViaDef>,
    default_via: Option<String>,
    outline: Vec<P>,
    cutout_outlines: usize,
    fills: Vec<(i32, String, Vec<P>)>,
    keepouts: Vec<(i32, String, Vec<P>)>,
    pours: Vec<PourItem>,
    rules: HashMap<String, f64>,
    c: Counts,
    notes: Vec<String>,
}

/// Millimetres in a BASIC unit: a mil is 38100 of them.
const BASIC_MM: f64 = 1.0 / 1_500_000.0;

impl<'a> Reader<'a> {
    fn mm(&self, v: f64) -> f64 {
        if self.basic { v * BASIC_MM } else { v * self.unit_mm }
    }

    fn pt(&self, x: f64, y: f64) -> P {
        p(self.mm(x), self.mm(y))
    }

    /// A decal's own unit letter, for files that aren't in BASIC units.
    fn decal_scale(&self, letter: &str) -> f64 {
        if self.basic {
            return BASIC_MM;
        }
        match letter.to_ascii_uppercase().as_str() {
            "I" | "MIL" | "MILS" => 0.0254,
            "M" | "MM" | "METRIC" => 1.0,
            _ => self.unit_mm,
        }
    }

    // -- Sections ----------------------------------------------------------------

    fn run(&mut self) {
        while let Some(line) = self.l.next() {
            let Some(name) = section_name(line) else { continue };
            match name {
                "PCB" => self.pcb(),
                "TEXT" => self.text_section(),
                "LINES" => self.lines_section(),
                "VIA" => self.via_section(),
                "PARTDECAL" => self.decal_section(),
                "PARTTYPE" => self.type_section(),
                "PART" => self.part_section(),
                "ROUTE" => self.route_section(),
                "POUR" => self.pour_section(),
                "MISC" => self.misc_section(),
                "END" => break,
                _ => self.skip_section(),
            }
        }
    }

    fn skip_section(&mut self) {
        while !self.l.at_section() {
            self.l.next();
        }
    }

    fn pcb(&mut self) {
        while !self.l.at_section() {
            let Some(line) = self.l.next() else { break };
            let t = toks(line);
            match t.first().copied() {
                Some("MAXIMUMLAYER") => self.copper = (n(&t, 1) as usize).max(1),
                Some("UNITS") if !self.basic => {
                    self.unit_mm = match n(&t, 1) as i32 {
                        1 => 1.0,
                        2 => 25.4,
                        _ => 0.0254,
                    }
                }
                Some("VIAPSHVIA") => self.default_via = t.get(1).map(|s| s.to_string()),
                _ => {}
            }
        }
    }

    /// Free text: three lines a piece (two before V2005).
    fn text_section(&mut self) {
        while !self.l.at_section() {
            self.l.next();
            if self.era.font_line() {
                self.l.next();
            }
            self.l.raw();
            self.c.free_text += 1;
        }
    }

    /// A line of text: its attribute line, the font if the version has one, the words.
    fn text_record(&mut self) -> (Vec<&'a str>, String) {
        let attr = self.l.next().map(toks).unwrap_or_default();
        if self.era.font_line() {
            self.l.next();
        }
        let words = self.l.raw().to_string();
        (attr, words)
    }

    // -- Pieces --------------------------------------------------------------------

    /// A piece of drawing: `TYPE CORNERS WIDTH [LINESTYLE] LEVEL [RESTRICTIONS | PINNUM]`, then its corners.
    fn piece(&mut self, scale: f64, origin: P) -> Option<Piece> {
        let t = toks(self.l.next()?);
        if t.len() < 4 {
            return None;
        }
        let corners = n(&t, 1) as usize;
        let width = n(&t, 2) * scale;
        let k = if self.era.corner() { 4 } else { 3 };
        let level = n(&t, k) as i32;
        let extra = t.get(k + 1).copied();
        let kind = t[0].to_string();
        // The last field is a keepout's restrictions; on copper it is the number of the pin it belongs to, which isn't used.
        let restrict = match extra {
            Some(e) if kind.starts_with("COP") && num(e).is_some() => String::new(),
            Some(e) => e.to_string(),
            None => String::new(),
        };
        // Each corner is `x y`, or `x y begin sweep x1 y1 x2 y2` where it starts an arc.
        struct V {
            at: P,
            arc: Option<(f64, f64, P, f64)>,
        }
        let mut vs: Vec<V> = Vec::new();
        for _ in 0..corners {
            let c = toks(self.l.next()?);
            let at = p(c.first().and_then(|s| num(s)).unwrap_or(0.0) * scale, c.get(1).and_then(|s| num(s)).unwrap_or(0.0) * scale).add(origin);
            let arc = (c.len() >= 8).then(|| {
                let (x1, y1, x2, y2) = (n(&c, 4) * scale, n(&c, 5) * scale, n(&c, 6) * scale, n(&c, 7) * scale);
                (n(&c, 2) / 10.0, n(&c, 3) / 10.0, p((x1 + x2) / 2.0, (y1 + y2) / 2.0).add(origin), ((x2 - x1) / 2.0).abs())
            });
            vs.push(V { at, arc });
        }
        let mut pts = Vec::new();
        for (i, v) in vs.iter().enumerate() {
            pts.push(v.at);
            if let (Some((_, sweep, c, r)), Some(next)) = (v.arc, vs.get(i + 1)) {
                let _ = (next, r);
                pts.extend(arc_between(c, v.at, next.at, sweep));
            }
        }
        Some(Piece { kind, width, level, restrict, pts })
    }

    // -- *LINES* ---------------------------------------------------------------------

    fn lines_section(&mut self) {
        while !self.l.at_section() {
            let Some(head) = self.l.next() else { break };
            let t = toks(head);
            if t.len() < 5 {
                continue;
            }
            let (kind, pieces) = (t[1].to_string(), n(&t, 4) as usize);
            let origin = self.pt(n(&t, 2), n(&t, 3));
            let mut i = 5;
            if self.era.line_flags() {
                i += 1;
            }
            let texts = n(&t, i) as usize;
            let signame = t.get(i + 1).map(|s| s.to_string()).unwrap_or_default();
            if self.l.peek().is_some_and(|l| l.starts_with(".REUSE.")) {
                self.l.next();
            }
            let mut ps = Vec::new();
            for _ in 0..pieces {
                match self.piece(if self.basic { BASIC_MM } else { self.unit_mm }, origin) {
                    Some(pc) => ps.push(pc),
                    None => break,
                }
            }
            for _ in 0..texts {
                self.text_record();
            }
            match kind.as_str() {
                "BOARD" => {
                    for (k, pc) in ps.iter().enumerate() {
                        if k == 0 && matches!(pc.kind.as_str(), "CLOSED" | "BRDCLS" | "OPEN") && pc.pts.len() >= 3 {
                            self.outline = pc.pts.clone();
                        } else {
                            self.cutout_outlines += 1;
                            self.c.cutouts += 1;
                        }
                    }
                }
                "COPPER" => {
                    for pc in ps {
                        match pc.kind.as_str() {
                            "COPCLS" if pc.pts.len() >= 3 => self.fills.push((pc.level, signame.clone(), pc.pts)),
                            "COPCIR" if pc.pts.len() == 2 => {
                                let (c, r) = (pc.pts[0].lerp(pc.pts[1], 0.5), pc.pts[0].dist(pc.pts[1]) / 2.0);
                                let ring = (0..circle_steps(r).max(8)).map(|k| {
                                    let a = k as f64 * std::f64::consts::TAU / circle_steps(r).max(8) as f64;
                                    c.add(p(a.cos(), a.sin()).scale(r))
                                });
                                self.fills.push((pc.level, signame.clone(), ring.collect()));
                            }
                            _ => self.c.copper_cutouts += 1,
                        }
                    }
                }
                "KEEPOUT" => {
                    for pc in ps {
                        match pc.kind.as_str() {
                            "KPTCLS" if pc.pts.len() >= 3 => self.keepouts.push((pc.level, pc.restrict, pc.pts)),
                            "KPTCIR" if pc.pts.len() == 2 => {
                                let (c, r) = (pc.pts[0].lerp(pc.pts[1], 0.5), pc.pts[0].dist(pc.pts[1]) / 2.0);
                                let m = circle_steps(r).max(8);
                                let ring = (0..m).map(|k| c.add(p((k as f64 * std::f64::consts::TAU / m as f64).cos(), (k as f64 * std::f64::consts::TAU / m as f64).sin()).scale(r)));
                                self.keepouts.push((pc.level, pc.restrict, ring.collect()));
                            }
                            _ => {}
                        }
                    }
                }
                "LINES" => {
                    if ps.iter().any(|pc| pc.kind.starts_with("ARW") || pc.kind.starts_with("EXT") || pc.kind == "BASPNT") {
                        self.c.dimensions += 1;
                    } else {
                        self.c.free_graphics += ps.len();
                    }
                }
                _ => self.c.copper_cutouts += ps.len(),
            }
        }
    }

    // -- Pads and vias ---------------------------------------------------------------

    /// A row of a pad stack or via: `LEVEL SIZE SHAPE [fields] [DRILL [PLATED]]`.
    fn row(&self, line: &str, scale: f64) -> Row {
        let t = toks(line);
        let mut r = Row { level: n(&t, 0) as i32, size: n(&t, 1) * scale, shape: t.get(2).unwrap_or(&"R").to_string(), plated: true, ..Row::default() };
        let mut i = 3;
        let take_corner = |i: &mut usize, r: &mut Row| {
            if self.era.corner() && matches!(r.shape.as_str(), "S" | "RF" | "RC") {
                r.corner = n(&t, *i) * scale;
                *i += 1;
            }
        };
        match r.shape.as_str() {
            "RF" | "OF" => {
                r.fin = (n(&t, 3), n(&t, 4) * scale, n(&t, 5) * scale);
                i = 6;
                take_corner(&mut i, &mut r);
            }
            "S" | "RC" => take_corner(&mut i, &mut r),
            "RT" | "ST" | "RA" | "SA" | "A" => return r,
            _ => {}
        }
        if let Some(d) = t.get(i).and_then(|s| num(s)) {
            r.drill = d * scale;
            if let Some(&pl) = t.get(i + 1) {
                r.plated = pl != "N";
            }
            r.slot = t.len() > i + 2;
        }
        r
    }

    fn via_section(&mut self) {
        while !self.l.at_section() {
            let Some(head) = self.l.next() else { break };
            let t = toks(head);
            if t.len() < 3 {
                continue;
            }
            let (name, drill, rows) = (t[0].to_string(), self.mm(n(&t, 1)), n(&t, 2) as usize);
            let scale = if self.basic { BASIC_MM } else { self.unit_mm };
            let mut dia = 0.0f64;
            for _ in 0..rows {
                let Some(line) = self.l.next() else { break };
                let r = self.row(line, scale);
                if r.level == -2 || (dia == 0.0 && r.size > 0.0 && matches!(r.shape.as_str(), "R" | "S" | "A")) {
                    dia = dia.max(r.size);
                }
            }
            self.vias.insert(name, ViaDef { drill, dia: dia.max(drill), spans: t.len() > 4 });
        }
    }

    // -- *PARTDECAL* ---------------------------------------------------------------------

    fn decal_section(&mut self) {
        while !self.l.at_section() {
            let Some(head) = self.l.next() else { break };
            let t = toks(head);
            if t.len() < 9 {
                continue;
            }
            let name = t[0].to_string();
            let scale = self.decal_scale(t[1]);
            let counts: Vec<usize> = (4..9).map(|i| n(&t, i) as usize).collect();
            let mut d = Decal::default();
            for _ in 0..counts[0] {
                match self.piece(scale, p(0.0, 0.0)) {
                    Some(pc) => d.pieces.push(pc),
                    None => break,
                }
            }
            for _ in 0..counts[3] {
                self.text_record();
            }
            for _ in 0..counts[4] {
                let attr = self.l.next().map(toks).unwrap_or_default();
                if self.era.font_line() {
                    self.l.next();
                }
                let label = self.l.raw().to_string();
                d.labels.push(Label { name: label, visible: attr.first().is_some_and(|v| *v != "NONE"), at: p(n(&attr, 1) * scale, n(&attr, 2) * scale), height: n(&attr, 5) * scale });
            }
            for k in 0..counts[1] {
                let Some(line) = self.l.next() else { break };
                let t = toks(line);
                let x = t.first().map(|s| s.trim_start_matches('T')).and_then(num).unwrap_or(0.0) * scale;
                let pin = if self.era.pin_names() { t.get(4).map(|s| s.to_string()) } else { None };
                d.terms.push(Term { at: p(x, n(&t, 1) * scale), name: pin.unwrap_or_else(|| (k + 1).to_string()) });
            }
            for _ in 0..counts[2] {
                let Some(line) = self.l.next() else { break };
                let t = toks(line);
                let (k, rows) = (n(&t, 1) as usize, n(&t, 2) as usize);
                let mut rs = Vec::new();
                for _ in 0..rows {
                    if let Some(l) = self.l.next() {
                        rs.push(self.row(l, scale));
                    }
                }
                d.stacks.insert(k, rs);
            }
            self.decals.insert(name, d);
        }
    }

    // -- *PARTTYPE* and *PART* --------------------------------------------------------------

    fn type_section(&mut self) {
        while !self.l.at_section() {
            let Some(head) = self.l.next() else { break };
            let t = toks(head);
            if t.len() < 3 {
                continue;
            }
            // NAME DECALNM TYPE GATES SIGPINS UNUSED FLAGS ECO; before V2007 a UNITS field comes after DECALNM.
            let k = if self.era.type_units() { 1 } else { 0 };
            let (gates, sigpins, unused) = (n(&t, 3 + k) as usize, n(&t, 4 + k) as usize, n(&t, 5 + k) as usize);
            let mut gate_pins = HashSet::new();
            let mut unused_names = Vec::new();
            for _ in 0..gates {
                let Some(g) = self.l.next() else { break };
                let mut want = n(&toks(g), 2) as usize;
                while want > 0 {
                    let Some(l) = self.l.next() else { break };
                    let pins = toks(l);
                    want = want.saturating_sub(pins.len());
                    gate_pins.extend(pins.iter().map(|x| x.split('.').next().unwrap_or("").to_string()));
                }
            }
            for _ in 0..sigpins {
                self.l.next();
            }
            let mut want = unused;
            while want > 0 {
                let Some(l) = self.l.next() else { break };
                let names = toks(l);
                want = want.saturating_sub(names.len());
                unused_names.extend(names.iter().map(|x| x.to_string()));
            }
            self.types.insert(t[0].to_string(), PartType { decals: t[1].split(':').map(|s| s.to_string()).collect(), gate_pins, unused: unused_names });
            // An optional block of attributes.
            if self.l.peek() == Some("{") {
                while let Some(l) = self.l.next() {
                    if l == "}" {
                        break;
                    }
                }
            }
        }
    }

    fn part_section(&mut self) {
        while !self.l.at_section() {
            let Some(head) = self.l.next() else { break };
            let t = toks(head);
            if t.len() < 12 {
                continue;
            }
            let labels = n(&t, 11) as usize;
            self.parts.push(Part {
                refdes: t[0].to_string(),
                ptype: t[1].to_string(),
                at: self.pt(n(&t, 2), n(&t, 3)),
                rot: n(&t, 4),
                glued: t[5] == "G",
                bottom: t[6] == "M",
                alt: n(&t, 7) as usize,
            });
            if self.l.peek().is_some_and(|l| l.starts_with(".REUSE.")) {
                self.l.next();
            }
            for _ in 0..labels {
                self.text_record();
            }
        }
    }

    // -- *ROUTE* ---------------------------------------------------------------------------------

    fn route_section(&mut self) {
        let mut sig: Option<Signal> = None;
        while !self.l.at_section() {
            let Some(line) = self.l.next() else { break };
            let t = toks(line);
            if t.first() == Some(&"*SIGNAL*") {
                if let Some(s) = sig.take() {
                    self.signals.push(s);
                }
                // The name runs up to the flags and colour (a name may have spaces in it).
                let name = t.get(1).map(|s| s.to_string()).unwrap_or_default();
                sig = Some(Signal { name, ..Signal::default() });
                continue;
            }
            let Some(s) = sig.as_mut() else { continue };
            let is_point = t.len() >= 5 && t[..5].iter().all(|x| num(x).is_some());
            if is_point {
                let at = p(n(&t, 0), n(&t, 1));
                let (layer, width) = (n(&t, 2) as i32, n(&t, 3));
                let mut via = None;
                let mut arc = None;
                if let Some(&w) = t.get(5) {
                    match w {
                        "THERMAL" | "TEARDROP" | "POWER" => {}
                        "CW" => arc = Some(true),
                        "CCW" => arc = Some(false),
                        v if num(v).is_none() => via = Some(v.to_string()),
                        _ => {}
                    }
                }
                if let Some(c) = s.conns.last_mut() {
                    c.push(RPoint { at, layer, width, via, arc });
                }
            } else if t.len() >= 2 && t[0].contains('.') && t[1].contains('.') {
                for pin in &t[..2] {
                    if let Some((r, k)) = pin.split_once('.') {
                        let pair = (r.to_string(), k.to_string());
                        if !s.pins.contains(&pair) {
                            s.pins.push(pair);
                        }
                    }
                }
                s.conns.push(Vec::new());
            }
        }
        if let Some(s) = sig {
            self.signals.push(s);
        }
    }

    // -- *POUR* -------------------------------------------------------------------------------------

    fn pour_section(&mut self) {
        while !self.l.at_section() {
            let Some(head) = self.l.next() else { break };
            let t = toks(head);
            if t.len() < 6 {
                continue;
            }
            let (kind, pieces) = (t[1].to_string(), n(&t, 4) as usize);
            let origin = self.pt(n(&t, 2), n(&t, 3));
            let net = t.get(7).map(|s| s.to_string()).unwrap_or_default();
            let order = t.get(10).and_then(|s| s.parse::<i32>().ok()).unwrap_or(1);
            let scale = if self.basic { BASIC_MM } else { self.unit_mm };
            let mut first: Option<(i32, Vec<P>)> = None;
            for _ in 0..pieces {
                let Some(ph) = self.l.next() else { break };
                let pt = toks(ph);
                let (corners, arcs) = (n(&pt, 1) as usize, n(&pt, 2) as usize);
                let level = n(&pt, 4) as i32;
                // Corners are `x y`; an arc is `cx cy begin sweep` between its two ends.
                enum E {
                    V(P),
                    A(P, f64),
                }
                let mut es = Vec::new();
                for _ in 0..corners + arcs {
                    let c = toks(self.l.next().unwrap_or(""));
                    let at = p(n(&c, 0) * scale, n(&c, 1) * scale).add(origin);
                    es.push(if c.len() >= 4 { E::A(at, n(&c, 3) / 10.0) } else { E::V(at) });
                }
                let mut ring = Vec::new();
                for (i, e) in es.iter().enumerate() {
                    match e {
                        E::V(v) => ring.push(*v),
                        E::A(c, sweep) => {
                            if let (Some(E::V(a)), Some(E::V(b))) = (i.checked_sub(1).and_then(|j| es.get(j)), es.get(i + 1)) {
                                ring.extend(arc_between(*c, *a, *b, *sweep));
                            }
                        }
                    }
                }
                if first.is_none() && matches!(pt.first().copied(), Some("POLY") | Some("CIRCLE")) {
                    if pt[0] == "CIRCLE" && ring.len() == 2 {
                        let (c, r) = (ring[0].lerp(ring[1], 0.5), ring[0].dist(ring[1]) / 2.0);
                        let m = circle_steps(r).max(8);
                        ring = (0..m).map(|k| c.add(p((k as f64 * std::f64::consts::TAU / m as f64).cos(), (k as f64 * std::f64::consts::TAU / m as f64).sin()).scale(r))).collect();
                    }
                    first = Some((level, ring));
                }
            }
            if kind == "POUROUT" {
                if let Some((level, outline)) = first {
                    self.pours.push(PourItem { net, level, order, outline });
                }
            }
        }
    }

    // -- *MISC*: the layers and the rules ----------------------------------------------------------------

    fn misc_section(&mut self) {
        let mut depth = 0i32;
        let mut block = String::new();
        let mut layer: Option<i32> = None;
        let mut in_default_rules = false;
        let mut rule_depth = 0;
        while !self.l.at_section() {
            let Some(line) = self.l.next() else { break };
            if line == "{" {
                depth += 1;
                continue;
            }
            if line == "}" {
                depth -= 1;
                if depth <= 0 {
                    block.clear();
                    depth = 0;
                }
                if in_default_rules && depth < rule_depth {
                    in_default_rules = false;
                }
                if block == "LAYER DATA" && depth == 1 {
                    layer = None;
                }
                continue;
            }
            if depth == 0 {
                block = line.to_string();
                continue;
            }
            let t = toks(line);
            match block.as_str() {
                "LAYER DATA" => {
                    if t.first() == Some(&"LAYER") && t.len() == 2 && depth == 1 {
                        layer = t[1].parse().ok();
                        if let Some(k) = layer {
                            self.layers.entry(k).or_default();
                        }
                    } else if let Some(k) = layer {
                        let info = self.layers.entry(k).or_default();
                        let rest = line.split_once(char::is_whitespace).map(|x| x.1.trim().to_string()).unwrap_or_default();
                        match t.first().copied() {
                            Some("LAYER_NAME") => info.name = rest,
                            Some("LAYER_TYPE") => info.kind = rest,
                            Some("PLANE") => info.plane = rest,
                            Some("NET") => info.nets.push(rest),
                            _ => {}
                        }
                    }
                }
                "RULES_SECTION PARENT" => {
                    if line.starts_with("RULE_SET") && !self.rules.contains_key("seen") {
                        self.rules.insert("seen".into(), 1.0);
                        in_default_rules = true;
                        rule_depth = depth + 1;
                    } else if in_default_rules {
                        if let (Some(k), Some(v)) = (t.first(), t.get(1).and_then(|s| num(s))) {
                            if k.chars().all(|c| c.is_ascii_uppercase() || c == '_') {
                                let value = self.mm(v);
                                self.rules.entry(k.to_string()).or_insert(value);
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }

    // -- Layers ---------------------------------------------------------------------------------------------

    /// A PADS copper layer as ours: the top is 0, the bottom 31, the inner ones from 1.
    fn cu(&self, level: i32) -> Option<u8> {
        let n = self.copper as i32;
        if level < 1 || level > n {
            None
        } else if level == 1 {
            Some(TOP)
        } else if level == n {
            Some(BOTTOM)
        } else {
            Some(level as u8 - 1)
        }
    }

    /// What a drawing on a PADS layer is: silkscreen or assembly, on a side; or something this doesn't draw.
    fn doc(&self, level: i32) -> Option<Layer> {
        // Drawings on copper or on "all layers" are documentation: silkscreen on top.
        if level <= self.copper as i32 {
            return Some(Layer::Silk(Side::Top));
        }
        let side = |name: &str| {
            let n = name.to_lowercase();
            if n.contains("bottom") || n.contains("bot") { Side::Bottom } else { Side::Top }
        };
        match self.layers.get(&level) {
            Some(i) => match i.kind.as_str() {
                "SILK_SCREEN" => Some(Layer::Silk(side(&i.name))),
                "ASSEMBLY" => Some(Layer::Fab(side(&i.name))),
                _ => None,
            },
            // No layer table: the numbering most boards use.
            None => match level % 100 {
                26 => Some(Layer::Silk(Side::Top)),
                29 => Some(Layer::Silk(Side::Bottom)),
                27 => Some(Layer::Fab(Side::Top)),
                30 => Some(Layer::Fab(Side::Bottom)),
                _ => None,
            },
        }
    }
}

/// The points along an arc from `a` to `b` round `c`, not counting the ends: it sweeps `sweep` degrees
/// (counter-clockwise if positive) from `a`.
fn arc_between(c: P, a: P, b: P, sweep: f64) -> Vec<P> {
    let r = c.dist(a);
    if r < 1e-9 || sweep.abs() < 1e-6 {
        return Vec::new();
    }
    let _ = b;
    let a0 = (a.y - c.y).atan2(a.x - c.x);
    let steps = ((sweep.abs() / 360.0) * circle_steps(r) as f64).ceil().max(2.0) as usize;
    (1..steps).map(|k| {
        let t = a0 + sweep.to_radians() * k as f64 / steps as f64;
        c.add(p(t.cos(), t.sin()).scale(r))
    }).collect()
}

pub fn read(text: &str, name: &str) -> Result<ImportedBoard, String> {
    let first = text.lines().next().unwrap_or("");
    if !first.starts_with("!PADS-") {
        return Err("isn't a PADS board (its first line doesn't start with !PADS-)".into());
    }
    // `!PADS-POWERPCB-V9.5-BASIC-250L!`
    let inside = first.trim_start_matches('!').split('!').next().unwrap_or("");
    let parts: Vec<&str> = inside.split('-').collect();
    let major = parts.get(2).and_then(|v| v.trim_start_matches('V').split('.').next()).and_then(|v| v.parse::<u32>().ok()).unwrap_or(9);
    let units = parts.get(3).copied().unwrap_or("BASIC").to_ascii_uppercase();
    let (basic, unit_mm) = match units.as_str() {
        "BASIC" => (true, BASIC_MM),
        "METRIC" | "MM" => (false, 1.0),
        "INCH" | "INCHES" => (false, 25.4),
        _ => (false, 0.0254),
    };
    let mut r = Reader {
        l: Lines { v: text.lines().collect(), i: 1 },
        era: Era::of(major),
        basic,
        unit_mm,
        copper: 2,
        layers: HashMap::new(),
        decals: HashMap::new(),
        types: HashMap::new(),
        parts: Vec::new(),
        signals: Vec::new(),
        vias: HashMap::new(),
        default_via: None,
        outline: Vec::new(),
        cutout_outlines: 0,
        fills: Vec::new(),
        keepouts: Vec::new(),
        pours: Vec::new(),
        rules: HashMap::new(),
        c: Counts::default(),
        notes: Vec::new(),
    };
    r.run();
    Ok(build(r, name, first))
}

// -- Making the board ---------------------------------------------------------------------------------------------

fn build(mut r: Reader, name: &str, header: &str) -> ImportedBoard {
    let mut c = std::mem::take(&mut r.c);
    let mut b = Board { name: name.to_string(), footprints: Vec::new(), outline: Vec::new(), ..Board::default() };
    b.copper = (r.copper as u8).clamp(2, 32);
    r.copper = b.copper as usize;
    b.rules.tolerance = 1e-3;

    // Rules: the default set's clearances (the least, as one gap has to serve), widths, and the via.
    let least = ["TRACK_TO_TRACK", "VIA_TO_TRACK", "VIA_TO_VIA", "PAD_TO_TRACK", "PAD_TO_VIA", "PAD_TO_PAD", "SMD_TO_TRACK", "SMD_TO_VIA", "SMD_TO_PAD", "SMD_TO_SMD"]
        .iter()
        .filter_map(|k| r.rules.get(*k).copied())
        .filter(|v| *v > 0.0)
        .fold(f64::MAX, f64::min);
    if least < f64::MAX {
        b.rules.clearance = least;
        use crate::pcb::model::Kind as K;
        let pairs = [
            ("TRACK_TO_TRACK", K::Track, K::Track),
            ("VIA_TO_TRACK", K::Via, K::Track),
            ("VIA_TO_VIA", K::Via, K::Via),
            ("PAD_TO_TRACK", K::ThPad, K::Track),
            ("PAD_TO_VIA", K::ThPad, K::Via),
            ("SMD_TO_TRACK", K::SmdPad, K::Track),
            ("SMD_TO_VIA", K::SmdPad, K::Via),
            ("COPPER_TO_TRACK", K::Pour, K::Track),
            ("COPPER_TO_VIA", K::Pour, K::Via),
            ("COPPER_TO_PAD", K::Pour, K::ThPad),
            ("COPPER_TO_COPPER", K::Pour, K::Pour),
        ];
        for (key, a, c) in pairs {
            if let Some(v) = r.rules.get(key).copied().filter(|v| *v > least + 1e-9) {
                b.rules.gaps.push(Gap { a, b: c, mm: v });
            }
        }
    }
    if let Some(v) = r.rules.get("REC_TRACK_WIDTH").copied().filter(|v| *v > 0.0) {
        b.rules.track = v;
    }
    if let Some(v) = r.rules.get("MIN_TRACK_WIDTH").copied().filter(|v| *v > 0.0) {
        b.rules.min_track = v;
    }
    if let Some(v) = ["OUTLINE_TO_TRACK", "OUTLINE_TO_COPPER"].iter().filter_map(|k| r.rules.get(*k).copied()).filter(|v| *v > 0.0).reduce(f64::min) {
        b.rules.edge_clearance = v;
    }
    let default_via = r.default_via.clone().filter(|n| r.vias.contains_key(n)).or_else(|| r.vias.keys().filter(|k| !k.starts_with("JMP")).min().cloned());
    if let Some(v) = default_via.as_ref().and_then(|n| r.vias.get(n)) {
        b.rules.via_dia = v.dia;
        b.rules.via_drill = v.drill;
    }

    // Nets: every pin a route names.
    let mut pin_net: HashMap<(String, String), u32> = HashMap::new();
    for s in &r.signals {
        if s.pins.is_empty() {
            continue;
        }
        let id = b.net(&s.name);
        for pin in &s.pins {
            pin_net.insert(pin.clone(), id);
        }
    }

    // Parts.
    let mut missing_decals: HashSet<String> = HashSet::new();
    for part in &r.parts {
        let Some((decal_name, decal)) = decal_of(&r, part) else {
            c.no_decal += 1;
            missing_decals.insert(part.ptype.clone());
            continue;
        };
        let (mut pads, graphics, keepouts, copper_bits, empty, slots, layer_shapes) = footprint_of(&r, decal, r.types.get(&part.ptype));
        c.layer_shapes += layer_shapes;
        c.decal_copper += copper_bits;
        c.empty_pads += empty;
        c.slots += slots;
        for pad in &mut pads {
            pad.net = pin_net.get(&(part.refdes.clone(), pad.num.clone())).copied().unwrap_or(0);
        }
        let label = decal.labels.iter().find(|l| l.name == "Ref.Des." && l.visible);
        let (label_at, label_height) = match label {
            Some(l) => (l.at, Some(l.height).filter(|h| *h > 0.0)),
            None => (crate::pcb::altium::pcblib::label_spot(&pads), None),
        };
        // The part type is where its value is; one named for the decal and a value (as this writes them) gives the value.
        let value = part.ptype.strip_prefix(&format!("{decal_name}_")).unwrap_or(&part.ptype).to_string();
        let id = b.new_id();
        b.footprints.push(Footprint {
            id,
            def: decal_name.clone(),
            refdes: part.refdes.clone(),
            value: if value == decal_name { String::new() } else { value },
            at: part.at,
            // A part underneath is turned the other way: the board is seen through it.
            rot: nm(if part.bottom { -part.rot } else { part.rot }.rem_euclid(360.0)),
            side: if part.bottom { Side::Bottom } else { Side::Top },
            label_at,
            label_height,
            pads,
            graphics,
            locked: part.glued,
            keepouts,
        });
    }
    // Tracks and vias, each once however many connections run along it.
    let key = |q: P| ((q.x * 1e6).round() as i64, (q.y * 1e6).round() as i64);
    let mut seen_seg: HashSet<(u32, u8, (i64, i64), (i64, i64))> = HashSet::new();
    let mut seen_via: HashSet<(u32, (i64, i64))> = HashSet::new();
    let mut implicit: Vec<(u32, P)> = Vec::new();
    for s in &r.signals {
        let net = b.net_by_name(&s.name).unwrap_or(0);
        for conn in &s.conns {
            let pts: Vec<(P, &RPoint)> = conn.iter().map(|q| (r.pt(q.at.x, q.at.y), q)).collect();
            let mut last_copper: Option<(P, u8)> = None;
            let mut i = 0;
            while i < pts.len() {
                let (a, q) = pts[i];
                if let Some(name) = &q.via {
                    if seen_via.insert((net, key(a))) {
                        let (dia, drill) = match r.vias.get(name) {
                            Some(v) => (v.dia, v.drill),
                            None => (b.rules.via_dia, b.rules.via_drill),
                        };
                        if r.vias.get(name).is_some_and(|v| v.spans) {
                            c.blind_vias += 1;
                        }
                        let id = b.new_id();
                        b.vias.push(Via { id, at: a.rounded(), dia, drill, net });
                    }
                }
                let Some(layer) = r.cu(q.layer) else {
                    i += 1;
                    continue;
                };
                let Some(&(next, nq)) = pts.get(i + 1) else { break };
                // A point that is the centre of an arc: the arc runs from here, round it, to the point after.
                if let (Some(cw), Some(&(end, _))) = (nq.arc, pts.get(i + 2)) {
                    c.route_arcs += 1;
                    let (a0, a1) = ((a.y - next.y).atan2(a.x - next.x), (end.y - next.y).atan2(end.x - next.x));
                    let tau = std::f64::consts::TAU;
                    let sweep = if cw { -((a0 - a1).rem_euclid(tau)) } else { (a1 - a0).rem_euclid(tau) };
                    let rad = next.dist(a);
                    let mid = next.add(p((a0 + sweep / 2.0).cos(), (a0 + sweep / 2.0).sin()).scale(rad));
                    let id = b.new_id();
                    b.tracks.push(Track { mid: Some(mid.rounded()), id, a: a.rounded(), b: end.rounded(), w: r.mm(q.width), layer, net });
                    last_copper = Some((end, layer));
                    i += 2;
                    continue;
                }
                if a.dist(next) > 1e-9 {
                    let (ka, kb) = (key(a), key(next));
                    let seg = (net, layer, ka.min(kb), ka.max(kb));
                    if seen_seg.insert(seg) {
                        let id = b.new_id();
                        b.tracks.push(Track { mid: None, id, a: a.rounded(), b: next.rounded(), w: r.mm(q.width), layer, net });
                    }
                }
                // Changing layers where the last copper stopped, without a via named: the via is implied (EasyEDA does this).
                if let Some((end, l)) = last_copper {
                    if l != layer && end.dist(a) < 1e-4 {
                        implicit.push((net, a));
                    }
                }
                last_copper = Some((next, layer));
                i += 1;
            }
        }
    }
    for (net, at) in implicit {
        if seen_via.insert((net, key(at))) {
            let id = b.new_id();
            b.vias.push(Via { id, at: at.rounded(), dia: b.rules.via_dia, drill: b.rules.via_drill, net });
        }
    }

    // The board's edge: its outline, or a box round everything if the file draws none.
    b.outline = r.outline.clone();
    while b.outline.len() > 1 && b.outline[0].same(b.outline[b.outline.len() - 1]) {
        b.outline.pop();
    }
    if ring_area(&b.outline) < 0.0 {
        b.outline.reverse();
    }
    if b.outline.len() < 3 {
        let mut all: Vec<P> = b.footprints.iter().map(|f| f.at).chain(b.tracks.iter().flat_map(|t| [t.a, t.b])).collect();
        if all.is_empty() {
            all = vec![p(0.0, 0.0), p(50.0, 40.0)];
        }
        let bb = BBox::of(&all);
        b.outline = rect_outline(bb.min.sub(p(2.0, 2.0)), bb.max.add(p(2.0, 2.0)));
        r.notes.push("The file draws no board outline: a box was put round the parts".into());
    }

    // Free copper and keepouts.
    for (level, net, pts) in &r.fills {
        let Some(l) = r.cu(*level) else { continue };
        // Copper with no net could be anything drawn on a copper layer, and it would short whatever it overlaps.
        if net.is_empty() || net == "0" || net == "-1" {
            c.unnamed_copper += 1;
            continue;
        }
        let net = b.net_by_name(net).unwrap_or(0);
        let mut pts = pts.clone();
        pts.dedup_by(|a, c| a.same(*c));
        while pts.len() > 1 && pts[0].same(pts[pts.len() - 1]) {
            pts.pop();
        }
        if pts.len() >= 3 {
            if ring_area(&pts) < 0.0 {
                pts.reverse();
            }
            let id = b.new_id();
            b.fills.push(Fill { id, layer: l, net, pts });
        }
    }
    for (level, restrict, pts) in &r.keepouts {
        let mut pts = pts.clone();
        while pts.len() > 1 && pts[0].same(pts[pts.len() - 1]) {
            pts.pop();
        }
        if pts.len() < 3 {
            continue;
        }
        if ring_area(&pts) < 0.0 {
            pts.reverse();
        }
        let has = |c: char| restrict.contains(c);
        let all = restrict.is_empty();
        let rs = Restrict { vias: all || has('V'), tracks: all || has('R'), pours: all || has('C'), smd_pads: false, th_pads: false };
        if !rs.any() {
            continue;
        }
        let id = b.new_id();
        b.keepouts.push(Keepout { id, layer: r.cu(*level), pts, restrict: rs });
    }

    // Pours, poured again here. A plane layer (a "CAM" plane) with a net is one plane over the board.
    let mut poured_layers: HashSet<u8> = HashSet::new();
    let mut pours = r.pours.clone();
    pours.sort_by_key(|p| p.order);
    for (k, pour) in pours.iter().enumerate() {
        let Some(l) = r.cu(pour.level) else { continue };
        let net = if matches!(pour.net.as_str(), "" | "0" | "-1") { 0 } else { b.net_by_name(&pour.net).unwrap_or_else(|| b.net(&pour.net)) };
        let mut outline = pour.outline.clone();
        while outline.len() > 1 && outline[0].same(outline[outline.len() - 1]) {
            outline.pop();
        }
        if outline.len() < 3 {
            continue;
        }
        if ring_area(&outline) < 0.0 {
            outline.reverse();
        }
        let id = b.new_id();
        let mut pour = Pour::new(id, l, net, outline, &b.rules);
        pour.order = k as u32 + 1;
        poured_layers.insert(l);
        b.pours.push(pour);
    }
    let mut planes: Vec<(&i32, &LayerInfo)> = r.layers.iter().filter(|(k, i)| r.cu(**k).is_some() && !matches!(i.plane.as_str(), "" | "NONE" | "MIXED") && !i.nets.is_empty()).collect();
    planes.sort_by_key(|(k, _)| **k);
    for (k, info) in planes {
        let l = r.cu(*k).unwrap_or(TOP);
        if poured_layers.contains(&l) {
            continue;
        }
        let net = b.net_by_name(&info.nets[0]).unwrap_or_else(|| b.net(&info.nets[0]));
        let id = b.new_id();
        let mut pour = Pour::new(id, l, net, Vec::new(), &b.rules);
        pour.plane = true;
        pour.remove_islands = false;
        b.pours.push(pour);
    }

    // What didn't come across.
    let mut notes = std::mem::take(&mut r.notes);
    let s = |k: usize| if k == 1 { "" } else { "s" };
    if !missing_decals.is_empty() {
        notes.push(format!("{} part{} had no footprint in the file and {} left out", c.no_decal, s(c.no_decal), if c.no_decal == 1 { "was" } else { "were" }));
    }
    if c.decal_copper > 0 {
        notes.push(format!("{} copper shape{} drawn in footprints weren't taken (the pads are)", c.decal_copper, s(c.decal_copper)));
    }
    if c.free_graphics > 0 {
        notes.push(format!("{} board drawing line{} not on a part weren't taken", c.free_graphics, s(c.free_graphics)));
    }
    if c.free_text > 0 {
        notes.push(format!("{} free text{} weren't taken", c.free_text, s(c.free_text)));
    }
    if c.cutouts > 0 {
        notes.push(format!("{} board cutout{} or hole{} weren't taken", c.cutouts, s(c.cutouts), s(c.cutouts)));
    }
    if c.unnamed_copper > 0 {
        notes.push(format!("{} piece{} of copper on no net weren't taken", c.unnamed_copper, s(c.unnamed_copper)));
    }
    if c.layer_shapes > 0 {
        notes.push(format!("{} pad{} shaped differently on the inner layers: each takes its outer shape on every layer", c.layer_shapes, s(c.layer_shapes)));
    }
    if c.copper_cutouts > 0 {
        notes.push(format!("{} copper cutout{} weren't taken", c.copper_cutouts, s(c.copper_cutouts)));
    }
    if c.dimensions > 0 {
        notes.push(format!("{} dimension{} weren't taken", c.dimensions, s(c.dimensions)));
    }
    if c.blind_vias > 0 {
        notes.push(format!("{} via{} blind or buried, taken as through", c.blind_vias, s(c.blind_vias)));
    }
    if c.slots > 0 {
        notes.push(format!("{} slotted hole{} taken as round", c.slots, s(c.slots)));
    }
    if c.empty_pads > 0 {
        notes.push(format!("{} pad{} with no copper and no hole left out", c.empty_pads, s(c.empty_pads)));
    }
    if r.layers.is_empty() {
        notes.push("The file has no layer table: silkscreen is taken from the usual layer numbers".into());
    }
    if header.contains("250L") {
        notes.push("A 250-layer numbering file (EasyEDA): layers are read from its own table".into());
    }
    // A file's rules are about clearances and widths; ring, hole size and part gaps aren't in them. A narrower track
    // than the default rule is a rule for some nets, which aren't read: the least used stands.
    {
        use crate::pcb::rulebook::RuleId;
        for id in [RuleId::MinimumAnnularRing, RuleId::HoleSize, RuleId::ComponentClearance] {
            b.rules.set_on(id, false);
        }
        if let Some(least) = b.tracks.iter().map(|t| t.w).reduce(f64::min) {
            if least < b.rules.min_track - 1e-9 {
                b.rules.min_track = least;
                notes.push("Some tracks are narrower than the default width rule (a rule for particular nets, which isn't read): the narrowest is allowed".into());
            }
        }
    }
    b.repour();
    ImportedBoard { board: b, notes }
}

/// The decal a part uses: its part type's, by the part's alternate number; or, with no part type, a decal of that name.
fn decal_of<'r>(r: &'r Reader, part: &Part) -> Option<(String, &'r Decal)> {
    let name = match r.types.get(&part.ptype) {
        Some(t) => t.decals.get(part.alt).or_else(|| t.decals.first())?.clone(),
        None => part.ptype.split('@').next_back().unwrap_or(&part.ptype).to_string(),
    };
    r.decals.get(&name).map(|d| (name, d))
}

/// A decal as a footprint's own pads, drawing and keepouts, with counts of what couldn't be taken.
fn footprint_of(r: &Reader, d: &Decal, pt: Option<&PartType>) -> (Vec<Pad>, Vec<Graphic>, Vec<Keepout>, usize, usize, usize, usize) {
    let mut pads = Vec::new();
    let (mut empty, mut slots, mut layer_shapes) = (0, 0, 0);
    // A pin the gates don't number is named by the part type, in the order of the terminals.
    let mut names: Vec<String> = d.terms.iter().map(|t| t.name.clone()).collect();
    if let Some(pt) = pt.filter(|pt| !pt.unused.is_empty()) {
        let mut spare = pt.unused.iter();
        for name in names.iter_mut().filter(|nm| !pt.gate_pins.contains(*nm)) {
            if let Some(u) = spare.next() {
                *name = u.clone();
            }
        }
    }
    for (k, t) in d.terms.iter().enumerate() {
        let rows = d.stacks.get(&(k + 1)).or_else(|| d.stacks.get(&0));
        let Some(rows) = rows else { continue };
        let Some(row) = rows.iter().find(|x| x.level == -2).or_else(|| rows.first()) else { continue };
        let s = row.size;
        let (mut size, mut shape, mut rot, mut at) = (p(s, s), PadShape::Circle, 0.0, t.at);
        let round = |size: P, corner: f64| if corner > 0.0 { PadShape::RoundRect((corner / size.x.min(size.y).max(1e-9)).clamp(0.0, 0.5)) } else { PadShape::Rect };
        match row.shape.as_str() {
            "S" => shape = round(size, row.corner),
            "RF" | "OF" => {
                let (ori, len, off) = row.fin;
                size = p(len.max(s), s);
                rot = ori;
                at = t.at.add(p(off, 0.0).rot(ori));
                shape = if row.shape == "OF" { PadShape::Oval } else { round(size, row.corner) };
            }
            _ => {}
        }
        if row.slot {
            slots += 1;
        }
        // One shape serves every layer here; a pad that is square outside and round inside (say) takes the outside's.
        if let Some(inner) = rows.iter().find(|x| x.level == -1) {
            if inner.size > 0.0 && (inner.shape != row.shape || (inner.size - row.size).abs() > 1e-6) {
                layer_shapes += 1;
            }
        }
        let drill = row.drill;
        if drill > 0.0 && (!row.plated || size.x <= 0.0 || size.y <= 0.0) {
            // A hole with no copper round it.
            size = p(drill, drill);
            shape = PadShape::Circle;
            rot = 0.0;
            at = t.at;
        }
        if size.x <= 0.0 || size.y <= 0.0 {
            empty += 1;
            continue;
        }
        pads.push(Pad { num: names[k].clone(), at: at.rounded(), size: p(nm(size.x), nm(size.y)), rot: nm(rot.rem_euclid(360.0)), shape, drill: nm(drill), net: 0 });
    }
    let mut graphics = Vec::new();
    let mut keepouts = Vec::new();
    let mut copper = 0;
    for pc in &d.pieces {
        match pc.kind.as_str() {
            k if k.starts_with("COP") => copper += 1,
            "KPTCLS" | "KPTCIR" => {
                let mut pts = pc.pts.clone();
                if pc.kind == "KPTCIR" && pts.len() == 2 {
                    let (c, rad) = (pts[0].lerp(pts[1], 0.5), pts[0].dist(pts[1]) / 2.0);
                    let m = circle_steps(rad).max(8);
                    pts = (0..m).map(|k| c.add(p((k as f64 * std::f64::consts::TAU / m as f64).cos(), (k as f64 * std::f64::consts::TAU / m as f64).sin()).scale(rad))).collect();
                }
                while pts.len() > 1 && pts[0].same(pts[pts.len() - 1]) {
                    pts.pop();
                }
                if pts.len() >= 3 {
                    if ring_area(&pts) < 0.0 {
                        pts.reverse();
                    }
                    let has = |c: char| pc.restrict.contains(c);
                    let rs = Restrict { vias: has('V'), tracks: has('R'), pours: has('C'), smd_pads: false, th_pads: false };
                    if rs.any() {
                        keepouts.push(Keepout { id: 0, layer: r.cu(pc.level), pts, restrict: rs });
                    }
                }
            }
            "OPEN" | "CLOSED" | "CIRCLE" => {
                let Some(layer) = r.doc(pc.level) else { continue };
                let w = if pc.width > 0.0 { pc.width } else { 0.15 };
                if pc.kind == "CIRCLE" && pc.pts.len() == 2 {
                    graphics.push(Graphic::Circle { c: pc.pts[0].lerp(pc.pts[1], 0.5).rounded(), r: nm(pc.pts[0].dist(pc.pts[1]) / 2.0), w, layer, fill: false });
                } else {
                    for pair in pc.pts.windows(2) {
                        if !pair[0].same(pair[1]) {
                            graphics.push(Graphic::Line { a: pair[0].rounded(), b: pair[1].rounded(), w, layer });
                        }
                    }
                }
            }
            _ => {}
        }
    }
    (pads, graphics, keepouts, copper, empty, slots, layer_shapes)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub const DIR: &str = "/Users/dal/Documents/examplelayouts/pads";

    pub fn sample(name: &str) -> Option<ImportedBoard> {
        let dir = std::env::var_os("COPPERSMITH_PADS_DIR").map(std::path::PathBuf::from).unwrap_or_else(|| DIR.into());
        let path = dir.join(name).join(format!("{name}.asc"));
        if !path.exists() {
            return None;
        }
        Some(crate::pcb::pads::read_board(&path).unwrap_or_else(|e| panic!("{e}")))
    }

    /// COPPERSMITH_PADS_DIR=… cargo test dump_one -- --ignored --nocapture
    #[test]
    #[ignore]
    fn dump_one() {
        let name = std::env::var("PADS_ONE").unwrap_or_else(|_| "TMS1mmX19".into());
        let Some(got) = sample(&name) else { return };
        let b = &got.board;
        println!("outline {:?}", b.outline);
        for f in b.footprints.iter().take(5) {
            println!("{} {} at {:?} rot {} side {:?} pads {}", f.refdes, f.def, f.at, f.rot, f.side, f.pads.len());
        }
        let c = crate::pcb::copper::analyse(b);
        for f in crate::pcb::drc::check(b, &c).iter().filter(|f| ["Short circuit", "Too close", "Off the board", "Track too thin"].contains(&f.title.as_str())).take(12) {
            println!("{}: {:?}", f.title, f);
        }
    }

    /// PADS_ONE=Dexter_MotorCtrl PADS_PART=J4 cargo test dump_part -- --ignored --nocapture
    #[test]
    #[ignore]
    fn dump_part() {
        let name = std::env::var("PADS_ONE").unwrap_or_else(|_| "Dexter_MotorCtrl".into());
        let want = std::env::var("PADS_PART").unwrap_or_else(|_| "J4".into());
        let Some(got) = sample(&name) else { return };
        let b = &got.board;
        let f = b.footprints.iter().find(|f| f.refdes == want).expect("part");
        println!("{} {} at {:?} rot {} side {:?}", f.refdes, f.def, f.at, f.rot, f.side);
        for (i, pd) in f.pads.iter().enumerate() {
            println!("  pad {} {:?} size {:?} rot {} shape {:?} drill {} net {} centre {:?}", pd.num, pd.at, pd.size, pd.rot, pd.shape, pd.drill, b.net_name(pd.net), f.pad_center(i));
        }
        let c = f.at;
        for t in b.tracks.iter().filter(|t| t.a.dist(c) < 12.0 || t.b.dist(c) < 12.0).take(12) {
            println!("  track {:?} -> {:?} w {} layer {} net {}", t.a, t.b, t.w, t.layer, b.net_name(t.net));
        }
    }

    /// COPPERSMITH_PADS_DIR=… cargo test dump_pads -- --ignored --nocapture
    #[test]
    #[ignore]
    fn dump_pads() {
        for name in ["ClaySight_MK1", "MAIS_FC", "LCORE_2", "TMS1mmX19", "Ems4_Rev2", "Dexter_MotorCtrl"] {
            let Some(got) = sample(name) else { continue };
            let b = &got.board;
            let c = crate::pcb::copper::analyse(b);
            let found = crate::pcb::drc::check(b, &c);
            let mut kinds: HashMap<String, usize> = HashMap::new();
            for f in &found {
                *kinds.entry(f.title.clone()).or_default() += 1;
            }
            println!(
                "{name}: {} layers, {} parts, {} pads, {} nets, {} tracks, {} vias, {} pours, {} fills, {} keepouts, outline {} corners; unrouted {}; problems {:?}",
                b.copper,
                b.footprints.len(),
                b.footprints.iter().map(|f| f.pads.len()).sum::<usize>(),
                b.nets.len(),
                b.tracks.len(),
                b.vias.len(),
                b.pours.len(),
                b.fills.len(),
                b.keepouts.len(),
                b.outline.len(),
                c.rats.len(),
                kinds
            );
            for n in &got.notes {
                println!("    note: {n}");
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod spec_tests {
    use super::tests::sample;
    use super::*;
    use crate::pcb::copper::analyse;
    use crate::pcb::drc;

    /// A 2-layer board of 40 × 30 mm with one through-hole resistor, and a net routed top, via, bottom.
    pub(crate) const EXAMPLE: &str = r#"!PADS-POWERPCB-V9.5-BASIC! DESIGN DATABASE ASCII FILE 1.0
*PCB*        GENERAL PARAMETERS OF THE PCB DESIGN
UNITS        0
MAXIMUMLAYER 2
ORIGIN       0 0
*TEXT*
*LINES*
OUTLINE      BOARD    0 0 1 0
CLOSED 5   152400 0   0
0 0
60000000 0
60000000 45000000
0 45000000
0 0
*VIA*
STANDARDVIA      762000 3
-2 1524000 R
-1 1524000 R
0  1524000 R
*PARTDECAL*
RES200 I 0 0 1 2 1 0 2
OPEN 2   254000 0   26
-2286000 0
2286000 0
VALUE 0 2286000 0.000 26 1905000 190500 N CENTER CENTER
Regular <Romansim Stroke Font>
Ref.Des.
VALUE 0 -2286000 0.000 27 1905000 190500 N CENTER CENTER
Regular <Romansim Stroke Font>
Part Type
T-3810000 0 -3810000 0 1
T3810000 0 3810000 0 2
PAD 0 3
-2 2286000 R 1143000 P
-1 2286000 R
0  2286000 R
*PARTTYPE*
RES RES200 RES 1 0 0 0 Y
G 0 2
1.0.U. 2.0.U.
*PART*
R1 RES 30000000 22500000 0.000 U N 0 -1 0 -1 0
*ROUTE*
*SIGNAL* N1 0 -2
R1.1 R1.2
26190000 22500000 1 381000 1792 THERMAL
26190000 30000000 1 381000 1536
30000000 30000000 2 381000 1792 STANDARDVIA THERMAL
33810000 30000000 2 381000 1536
33810000 22500000 65 381000 1792 THERMAL
*MISC*      MISCELLANEOUS PARAMETERS
LAYER DATA
{
LAYER 1
{
LAYER_NAME Top
LAYER_TYPE ROUTING
PLANE NONE
}
LAYER 2
{
LAYER_NAME Bottom
LAYER_TYPE ROUTING
PLANE NONE
}
LAYER 26
{
LAYER_NAME Silkscreen Top
LAYER_TYPE SILK_SCREEN
}
LAYER 27
{
LAYER_NAME Assembly Drawing Top
LAYER_TYPE ASSEMBLY
}
}
*END*     OF ASCII OUTPUT FILE
"#;

    fn mm(v: f64) -> f64 {
        v / 1_500_000.0
    }

    pub(crate) fn example() -> &'static str {
        EXAMPLE
    }

    #[test]
    fn a_small_board_reads_part_track_and_via() {
        let got = read(EXAMPLE, "small").unwrap();
        let b = &got.board;
        assert_eq!(b.copper, 2);
        assert!((ring_area(&b.outline) - 40.0 * 30.0).abs() < 1e-6, "{:?}", b.outline);
        assert_eq!(b.footprints.len(), 1);
        let f = &b.footprints[0];
        assert_eq!((f.refdes.as_str(), f.def.as_str(), f.side, f.rot), ("R1", "RES200", Side::Top, 0.0));
        assert!((f.at.x - mm(30_000_000.0)).abs() < 1e-9 && (f.at.y - mm(22_500_000.0)).abs() < 1e-9);
        assert_eq!(f.pads.len(), 2);
        assert!((f.pad_center(0).x - mm(26_190_000.0)).abs() < 1e-6, "pins are 100 mil either side");
        assert!(f.pads.iter().all(|p| b.net_name(p.net) == "N1" && p.drill > 0.0));
        assert_eq!(f.graphics.len(), 1, "one silkscreen line");
        // Four segments, two on each layer, and the via between them.
        assert_eq!(b.tracks.len(), 4);
        assert_eq!(b.tracks.iter().filter(|t| t.layer == TOP).count(), 2);
        assert_eq!(b.tracks.iter().filter(|t| t.layer == BOTTOM).count(), 2);
        assert_eq!(b.vias.len(), 1);
        assert!((b.vias[0].dia - mm(1_524_000.0)).abs() < 1e-9 && (b.vias[0].drill - mm(762_000.0)).abs() < 1e-9);
        assert!((b.vias[0].at.x - 20.0).abs() < 1e-6 && (b.vias[0].at.y - 20.0).abs() < 1e-6);
        // Connected: the route runs pin to pin.
        let c = analyse(b);
        assert!(c.rats.is_empty(), "{:?}", c.rats);
        assert!(drc::check(b, &c).iter().all(|p| p.title != "Short circuit"));
    }

    #[test]
    fn a_part_underneath_is_turned_then_mirrored() {
        // On the bottom, turned a quarter: the decal turns first and is then mirrored left to right.
        let text = EXAMPLE.replace("R1 RES 30000000 22500000 0.000 U N", "R1 RES 30000000 22500000 90.000 U M");
        let got = read(&text, "under").unwrap();
        let f = &got.board.footprints[0];
        assert_eq!(f.side, Side::Bottom);
        // Pin 1 is at (-100 mil, 0): turned 90° it is (0, -100 mil), and mirroring leaves that.
        let (a, c) = (f.pad_center(0), f.pad_center(1));
        assert!((a.x - 20.0).abs() < 1e-6 && (a.y - (15.0 - 2.54)).abs() < 1e-6, "{a:?}");
        assert!((c.x - 20.0).abs() < 1e-6 && (c.y - (15.0 + 2.54)).abs() < 1e-6, "{c:?}");
        // The same part at 0°: the mirror swaps the pins left to right.
        let text = EXAMPLE.replace("R1 RES 30000000 22500000 0.000 U N", "R1 RES 30000000 22500000 0.000 U M");
        let f = &read(&text, "under").unwrap().board.footprints[0];
        assert!(f.pad_center(0).x > f.pad_center(1).x, "pin 1 is on the right when seen from underneath");
    }

    #[test]
    fn versions_before_9_have_no_font_lines_or_corners() {
        // The oldest dialect: no font line after a label, no pin names on terminals.
        let old = EXAMPLE
            .replace("V9.5", "V5.0")
            .replace("OPEN 2   254000 0   26", "OPEN 2   254000 26")
            .replace("CLOSED 5   152400 0   0", "CLOSED 5   152400 0")
            .replace("OUTLINE      BOARD    0 0 1 0", "OUTLINE      BOARD    0 0 1")
            .replace("Regular <Romansim Stroke Font>\n", "")
            .replace("T-3810000 0 -3810000 0 1", "T-3810000 0 -3810000 0")
            .replace("T3810000 0 3810000 0 2", "T3810000 0 3810000 0")
            .replace("RES RES200 RES 1 0 0 0 Y", "RES RES200 I RES 1 0 0 0 Y");
        let got = read(&old, "old").unwrap();
        let b = &got.board;
        assert_eq!((b.footprints.len(), b.tracks.len(), b.vias.len()), (1, 4, 1));
        assert!(b.footprints[0].pads.iter().all(|p| b.net_name(p.net) == "N1"), "pins are numbered in terminal order");
        assert!(analyse(b).rats.is_empty());
    }

    #[test]
    fn what_isnt_a_pads_board_is_refused() {
        assert!(read("(kicad_pcb (version 20240108))", "x").is_err());
        assert!(read("", "x").is_err());
    }

    #[test]
    fn the_real_boards_read_with_their_connections() {
        // (name, copper layers, parts, nets, unrouted): TMS is a layout with nothing routed yet.
        for (name, layers, parts, nets, unrouted) in [("ClaySight_MK1", 2, 36, 32, 0), ("MAIS_FC", 2, 5, 11, 0), ("LCORE_2", 2, 30, 14, 0), ("TMS1mmX19", 2, 373, 346, 1202), ("Ems4_Rev2", 6, 375, 284, 1), ("Dexter_MotorCtrl", 8, 251, 222, 0)] {
            let Some(got) = sample(name) else { continue };
            let b = &got.board;
            assert_eq!((b.copper as usize, b.footprints.len(), b.nets.len()), (layers, parts, nets), "{name}");
            let c = analyse(b);
            assert_eq!(c.rats.len(), unrouted, "{name}: open connections");
            assert!(b.outline.len() >= 3, "{name}: an outline");
            // Every pad on a net is on one the file names.
            assert!(b.footprints.iter().flat_map(|f| &f.pads).filter(|p| p.net != 0).count() > 0, "{name}");
        }
    }

    #[test]
    fn the_real_boards_have_only_the_findings_their_files_would() {
        // ClaySight and MAIS are clean; Dexter's three are a square pad that is round on the inner layers.
        for name in ["ClaySight_MK1", "MAIS_FC"] {
            let Some(got) = sample(name) else { continue };
            let b = &got.board;
            assert!(drc::check(b, &analyse(b)).is_empty(), "{name}");
        }
        if let Some(got) = sample("Dexter_MotorCtrl") {
            let b = &got.board;
            let shorts = drc::check(b, &analyse(b)).iter().filter(|p| p.title == "Short circuit").count();
            assert_eq!(shorts, 3);
            assert!(got.notes.iter().any(|n| n.contains("shaped differently on the inner layers")));
        }
    }

    #[test]
    fn a_part_whose_pins_are_named_by_its_type_is_connected() {
        // Ems4's test points have a terminal "1" that the nets call "A".
        let Some(got) = sample("Ems4_Rev2") else { return };
        let b = &got.board;
        let tp = b.footprints.iter().find(|f| f.refdes == "TP7").expect("TP7");
        assert_eq!(tp.pads[0].num, "A");
        assert_ne!(tp.pads[0].net, 0);
    }
}
