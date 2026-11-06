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

pub mod pcblib;
pub mod schlib;

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
    fn the_intlib_holds_the_same_parts_as_its_libraries() {
        let int = read(&data("DAL_INTLIB.IntLib")).unwrap();
        let pcb = read(&data("DAL_LIB.PcbLib")).unwrap();
        assert_eq!(int.footprints.len(), pcb.footprints.len());
        assert!(!int.symbols.is_empty());
    }
}
