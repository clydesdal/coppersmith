//! The library editor's window: a list of what's in the library, the drawing
//! to edit, and the details of what's picked. What the edits do is in
//! `libedit`; this is drawing, pointing and the file dialogs.

use crate::pcb::altium::schlib::{Drawing, PinKind};
use crate::pcb::geom::*;
use crate::pcb::model::*;
use crate::ui::icons::Icon;
use crate::ui::libedit::{FOOTPRINT_GRIDS, Item, Kind, LibEditor, SYMBOL_GRIDS, Tool};
use crate::ui::panels::{card, chip, field, segmented, small_icon};
use crate::ui::theme::{self, ACCENT, DIM, FAINT, FG, RED, YELLOW, semibold};
use crate::ui::view::{self, Tr, fill_blob, layer_color};
use eframe::egui::{self, Align, Align2, Color32, FontId, Layout, Panel, PointerButton, Rect, RichText, ScrollArea, Sense, Stroke, Ui, pos2, vec2};
use std::path::PathBuf;
use std::sync::mpsc;

/// What the user chose in a file dialog.
enum Dialog {
    Open(PathBuf),
    SaveAs(PathBuf),
}

/// What the window asks of the app around it.
#[derive(Default)]
pub struct Outcome {
    pub close: bool,
    /// Footprints were saved where the board's library reads them from.
    pub reload_parts: bool,
    pub notice: Option<String>,
}

pub struct LibWindow {
    pub ed: LibEditor,
    center: P,
    scale: f64,
    /// Fit the drawing to the window on the next frame.
    fit: bool,
    shown: (Kind, usize),
    /// A thing being dragged: what, where it started, where the pointer did.
    grab: Option<(Item, P, P)>,
    hover: Option<P>,
    rows: usize,
    pitch: f64,
    spacing: f64,
    tx: mpsc::Sender<Dialog>,
    rx: mpsc::Receiver<Dialog>,
}

impl LibWindow {
    pub fn new(kind: Kind) -> LibWindow {
        let (tx, rx) = mpsc::channel();
        LibWindow { ed: LibEditor::new(kind), center: p(0.0, 0.0), scale: 40.0, fit: true, shown: (kind, usize::MAX), grab: None, hover: None, rows: 8, pitch: 1.27, spacing: 5.4, tx, rx }
    }

    pub fn open_file(&mut self, path: &std::path::Path) -> Option<String> {
        let out = self.ed.open(path);
        self.fit = true;
        Some(out.unwrap_or_else(|e| e))
    }

