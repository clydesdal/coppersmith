//! Schematic sheets from an Altium `.SchDoc` (Circuit Studio uses the same).
//!
//! A sheet's `FileHeader` stream holds every record in turn, in the same
//! form as a SchLib symbol's (text records, and binary pins). Records are
//! numbered from 0 after the header; `OWNERINDEX` says which record one
//! belongs to — pins, designators, values and footprint links to their part.
//!
//! Coppersmith has no schematic editor yet; sheets are read so their parts
//! and netlist can be checked, and are ready for one.

#![allow(dead_code)]

use super::schlib::{Drawing, Pin, binary_pin, text_pin};
use super::{Reader, open, params, stream};
use crate::pcb::geom::*;
use std::collections::HashMap;

const SCH: f64 = 0.254;

/// A part placed on a sheet.
#[derive(Clone, Debug)]
pub struct Part {
    pub designator: String,
    pub lib_ref: String,
    /// Its Comment (usually the value or part number).
    pub value: String,
    pub footprint: String,
    pub at: P,
    /// Which part of a many-part symbol is shown (from 1).
    pub part: u32,
    /// Pins, where they are on the sheet.
    pub pins: Vec<Pin>,
}

#[derive(Clone, Debug, Default)]
pub struct Sheet {
    pub name: String,
    pub parts: Vec<Part>,
    pub wires: Vec<Vec<P>>,
    pub junctions: Vec<P>,
    /// Net labels, ports and power ports: a name at a point.
    pub names: Vec<(P, String)>,
    pub drawings: Vec<Drawing>,
}

fn coord(p: &HashMap<String, String>, key: &str) -> f64 {
    let whole: f64 = p.get(key).and_then(|v| v.parse().ok()).unwrap_or(0.0);
    let frac: f64 = p.get(&format!("{key}_FRAC")).and_then(|v| v.parse().ok()).unwrap_or(0.0);
    nm((whole + frac / 100_000.0) * SCH)
}

fn point(p: &HashMap<String, String>, x: &str, y: &str) -> P {
    self::p(coord(p, x), coord(p, y))
}

enum Rec {
    Text(HashMap<String, String>),
    Pin(Pin, i64),
}

