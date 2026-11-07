//! Writing Altium library files: the other half of reading them.
//!
//! A `.PcbLib` is a compound file with a storage per footprint, as described
//! in `pcblib`; this writes the same records back. It is checked by reading
//! its own output (and that of the real test libraries) back and comparing,
//! not yet by opening it in Altium or Circuit Studio, which hasn't been done.
//!
//! What is written: pads (round, rectangular, rounded rectangular, oval; with
//! holes), lines and circles on the silkscreen, courtyard and assembly
//! layers, and keepouts. Not written: 3D bodies, text, and the extra
//! bookkeeping Altium keeps (unique ids, embedded models). Altium fills those
//! in itself when it next saves.

use super::{UNIT, prims};
use crate::pcb::geom::*;
use crate::pcb::library::FootprintDef;
use crate::pcb::model::*;
use std::io::{Cursor, Write};

/// The mechanical layers a library's courtyard and assembly drawings go on,
/// and the names that tell the reader which is which.
const COURT_LAYER: u8 = prims::MECHANICAL_1;
const FAB_LAYER: u8 = prims::MECHANICAL_1 + 1;

/// Bytes, built up the way Altium's files are: little-endian numbers,
/// strings with a one-byte length, blocks with a four-byte one.
#[derive(Default)]
pub struct Out(pub Vec<u8>);

impl Out {
    pub fn u8(&mut self, v: u8) -> &mut Self {
        self.0.push(v);
        self
    }
    pub fn u16(&mut self, v: u16) -> &mut Self {
        self.0.extend(v.to_le_bytes());
        self
    }
    pub fn u32(&mut self, v: u32) -> &mut Self {
        self.0.extend(v.to_le_bytes());
        self
    }
    pub fn i32(&mut self, v: i32) -> &mut Self {
        self.0.extend(v.to_le_bytes());
        self
    }
    pub fn f64(&mut self, v: f64) -> &mut Self {
        self.0.extend(v.to_le_bytes());
        self
    }
    /// A length in millimetres, in Altium's unit.
    pub fn mm(&mut self, v: f64) -> &mut Self {
        self.i32((v / UNIT).round() as i32)
    }
    pub fn zeros(&mut self, n: usize) -> &mut Self {
        self.0.resize(self.0.len() + n, 0);
        self
    }
    pub fn pascal(&mut self, s: &str) -> &mut Self {
        let b = latin(s);
        self.u8(b.len().min(255) as u8);
        self.0.extend(&b[..b.len().min(255)]);
        self
    }
    pub fn block(&mut self, b: &[u8]) -> &mut Self {
        self.u32(b.len() as u32);
        self.0.extend(b);
        self
    }
}

/// Text as Altium's older strings hold it: one byte a letter, so anything
/// outside Latin-1 becomes a question mark.
fn latin(s: &str) -> Vec<u8> {
    s.chars().map(|c| if (c as u32) < 256 { c as u8 } else { b'?' }).collect()
}

/// `|KEY=VALUE|…` as a block's contents, ending in a zero as Altium's do.
pub fn param_text<K: AsRef<str>>(pairs: &[(K, String)]) -> Vec<u8> {
    let mut t = String::new();
    for (k, v) in pairs {
        // A bar in a value would end it early.
        t.push_str(&format!("|{}={}", k.as_ref(), v.replace('|', "/")));
    }
    let mut b = latin(&t);
    b.push(0);
    b
}

/// The head every board primitive starts with.
fn head(o: &mut Out, layer: u8, flags: u16) {
    o.u8(layer).u16(flags).u16(prims::NONE).u16(prims::NONE).u16(prims::NONE).zeros(4);
}

/// Altium's layer for a drawing here, if it has one.
fn altium_layer(l: Layer) -> Option<u8> {
    match l {
        Layer::Silk(Side::Top) => Some(prims::TOP_OVERLAY),
        Layer::Silk(Side::Bottom) => Some(prims::BOTTOM_OVERLAY),
        Layer::Court(Side::Top) => Some(COURT_LAYER),
        Layer::Fab(Side::Top) => Some(FAB_LAYER),
        _ => None,
    }
}

