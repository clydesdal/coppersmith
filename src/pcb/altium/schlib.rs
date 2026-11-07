//! Symbols from an Altium `.SchLib`.
//!
//! Each symbol is a storage with a `Data` stream of records. A record is a
//! four-byte header — length in the low three bytes, kind in the top one —
//! then either text (`|RECORD=13|LOCATION.X=…|`, kind 0) or a binary pin
//! (kind 1). Coordinates are in Altium's schematic unit of 10 mil, with
//! `_FRAC` parts in 1/100 000 of that; here they're kept in millimetres.
//!
//! Coppersmith has no schematic editor yet: these are read so they're ready
//! for one, and so a library's symbols can be checked against its footprints.

// Read now, used once there's a schematic editor.
#![allow(dead_code)]

use super::{Imported, Reader, open, params, stream};
use crate::pcb::geom::*;
use std::collections::HashMap;

/// One schematic unit, 10 mil, in millimetres.
const SCH: f64 = 0.254;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PinKind {
    Input,
    InOut,
    Output,
    OpenCollector,
    Passive,
    HiZ,
    OpenEmitter,
    Power,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Pin {
    /// Which part of a many-part symbol it belongs to (from 1).
    pub part: u32,
    pub number: String,
    pub name: String,
    /// The end that's joined to a wire.
    pub at: P,
    pub length: f64,
    /// Which way the pin points out of the body: 0 right, 90 up, 180 left, 270 down.
    pub angle: f64,
    pub kind: PinKind,
    pub hidden: bool,
    /// Whether Altium draws its name, and its number, beside it.
    pub name_shown: bool,
    pub number_shown: bool,
}

impl Pin {
    /// Where wires join it: `at` is where it meets the body, and it reaches
    /// out from there by its length in the way it points.
    pub fn tip(&self) -> P {
        let t = self.angle.to_radians();
        self.at.add(p(t.cos(), t.sin()).scale(self.length)).rounded()
    }
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Drawing {
    Line(P, P),
    Rect(P, P),
    Polyline(Vec<P>),
    Polygon(Vec<P>),
    Arc { c: P, r: f64, start: f64, end: f64 },
    Ellipse { c: P, rx: f64, ry: f64 },
    Text { at: P, text: String },
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Symbol {
    pub name: String,
    pub description: String,
    /// The letter its parts are named with, like U from "U?".
    pub prefix: String,
    pub parts: u32,
    pub pins: Vec<Pin>,
    /// Each drawing, with the part it belongs to (0 for all of them).
    pub drawings: Vec<(u32, Drawing)>,
    /// Footprints it's linked to, the first being the one in use.
    pub footprints: Vec<String>,
    pub library: String,
}

pub fn read(bytes: &[u8], lib: &str) -> Result<Imported, String> {
    let mut file = open(bytes)?;
    let storages: Vec<String> = file.read_root_storage().filter(|e| e.is_storage()).map(|e| e.name().to_string()).collect();
    let mut out = Imported::default();
    for storage in storages {
        let Ok(data) = stream(&mut file, &format!("/{storage}/Data")) else { continue };
        match symbol(&data, lib) {
            Ok(Some(s)) => out.symbols.push(s),
            Ok(None) => {}
            Err(e) => out.notes.push(format!("{storage}: couldn't be read ({e}); left out")),
        }
    }
    Ok(out)
}

/// A coordinate from `KEY` and `KEY_FRAC`, in millimetres.
fn coord(p: &HashMap<String, String>, key: &str) -> f64 {
    let whole: f64 = p.get(key).and_then(|v| v.parse().ok()).unwrap_or(0.0);
    let frac: f64 = p.get(&format!("{key}_FRAC")).and_then(|v| v.parse().ok()).unwrap_or(0.0);
    nm((whole + frac / 100_000.0) * SCH)
}

fn point(p: &HashMap<String, String>, x: &str, y: &str) -> P {
    self::p(coord(p, x), coord(p, y))
}

fn symbol(data: &[u8], lib: &str) -> Result<Option<Symbol>, String> {
    let mut r = Reader::new(data);
    let mut sym: Option<Symbol> = None;
    let mut pins = Vec::new();
    let mut drawings = Vec::new();
    let mut footprints = Vec::new();
    let mut prefix = String::new();
    while r.left() >= 4 {
        let head = r.u32()?;
        let (len, kind) = ((head & 0x00ff_ffff) as usize, head >> 24);
        let body = r.bytes(len)?;
        if kind == 1 {
            pins.push(binary_pin(body)?);
            continue;
        }
        let p = params(body);
        let part: u32 = p.get("OWNERPARTID").and_then(|v| v.parse().ok()).unwrap_or(0).max(0);
        let record: u32 = p.get("RECORD").and_then(|v| v.parse().ok()).unwrap_or(0);
        match record {
            1 => {
                sym = Some(Symbol {
                    name: p.get("LIBREFERENCE").cloned().unwrap_or_default(),
                    description: p.get("COMPONENTDESCRIPTION").cloned().unwrap_or_default(),
                    prefix: String::new(),
                    parts: p.get("PARTCOUNT").and_then(|v| v.parse::<u32>().ok()).unwrap_or(2).saturating_sub(1).max(1),
                    pins: Vec::new(),
                    drawings: Vec::new(),
                    footprints: Vec::new(),
                    library: lib.to_string(),
                })
            }
            2 => pins.push(text_pin(&p, part)),
            13 => drawings.push((part, Drawing::Line(point(&p, "LOCATION.X", "LOCATION.Y"), point(&p, "CORNER.X", "CORNER.Y")))),
            14 | 10 => drawings.push((part, Drawing::Rect(point(&p, "LOCATION.X", "LOCATION.Y"), point(&p, "CORNER.X", "CORNER.Y")))),
            6 | 7 => {
                let n: usize = p.get("LOCATIONCOUNT").and_then(|v| v.parse().ok()).unwrap_or(0);
                let pts: Vec<P> = (1..=n).map(|i| point(&p, &format!("X{i}"), &format!("Y{i}"))).collect();
                drawings.push((part, if record == 6 { Drawing::Polyline(pts) } else { Drawing::Polygon(pts) }));
            }
            12 => {
                let angle = |k: &str| p.get(k).and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0);
                drawings.push((
                    part,
                    Drawing::Arc {
                        c: point(&p, "LOCATION.X", "LOCATION.Y"),
                        r: coord(&p, "RADIUS"),
                        start: angle("STARTANGLE"),
                        end: if p.contains_key("ENDANGLE") { angle("ENDANGLE") } else { 360.0 },
                    },
                ));
            }
            8 => {
                let rx = coord(&p, "RADIUS");
                let ry = if p.contains_key("SECONDARYRADIUS") { coord(&p, "SECONDARYRADIUS") } else { rx };
                drawings.push((part, Drawing::Ellipse { c: point(&p, "LOCATION.X", "LOCATION.Y"), rx, ry }));
            }
            4 => {
                if let Some(text) = p.get("TEXT") {
                    drawings.push((part, Drawing::Text { at: point(&p, "LOCATION.X", "LOCATION.Y"), text: text.clone() }));
                }
            }
            34 => {
                // The designator, like "U?": its letters are the prefix.
                if let Some(t) = p.get("TEXT") {
                    prefix = t.chars().take_while(|c| c.is_alphabetic()).collect();
                }
            }
            45 => {
                // A model link: footprints are the PCBLIB ones.
                if p.get("MODELTYPE").is_some_and(|t| t.eq_ignore_ascii_case("PCBLIB"))
                    && let Some(name) = p.get("MODELNAME")
                {
                    if p.get("ISCURRENT").is_some_and(|v| v == "T") {
                        footprints.insert(0, name.clone());
                    } else {
                        footprints.push(name.clone());
                    }
                }
            }
            _ => {}
        }
    }
    Ok(sym.map(|mut s| {
        s.pins = pins;
        s.drawings = drawings;
        s.footprints = footprints;
        s.prefix = if prefix.is_empty() { "U".into() } else { prefix };
        s
    }))
}

fn pin_kind(k: u8) -> PinKind {
    match k {
        0 => PinKind::Input,
        1 => PinKind::InOut,
        2 => PinKind::Output,
        3 => PinKind::OpenCollector,
        5 => PinKind::HiZ,
        6 => PinKind::OpenEmitter,
        7 => PinKind::Power,
        _ => PinKind::Passive,
    }
}

/// The binary pin record of newer files.
pub(super) fn binary_pin(b: &[u8]) -> Result<Pin, String> {
    let mut r = Reader::new(b);
    r.u32()?; // record kind, 2
    r.skip(1)?;
    let part = r.i16()?.max(0) as u32;
    r.skip(1 + 4)?; // display mode, the four pin symbol decorations
    r.pascal()?; // description
    r.skip(1)?;
    let kind = pin_kind(r.u8()?);
    let flags = r.u8()?;
    let length = r.i16()? as f64 * SCH;
    let at = p(r.i16()? as f64 * SCH, r.i16()? as f64 * SCH).rounded();
    r.u32()?; // colour
    let name = r.pascal()?;
    let number = r.pascal()?;
    Ok(Pin { part, number, name, at, length: nm(length), angle: (flags & 0x03) as f64 * 90.0, kind, hidden: flags & 0x04 != 0, name_shown: flags & 0x08 != 0, number_shown: flags & 0x10 != 0 })
}

/// The text pin record of older files.
pub(super) fn text_pin(p: &HashMap<String, String>, part: u32) -> Pin {
    let flags: u32 = p.get("PINCONGLOMERATE").and_then(|v| v.parse().ok()).unwrap_or(0);
    let kind: u8 = p.get("ELECTRICAL").and_then(|v| v.parse().ok()).unwrap_or(4);
    Pin {
        part,
        number: p.get("DESIGNATOR").cloned().unwrap_or_default(),
        name: p.get("NAME").cloned().unwrap_or_default(),
        at: point(p, "LOCATION.X", "LOCATION.Y"),
        length: coord(p, "PINLENGTH"),
        angle: (flags & 0x03) as f64 * 90.0,
        kind: pin_kind(kind),
        hidden: flags & 0x04 != 0,
        name_shown: flags & 0x08 != 0,
        number_shown: flags & 0x10 != 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lib(name: &str) -> Imported {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata/altium").join(name);
        read(&std::fs::read(path).unwrap(), name).unwrap()
    }

    #[test]
    fn symbols_come_with_pins_and_footprints() {
        let got = lib("DAL_LIB.SchLib");
        assert!(got.symbols.len() >= 10, "{}", got.symbols.len());
        let conn = got.symbols.iter().find(|s| s.name == "10137238").expect("10137238");
        assert!(conn.description.contains("Connector"));
        // Its first pins, 10 mil units: pin 1 at (0, -30) pointing left, 30 long.
        let p1 = conn.pins.iter().find(|p| p.number == "1").unwrap();
        assert!((p1.at.y + 30.0 * SCH).abs() < 1e-6 && p1.at.x.abs() < 1e-9, "{:?}", p1.at);
        assert!((p1.length - 30.0 * SCH).abs() < 1e-6);
        assert_eq!(p1.angle, 180.0);
        assert_eq!(p1.kind, PinKind::Passive);
        assert!(!conn.drawings.is_empty());
        assert!(got.symbols.iter().any(|s| !s.footprints.is_empty()), "some symbols link to footprints");
    }
}
