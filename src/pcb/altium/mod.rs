//! Reading Altium Designer and Circuit Studio libraries.
//!
//! Altium's library files are Microsoft compound files (the container old
//! Word documents use) holding one storage per part:
//! - `.PcbLib`: footprints. Each part's `Data` stream is a run of binary
//!   records (pads, tracks, arcs, text, fills, regions, 3D bodies), with
//!   lengths in Altium's internal unit of 1/10 000 mil.
//! - `.SchLib`: symbols. Each part's `Data` stream is a run of records, most
//!   of them text (`|RECORD=13|LOCATION.X=…|`), pins in binary.
//! - `.IntLib`: an integrated library, holding a whole PcbLib and SchLib,
//!   each compressed with zlib behind a one-byte marker.
//!
//! KiCad's Altium importer is the open reference for these layouts; this
//! follows it, checked against real Circuit Studio files in testdata/altium.

pub mod pcbdoc;
pub mod pcblib;
pub mod prims;
pub mod schdoc;
pub mod schlib;
pub mod schsheet;
pub mod write;

use std::collections::HashMap;
use std::io::{Cursor, Read, Seek};
use std::path::Path;

use crate::pcb::library::FootprintDef;

/// Altium's internal unit, in millimetres: 1/10 000 mil.
pub const UNIT: f64 = 0.000_002_54;

/// What came out of a library file.
#[derive(Default)]
pub struct Imported {
    pub footprints: Vec<FootprintDef>,
    pub symbols: Vec<schlib::Symbol>,
    /// Things that were left out or approximated, worth telling someone.
    pub notes: Vec<String>,
}

/// Reads any Altium library file: PcbLib, SchLib or IntLib.
pub fn read(path: &Path) -> Result<Imported, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let name = path.file_stem().unwrap_or_default().to_string_lossy().to_string();
    let ext = path.extension().unwrap_or_default().to_string_lossy().to_lowercase();
    let mut got = match ext.as_str() {
        "pcblib" => pcblib::read(&bytes, &name)?,
        "schlib" => schlib::read(&bytes, &name)?,
        "intlib" => read_intlib(&bytes, &name)?,
        _ => return Err(format!("{} isn't an Altium library (PcbLib, SchLib or IntLib)", path.display())),
    };
    // A footprint takes its name letter (R, C, U…) from a symbol that uses it.
    for f in &mut got.footprints {
        if let Some(s) = got.symbols.iter().find(|s| s.footprints.iter().any(|n| n == &f.name)) {
            f.prefix = s.prefix.clone();
            if f.description.is_empty() {
                f.description = s.description.clone();
            }
        }
    }
    Ok(got)
}

/// Each extension as it's given, in lower case and in capitals, so a file
/// dialog that matches exactly still shows `.SCHLIB` and `.SchDoc` alike.
pub fn with_cases(exts: &[&str]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for e in exts {
        for v in [e.to_string(), e.to_lowercase(), e.to_uppercase()] {
            if !out.contains(&v) {
                out.push(v);
            }
        }
    }
    out
}

/// A library of footprints or symbols: PcbLib, SchLib or IntLib.
pub fn is_library(path: &Path) -> bool {
    path.extension().is_some_and(|e| ["pcblib", "schlib", "intlib"].iter().any(|x| e.eq_ignore_ascii_case(x)))
}

/// A schematic sheet: an Altium or Circuit Studio SchDoc, or Coppersmith's own.
pub fn is_schematic(path: &Path) -> bool {
    path.extension().is_some_and(|e| ["schdoc", "csschdoc", "cssch"].iter().any(|x| e.eq_ignore_ascii_case(x)))
}

/// The kinds of Altium file there are readers for.
pub fn is_board(path: &Path) -> bool {
    path.extension().is_some_and(|e| e.eq_ignore_ascii_case("pcbdoc") || e.eq_ignore_ascii_case("cspcbdoc"))
}