    fn ask_open(&self, ctx: &egui::Context) {
        let (tx, ctx) = (self.tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            let dialog = rfd::FileDialog::new().set_title("Open an Altium library").add_filter("Altium libraries", &["PcbLib", "pcblib", "SchLib", "schlib", "IntLib", "intlib"]);
            if let Some(path) = dialog.pick_file() {
                let _ = tx.send(Dialog::Open(path));
                ctx.request_repaint();
            }
        });
    }

    fn ask_save_as(&self, ctx: &egui::Context) {
        let (tx, ctx) = (self.tx.clone(), ctx.clone());
        let (title, ext, file) = match self.ed.kind {
            Kind::Footprints => ("Save the footprints as a PcbLib", "PcbLib", format!("{}.PcbLib", self.ed.name)),
            Kind::Symbols => ("Save the symbols as a SchLib", "SchLib", format!("{}.SchLib", self.ed.name)),
        };
        std::thread::spawn(move || {
            let dialog = rfd::FileDialog::new().set_title(title).add_filter("Altium library", &[ext]).set_file_name(file);
            if let Some(path) = dialog.save_file() {
                let _ = tx.send(Dialog::SaveAs(path));
                ctx.request_repaint();
            }
        });
    }

    fn save(&mut self, ctx: &egui::Context, out: &mut Outcome) {
        match self.ed.path().map(|p| p.to_path_buf()) {
            Some(path) => self.save_to(path, out),
            None => self.ask_save_as(ctx),
        }
    }

    fn save_to(&mut self, path: PathBuf, out: &mut Outcome) {
        out.notice = Some(match self.ed.save_to(&path) {
            Ok(()) => format!("Saved {}", path.display()),
            Err(e) => e,
        });
    }

    /// Saves the footprints where the board's Parts list looks for them.
    fn save_to_parts(&mut self, out: &mut Outcome) {
        let Some(dir) = crate::pcb::library::user_folder() else {
            out.notice = Some("There's no folder for your own footprints on this machine".into());
            return;
        };
        let name: String = self.ed.name.chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
        let was = self.ed.kind;
        self.ed.kind = Kind::Footprints;
        let path = dir.join(format!("{name}.PcbLib"));
        match self.ed.save_to(&path) {
            Ok(()) => {
                out.reload_parts = true;
                out.notice = Some(format!("Saved {}; its footprints are in the Parts list", path.display()));
            }
            Err(e) => out.notice = Some(e),
        }
        self.ed.kind = was;
    }

    fn pump(&mut self, out: &mut Outcome) {
        while let Ok(d) = self.rx.try_recv() {
            match d {
                Dialog::Open(path) => out.notice = self.open_file(&path),
                Dialog::SaveAs(path) => self.save_to(path, out),
            }
        }
    }

    /// The whole window: toolbar, list, details and drawing.
    pub fn show(&mut self, ui: &mut Ui, elsewhere: &[String]) -> Outcome {
        let ctx = ui.ctx().clone();
        let mut out = Outcome::default();
        self.pump(&mut out);
        if self.shown != (self.ed.kind, self.ed.current()) {
            self.shown = (self.ed.kind, self.ed.current());
            self.fit = true;
        }
        let bar = egui::Frame::new().fill(theme::PANEL).inner_margin(egui::Margin::symmetric(12, 6));
        Panel::top("lib_tools").resizable(false).frame(bar).show(ui, |ui| {
            ScrollArea::horizontal().auto_shrink([false, true]).show(ui, |ui| self.toolbar(ui, &ctx, &mut out));
        });
        let side = egui::Frame::new().fill(theme::PANEL).inner_margin(egui::Margin::same(10));
        Panel::left("lib_list").resizable(true).default_size(250.0).size_range(200.0..=400.0).frame(side).show(ui, |ui| self.list(ui));
        let side = egui::Frame::new().fill(theme::PANEL).inner_margin(egui::Margin::same(10));
        Panel::right("lib_props").resizable(true).default_size(290.0).size_range(240.0..=440.0).frame(side).show(ui, |ui| {
            ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| self.details(ui, elsewhere));
        });
        egui::CentralPanel::default().frame(egui::Frame::new().fill(theme::WELL)).show(ui, |ui| self.canvas(ui, &ctx, &mut out));
        out
    }

    fn toolbar(&mut self, ui: &mut Ui, ctx: &egui::Context, out: &mut Outcome) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            let mut kind = self.ed.kind;
            if segmented(ui, &mut kind, &[(Kind::Footprints, "Footprints"), (Kind::Symbols, "Symbols")]) {
                self.ed.set_kind(kind);
            }
            ui.separator();
            let symbols = self.ed.kind == Kind::Symbols;
            let tools = [(Tool::Select, "Select"), (Tool::Pad, if symbols { "Pin" } else { "Pad" }), (Tool::Line, "Line"), (Tool::Shape, if symbols { "Rectangle" } else { "Circle" })];
            for (tool, name) in tools {
                if chip(ui, name, self.ed.tool == tool).clicked() {
                    self.ed.tool = tool;
                    self.ed.pending = None;
                }
            }
            ui.separator();
            if !symbols {
                let layers = [(Layer::Silk(Side::Top), "Silkscreen"), (Layer::Court(Side::Top), "Courtyard"), (Layer::Fab(Side::Top), "Assembly")];
                let name = layers.iter().find(|(l, _)| *l == self.ed.layer).map(|(_, n)| *n).unwrap_or("Silkscreen");
                egui::ComboBox::from_id_salt("lib_layer").selected_text(name).width(110.0).show_ui(ui, |ui| {
                    for (l, n) in layers {
                        ui.selectable_value(&mut self.ed.layer, l, n);
                    }
                });
            } else if let Some(s) = self.ed.symbol() {
                let parts = s.parts;
                if parts > 1 {
                    let mut part = self.ed.part;
                    egui::ComboBox::from_id_salt("lib_part").selected_text(format!("Part {part}")).width(80.0).show_ui(ui, |ui| {
                        for k in 1..=parts {
                            ui.selectable_value(&mut part, k, format!("Part {k}"));
                        }
                    });
                    self.ed.part = part;
                }
            }
            let grids: &[f64] = if symbols { &SYMBOL_GRIDS } else { &FOOTPRINT_GRIDS };
            egui::ComboBox::from_id_salt("lib_grid").selected_text(format!("Grid {} mm", self.ed.grid)).width(110.0).show_ui(ui, |ui| {
                for g in grids {
                    ui.selectable_value(&mut self.ed.grid, *g, format!("{g} mm"));
                }
            });
            ui.separator();
            if small_icon(ui, Icon::Undo, "Undo (Ctrl+Z)", self.ed.can_undo()).clicked() {
                self.ed.undo();
            }
            if small_icon(ui, Icon::Redo, "Redo (Ctrl+Shift+Z)", self.ed.can_redo()).clicked() {
                self.ed.redo();
            }
            if small_icon(ui, Icon::Fit, "Fit the drawing to the window", true).clicked() {
                self.fit = true;
            }
            ui.separator();
            if small_icon(ui, Icon::Folder, "Open a PcbLib, SchLib or IntLib", true).clicked() {
                self.ask_open(ctx);
            }
            if small_icon(ui, Icon::Save, "Save the library (Ctrl+S)", self.ed.count() > 0).clicked() {
                self.save(ctx, out);
            }
            if chip(ui, "Save as…", false).clicked() {
                self.ask_save_as(ctx);
            }
            if !symbols && chip(ui, "Save to my footprints", false).on_hover_text("Write a PcbLib into the folder the Parts list reads, so they can be placed on a board").clicked() {
                self.save_to_parts(out);
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if chip(ui, "Back to the board", false).clicked() {
                    out.close = true;
                }
                if self.ed.dirty {
                    ui.label(RichText::new("unsaved").size(12.0).color(YELLOW));
                }
            });
        });
    }

    // -- Left: what's in the library ----------------------------------------------------

    fn list(&mut self, ui: &mut Ui) {
        let symbols = self.ed.kind == Kind::Symbols;
        ui.horizontal(|ui| {
            ui.label(RichText::new(if symbols { "Symbols" } else { "Footprints" }).size(14.0).color(DIM));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if small_icon(ui, Icon::Trash, "Remove it from the library", self.ed.count() > 0).clicked() {
                    self.ed.remove_current();
                }
                if small_icon(ui, Icon::Copy, "Copy it", self.ed.count() > 0).clicked() {
                    self.ed.duplicate();
                }
                if small_icon(ui, Icon::Plus, "A new one", true).clicked() {
                    self.ed.new_item();
                }
            });
        });
        ui.label(RichText::new(&self.ed.name).size(12.0).color(FAINT));
        ui.add(egui::TextEdit::singleline(&mut self.ed.filter).hint_text("Search").desired_width(f32::INFINITY));
        if !self.ed.notes.is_empty() {
            let text = format!("{} approximated on reading", self.ed.notes.len());
            ui.label(RichText::new(text).size(11.5).color(FAINT)).on_hover_text(self.ed.notes.join("\n"));
        }
        ui.add_space(6.0);
        let words = self.ed.filter.to_lowercase();
        let names = self.ed.names();
        let mut pick = None;
        let room = if self.wizard_room() { 215.0 } else { 0.0 };
        ScrollArea::vertical().max_height((ui.available_height() - room).max(80.0)).auto_shrink([false, false]).show(ui, |ui| {
            for (i, name) in names.iter().enumerate() {
                if !words.split_whitespace().all(|w| name.to_lowercase().contains(w)) {
                    continue;
                }
                let on = i == self.ed.current();
                let (rect, r) = ui.allocate_exact_size(vec2(ui.available_width(), 28.0), Sense::click());
                let fill = if on { theme::ACCENT_SOFT } else if r.hovered() { theme::RAISED } else { Color32::TRANSPARENT };
                ui.painter().rect_filled(rect, 7.0, fill);
                ui.painter().text(rect.left_center() + vec2(8.0, 0.0), Align2::LEFT_CENTER, name, FontId::proportional(13.5), if on { ACCENT } else { FG });
                if r.clicked() {
                    pick = Some(i);
                }
            }
        });
        if let Some(i) = pick {
            self.ed.choose(i);
        }
        if !symbols {
            self.wizards(ui);
        }
    }

    fn wizard_room(&self) -> bool {
        self.ed.kind == Kind::Footprints
    }

    /// Makers of pad patterns.
    fn wizards(&mut self, ui: &mut Ui) {
        ui.separator();
        ui.label(RichText::new("Pad patterns").size(13.0).color(DIM));
        field(ui, "Pins a side", |ui| ui.add(egui::DragValue::new(&mut self.rows).range(1..=200)));
        field(ui, "Pitch", |ui| ui.add(egui::DragValue::new(&mut self.pitch).speed(0.01).range(0.1..=10.0).suffix(" mm")));
        field(ui, "Row spacing", |ui| ui.add(egui::DragValue::new(&mut self.spacing).speed(0.05).range(0.5..=60.0).suffix(" mm")));
        ui.horizontal(|ui| {
            let has = self.ed.footprint().is_some();
            if ui.add_enabled(has, egui::Button::new("Two rows")).on_hover_text("Replace the pads with two rows, numbered as a DIP or SOIC is, with an outline").clicked() {
                self.ed.dual_row(self.rows, self.pitch, self.spacing);
                self.fit = true;
            }
            if ui.add_enabled(has, egui::Button::new("One row")).on_hover_text("Add a row of pads along x, numbered on").clicked() {
                self.ed.pad_row(self.rows, self.pitch, p(0.0, 0.0), true);
                self.fit = true;
            }
        });
    }

    // -- Right: the details of what's picked ------------------------------------------------

    fn details(&mut self, ui: &mut Ui, elsewhere: &[String]) {
        if self.ed.count() == 0 {
            card(ui, |ui| {
                ui.label(RichText::new("Nothing here yet").font(semibold(14.0)));
                ui.label(RichText::new("Press + for a new one, or open a library with the folder button.").size(12.5).color(FAINT));
            });
            return;
        }
        match self.ed.kind {
            Kind::Footprints => self.footprint_details(ui),
            Kind::Symbols => self.symbol_details(ui, elsewhere),
        }
    }

    fn footprint_details(&mut self, ui: &mut Ui) {
        let Some(orig) = self.ed.footprint().cloned() else { return };
        let mut f = orig.clone();
        card(ui, |ui| {
            ui.label(RichText::new("Footprint").font(semibold(14.0)));
            field(ui, "Name", |ui| ui.add(egui::TextEdit::singleline(&mut f.name).desired_width(150.0)));
            field(ui, "Description", |ui| ui.add(egui::TextEdit::singleline(&mut f.description).desired_width(150.0)));
            field(ui, "Letter", |ui| ui.add(egui::TextEdit::singleline(&mut f.prefix).desired_width(60.0)));
            ui.label(RichText::new(format!("{} pads · {} lines and circles", f.pads.len(), f.graphics.len())).size(12.0).color(FAINT));
        });
        match self.ed.sel {
            Some(Item::Pad(i)) if i < f.pads.len() => {
                card(ui, |ui| {
                    ui.label(RichText::new("Pad").font(semibold(14.0)));
                    pad_fields(ui, &mut f.pads[i], true);
                });
            }
            Some(Item::Gfx(i)) if i < f.graphics.len() => {
                card(ui, |ui| {
                    ui.label(RichText::new(if matches!(f.graphics[i], Graphic::Line { .. }) { "Line" } else { "Circle" }).font(semibold(14.0)));
                    graphic_fields(ui, &mut f.graphics[i]);
                });
            }
            _ if self.ed.tool == Tool::Pad => {
                card(ui, |ui| {
                    ui.label(RichText::new("Pads put down").font(semibold(14.0)));
                    pad_fields(ui, &mut self.ed.pad_proto, false);
                });
            }
            _ => {}
        }
        if let Some(item) = self.ed.sel
            && ui.button("Delete").clicked()
        {
            self.ed.delete(item);
            return;
        }
        if f != orig {
            // A name that another footprint has isn't taken.
            if f.name != orig.name && (f.name.trim().is_empty() || self.ed.footprints.iter().enumerate().any(|(k, o)| k != self.ed.fp && o.name == f.name)) {
                f.name = orig.name.clone();
            }
            self.ed.remember_run();
            self.ed.footprints[self.ed.fp] = f;
        }
    }

    fn symbol_details(&mut self, ui: &mut Ui, elsewhere: &[String]) {
        let Some(orig) = self.ed.symbol().cloned() else { return };
        let mut s = orig.clone();
        let unknown = self.ed.unknown_links(elsewhere);
        card(ui, |ui| {
            ui.label(RichText::new("Symbol").font(semibold(14.0)));
            field(ui, "Name", |ui| ui.add(egui::TextEdit::singleline(&mut s.name).desired_width(150.0)));
            field(ui, "Description", |ui| ui.add(egui::TextEdit::singleline(&mut s.description).desired_width(150.0)));
            field(ui, "Letter", |ui| ui.add(egui::TextEdit::singleline(&mut s.prefix).desired_width(60.0)));
            field(ui, "Parts", |ui| ui.add(egui::DragValue::new(&mut s.parts).range(1..=26)));
            let mut links = s.footprints.join(", ");
            let r = field(ui, "Footprints", |ui| ui.add(egui::TextEdit::singleline(&mut links).desired_width(150.0).hint_text("first is the one used")));
            if r.changed() {
                s.footprints = links.split(',').map(|n| n.trim().to_string()).filter(|n| !n.is_empty()).collect();
            }
            if !unknown.is_empty() {
                let shown: Vec<&str> = unknown.iter().take(2).map(|s| s.as_str()).collect();
                let more = if unknown.len() > 2 { format!(" and {} more", unknown.len() - 2) } else { String::new() };
                ui.label(RichText::new(format!("Not in a footprint library here: {}{more}", shown.join(", "))).size(12.0).color(YELLOW)).on_hover_text(unknown.join("\n"));
            }
            ui.label(RichText::new(format!("{} pins", s.pins.len())).size(12.0).color(FAINT));
        });
        match self.ed.sel {
            Some(Item::Pin(i)) if i < s.pins.len() => {
                card(ui, |ui| {
                    ui.label(RichText::new("Pin").font(semibold(14.0)));
                    let pin = &mut s.pins[i];
                    field(ui, "Number", |ui| ui.add(egui::TextEdit::singleline(&mut pin.number).desired_width(80.0)));
                    field(ui, "Name", |ui| ui.add(egui::TextEdit::singleline(&mut pin.name).desired_width(110.0)));
                    let kinds = [
                        (PinKind::Input, "Input"),
                        (PinKind::InOut, "I/O"),
                        (PinKind::Output, "Output"),
                        (PinKind::OpenCollector, "Open collector"),
                        (PinKind::Passive, "Passive"),
                        (PinKind::HiZ, "Hi-Z"),
                        (PinKind::OpenEmitter, "Open emitter"),
                        (PinKind::Power, "Power"),
                    ];
                    let shown = kinds.iter().find(|(k, _)| *k == pin.kind).map(|(_, n)| *n).unwrap_or("");
                    field(ui, "Type", |ui| {
                        egui::ComboBox::from_id_salt("pin_kind").selected_text(shown).width(110.0).show_ui(ui, |ui| {
                            for (k, n) in kinds {
                                ui.selectable_value(&mut pin.kind, k, n);
                            }
                        })
                    });
                    field(ui, "Points", |ui| {
                        egui::ComboBox::from_id_salt("pin_angle").selected_text(["right", "up", "left", "down"][((pin.angle / 90.0).round() as usize) & 3]).width(80.0).show_ui(ui, |ui| {
                            for (a, n) in [(0.0, "right"), (90.0, "up"), (180.0, "left"), (270.0, "down")] {
                                ui.selectable_value(&mut pin.angle, a, n);
                            }
                        })
                    });
                    field(ui, "Length", |ui| ui.add(egui::DragValue::new(&mut pin.length).speed(0.254).range(0.0..=50.0).suffix(" mm")));
                    field(ui, "X", |ui| ui.add(egui::DragValue::new(&mut pin.at.x).speed(0.254).suffix(" mm")));
                    field(ui, "Y", |ui| ui.add(egui::DragValue::new(&mut pin.at.y).speed(0.254).suffix(" mm")));
                    ui.checkbox(&mut pin.hidden, "Hidden");
                });
            }
            Some(Item::Draw(i)) if i < s.drawings.len() => {
                card(ui, |ui| {
                    let what = match &s.drawings[i].1 {
                        Drawing::Line(..) => "Line",
                        Drawing::Rect(..) => "Rectangle",
                        Drawing::Polyline(..) => "Polyline",
                        Drawing::Polygon(..) => "Polygon",
                        Drawing::Arc { .. } => "Arc",
                        Drawing::Ellipse { .. } => "Ellipse",
                        Drawing::Text { .. } => "Text",
                    };
                    ui.label(RichText::new(what).font(semibold(14.0)));
                    ui.label(RichText::new("Drag it to move it.").size(12.0).color(FAINT));
                });
            }
            _ if self.ed.tool == Tool::Pad => {
                card(ui, |ui| {
                    ui.label(RichText::new("Pins put down").font(semibold(14.0)));
                    ui.label(RichText::new("R turns the next one").size(12.0).color(FAINT));
                    field(ui, "Points", |ui| {
                        egui::ComboBox::from_id_salt("proto_angle").selected_text(["right", "up", "left", "down"][((self.ed.pin_angle / 90.0).round() as usize) & 3]).width(80.0).show_ui(ui, |ui| {
                            for (a, n) in [(0.0, "right"), (90.0, "up"), (180.0, "left"), (270.0, "down")] {
                                ui.selectable_value(&mut self.ed.pin_angle, a, n);
                            }
                        })
                    });
                });
            }
            _ => {}
        }
        if let Some(item) = self.ed.sel
            && ui.button("Delete").clicked()
        {
            self.ed.delete(item);
            return;
        }
        if s != orig {
            if s.name != orig.name && (s.name.trim().is_empty() || self.ed.symbols.iter().enumerate().any(|(k, o)| k != self.ed.sym && o.name == s.name)) {
                s.name = orig.name.clone();
            }
            self.ed.remember_run();
            self.ed.symbols[self.ed.sym] = s;
        }
    }

    // -- The drawing --------------------------------------------------------------------

    fn canvas(&mut self, ui: &mut Ui, ctx: &egui::Context, out: &mut Outcome) {
        let (rect, resp) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
        if self.fit {
            self.fit = false;
            let bb = self.ed.extent();
            if bb.is_empty() {
                self.center = p(0.0, 0.0);
                self.scale = if self.ed.kind == Kind::Footprints { 40.0 } else { 12.0 };
            } else {
                self.center = bb.center();
                let (w, h) = (bb.w().max(1.0) * 1.4, bb.h().max(1.0) * 1.4);
                self.scale = ((rect.width() as f64 / w).min(rect.height() as f64 / h)).clamp(4.0, 300.0);
            }
        }
        // Zoom about the pointer; pan with the right or middle button.
        if let Some(at) = resp.hover_pos() {
            let (scroll, zoom) = ui.input(|i| (i.smooth_scroll_delta.y, i.zoom_delta()));
            let factor = ((scroll as f64) * 0.003).exp() * zoom as f64;
            if (factor - 1.0).abs() > 1e-6 {
                let tr = Tr { rect, c: self.center, s: self.scale, m: 1.0 };
                let before = tr.board(at);
                self.scale = (self.scale * factor).clamp(2.0, 2000.0);
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
        let tol = 5.0 / self.scale;

        // Pointing.
        if self.ed.tool == Tool::Select {
            if resp.drag_started_by(PointerButton::Primary)
                && let Some(q) = resp.interact_pointer_pos().map(|q| tr.board(q))
            {
                self.grab = self.ed.pick(q, tol).and_then(|item| {
                    self.ed.sel = Some(item);
                    self.ed.remember();
                    self.ed.anchor(item).map(|a| (item, a, q))
                });
            }
            if resp.dragged_by(PointerButton::Primary)
                && let (Some((item, a0, q0)), Some(q)) = (self.grab, resp.interact_pointer_pos().map(|q| tr.board(q)))
                && let Some(now) = self.ed.anchor(item)
            {
                let want = self.ed.snap(a0.add(q.sub(q0)));
                let d = want.sub(now);
                if d.x != 0.0 || d.y != 0.0 {
                    self.ed.translate(item, d);
                }
            }
            if resp.drag_stopped() {
                self.grab = None;
            }
            if resp.clicked_by(PointerButton::Primary)
                && let Some(q) = resp.interact_pointer_pos().map(|q| tr.board(q))
            {
                self.ed.sel = self.ed.pick(q, tol);
            }
        } else if resp.clicked_by(PointerButton::Primary)
            && let Some(q) = resp.interact_pointer_pos().map(|q| tr.board(q))
        {
            self.ed.place(q);
        }
        if resp.clicked_by(PointerButton::Secondary) {
            self.ed.pending = None;
            self.ed.tool = Tool::Select;
        }

        // Keys, when no field is being typed in.
        if !ctx.egui_wants_keyboard_input() {
            let (esc, del, turn, undo, redo, save) = ctx.input(|i| {
                let cmd = i.modifiers.command;
                (
                    i.key_pressed(egui::Key::Escape),
                    i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace),
                    i.key_pressed(egui::Key::R) && !cmd,
                    cmd && !i.modifiers.shift && i.key_pressed(egui::Key::Z),
                    cmd && i.modifiers.shift && i.key_pressed(egui::Key::Z),
                    cmd && i.key_pressed(egui::Key::S),
                )
            });
            if esc {
                if self.ed.pending.is_some() {
                    self.ed.pending = None;
                } else if self.ed.tool != Tool::Select {
                    self.ed.tool = Tool::Select;
                } else {
                    self.ed.sel = None;
                }
            }
            if del && let Some(item) = self.ed.sel {
                self.ed.delete(item);
            }
            if turn {
                match self.ed.sel {
                    Some(item) => self.ed.rotate(item),
                    None if self.ed.kind == Kind::Symbols => self.ed.pin_angle = (self.ed.pin_angle + 90.0) % 360.0,
                    None => {}
                }
            }
            if undo {
                self.ed.undo();
            }
            if redo {
                self.ed.redo();
            }
            if save {
                self.save(ctx, out);
            }
        }

        // Drawing.
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, view::BOARD);
        self.draw_grid(&painter, &tr, rect);
        match self.ed.kind {
            Kind::Footprints => self.draw_footprint(&painter, &tr),
            Kind::Symbols => self.draw_symbol(&painter, &tr),
        }
        self.draw_pending(&painter, &tr);
        if self.ed.count() == 0 {
            painter.text(rect.center(), Align2::CENTER_CENTER, "Press + to make one", FontId::proportional(15.0), FAINT);
        }
        if let Some(q) = self.hover {
            let q = self.ed.snap(q);
            let text = format!("X {:.3}  Y {:.3} mm", q.x, q.y);
            painter.text(rect.left_bottom() + vec2(10.0, -8.0), Align2::LEFT_BOTTOM, text, theme::mono(12.0), DIM);
        }
        let hint = match (self.ed.kind, self.ed.tool) {
            (_, Tool::Select) => "Click to pick · drag to move · R turns · Delete removes · right-drag pans · scroll zooms",
            (Kind::Footprints, Tool::Pad) => "Click to put a pad down · Esc stops",
            (Kind::Symbols, Tool::Pad) => "Click to put a pin down · R turns the next one · Esc stops",
            (_, Tool::Line) => "Click the two ends · Esc stops",
            (Kind::Footprints, Tool::Shape) => "Click the centre, then a point on the circle",
            (Kind::Symbols, Tool::Shape) => "Click two opposite corners",
        };
        painter.text(rect.right_bottom() + vec2(-10.0, -8.0), Align2::RIGHT_BOTTOM, hint, FontId::proportional(12.0), FAINT);
    }

    fn draw_grid(&self, painter: &egui::Painter, tr: &Tr, rect: Rect) {
        let mut step = self.ed.grid.max(0.01);
        while step * tr.s < 10.0 {
            step *= 2.0;
        }
        let (lo, hi) = (tr.board(rect.left_bottom()), tr.board(rect.right_top()));
        let dot = Color32::from_gray(48);
        let mut x = (lo.x / step).floor() * step;
        while x <= hi.x {
            let mut y = (lo.y / step).floor() * step;
            while y <= hi.y {
                painter.circle_filled(tr.pt(p(x, y)), 0.9, dot);
                y += step;
            }
            x += step;
        }
        let o = tr.pt(p(0.0, 0.0));
        let axis = Stroke::new(1.0, Color32::from_gray(70));
        painter.line_segment([pos2(rect.left(), o.y), pos2(rect.right(), o.y)], axis);
        painter.line_segment([pos2(o.x, rect.top()), pos2(o.x, rect.bottom())], axis);
    }

    fn draw_footprint(&self, painter: &egui::Painter, tr: &Tr) {
        let Some(f) = self.ed.footprint() else { return };
        let picked = self.ed.sel;
        for (i, g) in f.graphics.iter().enumerate() {
            let on = picked == Some(Item::Gfx(i));
            let color = if on { ACCENT } else { layer_color(g.layer()) };
            match g {
                Graphic::Line { a, b, w, .. } => {
                    painter.line_segment([tr.pt(*a), tr.pt(*b)], Stroke::new(tr.len(*w).max(1.5), color));
                }
                Graphic::Circle { c, r, w, fill, .. } => {
                    if *fill {
                        painter.circle_filled(tr.pt(*c), tr.len(*r), color);
                    } else {
                        painter.circle_stroke(tr.pt(*c), tr.len(*r), Stroke::new(tr.len(*w).max(1.5), color));
                    }
                }
            }
        }
        for k in &f.keepouts {
            let mut pts: Vec<egui::Pos2> = k.pts.iter().map(|q| tr.pt(*q)).collect();
            if let Some(first) = pts.first().copied() {
                pts.push(first);
                painter.add(egui::Shape::line(pts, Stroke::new(1.0, view::KEEPOUT)));
            }
        }
        for (i, pad) in f.pads.iter().enumerate() {
            let on = picked == Some(Item::Pad(i));
            let color = if pad.through() { view::THROUGH } else { layer_color(Layer::Cu(TOP)) };
            if !pad.bare_hole() {
                fill_blob(painter, tr, &pad.blob_local(0.0), color);
            }
            if pad.through() {
                painter.circle_filled(tr.pt(pad.at), tr.len(pad.drill / 2.0), theme::WELL);
            }
            if on {
                let b = pad.blob_local(0.06).bbox();
                painter.rect_stroke(Rect::from_two_pos(tr.pt(p(b.min.x, b.max.y)), tr.pt(p(b.max.x, b.min.y))), 2.0, Stroke::new(1.5, ACCENT), egui::StrokeKind::Outside);
            }
            if tr.len(pad.size.x.min(pad.size.y)) > 11.0 && !pad.num.is_empty() {
                painter.text(tr.pt(pad.at), Align2::CENTER_CENTER, &pad.num, FontId::proportional(11.0), Color32::BLACK);
            }
        }
    }

    fn draw_symbol(&self, painter: &egui::Painter, tr: &Tr) {
        let Some(s) = self.ed.symbol() else { return };
        let picked = self.ed.sel;
        let body = |on: bool| if on { ACCENT } else { Color32::from_rgb(0xe6, 0xc0, 0x7b) };
        for (i, (part, d)) in s.drawings.iter().enumerate() {
            if *part != 0 && *part != self.ed.part {
                continue;
            }
            let stroke = Stroke::new(1.6, body(picked == Some(Item::Draw(i))));
            match d {
                Drawing::Line(a, b) => {
                    painter.line_segment([tr.pt(*a), tr.pt(*b)], stroke);
                }
                Drawing::Rect(a, b) => {
                    painter.rect_stroke(Rect::from_two_pos(tr.pt(*a), tr.pt(*b)), 0.0, stroke, egui::StrokeKind::Middle);
                }
                Drawing::Polyline(v) => {
                    painter.add(egui::Shape::line(v.iter().map(|q| tr.pt(*q)).collect(), stroke));
                }
                Drawing::Polygon(v) => {
                    painter.add(egui::Shape::closed_line(v.iter().map(|q| tr.pt(*q)).collect(), stroke));
                }
                Drawing::Arc { c, r, start, end } => {
                    let mut sweep = (end - start).rem_euclid(360.0);
                    if sweep < 1e-9 {
                        sweep = 360.0;
                    }
                    let n = ((sweep / 6.0).ceil() as usize).max(2);
                    let pts = (0..=n).map(|k| {
                        let t = (start + sweep * k as f64 / n as f64).to_radians();
                        tr.pt(c.add(p(t.cos(), t.sin()).scale(*r)))
                    });
                    painter.add(egui::Shape::line(pts.collect(), stroke));
                }
                Drawing::Ellipse { c, rx, ry } => {
                    let pts = (0..48).map(|k| {
                        let t = k as f64 * std::f64::consts::TAU / 48.0;
                        tr.pt(c.add(p(t.cos() * rx, t.sin() * ry)))
                    });
                    painter.add(egui::Shape::closed_line(pts.collect(), stroke));
                }
                Drawing::Text { at, text } => {
                    painter.text(tr.pt(*at), Align2::LEFT_BOTTOM, text, FontId::proportional(13.0), body(picked == Some(Item::Draw(i))));
                }
            }
        }
        for (i, pin) in s.pins.iter().enumerate() {
            if pin.part != self.ed.part {
                continue;
            }
            let on = picked == Some(Item::Pin(i));
            let color = if on { ACCENT } else { RED };
            let (a, tip) = (tr.pt(pin.at), tr.pt(pin.tip()));
            painter.line_segment([a, tip], Stroke::new(if on { 2.4 } else { 1.6 }, color));
            painter.circle_filled(tip, 2.5, color);
            if !pin.hidden || on {
                let dir = vec2(pin.angle.to_radians().cos() as f32, -(pin.angle.to_radians().sin() as f32));
                let side = vec2(-dir.y, dir.x);
                // The number sits above the pin's outer half, the name inside the body.
                painter.text((a + tip.to_vec2()) / 2.0 - side * 9.0, Align2::CENTER_CENTER, &pin.number, FontId::proportional(11.0), DIM);
                if tr.len(1.0) > 6.0 {
                    let (anchor, align) = match ((pin.angle / 90.0).round() as i32) & 3 {
                        0 => (a + vec2(6.0, 0.0), Align2::LEFT_CENTER),
                        2 => (a + vec2(-6.0, 0.0), Align2::RIGHT_CENTER),
                        1 => (a + vec2(0.0, -6.0), Align2::CENTER_BOTTOM),
                        _ => (a + vec2(0.0, 6.0), Align2::CENTER_TOP),
                    };
                    painter.text(anchor, align, &pin.name, FontId::proportional(12.0), FG);
                }
            }
        }
    }

    /// What the next click would complete, drawn faintly.
    fn draw_pending(&self, painter: &egui::Painter, tr: &Tr) {
        let (Some(a), Some(q)) = (self.ed.pending, self.hover) else { return };
        let q = self.ed.snap(q);
        let stroke = Stroke::new(1.2, ACCENT);
        match (self.ed.kind, self.ed.tool) {
            (_, Tool::Line) => {
                painter.line_segment([tr.pt(a), tr.pt(q)], stroke);
            }
            (Kind::Footprints, Tool::Shape) => {
                painter.circle_stroke(tr.pt(a), tr.len(a.dist(q)), stroke);
            }
            (Kind::Symbols, Tool::Shape) => {
                painter.rect_stroke(Rect::from_two_pos(tr.pt(a), tr.pt(q)), 0.0, stroke, egui::StrokeKind::Middle);
            }
            _ => {}
        }
        painter.circle_filled(tr.pt(a), 3.0, ACCENT);
    }
}