fn track(a: P, b: P, w: f64, layer: u8) -> Out {
    let mut blk = Out::default();
    head(&mut blk, layer, 0);
    blk.mm(a.x).mm(a.y).mm(b.x).mm(b.y).mm(w).zeros(9);
    let mut o = Out::default();
    o.u8(4).block(&blk.0);
    o
}

fn arc(c: P, r: f64, w: f64, layer: u8) -> Out {
    let mut blk = Out::default();
    head(&mut blk, layer, 0);
    blk.mm(c.x).mm(c.y).mm(r).f64(0.0).f64(360.0).mm(w).zeros(4);
    let mut o = Out::default();
    o.u8(1).block(&blk.0);
    o
}

/// A region: corners as pairs of doubles in Altium units.
fn region(pts: &[P], layer: u8, flags: u16, props: &[u8]) -> Out {
    let mut blk = Out::default();
    blk.u8(layer).u16(flags).u16(prims::NONE).u16(prims::NONE).u16(prims::NONE).zeros(5).u16(0).zeros(2).block(props);
    blk.u32(pts.len() as u32);
    for q in pts {
        blk.f64(q.x / UNIT).f64(q.y / UNIT);
    }
    let mut o = Out::default();
    o.u8(11).block(&blk.0);
    o
}

fn pad(pad: &Pad) -> Out {
    let through = pad.drill > 0.0;
    // A hole with no copper round it is a pad as big as its hole, unplated.
    let plain_hole = through && (pad.size.x - pad.drill).abs() < 1e-6 && (pad.size.y - pad.drill).abs() < 1e-6;
    let shape = match pad.shape {
        PadShape::Circle | PadShape::Oval => 1,
        PadShape::Rect | PadShape::RoundRect(_) => 2,
    };
    let mut g = Out::default();
    head(&mut g, if through { prims::MULTI_LAYER } else { prims::TOP }, 0);
    g.mm(pad.at.x).mm(pad.at.y).mm(pad.size.x).mm(pad.size.y);
    g.mm(pad.size.x).mm(pad.size.y).mm(pad.size.x).mm(pad.size.y); // middle and bottom
    g.mm(if through { pad.drill } else { 0.0 }).u8(shape).zeros(2).f64(pad.rot).u8(u8::from(!plain_hole));
    g.zeros(120usize.saturating_sub(g.0.len()));
    // The size-and-shape block: where a rounded rectangle's corner is said.
    let mut s = Out::default();
    s.zeros(596);
    if let PadShape::RoundRect(k) = pad.shape {
        s.0[532] = 9;
        s.0[564] = (k * 200.0).round().clamp(0.0, 100.0) as u8;
    }
    let mut o = Out::default();
    o.u8(2).block(&{
        let mut n = Out::default();
        n.pascal(&pad.num);
        n.0
    });
    o.block(&[]).block(&{
        let mut n = Out::default();
        n.pascal("|&|0");
        n.0
    });
    o.block(&[]).block(&g.0).block(&s.0);
    o
}

/// A circle that's filled, as a many-sided region.
fn disc(c: P, r: f64) -> Vec<P> {
    (0..32).map(|k| c.add(p((k as f64 * std::f64::consts::TAU / 32.0).cos(), (k as f64 * std::f64::consts::TAU / 32.0).sin()).scale(r))).collect()
}