pub fn is_sheet(path: &Path) -> bool {
    path.extension().is_some_and(|e| e.eq_ignore_ascii_case("schdoc") || e.eq_ignore_ascii_case("csschdoc"))
}

pub fn is_project(path: &Path) -> bool {
    path.extension().is_some_and(|e| e.eq_ignore_ascii_case("prjpcb") || e.eq_ignore_ascii_case("csprjpcb"))
}

pub fn read_board(path: &Path) -> Result<pcbdoc::ImportedBoard, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let name = path.file_stem().unwrap_or_default().to_string_lossy().to_string();
    pcbdoc::read(&bytes, &name).map_err(|e| format!("{}: {e}", path.display()))
}

pub fn read_sheet(path: &Path) -> Result<schdoc::Sheet, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let name = path.file_stem().unwrap_or_default().to_string_lossy().to_string();
    schdoc::read(&bytes, &name).map_err(|e| format!("{}: {e}", path.display()))
}

/// A project file's documents (an INI listing them, paths Windows-style).
pub fn project_documents(path: &Path) -> Result<Vec<std::path::PathBuf>, String> {
    let text = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let text = String::from_utf8_lossy(&text);
    let dir = path.parent().unwrap_or(Path::new("."));
    let mut out = Vec::new();
    let mut in_doc = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_doc = line.starts_with("[Document");
        } else if in_doc && let Some(p) = line.strip_prefix("DocumentPath=") {
            let p = dir.join(p.replace('\\', "/"));
            if is_board(&p) || is_sheet(&p) {
                out.push(p);
            }
        }
    }
    Ok(out)
}

