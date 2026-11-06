//! The bars and side panels round the board.

use crate::ui::app::{App, Sel, Tool, Unit};
use crate::pcb::drc::Severity;
use crate::pcb::geom::*;
use crate::ui::icons::{self, Icon};
use crate::pcb::library::FootprintDef;
use crate::pcb::model::*;
use crate::ui::theme::{self, ACCENT, ACCENT_SOFT, DIM, FAINT, FG, LINE, RAISED, SELECTION, semibold};
use crate::ui::view::{self, Tr, fill_blob, layer_color};
use eframe::egui::{
    self, Align, Align2, Color32, CornerRadius, FontId, Frame, Layout, Rect, Response, RichText, ScrollArea, Sense,
    Stroke, StrokeKind, Ui, Vec2, pos2, vec2,
};

// -- Pieces shared with Copperview -------------------------------------------

pub fn card<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    let inner = Frame::new()
        .fill(theme::CARD)
        .stroke(Stroke::new(1.0, LINE))
        .corner_radius(CornerRadius::same(theme::CARD_RADIUS))
        .inner_margin(egui::Margin::same(14))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner;
    ui.add_space(10.0);
    inner
}

fn heading(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).font(semibold(15.0)));
    ui.add_space(2.0);
}

fn row(ui: &mut Ui, key: &str, value: &str) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        ui.label(RichText::new(key).color(DIM));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.add(egui::Label::new(RichText::new(value).font(theme::mono(12.5)).color(FG)).truncate());
        });
    });
}

/// A label on the left and something to change on the right.
fn field<R>(ui: &mut Ui, key: &str, add: impl FnOnce(&mut Ui) -> R) -> R {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        ui.label(RichText::new(key).color(DIM));
        ui.with_layout(Layout::right_to_left(Align::Center), add).inner
    })
    .inner
}

pub fn segmented<T: Copy + PartialEq>(ui: &mut Ui, value: &mut T, options: &[(T, &str)]) -> bool {
    let font = FontId::proportional(13.5);
    let widths: Vec<f32> = options
        .iter()
        .map(|(_, t)| ui.painter().layout_no_wrap(t.to_string(), font.clone(), FG).size().x + 20.0)
        .collect();
    let total: f32 = widths.iter().sum::<f32>() + 6.0;
    let (rect, _) = ui.allocate_exact_size(vec2(total, 30.0), Sense::hover());
    let painter = ui.painter_at(rect.expand(1.0));
    painter.rect(rect, 9.0, theme::WELL, Stroke::new(1.0, LINE), StrokeKind::Inside);
    let mut x = rect.left() + 3.0;
    let mut changed = false;
    for (i, (option, text)) in options.iter().enumerate() {
        let seg = Rect::from_min_size(pos2(x, rect.top() + 3.0), vec2(widths[i], rect.height() - 6.0));
        x += widths[i];
        let response = ui.interact(seg, ui.scope_id().with(("seg", i, *text)), Sense::click());
        let on = *value == *option;
        if on {
            painter.rect_filled(seg, 7.0, SELECTION);
        } else if response.hovered() {
            painter.rect_filled(seg, 7.0, RAISED);
        }
        let color = if on || response.hovered() { FG } else { DIM };
        painter.text(seg.center(), Align2::CENTER_CENTER, *text, font.clone(), color);
        if response.clicked() && !on {
            *value = *option;
            changed = true;
        }
    }
    changed
}

pub fn tool_button(ui: &mut Ui, icon: Icon, text: &str, enabled: bool) -> Response {
    let font = FontId::proportional(14.0);
    let galley = ui.painter().layout_no_wrap(text.to_string(), font, FG);
    let size = vec2(galley.size().x + 42.0, 32.0);
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let (rect, response) = ui.allocate_exact_size(size, sense);
    let hot = enabled && response.hovered();
    let fill = if hot { SELECTION } else { Color32::TRANSPARENT };
    ui.painter().rect(rect, theme::BUTTON_RADIUS, fill, Stroke::new(1.0, LINE), StrokeKind::Inside);
    let color = if enabled { FG } else { FAINT };
    let icon_rect = Rect::from_center_size(pos2(rect.left() + 20.0, rect.center().y), Vec2::splat(16.0));
    icons::draw(ui.painter(), icon_rect, icon, color);
    ui.painter().galley(pos2(rect.left() + 32.0, rect.center().y - galley.size().y / 2.0), galley, color);
    response
}