/// A footprint's records, with how many there are.
fn records(f: &FootprintDef) -> (Vec<u8>, u32) {
    let mut body = Out::default();
    body.block(&{
        let mut n = Out::default();
        n.pascal(&f.name);
        n.0
    });
    let mut count = 0;
    let mut add = |o: Out| {
        body.0.extend(o.0);
        count += 1;
    };
    for pd in &f.pads {
        add(pad(pd));
    }
    for g in &f.graphics {
        let Some(layer) = altium_layer(g.layer()) else { continue };
        match g {
            Graphic::Line { a, b, w, .. } => add(track(*a, *b, *w, layer)),
            Graphic::Circle { c, r, w, fill: false, .. } => add(arc(*c, *r, *w, layer)),
            Graphic::Circle { c, r, fill: true, .. } => add(region(&disc(*c, *r), layer, 0, &param_text(&[("KIND", "0".into())]))),
        }
    }
    for k in &f.keepouts {
        // On every layer: Altium's Keep-Out layer. On one, that copper layer, flagged.
        let (layer, flags) = match k.layer {
            None => (prims::KEEPOUT_LAYER, 0),
            Some(l) if l == TOP => (prims::TOP, prims::KEEPOUT_FLAG),
            Some(_) => (prims::BOTTOM, prims::KEEPOUT_FLAG),
        };
        let props = param_text(&[("KIND", "0".into()), ("KEEPOUTRESTRICTIONS", k.restrict.to_altium().to_string())]);
        add(region(&k.pts, layer, flags, &props));
    }
    (body.0, count)
}

/// A storage's name has to fit in 31 letters and not hold a slash; the true
/// name goes in its parameters. Two that come out the same are told apart.
fn storage_names(names: &[&str]) -> Vec<String> {
    let mut used: Vec<String> = Vec::new();
    for n in names {
        let clean: String = n.chars().map(|c| if matches!(c, '/' | '\\' | ':' | '!') || c.is_control() { '_' } else { c }).collect();
        let mut base: String = clean.chars().take(31).collect();
        let mut k = 1;
        while base.is_empty() || used.iter().any(|u| u.eq_ignore_ascii_case(&base)) {
            let tail = format!("~{k}");
            base = format!("{}{tail}", clean.chars().take(31 - tail.len()).collect::<String>());
            k += 1;
        }
        used.push(base);
    }
    used
}

/// A PcbLib holding these footprints.
pub fn pcblib(footprints: &[FootprintDef]) -> Result<Vec<u8>, String> {
    let err = |e: std::io::Error| format!("couldn't write the library: {e}");
    let mut file = cfb::CompoundFile::create(Cursor::new(Vec::new())).map_err(err)?;
    let put = |file: &mut cfb::CompoundFile<Cursor<Vec<u8>>>, path: &str, bytes: &[u8]| -> Result<(), String> {
        file.create_stream(path).map_err(err)?.write_all(bytes).map_err(err)
    };
    let mut header = Out::default();
    header.block(&{
        let mut n = Out::default();
        n.pascal("PCB 6.0 Binary Library File");
        n.0
    });
    header.f64(5.01).block(&{
        let mut n = Out::default();
        n.pascal("COPPERSM");
        n.0
    });
    put(&mut file, "/FileHeader", &header.0)?;
    file.create_storage("/Library").map_err(err)?;
    let lib = param_text(&[
        ("KIND", "Protel_Advanced_PCB_Library".into()),
        ("VERSION", "3.00".into()),
        ("LAYER57NAME", "Courtyard".into()),
        ("LAYER58NAME", "Assembly".into()),
    ]);
    let mut data = Out::default();
    data.block(&lib);
    put(&mut file, "/Library/Data", &data.0)?;
    put(&mut file, "/Library/Header", &1u32.to_le_bytes())?;
    let names: Vec<&str> = footprints.iter().map(|f| f.name.as_str()).collect();
    for (f, storage) in footprints.iter().zip(storage_names(&names)) {
        file.create_storage(format!("/{storage}")).map_err(err)?;
        let (body, count) = records(f);
        put(&mut file, &format!("/{storage}/Data"), &body)?;
        put(&mut file, &format!("/{storage}/Header"), &count.to_le_bytes())?;
        let mut params = Out::default();
        params.block(&param_text(&[("PATTERN", f.name.clone()), ("HEIGHT", "0mil".into()), ("DESCRIPTION", f.description.clone())]));
        put(&mut file, &format!("/{storage}/Parameters"), &params.0)?;
        let mut wide = Out::default();
        wide.block(&[0]);
        put(&mut file, &format!("/{storage}/WideStrings"), &wide.0)?;
    }
    Ok(file.into_inner().into_inner())
}

