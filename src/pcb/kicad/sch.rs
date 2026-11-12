//! Reading KiCad symbols (`.kicad_sym`, or the `lib_symbols` of a sheet) and sheets (`.kicad_sch`).
//!
//! A symbol's own coordinates run up the page, as Coppersmith's do; a sheet's run
//! down, so its points are flipped. A KiCad pin is given at its outer end and points
//! into the body; here a pin is given at the body and points out.

use super::pcb::{Left, circumcircle};
use super::sexpr::{Sx, parse};
use crate::pcb::altium::schlib::{Drawing, Pin, PinKind, Symbol};
use crate::pcb::geom::*;
use crate::pcb::schematic::{LabelKind, Paper, PortIo, PowerStyle, Schematic};
use std::collections::HashMap;

/// KiCad writes some characters in names as `{slash}` and the like.
fn unescape(s: &str) -> String {
    let mut out = s.to_string();
    for (code, ch) in [("{slash}", "/"), ("{backslash}", "\\"), ("{dblquote}", "\""), ("{quote}", "'"), ("{lt}", "<"), ("{gt}", ">"), ("{colon}", ":"), ("{brace}", "{"), ("{comma}", ","), ("{space}", " "), ("{tab}", "\t")] {
        out = out.replace(code, ch);
    }
    out
}