pub fn segmented_tool(ui: &mut Ui, icon: Icon, text: &str, on: bool) -> Response {
    let font = FontId::proportional(13.5);
    let galley = ui.painter().layout_no_wrap(text.to_string(), font, FG);
    let (rect, response) = ui.allocate_exact_size(vec2(galley.size().x + 36.0, 30.0), Sense::click());
    let fill = if on {
        ACCENT_SOFT
    } else if response.hovered() {
        RAISED
    } else {
        Color32::TRANSPARENT
    };
    ui.painter().rect_filled(rect, 8.0, fill);
    let color = if on { ACCENT } else if response.hovered() { FG } else { DIM };
    let icon_rect = Rect::from_center_size(pos2(rect.left() + 17.0, rect.center().y), Vec2::splat(15.0));
    icons::draw(ui.painter(), icon_rect, icon, color);
    ui.painter().galley(pos2(rect.left() + 28.0, rect.center().y - galley.size().y / 2.0), galley, color);
    response
}

pub fn icon_button(ui: &mut Ui, icon: Icon, on: bool) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(30.0), Sense::click());
    let fill = if on {
        ACCENT_SOFT
    } else if response.hovered() {
        RAISED
    } else {
        Color32::TRANSPARENT
    };
    ui.painter().rect_filled(rect, 8.0, fill);
    let color = if on { ACCENT } else if response.hovered() { FG } else { DIM };
    icons::draw(ui.painter(), Rect::from_center_size(rect.center(), Vec2::splat(15.0)), icon, color);
    response
}

fn small_icon(ui: &mut Ui, icon: Icon, tip: &str, enabled: bool) -> Response {
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(26.0), sense);
    if enabled && response.hovered() {
        ui.painter().rect_filled(rect, 6.0, RAISED);
    }
    let color = if !enabled {
        theme::SELECTION
    } else if response.hovered() {
        FG
    } else {
        DIM
    };
    icons::draw(ui.painter(), Rect::from_center_size(rect.center(), Vec2::splat(15.0)), icon, color);
    response.on_hover_text(tip)
}

fn chip(ui: &mut Ui, text: &str, on: bool) -> Response {
    let font = FontId::proportional(12.5);
    let galley = ui.painter().layout_no_wrap(text.to_string(), font, FG);
    let (rect, response) = ui.allocate_exact_size(galley.size() + vec2(14.0, 8.0), Sense::click());
    let fill = if on {
        ACCENT_SOFT
    } else if response.hovered() {
        SELECTION
    } else {
        RAISED
    };
    ui.painter().rect_filled(rect, 7.0, fill);
    let color = if on { ACCENT } else if response.hovered() { FG } else { DIM };
    ui.painter().galley(rect.min + vec2(7.0, 4.0), galley, color);
    response
}

/// A length typed or dragged in the units on show; true when it changed.
fn length(ui: &mut Ui, unit: Unit, mm: &mut f64) -> bool {
    let mut v = unit.from_mm(*mm);
    let speed = match unit {
        Unit::Mm => 0.01,
        Unit::Mil => 0.5,
        Unit::Inch => 0.0005,
    };
    let r = ui.add(
        egui::DragValue::new(&mut v).speed(speed).max_decimals(unit.decimals()).suffix(format!(" {}", unit.name())),
    );
    if r.changed() {
        *mm = nm(unit.to_mm(v));
        true
    } else {
        false
    }
}