fn pad_fields(ui: &mut Ui, pad: &mut Pad, with_position: bool) {
    if with_position {
        field(ui, "Number", |ui| ui.add(egui::TextEdit::singleline(&mut pad.num).desired_width(80.0)));
    }
    let shapes = [("Circle", 0), ("Rectangle", 1), ("Rounded", 2), ("Oval", 3)];
    let current = match pad.shape {
        PadShape::Circle => 0,
        PadShape::Rect => 1,
        PadShape::RoundRect(_) => 2,
        PadShape::Oval => 3,
    };
    let mut chosen = current;
    field(ui, "Shape", |ui| {
        egui::ComboBox::from_id_salt(("pad_shape", with_position)).selected_text(shapes[current].0).width(110.0).show_ui(ui, |ui| {
            for (name, k) in shapes {
                ui.selectable_value(&mut chosen, k, name);
            }
        })
    });
    if chosen != current {
        pad.shape = match chosen {
            0 => PadShape::Circle,
            1 => PadShape::Rect,
            2 => PadShape::RoundRect(0.25),
            _ => PadShape::Oval,
        };
        if chosen == 0 {
            pad.size = p(pad.size.x, pad.size.x);
        }
    }
    if let PadShape::RoundRect(k) = &mut pad.shape {
        field(ui, "Corners", |ui| ui.add(egui::Slider::new(k, 0.0..=0.5).show_value(false)));
    }
    field(ui, "Width", |ui| ui.add(egui::DragValue::new(&mut pad.size.x).speed(0.02).range(0.05..=50.0).suffix(" mm")));
    if pad.shape != PadShape::Circle {
        field(ui, "Height", |ui| ui.add(egui::DragValue::new(&mut pad.size.y).speed(0.02).range(0.05..=50.0).suffix(" mm")));
    } else {
        pad.size.y = pad.size.x;
    }
    field(ui, "Hole", |ui| ui.add(egui::DragValue::new(&mut pad.drill).speed(0.02).range(0.0..=20.0).suffix(" mm")));
    field(ui, "Turn", |ui| ui.add(egui::DragValue::new(&mut pad.rot).speed(1.0).range(0.0..=360.0).suffix("°")));
    if with_position {
        field(ui, "X", |ui| ui.add(egui::DragValue::new(&mut pad.at.x).speed(0.05).suffix(" mm")));
        field(ui, "Y", |ui| ui.add(egui::DragValue::new(&mut pad.at.y).speed(0.05).suffix(" mm")));
    }
}

