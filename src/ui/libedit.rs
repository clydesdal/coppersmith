//! The library editor's working parts: what is open, what is picked, and the
//! edits, with nothing about drawing in it (that's `libview`).
//!
//! A library here is what an Altium `.PcbLib` (footprints) and `.SchLib`
//! (symbols) hold; both can be open together, as an `.IntLib` has both. Edits
//! are made to one footprint or symbol at a time, and each can be undone.

use crate::pcb::altium::schlib::{Drawing, Pin, PinKind, Symbol};
use crate::pcb::altium::{self, write};
use crate::pcb::geom::*;
use crate::pcb::library::FootprintDef;
use crate::pcb::model::*;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Footprints,
    Symbols,
}

/// A thing in the footprint or symbol being edited, by its place in the list.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Item {
    Pad(usize),
    /// A line or circle of a footprint's drawing.
    Gfx(usize),
    Pin(usize),
    /// A line, rectangle or other shape of a symbol's body.
    Draw(usize),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tool {
    Select,
    /// A pad (footprints) or a pin (symbols).
    Pad,
    Line,
    /// A circle (footprints) or a rectangle (symbols).
    Shape,
}

/// What an undo puts back.
#[derive(Clone)]
enum Undo {
    Footprint(usize, FootprintDef),
    Symbol(usize, Symbol),
    /// The lists themselves, after one was added to or taken from.
    Lists(Vec<FootprintDef>, Vec<Symbol>, usize, usize),
}

pub struct LibEditor {
    pub kind: Kind,
    pub name: String,
    pub footprints: Vec<FootprintDef>,
    pub symbols: Vec<Symbol>,
    pub fp_path: Option<PathBuf>,
    pub sym_path: Option<PathBuf>,
    /// Which footprint and which symbol are being edited.
    pub fp: usize,
    pub sym: usize,
    /// The part of a many-part symbol on show, from 1.
    pub part: u32,
    pub sel: Option<Item>,
    pub tool: Tool,
    /// The drawing layer lines and circles are put on.
    pub layer: Layer,
    pub grid: f64,
    /// The first click of a line or shape.
    pub pending: Option<P>,
    /// A pad being put down starts as this; a pin points this way.
    pub pad_proto: Pad,
    pub pin_angle: f64,
    pub dirty: bool,
    pub filter: String,
    pub notes: Vec<String>,
    undo: Vec<Undo>,
    redo: Vec<Undo>,
    /// When the last edit by typing or dragging a number was made, and to what,
    /// so a run of them is one undo.
    last_edit: Option<(std::time::Instant, Kind, usize)>,
}

fn pad_proto() -> Pad {
    Pad { num: String::new(), at: p(0.0, 0.0), size: p(1.5, 1.5), rot: 0.0, shape: PadShape::Rect, drill: 0.0, net: 0 }
}

impl Default for LibEditor {
    fn default() -> Self {
        LibEditor {
            kind: Kind::Footprints,
            name: "My library".into(),
            footprints: Vec::new(),
            symbols: Vec::new(),
            fp_path: None,
            sym_path: None,
            fp: 0,
            sym: 0,
            part: 1,
            sel: None,
            tool: Tool::Select,
            layer: Layer::Silk(Side::Top),
            grid: 0.25,
            pending: None,
            pad_proto: pad_proto(),
            pin_angle: 180.0,
            dirty: false,
            filter: String::new(),
            notes: Vec::new(),
            undo: Vec::new(),
            redo: Vec::new(),
            last_edit: None,
        }
    }
}

/// The grids on offer: for footprints in millimetres, for symbols the
/// schematic's 10, 50 and 100 mil.
pub const FOOTPRINT_GRIDS: [f64; 6] = [0.05, 0.1, 0.25, 0.5, 1.0, 1.27];
pub const SYMBOL_GRIDS: [f64; 4] = [0.254, 1.27, 2.54, 5.08];

impl LibEditor {
    pub fn new(kind: Kind) -> LibEditor {
        let mut e = LibEditor { kind, ..Default::default() };
        e.set_kind(kind);
        e
    }

    pub fn set_kind(&mut self, kind: Kind) {
        self.kind = kind;
        self.sel = None;
        self.pending = None;
        self.tool = Tool::Select;
        self.grid = if kind == Kind::Footprints { 0.25 } else { 2.54 };
    }

    // -- What's being edited ---------------------------------------------------

    pub fn footprint(&self) -> Option<&FootprintDef> {
        self.footprints.get(self.fp)
    }

    pub fn symbol(&self) -> Option<&Symbol> {
        self.symbols.get(self.sym)
    }

    pub fn count(&self) -> usize {
        if self.kind == Kind::Footprints { self.footprints.len() } else { self.symbols.len() }
    }

