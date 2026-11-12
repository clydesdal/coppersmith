//! Reading and writing an Altium `.SchDoc` as an editable `Schematic`.
//!
//! A sheet's `FileHeader` stream is a run of records, as in a SchLib: a header,
//! then numbered records. A part is a record 1 whose children (its pins,
//! drawings, designator, value, footprint link) say `OWNERINDEX` of it; its
//! children are stored where they lie on the sheet, with the part's
//! `ORIENTATION` and `ISMIRRORED` saying how the symbol was turned. Reading
//! turns the children back to the symbol's own coordinates, so a part can be
//! turned and moved again. Wires are record 27, junctions 29, net labels 25,
//! power ports 17, ports 18.
//!
//! Written files are read back by this reader in tests, and the real test
//! sheets are read and their netlists compared with the older reader's. They
//! haven't been opened in Altium or Circuit Studio.

use super::schlib::{Drawing, Pin, PinKind, Symbol, binary_pin, text_pin};
use super::write::{Out, Rec, drawing_rec, param_text};
use super::{Reader, open, params, stream};
use crate::pcb::geom::*;
use crate::pcb::schematic::{Label, LabelKind, Paper, Placed, PortIo, PowerStyle, Schematic, Wire, transform, transform_angle};
use std::collections::HashMap;
use std::io::Write;

const SCH: f64 = 0.254;

fn coord(p: &HashMap<String, String>, key: &str) -> f64 {
    let whole: f64 = p.get(key).and_then(|v| v.parse().ok()).unwrap_or(0.0);
    let frac: f64 = p.get(&format!("{key}_FRAC")).and_then(|v| v.parse().ok()).unwrap_or(0.0);
    nm((whole + frac / 100_000.0) * SCH)
}

fn point(p: &HashMap<String, String>, x: &str, y: &str) -> P {
    self::p(coord(p, x), coord(p, y))
}

/// A sheet point back in the symbol's own coordinates.
fn inverse(s: P, at: P, turns: u8, mirror: bool) -> P {
    let d = s.sub(at);
    let q = match turns & 3 {
        0 => d,
        1 => p(d.y, -d.x),
        2 => p(-d.x, -d.y),
        _ => p(-d.y, d.x),
    };
    (if mirror { p(-q.x, q.y) } else { q }).rounded()
}

fn inverse_angle(a: f64, turns: u8, mirror: bool) -> f64 {
    let a = a - 90.0 * (turns & 3) as f64;
    (if mirror { 180.0 - a } else { a }).rem_euclid(360.0)
}

/// A drawing moved from the symbol to the sheet (`forward`) or back.
fn move_drawing(d: &Drawing, at: P, turns: u8, mirror: bool, forward: bool) -> Drawing {
    let pt = |q: P| if forward { transform(q, at, turns, mirror) } else { inverse(q, at, turns, mirror) };
    let pts = |v: &[P]| v.iter().map(|q| pt(*q)).collect::<Vec<_>>();
    match d {
        Drawing::Line(a, b) => Drawing::Line(pt(*a), pt(*b)),
        Drawing::Rect(a, b) => Drawing::Rect(pt(*a), pt(*b)),
        Drawing::Polyline(v) => Drawing::Polyline(pts(v)),
        Drawing::Polygon(v) => Drawing::Polygon(pts(v)),
        Drawing::Arc { c, r, start, end } => {
            let (s, e) = if forward {
                let (s, e) = if mirror { (180.0 - end, 180.0 - start) } else { (*start, *end) };
                (s + 90.0 * turns as f64, e + 90.0 * turns as f64)
            } else {
                let (s, e) = (start - 90.0 * turns as f64, end - 90.0 * turns as f64);
                if mirror { (180.0 - e, 180.0 - s) } else { (s, e) }
            };
            Drawing::Arc { c: pt(*c), r: *r, start: s.rem_euclid(360.0), end: e.rem_euclid(360.0) }
        }
        Drawing::Ellipse { c, rx, ry } => {
            let (rx, ry) = if turns & 1 == 1 { (*ry, *rx) } else { (*rx, *ry) };
            Drawing::Ellipse { c: pt(*c), rx, ry }
        }
        Drawing::Text { at: a, text } => Drawing::Text { at: pt(*a), text: text.clone() },
    }
}

