//! The footprints a board can be built from. The common packages are made
//! here from their datasheet sizes; more can be added as JSON files in
//! ~/.local/share/coppersmith/footprints.

use crate::geom::*;
use crate::model::*;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct FootprintDef {
    pub name: String,
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub description: String,
    /// The letter its parts are named with, like R for resistors.
    #[serde(default = "default_prefix")]
    pub prefix: String,
    pub pads: Vec<Pad>,
    #[serde(default)]
    pub graphics: Vec<Graphic>,
    #[serde(default)]
    pub label_at: P,
    /// Where it came from: "Built in" or a file name.
    #[serde(skip)]
    pub source: String,
}

fn default_prefix() -> String {
    "U".into()
}

impl FootprintDef {
    pub fn instantiate(&self, id: u32, refdes: String, at: P) -> Footprint {
        Footprint {
            id,
            def: self.name.clone(),
            refdes,
            value: String::new(),
            at,
            rot: 0.0,
            side: Side::Top,
            pads: self.pads.iter().cloned().map(|mut pad| {
                pad.net = 0;
                pad
            }).collect(),
            graphics: self.graphics.clone(),
            label_at: self.label_at,
            locked: false,
        }
    }

    pub fn bbox(&self) -> BBox {
        let mut b = BBox::default();
        for pad in &self.pads {
            b.union(&pad.blob_local(0.0).bbox());
        }
        for g in &self.graphics {
            match g {
                Graphic::Line { a, b: e, .. } => {
                    b.add(*a);
                    b.add(*e);
                }
                Graphic::Circle { c, r, .. } => b.union(&BBox::of(&[*c]).grown(*r)),
            }
        }
        b
    }
}

pub struct Library {
    pub defs: Vec<FootprintDef>,
    pub problems: Vec<String>,
}

pub fn user_folder() -> Option<PathBuf> {
    let base = match std::env::var_os("XDG_DATA_HOME").filter(|v| !v.is_empty()) {
        Some(dir) => PathBuf::from(dir),
        None => PathBuf::from(std::env::var_os("HOME")?).join(".local/share"),
    };
    Some(base.join("coppersmith/footprints"))
}

impl Library {
    pub fn load() -> Library {
        let mut defs = built_in();
        let mut problems = Vec::new();
        if let Some(dir) = user_folder()
            && let Ok(entries) = std::fs::read_dir(&dir)
        {
            let mut files: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
            files.sort();
            for path in files.into_iter().filter(|p| p.extension().is_some_and(|e| e == "json")) {
                let name = path.file_name().unwrap_or_default().to_string_lossy().to_string();
                let read = std::fs::read_to_string(&path).map_err(|e| e.to_string()).and_then(|text| {
                    serde_json::from_str::<Vec<FootprintDef>>(&text)
                        .or_else(|_| serde_json::from_str::<FootprintDef>(&text).map(|d| vec![d]))
                        .map_err(|e| e.to_string())
                });
                match read {
                    Ok(list) => {
                        for mut d in list {
                            d.source = name.clone();
                            if d.category.is_empty() {
                                d.category = "My footprints".into();
                            }
                            // A file can replace a built-in footprint by using its name.
                            defs.retain(|old| old.name != d.name);
                            defs.push(d);
                        }
                    }
                    Err(e) => problems.push(format!("{name}: {e}")),
                }
            }
        }
        Library { defs, problems }
    }

    #[allow(dead_code)]
    pub fn find(&self, name: &str) -> Option<&FootprintDef> {
        self.defs.iter().find(|d| d.name == name)
    }

    pub fn categories(&self) -> Vec<String> {
        let mut c: Vec<String> = Vec::new();
        for d in &self.defs {
            if !c.contains(&d.category) {
                c.push(d.category.clone());
            }
        }
        c
    }
}

// ---------------------------------------------------------------------------
// The built-in packages.

const SILK: f64 = 0.12;
const FAB: f64 = 0.1;
const COURT: f64 = 0.05;