/// A pin's name as shown: `~` is none, and `~{RESET}` (overlined) is its letters.
fn pin_text(s: &str) -> String {
    let s = unescape(s);
    if s == "~" {
        return String::new();
    }
    let mut out = String::new();
    let mut rest = s.as_str();
    while let Some(i) = rest.find("~{") {
        out.push_str(&rest[..i]);
        match rest[i + 2..].find('}') {
            Some(j) => {
                out.push_str(&rest[i + 2..i + 2 + j]);
                rest = &rest[i + 3 + j..];
            }
            None => {
                rest = &rest[i + 2..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn point(n: &Sx, name: &str) -> Option<P> {
    let c = n.get(name)?;
    Some(p(c.num_arg(0)?, c.num_arg(1)?))
}

fn kind_of(electrical: &str) -> PinKind {
    match electrical {
        "input" => PinKind::Input,
        "output" => PinKind::Output,
        "bidirectional" => PinKind::InOut,
        "tri_state" => PinKind::HiZ,
        "power_in" | "power_out" => PinKind::Power,
        "open_collector" => PinKind::OpenCollector,
        "open_emitter" => PinKind::OpenEmitter,
        _ => PinKind::Passive,
    }
}

fn filled(n: &Sx) -> bool {
    match n.get("fill") {
        Some(f) => !matches!(f.child_str("type"), Some("none")) && !matches!(f.str_arg(0), Some("none")),
        None => false,
    }
}

/// One symbol: `(symbol "Lib:Name" …)` in a library or in a sheet's `lib_symbols`.
/// Returns whether it's a power symbol (a supply or ground that's drawn as a port).
pub fn read_symbol(n: &Sx, left: &mut Left) -> Option<(String, Symbol, bool)> {
    let id = n.str_arg(0)?.to_string();
    let (library, name) = match id.split_once(':') {
        Some((l, nm)) => (l.to_string(), nm.to_string()),
        None => (String::new(), id.clone()),
    };
    let mut props: HashMap<&str, &str> = HashMap::new();
    for c in n.all("property") {
        if let (Some(k), Some(v)) = (c.str_arg(0), c.str_arg(1)) {
            props.insert(k, v);
        }
    }
    let reference = props.get("Reference").copied().unwrap_or("U");
    let prefix = reference.trim_end_matches(|c: char| c == '?' || c.is_ascii_digit());
    let hide_names = n.get("pin_names").is_some_and(|p| p.flag("hide"));
    let hide_numbers = n.get("pin_numbers").is_some_and(|p| p.flag("hide"));
    let footprint = props.get("Footprint").copied().filter(|f| !f.is_empty()).map(|f| f.rsplit(':').next().unwrap_or(f).to_string());
    let description = props.get("Description").copied().filter(|d| !d.is_empty()).or_else(|| props.get("ki_description").copied()).unwrap_or("").to_string();
    let mut s = Symbol {
        name,
        description,
        prefix: if prefix.is_empty() { "U".into() } else { prefix.to_string() },
        parts: 1,
        pins: Vec::new(),
        drawings: Vec::new(),
        footprints: footprint.into_iter().collect(),
        library,
    };
    for sub in n.all("symbol") {
        // "Name_<unit>_<style>": the unit it draws (0 for all of them), and the body style (2 is De Morgan's alternate).
        let sub_name = sub.str_arg(0).unwrap_or("");
        let mut parts = sub_name.rsplitn(3, '_');
        let (style, unit) = (parts.next().and_then(|x| x.parse::<u32>().ok()).unwrap_or(0), parts.next().and_then(|x| x.parse::<u32>().ok()).unwrap_or(0));
        if style == 2 {
            left.add("alternate (De Morgan) symbol bodies left out");
            continue;
        }
        s.parts = s.parts.max(unit);
        for g in sub.args() {
            let drawing = match g.head() {
                Some("pin") => {
                    let (Some(at), Some(angle), Some(length)) = (point(g, "at"), g.get("at").and_then(|a| a.num_arg(2)), g.child_num("length")) else { continue };
                    let t = angle.to_radians();
                    // KiCad gives the outer end and points in; this gives the body end and points out.
                    let body = p(at.x + t.cos() * length, at.y + t.sin() * length).rounded();
                    let name = pin_text(g.get("name").and_then(|x| x.str_arg(0)).unwrap_or(""));
                    s.pins.push(Pin {
                        part: unit,
                        number: pin_text(g.get("number").and_then(|x| x.str_arg(0)).unwrap_or("")),
                        name_shown: !hide_names && !name.is_empty(),
                        name,
                        at: body,
                        length,
                        angle: (angle + 180.0).rem_euclid(360.0),
                        kind: kind_of(g.str_arg(0).unwrap_or("")),
                        hidden: g.flag("hide"),
                        number_shown: !hide_numbers,
                    });
                    continue;
                }
                Some("polyline") => {
                    let mut pts: Vec<P> = g.points().into_iter().map(|(x, y)| p(x, y)).collect();
                    if pts.len() < 2 {
                        continue;
                    }
                    if pts.len() > 3 && pts[0].same(pts[pts.len() - 1]) && filled(g) {
                        pts.pop();
                        Drawing::Polygon(pts)
                    } else {
                        Drawing::Polyline(pts)
                    }
                }
                Some("rectangle") => match (point(g, "start"), point(g, "end")) {
                    (Some(a), Some(b)) => Drawing::Rect(a, b),
                    _ => continue,
                },
                Some("circle") => match (point(g, "center"), g.child_num("radius")) {
                    (Some(c), Some(r)) => Drawing::Ellipse { c, rx: r, ry: r },
                    _ => continue,
                },
                Some("arc") => {
                    let (Some(a), Some(m), Some(e)) = (point(g, "start"), point(g, "mid"), point(g, "end")) else { continue };
                    match circumcircle(a, m, e) {
                        Some((c, r)) => {
                            let ang = |q: P| (q.y - c.y).atan2(q.x - c.x).to_degrees().rem_euclid(360.0);
                            // Drawn anticlockwise from its start: swap the ends if KiCad's goes the other way.
                            if m.sub(a).cross(e.sub(m)) > 0.0 {
                                Drawing::Arc { c, r, start: ang(a), end: ang(e) }
                            } else {
                                Drawing::Arc { c, r, start: ang(e), end: ang(a) }
                            }
                        }
                        None => Drawing::Line(a, e),
                    }
                }
                Some("text") => match (g.str_arg(0), point(g, "at")) {
                    (Some(t), Some(at)) => Drawing::Text { at, text: unescape(t) },
                    _ => continue,
                },
                Some("bezier") => {
                    left.add("curves (Bézier) in symbols left out");
                    continue;
                }
                _ => continue,
            };
            s.drawings.push((unit, drawing));
        }
    }
    // A supply is flagged `(power)`; an older library's is a one-pin symbol named for a power rail (#PWR).
    let power = n.get("power").is_some() || (s.prefix.starts_with("#PWR") && s.pins.len() == 1 && s.pins[0].kind == PinKind::Power);
    Some((id, s, power))
}

/// The symbols in a library file or a sheet's `lib_symbols`, by the name each is placed by. A
/// symbol that `extends` another has that one's pins and drawing.
pub fn read_symbols(list: &Sx, left: &mut Left) -> Vec<(String, Symbol, bool)> {
    let mut out: Vec<(String, Symbol, bool)> = Vec::new();
    let mut later: Vec<&Sx> = Vec::new();
    for n in list.all("symbol") {
        if n.get("extends").is_some() {
            later.push(n);
        } else if let Some(one) = read_symbol(n, left) {
            out.push(one);
        }
    }
    for n in later {
        let Some(parent) = n.child_str("extends") else { continue };
        let Some((_, base, power)) = out.iter().find(|(id, s, _)| id == parent || s.name == parent).cloned() else {
            left.add("symbols that extend one that isn't there left out");
            continue;
        };
        let Some((id, mut s, _)) = read_symbol(n, left) else { continue };
        s.pins = base.pins;
        s.drawings = base.drawings;
        s.parts = base.parts;
        if s.footprints.is_empty() {
            s.footprints = base.footprints;
        }
        out.push((id, s, power));
    }
    out
}

/// The symbols of a `.kicad_sym` library.
pub fn read_library(text: &str, library: &str) -> Result<(Vec<Symbol>, Vec<String>), String> {
    let root = parse(text)?;
    if root.head() != Some("kicad_symbol_lib") {
        return Err("isn't a KiCad symbol library (it should start with kicad_symbol_lib)".into());
    }
    let mut left = Left::default();
    let symbols = read_symbols(&root, &mut left).into_iter().map(|(_, mut s, _)| {
        s.library = library.to_string();
        s
    });
    Ok((symbols.collect(), left.notes()))
}

/// A 2×2 matrix of whole numbers: a turn by quarters, or one with a mirror.
type M = [[i32; 2]; 2];

fn turn(q: i32) -> M {
    match q.rem_euclid(4) {
        0 => [[1, 0], [0, 1]],
        1 => [[0, -1], [1, 0]],
        2 => [[-1, 0], [0, -1]],
        _ => [[0, 1], [-1, 0]],
    }
}

fn times(a: M, b: M) -> M {
    let mut r = [[0; 2]; 2];
    for i in 0..2 {
        for j in 0..2 {
            r[i][j] = a[i][0] * b[0][j] + a[i][1] * b[1][j];
        }
    }
    r
}

/// How a placed symbol is turned and mirrored in KiCad's terms (`(at x y rot)` and `(mirror x|y)`),
/// as Coppersmith's: mirrored left to right, then turned `turns` quarters anticlockwise.
pub fn orientation(rot: f64, mirror: Option<&str>) -> (u8, bool) {
    let q = (rot / 90.0).round() as i32;
    // `mirror x` flips about the x axis (up ↔ down), `mirror y` flips left ↔ right.
    let flip: M = match mirror {
        Some("x") => [[1, 0], [0, -1]],
        Some("y") => [[-1, 0], [0, 1]],
        _ => [[1, 0], [0, 1]],
    };
    // KiCad turns the symbol first and then mirrors it as it lies (it reads `at` and then `mirror`,
    // and each new transform is laid over the last).
    let t = times(flip, turn(q));
    let det = t[0][0] * t[1][1] - t[0][1] * t[1][0];
    let (r, mirrored) = if det > 0 { (t, false) } else { (times(t, [[-1, 0], [0, 1]]), true) };
    // r is a turn: its first column is (cos, sin).
    let quarters = match (r[0][0], r[1][0]) {
        (1, 0) => 0,
        (0, 1) => 1,
        (-1, 0) => 2,
        _ => 3,
    };
    (quarters, mirrored)
}

fn io_of(shape: &str) -> PortIo {
    match shape {
        "input" => PortIo::Input,
        "output" => PortIo::Output,
        "passive" | "unspecified" => PortIo::Unspecified,
        _ => PortIo::Bidirectional,
    }
}

/// The sizes of KiCad's named papers, landscape, in millimetres.
fn kicad_paper(name: &str) -> Option<(f64, f64)> {
    Some(match name {
        "A5" => (210.0, 148.0),
        "A4" => (297.0, 210.0),
        "A3" => (420.0, 297.0),
        "A2" => (594.0, 420.0),
        "A1" => (841.0, 594.0),
        "A0" => (1189.0, 841.0),
        "A" | "USLetter" => (279.4, 215.9),
        "USLegal" => (355.6, 215.9),
        "B" | "USLedger" => (431.8, 279.4),
        "C" => (558.8, 431.8),
        "D" => (863.6, 558.8),
        "E" => (1117.6, 863.6),
        _ => return None,
    })
}

/// A KiCad sheet. Returns it with what was left out.
pub fn read_sheet(text: &str, name: &str) -> Result<(Schematic, Vec<String>), String> {
    let root = parse(text)?;
    if root.head() != Some("kicad_sch") {
        return Err("isn't a KiCad schematic (it should start with kicad_sch)".into());
    }
    let mut left = Left::default();
    let mut sch = Schematic::default();
    sch.name = name.to_string();
    // The page is KiCad's own size (an Altium A4 is a little smaller than a KiCad one, which would
    // leave the drawing off its edge), as a custom page.
    let (mut w, mut h) = (297.0, 210.0);
    if let Some(paper) = root.get("paper") {
        match paper.str_arg(0) {
            Some("User") => {
                if let (Some(pw), Some(ph)) = (paper.num_arg(1), paper.num_arg(2)) {
                    (w, h) = (pw, ph);
                }
            }
            Some(n) => {
                if let Some((pw, ph)) = kicad_paper(n) {
                    (w, h) = (pw, ph);
                }
            }
            None => {}
        }
        if paper.flag("portrait") {
            (w, h) = (h, w);
        }
    }
    sch.paper = Paper::Custom(w, h);
    let lib: HashMap<String, (Symbol, bool)> = match root.get("lib_symbols") {
        Some(l) => read_symbols(l, &mut left).into_iter().map(|(id, s, power)| (id, (s, power))).collect(),
        None => HashMap::new(),
    };
    // A sheet's points are from the page's top left, running down; here the page's bottom left, running up.
    let page_height = sch.paper.size().1;
    let at = |x: f64, y: f64| p(x, page_height - y);
    for c in root.args() {
        match c.head() {
            Some("symbol") => {
                let Some(id) = c.child_str("lib_id") else { continue };
                let Some((symbol, power)) = c.child_str("lib_name").and_then(|n| lib.get(n)).or_else(|| lib.get(id)) else {
                    left.add("parts whose symbol isn't in the sheet");
                    continue;
                };
                let pos = c.get("at");
                let (x, y, rot) = (pos.and_then(|a| a.num_arg(0)).unwrap_or(0.0), pos.and_then(|a| a.num_arg(1)).unwrap_or(0.0), pos.and_then(|a| a.num_arg(2)).unwrap_or(0.0));
                let (turns, mirror) = orientation(rot, c.get("mirror").and_then(|m| m.str_arg(0)));
                let props: HashMap<&str, &str> = c.all("property").filter_map(|q| Some((q.str_arg(0)?, q.str_arg(1)?))).collect();
                let reference = props.get("Reference").copied().unwrap_or("");
                let value = props.get("Value").copied().unwrap_or("");
                if *power {
                    // Flags that only tell the checker a net is driven aren't drawn.
                    if value == "PWR_FLAG" {
                        continue;
                    }
                    // Which way its body lies from its pin.
                    let toward = symbol.pins.first().map(|pin| (pin.angle + 180.0).rem_euclid(360.0)).unwrap_or(90.0);
                    let a = crate::pcb::schematic::transform_angle(toward, turns, mirror);
                    // Coppersmith counts which way the symbol lies from its wire, from up, in quarter turns anticlockwise.
                    let away = ((a - 90.0) / 90.0).round() as i32;
                    let id = sch.add_marker(at(x, y), value, LabelKind::Power, PowerStyle::for_name(value), PortIo::default());
                    if let Some(l) = sch.labels.iter_mut().find(|l| l.id == id) {
                        l.turns = away.rem_euclid(4) as u8;
                    }
                    continue;
                }
                let id = sch.place(symbol, at(x, y));
                if let Some(part) = sch.part_mut(id) {
                    part.designator = reference.to_string();
                    part.value = value.to_string();
                    if let Some(f) = props.get("Footprint").filter(|f| !f.is_empty()) {
                        part.footprint = f.rsplit(':').next().unwrap_or(f).to_string();
                    }
                    part.turns = turns;
                    part.mirror = mirror;
                    part.part = c.child_num("unit").unwrap_or(1.0).max(1.0) as u32;
                }
            }
            Some("wire") => {
                let pts: Vec<P> = c.points().into_iter().map(|(x, y)| at(x, y)).collect();
                sch.add_wire(pts);
            }
            Some("label") => {
                if let (Some(t), Some(pos)) = (c.str_arg(0), c.get("at")) {
                    sch.add_marker(at(pos.num_arg(0).unwrap_or(0.0), pos.num_arg(1).unwrap_or(0.0)), &unescape(t), LabelKind::Net, PowerStyle::default(), PortIo::default());
                }
            }
            Some("global_label" | "hierarchical_label") => {
                if let (Some(t), Some(pos)) = (c.str_arg(0), c.get("at")) {
                    let io = io_of(c.child_str("shape").unwrap_or("bidirectional"));
                    sch.add_marker(at(pos.num_arg(0).unwrap_or(0.0), pos.num_arg(1).unwrap_or(0.0)), &unescape(t), LabelKind::Port, PowerStyle::default(), io);
                }
            }
            Some("bus" | "bus_entry" | "bus_alias") => left.add("buses and bus entries left out (the wires of a bus's members come in)"),
            Some("sheet") => left.add("hierarchical sheets left out (each sheet's own file opens by itself)"),
            Some("image") => left.add("images left out"),
            _ => {}
        }
    }
    sch.fix_junctions();
    Ok((sch, left.notes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_unescaped_and_overlines_become_plain() {
        assert_eq!(unescape("VPP{slash}MCLR"), "VPP/MCLR");
        assert_eq!(pin_text("~{RESET}"), "RESET");
        assert_eq!(pin_text("A~{B}C"), "ABC");
        assert_eq!(pin_text("~"), "");
        assert_eq!(pin_text("D0"), "D0");
    }

    #[test]
    fn a_turn_and_a_mirror_become_a_turn_and_a_mirror_here() {
        assert_eq!(orientation(0.0, None), (0, false));
        assert_eq!(orientation(90.0, None), (1, false));
        assert_eq!(orientation(270.0, None), (3, false));
        // Mirrored left to right is this crate's own mirror.
        assert_eq!(orientation(0.0, Some("y")), (0, true));
        // Mirrored top to bottom is a mirror and a half turn.
        assert_eq!(orientation(0.0, Some("x")), (2, true));
    }

    #[test]
    fn the_orientations_agree_with_a_point_moved_by_hand() {
        use crate::pcb::schematic::transform;
        // A point right of the origin, and where KiCad's rotation and mirror leave it (Y up): turned
        // first, then mirrored as it lies, so a turned and mirrored one differs from a mirrored and turned one.
        for (rot, mirror, want) in [(0.0, None, p(1.0, 0.0)), (90.0, None, p(0.0, 1.0)), (180.0, None, p(-1.0, 0.0)), (0.0, Some("y"), p(-1.0, 0.0)), (0.0, Some("x"), p(1.0, 0.0)), (90.0, Some("y"), p(0.0, 1.0)), (90.0, Some("x"), p(0.0, -1.0))] {
            let (turns, m) = orientation(rot, mirror);
            let got = transform(p(1.0, 0.0), P::default(), turns, m);
            assert!(got.dist(want) < 1e-9, "rot {rot} mirror {mirror:?}: {got:?} not {want:?}");
        }
        // A point above: flipping about the x axis turns it over.
        let (turns, m) = orientation(0.0, Some("x"));
        assert!(transform(p(0.0, 1.0), P::default(), turns, m).dist(p(0.0, -1.0)) < 1e-9);
    }

    const LIB: &str = r#"(kicad_symbol_lib (version 20231120) (generator "kicad_symbol_editor")
 (symbol "R" (pin_numbers (hide yes)) (pin_names (offset 0))
  (property "Reference" "R" (at 2 0 90)) (property "Value" "R" (at 0 0 90))
  (property "Footprint" "Resistor_SMD:R_0603_1608Metric" (at 0 0 0)) (property "Description" "Resistor" (at 0 0 0))
  (symbol "R_0_1" (rectangle (start -1.016 -2.54) (end 1.016 2.54) (stroke (width 0.254) (type default)) (fill (type none))))
  (symbol "R_1_1"
   (pin passive line (at 0 3.81 270) (length 1.27) (name "~" (effects (font (size 1.27 1.27)))) (number "1" (effects (font (size 1.27 1.27)))))
   (pin passive line (at 0 -3.81 90) (length 1.27) (name "~" (effects (font (size 1.27 1.27)))) (number "2" (effects (font (size 1.27 1.27)))))))
 (symbol "R_US" (extends "R") (property "Reference" "R" (at 0 0 0)) (property "Value" "R_US" (at 0 0 0)) (property "Description" "US-style resistor" (at 0 0 0))))"#;

    #[test]
    fn a_symbol_library_reads_with_pins_pointing_out_of_the_body() {
        let (symbols, notes) = read_library(LIB, "Device").unwrap();
        assert!(notes.is_empty(), "{notes:?}");
        assert_eq!(symbols.len(), 2);
        let r = &symbols[0];
        assert_eq!((r.name.as_str(), r.prefix.as_str(), r.parts, r.library.as_str()), ("R", "R", 1, "Device"));
        assert_eq!(r.footprints, vec!["R_0603_1608Metric".to_string()]);
        assert_eq!(r.description, "Resistor");
        // Pin 1: KiCad has its outer end at (0, 3.81) pointing down into the body, 1.27 long.
        let one = r.pins.iter().find(|p| p.number == "1").unwrap();
        assert!(one.at.same(p(0.0, 2.54)), "the body end: {:?}", one.at);
        assert!((one.angle - 90.0).abs() < 1e-9, "points out, up: {}", one.angle);
        assert!(!one.name_shown && !one.number_shown, "names are `~`, numbers hidden by the symbol");
        assert!(r.drawings.iter().any(|(_, d)| matches!(d, Drawing::Rect(..))));
        // The one that extends it has its pins and drawing, and its own name.
        let us = &symbols[1];
        assert_eq!((us.name.as_str(), us.pins.len(), us.description.as_str()), ("R_US", 2, "US-style resistor"));
        assert_eq!(us.drawings.len(), r.drawings.len());
    }

    const SHEET: &str = r##"(kicad_sch (version 20231120) (generator "eeschema") (paper "A3")
 (lib_symbols
  (symbol "Device:R" (property "Reference" "R" (at 0 0 0)) (property "Value" "R" (at 0 0 0))
   (symbol "R_0_1" (rectangle (start -1 -2.5) (end 1 2.5) (fill (type none))))
   (symbol "R_1_1"
    (pin passive line (at 0 3.81 270) (length 1.27) (name "~") (number "1"))
    (pin passive line (at 0 -3.81 90) (length 1.27) (name "~") (number "2"))))
  (symbol "power:GND" (power) (property "Reference" "#PWR" (at 0 0 0)) (property "Value" "GND" (at 0 0 0))
   (symbol "GND_1_1" (pin power_in line (at 0 0 270) (length 0) (name "GND") (number "1")))))
 (symbol (lib_id "Device:R") (at 100 50 0) (unit 1) (property "Reference" "R1" (at 0 0 0)) (property "Value" "10k" (at 0 0 0)) (property "Footprint" "Resistor_SMD:R_0603_1608Metric" (at 0 0 0)))
 (symbol (lib_id "power:GND") (at 100 60 0) (unit 1) (property "Reference" "#PWR01" (at 0 0 0)) (property "Value" "GND" (at 0 0 0)))
 (wire (pts (xy 100 53.81) (xy 100 60)) (stroke (width 0) (type default)))
 (label "OUT{slash}1" (at 100 46.19 0))
 (global_label "SIG" (shape input) (at 90 46.19 0))
 (sheet (at 10 10) (size 20 20))
 (bus (pts (xy 0 0) (xy 5 0))))"##;

    #[test]
    fn a_sheet_reads_with_its_part_wire_supply_and_labels() {
        let (s, notes) = read_sheet(SHEET, "demo").unwrap();
        assert_eq!(s.paper, Paper::Custom(420.0, 297.0), "KiCad's A3, its own size");
        assert_eq!(s.parts.len(), 1);
        let r = &s.parts[0];
        assert_eq!((r.designator.as_str(), r.value.as_str(), r.footprint.as_str()), ("R1", "10k", "R_0603_1608Metric"));
        // On the page 100 across and 50 down is 100 across and 297 - 50 up here (an A3 sheet is 297 high); pin 1 is the end up the page.
        assert!(r.at.same(p(100.0, 247.0)), "{:?}", r.at);
        let pins = r.pins();
        let one = pins.iter().find(|x| x.number == "1").unwrap();
        let two = pins.iter().find(|x| x.number == "2").unwrap();
        assert!(one.tip.same(p(100.0, 250.81)), "{:?}", one.tip);
        assert!(two.tip.same(p(100.0, 243.19)), "{:?}", two.tip);
        // The ground is a power port at the end of the wire, hanging down.
        let gnd = s.labels.iter().find(|l| l.kind == LabelKind::Power).unwrap();
        assert_eq!(gnd.text, "GND");
        assert!(gnd.at.same(p(100.0, 237.0)) && gnd.style == PowerStyle::Ground);
        assert_eq!(gnd.turns, 2, "the body is below its pin");
        assert_eq!(s.wires.len(), 1);
        assert!(s.labels.iter().any(|l| l.text == "OUT/1" && l.kind == LabelKind::Net));
        assert!(s.labels.iter().any(|l| l.text == "SIG" && l.kind == LabelKind::Port && l.io == PortIo::Input));
        assert!(notes.iter().any(|n| n.contains("hierarchical")) && notes.iter().any(|n| n.contains("buses")), "{notes:?}");
        // The wire runs from pin 2 of the resistor to the ground: so the sheet's nets agree.
        assert!(s.check().iter().all(|p| !p.text.contains("isn't connected") || p.text.contains("R1-1")), "{:?}", s.check());
    }

    /// Each net as the pins on it, so nets compare whatever they're called.
    fn nets_by_pins(s: &Schematic) -> Vec<Vec<String>> {
        let mut v: Vec<Vec<String>> = s.netlist().into_iter().map(|(_, mut pins)| {
            pins.sort();
            pins
        }).collect();
        v.sort();
        v
    }

    #[test]
    fn a_kicad_sheet_survives_being_written_as_an_altium_schdoc() {
        let (s, _) = read_sheet(SHEET, "demo").unwrap();
        let bytes = crate::pcb::altium::schsheet::write(&s).unwrap();
        let (back, _) = crate::pcb::altium::schsheet::read(&bytes, "demo").unwrap();
        let (bw, bh) = back.paper.size();
        assert!((bw - 420.0).abs() < 0.1 && (bh - 297.0).abs() < 0.1, "the page keeps its size: {bw} × {bh}");
        assert_eq!((back.parts.len(), back.wires.len()), (s.parts.len(), s.wires.len()));
        let (a, b) = (&s.parts[0], &back.parts[0]);
        assert_eq!((b.designator.as_str(), b.value.as_str(), b.footprint.as_str()), (a.designator.as_str(), a.value.as_str(), a.footprint.as_str()));
        // Each pin ends where it did.
        for pin in a.pins() {
            let q = b.pins().into_iter().find(|x| x.number == pin.number).unwrap();
            assert!(q.tip.dist(pin.tip) < 1e-3, "pin {} moved: {:?} -> {:?}", pin.number, pin.tip, q.tip);
        }
        // The ground stays a ground, the labels keep their names and their kind.
        let gnd = back.labels.iter().find(|l| l.kind == LabelKind::Power).unwrap();
        assert_eq!((gnd.text.as_str(), gnd.style), ("GND", PowerStyle::Ground));
        assert!(back.labels.iter().any(|l| l.text == "SIG" && l.kind == LabelKind::Port && l.io == PortIo::Input));
        assert!(back.labels.iter().any(|l| l.text == "OUT/1" && l.kind == LabelKind::Net));
        assert_eq!(nets_by_pins(&back), nets_by_pins(&s), "the same pins are joined");
    }

    #[test]
    fn kicad_symbols_survive_being_written_as_an_altium_schlib() {
        let (symbols, _) = read_library(LIB, "Device").unwrap();
        let bytes = crate::pcb::altium::write::schlib(&symbols).unwrap();
        let back = crate::pcb::altium::schlib::read(&bytes, "Device").unwrap();
        assert_eq!(back.symbols.len(), 2);
        let (a, b) = (&symbols[0], back.symbols.iter().find(|s| s.name == "R").unwrap());
        assert_eq!((b.prefix.as_str(), b.pins.len(), b.parts), (a.prefix.as_str(), a.pins.len(), a.parts));
        assert_eq!(b.footprints, a.footprints);
        for pin in &a.pins {
            let q = b.pins.iter().find(|x| x.number == pin.number).unwrap();
            assert!(q.at.dist(pin.at) < 1e-3 && (q.angle - pin.angle).abs() < 1e-6 && (q.length - pin.length).abs() < 1e-3, "pin {}: {:?} -> {:?}", pin.number, pin, q);
        }
    }

    #[test]
    fn something_that_is_not_a_sheet_is_said_so() {
        let Err(e) = read_sheet("(kicad_pcb)", "x") else { panic!("a board isn't a sheet") };
        assert!(e.contains("isn't a KiCad schematic"));
        let Err(e) = read_library("(kicad_sch)", "x") else { panic!("a sheet isn't a library") };
        assert!(e.contains("isn't a KiCad symbol library"));
    }
}
