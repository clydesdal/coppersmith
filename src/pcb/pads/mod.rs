//! PADS (PowerPCB, PADS Layout) boards in the ASCII format (`.asc`): read and written.
//!
//! The format is line-oriented text. A file is a header line (`!PADS-POWERPCB-V9.5-BASIC!`)
//! and then sections (`*PCB*`, `*LINES*`, `*PARTDECAL*`, `*PART*`, `*ROUTE*`…), whose records have
//! no terminator: each says how many lines follow it. The line layouts were worked out from
//! real boards (several versions, 2 to 8 copper layers) and from KiCad's importer, which is read
//! as a reference for the format only.

pub mod read;
pub mod write;

use crate::pcb::altium::pcbdoc::ImportedBoard;
use std::path::Path;

/// Does this file look like a PADS ASCII board? `.asc` is also what other tools (LTspice) call
/// theirs, so the first line has to say so.
pub fn is_board(path: &Path) -> bool {
    let named = path.extension().is_some_and(|e| ["asc", "pads"].iter().any(|x| e.eq_ignore_ascii_case(x)));
    if !named {
        return false;
    }
    use std::io::Read;
    let Ok(mut f) = std::fs::File::open(path) else { return false };
    let mut head = [0u8; 16];
    let n = f.read(&mut head).unwrap_or(0);
    head[..n].starts_with(b"!PADS-")
}

/// Reads a board.
pub fn read_board(path: &Path) -> Result<ImportedBoard, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    // Some boards have stray bytes in their settings that aren't text.
    let text = String::from_utf8_lossy(&bytes);
    let name = path.file_stem().unwrap_or_default().to_string_lossy().to_string();
    read::read(&text, &name).map_err(|e| format!("{}: {e}", path.display()))
}