fn pad(num: impl ToString, at: P, size: P, shape: PadShape, drill: f64) -> Pad {
    Pad { num: num.to_string(), at, size, rot: 0.0, shape, drill, net: 0 }
}

fn smd(num: impl ToString, at: P, size: P) -> Pad {
    pad(num, at, size, PadShape::RoundRect(0.25), 0.0)
}

fn line(a: P, b: P, w: f64, layer: Layer) -> Graphic {
    Graphic::Line { a, b, w, layer }
}

fn rect_lines(min: P, max: P, w: f64, layer: Layer) -> Vec<Graphic> {
    let c = [p(min.x, min.y), p(max.x, min.y), p(max.x, max.y), p(min.x, max.y)];
    (0..4).map(|i| line(c[i], c[(i + 1) % 4], w, layer)).collect()
}

/// The body on the assembly layer and a courtyard round everything.
fn frame(pads: &[Pad], body: P, extra: &mut Vec<Graphic>) -> P {
    let mut g = rect_lines(p(-body.x / 2.0, -body.y / 2.0), p(body.x / 2.0, body.y / 2.0), FAB, Layer::Fab(Side::Top));
    let mut b = BBox::of(&[p(-body.x / 2.0, -body.y / 2.0), p(body.x / 2.0, body.y / 2.0)]);
    for pd in pads {
        b.union(&pd.blob_local(0.0).bbox());
    }
    let c = b.grown(0.25);
    let c = BBox { min: c.min.snap(0.05), max: c.max.snap(0.05) };
    g.extend(rect_lines(c.min, c.max, COURT, Layer::Court(Side::Top)));
    g.append(extra);
    *extra = g;
    p(0.0, c.max.y + 0.8)
}

fn def(name: &str, category: &str, prefix: &str, description: String, pads: Vec<Pad>, body: P, mut g: Vec<Graphic>) -> FootprintDef {
    let label_at = frame(&pads, body, &mut g);
    FootprintDef {
        name: name.into(),
        category: category.into(),
        description,
        prefix: prefix.into(),
        pads,
        graphics: g,
        label_at,
        source: "Built in".into(),
    }
}

/// Two-pad chip parts: (size code, metric code, pad w, pad h, pad centre x, body l, body w).
const CHIPS: [(&str, &str, f64, f64, f64, f64, f64); 4] = [
    ("0402", "1005", 0.59, 0.64, 0.485, 1.0, 0.5),
    ("0603", "1608", 0.8, 0.95, 0.825, 1.6, 0.8),
    ("0805", "2012", 1.025, 1.4, 0.9125, 2.0, 1.25),
    ("1206", "3216", 1.125, 1.75, 1.4625, 3.2, 1.6),
];

fn chip(kind: &str, prefix: &str, code: &str, metric: &str, pw: f64, ph: f64, px: f64, bl: f64, bw: f64) -> FootprintDef {
    let pads = vec![smd(1, p(-px, 0.0), p(pw, ph)), smd(2, p(px, 0.0), p(pw, ph))];
    let mut g = Vec::new();
    let sy = (bw / 2.0).max(ph / 2.0) + 0.15;
    let sx = (px - pw / 2.0 - 0.2).max(0.0);
    if sx > 0.05 {
        g.push(line(p(-sx, sy), p(sx, sy), SILK, Layer::Silk(Side::Top)));
        g.push(line(p(-sx, -sy), p(sx, -sy), SILK, Layer::Silk(Side::Top)));
    }
    if prefix == "D" {
        // A bar at the cathode, pad 1.
        let x = -(px + pw / 2.0 + 0.25);
        g.push(line(p(x, ph / 2.0), p(x, -ph / 2.0), SILK, Layer::Silk(Side::Top)));
    }
    def(
        &format!("{kind}_{code}"),
        "Resistors, capacitors, LEDs",
        prefix,
        format!("{} {code} ({metric} metric), surface mount", match prefix {
            "R" => "Resistor",
            "C" => "Capacitor",
            "D" => "LED",
            _ => "Inductor",
        }),
        pads,
        p(bl, bw),
        g,
    )
}

