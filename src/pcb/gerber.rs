//! Writing the board out for the maker: one RS-274X Gerber per layer with X2
//! attributes and Altium's file endings, and Excellon drill files.

use crate::pcb::geom::*;
use crate::pcb::model::*;
use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

fn c(v: f64) -> i64 {
    // A coordinate that isn't a sensible number is written as 0 rather than
    // as nonsense; boards are checked for these on the way in.
    if v.is_finite() && v.abs() < 1e6 { (v * 1e6).round() as i64 } else { 0 }
}

struct Gerber {
    head: String,
    body: String,
    apertures: HashMap<String, u32>,
    current: u32,
    next: u32,
}

impl Gerber {
    fn new(function: &str, polarity_negative: bool) -> Gerber {
        let mut head = String::new();
        let _ = writeln!(head, "G04 Written by Coppersmith {}*", env!("CARGO_PKG_VERSION"));
        let _ = writeln!(head, "%TF.GenerationSoftware,Coppersmith,Coppersmith,{}*%", env!("CARGO_PKG_VERSION"));
        let _ = writeln!(head, "%TF.FileFunction,{function}*%");
        let _ = writeln!(head, "%TF.FilePolarity,{}*%", if polarity_negative { "Negative" } else { "Positive" });
        head.push_str("%FSLAX46Y46*%\n%MOMM*%\n%LPD*%\nG75*\nG01*\n");
        Gerber { head, body: String::new(), apertures: HashMap::new(), current: 0, next: 10 }
    }

    fn aperture(&mut self, def: String) {
        let code = match self.apertures.get(&def) {
            Some(code) => *code,
            None => {
                let code = self.next;
                self.next += 1;
                let _ = writeln!(self.head, "%ADD{code}{def}*%");
                self.apertures.insert(def, code);
                code
            }
        };
        if code != self.current {
            let _ = writeln!(self.body, "D{code}*");
            self.current = code;
        }
    }

    fn round(&mut self, d: f64) {
        self.aperture(format!("C,{:.6}", d.max(0.001)));
    }

    fn line(&mut self, a: P, b: P, w: f64) {
        self.round(w);
        let _ = writeln!(self.body, "X{}Y{}D02*", c(a.x), c(a.y));
        let _ = writeln!(self.body, "X{}Y{}D01*", c(b.x), c(b.y));
    }

    /// A true circular arc, drawn with a round aperture.
    fn arc(&mut self, a: P, m: P, b: P, w: f64) {
        let Some((centre, _, _, sweep)) = arc_sweep(a, m, b) else { return self.line(a, b, w) };
        self.round(w);
        let _ = writeln!(self.body, "X{}Y{}D02*", c(a.x), c(a.y));
        let mode = if sweep > 0.0 { "G03" } else { "G02" };
        let _ = writeln!(self.body, "{mode}X{}Y{}I{}J{}D01*", c(b.x), c(b.y), c(centre.x - a.x), c(centre.y - a.y));
        self.body.push_str("G01*\n");
    }

    fn flash(&mut self, at: P) {
        let _ = writeln!(self.body, "X{}Y{}D03*", c(at.x), c(at.y));
    }

    fn region(&mut self, ring: &[P]) {
        if ring.len() < 3 {
            return;
        }
        self.body.push_str("G36*\n");
        let _ = writeln!(self.body, "X{}Y{}D02*", c(ring[0].x), c(ring[0].y));
        for q in ring[1..].iter().chain(std::iter::once(&ring[0])) {
            let _ = writeln!(self.body, "X{}Y{}D01*", c(q.x), c(q.y));
        }
        self.body.push_str("G37*\n");
    }