/// A little drawing of a footprint, for the library.
fn preview(ui: &mut Ui, def: &FootprintDef, size: Vec2) {
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    ui.painter().rect_filled(rect, 8.0, view::BOARD);
    let b = def.bbox();
    if b.is_empty() {
        return;
    }
    let s = ((rect.width() - 20.0) as f64 / b.w().max(0.5)).min((rect.height() - 20.0) as f64 / b.h().max(0.5));
    let tr = Tr { rect, c: b.center(), s, m: 1.0 };
    let painter = ui.painter_at(rect);
    let f = def.instantiate(0, String::new(), p(0.0, 0.0));
    for layer in [Layer::Court(Side::Top), Layer::Fab(Side::Top)] {
        for g in f.art(layer, &Rules::default()) {
            if let Ink::Line(a, e, _) = g {
                painter.line_segment([tr.pt(a), tr.pt(e)], Stroke::new(1.0, layer_color(layer).gamma_multiply(0.6)));
            }
        }
    }
    for (i, pad) in f.pads.iter().enumerate() {
        let c = if pad.through() { view::THROUGH } else { layer_color(Layer::Cu(TOP)) };
        if !pad.bare_hole() {
            fill_blob(&painter, &tr, &f.pad_blob(i, 0.0), c);
        }
        if pad.through() {
            painter.circle_filled(tr.pt(f.pad_center(i)), tr.len(pad.drill / 2.0), theme::WELL);
        }
    }
    for g in f.art(Layer::Silk(Side::Top), &Rules::default()) {
        match g {
            Ink::Line(a, e, _) => {
                painter.line_segment([tr.pt(a), tr.pt(e)], Stroke::new(1.2, layer_color(Layer::Silk(Side::Top))));
            }
            Ink::Dot(c, d) => {
                painter.circle_filled(tr.pt(c), tr.len(d / 2.0).max(1.5), layer_color(Layer::Silk(Side::Top)));
            }
            Ink::Ring(c, r, _) => {
                painter.circle_stroke(tr.pt(c), tr.len(r), Stroke::new(1.2, layer_color(Layer::Silk(Side::Top))));
            }
        }
    }
}