// -- Symbols ----------------------------------------------------------------

use super::schlib::{Drawing, PinKind, Symbol};

/// One schematic unit, 10 mil, in millimetres.
const SCH: f64 = 0.254;

/// A record's fields, written in the order they're added.
#[derive(Default)]
struct Rec(Vec<(String, String)>);

impl Rec {
    fn new(kind: u32) -> Rec {
        let mut r = Rec::default();
        r.set("RECORD", kind);
        r
    }
    fn set(&mut self, key: &str, v: impl ToString) -> &mut Self {
        self.0.push((key.to_string(), v.to_string()));
        self
    }
    /// A position or length in millimetres, as whole units and a fraction of one.
    fn mm(&mut self, key: &str, v: f64) -> &mut Self {
        let u = v / SCH;
        let mut whole = u.trunc();
        let mut frac = ((u - whole) * 100_000.0).round();
        if frac.abs() >= 100_000.0 {
            whole += frac.signum();
            frac = 0.0;
        }
        self.set(key, whole as i64);
        if frac != 0.0 {
            self.set(&format!("{key}_FRAC"), frac as i64);
        }
        self
    }
    fn at(&mut self, x: &str, y: &str, q: P) -> &mut Self {
        self.mm(x, q.x).mm(y, q.y)
    }
    /// Which part of the symbol it's on: -1 for all of them.
    fn owner(&mut self, part: u32) -> &mut Self {
        self.set("OWNERPARTID", if part == 0 { -1 } else { part as i64 })
    }
    /// The record with its four-byte head (length, then 0 for text).
    fn bytes(&self) -> Vec<u8> {
        let body = param_text(&self.0);
        let mut o = Out::default();
        o.u32(body.len() as u32);
        o.0.extend(body);
        o.0
    }
}

fn pin_code(k: PinKind) -> u8 {
    match k {
        PinKind::Input => 0,
        PinKind::InOut => 1,
        PinKind::Output => 2,
        PinKind::OpenCollector => 3,
        PinKind::Passive => 4,
        PinKind::HiZ => 5,
        PinKind::OpenEmitter => 6,
        PinKind::Power => 7,
    }
}

