//! The schematic editor's window: symbols to place on the left, the sheet in
//! the middle, the details of what's picked and the checks on the right.
//! What edits do is in `pcb::schematic`; this is drawing and pointing.

use crate::pcb::altium::schlib::{Drawing, Symbol};
use crate::pcb::geom::*;
use crate::pcb::schematic::{LabelKind, Paper, Placed, PortIo, PowerStyle, Schematic, Sel, elbow, transform};
use crate::ui::dock::EditorCmd;
use crate::ui::icons::Icon;
use crate::ui::panels::{card, chip, field, segmented, small_icon};
use crate::ui::theme::{self, ACCENT, DIM, FAINT, FG, GREEN, YELLOW, semibold};
use crate::ui::view::Tr;
use eframe::egui::{self, Align, Align2, Color32, FontId, Layout, Panel, PointerButton, Rect, RichText, ScrollArea, Sense, Stroke, Ui, pos2, vec2};
use std::path::{Path, PathBuf};
use std::sync::mpsc;

const GRID: f64 = 2.54;
/// How far the sheet's border is in from its edge.
const BORDER: f64 = 5.08;
// The sheet is cream paper with ink on it, each kind of thing its own hue so a
// sheet reads at a glance: wires in near-black ink, parts in brick over a
// see-through butter, pins in a steady blue, net labels in teal, supplies in
// wine, ports in steel blue, a clear blue for what's picked.
const PAPER: Color32 = theme::rgb(0xf8_f4_e8);
/// The desk the sheet of paper lies on.
const DESK: Color32 = theme::rgb(0xcf_c8_b7);
const DOTS: Color32 = theme::rgb(0xe1_d9_c5);
const WIRE: Color32 = theme::rgb(0x1a_22_36);
const BODY: Color32 = theme::rgb(0x9a_3b_24);
const PIN: Color32 = theme::rgb(0x2f_66_b3);
const PIN_NUMBER: Color32 = theme::rgb(0x5f_73_96);
const TEXT: Color32 = theme::rgb(0x25_22_1e);
const TEXT_DIM: Color32 = theme::rgb(0x6a_64_58);
/// Designators: the part's own brown.
const DESIGNATOR: Color32 = theme::rgb(0x6e_2d_18);
const NET: Color32 = theme::rgb(0x0d_6a_73);
const POWER: Color32 = theme::rgb(0xb3_29_3a);
const PORT: Color32 = theme::rgb(0x1f_5f_8b);
const PICK: Color32 = theme::rgb(0x17_68_e0);
const LOOSE: Color32 = theme::rgb(0xd3_2f_2f);

// Sizes on the sheet, in millimetres, so a symbol's text, strokes and markers
// grow and shrink together as the sheet is zoomed. (Text is the height of the em.)
const EM_PIN_NAME: f64 = 1.45;
const EM_PIN_NUMBER: f64 = 1.15;
const EM_DESIGNATOR: f64 = 1.6;
const EM_VALUE: f64 = 1.45;
const EM_NOTE: f64 = 1.35;
const EM_LABEL: f64 = 1.6;
const WIRE_MM: f64 = 0.2;
const BODY_MM: f64 = 0.17;
const PIN_MM: f64 = 0.15;

/// A length on the sheet in pixels, kept at least `floor` so it's still there zoomed out.
fn mm(tr: &Tr, v: f64, floor: f32) -> f32 {
    ((v * tr.s) as f32).max(floor)
}

/// A line of words over the sheet, on a little paper of its own so it reads over the drawing.
fn note(painter: &egui::Painter, at: egui::Pos2, align: Align2, text: &str, font: FontId, color: Color32) {
    let galley = painter.layout_no_wrap(text.to_string(), font, color);
    let rect = align.anchor_size(at, galley.size()).expand2(vec2(6.0, 3.0));
    painter.rect_filled(rect, 5.0, PAPER.gamma_multiply(0.92));
    painter.galley(rect.min + vec2(6.0, 3.0), galley, color);
}

/// What a symbol's closed shapes are filled with: a butter that lets the paper show through.
fn body_fill() -> Color32 {
    Color32::from_rgba_unmultiplied(243, 206, 110, 105)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tool {
    Select,
    Place,
    Wire,
    Net,
    Power,
}

enum Dialog {
    Open(PathBuf),
    SaveAs(PathBuf),
    AddSymbols(PathBuf),
}

#[derive(Default)]
pub struct Outcome {
    pub notice: Option<String>,
    /// A tool was chosen that has its list in the left panel: bring that forward.
    pub show_symbols: bool,
}

pub struct SchWindow {
    pub sch: Schematic,
    pub path: Option<PathBuf>,
    pub dirty: bool,
    undo: Vec<Schematic>,
    redo: Vec<Schematic>,
    tool: Tool,
    sel: Option<Sel>,
    center: P,
    scale: f64,
    fit: bool,
    hover: Option<P>,
    /// The libraries the project and the tabs open offer, and any added by hand.
    libraries: Vec<SymbolLibrary>,
    added: Vec<SymbolLibrary>,
    /// Which one the list shows (None: all of them).
    library: Option<usize>,
    panel: SymPanel,
    /// What the Net label and Power tools put down.
    marker: Marker,
    filter: String,
    /// The symbol being placed, how far turned, and whether mirrored.
    ghost: Option<(Symbol, u8, bool)>,
    /// A wire being drawn: the corners put down so far.
    wire: Vec<P>,
    x_first: bool,
    /// The name the next label or power port gets.
    name: String,
    grab: Option<(Sel, P, P)>,
    /// What reading a file left out.
    notes: Vec<String>,
    tx: mpsc::Sender<Dialog>,
    rx: mpsc::Receiver<Dialog>,
}

/// A library of symbols to place parts from: one of the project's, or one open in a tab.
#[derive(Clone, Debug)]
pub struct SymbolLibrary {
    pub name: String,
    pub path: Option<PathBuf>,
    pub symbols: Vec<Symbol>,
}

/// What the left panel is showing: the markers that are always there (power,
/// nets, ports, buses and pairs), or the symbols of a library.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SymPanel {
    Markers,
    Parts,
}

/// The standard things that aren't parts: names on wires, supplies, ports.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Marker {
    Net,
    /// A name for a bus, `NAME[0..7]`.
    Bus,
    /// A pair of net labels, `NAME_P` and `NAME_N`, a grid apart.
    Pair,
    Power(PowerStyle),
    Port(PortIo),
}

/// The supplies in the markers panel: how each is drawn, and its name.
const SUPPLIES: &[(PowerStyle, &str)] = &[
    (PowerStyle::Ground, "GND"),
    (PowerStyle::SignalGround, "AGND"),
    (PowerStyle::Earth, "PE"),
    (PowerStyle::Bar, "VCC"),
    (PowerStyle::Bar, "+3V3"),
    (PowerStyle::Bar, "+5V"),
    (PowerStyle::Bar, "+12V"),
    (PowerStyle::Bar, "-12V"),
    (PowerStyle::Circle, "VDD"),
    (PowerStyle::Circle, "VBAT"),
    (PowerStyle::Arrow, "VIN"),
    (PowerStyle::Wave, "VAC"),
];

impl SchWindow {
    pub fn new() -> SchWindow {
        let (tx, rx) = mpsc::channel();
        let w = SchWindow {
            sch: Schematic::default(),
            path: None,
            dirty: false,
            undo: Vec::new(),
            redo: Vec::new(),
            tool: Tool::Select,
            sel: None,
            center: p(30.0, 20.0),
            scale: 11.0,
            fit: true,
            hover: None,
            libraries: Vec::new(),
            added: Vec::new(),
            library: None,
            panel: SymPanel::Markers,
            marker: Marker::Net,
            filter: String::new(),
            ghost: None,
            wire: Vec::new(),
            x_first: true,
            name: "GND".into(),
            grab: None,
            notes: Vec::new(),
            tx,
            rx,
        };
        w
    }

    /// The libraries the project and the open tabs offer. A library added by hand
    /// (the + in the panel) stays beside them.
    pub fn set_libraries(&mut self, libs: Vec<SymbolLibrary>) {
        let keep = self.library_names().get(self.library.unwrap_or(usize::MAX)).cloned();
        self.libraries = libs;
        self.library = keep.and_then(|n| self.library_names().iter().position(|x| *x == n));
    }

    /// Every library on offer: the project's, then those added by hand.
    fn all_libraries(&self) -> impl Iterator<Item = &SymbolLibrary> {
        self.libraries.iter().chain(self.added.iter())
    }

    fn library_names(&self) -> Vec<String> {
        self.all_libraries().map(|l| l.name.clone()).collect()
    }

