//! Reading KiCad files: boards (`.kicad_pcb`), footprints (`.kicad_mod`, or a
//! `.pretty` folder of them), symbol libraries (`.kicad_sym`), schematics
//! (`.kicad_sch`), and the project file beside a board (`.kicad_pro`) for its rules.
//!
//! KiCad 6 and later write S-expressions; the readers follow what 6 to 10 write.
//! The older formats (KiCad 5 and before) are read as far as the boards go.

pub mod pcb;
pub mod sch;
pub mod sexpr;

use crate::pcb::altium::Imported;
use crate::pcb::altium::pcbdoc::ImportedBoard;
use crate::pcb::library::FootprintDef;
use std::path::Path;

fn ext_is(path: &Path, what: &[&str]) -> bool {
    path.extension().is_some_and(|e| what.iter().any(|w| e.eq_ignore_ascii_case(w)))
}

pub fn is_board(path: &Path) -> bool {
    ext_is(path, &["kicad_pcb"])
}

pub fn is_sheet(path: &Path) -> bool {
    ext_is(path, &["kicad_sch"])
}

/// A KiCad library: a footprint, a folder of them, or symbols.
pub fn is_library(path: &Path) -> bool {
    ext_is(path, &["kicad_mod", "kicad_sym", "pretty"])
}