/// A symbol's records: the component, its pins and drawings, designator and
/// footprint links.
fn symbol_records(s: &Symbol) -> Vec<u8> {
    let mut out = Vec::new();
    let mut add = |r: &Rec| out.extend(r.bytes());
    let mut c = Rec::new(1);
    c.set("LIBREFERENCE", &s.name).set("COMPONENTDESCRIPTION", &s.description).set("PARTCOUNT", s.parts + 1).set("DISPLAYMODECOUNT", 1).set("CURRENTPARTID", 1);
    add(&c);
    for (part, d) in &s.drawings {
        let mut r;
        match d {
            Drawing::Line(a, b) => {
                r = Rec::new(13);
                r.at("LOCATION.X", "LOCATION.Y", *a).at("CORNER.X", "CORNER.Y", *b);
            }
            Drawing::Rect(a, b) => {
                r = Rec::new(14);
                r.at("LOCATION.X", "LOCATION.Y", *a).at("CORNER.X", "CORNER.Y", *b);
            }
            Drawing::Polyline(pts) | Drawing::Polygon(pts) => {
                r = Rec::new(if matches!(d, Drawing::Polyline(_)) { 6 } else { 7 });
                r.set("LOCATIONCOUNT", pts.len());
                for (i, q) in pts.iter().enumerate() {
                    r.at(&format!("X{}", i + 1), &format!("Y{}", i + 1), *q);
                }
            }
            Drawing::Arc { c, r: radius, start, end } => {
                r = Rec::new(12);
                r.at("LOCATION.X", "LOCATION.Y", *c).mm("RADIUS", *radius).set("STARTANGLE", start).set("ENDANGLE", end);
            }
            Drawing::Ellipse { c, rx, ry } => {
                r = Rec::new(8);
                r.at("LOCATION.X", "LOCATION.Y", *c).mm("RADIUS", *rx).mm("SECONDARYRADIUS", *ry);
            }
            Drawing::Text { at, text } => {
                r = Rec::new(4);
                r.at("LOCATION.X", "LOCATION.Y", *at).set("TEXT", text);
            }
        }
        r.owner(*part);
        add(&r);
    }
    for pin in &s.pins {
        let turn = (pin.angle / 90.0).round() as u32 & 3;
        let mut r = Rec::new(2);
        r.owner(pin.part.max(1))
            .set("ELECTRICAL", pin_code(pin.kind))
            .set("PINCONGLOMERATE", turn | if pin.hidden { 4 } else { 0 } | 0x18)
            .set("NAME", &pin.name)
            .set("DESIGNATOR", &pin.number)
            .mm("PINLENGTH", pin.length)
            .at("LOCATION.X", "LOCATION.Y", pin.at);
        add(&r);
    }
    let mut d = Rec::new(34);
    d.owner(0).set("TEXT", format!("{}?", s.prefix)).set("NAME", "Designator");
    add(&d);
    for (i, name) in s.footprints.iter().enumerate() {
        let mut m = Rec::new(45);
        m.owner(0).set("MODELNAME", name).set("MODELTYPE", "PCBLIB").set("ISCURRENT", if i == 0 { "T" } else { "F" });
        add(&m);
    }
    out
}

/// A SchLib holding these symbols.
pub fn schlib(symbols: &[Symbol]) -> Result<Vec<u8>, String> {
    let err = |e: std::io::Error| format!("couldn't write the library: {e}");
    let mut file = cfb::CompoundFile::create(Cursor::new(Vec::new())).map_err(err)?;
    let names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
    let storages = storage_names(&names);
    let mut header = Rec::default();
    header.set("HEADER", "Protel for Windows - Schematic Library Editor Binary File Version 5.0").set("WEIGHT", symbols.len());
    for (i, s) in symbols.iter().enumerate() {
        header.set(&format!("LIBREF{i}"), &s.name).set(&format!("COMPDESCR{i}"), &s.description);
    }
    let mut hb = Out::default();
    hb.block(&param_text(&header.0));
    file.create_stream("/FileHeader").map_err(err)?.write_all(&hb.0).map_err(err)?;
    // Which storage holds which symbol.
    let mut keys = Rec::default();
    keys.set("KEYCOUNT", symbols.len());
    for (i, (s, st)) in symbols.iter().zip(&storages).enumerate() {
        keys.set(&format!("LIBREF{i}"), &s.name).set(&format!("SECTIONKEY{i}"), st);
    }
    let mut kb = Out::default();
    kb.block(&param_text(&keys.0));
    file.create_stream("/SectionKeys").map_err(err)?.write_all(&kb.0).map_err(err)?;
    for (s, st) in symbols.iter().zip(&storages) {
        file.create_storage(format!("/{st}")).map_err(err)?;
        file.create_stream(format!("/{st}/Data")).map_err(err)?.write_all(&symbol_records(s)).map_err(err)?;
    }
    Ok(file.into_inner().into_inner())
}

#[cfg(test)]
mod symbol_tests {
    use super::*;
    use crate::pcb::altium::{self, schlib as reader};