    pub fn current(&self) -> usize {
        if self.kind == Kind::Footprints { self.fp } else { self.sym }
    }

    pub fn choose(&mut self, i: usize) {
        if i < self.count() {
            if self.kind == Kind::Footprints {
                self.fp = i;
            } else {
                self.sym = i;
                self.part = 1;
            }
            self.sel = None;
            self.pending = None;
        }
    }

    pub fn names(&self) -> Vec<String> {
        match self.kind {
            Kind::Footprints => self.footprints.iter().map(|f| f.name.clone()).collect(),
            Kind::Symbols => self.symbols.iter().map(|s| s.name.clone()).collect(),
        }
    }

    // -- Undo ------------------------------------------------------------------

    fn snapshot(&self) -> Option<Undo> {
        match self.kind {
            Kind::Footprints => self.footprints.get(self.fp).map(|f| Undo::Footprint(self.fp, f.clone())),
            Kind::Symbols => self.symbols.get(self.sym).map(|s| Undo::Symbol(self.sym, s.clone())),
        }
    }

    /// Notes the current footprint or symbol, before it's changed.
    pub fn remember(&mut self) {
        if let Some(u) = self.snapshot() {
            self.undo.push(u);
            self.redo.clear();
        }
        self.last_edit = None;
        self.dirty = true;
    }

    /// As `remember`, but a run of edits to one thing made close together
    /// (dragging a number) counts as one.
    pub fn remember_run(&mut self) {
        let now = std::time::Instant::now();
        let this = (self.kind, self.current());
        if let Some((t, k, i)) = self.last_edit
            && (k, i) == this
            && now.duration_since(t).as_millis() < 700
        {
            self.last_edit = Some((now, k, i));
            self.dirty = true;
            return;
        }
        self.remember();
        self.last_edit = Some((now, this.0, this.1));
    }