fn graphic_fields(ui: &mut Ui, g: &mut Graphic) {
    let layers = [(Layer::Silk(Side::Top), "Silkscreen"), (Layer::Court(Side::Top), "Courtyard"), (Layer::Fab(Side::Top), "Assembly")];
    match g {
        Graphic::Line { a, b, w, layer } => {
            layer_field(ui, layer, &layers);
            field(ui, "Width", |ui| ui.add(egui::DragValue::new(w).speed(0.01).range(0.0..=5.0).suffix(" mm")));
            for (name, q) in [("Start", a), ("End", b)] {
                field(ui, &format!("{name} X"), |ui| ui.add(egui::DragValue::new(&mut q.x).speed(0.05).suffix(" mm")));
                field(ui, &format!("{name} Y"), |ui| ui.add(egui::DragValue::new(&mut q.y).speed(0.05).suffix(" mm")));
            }
        }
        Graphic::Circle { c, r, w, layer, fill } => {
            layer_field(ui, layer, &layers);
            field(ui, "Radius", |ui| ui.add(egui::DragValue::new(r).speed(0.02).range(0.01..=100.0).suffix(" mm")));
            field(ui, "Width", |ui| ui.add(egui::DragValue::new(w).speed(0.01).range(0.0..=5.0).suffix(" mm")));
            field(ui, "X", |ui| ui.add(egui::DragValue::new(&mut c.x).speed(0.05).suffix(" mm")));
            field(ui, "Y", |ui| ui.add(egui::DragValue::new(&mut c.y).speed(0.05).suffix(" mm")));
            ui.checkbox(fill, "Filled");
        }
    }
}

fn layer_field(ui: &mut Ui, layer: &mut Layer, layers: &[(Layer, &str)]) {
    let name = layers.iter().find(|(l, _)| l == layer).map(|(_, n)| *n).unwrap_or("Silkscreen");
    field(ui, "Layer", |ui| {
        egui::ComboBox::from_id_salt("gfx_layer").selected_text(name).width(110.0).show_ui(ui, |ui| {
            for (l, n) in layers {
                ui.selectable_value(layer, *l, *n);
            }
        })
    });
}