/// Reads a board, with the rules from the project file beside it if there is one.
pub fn read_board(path: &Path) -> Result<ImportedBoard, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let name = path.file_stem().unwrap_or_default().to_string_lossy().to_string();
    let project = std::fs::read_to_string(path.with_extension("kicad_pro")).ok();
    pcb::read(&text, &name, project.as_deref()).map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pcb::copper::analyse;
    use crate::pcb::drc;
    use crate::pcb::model::Side;

    /// Where KiCad's own demo boards were fetched to for testing (not kept in the repository).
    fn sample(name: &str) -> Option<std::path::PathBuf> {
        let dir = std::env::var_os("COPPERSMITH_KICAD_DIR").map(std::path::PathBuf::from).unwrap_or_else(|| "/Users/dal/.claude/jobs/8cfb36ba/tmp/kicad".into());
        let p = dir.join(name);
        p.exists().then_some(p)
    }

    fn fixture(name: &str) -> std::path::PathBuf {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata/kicad").join(name)
    }

    #[test]
    fn real_kicad_footprints_read_with_their_pads_and_drawing() {
        let r = read_library(&fixture("R_0603_1608Metric.kicad_mod")).unwrap();
        assert_eq!(r.footprints.len(), 1);
        let f = &r.footprints[0];
        assert_eq!((f.name.as_str(), f.prefix.as_str(), f.pads.len()), ("R_0603_1608Metric", "R", 2));
        assert!(f.pads.iter().all(|p| matches!(p.shape, crate::pcb::model::PadShape::RoundRect(_)) && p.drill == 0.0));
        assert!(f.pads[0].at.x < 0.0 && f.pads[1].at.x > 0.0 && f.pads[0].at.y == 0.0, "pad 1 on the left");
        use crate::pcb::model::{Graphic, Layer, Side};
        let on = |l: Layer| f.graphics.iter().filter(|g| g.layer() == l).count();
        assert_eq!(on(Layer::Court(Side::Top)), 4, "the courtyard is a rectangle");
        assert!(on(Layer::Silk(Side::Top)) >= 2 && on(Layer::Fab(Side::Top)) >= 4);
        assert!(f.graphics.iter().all(|g| matches!(g, Graphic::Line { .. } | Graphic::Circle { .. })));

        let s = read_library(&fixture("SOIC-8_3.9x4.9mm_P1.27mm.kicad_mod")).unwrap();
        let f = &s.footprints[0];
        assert_eq!((f.pads.len(), f.prefix.as_str()), (8, "U"));
        let pad = |n: &str| f.pads.iter().find(|p| p.num == n).unwrap();
        // Pin 1 is top left in KiCad's picture (y down), so it's at positive y here.
        assert!(pad("1").at.x < 0.0 && pad("1").at.y > 0.0 && pad("8").at.x > 0.0 && pad("8").at.y > 0.0, "{:?} {:?}", pad("1").at, pad("8").at);
        assert!(pad("4").at.y < 0.0 && (pad("1").at.y - pad("4").at.y - 3.81).abs() < 1e-6, "four pins 1.27 apart");
    }

    #[test]
    fn a_kicad_footprint_survives_being_written_as_an_altium_pcblib() {
        let r = read_library(&fixture("SOIC-8_3.9x4.9mm_P1.27mm.kicad_mod")).unwrap();
        let bytes = crate::pcb::altium::write::pcblib(&r.footprints).unwrap();
        let back = crate::pcb::altium::pcblib::read(&bytes, "x").unwrap();
        assert_eq!(back.footprints.len(), 1);
        let (a, b) = (&r.footprints[0], &back.footprints[0]);
        assert_eq!(b.name, a.name);
        assert_eq!(b.pads.len(), 8);
        for pad in &a.pads {
            let q = b.pads.iter().find(|x| x.num == pad.num).unwrap_or_else(|| panic!("pad {} lost", pad.num));
            assert!(q.at.dist(pad.at) < 1e-3, "pad {} moved: {:?} -> {:?}", pad.num, pad.at, q.at);
            assert!((q.size.x - pad.size.x).abs() < 1e-3 && (q.size.y - pad.size.y).abs() < 1e-3, "pad {} resized", pad.num);
        }
        // The courtyard and silkscreen are still there as lines.
        assert!(b.graphics.len() >= 8, "{}", b.graphics.len());
    }

    #[test]
    fn a_folder_of_footprints_reads_as_a_library() {
        let dir = fixture("");
        let r = read_library(&dir).map(|r| r.footprints.len());
        // The fixtures folder isn't named .pretty but reads the same: both footprints, and nothing else.
        assert_eq!(r.unwrap(), 2);
    }

    #[test]
    fn kicads_own_demo_sheet_comes_in_with_its_nets_and_goes_out_as_an_altium_schdoc() {
        let Some(path) = sample("pic.kicad_sch") else { return };
        let (s, notes) = sch::read_sheet(&std::fs::read_to_string(&path).unwrap(), "pic").unwrap();
        // The file has 105 symbol instances: 59 parts, and the rest supplies (and flags, which aren't drawn).
        assert_eq!(s.parts.len(), 59, "{}", s.parts.len());
        assert!(s.labels.iter().filter(|l| l.kind == crate::pcb::schematic::LabelKind::Power).count() >= 44);
        assert_eq!(s.wires.len(), 131);
        assert!(notes.iter().any(|n| n.contains("hierarchical sheets")), "{notes:?}");
        // Every pin is on a wire or a label, but the ones the designer marked with no-connect: and
        // those are exactly the pins that aren't joined.
        let root = sexpr::parse(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let height = s.paper.size().1;
        let flagged: Vec<crate::pcb::geom::P> = root.all("no_connect").filter_map(|n| n.get("at")).map(|a| crate::pcb::geom::p(a.num_arg(0).unwrap(), height - a.num_arg(1).unwrap())).collect();
        // The sheet is on its page: every part inside it.
        let (w, h) = s.paper.size();
        assert!(s.parts.iter().all(|p| p.at.x > 0.0 && p.at.x < w && p.at.y > 0.0 && p.at.y < h), "parts off the page");
        assert_eq!(flagged.len(), 6);
        let loose: Vec<_> = s.check().into_iter().filter(|p| p.text.contains("isn't connected")).collect();
        assert_eq!(loose.len(), 6, "{:?}", loose.iter().map(|p| &p.text).collect::<Vec<_>>());
        for l in &loose {
            let at = l.at.expect("where it is");
            assert!(flagged.iter().any(|f| f.dist(at) < 1e-3), "{} isn't on a no-connect flag", l.text);
        }
        // Out to Altium and back: the same parts and the same nets.
        let bytes = crate::pcb::altium::schsheet::write(&s).unwrap();
        let (back, _) = crate::pcb::altium::schsheet::read(&bytes, "pic").unwrap();
        assert_eq!((back.parts.len(), back.wires.len()), (s.parts.len(), s.wires.len()));
        let nets = |s: &crate::pcb::schematic::Schematic| {
            let mut v: Vec<Vec<String>> = s.netlist().into_iter().map(|(_, mut pins)| {
                pins.sort();
                pins
            }).collect();
            v.sort();
            v
        };
        assert_eq!(nets(&back), nets(&s));
    }

    #[test]
    fn kicads_own_demo_boards_come_in_whole_and_connected() {
        for (name, parts, copper) in [("pic.kicad_pcb", 63, 2), ("video.kicad_pcb", 189, 4)] {
            let Some(path) = sample(name) else { continue };
            let got = read_board(&path).unwrap();
            let mut b = got.board;
            assert_eq!(b.copper, copper, "{name}");
            assert!(b.footprints.len() >= parts && b.footprints.len() <= parts + 2, "{name}: {} parts", b.footprints.len());
            assert!(b.outline.len() >= 4, "{name} has an outline");
            b.repour();
            let c = analyse(&b);
            assert!(c.rats.is_empty(), "{name}: {} connections still to make", c.rats.len());
            let found = drc::check(&b, &c);
            let errors: Vec<_> = found.iter().filter(|p| p.severity == drc::Severity::Error).map(|p| format!("{}: {}", p.title, p.detail)).collect();
            assert!(errors.is_empty(), "{name}: {errors:?}");
        }
    }

    /// What a sheet came out as: COPPERSMITH_KICAD_SHEET=file cargo test dump_sheet -- --ignored --nocapture
    #[test]
    #[ignore]
    fn dump_sheet() {
        let Some(f) = std::env::var_os("COPPERSMITH_KICAD_SHEET") else { return };
        let text = std::fs::read_to_string(&f).unwrap();
        let (s, notes) = sch::read_sheet(&text, "x").unwrap();
        println!("parts {} wires {} labels {} (power {}) junctions {}", s.parts.len(), s.wires.len(), s.labels.len(), s.labels.iter().filter(|l| l.kind == crate::pcb::schematic::LabelKind::Power).count(), s.junctions.len());
        for n in &notes {
            println!("NOTE {n}");
        }
        let pins: usize = s.parts.iter().map(|p| p.pins().len()).sum();
        let problems = s.check();
        let loose = problems.iter().filter(|p| p.text.contains("isn't connected")).count();
        println!("pins {pins}, {loose} not joined to anything, {} checks in all", problems.len());
        for p in problems.iter().take(6) {
            println!("CHECK {}", p.text);
        }
        println!("nets {}", s.netlist().len());
    }

    /// What a board came out as: COPPERSMITH_KICAD_BOARD=file cargo test dump_kicad -- --ignored --nocapture
    #[test]
    #[ignore]
    fn dump_kicad() {
        let Some(f) = std::env::var_os("COPPERSMITH_KICAD_BOARD") else { return };
        let got = read_board(Path::new(&f)).unwrap();
        let mut b = got.board;
        if let Some(d) = std::env::var("COPPERSMITH_KICAD_LESS").ok().and_then(|v| v.parse::<f64>().ok()) {
            b.rules.clearance -= d;
        }
        println!("rules: clearance {} track {} min track {} via {}/{} min drill {} ring {} edge {}", b.rules.clearance, b.rules.track, b.rules.min_track, b.rules.via_dia, b.rules.via_drill, b.rules.min_drill, b.rules.min_ring, b.rules.edge_clearance);
        println!("copper {} fps {} tracks {} vias {} pours {} keepouts {} nets {} outline {} pts", b.copper, b.footprints.len(), b.tracks.len(), b.vias.len(), b.pours.len(), b.keepouts.len(), b.nets.len(), b.outline.len());
        println!("outline bbox {:?}", crate::pcb::geom::BBox::of(&b.outline));
        for n in &got.notes {
            println!("NOTE {n}");
        }
        b.repour();
        let c = analyse(&b);
        println!("rats {} (unrouted connections)", c.rats.len());
        let mut by: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
        for p in drc::check(&b, &c) {
            *by.entry(format!("{} [{}]", p.title, p.rule.info().name)).or_default() += 1;
        }
        for (k, n) in by {
            println!("{n:5} {k}");
        }
        println!("bottom parts {} of {}", b.footprints.iter().filter(|f| f.side == Side::Bottom).count(), b.footprints.len());
        let shown = std::env::var("COPPERSMITH_KICAD_SHOW").unwrap_or_default();
        for p in drc::check(&b, &c).iter().filter(|p| !shown.is_empty() && p.title == shown).take(12) {
            println!("EG {} | {} | at ({:.2},{:.2})", p.title, p.detail, p.at.x, p.at.y);
        }
    }
}