    fn is_altium(path: &Path) -> bool {
        path.extension().is_some_and(|e| e.eq_ignore_ascii_case("schdoc") || e.eq_ignore_ascii_case("csschdoc"))
    }

    pub fn open_file(&mut self, path: &Path) -> String {
        // A KiCad sheet is read in and kept as a new sheet: Save asks where, so the KiCad file is never written over.
        if crate::pcb::kicad::is_sheet(path) {
            let name = path.file_stem().unwrap_or_default().to_string_lossy().to_string();
            return match std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display())).and_then(|t| crate::pcb::kicad::sch::read_sheet(&t, &name).map_err(|e| format!("{}: {e}", path.display()))) {
                Ok((s, notes)) => {
                    self.notes = notes;
                    self.sch = s;
                    self.path = None;
                    self.undo.clear();
                    self.redo.clear();
                    self.dirty = true;
                    self.sel = None;
                    self.fit = true;
                    format!("Read {}; save it as a SchDoc or a Coppersmith sheet to keep it", path.display())
                }
                Err(e) => e,
            };
        }
        // Anything that isn't Coppersmith's own is read as an Altium sheet, so a renamed one still opens.
        let ours = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("cssch") || e.eq_ignore_ascii_case("json"));
        let read = if Self::is_altium(path) || !ours {
            std::fs::read(path).map_err(|e| format!("{}: {e}", path.display())).and_then(|b| {
                let name = path.file_stem().unwrap_or_default().to_string_lossy().to_string();
                crate::pcb::altium::schsheet::read(&b, &name).map_err(|e| format!("{}: {e}", path.display()))
            })
        } else {
            std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display())).and_then(|t| Schematic::load(&t)).map(|s| (s, Vec::new()))
        };
        match read {
            Ok((s, notes)) => {
                if !notes.is_empty() {
                    self.notes = notes;
                }
                self.sch = s;
                self.path = Some(path.to_path_buf());
                self.undo.clear();
                self.redo.clear();
                self.dirty = false;
                self.sel = None;
                self.fit = true;
                format!("Opened {}", path.display())
            }
            Err(e) => e,
        }
    }

    // -- Edits -------------------------------------------------------------------------

    fn remember(&mut self) {
        self.undo.push(self.sch.clone());
        self.redo.clear();
        self.dirty = true;
    }

    fn undo(&mut self) {
        if let Some(s) = self.undo.pop() {
            self.redo.push(std::mem::replace(&mut self.sch, s));
            self.sel = None;
            self.dirty = true;
        }
    }

    fn redo(&mut self) {
        if let Some(s) = self.redo.pop() {
            self.undo.push(std::mem::replace(&mut self.sch, s));
            self.sel = None;
            self.dirty = true;
        }
    }

    fn snap(&self, q: P) -> P {
        q.snap(GRID)
    }

    /// Does a wire end at q anywhere that joins it to something (so drawing stops there)?
    fn is_joint(&self, q: P) -> bool {
        self.sch.parts.iter().any(|p| p.pins().iter().any(|s| s.tip.same(q)))
            || self.sch.labels.iter().any(|l| l.at.same(q))
            || self.sch.wires.iter().any(|w| w.pts.windows(2).any(|s| dist_to_segment(q, s[0], s[1]).0 < 1e-3))
    }

    fn finish_wire(&mut self) {
        let pts = std::mem::take(&mut self.wire);
        if pts.len() >= 2 {
            self.remember();
            self.sch.add_wire(pts);
        }
    }

    /// A label's name for the next one: a trailing number counts up.
    fn bump(name: &str) -> String {
        let digits: String = name.chars().rev().take_while(|c| c.is_ascii_digit()).collect::<Vec<_>>().into_iter().rev().collect();
        match digits.parse::<u64>() {
            Ok(n) if !digits.is_empty() => format!("{}{:0w$}", &name[..name.len() - digits.len()], n + 1, w = digits.len()),
            _ => name.to_string(),
        }
    }

    fn click(&mut self, q: P) {
        let q = self.snap(q);
        match self.tool {
            Tool::Select => {}
            Tool::Place => {
                if let Some((sym, turns, mirror)) = self.ghost.clone() {
                    self.remember();
                    let id = self.sch.place(&sym, q);
                    if let Some(p) = self.sch.part_mut(id) {
                        p.turns = turns;
                        p.mirror = mirror;
                    }
                    self.sch.fix_junctions();
                    self.sel = Some(Sel::Part(id));
                }
            }
            Tool::Wire => {
                if self.wire.is_empty() {
                    self.wire.push(q);
                } else {
                    let last = *self.wire.last().unwrap();
                    let seg = elbow(last, q, self.x_first);
                    self.wire.extend(seg.into_iter().skip(1));
                    if self.is_joint(q) {
                        self.finish_wire();
                    }
                }
            }
            Tool::Net | Tool::Power => {
                if self.name.trim().is_empty() {
                    return;
                }
                self.remember();
                let name = self.name.trim().to_string();
                let (plain, io) = (PowerStyle::default(), PortIo::default());
                let id = match self.marker {
                    Marker::Net => self.sch.add_marker(q, &name, LabelKind::Net, plain, io),
                    Marker::Bus => self.sch.add_marker(q, &name, LabelKind::Net, plain, io),
                    // Both halves, a grid apart: the positive where it was clicked.
                    Marker::Pair => {
                        let first = self.sch.add_marker(q, &format!("{name}_P"), LabelKind::Net, plain, io);
                        self.sch.add_marker(q.sub(p(0.0, GRID)), &format!("{name}_N"), LabelKind::Net, plain, io);
                        first
                    }
                    Marker::Power(style) => self.sch.add_marker(q, &name, LabelKind::Power, style, io),
                    Marker::Port(io) => self.sch.add_marker(q, &name, LabelKind::Port, plain, io),
                };
                self.sel = Some(Sel::Label(id));
                // The next one counts on: a bus keeps its range, a supply its name.
                if matches!(self.marker, Marker::Net | Marker::Pair | Marker::Port(_)) {
                    self.name = Self::bump(&self.name);
                }
            }
        }
    }

    /// Picks a tool. Those that put something down bring their list in the left panel forward.
    fn choose_tool(&mut self, tool: Tool, out: &mut Outcome) {
        self.wire.clear();
        self.tool = tool;
        if tool != Tool::Place {
            self.ghost = None;
        }
        match tool {
            Tool::Place => {
                self.panel = SymPanel::Parts;
                out.show_symbols = true;
            }
            Tool::Net => {
                if !matches!(self.marker, Marker::Net | Marker::Bus | Marker::Pair | Marker::Port(_)) {
                    self.arm(Marker::Net, "NET1");
                }
                self.panel = SymPanel::Markers;
                out.show_symbols = true;
            }
            Tool::Power => {
                if !matches!(self.marker, Marker::Power(_)) {
                    self.arm(Marker::Power(PowerStyle::Ground), "GND");
                }
                self.panel = SymPanel::Markers;
                out.show_symbols = true;
            }
            Tool::Select | Tool::Wire => {}
        }
    }

    /// Gets a marker ready to put down, with the name it starts with.
    fn arm(&mut self, marker: Marker, name: &str) {
        self.wire.clear();
        self.ghost = None;
        self.marker = marker;
        self.name = name.to_string();
        self.tool = if matches!(marker, Marker::Power(_)) { Tool::Power } else { Tool::Net };
    }

    fn stop(&mut self) {
        if !self.wire.is_empty() {
            self.finish_wire();
        } else if self.tool != Tool::Select {
            self.tool = Tool::Select;
            self.ghost = None;
        } else {
            self.sel = None;
        }
    }

    // -- Files -------------------------------------------------------------------------

    fn ask_open(&self, ctx: &egui::Context) {
        let (tx, ctx) = (self.tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            if let Some(path) = rfd::FileDialog::new().set_title("Open a schematic").add_filter("Schematics", &crate::pcb::altium::with_cases(&["cssch", "json", "SchDoc", "CSSchDoc", "kicad_sch"])).add_filter("KiCad sheet", &crate::pcb::altium::with_cases(&["kicad_sch"])).add_filter("Altium or Circuit Studio sheet", &crate::pcb::altium::with_cases(&["SchDoc", "CSSchDoc"])).add_filter("Coppersmith schematic", &["cssch", "json"]).pick_file() {
                let _ = tx.send(Dialog::Open(path));
                ctx.request_repaint();
            }
        });
    }

    fn ask_save_as(&self, ctx: &egui::Context) {
        let (tx, ctx, name) = (self.tx.clone(), ctx.clone(), format!("{}.cssch", crate::ui::app::copy_stem(&self.sch.name)));
        std::thread::spawn(move || {
            if let Some(path) = rfd::FileDialog::new().set_title("Save the schematic").add_filter("Coppersmith schematic", &["cssch"]).add_filter("Altium sheet", &["SchDoc"]).set_file_name(name).save_file() {
                let _ = tx.send(Dialog::SaveAs(path));
                ctx.request_repaint();
            }
        });
    }

    fn ask_add_symbols(&self, ctx: &egui::Context) {
        let (tx, ctx) = (self.tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            if let Some(path) = rfd::FileDialog::new().set_title("Open a symbol library").add_filter("Symbol library", &crate::pcb::altium::with_cases(&["SchLib", "IntLib", "kicad_sym"])).pick_file() {
                let _ = tx.send(Dialog::AddSymbols(path));
                ctx.request_repaint();
            }
        });
    }

    fn save(&mut self, ctx: &egui::Context, out: &mut Outcome) {
        match self.path.clone() {
            Some(p) => self.save_to(&p, out),
            None => self.ask_save_as(ctx),
        }
    }

    fn save_to(&mut self, path: &Path, out: &mut Outcome) {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let bytes = if Self::is_altium(path) { crate::pcb::altium::schsheet::write(&self.sch) } else { Ok(self.sch.save().into_bytes()) };
        out.notice = Some(match bytes.and_then(|b| std::fs::write(path, b).map_err(|e| e.to_string())) {
            Ok(()) => {
                self.path = Some(path.to_path_buf());
                self.dirty = false;
                format!("Saved {}", path.display())
            }
            Err(e) => format!("{}: {e}", path.display()),
        });
    }

    /// Offers the symbols of a library file beside the project's, for as long as this sheet is open.
    fn add_symbol_file(&mut self, path: &Path) -> String {
        let got = match crate::pcb::altium::read(path) {
            Ok(g) => g,
            Err(e) => return e,
        };
        let name = path.file_stem().unwrap_or_default().to_string_lossy().to_string();
        self.added.retain(|l| l.path.as_deref() != Some(path));
        let count = got.symbols.len();
        self.added.push(SymbolLibrary { name: name.clone(), path: Some(path.to_path_buf()), symbols: got.symbols });
        self.library = self.library_names().iter().position(|n| *n == name);
        self.panel = SymPanel::Parts;
        format!("{count} symbols from {}", path.file_name().unwrap_or_default().to_string_lossy())
    }

    /// What a menu item asks of the editor.
    pub fn command(&mut self, cmd: EditorCmd, ctx: &egui::Context, out: &mut Outcome) {
        match cmd {
            EditorCmd::New => {
                self.remember();
                self.sch = Schematic::default();
                self.path = None;
                self.sel = None;
                self.fit = true;
            }
            EditorCmd::Open => self.ask_open(ctx),
            EditorCmd::Save => self.save(ctx, out),
            EditorCmd::SaveAs => self.ask_save_as(ctx),
            EditorCmd::SaveToParts => {}
            EditorCmd::Undo => self.undo(),
            EditorCmd::Redo => self.redo(),
            EditorCmd::Delete => self.delete_selected(),
            EditorCmd::Turn => {
                if let Some(sel) = self.sel {
                    self.remember();
                    self.sch.rotate(sel);
                }
            }
            EditorCmd::Mirror => {
                if let Some(Sel::Part(id)) = self.sel {
                    self.remember();
                    self.sch.mirror(id);
                }
            }
            EditorCmd::Fit => self.fit = true,
            EditorCmd::Tool(n) => {
                self.choose_tool([Tool::Select, Tool::Place, Tool::Wire, Tool::Net, Tool::Power][(n as usize).min(4)], out);
            }
        }
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn has_selection(&self) -> bool {
        self.sel.is_some()
    }

    fn pump(&mut self, out: &mut Outcome) {
        while let Ok(d) = self.rx.try_recv() {
            match d {
                Dialog::Open(p) => out.notice = Some(self.open_file(&p)),
                Dialog::SaveAs(p) => self.save_to(&p, out),
                Dialog::AddSymbols(p) => out.notice = Some(self.add_symbol_file(&p)),
            }
        }
    }

    // -- The window ----------------------------------------------------------------------

    pub fn show(&mut self, ui: &mut Ui, footprints: &[String]) -> Outcome {
        let ctx = ui.ctx().clone();
        let mut out = Outcome::default();
        self.pump(&mut out);
        let bar = egui::Frame::new().fill(theme::PANEL).inner_margin(egui::Margin::symmetric(12, 6));
        Panel::top("sch_tools").resizable(false).frame(bar).show(ui, |ui| {
            ScrollArea::horizontal().auto_shrink([false, true]).show(ui, |ui| self.toolbar(ui, &ctx, &mut out));
        });
        let side = egui::Frame::new().fill(theme::PANEL).inner_margin(egui::Margin::same(10));
        Panel::right("sch_props").resizable(true).default_size(210.0).size_range(170.0..=320.0).frame(side).show(ui, |ui| {
            ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| self.details(ui, footprints));
        });
        egui::CentralPanel::default().frame(egui::Frame::new().fill(theme::WELL)).show(ui, |ui| self.canvas(ui, &ctx, &mut out));
        out
    }

    fn toolbar(&mut self, ui: &mut Ui, ctx: &egui::Context, out: &mut Outcome) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            for (tool, name, tip) in [
                (Tool::Select, "Select", "Click to pick, drag to move"),
                (Tool::Place, "Part", "Pick a symbol from a library of the project, then click to put it down"),
                (Tool::Wire, "Wire", "Click the corners; it stops on a pin or wire; Esc ends it"),
                (Tool::Net, "Net label", "Names a wire, a bus, a pair or a port; a number at the end counts up"),
                (Tool::Power, "Power", "Ground and supply symbols (GND, +3V3…) that join everywhere"),
            ] {
                if chip(ui, name, self.tool == tool).on_hover_text(tip).clicked() {
                    self.choose_tool(tool, out);
                }
            }
            if matches!(self.tool, Tool::Net | Tool::Power) {
                let r = ui.add(egui::TextEdit::singleline(&mut self.name).desired_width(90.0).hint_text("name"));
                // A supply typed over takes the look its name usually has.
                if r.changed()
                    && let Marker::Power(style) = self.marker
                {
                    let guess = PowerStyle::for_name(&self.name);
                    if guess.is_ground() || style.is_ground() {
                        self.marker = Marker::Power(guess);
                    }
                }
            }
            ui.separator();
            if small_icon(ui, Icon::Undo, "Undo (Ctrl+Z)", !self.undo.is_empty()).clicked() {
                self.undo();
            }
            if small_icon(ui, Icon::Redo, "Redo (Ctrl+Shift+Z)", !self.redo.is_empty()).clicked() {
                self.redo();
            }
            if small_icon(ui, Icon::Fit, "Fit the sheet to the window", true).clicked() {
                self.fit = true;
            }
            ui.separator();
            if small_icon(ui, Icon::Folder, "Open a schematic", true).clicked() {
                self.ask_open(ctx);
            }
            if small_icon(ui, Icon::Save, "Save (Ctrl+S)", true).clicked() {
                self.save(ctx, out);
            }
            if chip(ui, "Save as…", false).clicked() {
                self.ask_save_as(ctx);
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if self.dirty {
                    ui.label(RichText::new("unsaved").size(12.0).color(YELLOW));
                }
            });
        });
    }

    /// A supply typed over takes the look its name usually has.
    fn name_edited(&mut self) {
        if let Marker::Power(style) = self.marker {
            let guess = PowerStyle::for_name(&self.name);
            if guess.is_ground() || style.is_ground() {
                self.marker = Marker::Power(guess);
            }
        }
    }

    /// The left panel: the markers every sheet has, and the symbols of the libraries.
    pub fn symbol_panel(&mut self, ui: &mut Ui, ctx: &egui::Context) {
        let mut panel = self.panel;
        segmented(ui, &mut panel, &[(SymPanel::Markers, "Markers"), (SymPanel::Parts, "Parts")]);
        self.panel = panel;
        ui.add_space(8.0);
        match self.panel {
            SymPanel::Markers => self.markers_page(ui),
            SymPanel::Parts => self.parts_page(ui, ctx),
        }
    }

    /// Power and ground symbols, net labels, buses, pairs and ports, each shown as it will be drawn.
    fn markers_page(&mut self, ui: &mut Ui) {
        ui.label(RichText::new("Name for the next one").size(12.0).color(DIM));
        if ui.add(egui::TextEdit::singleline(&mut self.name).hint_text("name").desired_width(f32::INFINITY)).changed() {
            self.name_edited();
        }
        ui.add_space(6.0);
        let mut armed: Option<(Marker, String)> = None;
        let (tool, current, name) = (self.tool, self.marker, self.name.clone());
        ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            let mut section = |ui: &mut Ui, title: &str, tiles: &[(Marker, &str, &str)]| {
                ui.add_space(6.0);
                ui.label(RichText::new(title).font(semibold(13.0)).color(DIM));
                ui.add_space(3.0);
                let width = ((ui.available_width() - 6.0) / 2.0).floor();
                for pair in tiles.chunks(2) {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        for (marker, default, label) in pair {
                            let on = matches!(tool, Tool::Net | Tool::Power) && current == *marker && (!matches!(marker, Marker::Power(_)) || name == *default);
                            if marker_tile(ui, width, on, *marker, label).clicked() {
                                armed = Some((*marker, default.to_string()));
                            }
                        }
                    });
                    ui.add_space(6.0);
                }
            };
            let supplies: Vec<(Marker, &str, &str)> = SUPPLIES.iter().map(|(style, n)| (Marker::Power(*style), *n, *n)).collect();
            section(ui, "Power and ground", &supplies);
            section(ui, "Nets", &[(Marker::Net, "NET1", "Net label"), (Marker::Bus, "BUS[0..7]", "Bus"), (Marker::Pair, "DP", "Differential pair")]);
            section(
                ui,
                "Ports",
                &[
                    (Marker::Port(PortIo::Input), "IN1", "Input"),
                    (Marker::Port(PortIo::Output), "OUT1", "Output"),
                    (Marker::Port(PortIo::Bidirectional), "IO1", "Bidirectional"),
                    (Marker::Port(PortIo::Unspecified), "PORT1", "Unspecified"),
                ],
            );
        });
        if let Some((marker, default)) = armed {
            self.arm(marker, &default);
        }
    }

    /// The symbols of the project's libraries: which library, a search, the list, and how the one picked looks.
    fn parts_page(&mut self, ui: &mut Ui, ctx: &egui::Context) {
        let names = self.library_names();
        // The panel's width, taken before anything in it can ask for more.
        let panel_width = ui.available_width().min(320.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            let shown = self.library.and_then(|i| names.get(i)).cloned().unwrap_or_else(|| "All libraries".to_string());
            egui::ComboBox::from_id_salt("sym-library").selected_text(shown).width((panel_width - 38.0).max(60.0)).truncate().show_ui(ui, |ui| {
                if ui.selectable_label(self.library.is_none(), "All libraries").clicked() {
                    self.library = None;
                }
                for (i, n) in names.iter().enumerate() {
                    if ui.selectable_label(self.library == Some(i), n).clicked() {
                        self.library = Some(i);
                    }
                }
            });
            if small_icon(ui, Icon::Plus, "Open another symbol library (.SchLib or .IntLib) for this sheet", true).clicked() {
                self.ask_add_symbols(ctx);
            }
        });
        if names.is_empty() {
            ui.add_space(10.0);
            ui.label(RichText::new("No symbol libraries yet.").color(DIM));
            ui.add_space(4.0);
            ui.label(RichText::new("Open a project that has .SchLib files (the Project tab) and they appear here, or use + to open one.").size(12.0).color(FAINT));
            return;
        }
        ui.add_space(4.0);
        ui.add(egui::TextEdit::singleline(&mut self.filter).hint_text("Search").desired_width(f32::INFINITY));
        ui.add_space(6.0);
        let words = self.filter.to_lowercase();
        let one = self.library;
        let mut pick: Option<Symbol> = None;
        let mut hovered: Option<Symbol> = None;
        let mut count = 0;
        let list_height = (ui.available_height() - 168.0).max(90.0);
        ScrollArea::vertical().max_height(list_height).auto_shrink([false, false]).show(ui, |ui| {
            let libs: Vec<&SymbolLibrary> = self.libraries.iter().chain(self.added.iter()).collect();
            for (li, lib) in libs.iter().enumerate() {
                if one.is_some_and(|o| o != li) {
                    continue;
                }
                for s in &lib.symbols {
                    let hay = format!("{} {} {}", s.name, s.description, lib.name).to_lowercase();
                    if !words.split_whitespace().all(|w| hay.contains(w)) {
                        continue;
                    }
                    count += 1;
                    let on = self.ghost.as_ref().is_some_and(|(g, _, _)| g.name == s.name && g.library == s.library);
                    let (rect, r) = ui.allocate_exact_size(vec2(ui.available_width(), 38.0), Sense::click());
                    let fill = if on { theme::ACCENT_SOFT } else if r.hovered() { theme::RAISED } else { Color32::TRANSPARENT };
                    ui.painter().rect_filled(rect, 7.0, fill);
                    ui.painter().with_clip_rect(rect.shrink2(vec2(8.0, 0.0))).text(rect.left_top() + vec2(8.0, 5.0), Align2::LEFT_TOP, &s.name, FontId::proportional(13.5), if on { ACCENT } else { FG });
                    let sub = if one.is_some() {
                        if s.description.is_empty() { format!("{} pins", s.pins.len()) } else { format!("{} · {} pins", s.description, s.pins.len()) }
                    } else {
                        format!("{} · {} pins", lib.name, s.pins.len())
                    };
                    ui.painter().with_clip_rect(rect.shrink2(vec2(8.0, 0.0))).text(rect.left_top() + vec2(8.0, 21.0), Align2::LEFT_TOP, sub, FontId::proportional(11.5), FAINT);
                    if r.hovered() {
                        hovered = Some((*s).clone());
                    }
                    if r.clicked() {
                        pick = Some((*s).clone());
                    }
                }
            }
            if count == 0 {
                ui.label(RichText::new("Nothing matches.").size(12.0).color(FAINT));
            }
        });
        // How the symbol under the pointer (or the one being placed) looks.
        ui.add_space(6.0);
        let shown = hovered.or_else(|| self.ghost.as_ref().map(|(g, _, _)| g.clone()));
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 120.0), Sense::hover());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 9.0, PAPER);
        match shown {
            Some(sym) => {
                let part = Placed { id: 0, symbol: sym.clone(), designator: String::new(), value: String::new(), footprint: String::new(), at: p(0.0, 0.0), turns: 0, mirror: false, part: 1 };
                let bb = part.bbox();
                let inner = rect.shrink(12.0);
                let s = (inner.width() as f64 / bb.w().max(1.0)).min(inner.height() as f64 / bb.h().max(1.0)).min(14.0);
                let tr = Tr { rect: inner, c: bb.center(), s, m: 1.0 };
                draw_part(&painter, &tr, &part, false, false);
            }
            None => {
                painter.text(rect.center(), Align2::CENTER_CENTER, "Hover a symbol to see it", FontId::proportional(12.0), TEXT_DIM);
            }
        }
        if let Some(sym) = pick {
            self.ghost = Some((sym, 0, false));
            self.tool = Tool::Place;
            self.wire.clear();
        }
    }

    fn details(&mut self, ui: &mut Ui, footprints: &[String]) {
        match self.sel {
            Some(Sel::Part(id)) => self.part_card(ui, id, footprints),
            Some(Sel::Label(id)) => self.label_card(ui, id),
            Some(Sel::Wire(_)) => {
                card(ui, |ui| {
                    ui.label(RichText::new("Wire").font(semibold(14.0)));
                    if ui.button("Delete").clicked() {
                        self.delete_selected();
                    }
                });
            }
            None => {
                card(ui, |ui| {
                    ui.label(RichText::new("Sheet").font(semibold(14.0)));
                    field(ui, "Name", |ui| ui.add(egui::TextEdit::singleline(&mut self.sch.name).desired_width(140.0)));
                    let mut chosen = None;
                    field(ui, "Paper", |ui| {
                        egui::ComboBox::from_id_salt("paper").selected_text(self.sch.paper.name()).width(110.0).show_ui(ui, |ui| {
                            for paper in Paper::ALL {
                                if ui.selectable_label(self.sch.paper == paper, paper.name()).clicked() {
                                    chosen = Some(paper);
                                }
                            }
                        })
                    });
                    if let Some(paper) = chosen {
                        self.remember();
                        self.sch.paper = paper;
                        self.fit = true;
                    }
                    ui.label(RichText::new(format!("{} parts · {} wires · {} labels", self.sch.parts.len(), self.sch.wires.len(), self.sch.labels.len())).size(12.0).color(FAINT));
                    if !self.notes.is_empty() {
                        ui.label(RichText::new(format!("{} left out on reading", self.notes.len())).size(12.0).color(FAINT)).on_hover_text(self.notes.join("\n"));
                    }
                });
            }
        }
        self.checks_card(ui);
        self.nets_card(ui);
    }

    fn delete_selected(&mut self) {
        if let Some(sel) = self.sel.take() {
            self.remember();
            self.sch.delete(sel);
        }
    }

    fn part_card(&mut self, ui: &mut Ui, id: u32, footprints: &[String]) {
        let Some(orig) = self.sch.part(id).cloned() else { return };
        let mut part = orig.clone();
        card(ui, |ui| {
            ui.label(RichText::new(&part.symbol.name).font(semibold(14.0)));
            field(ui, "Designator", |ui| ui.add(egui::TextEdit::singleline(&mut part.designator).desired_width(90.0)));
            field(ui, "Value", |ui| ui.add(egui::TextEdit::singleline(&mut part.value).desired_width(140.0)));
            field(ui, "Footprint", |ui| {
                ui.add(egui::TextEdit::singleline(&mut part.footprint).desired_width(140.0));
            });
            let mut choose = None;
            let options: Vec<&String> = part.symbol.footprints.iter().chain(footprints.iter().filter(|f| part.footprint.len() >= 2 && f.to_lowercase().contains(&part.footprint.to_lowercase()) && **f != part.footprint).take(8)).collect();
            if !options.is_empty() {
                ui.horizontal_wrapped(|ui| {
                    for f in options.iter().take(10) {
                        if ui.small_button(f.as_str()).clicked() {
                            choose = Some((*f).clone());
                        }
                    }
                });
            }
            if let Some(f) = choose {
                part.footprint = f;
            }
            if part.symbol.parts > 1 {
                field(ui, "Part", |ui| ui.add(egui::DragValue::new(&mut part.part).range(1..=part.symbol.parts)));
            }
            ui.label(RichText::new(format!("Rotation {}°{}", u32::from(part.turns & 3) * 90, if part.mirror { " · mirrored" } else { "" })).size(12.5).color(DIM));
            ui.horizontal(|ui| {
                if ui.button("Turn (R)").clicked() {
                    part.turns = (part.turns + 1) & 3;
                }
                if ui.button("Mirror (X)").clicked() {
                    part.mirror = !part.mirror;
                }
                if ui.button("Delete").clicked() {
                    self.delete_selected();
                }
            });
            ui.label(RichText::new(format!("{} pins", part.pins_count())).size(12.0).color(FAINT));
        });
        if self.sel == Some(Sel::Part(id)) && part != orig {
            self.remember();
            if let Some(slot) = self.sch.part_mut(id) {
                *slot = part;
            }
            self.sch.fix_junctions();
        }
    }

    fn label_card(&mut self, ui: &mut Ui, id: u32) {
        let Some(orig) = self.sch.labels.iter().find(|l| l.id == id).cloned() else { return };
        let mut l = orig.clone();
        card(ui, |ui| {
            ui.label(RichText::new(match l.kind {
                LabelKind::Net => "Net label",
                LabelKind::Power => "Power port",
                LabelKind::Port => "Port",
            }).font(semibold(14.0)));
            field(ui, "Name", |ui| ui.add(egui::TextEdit::singleline(&mut l.text).desired_width(140.0)));
            if l.kind == LabelKind::Power {
                field(ui, "Style", |ui| {
                    egui::ComboBox::from_id_salt("power-style").selected_text(l.style.name()).width(110.0).show_ui(ui, |ui| {
                        for style in PowerStyle::ALL {
                            ui.selectable_value(&mut l.style, style, style.name());
                        }
                    })
                });
            }
            if l.kind == LabelKind::Port {
                field(ui, "Direction", |ui| {
                    egui::ComboBox::from_id_salt("port-io").selected_text(l.io.name()).width(110.0).show_ui(ui, |ui| {
                        for io in [PortIo::Input, PortIo::Output, PortIo::Bidirectional, PortIo::Unspecified] {
                            ui.selectable_value(&mut l.io, io, io.name());
                        }
                    })
                });
            }
            if l.kind != LabelKind::Net {
                ui.label(RichText::new(format!("Rotation {}°", u32::from(l.turns & 3) * 90)).size(12.5).color(DIM));
            }
            ui.horizontal(|ui| {
                if ui.button("Turn (R)").clicked() {
                    l.turns = (l.turns + 1) & 3;
                }
                if ui.button("Delete").clicked() {
                    self.delete_selected();
                }
            });
        });
        if self.sel == Some(Sel::Label(id)) && l != orig {
            self.remember();
            if let Some(slot) = self.sch.labels.iter_mut().find(|x| x.id == id) {
                *slot = l;
            }
        }
    }

    fn checks_card(&mut self, ui: &mut Ui) {
        let problems = self.sch.check();
        card(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("Checks").font(semibold(14.0)));
                let (text, color) = if problems.is_empty() { ("clean".to_string(), GREEN) } else { (format!("{}", problems.len()), YELLOW) };
                ui.label(RichText::new(text).size(12.5).color(color));
            });
            for pr in problems.iter().take(30) {
                ui.label(RichText::new(&pr.text).size(12.0).color(DIM));
            }
            if problems.len() > 30 {
                ui.label(RichText::new(format!("and {} more", problems.len() - 30)).size(12.0).color(FAINT));
            }
        });
    }

    fn nets_card(&mut self, ui: &mut Ui) {
        let nets = self.sch.netlist();
        card(ui, |ui| {
            ui.label(RichText::new(format!("Nets · {}", nets.len())).font(semibold(14.0)));
            for (name, pins) in nets.iter().take(40) {
                ui.label(RichText::new(name).size(12.5).color(FG)).on_hover_text(pins.join(", "));
                ui.label(RichText::new(pins.join("  ")).size(11.5).color(FAINT));
            }
        });
    }

    // -- The sheet ------------------------------------------------------------------------

    fn canvas(&mut self, ui: &mut Ui, ctx: &egui::Context, out: &mut Outcome) {
        let (rect, resp) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
        if self.fit {
            self.fit = false;
            // The page, and anything lying off it.
            let (w, h) = self.sch.paper.size();
            let mut bb = BBox::of(&[p(0.0, 0.0), p(w, h)]);
            bb.union(&self.sch.bbox());
            self.center = bb.center();
            self.scale = ((rect.width() as f64 / (bb.w().max(10.0) * 1.08)).min(rect.height() as f64 / (bb.h().max(10.0) * 1.08))).clamp(0.5, 60.0);
        }
        if let Some(at) = resp.hover_pos() {
            let (scroll, zoom) = ui.input(|i| (i.smooth_scroll_delta.y, i.zoom_delta()));
            let factor = ((scroll as f64) * 0.003).exp() * zoom as f64;
            if (factor - 1.0).abs() > 1e-6 {
                let before = Tr { rect, c: self.center, s: self.scale, m: 1.0 }.board(at);
                self.scale = (self.scale * factor).clamp(1.5, 200.0);
                let after = Tr { rect, c: self.center, s: self.scale, m: 1.0 }.board(at);
                self.center = self.center.add(before.sub(after));
            }
        }
        if resp.dragged_by(PointerButton::Secondary) || resp.dragged_by(PointerButton::Middle) {
            let d = resp.drag_delta();
            self.center.x -= d.x as f64 / self.scale;
            self.center.y += d.y as f64 / self.scale;
        }
        let tr = Tr { rect, c: self.center, s: self.scale, m: 1.0 };
        self.hover = resp.hover_pos().map(|q| tr.board(q));
        let tol = 6.0 / self.scale;

        if self.tool == Tool::Select {
            if resp.drag_started_by(PointerButton::Primary)
                && let Some(q) = resp.interact_pointer_pos().map(|q| tr.board(q))
            {
                self.grab = self.sch.pick(q, tol).map(|s| {
                    self.sel = Some(s);
                    self.remember();
                    (s, p(0.0, 0.0), q)
                });
            }
            if resp.dragged_by(PointerButton::Primary)
                && let (Some((sel, moved, q0)), Some(q)) = (self.grab, resp.interact_pointer_pos().map(|q| tr.board(q)))
            {
                let want = self.snap(q.sub(q0));
                let d = want.sub(moved);
                if d.x != 0.0 || d.y != 0.0 {
                    match sel {
                        Sel::Part(id) => self.sch.move_part_with_wires(id, d),
                        other => self.sch.translate(other, d),
                    }
                    self.grab = Some((sel, want, q0));
                }
            }
            if resp.drag_stopped() {
                self.grab = None;
            }
            if resp.clicked_by(PointerButton::Primary)
                && let Some(q) = resp.interact_pointer_pos().map(|q| tr.board(q))
            {
                self.sel = self.sch.pick(q, tol);
            }
        } else if resp.clicked_by(PointerButton::Primary)
            && let Some(q) = resp.interact_pointer_pos().map(|q| tr.board(q))
        {
            self.click(q);
        }
        if resp.clicked_by(PointerButton::Secondary) {
            self.stop();
        }

        if !ctx.egui_wants_keyboard_input() {
            let (esc, del, turn, mirror, space, undo, redo, save) = ctx.input(|i| {
                let cmd = i.modifiers.command;
                (
                    i.key_pressed(egui::Key::Escape),
                    i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace),
                    i.key_pressed(egui::Key::R) && !cmd,
                    i.key_pressed(egui::Key::X) && !cmd,
                    i.key_pressed(egui::Key::Space),
                    cmd && !i.modifiers.shift && i.key_pressed(egui::Key::Z),
                    cmd && i.modifiers.shift && i.key_pressed(egui::Key::Z),
                    cmd && i.key_pressed(egui::Key::S),
                )
            });
            if esc {
                self.stop();
            }
            if del {
                self.delete_selected();
            }
            if space {
                self.x_first = !self.x_first;
            }
            if turn {
                if let Some((_, t, _)) = self.ghost.as_mut().filter(|_| self.tool == Tool::Place) {
                    *t = (*t + 1) & 3;
                } else if let Some(sel) = self.sel {
                    self.remember();
                    self.sch.rotate(sel);
                }
            }
            if mirror {
                if let Some((_, _, m)) = self.ghost.as_mut().filter(|_| self.tool == Tool::Place) {
                    *m = !*m;
                } else if let Some(Sel::Part(id)) = self.sel {
                    self.remember();
                    self.sch.mirror(id);
                }
            }
            if undo {
                self.undo();
            }
            if redo {
                self.redo();
            }
            if save {
                self.save(ctx, out);
            }
        }

        let painter = ui.painter_at(rect);
        self.draw_page(&painter, &tr, rect);
        self.draw_sheet(&painter, &tr);
        self.draw_tool(&painter, &tr);
        if let Some(q) = self.hover {
            let q = self.snap(q);
            note(&painter, rect.left_bottom() + vec2(10.0, -8.0), Align2::LEFT_BOTTOM, &format!("X {:.2}  Y {:.2} mm", q.x, q.y), theme::mono(12.0), TEXT_DIM);
        }
        let hint = match self.tool {
            Tool::Select => "Click to pick · drag to move (wires follow) · R turns · X mirrors · Delete removes",
            Tool::Place => "Click to put it down · R turns · X mirrors · Esc stops",
            Tool::Wire => "Click the corners · Space flips the corner · it ends on a pin or wire · Esc ends",
            Tool::Net => "Click a wire or pin to name it · pick another kind in the Markers list",
            Tool::Power => "Click a pin or wire to attach it · R turns it · pick another kind in the Markers list",
        };
        note(&painter, rect.right_bottom() + vec2(-10.0, -8.0), Align2::RIGHT_BOTTOM, hint, FontId::proportional(12.0), TEXT_DIM);
    }

    /// The sheet of paper on the desk, with its border and the grid on it.
    fn draw_page(&self, painter: &egui::Painter, tr: &Tr, rect: Rect) {
        let (w, h) = self.sch.paper.size();
        let page = Rect::from_two_pos(tr.pt(p(0.0, h)), tr.pt(p(w, 0.0)));
        painter.rect_filled(rect, 0.0, DESK);
        painter.rect_filled(page.translate(vec2(4.0, 5.0)), 0.0, Color32::from_black_alpha(45));
        painter.rect_filled(page, 0.0, PAPER);
        self.draw_grid(&painter.with_clip_rect(page.intersect(rect)), tr, rect);
        let edge = Stroke::new(1.0, TEXT_DIM.gamma_multiply(0.7));
        painter.rect_stroke(page, 0.0, edge, egui::StrokeKind::Middle);
        // The border, in from the edge.
        let border = Rect::from_two_pos(tr.pt(p(BORDER, h - BORDER)), tr.pt(p(w - BORDER, BORDER)));
        painter.rect_stroke(border, 0.0, Stroke::new(1.4, TEXT_DIM), egui::StrokeKind::Middle);
    }

    fn draw_grid(&self, painter: &egui::Painter, tr: &Tr, rect: Rect) {
        let mut step = GRID;
        while step * tr.s < 9.0 {
            step *= 2.0;
        }
        let (lo, hi) = (tr.board(rect.left_bottom()), tr.board(rect.right_top()));
        let dot = DOTS;
        let mut x = (lo.x / step).floor() * step;
        while x <= hi.x {
            let mut y = (lo.y / step).floor() * step;
            while y <= hi.y {
                painter.circle_filled(tr.pt(p(x, y)), 0.9, dot);
                y += step;
            }
            x += step;
        }
    }

    fn draw_sheet(&self, painter: &egui::Painter, tr: &Tr) {
        let loose: Vec<P> = self.sch.check().iter().filter(|p| p.text.contains("isn't connected")).filter_map(|p| p.at).collect();
        for w in &self.sch.wires {
            let on = self.sel == Some(Sel::Wire(w.id));
            let pts: Vec<egui::Pos2> = w.pts.iter().map(|q| tr.pt(*q)).collect();
            painter.add(egui::Shape::line(pts, Stroke::new(mm(tr, if on { WIRE_MM + 0.1 } else { WIRE_MM }, if on { 2.6 } else { 1.5 }), if on { PICK } else { WIRE })));
        }
        for j in &self.sch.junctions {
            painter.circle_filled(tr.pt(*j), mm(tr, 0.45, 2.4), WIRE);
        }
        for part in &self.sch.parts {
            draw_part(painter, tr, part, self.sel == Some(Sel::Part(part.id)), false);
        }
        for q in &loose {
            let c = tr.pt(*q);
            let s = Stroke::new(1.6, LOOSE);
            painter.line_segment([c + vec2(-4.0, -4.0), c + vec2(4.0, 4.0)], s);
            painter.line_segment([c + vec2(-4.0, 4.0), c + vec2(4.0, -4.0)], s);
        }
        for l in &self.sch.labels {
            draw_label(painter, tr, l.at, &l.text, l.kind, l.turns, self.sel == Some(Sel::Label(l.id)), l.style, l.io);
        }
    }

    fn draw_tool(&self, painter: &egui::Painter, tr: &Tr) {
        let Some(h) = self.hover else { return };
        let q = self.snap(h);
        match self.tool {
            Tool::Place => {
                if let Some((sym, turns, mirror)) = &self.ghost {
                    let part = Placed { id: 0, symbol: sym.clone(), designator: self.sch.next_designator(&sym.prefix), value: String::new(), footprint: String::new(), at: q, turns: *turns, mirror: *mirror, part: 1 };
                    draw_part(painter, tr, &part, true, true);
                }
            }
            Tool::Wire => {
                let ink = Stroke::new(1.6, PICK);
                if let Some(last) = self.wire.last() {
                    let mut pts = self.wire.clone();
                    pts.extend(elbow(*last, q, self.x_first).into_iter().skip(1));
                    painter.add(egui::Shape::line(pts.iter().map(|p| tr.pt(*p)).collect(), ink));
                }
                painter.circle_stroke(tr.pt(q), 4.0, ink);
            }
            Tool::Net | Tool::Power => {
                let name = self.name.trim();
                let (kind, style, io) = match self.marker {
                    Marker::Power(style) => (LabelKind::Power, style, PortIo::default()),
                    Marker::Port(io) => (LabelKind::Port, PowerStyle::default(), io),
                    _ => (LabelKind::Net, PowerStyle::default(), PortIo::default()),
                };
                let turns = self.sch.pin_direction_at(q).map(|a| (((a - 90.0) / 90.0).round() as i32).rem_euclid(4) as u8).unwrap_or(if kind == LabelKind::Power && style.is_ground() { 2 } else { 0 });
                if self.marker == Marker::Pair {
                    draw_label(painter, tr, q, &format!("{name}_P"), kind, turns, true, style, io);
                    draw_label(painter, tr, q.sub(p(0.0, GRID)), &format!("{name}_N"), kind, turns, true, style, io);
                } else {
                    draw_label(painter, tr, q, name, kind, turns, true, style, io);
                }
            }
            Tool::Select => {}
        }
    }
}