enum Item {
    Text(HashMap<String, String>),
    Pin(Pin),
}

/// Reads a sheet. The notes say what wasn't taken.
pub fn read(bytes: &[u8], name: &str) -> Result<(Schematic, Vec<String>), String> {
    let mut file = open(bytes)?;
    let data = stream(&mut file, "/FileHeader")?;
    let mut r = Reader::new(&data);
    let mut recs: Vec<Item> = Vec::new();
    let mut first = true;
    while r.left() >= 4 {
        let head = r.u32()?;
        let (len, kind) = ((head & 0x00ff_ffff) as usize, head >> 24);
        let body = r.bytes(len)?;
        if first {
            first = false;
            continue;
        }
        recs.push(if kind == 1 { Item::Pin(binary_pin(body)?) } else { Item::Text(params(body)) });
    }
    let text = |i: usize| match &recs[i] {
        Item::Text(p) => Some(p),
        Item::Pin(_) => None,
    };
    let owner = |i: usize| -> i64 { text(i).and_then(|p| p.get("OWNERINDEX")).and_then(|v| v.parse().ok()).unwrap_or(-1) };
    let record = |i: usize| -> u32 { text(i).and_then(|p| p.get("RECORD")).and_then(|v| v.parse().ok()).unwrap_or(0) };

    // Parts, with the index of the record that makes each.
    struct Draft {
        index: usize,
        at: P,
        turns: u8,
        mirror: bool,
        part: u32,
        symbol: Symbol,
        designator: String,
        value: String,
        footprint: String,
        /// Children as they lie on the sheet.
        pins: Vec<Pin>,
        drawings: Vec<(u32, Drawing)>,
    }
    let mut drafts: Vec<Draft> = Vec::new();
    for i in 0..recs.len() {
        if record(i) != 1 {
            continue;
        }
        let p = text(i).unwrap();
        let parts = p.get("PARTCOUNT").and_then(|v| v.parse::<u32>().ok()).unwrap_or(2).saturating_sub(1).max(1);
        drafts.push(Draft {
            index: i,
            at: point(p, "LOCATION.X", "LOCATION.Y"),
            turns: (p.get("ORIENTATION").and_then(|v| v.parse::<u8>().ok()).unwrap_or(0)) & 3,
            mirror: p.get("ISMIRRORED").is_some_and(|v| v == "T"),
            part: p.get("CURRENTPARTID").and_then(|v| v.parse().ok()).unwrap_or(1),
            symbol: Symbol {
                name: p.get("LIBREFERENCE").or(p.get("DESIGNITEMID")).cloned().unwrap_or_default(),
                description: p.get("COMPONENTDESCRIPTION").cloned().unwrap_or_default(),
                prefix: "U".into(),
                parts,
                pins: Vec::new(),
                drawings: Vec::new(),
                footprints: Vec::new(),
                library: p.get("SOURCELIBRARYNAME").cloned().unwrap_or_default(),
            },
            designator: String::new(),
            value: String::new(),
            footprint: String::new(),
            pins: Vec::new(),
            drawings: Vec::new(),
        });
    }
    let by_index: HashMap<usize, usize> = drafts.iter().enumerate().map(|(k, d)| (d.index, k)).collect();
    // A record's part: its owner, or its owner's owner (footprint links sit
    // under an implementation list).
    let part_for = |i: usize| -> Option<usize> {
        let mut o = owner(i);
        for _ in 0..3 {
            if o < 0 {
                return None;
            }
            if let Some(k) = by_index.get(&(o as usize)) {
                return Some(*k);
            }
            o = owner(o as usize);
        }
        None
    };
    let mut out = Schematic::default();
    out.name = name.to_string();
    let mut notes = Vec::new();
    let mut last_part: Option<usize> = None;
    let mut skipped = 0usize;
    for i in 0..recs.len() {
        if let Some(k) = by_index.get(&i) {
            last_part = Some(*k);
        }
        let p = match &recs[i] {
            Item::Pin(pin) => {
                // A binary pin belongs to the part before it.
                if let Some(k) = last_part {
                    drafts[k].pins.push(pin.clone());
                }
                continue;
            }
            Item::Text(p) => p,
        };
        let part_of_record = || p.get("OWNERPARTID").and_then(|v| v.parse::<i64>().ok()).unwrap_or(0).max(0) as u32;
        match record(i) {
            2 => {
                if let Some(k) = part_for(i) {
                    drafts[k].pins.push(text_pin(p, part_of_record()));
                }
            }
            13 | 14 | 10 | 6 | 7 | 12 | 8 | 4 if part_for(i).is_some() => {
                let k = part_for(i).unwrap();
                let part = part_of_record();
                let d = match record(i) {
                    13 => Some(Drawing::Line(point(p, "LOCATION.X", "LOCATION.Y"), point(p, "CORNER.X", "CORNER.Y"))),
                    14 | 10 => Some(Drawing::Rect(point(p, "LOCATION.X", "LOCATION.Y"), point(p, "CORNER.X", "CORNER.Y"))),
                    6 | 7 => {
                        let n: usize = p.get("LOCATIONCOUNT").and_then(|v| v.parse().ok()).unwrap_or(0);
                        let pts: Vec<P> = (1..=n).map(|j| point(p, &format!("X{j}"), &format!("Y{j}"))).collect();
                        Some(if record(i) == 6 { Drawing::Polyline(pts) } else { Drawing::Polygon(pts) })
                    }
                    12 => {
                        let angle = |key: &str| p.get(key).and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0);
                        Some(Drawing::Arc { c: point(p, "LOCATION.X", "LOCATION.Y"), r: coord(p, "RADIUS"), start: angle("STARTANGLE"), end: if p.contains_key("ENDANGLE") { angle("ENDANGLE") } else { 360.0 } })
                    }
                    8 => {
                        let rx = coord(p, "RADIUS");
                        let ry = if p.contains_key("SECONDARYRADIUS") { coord(p, "SECONDARYRADIUS") } else { rx };
                        Some(Drawing::Ellipse { c: point(p, "LOCATION.X", "LOCATION.Y"), rx, ry })
                    }
                    _ => p.get("TEXT").map(|t| Drawing::Text { at: point(p, "LOCATION.X", "LOCATION.Y"), text: t.clone() }),
                };
                if let Some(d) = d {
                    drafts[k].drawings.push((part, d));
                }
            }
            34 => {
                if let (Some(k), Some(t)) = (part_for(i), p.get("TEXT")) {
                    drafts[k].designator = t.clone();
                }
            }
            41 => {
                if let (Some(k), Some(t)) = (part_for(i), p.get("TEXT"))
                    && p.get("NAME").is_some_and(|n| n.eq_ignore_ascii_case("Comment"))
                {
                    drafts[k].value = t.clone();
                }
            }
            45 => {
                if p.get("MODELTYPE").is_some_and(|t| t.eq_ignore_ascii_case("PCBLIB"))
                    && let (Some(k), Some(m)) = (part_for(i), p.get("MODELNAME"))
                {
                    if p.get("ISCURRENT").is_some_and(|v| v == "T") || drafts[k].footprint.is_empty() {
                        drafts[k].footprint = m.clone();
                    }
                    drafts[k].symbol.footprints.push(m.clone());
                }
            }
            27 => {
                let n: usize = p.get("LOCATIONCOUNT").and_then(|v| v.parse().ok()).unwrap_or(0);
                let pts: Vec<P> = (1..=n).map(|j| point(p, &format!("X{j}"), &format!("Y{j}"))).collect();
                if pts.len() >= 2 {
                    let id = out.new_id();
                    out.wires.push(Wire { id, pts });
                }
            }
            25 | 17 | 18 => {
                let (text_key, kind) = match record(i) {
                    25 => ("TEXT", LabelKind::Net),
                    17 => ("TEXT", LabelKind::Power),
                    _ => ("NAME", LabelKind::Port),
                };
                if let Some(t) = p.get(text_key) {
                    let id = out.new_id();
                    // A power port's ORIENTATION: 0 right, 1 up, 2 left, 3 down; ours count from up.
                    let turns = if kind == LabelKind::Power { (p.get("ORIENTATION").and_then(|v| v.parse::<u8>().ok()).unwrap_or(1).wrapping_sub(1)) & 3 } else { 0 };
                    let style = if kind == LabelKind::Power { PowerStyle::from_altium(p.get("STYLE").and_then(|v| v.parse().ok()).unwrap_or(2)) } else { PowerStyle::default() };
                    let io = if kind == LabelKind::Port { PortIo::from_altium(p.get("IOTYPE").and_then(|v| v.parse().ok()).unwrap_or(3)) } else { PortIo::default() };
                    out.labels.push(Label { id, at: point(p, "LOCATION.X", "LOCATION.Y"), text: t.clone(), kind, turns, style, io });
                }
            }
            // The sheet: its paper.
            31 => {
                let custom = p.get("USECUSTOMSHEET").is_some_and(|v| v == "T");
                out.paper = if custom && p.contains_key("CUSTOMX") {
                    Paper::Custom(coord(p, "CUSTOMX").max(10.0), coord(p, "CUSTOMY").max(10.0))
                } else {
                    Paper::from_style(p.get("SHEETSTYLE").and_then(|v| v.parse().ok()).unwrap_or(0))
                };
            }
            // Parts' own records, junctions (worked out), notes and such.
            1 | 29 | 44 | 46 | 47 | 48 => {}
            _ => skipped += 1,
        }
    }
    for d in drafts {
        let mut symbol = d.symbol;
        symbol.pins = d
            .pins
            .iter()
            .map(|pin| Pin { at: inverse(pin.at, d.at, d.turns, d.mirror), angle: inverse_angle(pin.angle, d.turns, d.mirror), ..pin.clone() })
            .collect();
        symbol.drawings = d.drawings.iter().map(|(part, dr)| (*part, move_drawing(dr, d.at, d.turns, d.mirror, false))).collect();
        symbol.prefix = {
            let letters: String = d.designator.chars().take_while(|c| c.is_alphabetic()).collect();
            if letters.is_empty() { "U".into() } else { letters }
        };
        let id = out.new_id();
        out.parts.push(Placed { id, symbol, designator: d.designator, value: d.value, footprint: d.footprint, at: d.at, turns: d.turns, mirror: d.mirror, part: d.part });
    }
    if skipped > 0 {
        notes.push(format!("{skipped} drawing or text records of kinds not taken were left out"));
    }
    out.fix_junctions();
    Ok((out, notes))
}