    /// A blob as a flash where a standard shape fits, or else a region.
    fn blob(&mut self, b: &Blob) {
        match b.core.len() {
            1 => {
                self.round(2.0 * b.r);
                self.flash(b.core[0]);
            }
            2 => self.line(b.core[0], b.core[1], 2.0 * b.r),
            4 if b.r <= 0.0 => {
                let q = &b.core;
                let (e0, e1) = (q[1].sub(q[0]), q[2].sub(q[1]));
                let square_on = e0.x.abs() < 1e-9 || e0.y.abs() < 1e-9;
                if square_on {
                    let (w, h) = (e0.x.abs() + e1.x.abs(), e0.y.abs() + e1.y.abs());
                    self.aperture(format!("R,{:.6}X{:.6}", w, h));
                    self.flash(BBox::of(q).center());
                } else {
                    self.region(q);
                }
            }
            _ => self.region(&b.ring()),
        }
    }

    fn ink(&mut self, ink: Ink) {
        match ink {
            Ink::Line(a, b, w) => self.line(a, b, w),
            Ink::Dot(at, d) => {
                self.round(d);
                self.flash(at);
            }
            Ink::Ring(at, r, w) => {
                let n = circle_steps(r).max(24);
                let pts: Vec<P> = (0..=n)
                    .map(|i| {
                        let t = i as f64 / n as f64 * std::f64::consts::TAU;
                        at.add(p(t.cos(), t.sin()).scale(r))
                    })
                    .collect();
                self.round(w);
                let _ = writeln!(self.body, "X{}Y{}D02*", c(pts[0].x), c(pts[0].y));
                for q in &pts[1..] {
                    let _ = writeln!(self.body, "X{}Y{}D01*", c(q.x), c(q.y));
                }
            }
        }
    }

    fn finish(self) -> String {
        format!("{}{}M02*\n", self.head, self.body)
    }
}

