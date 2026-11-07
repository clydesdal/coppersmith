//! The schematic editor's window: symbols to place on the left, the sheet in
//! the middle, the details of what's picked and the checks on the right.
//! What edits do is in `pcb::schematic`; this is drawing and pointing.

use crate::pcb::altium::schlib::{Drawing, Pin, PinKind, Symbol};
use crate::pcb::geom::*;
use crate::pcb::schematic::{LabelKind, Placed, Schematic, Sel, elbow, transform};
use crate::ui::dock::EditorCmd;
use crate::ui::icons::Icon;
use crate::ui::panels::{card, chip, field, small_icon};
use crate::ui::theme::{self, ACCENT, DIM, FAINT, FG, GREEN, RED, YELLOW, semibold};
use crate::ui::view::{self, Tr};
use eframe::egui::{self, Align, Align2, Color32, FontId, Layout, Panel, PointerButton, Rect, RichText, ScrollArea, Sense, Stroke, Ui, pos2, vec2};
use std::path::{Path, PathBuf};
use std::sync::mpsc;

const GRID: f64 = 2.54;
const WIRE: Color32 = theme::rgb(0x4d_db_7a);
const BODY: Color32 = theme::rgb(0xe6_c0_7b);
const PIN: Color32 = theme::rgb(0xd9_5a_55);
const PAPER: Color32 = theme::rgb(0x14_16_1b);

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
    symbols: Vec<Symbol>,
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

/// A few symbols to start with, drawn on the 2.54 mm grid.
pub fn builtin_symbols() -> Vec<Symbol> {
    let pin = |n: &str, name: &str, x: f64, y: f64, angle: f64| Pin { part: 1, number: n.into(), name: name.into(), at: p(x, y), length: 2.54, angle, kind: PinKind::Passive, hidden: false };
    let two = |name: &str, prefix: &str, desc: &str, drawings: Vec<Drawing>| Symbol {
        name: name.into(),
        description: desc.into(),
        prefix: prefix.into(),
        parts: 1,
        pins: vec![pin("1", "", 0.0, 2.54, 90.0), pin("2", "", 0.0, -2.54, 270.0)],
        drawings: drawings.into_iter().map(|d| (0, d)).collect(),
        footprints: Vec::new(),
        library: "Built in".into(),
    };
    let mut out = vec![
        two("Resistor", "R", "Resistor", vec![Drawing::Rect(p(-1.0, 2.54), p(1.0, -2.54))]),
        two("Capacitor", "C", "Capacitor", vec![Drawing::Line(p(-2.0, 0.6), p(2.0, 0.6)), Drawing::Line(p(-2.0, -0.6), p(2.0, -0.6)), Drawing::Line(p(0.0, 2.54), p(0.0, 0.6)), Drawing::Line(p(0.0, -0.6), p(0.0, -2.54))]),
        two("Inductor", "L", "Inductor", vec![Drawing::Arc { c: p(0.0, 1.7), r: 0.85, start: 90.0, end: 270.0 }, Drawing::Arc { c: p(0.0, 0.0), r: 0.85, start: 90.0, end: 270.0 }, Drawing::Arc { c: p(0.0, -1.7), r: 0.85, start: 90.0, end: 270.0 }]),
        two("Diode", "D", "Diode", vec![Drawing::Polygon(vec![p(-1.5, 1.0), p(1.5, 1.0), p(0.0, -1.0)]), Drawing::Line(p(-1.5, -1.0), p(1.5, -1.0)), Drawing::Line(p(0.0, 2.54), p(0.0, 1.0)), Drawing::Line(p(0.0, -1.0), p(0.0, -2.54))]),
    ];
    for s in &mut out {
        s.pins[0].angle = 90.0;
    }
    // A generic chip with `n` pins down each side.
    let mut ic = Symbol { name: "IC 8".into(), description: "Eight-pin chip".into(), prefix: "U".into(), parts: 1, pins: Vec::new(), drawings: Vec::new(), footprints: Vec::new(), library: "Built in".into() };
    for k in 0..4 {
        let y = 3.81 - 2.54 * k as f64;
        ic.pins.push(pin(&(k + 1).to_string(), "", -7.62, y, 180.0));
        ic.pins.push(pin(&(8 - k).to_string(), "", 7.62, y, 0.0));
    }
    ic.drawings.push((0, Drawing::Rect(p(-7.62, 6.35), p(7.62, -6.35))));
    out.push(ic);
    out
}