/// Writes a sheet.
pub fn write(s: &Schematic) -> Result<Vec<u8>, String> {
    let err = |e: std::io::Error| format!("couldn't write the sheet: {e}");
    let mut recs: Vec<Vec<u8>> = Vec::new();
    let mut sheet = Rec::new(31);
    sheet.set("FONTIDCOUNT", 1).set("SIZE1", 10).set("FONTNAME1", "Times New Roman").set("USEMBCS", "T").set("ISBOC", "T").set("SHEETSTYLE", s.paper.style()).set("SNAPGRIDON", "T").set("SNAPGRIDSIZE", 10).set("VISIBLEGRIDON", "T").set("VISIBLEGRIDSIZE", 10);
    if let Paper::Custom(w, h) = s.paper {
        sheet.set("USECUSTOMSHEET", "T").mm("CUSTOMX", w).mm("CUSTOMY", h);
    }
    recs.push(sheet.bytes());
    let mut add = |r: &Rec| -> usize {
        recs.push(r.bytes());
        recs.len() - 1
    };
    for part in &s.parts {
        let mut c = Rec::new(1);
        c.set("LIBREFERENCE", &part.symbol.name)
            .set("DESIGNITEMID", &part.symbol.name)
            .set("COMPONENTDESCRIPTION", &part.symbol.description)
            .set("SOURCELIBRARYNAME", &part.symbol.library)
            .set("PARTCOUNT", part.symbol.parts + 1)
            .set("DISPLAYMODECOUNT", 1)
            .set("CURRENTPARTID", part.part)
            .set("ORIENTATION", part.turns & 3)
            .set("ISMIRRORED", if part.mirror { "T" } else { "F" })
            .at("LOCATION.X", "LOCATION.Y", part.at);
        let owner = add(&c);
        let with_owner = |mut r: Rec| {
            r.set("OWNERINDEX", owner);
            r
        };
        for (k, d) in &part.symbol.drawings {
            add(&with_owner(drawing_rec(&move_drawing(d, part.at, part.turns, part.mirror, true), *k)));
        }
        for pin in &part.symbol.pins {
            let on = part.pin_on_sheet(pin);
            let moved = Pin { at: on.at, angle: on.angle, ..pin.clone() };
            add(&with_owner(super::write::pin_rec(&moved)));
        }
        let (bb, _) = (part.bbox(), ());
        let mut des = Rec::new(34);
        des.owner(0).set("TEXT", &part.designator).set("NAME", "Designator").at("LOCATION.X", "LOCATION.Y", p(bb.min.x, bb.max.y + 1.27));
        add(&with_owner(des));
        if !part.value.is_empty() {
            let mut v = Rec::new(41);
            v.owner(0).set("TEXT", &part.value).set("NAME", "Comment").at("LOCATION.X", "LOCATION.Y", p(bb.min.x, bb.min.y - 1.27));
            add(&with_owner(v));
        }
        if !part.footprint.is_empty() {
            let mut list = Rec::new(44);
            list.owner(0);
            let list_at = add(&with_owner(list));
            let mut m = Rec::new(45);
            m.owner(0).set("MODELNAME", &part.footprint).set("MODELTYPE", "PCBLIB").set("ISCURRENT", "T").set("OWNERINDEX", list_at);
            add(&m);
        }
    }
    for w in &s.wires {
        let mut r = Rec::new(27);
        r.set("LINEWIDTH", 1).set("LOCATIONCOUNT", w.pts.len());
        for (i, q) in w.pts.iter().enumerate() {
            r.at(&format!("X{}", i + 1), &format!("Y{}", i + 1), *q);
        }
        add(&r);
    }
    for j in &s.junctions {
        let mut r = Rec::new(29);
        r.at("LOCATION.X", "LOCATION.Y", *j);
        add(&r);
    }
    for l in &s.labels {
        let mut r;
        match l.kind {
            LabelKind::Net => {
                r = Rec::new(25);
                r.set("TEXT", &l.text).set("ORIENTATION", 0);
            }
            LabelKind::Power => {
                r = Rec::new(17);
                r.set("TEXT", &l.text).set("STYLE", l.style.altium()).set("SHOWNETNAME", "T").set("ORIENTATION", (l.turns + 1) & 3);
            }
            LabelKind::Port => {
                r = Rec::new(18);
                let width = (l.text.chars().count() as f64 * 1.0 + 2.0).ceil();
                r.set("NAME", &l.text).set("STYLE", 3).set("IOTYPE", l.io.altium()).set("ALIGNMENT", 2).mm("WIDTH", width * SCH * 10.0 / 10.0);
            }
        }
        r.at("LOCATION.X", "LOCATION.Y", l.at);
        add(&r);
    }
    let mut header = Rec::default();
    header.set("HEADER", "Protel for Windows - Schematic Capture Binary File Version 5.0").set("WEIGHT", recs.len());
    let mut body = Out::default();
    body.0.extend(header.bytes());
    for r in &recs {
        body.0.extend(r);
    }
    let mut file = cfb::CompoundFile::create(std::io::Cursor::new(Vec::new())).map_err(err)?;
    file.create_stream("/FileHeader").map_err(err)?.write_all(&body.0).map_err(err)?;
    let mut storage = Out::default();
    storage.block(&param_text(&[("HEADER", "Icon storage".to_string()), ("WEIGHT", "0".to_string())]));
    file.create_stream("/Storage").map_err(err)?.write_all(&storage.0).map_err(err)?;
    let _ = (PinKind::Passive, transform_angle(0.0, 0, false));
    Ok(file.into_inner().into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pcb::altium::{schdoc, schlib};

    fn symbol() -> Symbol {
        let pin = |part, n: &str, name: &str, x: f64, y: f64, a: f64| Pin { part, number: n.into(), name: name.into(), at: p(x, y), length: 2.54, angle: a, kind: PinKind::Passive, hidden: false, name_shown: true, number_shown: true };
        Symbol {
            name: "OPAMP".into(),
            description: "Dual op amp".into(),
            prefix: "U".into(),
            parts: 2,
            pins: vec![pin(1, "1", "OUT", 7.62, 0.0, 0.0), pin(1, "2", "-", -7.62, 2.54, 180.0), pin(1, "3", "+", -7.62, -2.54, 180.0), pin(2, "5", "+", -7.62, -2.54, 180.0), pin(2, "6", "-", -7.62, 2.54, 180.0)],
            drawings: vec![
                (0, Drawing::Polygon(vec![p(-5.08, 5.08), p(-5.08, -5.08), p(5.08, 0.0)])),
                (1, Drawing::Line(p(-5.08, 5.08), p(0.0, 5.08))),
                (2, Drawing::Arc { c: p(0.0, 0.0), r: 2.54, start: 10.0, end: 200.0 }),
                (0, Drawing::Ellipse { c: p(1.27, 1.27), rx: 1.27, ry: 0.635 }),
                (0, Drawing::Rect(p(-1.0, -1.0), p(1.0, 1.5))),
            ],
            footprints: vec!["SOIC8".into()],
            library: "mine".into(),
        }
    }

    fn sample() -> Schematic {
        let mut s = Schematic::default();
        let a = s.place(&symbol(), p(25.4, 25.4));
        let b = s.place(&symbol(), p(76.2, 25.4));
        s.part_mut(a).unwrap().value = "TLV9002".into();
        s.part_mut(b).unwrap().turns = 1;
        s.part_mut(b).unwrap().mirror = true;
        s.part_mut(b).unwrap().part = 2;
        let tip = |s: &Schematic, id: u32, n: &str| s.part(id).unwrap().pins().into_iter().find(|p| p.number == n).unwrap().tip;
        s.paper = Paper::B;
        let (t1, t2) = (tip(&s, a, "1"), tip(&s, b, "5"));
        s.add_wire(vec![t1, p(t1.x + 5.08, t1.y), p(t2.x, t1.y), t2]).unwrap();
        s.add_label(p(40.64, t1.y), "SIG", LabelKind::Net);
        s.add_label(tip(&s, a, "3"), "GND", LabelKind::Power);
        s.add_label(p(10.16, 50.8), "IN", LabelKind::Port);
        s
    }

    fn pins_on_sheet(s: &Schematic) -> Vec<(String, String, P)> {
        let mut v: Vec<(String, String, P)> = s.parts.iter().flat_map(|p| p.pins().into_iter().map(|x| (p.designator.clone(), x.number, x.tip)).collect::<Vec<_>>()).collect();
        v.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
        v
    }

    #[test]
    fn a_power_ports_style_and_a_ports_direction_survive_a_write_and_read() {
        use crate::pcb::schematic::{PortIo, PowerStyle};
        let mut s = Schematic::default();
        s.add_marker(p(10.16, 10.16), "PE", LabelKind::Power, PowerStyle::Earth, PortIo::default());
        s.add_marker(p(20.32, 10.16), "VIN", LabelKind::Power, PowerStyle::Arrow, PortIo::default());
        s.add_marker(p(30.48, 10.16), "+3V3", LabelKind::Power, PowerStyle::Circle, PortIo::default());
        s.add_marker(p(10.16, 30.48), "CLK", LabelKind::Port, PowerStyle::default(), PortIo::Input);
        s.add_marker(p(20.32, 30.48), "DATA", LabelKind::Port, PowerStyle::default(), PortIo::Output);
        let (back, _) = read(&write(&s).unwrap(), "x").unwrap();
        let find = |name: &str| back.labels.iter().find(|l| l.text == name).unwrap_or_else(|| panic!("no {name}"));
        assert_eq!(find("PE").style, PowerStyle::Earth);
        assert_eq!(find("VIN").style, PowerStyle::Arrow);
        assert_eq!(find("+3V3").style, PowerStyle::Circle);
        assert_eq!(find("CLK").io, PortIo::Input);
        assert_eq!(find("DATA").io, PortIo::Output);
    }

    #[test]
    fn a_pins_hidden_name_and_number_stay_hidden_through_a_write_and_read() {
        let mut sym = symbol();
        sym.pins[0].name_shown = false;
        sym.pins[1].number_shown = false;
        let mut s = Schematic::default();
        s.place(&sym, p(25.4, 25.4));
        let (back, _) = read(&write(&s).unwrap(), "x").unwrap();
        let pins = back.parts[0].pins();
        let one = pins.iter().find(|x| x.number == "1").unwrap();
        assert!(!one.name_shown && one.number_shown, "pin 1 keeps its number but not its name");
        let two = pins.iter().find(|x| x.name == sym.pins[1].name && x.number == sym.pins[1].number).unwrap();
        assert!(two.name_shown && !two.number_shown, "its neighbour keeps its name but not its number");
    }

    #[test]
    fn a_sheet_written_reads_back_the_same_where_it_matters() {
        let s = sample();
        let (back, _) = read(&write(&s).unwrap(), "x").unwrap();
        assert_eq!(back.parts.len(), 2);
        assert_eq!(back.paper, Paper::B, "the paper is kept");
        let mut wide = s.clone();
        wide.paper = Paper::Custom(300.0, 200.0);
        let (custom, _) = read(&write(&wide).unwrap(), "c").unwrap();
        assert!(matches!(custom.paper, Paper::Custom(w, h) if (w - 300.0).abs() < 0.01 && (h - 200.0).abs() < 0.01), "{:?}", custom.paper);
        for (a, b) in s.parts.iter().zip(&back.parts) {
            assert_eq!((&a.designator, &a.value, &a.footprint, a.turns, a.mirror, a.part), (&b.designator, &b.value, &b.footprint, b.turns, b.mirror, b.part));
            assert_eq!(a.at, b.at);
            assert_eq!((&a.symbol.name, a.symbol.parts, &a.symbol.prefix), (&b.symbol.name, b.symbol.parts, &b.symbol.prefix));
            assert_eq!(a.symbol.pins.len(), b.symbol.pins.len());
            for (x, y) in a.symbol.pins.iter().zip(&b.symbol.pins) {
                assert!(x.part == y.part && x.number == y.number && x.at.dist(y.at) < 1e-4 && (x.angle - y.angle).abs() < 1e-6 && (x.length - y.length).abs() < 1e-6, "{x:?} vs {y:?}");
            }
            assert_eq!(a.symbol.drawings.len(), b.symbol.drawings.len());
            for ((pa, da), (pb, db)) in a.symbol.drawings.iter().zip(&b.symbol.drawings) {
                assert_eq!(pa, pb);
                assert_eq!(std::mem::discriminant(da), std::mem::discriminant(db));
            }
        }
        assert_eq!(pins_on_sheet(&s), pins_on_sheet(&back), "pins land where they did on the sheet");
        assert_eq!(s.wires.iter().map(|w| w.pts.clone()).collect::<Vec<_>>(), back.wires.iter().map(|w| w.pts.clone()).collect::<Vec<_>>());
        let labels = |s: &Schematic| s.labels.iter().map(|l| (l.text.clone(), l.at, l.kind, l.turns)).collect::<Vec<_>>();
        assert_eq!(labels(&s), labels(&back));
        assert_eq!(s.netlist(), back.netlist());
        assert_eq!(s.junctions, back.junctions);
    }

    /// Shows a sheet's font table and which records use which font:
    /// COPPERSMITH_SHEET=file cargo test dump_fonts -- --ignored --nocapture
    #[test]
    #[ignore]
    fn dump_fonts() {
        let Some(f) = std::env::var_os("COPPERSMITH_SHEET") else { return };
        let bytes = std::fs::read(f).unwrap();
        let mut file = open(&bytes).unwrap();
        let data = stream(&mut file, "/FileHeader").unwrap();
        let mut r = Reader::new(&data);
        let mut uses: std::collections::BTreeMap<(String, String), usize> = Default::default();
        let mut first = true;
        while r.left() >= 4 {
            let head = r.u32().unwrap();
            let body = r.bytes((head & 0x00ff_ffff) as usize).unwrap();
            if first || head >> 24 == 1 {
                first = false;
                continue;
            }
            let p = params(body);
            let rec = p.get("RECORD").cloned().unwrap_or_default();
            if rec == "31" {
                let mut keys: Vec<_> = p.iter().filter(|(k, _)| k.starts_with("FONT") || k.starts_with("SIZE") || k.starts_with("ROTATION") || k.starts_with("BOLD") || k.starts_with("ITALIC")).collect();
                keys.sort();
                for (k, v) in keys {
                    println!("sheet {k} = {v}");
                }
            }
            if let Some(id) = p.get("FONTID") {
                *uses.entry((rec, id.clone())).or_default() += 1;
            }
        }
        for ((rec, id), n) in uses {
            println!("record {rec} uses font {id}: {n}");
        }
    }

    #[test]
    fn a_turned_and_mirrored_arc_and_ellipse_come_back_the_same() {
        let arc = Drawing::Arc { c: p(1.0, 2.0), r: 2.0, start: 30.0, end: 140.0 };
        for turns in 0..4u8 {
            for mirror in [false, true] {
                let at = p(10.0, 10.0);
                let there = move_drawing(&arc, at, turns, mirror, true);
                let Drawing::Arc { start, end, .. } = &there else { panic!() };
                // A point on the arc, moved with it, is on the moved arc.
                let Drawing::Arc { c, r, start: s0, end: e0 } = &arc else { panic!() };
                let mid = (s0 + e0) / 2.0;
                let on = c.add(p(mid.to_radians().cos(), mid.to_radians().sin()).scale(*r));
                let moved = transform(on, at, turns, mirror);
                let Drawing::Arc { c: c2, r: r2, .. } = &there else { panic!() };
                assert!((moved.dist(*c2) - r2).abs() < 1e-3, "turns {turns} mirror {mirror}");
                let a = (moved.y - c2.y).atan2(moved.x - c2.x).to_degrees().rem_euclid(360.0);
                let span = (end - start).rem_euclid(360.0);
                assert!((a - start).rem_euclid(360.0) <= span + 1e-6, "turns {turns} mirror {mirror}: {a} not in {start}..{end}");
                let back = move_drawing(&there, at, turns, mirror, false);
                let Drawing::Arc { start: bs, end: be, c: bc, .. } = back else { panic!() };
                assert!((bs - s0).abs() < 1e-6 && (be - e0).abs() < 1e-6 && bc.dist(*c) < 1e-4, "turns {turns} mirror {mirror}");
            }
        }
    }

    #[test]
    fn the_real_sheets_read_with_the_same_nets_as_the_older_reader() {
        for f in ["Sheet1.SchDoc", "Sheet2.SchDoc"] {
            let bytes = std::fs::read(altium_file!(f)).unwrap();
            let old = schdoc::read(&bytes, f).unwrap();
            let (new, _) = read(&bytes, f).unwrap();
            assert_eq!(old.parts.len(), new.parts.len(), "{f}");
            assert!(!new.parts.is_empty(), "{f}: no parts");
            // The same pins on the part shown, the same wires.
            assert_eq!(old.parts.iter().map(|p| p.pins.len()).sum::<usize>(), new.parts.iter().map(|p| p.pins().len()).sum::<usize>(), "{f}: pins");
            assert_eq!(old.wires.len(), new.wires.len(), "{f}: wires");
            for (a, b) in old.parts.iter().zip(&new.parts) {
                assert_eq!((&a.designator, &a.footprint), (&b.designator, &b.footprint), "{f}");
            }
            assert_eq!(schdoc::netlist(&[old]), new.netlist(), "{f}: nets differ");
        }
    }

    #[test]
    fn a_real_sheet_survives_being_written_and_read_again() {
        let bytes = std::fs::read(altium_file!("Sheet1.SchDoc")).unwrap();
        let (first, _) = read(&bytes, "Sheet1").unwrap();
        let (again, _) = read(&write(&first).unwrap(), "Sheet1").unwrap();
        assert_eq!(first.parts.len(), again.parts.len());
        assert_eq!(pins_on_sheet(&first), pins_on_sheet(&again));
        assert_eq!(first.netlist(), again.netlist());
        let _ = schlib::PinKind::Passive;
    }
}