/// How a schematic's netlist and a board's nets agree: every schematic net's
/// pins should sit on one board net, and that net on no other pins. Returns
/// the nets checked and what disagrees.
pub fn compare_nets(sch: &[(String, Vec<String>)], board: &crate::pcb::model::Board) -> (usize, Vec<String>) {
    let mut net_of: HashMap<String, u32> = HashMap::new();
    for f in &board.footprints {
        for pad in &f.pads {
            net_of.insert(format!("{}-{}", f.refdes, pad.num), pad.net);
        }
    }
    let mut problems = Vec::new();
    let mut checked = 0;
    let mut used: HashMap<u32, String> = HashMap::new();
    for (name, pins) in sch {
        let nets: Vec<Option<u32>> = pins.iter().map(|p| net_of.get(p).copied()).collect();
        let missing: Vec<&String> = pins.iter().zip(&nets).filter(|(_, n)| n.is_none()).map(|(p, _)| p).collect();
        if !missing.is_empty() {
            problems.push(format!("{name}: pins not on the board: {}", missing.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")));
            continue;
        }
        let mut found: Vec<u32> = nets.into_iter().flatten().collect();
        found.sort();
        found.dedup();
        checked += 1;
        if found.len() != 1 || found[0] == 0 {
            let names: Vec<&str> = found.iter().map(|n| board.net_name(*n)).collect();
            problems.push(format!("{name}: on the board it's {}", names.join(" + ")));
            continue;
        }
        if let Some(other) = used.insert(found[0], name.clone()) {
            problems.push(format!("{name} and {other} are one net on the board ({})", board.net_name(found[0])));
        }
    }
    (checked, problems)
}

/// An integrated library: the PcbLib and SchLib inside, unpacked and read.
fn read_intlib(bytes: &[u8], name: &str) -> Result<Imported, String> {
    let mut file = open(bytes)?;
    let mut out = Imported::default();
    let paths: Vec<String> = file.walk().filter(|e| e.is_stream()).map(|e| e.path().to_string_lossy().to_string()).collect();
    for p in paths {
        let lower = p.to_lowercase();
        let kind = if lower.ends_with(".pcblib") {
            "pcblib"
        } else if lower.ends_with(".schlib") {
            "schlib"
        } else {
            continue;
        };
        let packed = stream(&mut file, &p)?;
        let inner = unpack(&packed).map_err(|e| format!("{name}: {p}: {e}"))?;
        let got = if kind == "pcblib" { pcblib::read(&inner, name)? } else { schlib::read(&inner, name)? };
        out.footprints.extend(got.footprints);
        out.symbols.extend(got.symbols);
        out.notes.extend(got.notes);
    }
    if out.footprints.is_empty() && out.symbols.is_empty() {
        return Err(format!("{name}: no PcbLib or SchLib found inside"));
    }
    Ok(out)
}

/// An IntLib's inner file: a marker byte, then zlib.
fn unpack(packed: &[u8]) -> Result<Vec<u8>, String> {
    let body = if packed.first() == Some(&0x02) { &packed[1..] } else { packed };
    let mut out = Vec::new();
    flate2::read::ZlibDecoder::new(body).read_to_end(&mut out).map_err(|e| format!("couldn't unpack: {e}"))?;
    Ok(out)
}

pub type Compound = cfb::CompoundFile<Cursor<Vec<u8>>>;

pub fn open(bytes: &[u8]) -> Result<Compound, String> {
    cfb::CompoundFile::open(Cursor::new(bytes.to_vec())).map_err(|e| format!("not an Altium file ({e})"))
}

pub fn stream<F: Read + Seek>(file: &mut cfb::CompoundFile<F>, path: &str) -> Result<Vec<u8>, String> {
    let mut s = file.open_stream(path).map_err(|e| format!("{path}: {e}"))?;
    let mut out = Vec::new();
    s.read_to_end(&mut out).map_err(|e| format!("{path}: {e}"))?;
    Ok(out)
}

/// Altium's `|KEY=VALUE|KEY=VALUE` text, keys in capitals.
pub fn params(text: &[u8]) -> HashMap<String, String> {
    let text = String::from_utf8_lossy(text);
    text.trim_end_matches('\0')
        .split('|')
        .filter_map(|kv| kv.split_once('='))
        .map(|(k, v)| (k.trim().to_uppercase(), v.to_string()))
        .collect()
}

/// A little-endian reader that says so, rather than panicking, when a file
/// is shorter than it claims.
pub struct Reader<'a> {
    pub d: &'a [u8],
    pub i: usize,
}

impl<'a> Reader<'a> {
    pub fn new(d: &'a [u8]) -> Self {
        Reader { d, i: 0 }
    }
    pub fn done(&self) -> bool {
        self.i >= self.d.len()
    }
    pub fn left(&self) -> usize {
        self.d.len().saturating_sub(self.i)
    }
    pub fn bytes(&mut self, n: usize) -> Result<&'a [u8], String> {
        if self.i + n > self.d.len() {
            return Err(format!("record runs past the end ({} of {} bytes)", self.i + n, self.d.len()));
        }
        let out = &self.d[self.i..self.i + n];
        self.i += n;
        Ok(out)
    }
    pub fn skip(&mut self, n: usize) -> Result<(), String> {
        self.bytes(n).map(|_| ())
    }
    pub fn u8(&mut self) -> Result<u8, String> {
        Ok(self.bytes(1)?[0])
    }
    #[allow(dead_code)]
    pub fn u16(&mut self) -> Result<u16, String> {
        Ok(u16::from_le_bytes(self.bytes(2)?.try_into().unwrap()))
    }
    pub fn i16(&mut self) -> Result<i16, String> {
        Ok(i16::from_le_bytes(self.bytes(2)?.try_into().unwrap()))
    }
    pub fn u32(&mut self) -> Result<u32, String> {
        Ok(u32::from_le_bytes(self.bytes(4)?.try_into().unwrap()))
    }
    pub fn i32(&mut self) -> Result<i32, String> {
        Ok(i32::from_le_bytes(self.bytes(4)?.try_into().unwrap()))
    }
    pub fn f64(&mut self) -> Result<f64, String> {
        Ok(f64::from_le_bytes(self.bytes(8)?.try_into().unwrap()))
    }
    /// A length in Altium units, in millimetres.
    pub fn mm(&mut self) -> Result<f64, String> {
        Ok(self.i32()? as f64 * UNIT)
    }
    /// A string with a one-byte length in front.
    pub fn pascal(&mut self) -> Result<String, String> {
        let n = self.u8()? as usize;
        Ok(String::from_utf8_lossy(self.bytes(n)?).to_string())
    }
    /// A block with a four-byte length in front.
    pub fn block(&mut self) -> Result<&'a [u8], String> {
        let n = self.u32()? as usize;
        self.bytes(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data(name: &str) -> std::path::PathBuf {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata/altium").join(name)
    }

    #[test]
    fn reads_every_test_library() {
        for f in ["DAL_LIB.PcbLib", "fixturelib.PcbLib", "PcbLib1.PcbLib", "DAL_LIB.SchLib", "fixturelib.SchLib", "Schlib1.SchLib", "DAL_INTLIB.IntLib"] {
            let got = read(&data(f)).unwrap_or_else(|e| panic!("{f}: {e}"));
            // PcbLib1 holds a single empty footprint.
            if f != "PcbLib1.PcbLib" {
                assert!(!got.footprints.is_empty() || !got.symbols.is_empty(), "{f}: nothing read");
            }
        }
    }

    #[test]
    fn file_kinds_are_told_apart_whatever_the_case() {
        let ext = |s: &str| std::path::PathBuf::from(s);
        assert!(is_library(&ext("Ezdisplay.SCHLIB")) && is_library(&ext("a.PcbLib")) && is_library(&ext("b.intlib")));
        assert!(is_schematic(&ext("01 MAIN.SchDoc")) && is_schematic(&ext("x.CSSCHDOC")) && is_schematic(&ext("y.cssch")));
        assert!(!is_library(&ext("board.PcbDoc")) && !is_schematic(&ext("board.PcbDoc")) && !is_library(&ext("noext")));
        // A dialog that matches exactly gets every spelling.
        let v = with_cases(&["SchLib", "cssch"]);
        for want in ["SchLib", "schlib", "SCHLIB", "cssch", "CSSCH"] {
            assert!(v.iter().any(|x| x == want), "{want} missing from {v:?}");
        }
        assert_eq!(v.iter().filter(|x| *x == "cssch").count(), 1, "no repeats");
    }

    #[test]
    fn the_intlib_holds_the_same_parts_as_its_libraries() {
        let int = read(&data("DAL_INTLIB.IntLib")).unwrap();
        let pcb = read(&data("DAL_LIB.PcbLib")).unwrap();
        assert_eq!(int.footprints.len(), pcb.footprints.len());
        assert!(!int.symbols.is_empty());
    }

    /// Lists a file's storages and streams, to see how Altium lays one out:
    /// COPPERSMITH_DUMP=file cargo test dump_structure -- --ignored --nocapture
    #[test]
    #[ignore]
    fn dump_structure() {
        let Some(f) = std::env::var_os("COPPERSMITH_DUMP") else { return };
        let bytes = std::fs::read(f).unwrap();
        let mut file = open(&bytes).unwrap();
        let entries: Vec<(String, bool, u64)> = file.walk().map(|e| (e.path().to_string_lossy().to_string(), e.is_stream(), e.len())).collect();
        for (p, s, n) in entries.iter().take(60) {
            println!("{} {p} {n}", if *s { "S" } else { "D" });
        }
        println!("{} entries", entries.len());
        for p in ["/FileHeader", "/Library/Data", "/Library/Header", "/SectionKeys"] {
            if let Ok(d) = stream(&mut file, p) {
                println!("--- {p}\n{}", String::from_utf8_lossy(&d[..d.len().min(700)]).escape_debug());
            }
        }
    }
}