/// What a footprint is called after the part it's for: R, C, U…
fn prefix_of(name: &str) -> &'static str {
    let n = name.to_uppercase();
    for (starts, letter) in [("R_", "R"), ("C_", "C"), ("CP_", "C"), ("L_", "L"), ("D_", "D"), ("LED_", "D"), ("SOD", "D"), ("Q_", "Q"), ("SOT-23", "Q"), ("TO-", "Q"), ("FUSE", "F"), ("CRYSTAL", "Y"), ("XTAL", "Y"), ("PINHEADER", "J"), ("PINSOCKET", "J"), ("CONN", "J"), ("USB", "J"), ("BUTTON", "SW"), ("SW_", "SW")] {
        if n.starts_with(starts) {
            return letter;
        }
    }
    "U"
}

/// One footprint file.
fn footprint_def(text: &str, library: &str) -> Result<(FootprintDef, Vec<String>), String> {
    let root = sexpr::parse(text)?;
    if !matches!(root.head(), Some("footprint" | "module")) {
        return Err("isn't a KiCad footprint (it should start with footprint)".into());
    }
    let mut left = pcb::Left::default();
    let fp = pcb::parse_footprint(&root, &mut |_| 0, &mut left);
    let def = FootprintDef {
        prefix: prefix_of(&fp.name).to_string(),
        name: fp.name,
        category: if library.is_empty() { fp.library } else { library.to_string() },
        description: fp.description,
        pads: fp.pads,
        graphics: fp.graphics,
        label_at: fp.label_at,
        keepouts: Vec::new(),
        source: String::new(),
    };
    Ok((def, left.notes()))
}