/// Pins down the left side then up the right, pin 1 at the top left.
fn dual_row(n: usize, pitch: f64, row: f64, size: P, through: Option<f64>) -> Vec<Pad> {
    let half = n / 2;
    let top = (half as f64 - 1.0) * pitch / 2.0;
    (0..n)
        .map(|i| {
            let (x, y) = if i < half { (-row / 2.0, top - i as f64 * pitch) } else { (row / 2.0, -top + (i - half) as f64 * pitch) };
            match through {
                Some(drill) => {
                    let shape = if i == 0 { PadShape::Rect } else { PadShape::Oval };
                    pad(i + 1, p(x, y), size, shape, drill)
                }
                None => smd(i + 1, p(x, y), size),
            }
        })
        .collect()
}

fn ic_silk(body: P, pads: &[Pad]) -> Vec<Graphic> {
    // Lines along the top and bottom of the body, clear of the pads, and a
    // dot by pin 1.
    let (hx, hy) = (body.x / 2.0 + 0.06, body.y / 2.0 + 0.06);
    let mut g = vec![
        line(p(-hx, hy), p(hx, hy), SILK, Layer::Silk(Side::Top)),
        line(p(-hx, -hy), p(hx, -hy), SILK, Layer::Silk(Side::Top)),
    ];
    if let Some(first) = pads.first() {
        let edge = first.at.x - first.size.x / 2.0 - 0.35;
        g.push(Graphic::Circle { c: p(edge, first.at.y), r: 0.15, w: 0.0, layer: Layer::Silk(Side::Top), fill: true });
    }
    // The bevel on the assembly drawing.
    g.push(line(p(-body.x / 2.0, body.y / 2.0 - 0.8), p(-body.x / 2.0 + 0.8, body.y / 2.0), FAB, Layer::Fab(Side::Top)));
    g
}

fn soic(n: usize) -> FootprintDef {
    let pads = dual_row(n, 1.27, 4.95, p(1.95, 0.6), None);
    let body = p(3.9, (n / 2) as f64 * 1.27 - 0.2);
    let g = ic_silk(p(3.9, body.y), &pads);
    def(&format!("SOIC-{n}"), "Integrated circuits", "U", format!("SOIC, {n} pins, 1.27 mm pitch, 3.9 mm body"), pads, body, g)
}

fn tssop(n: usize) -> FootprintDef {
    let pads = dual_row(n, 0.65, 5.675, p(1.475, 0.4), None);
    let body = p(4.4, ((n / 2) as f64 * 0.65 + 0.2).max(3.0));
    let g = ic_silk(body, &pads);
    def(&format!("TSSOP-{n}"), "Integrated circuits", "U", format!("TSSOP, {n} pins, 0.65 mm pitch, 4.4 mm body"), pads, body, g)
}

fn dip(n: usize) -> FootprintDef {
    let pads = dual_row(n, 2.54, 7.62, p(1.6, 1.6), Some(0.8));
    let body = p(6.35, (n / 2) as f64 * 2.54);
    let g = ic_silk(p(5.2, body.y), &pads);
    def(&format!("DIP-{n}"), "Integrated circuits", "U", format!("DIP, {n} pins, 7.62 mm rows, through hole"), pads, body, g)
}