    fn remember_lists(&mut self) {
        self.undo.push(Undo::Lists(self.footprints.clone(), self.symbols.clone(), self.fp, self.sym));
        self.redo.clear();
        self.last_edit = None;
        self.dirty = true;
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Puts back what `u` holds and gives what was there.
    fn swap(&mut self, u: Undo) -> Undo {
        self.last_edit = None;
        self.sel = None;
        self.pending = None;
        self.dirty = true;
        match u {
            Undo::Footprint(i, f) => {
                self.kind = Kind::Footprints;
                self.fp = i;
                let was = std::mem::replace(&mut self.footprints[i], f);
                Undo::Footprint(i, was)
            }
            Undo::Symbol(i, s) => {
                self.kind = Kind::Symbols;
                self.sym = i;
                let was = std::mem::replace(&mut self.symbols[i], s);
                Undo::Symbol(i, was)
            }
            Undo::Lists(f, s, fi, si) => {
                let was = Undo::Lists(std::mem::replace(&mut self.footprints, f), std::mem::replace(&mut self.symbols, s), self.fp, self.sym);
                self.fp = fi;
                self.sym = si;
                was
            }
        }
    }

    pub fn undo(&mut self) {
        if let Some(u) = self.undo.pop() {
            let was = self.swap(u);
            self.redo.push(was);
        }
    }

    pub fn redo(&mut self) {
        if let Some(u) = self.redo.pop() {
            let was = self.swap(u);
            self.undo.push(was);
        }
    }

    // -- Footprints and symbols ----------------------------------------------------

    fn unique(&self, base: &str) -> String {
        let names = self.names();
        if !names.iter().any(|n| n == base) {
            return base.to_string();
        }
        (2..).map(|k| format!("{base}_{k}")).find(|n| !names.contains(n)).unwrap()
    }

    pub fn new_item(&mut self) {
        self.remember_lists();
        match self.kind {
            Kind::Footprints => {
                let name = self.unique("NEW_FOOTPRINT");
                self.footprints.push(FootprintDef {
                    name,
                    category: self.name.clone(),
                    description: String::new(),
                    prefix: "U".into(),
                    pads: Vec::new(),
                    graphics: Vec::new(),
                    label_at: p(0.0, 2.0),
                    keepouts: Vec::new(),
                    source: String::new(),
                });
                self.fp = self.footprints.len() - 1;
            }
            Kind::Symbols => {
                let name = self.unique("NEW_SYMBOL");
                self.symbols.push(Symbol {
                    name,
                    description: String::new(),
                    prefix: "U".into(),
                    parts: 1,
                    pins: Vec::new(),
                    drawings: Vec::new(),
                    footprints: Vec::new(),
                    library: self.name.clone(),
                });
                self.sym = self.symbols.len() - 1;
                self.part = 1;
            }
        }
        self.sel = None;
        self.pending = None;
    }

    pub fn duplicate(&mut self) {
        if self.count() == 0 {
            return;
        }
        self.remember_lists();
        match self.kind {
            Kind::Footprints => {
                let mut f = self.footprints[self.fp].clone();
                f.name = self.unique(&format!("{}_copy", f.name));
                self.footprints.push(f);
                self.fp = self.footprints.len() - 1;
            }
            Kind::Symbols => {
                let mut s = self.symbols[self.sym].clone();
                s.name = self.unique(&format!("{}_copy", s.name));
                self.symbols.push(s);
                self.sym = self.symbols.len() - 1;
            }
        }
        self.sel = None;
    }

    pub fn remove_current(&mut self) {
        if self.count() == 0 {
            return;
        }
        self.remember_lists();
        match self.kind {
            Kind::Footprints => {
                self.footprints.remove(self.fp);
                self.fp = self.fp.min(self.footprints.len().saturating_sub(1));
            }
            Kind::Symbols => {
                self.symbols.remove(self.sym);
                self.sym = self.sym.min(self.symbols.len().saturating_sub(1));
            }
        }
        self.sel = None;
        self.part = 1;
    }

    /// Gives a footprint or symbol a name that no other has.
    #[cfg(test)]
    pub fn rename_current(&mut self, name: &str) {
        let name = name.trim();
        if name.is_empty() || self.names().iter().enumerate().any(|(i, n)| n == name && i != self.current()) {
            return;
        }
        self.remember_run();
        match self.kind {
            Kind::Footprints => self.footprints[self.fp].name = name.to_string(),
            Kind::Symbols => self.symbols[self.sym].name = name.to_string(),
        }
    }

    // -- The drawing ---------------------------------------------------------------

    pub fn snap(&self, q: P) -> P {
        q.snap(self.grid)
    }

    /// The next pad number: one more than the largest there is.
    pub fn next_number(&self) -> String {
        let nums: Vec<&str> = match self.kind {
            Kind::Footprints => self.footprint().map(|f| f.pads.iter().map(|p| p.num.as_str()).collect()).unwrap_or_default(),
            Kind::Symbols => self.symbol().map(|s| s.pins.iter().map(|p| p.number.as_str()).collect()).unwrap_or_default(),
        };
        (nums.iter().filter_map(|n| n.parse::<u32>().ok()).max().unwrap_or(0) + 1).to_string()
    }

    /// A click with a tool: puts something down, or starts or finishes a shape.
    pub fn place(&mut self, q: P) {
        let q = self.snap(q);
        match (self.kind, self.tool) {
            (_, Tool::Select) => {}
            (Kind::Footprints, Tool::Pad) => {
                if self.footprint().is_none() {
                    return;
                }
                self.remember();
                let mut pad = self.pad_proto.clone();
                pad.at = q;
                pad.num = self.next_number();
                let f = &mut self.footprints[self.fp];
                f.pads.push(pad);
                self.sel = Some(Item::Pad(f.pads.len() - 1));
            }
            (Kind::Symbols, Tool::Pad) => {
                if self.symbol().is_none() {
                    return;
                }
                self.remember();
                let pin = Pin { part: self.part, number: self.next_number(), name: String::new(), at: q, length: 7.62, angle: self.pin_angle, kind: PinKind::Passive, hidden: false };
                let s = &mut self.symbols[self.sym];
                s.pins.push(pin);
                self.sel = Some(Item::Pin(s.pins.len() - 1));
            }
            (_, Tool::Line | Tool::Shape) => {
                let Some(first) = self.pending.take() else {
                    if self.count() > 0 {
                        self.pending = Some(q);
                    }
                    return;
                };
                if first.same(q) {
                    return;
                }
                self.remember();
                match (self.kind, self.tool) {
                    (Kind::Footprints, Tool::Line) => {
                        let layer = self.layer;
                        self.footprints[self.fp].graphics.push(Graphic::Line { a: first, b: q, w: 0.15, layer });
                        self.sel = Some(Item::Gfx(self.footprints[self.fp].graphics.len() - 1));
                    }
                    (Kind::Footprints, _) => {
                        let layer = self.layer;
                        self.footprints[self.fp].graphics.push(Graphic::Circle { c: first, r: nm(first.dist(q)), w: 0.15, layer, fill: false });
                        self.sel = Some(Item::Gfx(self.footprints[self.fp].graphics.len() - 1));
                    }
                    (Kind::Symbols, tool) => {
                        let part = self.part;
                        let s = &mut self.symbols[self.sym];
                        s.drawings.push((part, if tool == Tool::Line { Drawing::Line(first, q) } else { Drawing::Rect(first, q) }));
                        self.sel = Some(Item::Draw(s.drawings.len() - 1));
                    }
                }
            }
        }
    }

    /// Where a point of the picked thing is, to drag it by.
    pub fn anchor(&self, item: Item) -> Option<P> {
        Some(match item {
            Item::Pad(i) => self.footprint()?.pads.get(i)?.at,
            Item::Gfx(i) => match self.footprint()?.graphics.get(i)? {
                Graphic::Line { a, .. } => *a,
                Graphic::Circle { c, .. } => *c,
            },
            Item::Pin(i) => self.symbol()?.pins.get(i)?.at,
            Item::Draw(i) => match &self.symbol()?.drawings.get(i)?.1 {
                Drawing::Line(a, _) | Drawing::Rect(a, _) => *a,
                Drawing::Polyline(v) | Drawing::Polygon(v) => *v.first()?,
                Drawing::Arc { c, .. } | Drawing::Ellipse { c, .. } => *c,
                Drawing::Text { at, .. } => *at,
            },
        })
    }

    /// Moves a thing by `d`.
    pub fn translate(&mut self, item: Item, d: P) {
        let (fp, sym) = (self.fp, self.sym);
        match item {
            Item::Pad(i) => {
                if let Some(pad) = self.footprints.get_mut(fp).and_then(|f| f.pads.get_mut(i)) {
                    pad.at = pad.at.add(d).rounded();
                }
            }
            Item::Gfx(i) => match self.footprints.get_mut(fp).and_then(|f| f.graphics.get_mut(i)) {
                Some(Graphic::Line { a, b, .. }) => {
                    *a = a.add(d).rounded();
                    *b = b.add(d).rounded();
                }
                Some(Graphic::Circle { c, .. }) => *c = c.add(d).rounded(),
                None => {}
            },
            Item::Pin(i) => {
                if let Some(pin) = self.symbols.get_mut(sym).and_then(|s| s.pins.get_mut(i)) {
                    pin.at = pin.at.add(d).rounded();
                }
            }
            Item::Draw(i) => {
                if let Some((_, shape)) = self.symbols.get_mut(sym).and_then(|s| s.drawings.get_mut(i)) {
                    let mv = |q: &mut P| *q = q.add(d).rounded();
                    match shape {
                        Drawing::Line(a, b) | Drawing::Rect(a, b) => {
                            mv(a);
                            mv(b);
                        }
                        Drawing::Polyline(v) | Drawing::Polygon(v) => v.iter_mut().for_each(mv),
                        Drawing::Arc { c, .. } | Drawing::Ellipse { c, .. } => mv(c),
                        Drawing::Text { at, .. } => mv(at),
                    }
                }
            }
        }
    }

    pub fn delete(&mut self, item: Item) {
        if self.anchor(item).is_none() {
            return;
        }
        self.remember();
        match item {
            Item::Pad(i) => {
                self.footprints[self.fp].pads.remove(i);
            }
            Item::Gfx(i) => {
                self.footprints[self.fp].graphics.remove(i);
            }
            Item::Pin(i) => {
                self.symbols[self.sym].pins.remove(i);
            }
            Item::Draw(i) => {
                self.symbols[self.sym].drawings.remove(i);
            }
        }
        self.sel = None;
    }

    /// Turns a pad a quarter, or a pin to point the next way round.
    pub fn rotate(&mut self, item: Item) {
        if self.anchor(item).is_none() {
            return;
        }
        match item {
            Item::Pad(i) => {
                self.remember();
                let pad = &mut self.footprints[self.fp].pads[i];
                pad.rot = (pad.rot + 90.0).rem_euclid(360.0);
            }
            Item::Pin(i) => {
                self.remember();
                let pin = &mut self.symbols[self.sym].pins[i];
                pin.angle = (pin.angle + 90.0).rem_euclid(360.0);
            }
            _ => {}
        }
    }

    /// The thing at a point, to within `tol`; the topmost of what's there.
    pub fn pick(&self, q: P, tol: f64) -> Option<Item> {
        match self.kind {
            Kind::Footprints => {
                let f = self.footprint()?;
                for (i, pad) in f.pads.iter().enumerate().rev() {
                    if pad.blob_local(tol).bbox().contains(q) {
                        return Some(Item::Pad(i));
                    }
                }
                f.graphics.iter().enumerate().rev().find_map(|(i, g)| {
                    let near = match g {
                        Graphic::Line { a, b, w, .. } => dist_to_segment(q, *a, *b).0 <= tol + w / 2.0,
                        Graphic::Circle { c, r, w, fill, .. } => {
                            let d = q.dist(*c);
                            if *fill { d <= r + tol } else { (d - r).abs() <= tol + w / 2.0 }
                        }
                    };
                    near.then_some(Item::Gfx(i))
                })
            }
            Kind::Symbols => {
                let s = self.symbol()?;
                for (i, pin) in s.pins.iter().enumerate().rev() {
                    if pin.part == self.part && dist_to_segment(q, pin.at, pin.tip()).0 <= tol {
                        return Some(Item::Pin(i));
                    }
                }
                s.drawings.iter().enumerate().rev().find_map(|(i, (part, d))| {
                    if *part != 0 && *part != self.part {
                        return None;
                    }
                    let near_path = |pts: &[P], closed: bool| {
                        let n = pts.len();
                        (0..if closed { n } else { n.saturating_sub(1) }).any(|k| dist_to_segment(q, pts[k], pts[(k + 1) % n]).0 <= tol)
                    };
                    let near = match d {
                        Drawing::Line(a, b) => dist_to_segment(q, *a, *b).0 <= tol,
                        Drawing::Rect(a, b) => near_path(&[*a, p(b.x, a.y), *b, p(a.x, b.y)], true),
                        Drawing::Polyline(v) => near_path(v, false),
                        Drawing::Polygon(v) => near_path(v, true),
                        Drawing::Arc { c, r, .. } => (q.dist(*c) - r).abs() <= tol,
                        Drawing::Ellipse { c, rx, ry } => BBox::of(&[*c]).grown(rx.max(*ry)).contains(q) && ((q.x - c.x) / rx.max(1e-9)).powi(2) + ((q.y - c.y) / ry.max(1e-9)).powi(2) <= 1.0 + tol,
                        Drawing::Text { at, text } => BBox::of(&[*at, at.add(p(text.chars().count() as f64 * 1.3, 2.0))]).grown(tol).contains(q),
                    };
                    near.then_some(Item::Draw(i))
                })
            }
        }
    }

    /// Everything in the footprint or symbol, to fit in view.
    pub fn extent(&self) -> BBox {
        match self.kind {
            Kind::Footprints => self.footprint().map(|f| f.bbox()).unwrap_or_default(),
            Kind::Symbols => {
                let mut b = BBox::default();
                if let Some(s) = self.symbol() {
                    for pin in &s.pins {
                        b.add(pin.at);
                        b.add(pin.tip());
                    }
                    for (_, d) in &s.drawings {
                        match d {
                            Drawing::Line(a, c) | Drawing::Rect(a, c) => {
                                b.add(*a);
                                b.add(*c);
                            }
                            Drawing::Polyline(v) | Drawing::Polygon(v) => v.iter().for_each(|q| b.add(*q)),
                            Drawing::Arc { c, r, .. } => b.union(&BBox::of(&[*c]).grown(*r)),
                            Drawing::Ellipse { c, rx, ry } => b.union(&BBox::of(&[*c]).grown(rx.max(*ry))),
                            Drawing::Text { at, .. } => b.add(*at),
                        }
                    }
                }
                b
            }
        }
    }

    // -- Wizards -------------------------------------------------------------------

    /// Puts a row of pads down, numbered on from the last: `n` of them, `pitch`
    /// apart, going along x or y.
    pub fn pad_row(&mut self, n: usize, pitch: f64, from: P, along_x: bool) {
        if self.kind != Kind::Footprints || self.footprint().is_none() || n == 0 {
            return;
        }
        self.remember();
        let first: u32 = self.next_number().parse().unwrap_or(1);
        for k in 0..n {
            let mut pad = self.pad_proto.clone();
            let off = pitch * k as f64;
            pad.at = from.add(if along_x { p(off, 0.0) } else { p(0.0, -off) }).rounded();
            pad.num = (first + k as u32).to_string();
            self.footprints[self.fp].pads.push(pad);
        }
    }

    /// Replaces the pads with two rows of `n` each `spacing` apart (centre to
    /// centre) at `pitch`, numbered down the left and up the right as dual
    /// in-line packages are, and draws a body outline with a mark at pin 1.
    pub fn dual_row(&mut self, n: usize, pitch: f64, spacing: f64) {
        if self.kind != Kind::Footprints || self.footprint().is_none() || n == 0 {
            return;
        }
        self.remember();
        let top = pitch * (n as f64 - 1.0) / 2.0;
        let mut pads = Vec::new();
        for k in 0..n {
            let mut left = self.pad_proto.clone();
            left.at = p(-spacing / 2.0, top - pitch * k as f64).rounded();
            left.num = (k + 1).to_string();
            let mut right = self.pad_proto.clone();
            right.at = p(spacing / 2.0, -top + pitch * k as f64).rounded();
            right.num = (n + k + 1).to_string();
            pads.push(left);
            pads.push(right);
        }
        let half_w = spacing / 2.0 - self.pad_proto.size.x.max(self.pad_proto.size.y) / 2.0 - 0.3;
        let half_h = top + pitch / 2.0;
        let silk = Layer::Silk(Side::Top);
        let mut gfx = Vec::new();
        let corners = [p(-half_w, half_h), p(half_w, half_h), p(half_w, -half_h), p(-half_w, -half_h)];
        for k in 0..4 {
            gfx.push(Graphic::Line { a: corners[k].rounded(), b: corners[(k + 1) % 4].rounded(), w: 0.15, layer: silk });
        }
        gfx.push(Graphic::Circle { c: p(-half_w + 0.6, half_h - 0.6).rounded(), r: 0.25, w: 0.1, layer: silk, fill: false });
        let f = &mut self.footprints[self.fp];
        f.pads = pads;
        f.graphics.retain(|g| g.layer() != silk);
        f.graphics.extend(gfx);
        f.label_at = p(0.0, half_h + 1.0).rounded();
        self.sel = None;
    }

    // -- Files ---------------------------------------------------------------------

    /// Reads a library in. A PcbLib replaces the footprints, a SchLib the
    /// symbols, an IntLib both. Returns what to tell the user.
    pub fn open(&mut self, path: &Path) -> Result<String, String> {
        let got = altium::read(path)?;
        let ext = path.extension().unwrap_or_default().to_string_lossy().to_lowercase();
        let (nf, ns) = (got.footprints.len(), got.symbols.len());
        match ext.as_str() {
            "pcblib" => {
                self.footprints = got.footprints;
                self.fp_path = Some(path.to_path_buf());
            }
            "schlib" => {
                self.symbols = got.symbols;
                self.sym_path = Some(path.to_path_buf());
            }
            _ => {
                self.footprints = got.footprints;
                self.symbols = got.symbols;
                self.fp_path = None;
                self.sym_path = None;
            }
        }
        self.name = path.file_stem().unwrap_or_default().to_string_lossy().to_string();
        self.notes = got.notes;
        self.fp = 0;
        self.sym = 0;
        self.part = 1;
        self.undo.clear();
        self.redo.clear();
        self.dirty = false;
        let kind = if ext == "schlib" || (ext != "pcblib" && nf == 0) { Kind::Symbols } else { Kind::Footprints };
        self.set_kind(kind);
        self.pending = None;
        Ok(format!("{}: {nf} footprint{}, {ns} symbol{}", path.file_name().unwrap_or_default().to_string_lossy(), if nf == 1 { "" } else { "s" }, if ns == 1 { "" } else { "s" }))
    }

    /// The file the current kind would be saved to, if it has one.
    pub fn path(&self) -> Option<&Path> {
        match self.kind {
            Kind::Footprints => self.fp_path.as_deref(),
            Kind::Symbols => self.sym_path.as_deref(),
        }
    }

    /// Writes the footprints (to a PcbLib) or symbols (to a SchLib).
    pub fn save_to(&mut self, path: &Path) -> Result<(), String> {
        let bytes = match self.kind {
            Kind::Footprints => write::pcblib(&self.footprints)?,
            Kind::Symbols => write::schlib(&self.symbols)?,
        };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        std::fs::write(path, bytes).map_err(|e| format!("{}: {e}", path.display()))?;
        match self.kind {
            Kind::Footprints => self.fp_path = Some(path.to_path_buf()),
            Kind::Symbols => self.sym_path = Some(path.to_path_buf()),
        }
        self.dirty = false;
        Ok(())
    }

    /// Footprint names a symbol links to that aren't in the footprints here or
    /// in `elsewhere`.
    pub fn unknown_links(&self, elsewhere: &[String]) -> Vec<String> {
        let Some(s) = self.symbol() else { return Vec::new() };
        s.footprints.iter().filter(|n| !self.footprints.iter().any(|f| &f.name == *n) && !elsewhere.contains(n)).cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn editor() -> LibEditor {
        let mut e = LibEditor::new(Kind::Footprints);
        e.new_item();
        e
    }

    #[test]
    fn pads_are_put_down_on_the_grid_and_numbered_on() {
        let mut e = editor();
        e.tool = Tool::Pad;
        e.place(p(1.13, -0.9));
        e.place(p(3.6, 0.1));
        let f = e.footprint().unwrap();
        assert_eq!(f.pads.len(), 2);
        assert_eq!((f.pads[0].num.as_str(), f.pads[1].num.as_str()), ("1", "2"));
        assert_eq!(f.pads[0].at, p(1.25, -1.0), "snapped to the 0.25 grid");
        assert_eq!(e.sel, Some(Item::Pad(1)), "the new pad is picked");
        // A pad in a footprint takes the shape the editor holds.
        e.pad_proto = Pad { shape: PadShape::Circle, size: p(2.0, 2.0), drill: 1.0, ..pad_proto() };
        e.place(p(0.0, 0.0));
        assert_eq!(e.footprint().unwrap().pads[2].drill, 1.0);
    }

    #[test]
    fn a_line_takes_two_clicks_and_a_circle_gets_its_radius() {
        let mut e = editor();
        e.tool = Tool::Line;
        e.place(p(0.0, 0.0));
        assert!(e.footprint().unwrap().graphics.is_empty() && e.pending.is_some());
        e.place(p(5.0, 0.0));
        assert!(matches!(e.footprint().unwrap().graphics[0], Graphic::Line { b, layer: Layer::Silk(Side::Top), .. } if b == p(5.0, 0.0)));
        assert!(e.pending.is_none());
        e.tool = Tool::Shape;
        e.place(p(1.0, 1.0));
        e.place(p(1.0, 3.0));
        assert!(matches!(e.footprint().unwrap().graphics[1], Graphic::Circle { r, .. } if (r - 2.0).abs() < 1e-9));
        // The same point twice makes nothing.
        e.tool = Tool::Line;
        e.place(p(2.0, 2.0));
        e.place(p(2.0, 2.0));
        assert_eq!(e.footprint().unwrap().graphics.len(), 2);
    }

    #[test]
    fn what_is_under_the_pointer_is_picked() {
        let mut e = editor();
        e.tool = Tool::Pad;
        e.place(p(0.0, 0.0));
        e.tool = Tool::Line;
        e.place(p(-4.0, 3.0));
        e.place(p(4.0, 3.0));
        e.tool = Tool::Select;
        assert_eq!(e.pick(p(0.3, -0.2), 0.1), Some(Item::Pad(0)));
        assert_eq!(e.pick(p(1.0, 3.05), 0.1), Some(Item::Gfx(0)));
        assert_eq!(e.pick(p(1.0, 2.0), 0.1), None);
    }

    #[test]
    fn things_move_and_go_and_come_back() {
        let mut e = editor();
        e.tool = Tool::Pad;
        e.place(p(0.0, 0.0));
        e.tool = Tool::Line;
        e.place(p(0.0, 2.0));
        e.place(p(2.0, 2.0));
        e.translate(Item::Gfx(0), p(1.0, 1.0));
        let Graphic::Line { a, b, .. } = e.footprint().unwrap().graphics[0].clone() else { panic!() };
        assert_eq!((a, b), (p(1.0, 3.0), p(3.0, 3.0)));
        e.remember();
        e.translate(Item::Pad(0), p(0.5, 0.0));
        assert_eq!(e.anchor(Item::Pad(0)), Some(p(0.5, 0.0)));
        e.undo();
        assert_eq!(e.anchor(Item::Pad(0)), Some(p(0.0, 0.0)));
        e.redo();
        assert_eq!(e.anchor(Item::Pad(0)), Some(p(0.5, 0.0)));
        e.delete(Item::Pad(0));
        assert!(e.footprint().unwrap().pads.is_empty());
        e.undo();
        assert_eq!(e.footprint().unwrap().pads.len(), 1);
    }

    #[test]
    fn adding_and_removing_footprints_can_be_undone_with_their_names_kept_apart() {
        let mut e = editor();
        e.new_item();
        e.new_item();
        let names = e.names();
        assert_eq!(names, vec!["NEW_FOOTPRINT", "NEW_FOOTPRINT_2", "NEW_FOOTPRINT_3"]);
        assert_eq!(e.fp, 2);
        e.rename_current("NEW_FOOTPRINT");
        assert_eq!(e.names()[2], "NEW_FOOTPRINT_3", "a name already used is refused");
        e.rename_current("QFN-16");
        assert_eq!(e.names()[2], "QFN-16");
        e.duplicate();
        assert_eq!(e.names()[3], "QFN-16_copy");
        e.remove_current();
        assert_eq!(e.footprints.len(), 3);
        e.undo();
        assert_eq!(e.footprints.len(), 4);
        e.undo();
        assert_eq!(e.footprints.len(), 3);
    }

    #[test]
    fn a_run_of_number_edits_is_one_undo() {
        let mut e = editor();
        e.tool = Tool::Pad;
        e.place(p(0.0, 0.0));
        let before = e.undo.len();
        for k in 1..=5 {
            e.remember_run();
            e.footprints[0].pads[0].size.x = k as f64;
        }
        assert_eq!(e.undo.len(), before + 1);
        e.undo();
        assert_eq!(e.footprints[0].pads[0].size.x, 1.5);
    }

    #[test]
    fn a_dual_row_is_numbered_like_a_dip() {
        let mut e = editor();
        e.pad_proto.size = p(1.6, 1.6);
        e.dual_row(4, 2.54, 7.62);
        let f = e.footprint().unwrap();
        assert_eq!(f.pads.len(), 8);
        let at = |n: &str| f.pads.iter().find(|p| p.num == n).unwrap().at;
        // Pin 1 top left, down the left side, pin 5 bottom right and up.
        assert_eq!(at("1"), p(-3.81, 3.81));
        assert_eq!(at("4"), p(-3.81, -3.81));
        assert_eq!(at("5"), p(3.81, -3.81));
        assert_eq!(at("8"), p(3.81, 3.81));
        assert!(f.graphics.iter().any(|g| matches!(g, Graphic::Circle { .. })), "a mark at pin 1");
        // Run again: the pads are replaced, the outline too.
        e.dual_row(3, 2.54, 7.62);
        assert_eq!(e.footprint().unwrap().pads.len(), 6);
        assert_eq!(e.footprint().unwrap().graphics.iter().filter(|g| matches!(g, Graphic::Line { .. })).count(), 4);
    }

    #[test]
    fn a_row_of_pads_goes_on_from_the_last_number() {
        let mut e = editor();
        e.pad_row(3, 1.27, p(0.0, 0.0), true);
        e.pad_row(2, 1.27, p(0.0, 5.0), false);
        let nums: Vec<&str> = e.footprint().unwrap().pads.iter().map(|p| p.num.as_str()).collect();
        assert_eq!(nums, ["1", "2", "3", "4", "5"]);
        assert_eq!(e.footprint().unwrap().pads[4].at, p(0.0, 3.73));
    }

    #[test]
    fn symbols_get_pins_that_point_and_a_body() {
        let mut e = LibEditor::new(Kind::Symbols);
        e.new_item();
        e.tool = Tool::Pad;
        e.pin_angle = 0.0;
        e.place(p(-10.0, 5.0));
        e.place(p(-10.0, 0.0));
        e.tool = Tool::Shape;
        e.place(p(-5.0, 7.0));
        e.place(p(5.0, -3.0));
        let s = e.symbol().unwrap();
        assert_eq!(s.pins.len(), 2);
        assert_eq!(s.pins[1].number, "2");
        assert_eq!(s.pins[0].at, p(-10.16, 5.08), "on the schematic grid");
        assert_eq!(s.pins[0].tip(), p(-10.16 + 7.62, 5.08), "reaches out the way it points");
        assert!(matches!(s.drawings[0], (1, Drawing::Rect(..))), "{:?}", s.drawings);
        e.tool = Tool::Select;
        assert_eq!(e.pick(p(-7.0, 5.0), 0.5), Some(Item::Pin(0)));
        assert_eq!(e.pick(p(0.0, 7.62), 0.5), Some(Item::Draw(0)), "the top edge of the body");
        assert_eq!(e.pick(p(0.0, 2.0), 0.5), None, "inside a body is empty");
        e.rotate(Item::Pin(0));
        assert_eq!(e.symbol().unwrap().pins[0].angle, 90.0);
        // Another part's pins aren't there to pick.
        e.part = 2;
        assert_eq!(e.pick(p(-7.0, 5.0), 0.5), None);
    }

    #[test]
    fn a_library_is_written_and_opened_again() {
        let dir = std::env::temp_dir().join(format!("coppersmith-libedit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut e = editor();
        e.rename_current("TEST_SOT");
        e.dual_row(3, 0.95, 2.4);
        e.footprints[0].description = "three pins a side".into();
        e.save_to(&dir.join("mine.PcbLib")).unwrap();
        assert!(!e.dirty);
        let mut s = LibEditor::new(Kind::Symbols);
        s.new_item();
        s.symbols[0].footprints = vec!["TEST_SOT".into(), "NOT_THERE".into()];
        s.tool = Tool::Pad;
        s.place(p(0.0, 0.0));
        s.save_to(&dir.join("mine.SchLib")).unwrap();
        // A footprint in the other library counts as known.
        assert_eq!(s.unknown_links(&["TEST_SOT".into()]), vec!["NOT_THERE".to_string()]);
        assert!(s.unknown_links(&["TEST_SOT".into(), "NOT_THERE".into()]).is_empty());
        // Both come into one editor.
        let mut back = LibEditor::default();
        back.open(&dir.join("mine.PcbLib")).unwrap();
        back.open(&dir.join("mine.SchLib")).unwrap();
        assert_eq!(back.footprints[0].name, "TEST_SOT");
        assert_eq!(back.footprints[0].pads.len(), 6);
        assert_eq!(back.footprints[0].description, "three pins a side");
        assert_eq!(back.symbols[0].pins.len(), 1);
        assert_eq!(back.kind, Kind::Symbols);
        assert!(back.path().is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