pub fn read(bytes: &[u8], name: &str) -> Result<Sheet, String> {
    let mut file = open(bytes)?;
    let data = stream(&mut file, "/FileHeader")?;
    let mut r = Reader::new(&data);
    let mut recs: Vec<Rec> = Vec::new();
    let mut first = true;
    while r.left() >= 4 {
        let head = r.u32()?;
        let (len, kind) = ((head & 0x00ff_ffff) as usize, head >> 24);
        let body = r.bytes(len)?;
        if first {
            // The sheet's header comes before the numbered records.
            first = false;
            continue;
        }
        if kind == 1 {
            // A binary pin: its owner is the last part before it.
            let pin = binary_pin(body)?;
            recs.push(Rec::Pin(pin, -1));
        } else {
            recs.push(Rec::Text(params(body)));
        }
    }
    let owner = |i: usize| -> i64 {
        match &recs[i] {
            Rec::Text(p) => p.get("OWNERINDEX").and_then(|v| v.parse().ok()).unwrap_or(-1),
            Rec::Pin(_, o) => *o,
        }
    };
    let mut sheet = Sheet { name: name.to_string(), ..Default::default() };
    // Parts first, so the rest can find their owners.
    let mut part_of: HashMap<usize, usize> = HashMap::new();
    let mut last_part: Option<usize> = None;
    for i in 0..recs.len() {
        match &recs[i] {
            Rec::Text(p) if p.get("RECORD").map(|s| s.as_str()) == Some("1") => {
                part_of.insert(i, sheet.parts.len());
                last_part = Some(sheet.parts.len());
                sheet.parts.push(Part {
                    designator: String::new(),
                    lib_ref: p.get("LIBREFERENCE").or(p.get("DESIGNITEMID")).cloned().unwrap_or_default(),
                    value: String::new(),
                    footprint: String::new(),
                    at: point(p, "LOCATION.X", "LOCATION.Y"),
                    part: p.get("CURRENTPARTID").and_then(|v| v.parse().ok()).unwrap_or(1),
                    pins: Vec::new(),
                });
            }
            Rec::Pin(pin, _) => {
                if let Some(k) = last_part {
                    sheet.parts[k].pins.push(pin.clone());
                }
            }
            _ => {}
        }
    }
    // A record's part: its owner, or its owner's owner (footprint links sit
    // in an implementation list that belongs to the part).
    let part_for = |i: usize| -> Option<usize> {
        let mut o = owner(i);
        for _ in 0..3 {
            if o < 0 {
                return None;
            }
            if let Some(k) = part_of.get(&(o as usize)) {
                return Some(*k);
            }
            o = owner(o as usize);
        }
        None
    };
    for i in 0..recs.len() {
        let Rec::Text(p) = &recs[i] else { continue };
        let record: u32 = p.get("RECORD").and_then(|v| v.parse().ok()).unwrap_or(0);
        match record {
            2 => {
                if let Some(k) = part_for(i) {
                    // Each pin is of the part it says; only the part shown has its pins here.
                    let part = p.get("OWNERPARTID").and_then(|v| v.parse::<i64>().ok()).unwrap_or(0).max(0) as u32;
                    let pin = text_pin(p, part);
                    sheet.parts[k].pins.push(pin);
                }
            }
            34 => {
                if let (Some(k), Some(t)) = (part_for(i), p.get("TEXT")) {
                    sheet.parts[k].designator = t.clone();
                }
            }
            41 => {
                if let (Some(k), Some(t)) = (part_for(i), p.get("TEXT"))
                    && p.get("NAME").is_some_and(|n| n.eq_ignore_ascii_case("Comment"))
                {
                    sheet.parts[k].value = t.clone();
                }
            }
            45 => {
                if p.get("MODELTYPE").is_some_and(|t| t.eq_ignore_ascii_case("PCBLIB"))
                    && let Some(k) = part_for(i)
                    && let Some(m) = p.get("MODELNAME")
                    && (sheet.parts[k].footprint.is_empty() || p.get("ISCURRENT").is_some_and(|v| v == "T"))
                {
                    sheet.parts[k].footprint = m.clone();
                }
            }
            27 => {
                let n: usize = p.get("LOCATIONCOUNT").and_then(|v| v.parse().ok()).unwrap_or(0);
                sheet.wires.push((1..=n).map(|i| point(p, &format!("X{i}"), &format!("Y{i}"))).collect());
            }
            29 => sheet.junctions.push(point(p, "LOCATION.X", "LOCATION.Y")),
            17 | 25 => {
                if let Some(t) = p.get("TEXT") {
                    sheet.names.push((point(p, "LOCATION.X", "LOCATION.Y"), t.clone()));
                }
            }
            18 => {
                // A port: joins to the wire at either end of its box.
                if let Some(t) = p.get("NAME") {
                    let at = point(p, "LOCATION.X", "LOCATION.Y");
                    let w = coord(p, "WIDTH");
                    let style: u32 = p.get("STYLE").and_then(|v| v.parse().ok()).unwrap_or(0);
                    // Styles 0–3 lie along x, 4–7 along y.
                    let other = if style >= 4 { at.add(self::p(0.0, w)) } else { at.add(self::p(w, 0.0)) };
                    sheet.names.push((at, t.clone()));
                    sheet.names.push((other, t.clone()));
                }
            }
            _ => {}
        }
    }
    // Pins of the part shown only: a many-part symbol keeps every part's.
    for part in &mut sheet.parts {
        let shown = part.part;
        part.pins.retain(|p| p.part == 0 || p.part == shown);
    }
    Ok(sheet)
}