fn qfn(n: usize, side: f64, pitch: f64, ep: f64) -> FootprintDef {
    let per = n / 4;
    let span = (per as f64 - 1.0) * pitch / 2.0;
    let reach = side / 2.0 + 0.1;
    let (pl, pw) = (0.85, pitch * 0.5);
    let mut pads = Vec::new();
    for i in 0..n {
        let k = i / per;
        let t = span - (i % per) as f64 * pitch;
        let (at, size) = match k {
            0 => (p(-reach + pl / 2.0 - 0.2, t), p(pl, pw)),
            1 => (p(-t, -reach + pl / 2.0 - 0.2), p(pw, pl)),
            2 => (p(reach - pl / 2.0 + 0.2, -t), p(pl, pw)),
            _ => (p(t, reach - pl / 2.0 + 0.2), p(pw, pl)),
        };
        pads.push(smd(i + 1, at, size));
    }
    pads.push(pad(n + 1, p(0.0, 0.0), p(ep, ep), PadShape::RoundRect(0.05), 0.0));
    let h = side / 2.0;
    let c = h - (span + pw);
    let mut g = Vec::new();
    for (sx, sy) in [(-1.0, 1.0), (1.0, 1.0), (1.0, -1.0), (-1.0, -1.0)] {
        let corner = p(sx * (h + 0.1), sy * (h + 0.1));
        g.push(line(corner, p(corner.x - sx * (c - 0.1).max(0.1), corner.y), SILK, Layer::Silk(Side::Top)));
        if !(sx < 0.0 && sy > 0.0) {
            g.push(line(corner, p(corner.x, corner.y - sy * (c - 0.1).max(0.1)), SILK, Layer::Silk(Side::Top)));
        }
    }
    g.push(Graphic::Circle { c: p(-h - 0.45, span + 0.45), r: 0.15, w: 0.0, layer: Layer::Silk(Side::Top), fill: true });
    def(
        &format!("QFN-{n}_{side}x{side}"),
        "Integrated circuits",
        "U",
        format!("QFN, {n} pins and a centre pad, {pitch} mm pitch, {side} × {side} mm"),
        pads,
        p(side, side),
        g,
    )
}

fn sot23(n: usize) -> FootprintDef {
    let (x, size) = (1.1375, p(1.325, 0.6));
    let pads = match n {
        3 => vec![smd(1, p(-x, 0.95), size), smd(2, p(-x, -0.95), size), smd(3, p(x, 0.0), size)],
        _ => vec![
            smd(1, p(-x, 0.95), size),
            smd(2, p(-x, 0.0), size),
            smd(3, p(-x, -0.95), size),
            smd(4, p(x, -0.95), size),
            smd(5, p(x, 0.95), size),
        ],
    };
    let g = vec![
        line(p(-0.3, 1.56), p(0.3, 1.56), SILK, Layer::Silk(Side::Top)),
        line(p(-0.3, -1.56), p(0.3, -1.56), SILK, Layer::Silk(Side::Top)),
    ];
    let name = if n == 3 { "SOT-23".to_string() } else { format!("SOT-23-{n}") };
    let prefix = if n == 3 { "Q" } else { "U" };
    def(&name, "Transistors and diodes", prefix, format!("SOT-23, {n} pins"), pads, p(1.6, 2.9), g)
}

fn sod123() -> FootprintDef {
    let pads = vec![smd(1, p(-1.65, 0.0), p(0.9, 1.2)), smd(2, p(1.65, 0.0), p(0.9, 1.2))];
    let g = vec![
        line(p(-2.4, 0.9), p(1.0, 0.9), SILK, Layer::Silk(Side::Top)),
        line(p(-2.4, -0.9), p(1.0, -0.9), SILK, Layer::Silk(Side::Top)),
        line(p(-2.4, 0.9), p(-2.4, -0.9), SILK, Layer::Silk(Side::Top)),
    ];
    def("SOD-123", "Transistors and diodes", "D", "Diode, SOD-123, cathode is pad 1".into(), pads, p(2.7, 1.6), g)
}

fn to220() -> FootprintDef {
    let pads: Vec<Pad> = (0..3)
        .map(|i| {
            let shape = if i == 0 { PadShape::Rect } else { PadShape::Oval };
            pad(i + 1, p((i as f64 - 1.0) * 2.54, 0.0), p(1.9, 2.4), shape, 1.1)
        })
        .collect();
    let mut g = rect_lines(p(-5.2, -1.9), p(5.2, 2.8), SILK, Layer::Silk(Side::Top));
    g.push(line(p(-5.2, 1.6), p(5.2, 1.6), SILK, Layer::Silk(Side::Top)));
    def("TO-220-3", "Transistors and diodes", "Q", "TO-220, 3 pins, standing up".into(), pads, p(10.0, 4.4), g)
}