impl Placed {
    fn pins_count(&self) -> usize {
        self.pins().len()
    }
}

/// A font of this height in millimetres on the sheet, so text grows and shrinks
/// with the drawing as the footprints' does; none once it would be too small to read.
fn sized(tr: &Tr, mm: f64) -> Option<FontId> {
    let px = (mm * tr.s) as f32;
    (px >= 5.5).then(|| FontId::proportional(px))
}

/// A shape filled with a see-through colour, cut into triangles so one with a
/// notch in it (an arrow head) fills as it should.
fn fill_ring(painter: &egui::Painter, ring: &[P], to_screen: &dyn Fn(P) -> egui::Pos2, color: Color32) {
    let cut = triangulate(ring);
    if cut.is_empty() {
        return;
    }
    let mut mesh = egui::Mesh::default();
    for q in ring {
        mesh.colored_vertex(to_screen(*q), color);
    }
    for t in cut.chunks(3) {
        mesh.add_triangle(t[0], t[1], t[2]);
    }
    painter.add(egui::Shape::mesh(mesh));
}

fn draw_part(painter: &egui::Painter, tr: &Tr, part: &Placed, on: bool, ghost: bool) {
    let fade = |c: Color32| if ghost { c.gamma_multiply(0.6) } else { c };
    let body = fade(if on && !ghost { PICK } else { BODY });
    let fill = fade(body_fill());
    let stroke = Stroke::new(mm(tr, if on { BODY_MM + 0.08 } else { BODY_MM }, 1.4), body);
    let t = |q: P| tr.pt(transform(q, part.at, part.turns, part.mirror));
    for d in part.drawings() {
        match d {
            Drawing::Line(a, b) => {
                painter.line_segment([t(*a), t(*b)], stroke);
            }
            Drawing::Rect(a, b) => {
                let corners = [*a, p(b.x, a.y), *b, p(a.x, b.y)];
                fill_ring(painter, &corners, &t, fill);
                painter.add(egui::Shape::closed_line(corners.iter().map(|q| t(*q)).collect(), stroke));
            }
            Drawing::Polyline(v) => {
                painter.add(egui::Shape::line(v.iter().map(|q| t(*q)).collect(), stroke));
            }
            Drawing::Polygon(v) => {
                fill_ring(painter, v, &t, fill);
                painter.add(egui::Shape::closed_line(v.iter().map(|q| t(*q)).collect(), stroke));
            }
            Drawing::Arc { c, r, start, end } => {
                let mut sweep = (end - start).rem_euclid(360.0);
                if sweep < 1e-9 {
                    sweep = 360.0;
                }
                let n = ((sweep / 8.0).ceil() as usize).max(2);
                let pts = (0..=n).map(|k| {
                    let a = (start + sweep * k as f64 / n as f64).to_radians();
                    t(c.add(p(a.cos(), a.sin()).scale(*r)))
                });
                painter.add(egui::Shape::line(pts.collect(), stroke));
            }
            Drawing::Ellipse { c, rx, ry } => {
                let ring: Vec<P> = (0..40)
                    .map(|k| {
                        let a = k as f64 * std::f64::consts::TAU / 40.0;
                        c.add(p(a.cos() * rx, a.sin() * ry))
                    })
                    .collect();
                fill_ring(painter, &ring, &t, fill);
                painter.add(egui::Shape::closed_line(ring.iter().map(|q| t(*q)).collect(), stroke));
            }
            Drawing::Text { at, text } => {
                if let Some(font) = sized(tr, EM_NOTE) {
                    // In from the corner the text is anchored to, so it doesn't sit on the line.
                    let inset = vec2(mm(tr, 0.5, 1.0), -mm(tr, 0.4, 1.0));
                    painter.text(t(*at) + inset, Align2::LEFT_BOTTOM, text, font, fade(TEXT));
                }
            }
        }
    }
    let pin_stroke = Stroke::new(mm(tr, PIN_MM, 1.3), fade(PIN));
    for pin in part.pins() {
        let (a, tip) = (tr.pt(pin.at), tr.pt(pin.tip));
        painter.line_segment([a, tip], pin_stroke);
        if pin.hidden {
            continue;
        }
        let quarter = ((pin.angle / 90.0).round() as i32) & 3;
        let mid = (a + tip.to_vec2()) / 2.0;
        // The number sits against the pin: over a sideways one, to the left of an upright one.
        if pin.number_shown
            && let Some(font) = sized(tr, EM_PIN_NUMBER)
        {
            let off = mm(tr, 0.3, 1.0);
            if quarter % 2 == 0 {
                painter.text(mid + vec2(0.0, -off), Align2::CENTER_BOTTOM, &pin.number, font, fade(PIN_NUMBER));
            } else {
                painter.text(mid + vec2(-off, 0.0), Align2::RIGHT_CENTER, &pin.number, font, fade(PIN_NUMBER));
            }
        }
        if pin.name_shown
            && !pin.name.is_empty()
            && let Some(font) = sized(tr, EM_PIN_NAME)
        {
            // Inside the body, in from its edge: the pin reaches out from `a` the way it
            // points, so the name goes the other way.
            let inset = mm(tr, 0.9, 2.0);
            let (anchor, align) = match quarter {
                0 => (a + vec2(-inset, 0.0), Align2::RIGHT_CENTER),
                2 => (a + vec2(inset, 0.0), Align2::LEFT_CENTER),
                1 => (a + vec2(0.0, inset), Align2::CENTER_TOP),
                _ => (a + vec2(0.0, -inset), Align2::CENTER_BOTTOM),
            };
            painter.text(anchor, align, &pin.name, font, fade(TEXT));
        }
    }
    let bb = part.bbox();
    let gap = mm(tr, 0.5, 2.0);
    if !part.designator.is_empty()
        && let Some(font) = sized(tr, EM_DESIGNATOR)
    {
        painter.text(tr.pt(p(bb.min.x, bb.max.y)) + vec2(0.0, -gap), Align2::LEFT_BOTTOM, &part.designator, font, fade(if on && !ghost { PICK } else { DESIGNATOR }));
    }
    if !part.value.is_empty()
        && let Some(font) = sized(tr, EM_VALUE)
    {
        painter.text(tr.pt(p(bb.min.x, bb.min.y)) + vec2(0.0, gap), Align2::LEFT_TOP, &part.value, font, fade(TEXT_DIM));
    }
}