/// Which pins join, across sheets: wires, junctions, and names (net labels,
/// ports and power ports with the same name join everywhere). Each net is
/// its name (from a label, or made up from its first pin) and its pins as
/// "U1-3".
pub fn netlist(sheets: &[Sheet]) -> Vec<(String, Vec<String>)> {
    // Points that can join, numbered; union-find over them.
    let mut pts: Vec<P> = Vec::new();
    let mut parent: Vec<usize> = Vec::new();
    let mut pin_at: Vec<(String, usize)> = Vec::new();
    let mut named: Vec<(String, usize)> = Vec::new();
    fn find(parent: &mut [usize], mut i: usize) -> usize {
        while parent[i] != i {
            parent[i] = parent[parent[i]];
            i = parent[i];
        }
        i
    }
    for (si, sheet) in sheets.iter().enumerate() {
        let base = pts.len();
        // Each sheet's points kept apart from other sheets'.
        let off = p(si as f64 * 1.0e6, 0.0);
        let add = |q: P, pts: &mut Vec<P>, parent: &mut Vec<usize>| {
            pts.push(q.add(off));
            parent.push(parent.len());
            pts.len() - 1
        };
        let mut wire_ids: Vec<Vec<usize>> = Vec::new();
        for w in &sheet.wires {
            let ids: Vec<usize> = w.iter().map(|q| add(*q, &mut pts, &mut parent)).collect();
            for k in 1..ids.len() {
                let (a, b) = (find(&mut parent, ids[k - 1]), find(&mut parent, ids[k]));
                parent[a] = b;
            }
            wire_ids.push(ids);
        }
        for part in &sheet.parts {
            for pin in &part.pins {
                let id = add(pin.tip(), &mut pts, &mut parent);
                pin_at.push((format!("{}-{}", part.designator, pin.number), id));
            }
        }
        for (q, name) in &sheet.names {
            let id = add(*q, &mut pts, &mut parent);
            named.push((name.clone(), id));
        }
        for q in &sheet.junctions {
            add(*q, &mut pts, &mut parent);
        }
        // Anything lying on a wire (its ends, a pin's end, a label, a
        // junction, another wire's end) joins it.
        let all: Vec<usize> = (base..pts.len()).collect();
        for (wi, w) in sheet.wires.iter().enumerate() {
            for s in w.windows(2) {
                let (a, b) = (s[0].add(off), s[1].add(off));
                for &i in &all {
                    if dist_to_segment(pts[i], a, b).0 < 1e-3 {
                        let (x, y) = (find(&mut parent, i), find(&mut parent, wire_ids[wi][0]));
                        parent[x] = y;
                    }
                }
            }
        }
        // Pins touching pins, labels on pins.
        for i in base..pts.len() {
            for j in i + 1..pts.len() {
                if pts[i].same(pts[j]) {
                    let (x, y) = (find(&mut parent, i), find(&mut parent, j));
                    parent[x] = y;
                }
            }
        }
    }
    // The same name joins across the whole design.
    let mut first_of: HashMap<String, usize> = HashMap::new();
    for (name, id) in &named {
        match first_of.get(name) {
            Some(&f) => {
                let (x, y) = (find(&mut parent, *id), find(&mut parent, f));
                parent[x] = y;
            }
            None => {
                first_of.insert(name.clone(), *id);
            }
        }
    }
    let mut groups: HashMap<usize, (Option<String>, Vec<String>)> = HashMap::new();
    for (pin, id) in &pin_at {
        let g = find(&mut parent, *id);
        groups.entry(g).or_default().1.push(pin.clone());
    }
    for (name, id) in &named {
        let g = find(&mut parent, *id);
        let e = groups.entry(g).or_default();
        if e.0.as_ref().is_none_or(|n| name < n) {
            e.0 = Some(name.clone());
        }
    }
    let mut out: Vec<(String, Vec<String>)> = groups
        .into_values()
        .filter(|(_, pins)| !pins.is_empty())
        .map(|(name, mut pins)| {
            pins.sort();
            pins.dedup();
            (name.unwrap_or_else(|| format!("Net{}", pins[0])), pins)
        })
        .filter(|(name, pins)| pins.len() > 1 || !name.starts_with("Net"))
        .collect();
    out.sort();
    out
}