fn header(rows: usize, n: usize) -> FootprintDef {
    let mut pads = Vec::new();
    for i in 0..n {
        for r in 0..rows {
            let num = i * rows + r + 1;
            let shape = if num == 1 { PadShape::Rect } else { PadShape::Circle };
            pads.push(pad(num, p(r as f64 * 2.54, -(i as f64) * 2.54), p(1.7, 1.7), shape, 1.0));
        }
    }
    // Centred on the middle of the pins.
    let mid = p((rows as f64 - 1.0) * 1.27, -(n as f64 - 1.0) * 1.27);
    for pd in &mut pads {
        pd.at = pd.at.sub(mid).rounded();
    }
    let (hw, hh) = (rows as f64 * 1.27 + 0.06, n as f64 * 1.27 + 0.06);
    let g = rect_lines(p(-hw, -hh), p(hw, hh), SILK, Layer::Silk(Side::Top));
    def(
        &format!("PinHeader_{rows}x{n:02}"),
        "Connectors",
        "J",
        format!("Pin header, {rows} × {n}, 2.54 mm pitch"),
        pads,
        p(rows as f64 * 2.54, n as f64 * 2.54),
        g,
    )
}

fn terminal(n: usize) -> FootprintDef {
    let pads: Vec<Pad> = (0..n)
        .map(|i| {
            let shape = if i == 0 { PadShape::Rect } else { PadShape::Circle };
            pad(i + 1, p((i as f64 - (n as f64 - 1.0) / 2.0) * 5.08, 0.0), p(2.6, 2.6), shape, 1.3)
        })
        .collect();
    let hw = n as f64 * 2.54 + 0.06;
    let g = rect_lines(p(-hw, -3.4), p(hw, 4.2), SILK, Layer::Silk(Side::Top));
    def(&format!("Terminal_1x{n:02}_P5.08"), "Connectors", "J", format!("Screw terminal, {n} ways, 5.08 mm"), pads, p(n as f64 * 5.08, 7.6), g)
}

fn tact_switch() -> FootprintDef {
    let pads = vec![
        pad(1, p(-3.25, 2.25), p(2.0, 2.0), PadShape::Circle, 1.1),
        pad(2, p(3.25, 2.25), p(2.0, 2.0), PadShape::Circle, 1.1),
        pad(1, p(-3.25, -2.25), p(2.0, 2.0), PadShape::Circle, 1.1),
        pad(2, p(3.25, -2.25), p(2.0, 2.0), PadShape::Circle, 1.1),
    ];
    let mut g = rect_lines(p(-3.0, -3.0), p(3.0, 3.0), SILK, Layer::Silk(Side::Top));
    g.push(Graphic::Circle { c: p(0.0, 0.0), r: 1.75, w: SILK, layer: Layer::Silk(Side::Top), fill: false });
    def("Switch_Tactile_6x6", "Switches", "SW", "Push button, 6 × 6 mm, through hole".into(), pads, p(6.0, 6.0), g)
}

fn crystal_3225() -> FootprintDef {
    let size = p(1.4, 1.15);
    let pads = vec![
        smd(1, p(-1.1, -0.8), size),
        smd(2, p(1.1, -0.8), size),
        smd(3, p(1.1, 0.8), size),
        smd(4, p(-1.1, 0.8), size),
    ];
    let g = vec![Graphic::Circle { c: p(-2.15, -0.8), r: 0.15, w: 0.0, layer: Layer::Silk(Side::Top), fill: true }];
    def("Crystal_3225-4", "Crystals", "Y", "Crystal, 3.2 × 2.5 mm, 4 pads".into(), pads, p(3.2, 2.5), g)
}