/// A supply symbol with its stem from `c` going `step` (a unit step on screen), drawn in
/// millimetres so it keeps its proportions at any zoom. The name goes beyond its end.
fn draw_power(painter: &egui::Painter, tr: &Tr, c: egui::Pos2, step: egui::Vec2, style: PowerStyle, text: &str, color: Color32) {
    let side = vec2(-step.y, step.x);
    let at = |along: f64, across: f64| c + step * tr.len(along) + side * tr.len(across);
    let w = Stroke::new(mm(tr, 0.2, 1.5), color);
    let bold = Stroke::new(mm(tr, 0.3, 2.0), color);
    let end = match style {
        PowerStyle::Bar => {
            painter.line_segment([c, at(2.2, 0.0)], w);
            painter.line_segment([at(2.2, -1.2), at(2.2, 1.2)], bold);
            2.2
        }
        PowerStyle::Circle => {
            painter.line_segment([c, at(1.6, 0.0)], w);
            painter.circle_stroke(at(2.4, 0.0), tr.len(0.8).max(2.5), w);
            3.2
        }
        PowerStyle::Arrow => {
            painter.line_segment([c, at(2.0, 0.0)], w);
            let head = vec![at(2.9, 0.0), at(1.9, -0.9), at(1.9, 0.9)];
            painter.add(egui::Shape::convex_polygon(head, color, Stroke::NONE));
            2.9
        }
        PowerStyle::Wave => {
            painter.line_segment([c, at(1.6, 0.0)], w);
            let wave: Vec<egui::Pos2> = (0..=16)
                .map(|k| {
                    let x = -1.3 + 2.6 * k as f64 / 16.0;
                    at(2.1 + 0.45 * (x / 1.3 * std::f64::consts::PI).sin(), x)
                })
                .collect();
            painter.add(egui::Shape::line(wave, bold));
            2.7
        }
        PowerStyle::Ground => {
            painter.line_segment([c, at(1.2, 0.0)], w);
            for (along, half) in [(1.2, 1.5), (1.85, 0.95), (2.5, 0.4)] {
                painter.line_segment([at(along, -half), at(along, half)], bold);
            }
            2.5
        }
        PowerStyle::SignalGround => {
            painter.line_segment([c, at(1.2, 0.0)], w);
            let tri = vec![at(1.2, -1.3), at(1.2, 1.3), at(2.7, 0.0)];
            painter.add(egui::Shape::closed_line(tri, w));
            2.7
        }
        PowerStyle::Earth => {
            painter.line_segment([c, at(1.3, 0.0)], w);
            painter.line_segment([at(1.3, -1.3), at(1.3, 1.3)], bold);
            for x in [-1.0, 0.0, 1.0] {
                painter.line_segment([at(1.3, x), at(2.3, x - 0.55)], w);
            }
            2.3
        }
    };
    if text.is_empty() {
        return;
    }
    if let Some(font) = sized(tr, EM_LABEL) {
        let align = if step.y < 0.0 {
            Align2::CENTER_BOTTOM
        } else if step.y > 0.0 {
            Align2::CENTER_TOP
        } else if step.x > 0.0 {
            Align2::LEFT_CENTER
        } else {
            Align2::RIGHT_CENTER
        };
        painter.text(at(end + 0.4, 0.0), align, text, font, color);
    }
}