fn safe_name(name: &str) -> String {
    let s: String = name.chars().map(|ch| if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' { ch } else { '_' }).collect();
    if s.is_empty() { "board".into() } else { s }
}

/// Every layer's file name and text.
pub fn files(board: &Board) -> Vec<(String, String)> {
    let base = safe_name(&board.name);
    let r = &board.rules;
    let mut out = Vec::new();
    let layers = board.copper_layers();
    let count = layers.len();

    for (k, &layer) in layers.iter().enumerate() {
        let (ext, function) = match layer {
            TOP => ("GTL".to_string(), format!("Copper,L1,Top")),
            BOTTOM => ("GBL".to_string(), format!("Copper,L{count},Bot")),
            n => (format!("G{n}"), format!("Copper,L{},Inr", k + 1)),
        };
        let mut g = Gerber::new(&function, false);
        for f in &board.footprints {
            for (i, pad) in f.pads.iter().enumerate() {
                if pad.bare_hole() || !(pad.through() || f.copper() == layer) {
                    continue;
                }
                g.blob(&f.pad_blob(i, 0.0));
            }
        }
        for t in board.tracks.iter().filter(|t| t.layer == layer) {
            match t.mid {
                Some(m) => g.arc(t.a, m, t.b, t.w),
                None => g.line(t.a, t.b, t.w),
            }
        }
        for v in &board.vias {
            g.round(v.dia);
            g.flash(v.at);
        }
        out.push((format!("{base}.{ext}"), g.finish()));
    }

    for side in [Side::Top, Side::Bottom] {
        let s = if side == Side::Top { "T" } else { "B" };
        let word = if side == Side::Top { "Top" } else { "Bot" };

        // Solder mask: openings over the pads; vias stay covered.
        let mut g = Gerber::new(&format!("Soldermask,{word}"), true);
        for f in &board.footprints {
            for (i, pad) in f.pads.iter().enumerate() {
                if pad.bare_hole() {
                    g.round(pad.drill + 2.0 * r.mask_margin);
                    g.flash(f.pad_center(i));
                } else if pad.through() || f.side == side {
                    g.blob(&f.pad_blob(i, r.mask_margin));
                }
            }
        }
        out.push((format!("{base}.G{s}S"), g.finish()));

        let mut g = Gerber::new(&format!("Paste,{word}"), false);
        for f in board.footprints.iter().filter(|f| f.side == side) {
            for (i, pad) in f.pads.iter().enumerate() {
                if !pad.through() {
                    g.blob(&f.pad_blob(i, 0.0));
                }
            }
        }
        out.push((format!("{base}.G{s}P"), g.finish()));

        let mut g = Gerber::new(&format!("Legend,{word}"), false);
        for f in &board.footprints {
            for ink in f.art(Layer::Silk(side), r) {
                g.ink(ink);
            }
        }
        out.push((format!("{base}.G{s}O"), g.finish()));
    }

    let mut g = Gerber::new("Profile,NP", false);
    let o = &board.outline;
    for i in 0..o.len() {
        g.line(o[i], o[(i + 1) % o.len()], 0.1);
    }
    out.push((format!("{base}.GKO"), g.finish()));

    // Drill files, plated and plain.
    let mut plated: Vec<(f64, P)> = board.vias.iter().map(|v| (v.drill, v.at)).collect();
    let mut bare: Vec<(f64, P)> = Vec::new();
    for f in &board.footprints {
        for (i, pad) in f.pads.iter().enumerate() {
            if pad.bare_hole() {
                bare.push((pad.drill, f.pad_center(i)));
            } else if pad.through() {
                plated.push((pad.drill, f.pad_center(i)));
            }
        }
    }
    out.push((format!("{base}-PTH.drl"), excellon(&mut plated, true, count)));
    out.push((format!("{base}-NPTH.drl"), excellon(&mut bare, false, count)));
    out
}

fn excellon(holes: &mut [(f64, P)], plated: bool, layers: usize) -> String {
    holes.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.x.total_cmp(&b.1.x)).then(a.1.y.total_cmp(&b.1.y)));
    let mut s = String::from("M48\n");
    let _ = writeln!(s, "; Written by Coppersmith {}", env!("CARGO_PKG_VERSION"));
    let kind = if plated { "PTH" } else { "NPTH" };
    let _ = writeln!(s, "; #@! TF.FileFunction,{},1,{layers},{kind}", if plated { "Plated" } else { "NonPlated" });
    s.push_str("FMAT,2\nMETRIC\n");
    let mut sizes: Vec<f64> = holes.iter().map(|h| (h.0 * 1000.0).round() / 1000.0).collect();
    sizes.dedup();
    for (i, d) in sizes.iter().enumerate() {
        let _ = writeln!(s, "T{}C{:.3}", i + 1, d);
    }
    s.push_str("%\nG90\nG05\n");
    for (i, d) in sizes.iter().enumerate() {
        let _ = writeln!(s, "T{}", i + 1);
        for (_, at) in holes.iter().filter(|h| ((h.0 * 1000.0).round() / 1000.0 - d).abs() < 1e-9) {
            let _ = writeln!(s, "X{:.4}Y{:.4}", at.x, at.y);
        }
    }
    s.push_str("M30\n");
    s
}

pub fn write(board: &Board, dir: &Path) -> Result<Vec<PathBuf>, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{} couldn't be made: {e}", dir.display()))?;
    let mut written = Vec::new();
    for (name, text) in files(board) {
        let path = dir.join(name);
        std::fs::write(&path, text).map_err(|e| format!("{} couldn't be written: {e}", path.display()))?;
        written.push(path);
    }
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sample_board_writes_every_layer() {
        let b = crate::pcb::sample::board();
        let f = files(&b);
        let names: Vec<&str> = f.iter().map(|(n, _)| n.rsplit('.').next().unwrap()).collect();
        for want in ["GTL", "GBL", "GTS", "GBS", "GTP", "GBP", "GTO", "GBO", "GKO", "drl"] {
            assert!(names.contains(&want), "{want} missing from {names:?}");
        }
        for (name, text) in &f {
            if !name.ends_with(".drl") {
                assert!(text.ends_with("M02*\n"), "{name}");
                assert!(text.contains("%FSLAX46Y46*%"));
            }
        }
    }
}