fn mounting_hole(name: &str, drill: f64, ring: f64) -> FootprintDef {
    let size = if ring > 0.0 { ring } else { drill };
    let pads = vec![pad(1, p(0.0, 0.0), p(size, size), PadShape::Circle, drill)];
    let g = vec![Graphic::Circle { c: p(0.0, 0.0), r: size / 2.0 + 0.3, w: COURT, layer: Layer::Court(Side::Top), fill: false }];
    let what = if ring > 0.0 { format!("plated, {ring} mm pad") } else { "plain hole".into() };
    let mut d = def(name, "Mechanical", "H", format!("Mounting hole, {drill} mm, {what}"), pads, p(0.0, 0.0), g);
    // The frame round a hole is its circle; drop the empty body box.
    d.graphics.retain(|g| !matches!(g, Graphic::Line { layer: Layer::Fab(_), .. }));
    d
}

fn testpoint() -> FootprintDef {
    let pads = vec![pad(1, p(0.0, 0.0), p(1.5, 1.5), PadShape::Circle, 0.0)];
    def("TestPoint_1.5mm", "Mechanical", "TP", "Test pad, 1.5 mm, surface".into(), pads, p(0.0, 0.0), Vec::new())
}

pub fn built_in() -> Vec<FootprintDef> {
    let mut v = Vec::new();
    for (code, metric, pw, ph, px, bl, bw) in CHIPS {
        v.push(chip("R", "R", code, metric, pw, ph, px, bl, bw));
    }
    for (code, metric, pw, ph, px, bl, bw) in CHIPS {
        v.push(chip("C", "C", code, metric, pw, ph, px, bl, bw));
    }
    for (code, metric, pw, ph, px, bl, bw) in &CHIPS[1..3] {
        v.push(chip("LED", "D", code, metric, *pw, *ph, *px, *bl, *bw));
    }
    for n in [8, 14, 16] {
        v.push(soic(n));
    }
    for n in [14, 16, 20, 28] {
        v.push(tssop(n));
    }
    v.push(qfn(16, 3.0, 0.5, 1.7));
    v.push(qfn(32, 5.0, 0.5, 3.1));
    v.push(qfn(48, 7.0, 0.5, 5.15));
    for n in [8, 14, 16] {
        v.push(dip(n));
    }
    v.push(sot23(3));
    v.push(sot23(5));
    v.push(sod123());
    v.push(to220());
    for n in [2, 3, 4, 6, 8, 10] {
        v.push(header(1, n));
    }
    for n in [3, 5, 10, 20] {
        v.push(header(2, n));
    }
    v.push(terminal(2));
    v.push(terminal(3));
    v.push(tact_switch());
    v.push(crystal_3225());
    v.push(mounting_hole("MountingHole_M3", 3.2, 0.0));
    v.push(mounting_hole("MountingHole_M3_Pad", 3.2, 6.0));
    v.push(mounting_hole("MountingHole_M2.5", 2.7, 0.0));
    v.push(testpoint());
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_unique_and_pads_numbered() {
        let defs = built_in();
        let mut names: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), defs.len());
        let soic8 = defs.iter().find(|d| d.name == "SOIC-8").unwrap();
        assert_eq!(soic8.pads.len(), 8);
        // Pin 1 top left, pin 8 top right.
        assert!(soic8.pads[0].at.x < 0.0 && soic8.pads[0].at.y > 0.0);
        assert!(soic8.pads[7].at.x > 0.0 && soic8.pads[7].at.y > 0.0);
    }

    #[test]
    fn soic_pads_keep_clear_of_each_other() {
        let d = built_in().into_iter().find(|d| d.name == "SOIC-8").unwrap();
        let gap = d.pads[0].blob_local(0.0).gap(&d.pads[1].blob_local(0.0)).0;
        assert!((gap - 0.67).abs() < 1e-6, "{gap}");
    }
}