/// A net label, a supply or a port, at `at`. (A supply's style and a port's direction
/// come with it; they matter only for those.) The text may be empty for a bare symbol.
fn draw_label(painter: &egui::Painter, tr: &Tr, at: P, text: &str, kind: LabelKind, turns: u8, on: bool, style: PowerStyle, io: PortIo) {
    let color = if on {
        PICK
    } else {
        match kind {
            LabelKind::Power => POWER,
            LabelKind::Port => PORT,
            LabelKind::Net => NET,
        }
    };
    let c = tr.pt(at);
    // The way it points, as a unit step on screen.
    let step = match turns & 3 {
        0 => vec2(0.0, -1.0),
        1 => vec2(-1.0, 0.0),
        2 => vec2(0.0, 1.0),
        _ => vec2(1.0, 0.0),
    };
    match kind {
        LabelKind::Net => {
            painter.circle_filled(c, mm(tr, 0.4, 2.2), color);
            if !text.is_empty()
                && let Some(font) = sized(tr, EM_LABEL)
            {
                painter.text(c + vec2(mm(tr, 0.7, 3.0), -mm(tr, 0.4, 2.0)), Align2::LEFT_BOTTOM, text, font, color);
            }
        }
        LabelKind::Power => draw_power(painter, tr, c, step, style, text, color),
        LabelKind::Port => {
            let font = sized(tr, EM_LABEL).unwrap_or_else(|| FontId::proportional(5.5));
            let galley = painter.layout_no_wrap(text.to_string(), font, color);
            let (h, pad) = (mm(tr, 2.0, 7.0), mm(tr, 0.7, 2.5));
            let tip = if io == PortIo::Unspecified { 0.0 } else { h / 2.0 };
            let ends = if io == PortIo::Bidirectional { 2.0 } else { 1.0 };
            let (l, r) = (c.x, c.x + galley.size().x + 2.0 * pad + tip * ends);
            let (top, bot, mid) = (c.y - h / 2.0, c.y + h / 2.0, c.y);
            let pts = match io {
                PortIo::Unspecified => vec![pos2(l, top), pos2(r, top), pos2(r, bot), pos2(l, bot)],
                PortIo::Input => vec![pos2(l, mid), pos2(l + tip, top), pos2(r, top), pos2(r, bot), pos2(l + tip, bot)],
                PortIo::Output => vec![pos2(l, top), pos2(r - tip, top), pos2(r, mid), pos2(r - tip, bot), pos2(l, bot)],
                PortIo::Bidirectional => vec![pos2(l, mid), pos2(l + tip, top), pos2(r - tip, top), pos2(r, mid), pos2(r - tip, bot), pos2(l + tip, bot)],
            };
            painter.add(egui::Shape::convex_polygon(pts, color.gamma_multiply(0.14), Stroke::new(mm(tr, 0.18, 1.4), color)));
            let x = if io == PortIo::Input || io == PortIo::Bidirectional { l + tip + pad } else { l + pad };
            painter.galley(pos2(x, mid - galley.size().y / 2.0), galley, color);
        }
    }
}