impl SchWindow {
    pub fn new() -> SchWindow {
        let (tx, rx) = mpsc::channel();
        let mut w = SchWindow {
            sch: Schematic::default(),
            path: None,
            dirty: false,
            undo: Vec::new(),
            redo: Vec::new(),
            tool: Tool::Select,
            sel: None,
            center: p(30.0, 20.0),
            scale: 11.0,
            fit: false,
            hover: None,
            symbols: builtin_symbols(),
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
        w.load_symbol_folder();
        w
    }

    fn symbol_folder() -> Option<PathBuf> {
        Some(crate::platform::data_dir()?.join("symbols"))
    }

    /// Symbols from the SchLib files in the symbols folder.
    fn load_symbol_folder(&mut self) {
        self.symbols = builtin_symbols();
        let Some(dir) = Self::symbol_folder() else { return };
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        let mut files: Vec<PathBuf> = entries.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("schlib") || e.eq_ignore_ascii_case("intlib"))).collect();
        files.sort();
        for f in files {
            if let Ok(got) = crate::pcb::altium::read(&f) {
                self.symbols.extend(got.symbols);
            }
        }
    }

    /// Symbols from any library the app has open (the library editor's).
    pub fn add_symbols(&mut self, symbols: &[Symbol]) {
        for s in symbols {
            if !self.symbols.iter().any(|o| o.name == s.name && o.library == s.library) {
                self.symbols.push(s.clone());
            }
        }
    }

    fn is_altium(path: &Path) -> bool {
        path.extension().is_some_and(|e| e.eq_ignore_ascii_case("schdoc") || e.eq_ignore_ascii_case("csschdoc"))
    }

    pub fn open_file(&mut self, path: &Path) -> String {
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
                let kind = if self.tool == Tool::Net { LabelKind::Net } else { LabelKind::Power };
                let id = self.sch.add_label(q, self.name.trim(), kind);
                self.sel = Some(Sel::Label(id));
                if self.tool == Tool::Net {
                    self.name = Self::bump(&self.name);
                }
            }
        }
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
            if let Some(path) = rfd::FileDialog::new().set_title("Open a schematic").add_filter("Schematics", &crate::pcb::altium::with_cases(&["cssch", "json", "SchDoc", "CSSchDoc"])).add_filter("Altium or Circuit Studio sheet", &crate::pcb::altium::with_cases(&["SchDoc", "CSSchDoc"])).add_filter("Coppersmith schematic", &["cssch", "json"]).pick_file() {
                let _ = tx.send(Dialog::Open(path));
                ctx.request_repaint();
            }
        });
    }

    fn ask_save_as(&self, ctx: &egui::Context) {
        let (tx, ctx, name) = (self.tx.clone(), ctx.clone(), format!("{}.cssch", self.sch.name));
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
            if let Some(path) = rfd::FileDialog::new().set_title("Add symbols from an Altium library").add_filter("Altium symbol library", &crate::pcb::altium::with_cases(&["SchLib", "IntLib"])).pick_file() {
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

    /// Copies a symbol library into the symbols folder so it's there from now on.
    fn add_symbol_file(&mut self, path: &Path) -> String {
        let got = match crate::pcb::altium::read(path) {
            Ok(g) => g,
            Err(e) => return e,
        };
        if let Some(dir) = Self::symbol_folder() {
            let _ = std::fs::create_dir_all(&dir);
            if let Some(name) = path.file_name() {
                let _ = std::fs::copy(path, dir.join(name));
            }
        }
        self.add_symbols(&got.symbols);
        format!("{} symbols added from {}", got.symbols.len(), path.file_name().unwrap_or_default().to_string_lossy())
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
                self.wire.clear();
                self.tool = [Tool::Select, Tool::Place, Tool::Wire, Tool::Net, Tool::Power][(n as usize).min(4)];
                if self.tool != Tool::Place {
                    self.ghost = None;
                }
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
        Panel::left("sch_symbols").resizable(true).default_size(250.0).size_range(200.0..=400.0).frame(side).show(ui, |ui| self.symbol_list(ui, &ctx));
        let side = egui::Frame::new().fill(theme::PANEL).inner_margin(egui::Margin::same(10));
        Panel::right("sch_props").resizable(true).default_size(300.0).size_range(240.0..=460.0).frame(side).show(ui, |ui| {
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
                (Tool::Place, "Part", "Pick a symbol on the left, then click to put it down"),
                (Tool::Wire, "Wire", "Click the corners; it stops on a pin or wire; Esc ends it"),
                (Tool::Net, "Net label", "Names a wire; a number at the end counts up"),
                (Tool::Power, "Power", "A supply name (GND, +3V3) that joins everywhere"),
            ] {
                if chip(ui, name, self.tool == tool).on_hover_text(tip).clicked() {
                    self.wire.clear();
                    self.tool = tool;
                    if tool != Tool::Place {
                        self.ghost = None;
                    }
                }
            }
            if matches!(self.tool, Tool::Net | Tool::Power) {
                ui.add(egui::TextEdit::singleline(&mut self.name).desired_width(90.0).hint_text("name"));
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

    fn symbol_list(&mut self, ui: &mut Ui, ctx: &egui::Context) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("Symbols").size(14.0).color(DIM));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if small_icon(ui, Icon::Plus, "Add symbols from an Altium library (.SchLib or .IntLib)", true).clicked() {
                    self.ask_add_symbols(ctx);
                }
            });
        });
        ui.add(egui::TextEdit::singleline(&mut self.filter).hint_text("Search").desired_width(f32::INFINITY));
        ui.add_space(6.0);
        let words = self.filter.to_lowercase();
        let mut pick = None;
        ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            for (i, s) in self.symbols.iter().enumerate() {
                let hay = format!("{} {} {}", s.name, s.description, s.library).to_lowercase();
                if !words.split_whitespace().all(|w| hay.contains(w)) {
                    continue;
                }
                let on = self.ghost.as_ref().is_some_and(|(g, _, _)| g.name == s.name && g.library == s.library);
                let (rect, r) = ui.allocate_exact_size(vec2(ui.available_width(), 38.0), Sense::click());
                let fill = if on { theme::ACCENT_SOFT } else if r.hovered() { theme::RAISED } else { Color32::TRANSPARENT };
                ui.painter().rect_filled(rect, 7.0, fill);
                ui.painter().text(rect.left_top() + vec2(8.0, 5.0), Align2::LEFT_TOP, &s.name, FontId::proportional(13.5), if on { ACCENT } else { FG });
                let sub = format!("{} · {} pins", s.library, s.pins.len());
                ui.painter().with_clip_rect(rect.shrink2(vec2(8.0, 0.0))).text(rect.left_top() + vec2(8.0, 21.0), Align2::LEFT_TOP, sub, FontId::proportional(11.5), FAINT);
                if r.clicked() {
                    pick = Some(i);
                }
            }
        });
        if let Some(i) = pick {
            self.ghost = Some((self.symbols[i].clone(), 0, false));
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
            let bb = self.sch.bbox();
            if bb.is_empty() {
                self.center = p(30.0, 20.0);
                self.scale = 11.0;
            } else {
                self.center = bb.center();
                self.scale = ((rect.width() as f64 / (bb.w().max(10.0) * 1.3)).min(rect.height() as f64 / (bb.h().max(10.0) * 1.3))).clamp(2.0, 60.0);
            }
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
        painter.rect_filled(rect, 0.0, PAPER);
        self.draw_grid(&painter, &tr, rect);
        self.draw_sheet(&painter, &tr);
        self.draw_tool(&painter, &tr);
        if let Some(q) = self.hover {
            let q = self.snap(q);
            painter.text(rect.left_bottom() + vec2(10.0, -8.0), Align2::LEFT_BOTTOM, format!("X {:.2}  Y {:.2} mm", q.x, q.y), theme::mono(12.0), DIM);
        }
        let hint = match self.tool {
            Tool::Select => "Click to pick · drag to move (wires follow) · R turns · X mirrors · Delete removes",
            Tool::Place => "Click to put it down · R turns · X mirrors · Esc stops",
            Tool::Wire => "Click the corners · Space flips the corner · it ends on a pin or wire · Esc ends",
            Tool::Net => "Click a wire or pin to name it",
            Tool::Power => "Click a pin or wire to attach it",
        };
        painter.text(rect.right_bottom() + vec2(-10.0, -8.0), Align2::RIGHT_BOTTOM, hint, FontId::proportional(12.0), FAINT);
    }

    fn draw_grid(&self, painter: &egui::Painter, tr: &Tr, rect: Rect) {
        let mut step = GRID;
        while step * tr.s < 9.0 {
            step *= 2.0;
        }
        let (lo, hi) = (tr.board(rect.left_bottom()), tr.board(rect.right_top()));
        let dot = Color32::from_gray(44);
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
            painter.add(egui::Shape::line(pts, Stroke::new(if on { 2.8 } else { 1.8 }, if on { ACCENT } else { WIRE })));
        }
        for j in &self.sch.junctions {
            painter.circle_filled(tr.pt(*j), 3.2, WIRE);
        }
        for part in &self.sch.parts {
            draw_part(painter, tr, part, self.sel == Some(Sel::Part(part.id)), false);
        }
        for q in &loose {
            let c = tr.pt(*q);
            let s = Stroke::new(1.6, RED);
            painter.line_segment([c + vec2(-4.0, -4.0), c + vec2(4.0, 4.0)], s);
            painter.line_segment([c + vec2(-4.0, 4.0), c + vec2(4.0, -4.0)], s);
        }
        for l in &self.sch.labels {
            draw_label(painter, tr, l.at, &l.text, l.kind, l.turns, self.sel == Some(Sel::Label(l.id)));
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
                let ink = Stroke::new(1.6, ACCENT);
                if let Some(last) = self.wire.last() {
                    let mut pts = self.wire.clone();
                    pts.extend(elbow(*last, q, self.x_first).into_iter().skip(1));
                    painter.add(egui::Shape::line(pts.iter().map(|p| tr.pt(*p)).collect(), ink));
                }
                painter.circle_stroke(tr.pt(q), 4.0, ink);
            }
            Tool::Net | Tool::Power => {
                let kind = if self.tool == Tool::Net { LabelKind::Net } else { LabelKind::Power };
                let turns = self.sch.pin_direction_at(q).map(|a| (((a - 90.0) / 90.0).round() as i32).rem_euclid(4) as u8).unwrap_or(0);
                draw_label(painter, tr, q, &self.name, kind, turns, true);
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
    (px >= 3.5).then(|| FontId::proportional(px))
}