/// Reads a KiCad library: a `.kicad_mod`, a `.pretty` folder, or a `.kicad_sym`.
pub fn read_library(path: &Path) -> Result<Imported, String> {
    let name = path.file_stem().unwrap_or_default().to_string_lossy().to_string();
    let mut out = Imported::default();
    if path.is_dir() {
        let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(path).map_err(|e| format!("{}: {e}", path.display()))?.flatten().map(|e| e.path()).filter(|p| ext_is(p, &["kicad_mod"])).collect();
        files.sort();
        for f in files {
            let text = std::fs::read_to_string(&f).map_err(|e| format!("{}: {e}", f.display()))?;
            match footprint_def(&text, &name) {
                Ok((mut def, notes)) => {
                    def.source = name.clone();
                    out.footprints.push(def);
                    out.notes.extend(notes);
                }
                Err(e) => out.notes.push(format!("{}: {e}; left out", f.file_name().unwrap_or_default().to_string_lossy())),
            }
        }
        return Ok(out);
    }
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if ext_is(path, &["kicad_mod"]) {
        let library = path.parent().and_then(|d| d.file_stem()).map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        let (mut def, notes) = footprint_def(&text, &library).map_err(|e| format!("{}: {e}", path.display()))?;
        def.source = name;
        out.footprints.push(def);
        out.notes = notes;
        return Ok(out);
    }
    if ext_is(path, &["kicad_sym"]) {
        let (symbols, notes) = sch::read_library(&text, &name).map_err(|e| format!("{}: {e}", path.display()))?;
        out.symbols = symbols;
        out.notes = notes;
        return Ok(out);
    }
    Err(format!("{} isn't a KiCad footprint, footprint folder or symbol library", path.display()))
}