    fn sample() -> Symbol {
        let pin = |part, num: &str, name: &str, x: f64, y: f64, angle, kind| reader::Pin { part, number: num.into(), name: name.into(), at: p(x, y), length: 7.62, angle, kind, hidden: false };
        Symbol {
            name: "OPAMP".into(),
            description: "Dual op amp".into(),
            prefix: "U".into(),
            parts: 2,
            pins: vec![
                pin(1, "1", "OUT", 12.7, 0.0, 0.0, PinKind::Output),
                pin(1, "2", "-", -12.7, 2.54, 180.0, PinKind::Input),
                pin(1, "3", "+", -12.7, -2.54, 180.0, PinKind::Input),
                pin(2, "5", "+", -12.7, -2.54, 180.0, PinKind::Input),
                reader::Pin { hidden: true, ..pin(1, "8", "V+", 0.0, 7.62, 90.0, PinKind::Power) },
                pin(1, "4", "V-", 0.0, -7.62, 270.0, PinKind::Power),
            ],
            drawings: vec![
                (0, Drawing::Polygon(vec![p(-5.08, 5.08), p(-5.08, -5.08), p(5.08, 0.0)])),
                (1, Drawing::Line(p(-5.08, 5.08), p(0.0, 5.08))),
                (0, Drawing::Rect(p(-1.0, -1.0), p(1.0, 1.5))),
                (2, Drawing::Arc { c: p(0.0, 0.0), r: 2.54, start: 10.0, end: 200.5 }),
                (0, Drawing::Ellipse { c: p(1.27, 1.27), rx: 1.27, ry: 0.635 }),
                (0, Drawing::Polyline(vec![p(0.0, 0.0), p(1.27, 2.54), p(2.54, 0.0)])),
                (1, Drawing::Text { at: p(-2.54, 0.0), text: "A".into() }),
            ],
            footprints: vec!["SOIC8".into(), "DIP8".into()],
            library: String::new(),
        }
    }

    fn near(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-5
    }

    #[test]
    fn a_symbol_written_reads_back_the_same() {
        let s = sample();
        let bytes = schlib(std::slice::from_ref(&s)).unwrap();
        let got = reader::read(&bytes, "mine").unwrap();
        assert_eq!(got.symbols.len(), 1);
        let g = &got.symbols[0];
        assert_eq!((&g.name, &g.description, &g.prefix, g.parts), (&s.name, &s.description, &s.prefix, s.parts));
        assert_eq!(g.footprints, s.footprints);
        assert_eq!(g.pins.len(), s.pins.len());
        for (a, b) in s.pins.iter().zip(&g.pins) {
            assert_eq!((a.part, &a.number, &a.name, a.kind, a.hidden), (b.part, &b.number, &b.name, b.kind, b.hidden));
            assert!(near(a.at.x, b.at.x) && near(a.at.y, b.at.y) && near(a.length, b.length) && near(a.angle, b.angle), "{a:?} {b:?}");
        }
        assert_eq!(g.drawings.len(), s.drawings.len());
        for (a, b) in s.drawings.iter().zip(&g.drawings) {
            assert_eq!(a.0, b.0, "part of {:?}", a.1);
            assert_eq!(std::mem::discriminant(&a.1), std::mem::discriminant(&b.1));
        }
        let Drawing::Arc { r, start, end, .. } = &g.drawings[3].1 else { panic!() };
        assert!(near(*r, 2.54) && near(*start, 10.0) && near(*end, 200.5));
        let Drawing::Polygon(pts) = &g.drawings[0].1 else { panic!() };
        assert!(near(pts[2].x, 5.08) && near(pts[0].y, 5.08));
        let Drawing::Text { text, .. } = &g.drawings[6].1 else { panic!() };
        assert_eq!(text, "A");
    }

    #[test]
    fn a_fraction_of_a_unit_is_kept() {
        // 1.27 mm is 5 units; 0.127 mm is half of one.
        let mut r = Rec::new(2);
        r.mm("X", -0.127).mm("Y", 1.27).mm("Z", 0.254 * 3.999999);
        assert!(r.0.iter().any(|(k, v)| k == "X" && v == "0") && r.0.iter().any(|(k, v)| k == "X_FRAC" && v == "-50000"), "{:?}", r.0);
        assert!(r.0.iter().any(|(k, v)| k == "Y" && v == "5"));
        assert!(r.0.iter().any(|(k, v)| k == "Z" && v == "4"), "rounds up into the next whole unit: {:?}", r.0);
    }