/// How far a small offset from a text's anchor goes at this zoom (the sheet's
/// first zoom, 11 px to the millimetre, is 1).
fn nudge(tr: &Tr) -> f32 {
    (tr.s / 11.0).clamp(0.2, 4.0) as f32
}

fn draw_part(painter: &egui::Painter, tr: &Tr, part: &Placed, on: bool, ghost: bool) {
    let fade = |c: Color32| if ghost { c.gamma_multiply(0.6) } else { c };
    let body = fade(if on && !ghost { ACCENT } else { BODY });
    let stroke = Stroke::new(1.6, body);
    let t = |q: P| tr.pt(transform(q, part.at, part.turns, part.mirror));
    for d in part.drawings() {
        match d {
            Drawing::Line(a, b) => {
                painter.line_segment([t(*a), t(*b)], stroke);
            }
            Drawing::Rect(a, b) => {
                let corners = [*a, p(b.x, a.y), *b, p(a.x, b.y)];
                painter.add(egui::Shape::closed_line(corners.iter().map(|q| t(*q)).collect(), stroke));
            }
            Drawing::Polyline(v) => {
                painter.add(egui::Shape::line(v.iter().map(|q| t(*q)).collect(), stroke));
            }
            Drawing::Polygon(v) => {
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
                let pts = (0..40).map(|k| {
                    let a = k as f64 * std::f64::consts::TAU / 40.0;
                    t(c.add(p(a.cos() * rx, a.sin() * ry)))
                });
                painter.add(egui::Shape::closed_line(pts.collect(), stroke));
            }
            Drawing::Text { at, text } => {
                if let Some(font) = sized(tr, 1.1) {
                    painter.text(t(*at), Align2::LEFT_BOTTOM, text, font, body);
                }
            }
        }
    }
    for pin in part.pins() {
        let (a, tip) = (tr.pt(pin.at), tr.pt(pin.tip));
        painter.line_segment([a, tip], Stroke::new(1.6, fade(PIN)));
        if pin.hidden {
            continue;
        }
        let rad = pin.angle.to_radians();
        let dir = vec2(rad.cos() as f32, -(rad.sin() as f32));
        let mid = (a + tip.to_vec2()) / 2.0;
        let k = nudge(tr);
        if let Some(font) = sized(tr, 0.95) {
            painter.text(mid + vec2(-dir.y, dir.x) * 8.0 * k, Align2::CENTER_CENTER, &pin.number, font, fade(DIM));
        }
        if !pin.name.is_empty()
            && let Some(font) = sized(tr, 1.05)
        {
            let (anchor, align) = match ((pin.angle / 90.0).round() as i32) & 3 {
                0 => (a + vec2(5.0 * k, 0.0), Align2::LEFT_CENTER),
                2 => (a + vec2(-5.0 * k, 0.0), Align2::RIGHT_CENTER),
                1 => (a + vec2(0.0, -5.0 * k), Align2::CENTER_BOTTOM),
                _ => (a + vec2(0.0, 5.0 * k), Align2::CENTER_TOP),
            };
            painter.text(anchor, align, &pin.name, font, fade(FG));
        }
    }
    let bb = part.bbox();
    if !part.designator.is_empty() {
        if let Some(font) = sized(tr, 1.2) {
            painter.text(tr.pt(p(bb.min.x, bb.max.y)) + vec2(0.0, -3.0 * nudge(tr)), Align2::LEFT_BOTTOM, &part.designator, font, fade(FG));
        }
    }
    if !part.value.is_empty() {
        if let Some(font) = sized(tr, 1.1) {
            painter.text(tr.pt(p(bb.min.x, bb.min.y)) + vec2(0.0, 3.0 * nudge(tr)), Align2::LEFT_TOP, &part.value, font, fade(DIM));
        }
    }
}