impl App {
    pub fn top_bar(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        ui.horizontal(|ui| {
            ui.set_height(30.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.add_space(2.0);
                let icon = if self.show_right { Icon::Right } else { Icon::Left };
                if small_icon(ui, icon, "Show or hide the details", true).clicked() {
                    self.show_right = !self.show_right;
                }
                let mut unit = self.unit;
                if segmented(ui, &mut unit, &[(Unit::Inch, "in"), (Unit::Mil, "mil"), (Unit::Mm, "mm")]) {
                    self.unit = unit;
                }
                ui.add_space(8.0);
                if let Some(_dir) = &self.exported
                    && App::copperview().is_some()
                    && tool_button(ui, Icon::Picture, "View in Copperview", true)
                        .on_hover_text("Open the Gerbers you just wrote in Copperview")
                        .clicked()
                {
                    self.view_in_copperview();
                }
                if tool_button(ui, Icon::Export, "Gerbers", true)
                    .on_hover_text("Write Gerber and drill files for the board maker (Ctrl+E)")
                    .clicked()
                {
                    self.ask_export(&ctx);
                }
                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                    let icon = if self.show_left { Icon::Left } else { Icon::Right };
                    if small_icon(ui, icon, "Show or hide the layers and library", true).clicked() {
                        self.show_left = !self.show_left;
                    }
                    let (rect, _) = ui.allocate_exact_size(Vec2::splat(24.0), Sense::hover());
                    if let Some(logo) = &self.logo {
                        let whole = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
                        ui.painter().image(logo.id(), rect, whole, Color32::WHITE);
                    }
                    ui.label(RichText::new("Coppersmith").font(semibold(15.0)));
                    ui.add_space(8.0);
                    ui.spacing_mut().item_spacing.x = 2.0;
                    if small_icon(ui, Icon::New, "New board (Ctrl+N)", true).clicked() {
                        self.new_board();
                    }
                    if small_icon(ui, Icon::Folder, "Open a board (Ctrl+O)", true).clicked() {
                        self.ask_open(&ctx);
                    }
                    if small_icon(ui, Icon::Save, "Save (Ctrl+S)", true).clicked() {
                        self.save(&ctx);
                    }
                    ui.add_space(6.0);
                    let (u, r) = (self.can_undo(), self.can_redo());
                    if small_icon(ui, Icon::Undo, "Undo (Ctrl+Z)", u).clicked() {
                        self.undo();
                    }
                    if small_icon(ui, Icon::Redo, "Redo (Ctrl+Shift+Z)", r).clicked() {
                        self.redo();
                    }
                    ui.add_space(10.0);
                    if ui.available_width() > 60.0 {
                        ui.add(egui::Label::new(RichText::new(self.title()).color(DIM)).truncate());
                    }
                });
            });
        });
    }

    pub fn status_bar(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.set_height(18.0);
            let u = self.unit;
            if let Some(q) = self.cursor {
                let text = format!("X {}  Y {} {}", u.num(q.x), u.num(q.y), u.name());
                ui.label(RichText::new(text).font(theme::mono(12.0)).color(DIM));
            }
            if let Some(text) = &self.hover_text {
                ui.label(RichText::new(text).font(theme::mono(12.0)).color(FAINT));
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if let Some(notice) = self.notice.clone() {
                    if small_icon(ui, Icon::Close, "Dismiss", true).clicked() {
                        self.notice = None;
                    }
                    ui.add(egui::Label::new(RichText::new(notice).size(12.5).color(theme::YELLOW)).truncate());
                } else {
                    let hint = match self.tool {
                        Tool::Select if !self.sel.is_empty() => {
                            "Drag to move · R turn · L other side · Delete removes · Ctrl+D copies · N lights up a net"
                        }
                        Tool::Select => "Click to pick · Drag to move or box · Right-drag to pan · Scroll to zoom",
                        Tool::Route if self.route.is_some() => {
                            "Click places all but the hollow segment · Space flips · Shift+Space corners · Shift+R walk/stop/ignore · V via · Ctrl+click finishes · Esc ends"
                        }
                        Tool::Route => "Click a pad or track to start · + − PgUp PgDn change layer · 3 width · Ctrl+W clearance",
                        Tool::Via => "Click to drop a via; it joins the net under it · Right-click stops",
                        Tool::Outline => "Click each corner · Click the first one or Enter to close · Backspace takes one back",
                        Tool::Place => "Click to place · R turn · L other side · Esc or right-click stops",
                    };
                    ui.add(egui::Label::new(RichText::new(hint).size(12.5).color(FAINT)).truncate());
                }
            });
        });
    }

    // -- Left: layers and library ----------------------------------------------

    pub fn left_panel(&mut self, ui: &mut Ui) {
        ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.label(RichText::new("Layers").size(14.0).color(DIM));
            ui.add_space(2.0);
            for l in self.doc.copper_layers() {
                self.layer_row(ui, Layer::Cu(l), true);
            }
            ui.add_space(4.0);
            for layer in [
                Layer::Silk(Side::Top),
                Layer::Silk(Side::Bottom),
                Layer::Court(Side::Top),
                Layer::Court(Side::Bottom),
                Layer::Fab(Side::Top),
                Layer::Fab(Side::Bottom),
                Layer::Edge,
            ] {
                self.layer_row(ui, layer, false);
            }
            ui.horizontal(|ui| {
                if chip(ui, "Still to connect", self.show_rats).clicked() {
                    self.show_rats = !self.show_rats;
                }
                if chip(ui, "Problems", self.show_problems).clicked() {
                    self.show_problems = !self.show_problems;
                }
            });
            ui.add_space(14.0);
            ui.separator();
            ui.add_space(6.0);
            self.library_list(ui);
        });
    }

    fn layer_row(&mut self, ui: &mut Ui, layer: Layer, copper: bool) {
        let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 28.0), Sense::click());
        let active = copper && layer == Layer::Cu(self.layer);
        let fill = if active {
            ACCENT_SOFT
        } else if response.hovered() {
            RAISED
        } else {
            Color32::TRANSPARENT
        };
        ui.painter().rect_filled(rect, 7.0, fill);
        let shown = !self.hidden.contains(&layer);
        let eye = Rect::from_center_size(pos2(rect.left() + 16.0, rect.center().y), Vec2::splat(16.0));
        let eye_hit = ui.interact(eye.expand(4.0), ui.scope_id().with(("eye", format!("{layer:?}"))), Sense::click());
        icons::draw(ui.painter(), eye, if shown { Icon::Eye } else { Icon::EyeOff }, if eye_hit.hovered() { FG } else { DIM });
        let swatch = Rect::from_center_size(pos2(rect.left() + 40.0, rect.center().y), Vec2::splat(12.0));
        let c = layer_color(layer);
        if shown {
            ui.painter().rect_filled(swatch, 3.0, c);
        } else {
            ui.painter().rect_stroke(swatch, 3.0, Stroke::new(1.2, c), StrokeKind::Inside);
        }
        let color = if active { ACCENT } else if shown { FG } else { FAINT };
        ui.painter().text(pos2(rect.left() + 54.0, rect.center().y), Align2::LEFT_CENTER, layer.name(), FontId::proportional(13.5), color);
        if active {
            ui.painter().text(rect.right_center() - vec2(8.0, 0.0), Align2::RIGHT_CENTER, "working", FontId::proportional(11.5), ACCENT);
        }
        if eye_hit.clicked() {
            if shown {
                self.hidden.insert(layer);
            } else {
                self.hidden.remove(&layer);
            }
        } else if response.clicked() {
            if copper && let Layer::Cu(l) = layer {
                self.switch_layer(l);
            } else if shown {
                self.hidden.insert(layer);
            } else {
                self.hidden.remove(&layer);
            }
        }
    }

    fn library_list(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("Footprints").size(14.0).color(DIM));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if small_icon(ui, Icon::Folder, "Open the folder for your own footprint files", true).clicked()
                    && let Some(dir) = crate::pcb::library::user_folder()
                {
                    let _ = std::fs::create_dir_all(&dir);
                    let _ = crate::platform::open_folder(&dir);
                }
                if small_icon(ui, Icon::Undo, "Read the footprint files again", true).clicked() {
                    self.lib = crate::pcb::library::Library::load();
                    self.notice = Some(format!("{} footprints in the library", self.lib.defs.len()));
                }
            });
        });
        ui.add_space(2.0);
        ui.add(egui::TextEdit::singleline(&mut self.lib_filter).hint_text("Search, like 0603 or SOIC").desired_width(f32::INFINITY));
        ui.add_space(6.0);
        let filter = self.lib_filter.to_lowercase();
        let words: Vec<&str> = filter.split_whitespace().collect();
        let mut start: Option<FootprintDef> = None;
        for cat in self.lib.categories() {
            let defs: Vec<&FootprintDef> = self
                .lib
                .defs
                .iter()
                .filter(|d| d.category == cat)
                .filter(|d| {
                    let hay = format!("{} {} {}", d.name, d.description, d.category).to_lowercase();
                    words.iter().all(|w| hay.contains(w))
                })
                .collect();
            if defs.is_empty() {
                continue;
            }
            let open = !words.is_empty();
            let header = egui::CollapsingHeader::new(RichText::new(&cat).size(13.0).color(DIM))
                .id_salt(("cat", &cat))
                .default_open(cat.starts_with("Resistors"))
                .open(if open { Some(true) } else { None });
            header.show(ui, |ui| {
                for d in defs {
                    let placing = self.placing.as_ref().is_some_and(|(f, _)| f.def == d.name);
                    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 38.0), Sense::click());
                    let fill = if placing {
                        ACCENT_SOFT
                    } else if response.hovered() {
                        RAISED
                    } else {
                        Color32::TRANSPARENT
                    };
                    ui.painter().rect_filled(rect, 7.0, fill);
                    let name_color = if placing { ACCENT } else { FG };
                    ui.painter().text(rect.left_top() + vec2(8.0, 5.0), Align2::LEFT_TOP, &d.name, FontId::proportional(13.5), name_color);
                    let galley = ui.painter().layout(d.description.clone(), FontId::proportional(11.5), FAINT, rect.width() - 16.0);
                    let clipped = ui.painter().with_clip_rect(rect.shrink2(vec2(8.0, 0.0)));
                    clipped.galley(rect.left_top() + vec2(8.0, 21.0), galley, FAINT);
                    let response = response.on_hover_ui(|ui| {
                        ui.label(RichText::new(&d.name).font(semibold(14.0)));
                        ui.label(RichText::new(&d.description).color(DIM));
                        preview(ui, d, vec2(220.0, 160.0));
                        ui.label(RichText::new(format!("{} pads · {}", d.pads.len(), d.source)).size(12.0).color(FAINT));
                    });
                    if response.clicked() {
                        start = Some(d.clone());
                    }
                }
            });
        }
        if let Some(d) = start {
            self.start_placing(&d);
        }
    }

    // -- Right: what's selected, the board, the checks, the nets --------------

    pub fn right_panel(&mut self, ui: &mut Ui) {
        ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            let before = self.doc.clone();
            match self.sel.as_slice() {
                [] => self.board_card(ui),
                [Sel::Fp(id)] => self.footprint_card(ui, *id),
                [Sel::Track(id)] => self.track_card(ui, *id),
                [Sel::Via(id)] => self.via_card(ui, *id),
                many => self.many_card(ui, many.len()),
            }
            self.checks_card(ui);
            self.nets_card(ui);
            // Typing a number makes one undo step, not one per key.
            if self.doc != before {
                let now = ui.input(|i| i.time);
                if now - self.last_field_edit > 1.0 {
                    self.remember(before);
                } else {
                    self.touch();
                }
                self.last_field_edit = now;
            }
        });
    }

    fn net_picker(doc: &Board, ui: &mut Ui, id: impl std::hash::Hash + std::fmt::Debug, net: &mut u32) {
        egui::ComboBox::from_id_salt(id)
            .selected_text(doc.net_name(*net))
            .width(130.0)
            .height(320.0)
            .show_ui(ui, |ui| {
                ui.selectable_value(net, 0, "No net");
                let mut nets = doc.nets.clone();
                nets.sort_by(|a, b| a.name.cmp(&b.name));
                for n in nets {
                    ui.selectable_value(net, n.id, n.name);
                }
            });
    }

    fn board_card(&mut self, ui: &mut Ui) {
        let u = self.unit;
        card(ui, |ui| {
            heading(ui, "Board");
            field(ui, "Name", |ui| ui.add(egui::TextEdit::singleline(&mut self.doc.name).desired_width(150.0)));
            let b = self.doc.outline_bbox();
            if !b.is_empty() {
                row(ui, "Size", &format!("{} × {} {}", u.num(b.w()), u.num(b.h()), u.name()));
            }
            field(ui, "Copper layers", |ui| {
                let mut n = self.doc.copper;
                if segmented(ui, &mut n, &[(6, "6"), (4, "4"), (2, "2")]) {
                    self.doc.copper = n;
                    if !self.doc.copper_layers().contains(&self.layer) {
                        self.layer = TOP;
                    }
                }
            });
            row(ui, "Parts", &self.doc.footprints.len().to_string());
            row(ui, "Tracks · vias", &format!("{} · {}", self.doc.tracks.len(), self.doc.vias.len()));
            ui.add_space(6.0);
            ui.collapsing(RichText::new("Make it a rectangle").color(DIM), |ui| {
                let id = ui.make_persistent_id("rect-size");
                let mut size: (f64, f64) = ui.data_mut(|d| *d.get_temp_mut_or(id, (b.w().max(10.0), b.h().max(10.0))));
                field(ui, "Width", |ui| length(ui, u, &mut size.0));
                field(ui, "Height", |ui| length(ui, u, &mut size.1));
                ui.data_mut(|d| d.insert_temp(id, size));
                if ui.button("Set the outline").clicked() {
                    let o = if b.is_empty() { p(0.0, 0.0) } else { b.min };
                    self.doc.outline = rect_outline(o, o.add(p(size.0, size.1)));
                    self.fit_pending = true;
                }
            });
        });
        card(ui, |ui| {
            heading(ui, "Rules");
            let r = &mut self.doc.rules;
            field(ui, "Clearance", |ui| length(ui, u, &mut r.clearance));
            field(ui, "Track width", |ui| length(ui, u, &mut r.track));
            field(ui, "Via pad", |ui| length(ui, u, &mut r.via_dia));
            field(ui, "Via hole", |ui| length(ui, u, &mut r.via_drill));
            field(ui, "Thinnest track", |ui| length(ui, u, &mut r.min_track));
            field(ui, "Smallest hole", |ui| length(ui, u, &mut r.min_drill));
            field(ui, "Thinnest ring", |ui| length(ui, u, &mut r.min_ring));
            field(ui, "From the edge", |ui| length(ui, u, &mut r.edge_clearance));
            field(ui, "Mask margin", |ui| length(ui, u, &mut r.mask_margin));
            field(ui, "Name height", |ui| length(ui, u, &mut r.text_height));
        });
    }

    fn footprint_card(&mut self, ui: &mut Ui, id: u32) {
        let u = self.unit;
        let doc_nets = self.doc.clone();
        let Some(f) = self.doc.fp_mut(id) else { return };
        let mut rotate = 0.0;
        let mut flip = false;
        card(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(&f.refdes).font(semibold(17.0)));
                ui.label(RichText::new(&f.def).color(FAINT));
            });
            ui.add_space(4.0);
            field(ui, "Name", |ui| ui.add(egui::TextEdit::singleline(&mut f.refdes).desired_width(140.0)));
            field(ui, "Value", |ui| ui.add(egui::TextEdit::singleline(&mut f.value).desired_width(140.0)));
            field(ui, "X", |ui| length(ui, u, &mut f.at.x));
            field(ui, "Y", |ui| length(ui, u, &mut f.at.y));
            field(ui, "Turn", |ui| {
                if icon_button(ui, Icon::Rotate, false).on_hover_text("Turn 90° (R)").clicked() {
                    rotate = 90.0;
                }
                ui.label(RichText::new(format!("{:.1}°", f.rot)).font(theme::mono(12.5)));
            });
            field(ui, "Side", |ui| {
                let mut side = f.side;
                if segmented(ui, &mut side, &[(Side::Bottom, "Bottom"), (Side::Top, "Top")]) {
                    flip = true;
                }
            });
            field(ui, "Locked", |ui| ui.checkbox(&mut f.locked, ""));
        });
        card(ui, |ui| {
            heading(ui, "Pads");
            for (i, pad) in f.pads.iter_mut().enumerate() {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(&pad.num).font(theme::mono(12.5)).color(DIM));
                    let kind = if pad.bare_hole() {
                        "hole".to_string()
                    } else if pad.through() {
                        format!("Ø{}", u.num(pad.drill))
                    } else {
                        format!("{}×{}", u.num(pad.size.x), u.num(pad.size.y))
                    };
                    ui.label(RichText::new(kind).size(11.5).color(FAINT));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if !pad.bare_hole() {
                            App::net_picker(&doc_nets, ui, ("pad-net", id, i), &mut pad.net);
                        }
                    });
                });
            }
        });
        if rotate != 0.0 {
            self.rotate_selection(rotate);
        }
        if flip {
            self.flip_selection();
        }
    }

    fn track_card(&mut self, ui: &mut Ui, id: u32) {
        let u = self.unit;
        let layers = self.doc.copper_layers();
        let doc_nets = self.doc.clone();
        let Some(t) = self.doc.tracks.iter_mut().find(|t| t.id == id) else { return };
        card(ui, |ui| {
            heading(ui, "Track");
            field(ui, "Width", |ui| length(ui, u, &mut t.w));
            field(ui, "Layer", |ui| {
                egui::ComboBox::from_id_salt(("track-layer", id)).selected_text(Layer::Cu(t.layer).name()).show_ui(ui, |ui| {
                    for l in layers {
                        ui.selectable_value(&mut t.layer, l, Layer::Cu(l).name());
                    }
                });
            });
            field(ui, "Net", |ui| App::net_picker(&doc_nets, ui, ("track-net", id), &mut t.net));
            row(ui, "Length", &u.len(t.a.dist(t.b)));
            row(ui, "From", &format!("{}, {}", u.num(t.a.x), u.num(t.a.y)));
            row(ui, "To", &format!("{}, {}", u.num(t.b.x), u.num(t.b.y)));
            ui.add_space(4.0);
            ui.label(RichText::new("Double-click a track to pick up everything joined to it.").size(12.0).color(FAINT));
        });
    }

    fn via_card(&mut self, ui: &mut Ui, id: u32) {
        let u = self.unit;
        let doc_nets = self.doc.clone();
        let Some(v) = self.doc.vias.iter_mut().find(|v| v.id == id) else { return };
        card(ui, |ui| {
            heading(ui, "Via");
            field(ui, "X", |ui| length(ui, u, &mut v.at.x));
            field(ui, "Y", |ui| length(ui, u, &mut v.at.y));
            field(ui, "Pad", |ui| length(ui, u, &mut v.dia));
            field(ui, "Hole", |ui| length(ui, u, &mut v.drill));
            field(ui, "Net", |ui| App::net_picker(&doc_nets, ui, ("via-net", id), &mut v.net));
        });
    }

    fn many_card(&mut self, ui: &mut Ui, n: usize) {
        let mut act = None;
        card(ui, |ui| {
            heading(ui, &format!("{n} things picked"));
            ui.horizontal_wrapped(|ui| {
                if tool_button(ui, Icon::Rotate, "Turn", true).clicked() {
                    act = Some('r');
                }
                if tool_button(ui, Icon::Flip, "Other side", true).clicked() {
                    act = Some('l');
                }
                if tool_button(ui, Icon::Copy, "Copy", true).clicked() {
                    act = Some('d');
                }
                if tool_button(ui, Icon::Trash, "Remove", true).clicked() {
                    act = Some('x');
                }
            });
        });
        match act {
            Some('r') => self.rotate_selection(90.0),
            Some('l') => self.flip_selection(),
            Some('d') => self.duplicate_selection(),
            Some('x') => self.delete_selection(),
            _ => {}
        }
    }

    fn checks_card(&mut self, ui: &mut Ui) {
        let errors = self.errors();
        let unrouted = self.copper.rats.len();
        let warnings = self.problems.len() - errors - unrouted;
        let mut go: Option<usize> = None;
        card(ui, |ui| {
            ui.horizontal(|ui| {
                heading(ui, "Checks");
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let (text, color) = if errors == 0 { ("No errors", theme::GREEN) } else { ("Needs fixing", theme::RED) };
                    ui.label(RichText::new(text).size(12.5).color(color));
                });
            });
            row(ui, "Errors", &errors.to_string());
            row(ui, "Warnings", &warnings.to_string());
            row(ui, "Still to connect", &unrouted.to_string());
            let listed: Vec<(usize, &crate::pcb::drc::Problem)> =
                self.problems.iter().enumerate().filter(|(_, p)| p.title != "Not connected").collect();
            if !listed.is_empty() {
                ui.add_space(6.0);
                ScrollArea::vertical().id_salt("problems").max_height(220.0).show(ui, |ui| {
                    for (i, prob) in listed {
                        let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 40.0), Sense::click());
                        let on = self.focus == Some(i);
                        let fill = if on {
                            ACCENT_SOFT
                        } else if response.hovered() {
                            RAISED
                        } else {
                            Color32::TRANSPARENT
                        };
                        ui.painter().rect_filled(rect, 7.0, fill);
                        let dot = if prob.severity == Severity::Error { theme::RED } else { theme::YELLOW };
                        ui.painter().circle_filled(pos2(rect.left() + 10.0, rect.top() + 12.0), 3.5, dot);
                        ui.painter().text(rect.left_top() + vec2(20.0, 4.0), Align2::LEFT_TOP, &prob.title, FontId::proportional(13.0), FG);
                        let galley = ui.painter().layout_no_wrap(prob.detail.clone(), FontId::proportional(11.5), FAINT);
                        ui.painter().with_clip_rect(rect.shrink(2.0)).galley(rect.left_top() + vec2(20.0, 21.0), galley, FAINT);
                        if response.on_hover_text(&prob.detail).clicked() {
                            go = Some(i);
                        }
                    }
                });
            }
        });
        if let Some(i) = go {
            self.focus = Some(i);
            let at = self.problems[i].at;
            self.zoom_to(at, 8.0);
        }
    }

    fn nets_card(&mut self, ui: &mut Ui) {
        let mut pads: std::collections::HashMap<u32, usize> = std::collections::HashMap::new();
        for f in &self.doc.footprints {
            for pad in &f.pads {
                *pads.entry(pad.net).or_default() += 1;
            }
        }
        let mut open: std::collections::HashMap<u32, usize> = std::collections::HashMap::new();
        for r in &self.copper.rats {
            *open.entry(r.net).or_default() += 1;
        }
        let mut add: Option<String> = None;
        card(ui, |ui| {
            heading(ui, "Nets");
            let mut nets = self.doc.nets.clone();
            nets.sort_by(|a, b| a.name.cmp(&b.name));
            for n in nets {
                let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 26.0), Sense::click());
                let on = self.highlight == Some(n.id);
                let fill = if on {
                    ACCENT_SOFT
                } else if response.hovered() {
                    RAISED
                } else {
                    Color32::TRANSPARENT
                };
                ui.painter().rect_filled(rect, 7.0, fill);
                let left = open.get(&n.id).copied().unwrap_or(0);
                let dot = if left == 0 { theme::GREEN } else { theme::YELLOW };
                ui.painter().circle_filled(pos2(rect.left() + 10.0, rect.center().y), 3.5, dot);
                let name_color = if on { ACCENT } else { FG };
                ui.painter().text(pos2(rect.left() + 22.0, rect.center().y), Align2::LEFT_CENTER, &n.name, FontId::proportional(13.5), name_color);
                let count = pads.get(&n.id).copied().unwrap_or(0);
                let info = if left > 0 { format!("{count} pads · {left} open") } else { format!("{count} pads") };
                ui.painter().text(rect.right_center() - vec2(8.0, 0.0), Align2::RIGHT_CENTER, info, theme::mono(11.5), FAINT);
                if response.on_hover_text("Click to light up this net").clicked() {
                    self.highlight = if on { None } else { Some(n.id) };
                }
            }
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                let edit = ui.add(egui::TextEdit::singleline(&mut self.new_net).hint_text("New net name").desired_width(170.0));
                let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if (small_icon(ui, Icon::Plus, "Add the net", true).clicked() || enter) && !self.new_net.trim().is_empty() {
                    add = Some(self.new_net.trim().to_string());
                }
            });
        });
        if let Some(name) = add {
            self.edit(|b| {
                b.net(&name);
            });
            self.new_net.clear();
        }
    }
}