/// One marker in the panel, drawn the way it looks on a sheet, on paper.
fn marker_tile(ui: &mut Ui, width: f32, on: bool, marker: Marker, label: &str) -> egui::Response {
    let (rect, r) = ui.allocate_exact_size(vec2(width, 72.0), Sense::click());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 9.0, PAPER);
    let ring = if on {
        ACCENT
    } else if r.hovered() {
        FAINT
    } else {
        theme::LINE
    };
    painter.rect_stroke(rect, 9.0, Stroke::new(if on { 2.0 } else { 1.0 }, ring), egui::StrokeKind::Inside);
    let glyph = Rect::from_min_max(rect.min + vec2(0.0, 2.0), pos2(rect.max.x, rect.min.y + 52.0));
    let tr = Tr { rect: glyph, c: p(0.0, 0.0), s: 8.0, m: 1.0 };
    let (plain, io) = (PowerStyle::default(), PortIo::default());
    match marker {
        // A ground hangs down from its wire, anything else stands up from it.
        Marker::Power(style) if style.is_ground() => draw_label(&painter, &tr, p(0.0, 2.0), "", LabelKind::Power, 2, false, style, io),
        Marker::Power(style) => draw_label(&painter, &tr, p(0.0, -1.8), "", LabelKind::Power, 0, false, style, io),
        Marker::Net => draw_label(&painter, &tr, p(-2.8, -0.4), "NET1", LabelKind::Net, 0, false, plain, io),
        Marker::Bus => draw_label(&painter, &tr, p(-4.0, -0.4), "BUS[0..7]", LabelKind::Net, 0, false, plain, io),
        Marker::Pair => {
            draw_label(&painter, &tr, p(-3.0, 1.0), "DP_P", LabelKind::Net, 0, false, plain, io);
            draw_label(&painter, &tr, p(-3.0, -1.54), "DP_N", LabelKind::Net, 0, false, plain, io);
        }
        Marker::Port(io) => draw_label(&painter, &tr, p(-3.0, 0.0), "IN", LabelKind::Port, 0, false, plain, io),
    }
    painter.text(pos2(rect.center().x, rect.max.y - 7.0), Align2::CENTER_BOTTOM, label, FontId::proportional(11.5), TEXT_DIM);
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pair_puts_down_both_halves_a_grid_apart_and_counts_on() {
        let mut w = SchWindow::new();
        w.arm(Marker::Pair, "USB_D1");
        w.click(p(25.4, 25.4));
        let find = |w: &SchWindow, n: &str| w.sch.labels.iter().find(|l| l.text == n).cloned();
        let (pos, neg) = (find(&w, "USB_D1_P").unwrap(), find(&w, "USB_D1_N").unwrap());
        assert!(pos.at.same(p(25.4, 25.4)) && neg.at.same(p(25.4, 25.4 - GRID)), "{:?} {:?}", pos.at, neg.at);
        assert_eq!(w.name, "USB_D2", "the next pair is numbered on");
        assert_eq!(w.sch.labels.len(), 2);
    }

    #[test]
    fn a_supply_is_placed_in_the_style_picked_and_a_bus_keeps_its_range() {
        let mut w = SchWindow::new();
        w.arm(Marker::Power(PowerStyle::Circle), "VBAT");
        w.click(p(10.16, 10.16));
        assert_eq!(w.sch.labels[0].style, PowerStyle::Circle);
        assert_eq!(w.sch.labels[0].kind, LabelKind::Power);
        assert_eq!(w.name, "VBAT", "a supply keeps its name");
        w.arm(Marker::Bus, "D[0..7]");
        w.click(p(20.32, 10.16));
        assert_eq!(w.sch.labels[1].text, "D[0..7]");
        assert_eq!(w.name, "D[0..7]");
        w.arm(Marker::Port(PortIo::Output), "OUT1");
        w.click(p(30.48, 10.16));
        assert_eq!((w.sch.labels[2].kind, w.sch.labels[2].io), (LabelKind::Port, PortIo::Output));
        assert_eq!(w.name, "OUT2");
    }

    #[test]
    fn grounds_are_drawn_as_grounds_and_the_rest_as_bars() {
        assert_eq!(PowerStyle::for_name("GND"), PowerStyle::Ground);
        assert_eq!(PowerStyle::for_name("VSS"), PowerStyle::Ground);
        assert_eq!(PowerStyle::for_name("AGND"), PowerStyle::SignalGround);
        assert_eq!(PowerStyle::for_name("PE"), PowerStyle::Earth);
        assert_eq!(PowerStyle::for_name("+3V3"), PowerStyle::Bar);
        assert_eq!(PowerStyle::for_name("VCC"), PowerStyle::Bar);
        for style in PowerStyle::ALL {
            assert_eq!(PowerStyle::from_altium(style.altium()), style, "{style:?} keeps its number");
        }
    }

    #[test]
    fn with_no_project_there_are_no_libraries_and_no_orphan_parts() {
        let w = SchWindow::new();
        assert!(w.library_names().is_empty(), "nothing built in, nothing from a data folder");
    }
}