    #[test]
    fn real_symbols_survive_a_round_trip() {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata/altium/DAL_LIB.SchLib");
        let before = altium::read(&path).unwrap();
        let bytes = schlib(&before.symbols).unwrap();
        let after = reader::read(&bytes, "DAL_LIB").unwrap();
        assert_eq!(before.symbols.len(), after.symbols.len());
        for s in &before.symbols {
            let g = after.symbols.iter().find(|g| g.name == s.name).unwrap_or_else(|| panic!("{} lost", s.name));
            assert_eq!(s.pins.len(), g.pins.len(), "{}", s.name);
            for (a, b) in s.pins.iter().zip(&g.pins) {
                assert!(a.part == b.part && a.number == b.number &&a.at.dist(b.at) < 1e-4 && near(a.length, b.length) && near(a.angle, b.angle) && a.kind == b.kind && a.name == b.name, "{}: {a:?} vs {b:?}", s.name);
            }
            assert_eq!(s.drawings.len(), g.drawings.len(), "{}", s.name);
            assert_eq!(s.footprints, g.footprints, "{}", s.name);
            assert_eq!(s.prefix, g.prefix, "{}", s.name);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pcb::altium::{self, pcblib as reader};

    fn pad_at(num: &str, x: f64, y: f64, w: f64, h: f64, shape: PadShape, drill: f64, rot: f64) -> Pad {
        Pad { num: num.into(), at: p(x, y), size: p(w, h), rot, shape, drill, net: 0 }
    }

    fn sample() -> FootprintDef {
        FootprintDef {
            name: "TEST_DIP4".into(),
            category: "mine".into(),
            description: "A small test part".into(),
            prefix: "U".into(),
            pads: vec![
                pad_at("1", -3.81, -1.27, 1.6, 1.6, PadShape::Rect, 0.9, 0.0),
                pad_at("2", -3.81, 1.27, 1.6, 1.6, PadShape::Circle, 0.9, 0.0),
                pad_at("3", 3.81, 1.27, 1.2, 2.0, PadShape::Oval, 0.0, 90.0),
                pad_at("4", 3.81, -1.27, 1.2, 2.0, PadShape::RoundRect(0.25), 0.0, 0.0),
                pad_at("MH", 0.0, 0.0, 2.2, 2.2, PadShape::Circle, 2.2, 0.0),
            ],
            graphics: vec![
                Graphic::Line { a: p(-5.0, -3.0), b: p(5.0, -3.0), w: 0.15, layer: Layer::Silk(Side::Top) },
                Graphic::Line { a: p(-5.0, 3.0), b: p(5.0, 3.0), w: 0.1, layer: Layer::Court(Side::Top) },
                Graphic::Line { a: p(-5.0, 3.2), b: p(5.0, 3.2), w: 0.05, layer: Layer::Fab(Side::Top) },
                Graphic::Circle { c: p(-4.5, -2.5), r: 0.3, w: 0.1, layer: Layer::Silk(Side::Top), fill: false },
                Graphic::Circle { c: p(4.5, 2.5), r: 0.4, w: 0.0, layer: Layer::Silk(Side::Top), fill: true },
            ],
            label_at: p(0.0, 4.0),
            keepouts: vec![Keepout { id: 0, layer: None, pts: vec![p(-1.0, -1.0), p(1.0, -1.0), p(1.0, 1.0), p(-1.0, 1.0)], restrict: Restrict::pours_only() }],
            source: String::new(),
        }
    }

    fn near(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-5
    }

    #[test]
    fn a_footprint_written_reads_back_the_same() {
        let f = sample();
        let bytes = pcblib(std::slice::from_ref(&f)).unwrap();
        let got = reader::read(&bytes, "mine").unwrap();
        assert_eq!(got.footprints.len(), 1);
        let g = &got.footprints[0];
        assert_eq!(g.name, f.name);
        assert_eq!(g.description, f.description);
        assert_eq!(g.pads.len(), f.pads.len());
        for (a, b) in f.pads.iter().zip(&g.pads) {
            assert_eq!(a.num, b.num);
            assert!(near(a.at.x, b.at.x) && near(a.at.y, b.at.y), "{a:?} {b:?}");
            assert!(near(a.size.x, b.size.x) && near(a.size.y, b.size.y), "{a:?} {b:?}");
            assert!(near(a.drill, b.drill) && near(a.rot, b.rot), "{a:?} {b:?}");
            assert_eq!(a.shape, b.shape, "pad {}", a.num);
        }
        // Lines on each layer come back where they were.
        for want in f.graphics.iter().filter(|g| matches!(g, Graphic::Line { .. })) {
            let Graphic::Line { a, b, layer, .. } = want else { unreachable!() };
            assert!(
                g.graphics.iter().any(|h| matches!(h, Graphic::Line { a: a2, b: b2, layer: l2, .. } if l2 == layer && a2.dist(*a) < 1e-4 && b2.dist(*b) < 1e-4)),
                "{want:?} lost"
            );
        }
        assert!(g.graphics.iter().any(|h| matches!(h, Graphic::Circle { fill: false, r, .. } if near(*r, 0.3))));
        assert!(g.graphics.iter().any(|h| matches!(h, Graphic::Circle { fill: true, r, .. } if (r - 0.4).abs() < 0.01)));
        assert_eq!(g.keepouts.len(), 1);
        assert_eq!(g.keepouts[0].layer, None);
        assert_eq!(g.keepouts[0].pts.len(), 4);
    }

    #[test]
    fn many_footprints_and_awkward_names_are_kept_apart() {
        let mut a = sample();
        a.name = "A/very:long name that goes past thirty-one letters, surely".into();
        let mut b = a.clone();
        b.name = "A/very:long name that goes past thirty-one letters, again".into();
        let c = sample();
        let bytes = pcblib(&[a.clone(), b.clone(), c.clone()]).unwrap();
        let got = reader::read(&bytes, "x").unwrap();
        let mut names: Vec<&str> = got.footprints.iter().map(|f| f.name.as_str()).collect();
        names.sort();
        let mut want = vec![a.name.as_str(), b.name.as_str(), c.name.as_str()];
        want.sort();
        assert_eq!(names, want);
    }

    #[test]
    fn a_real_library_survives_a_round_trip() {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata/altium/DAL_LIB.PcbLib");
        let before = altium::read(&path).unwrap();
        let bytes = pcblib(&before.footprints).unwrap();
        let after = reader::read(&bytes, "DAL_LIB").unwrap();
        assert_eq!(before.footprints.len(), after.footprints.len());
        for f in &before.footprints {
            let g = after.footprints.iter().find(|g| g.name == f.name).unwrap_or_else(|| panic!("{} lost", f.name));
            assert_eq!(f.pads.len(), g.pads.len(), "{}", f.name);
            for (a, b) in f.pads.iter().zip(&g.pads) {
                assert!(a.num == b.num && a.at.dist(b.at) < 1e-4 && a.size.dist(b.size) < 1e-4 && near(a.drill, b.drill), "{}: {a:?} vs {b:?}", f.name);
                assert!(near(a.rot.rem_euclid(360.0), b.rot.rem_euclid(360.0)), "{}: {a:?} vs {b:?}", f.name);
                // A shape that was an octagon was read as a rounded rectangle; either way it stays one kind.
                assert_eq!(std::mem::discriminant(&a.shape), std::mem::discriminant(&b.shape), "{}: pad {}", f.name, a.num);
            }
            assert_eq!(f.keepouts.len(), g.keepouts.len(), "{}", f.name);
            let lines = |d: &FootprintDef| d.graphics.iter().filter(|x| matches!(x, Graphic::Line { .. })).count();
            assert_eq!(lines(f), lines(g), "{}", f.name);
        }
    }
}