fn draw_label(painter: &egui::Painter, tr: &Tr, at: P, text: &str, kind: LabelKind, turns: u8, on: bool) {
    let color = if on { ACCENT } else { WIRE };
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
            painter.circle_filled(c, 2.5, color);
            if let Some(font) = sized(tr, 1.2) {
                let k = nudge(tr);
                painter.text(c + vec2(4.0 * k, -3.0 * k), Align2::LEFT_BOTTOM, text, font, color);
            }
        }
        LabelKind::Power => {
            let top = c + step * 14.0;
            let side = vec2(-step.y, step.x);
            painter.line_segment([c, top], Stroke::new(1.6, color));
            painter.line_segment([top + side * 7.0, top - side * 7.0], Stroke::new(2.0, color));
            if let Some(font) = sized(tr, 1.15) {
                painter.text(top + step * 4.0 * nudge(tr), if step.y < 0.0 { Align2::CENTER_BOTTOM } else if step.y > 0.0 { Align2::CENTER_TOP } else if step.x > 0.0 { Align2::LEFT_CENTER } else { Align2::RIGHT_CENTER }, text, font, color);
            }
        }
        LabelKind::Port => {
            let k = nudge(tr);
            let font = sized(tr, 1.15).unwrap_or_else(|| FontId::proportional(3.5));
            let galley = painter.layout_no_wrap(text.to_string(), font, color);
            let r = Rect::from_min_size(c + vec2(0.0, -galley.size().y / 2.0 - 3.0 * k), galley.size() + vec2(14.0 * k, 6.0 * k));
            painter.rect_stroke(r, 2.0, Stroke::new(1.4, color), egui::StrokeKind::Middle);
            painter.galley(r.min + vec2(7.0 * k, 3.0 * k), galley, color);
        }
    }
    let _ = pos2(0.0, 0.0);
    let _ = view::BOARD;
}
