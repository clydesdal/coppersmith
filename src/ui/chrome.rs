//! What the window is made of round the board: the menus, the optional ribbon
//! of controls under them, the panels either side with the tabs along their
//! bottoms, and the controls for what can be picked.

use crate::pcb::model::*;
use crate::ui::app::{App, GRIDS, Tool, Unit, WIDTHS};
use crate::ui::dock::{Filter, LeftTab, Pick, RightTab};
use crate::ui::icons::Icon;
use crate::ui::panels::{chip, icon_button, segmented, small_icon, tool_button};
use crate::ui::theme::{self, ACCENT, ACCENT_SOFT, DIM, FAINT, FG, LINE, RAISED, semibold};
use eframe::egui::{self, Align, Align2, Color32, CornerRadius, FontId, Frame, Layout, Margin, Rect, RichText, ScrollArea, Sense, Stroke, StrokeKind, Ui, Vec2, pos2, vec2};

// -- Menu pieces -----------------------------------------------------------

/// A menu entry with its shortcut beside it; true when it's chosen, which
/// also closes the menu.
fn item(ui: &mut Ui, text: &str, shortcut: &str, enabled: bool) -> bool {
    let mut button = egui::Button::new(text);
    if !shortcut.is_empty() {
        button = button.shortcut_text(RichText::new(shortcut).color(FAINT));
    }
    let chosen = ui.add_enabled(enabled, button).clicked();
    if chosen {
        ui.close();
    }
    chosen
}

/// A menu entry that is on or off. `close`: whether choosing it closes the
/// menu, or leaves it open to switch more.
fn check(ui: &mut Ui, text: &str, shortcut: &str, on: bool, close: bool) -> bool {
    let mut button = egui::Button::selectable(on, text);
    if !shortcut.is_empty() {
        button = button.shortcut_text(RichText::new(shortcut).color(FAINT));
    }
    let chosen = ui.add(button).clicked();
    if chosen && close {
        ui.close();
    }
    chosen
}

// -- Tabs ------------------------------------------------------------------

/// What a tab shows beside its name: a count or a mark, in a colour.
type Badge = Option<(String, Color32)>;

/// A row of tabs along the bottom of a panel; true when another was chosen.
fn tab_strip<T: Copy + PartialEq>(ui: &mut Ui, current: &mut T, tabs: &[(T, &str, Badge)]) -> bool {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 36.0), Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, theme::WELL);
    painter.line_segment([rect.left_top(), rect.right_top()], Stroke::new(1.0, LINE));
    let each = rect.width() / tabs.len().max(1) as f32;
    let font = FontId::proportional(13.5);
    let mut changed = false;
    for (i, (tab, name, badge)) in tabs.iter().enumerate() {
        let r = Rect::from_min_size(pos2(rect.left() + i as f32 * each, rect.top()), vec2(each, rect.height()));
        let response = ui.interact(r, ui.scope_id().with(("tab", i, *name)), Sense::click());
        let on = *current == *tab;
        if on {
            painter.rect_filled(r.shrink2(vec2(0.0, 0.0)), 0.0, theme::PANEL);
            painter.line_segment([r.left_top() + vec2(0.0, 1.0), r.right_top() + vec2(0.0, 1.0)], Stroke::new(2.5, ACCENT));
        } else if response.hovered() {
            painter.rect_filled(r, 0.0, RAISED);
        }
        let color = if on { FG } else if response.hovered() { FG } else { DIM };
        let galley = painter.layout_no_wrap(name.to_string(), font.clone(), color);
        let badge_text = badge.as_ref().map(|(t, c)| (painter.layout_no_wrap(t.clone(), FontId::proportional(11.0), Color32::WHITE), *c));
        let badge_w = badge_text.as_ref().map(|(g, _)| g.size().x + 10.0 + 5.0).unwrap_or(0.0);
        let left = r.center().x - (galley.size().x + badge_w) / 2.0;
        painter.galley(pos2(left, r.center().y - galley.size().y / 2.0), galley.clone(), color);
        if let Some((g, c)) = badge_text {
            let pill = Rect::from_min_size(pos2(left + galley.size().x + 5.0, r.center().y - 8.0), vec2(g.size().x + 10.0, 16.0));
            painter.rect_filled(pill, 8.0, c);
            painter.galley(pill.center() - g.size() / 2.0, g, Color32::WHITE);
        }
        if response.clicked() && !on {
            *current = *tab;
            changed = true;
        }
    }
    changed
}

// -- The select filter -----------------------------------------------------

/// A small on/off switch, drawn.
fn switch(ui: &mut Ui, rect: Rect, on: bool, hovered: bool) {
    let track = if on { ACCENT } else if hovered { theme::SELECTION } else { theme::WELL };
    ui.painter().rect(rect, CornerRadius::same((rect.height() / 2.0) as u8), track, Stroke::new(1.0, LINE), StrokeKind::Inside);
    let r = rect.height() / 2.0 - 3.0;
    let x = if on { rect.right() - r - 3.0 } else { rect.left() + r + 3.0 };
    ui.painter().circle_filled(pos2(x, rect.center().y), r, if on { Color32::WHITE } else { DIM });
}

impl App {
    fn pick_count(&self, kind: Pick) -> usize {
        match kind {
            Pick::Parts | Pick::Names => self.doc.footprints.len(),
            Pick::Tracks => self.doc.tracks.len(),
            Pick::Vias => self.doc.vias.len(),
            Pick::Pours => self.doc.pours.len(),
            Pick::Fills => self.doc.fills.len(),
            Pick::Keepouts => self.doc.keepouts.len(),
        }
    }

    /// The select filter: presets, and a switch for each kind of thing with
    /// a button to take just that kind. Used in the Select tab and in the
    /// chip's pop-up.
    fn filter_controls(&mut self, ui: &mut Ui) {
        // Presets, lit when they're what's set.
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
            for (name, preset, tip) in [
                ("All", Filter::default(), "Everything can be picked"),
                ("Copper", Filter::copper(), "Tracks, vias, pours and fills"),
                ("Parts", Filter::parts(), "Parts and their names"),
                ("None", Filter::none(), "Nothing can be picked, so clicks and boxes only pan and look"),
            ] {
                if chip(ui, name, self.filter == preset).on_hover_text(tip).clicked() {
                    self.filter = preset;
                }
            }
        });
        ui.add_space(6.0);
        for kind in Pick::ALL {
            let on = self.filter.on(kind);
            let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 34.0), Sense::click());
            // The button that takes just this kind, over the row so it gets its own clicks.
            let only = Rect::from_center_size(pos2(rect.right() - 30.0, rect.center().y), vec2(50.0, 24.0));
            let only_response = ui.interact(only, ui.scope_id().with(("only", kind.name())), Sense::click());
            if response.hovered() || only_response.hovered() {
                ui.painter().rect_filled(rect, 8.0, RAISED);
            }
            switch(ui, Rect::from_center_size(pos2(rect.left() + 26.0, rect.center().y), vec2(34.0, 18.0)), on, response.hovered());
            let color = if on { FG } else { FAINT };
            ui.painter().text(pos2(rect.left() + 52.0, rect.center().y), Align2::LEFT_CENTER, kind.name(), FontId::proportional(14.0), color);
            let shown = self.pick_count(kind).to_string();
            ui.painter().text(pos2(only.left() - 8.0, rect.center().y), Align2::RIGHT_CENTER, shown, theme::mono(12.0), FAINT);
            let only_on = self.filter == Filter::only(kind);
            let hot = only_response.hovered();
            ui.painter().rect_filled(only, 6.0, if only_on { ACCENT_SOFT } else if hot { theme::SELECTION } else { Color32::TRANSPARENT });
            ui.painter().text(only.center(), Align2::CENTER_CENTER, "only", FontId::proportional(12.5), if only_on { ACCENT } else if hot { FG } else { DIM });
            if only_response.on_hover_text(format!("Pick {} and nothing else", kind.name().to_lowercase())).clicked() {
                self.filter = Filter::only(kind);
            } else if response.double_clicked() {
                self.filter = Filter::only(kind);
            } else if response.clicked() {
                self.filter.set(kind, !on);
            }
        }
    }

    /// The Select tab.
    pub fn select_page(&mut self, ui: &mut Ui) {
        ui.label(RichText::new("What can be picked").font(semibold(15.0)));
        ui.label(RichText::new("Clicks, boxes and Select all only take the kinds switched on. The rest are left alone, so a click goes through them.").size(12.0).color(FAINT));
        ui.add_space(8.0);
        self.filter_controls(ui);
        ui.add_space(8.0);
        if self.filter.is_none() {
            ui.label(RichText::new("Nothing can be picked: switch some kinds on, or press All.").size(12.5).color(theme::YELLOW));
        }
        ui.label(RichText::new("Names can only be picked while a silkscreen layer is the working layer.").size(12.0).color(FAINT));
    }

    /// The chip that shows what can be picked, and changes it. Lit when
    /// something is held back, so it's plain why a click did nothing.
    pub fn filter_chip(&mut self, ui: &mut Ui, below: bool) {
        let held = !self.filter.is_all();
        let text = format!("Select: {}", self.filter.summary());
        let font = FontId::proportional(12.5);
        let galley = ui.painter().layout_no_wrap(text, font, FG);
        let (rect, response) = ui.allocate_exact_size(galley.size() + vec2(30.0, 8.0), Sense::click());
        let fill = if held { theme::YELLOW.gamma_multiply(0.22) } else if response.hovered() { theme::SELECTION } else { RAISED };
        ui.painter().rect_filled(rect, 7.0, fill);
        let color = if held { theme::YELLOW } else if response.hovered() { FG } else { DIM };
        // A funnel's worth of mark: a dot when something is held back.
        ui.painter().circle_filled(pos2(rect.left() + 11.0, rect.center().y), 3.0, if held { theme::YELLOW } else { FAINT });
        ui.painter().galley(pos2(rect.left() + 20.0, rect.center().y - galley.size().y / 2.0), galley, color);
        let response = response.on_hover_text("What can be picked: click to choose (Edit ▸ Selection filter, or the Select tab)");
        let align = if below { egui::RectAlign::BOTTOM_END } else { egui::RectAlign::TOP_END };
        egui::Popup::from_toggle_button_response(&response)
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
            .align(align)
            .show(|ui| {
                ui.set_min_width(290.0);
                ui.label(RichText::new("What can be picked").font(semibold(14.0)));
                ui.add_space(4.0);
                self.filter_controls(ui);
            });
    }

    /// The filter as a menu.
    fn filter_menu(&mut self, ui: &mut Ui) {
        for kind in Pick::ALL {
            let on = self.filter.on(kind);
            if check(ui, kind.name(), "", on, false) {
                self.filter.set(kind, !on);
            }
        }
        ui.separator();
        if item(ui, "Pick everything", "", true) {
            self.filter = Filter::default();
        }
        if item(ui, "Copper only", "", true) {
            self.filter = Filter::copper();
        }
        if item(ui, "Parts only", "", true) {
            self.filter = Filter::parts();
        }
        if item(ui, "Pick nothing", "", true) {
            self.filter = Filter::none();
        }
    }

    // -- The menus ---------------------------------------------------------

    pub fn menu_bar(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        egui::MenuBar::new().ui(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 3.0;
            let (rect, _) = ui.allocate_exact_size(Vec2::splat(22.0), Sense::hover());
            if let Some(logo) = &self.logo {
                let whole = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
                ui.painter().image(logo.id(), rect, whole, Color32::WHITE);
            }
            ui.add_space(4.0);
            ui.label(RichText::new("Coppersmith").font(semibold(14.5)));
            ui.add_space(10.0);
            ui.spacing_mut().button_padding = vec2(9.0, 4.0);
            ui.menu_button("File", |ui| self.file_menu(ui, &ctx));
            ui.menu_button("Edit", |ui| self.edit_menu(ui));
            ui.menu_button("View", |ui| self.view_menu(ui));
            ui.menu_button("Place", |ui| self.place_menu(ui));
            ui.menu_button("Route", |ui| self.route_menu(ui));
            ui.menu_button("Tools", |ui| self.tools_menu(ui));
            ui.menu_button("Help", |ui| self.help_menu(ui));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.add_space(2.0);
                let icon = if self.show_right { Icon::Right } else { Icon::Left };
                if small_icon(ui, icon, "Show or hide the right panel (Ctrl+])", true).clicked() {
                    self.show_right = !self.show_right;
                }
                let icon = if self.show_left { Icon::Left } else { Icon::Right };
                if small_icon(ui, icon, "Show or hide the left panel (Ctrl+[)", true).clicked() {
                    self.show_left = !self.show_left;
                }
                let mut unit = self.unit;
                if segmented(ui, &mut unit, &[(Unit::Inch, "in"), (Unit::Mil, "mil"), (Unit::Mm, "mm")]) {
                    self.unit = unit;
                }
                ui.add_space(8.0);
                if ui.available_width() > 60.0 {
                    ui.add(egui::Label::new(RichText::new(self.title()).color(DIM)).truncate());
                }
            });
        });
    }

    fn file_menu(&mut self, ui: &mut Ui, ctx: &egui::Context) {
        if item(ui, "New board", "Ctrl+N", true) {
            self.new_board();
        }
        if item(ui, "Open…", "Ctrl+O", true) {
            self.ask_open(ctx);
        }
        if item(ui, "Add a footprint library…", "", true) {
            self.ask_add_library(ctx);
        }
        let editing = self.libwin.is_some() || self.schwin.is_some();
        if item(ui, if editing { "Back to the board" } else { "Library editor…" }, "", true) {
            if editing {
                self.libwin = None;
                self.schwin = None;
            } else {
                self.open_library_editor(None);
            }
        }
        if item(ui, "Schematic editor…", "", self.schwin.is_none()) {
            self.open_schematic_editor(None);
        }
        ui.separator();
        if item(ui, "Save", "Ctrl+S", true) {
            self.save(ctx);
        }
        if item(ui, "Save as…", "Ctrl+Shift+S", true) {
            self.ask_save_as(ctx);
        }
        ui.separator();
        if item(ui, "Write Gerbers…", "Ctrl+E", true) {
            self.ask_export(ctx);
        }
        let viewable = self.exported.is_some() && App::copperview().is_some();
        if item(ui, "View them in Copperview", "", viewable) {
            self.view_in_copperview();
        }
        ui.separator();
        if item(ui, "Quit", "", true) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    fn edit_menu(&mut self, ui: &mut Ui) {
        let (undo, redo, picked) = (self.can_undo(), self.can_redo(), !self.sel.is_empty());
        if item(ui, "Undo", "Ctrl+Z", undo) {
            self.undo();
        }
        if item(ui, "Redo", "Ctrl+Shift+Z", redo) {
            self.redo();
        }
        ui.separator();
        if item(ui, "Select all", "Ctrl+A", true) {
            self.select_all();
        }
        if item(ui, "Copy", "Ctrl+D", picked) {
            self.duplicate_selection();
        }
        if item(ui, "Delete", "Delete", picked) {
            self.delete_selection();
        }
        ui.separator();
        if item(ui, "Turn 90°", "R", picked) {
            self.rotate_selection(90.0);
        }
        if item(ui, "Put on the other side", "L", picked) {
            self.flip_selection();
        }
        ui.separator();
        ui.menu_button("Selection filter", |ui| self.filter_menu(ui));
    }

    fn view_menu(&mut self, ui: &mut Ui) {
        if item(ui, "Fit the board to the window", "F", true) {
            self.fit_pending = true;
        }
        if check(ui, "See it from underneath", "X", self.mirror, true) {
            self.mirror = !self.mirror;
        }
        ui.separator();
        ui.menu_button("Grid", |ui| {
            for g in GRIDS {
                if check(ui, &self.unit.len(g), "", (g - self.grid).abs() < 1e-9, true) {
                    self.grid = g;
                }
            }
            ui.separator();
            if check(ui, "Show the grid", "G", self.show_grid, true) {
                self.show_grid = !self.show_grid;
            }
        });
        ui.menu_button("Units", |ui| {
            for (unit, name) in [(Unit::Mm, "Millimetres"), (Unit::Mil, "Mils"), (Unit::Inch, "Inches")] {
                if check(ui, name, "", self.unit == unit, true) {
                    self.unit = unit;
                }
            }
        });
        ui.separator();
        ui.menu_button("Working layer", |ui| {
            for l in self.doc.copper_layers() {
                if check(ui, &Layer::Cu(l).name(), "", self.silk.is_none() && l == self.layer, true) {
                    self.switch_layer(l);
                }
            }
            ui.separator();
            for side in [Side::Top, Side::Bottom] {
                if check(ui, &Layer::Silk(side).name(), "", self.silk == Some(side), true) {
                    self.work_on_silk(side);
                }
            }
            ui.separator();
            if item(ui, "Next layer", "+", true) {
                self.next_layer(1);
            }
            if item(ui, "Previous layer", "−", true) {
                self.next_layer(-1);
            }
        });
        ui.menu_button("Show layers", |ui| {
            let layers: Vec<Layer> = self
                .doc
                .copper_layers()
                .into_iter()
                .map(Layer::Cu)
                .chain([Layer::Silk(Side::Top), Layer::Silk(Side::Bottom), Layer::Court(Side::Top), Layer::Court(Side::Bottom), Layer::Fab(Side::Top), Layer::Fab(Side::Bottom), Layer::Edge])
                .collect();
            for layer in layers {
                let shown = !self.hidden.contains(&layer);
                if check(ui, &layer.name(), "", shown, false) {
                    if shown {
                        self.hidden.insert(layer);
                    } else {
                        self.hidden.remove(&layer);
                    }
                }
            }
        });
        if check(ui, "Still to connect", "", self.show_rats, false) {
            self.show_rats = !self.show_rats;
        }
        if check(ui, "Problems", "", self.show_problems, false) {
            self.show_problems = !self.show_problems;
        }
        if !self.doc.pours.is_empty() && check(ui, "Pour fills", "", self.flood, false) {
            self.set_flood(!self.flood);
        }
        ui.separator();
        if check(ui, "Left panel", "Ctrl+[", self.show_left, false) {
            self.show_left = !self.show_left;
        }
        if check(ui, "Right panel", "Ctrl+]", self.show_right, false) {
            self.show_right = !self.show_right;
        }
        ui.menu_button("Panels", |ui| {
            for tab in LeftTab::ALL {
                if check(ui, &format!("Left: {}", tab.name()), "", self.show_left && self.left_tab == tab, true) {
                    self.left_tab = tab;
                    self.show_left = true;
                }
            }
            ui.separator();
            for tab in RightTab::ALL {
                if check(ui, &format!("Right: {}", tab.name()), "", self.show_right && self.right_tab == tab, true) {
                    self.right_tab = tab;
                    self.show_right = true;
                }
            }
        });
        if check(ui, "Ribbon of controls", "Ctrl+Shift+R", self.ribbon, true) {
            self.ribbon = !self.ribbon;
        }
    }

    fn place_menu(&mut self, ui: &mut Ui) {
        for (tool, name, key) in [
            (Tool::Select, "Select and move", "S"),
            (Tool::Route, "Track", "W"),
            (Tool::Via, "Via", "V"),
            (Tool::Outline, "Board outline", "O"),
            (Tool::Pour, "Copper pour", "P"),
            (Tool::Keepout, "Keepout", "K"),
        ] {
            if check(ui, name, key, self.tool == tool, true) {
                self.set_tool(tool);
            }
        }
        ui.separator();
        if item(ui, "Part from the library…", "", true) {
            self.left_tab = LeftTab::Parts;
            self.show_left = true;
            self.notice = Some("Choose a footprint in the Parts list to place it".into());
        }
    }

    fn route_menu(&mut self, ui: &mut Ui) {
        if check(ui, "Route a track", "W", self.tool == Tool::Route, true) {
            self.set_tool(Tool::Route);
        }
        ui.separator();
        ui.menu_button("Track width", |ui| {
            for w in WIDTHS {
                if check(ui, &self.unit.len(w), "", (w - self.width).abs() < 1e-9, true) {
                    self.width = w;
                    if let Some(r) = &mut self.route {
                        r.width = w;
                    }
                }
            }
        });
        ui.menu_button("Corners and getting past things", |ui| self.routing_menu(ui));
    }

    fn tools_menu(&mut self, ui: &mut Ui) {
        if !self.doc.pours.is_empty() && check(ui, "Pour fills", "", self.flood, true) {
            self.set_flood(!self.flood);
        }
        let odd = self.doc.footprints.iter().any(|f| f.label_height.is_some());
        if item(ui, "Make every name the board's height", "", odd) {
            self.edit(|b| {
                for f in &mut b.footprints {
                    f.label_height = None;
                }
            });
        }
        ui.separator();
        let errors = self.errors();
        let text = if errors == 0 { "The checks find no errors".to_string() } else { format!("The checks find {errors} errors") };
        if item(ui, &text, "", true) {
            self.right_tab = RightTab::Checks;
            self.show_right = true;
        }
    }

    fn help_menu(&mut self, ui: &mut Ui) {
        if item(ui, "Keyboard shortcuts", "F1", true) {
            self.show_keys = true;
        }
        if item(ui, "About Coppersmith", "", true) {
            self.notice = Some(format!("Coppersmith {} — lay out boards, check them, write Gerbers", env!("CARGO_PKG_VERSION")));
        }
    }

    /// The window of keyboard shortcuts.
    pub fn shortcuts_window(&mut self, ctx: &egui::Context) {
        let mut open = self.show_keys;
        egui::Window::new("Keyboard shortcuts").open(&mut open).collapsible(false).resizable(true).default_width(420.0).show(ctx, |ui| {
            ScrollArea::vertical().max_height(520.0).show(ui, |ui| {
                ui.label(RichText::new("Ctrl is ⌘ on a Mac.").size(12.0).color(FAINT));
                for (section, keys) in SHORTCUTS {
                    ui.add_space(8.0);
                    ui.label(RichText::new(*section).font(semibold(14.0)));
                    for (key, what) in *keys {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(*key).font(theme::mono(12.5)).color(ACCENT));
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| ui.label(RichText::new(*what).color(DIM)));
                        });
                    }
                }
            });
        });
        self.show_keys = open;
    }

    // -- The ribbon ----------------------------------------------------------

    /// A group of controls with its name under it.
    fn group(ui: &mut Ui, name: &str, add: impl FnOnce(&mut Ui)) {
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing = vec2(4.0, 2.0);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 3.0;
                add(ui);
            });
            ui.label(RichText::new(name).size(11.0).color(FAINT));
        });
    }

    pub fn ribbon_bar(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        // In a window too narrow for all of it, it scrolls sideways rather than being cut off.
        ScrollArea::horizontal().auto_shrink([false, true]).scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded).show(ui, |ui| self.ribbon_row(ui, &ctx));
    }

    fn ribbon_row(&mut self, ui: &mut Ui, ctx: &egui::Context) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            App::group(ui, "File", |ui| {
                if icon_button(ui, Icon::New, false).on_hover_text("New board (Ctrl+N)").clicked() {
                    self.new_board();
                }
                if icon_button(ui, Icon::Folder, false).on_hover_text("Open a board (Ctrl+O)").clicked() {
                    self.ask_open(ctx);
                }
                if icon_button(ui, Icon::Save, false).on_hover_text("Save (Ctrl+S)").clicked() {
                    self.save(ctx);
                }
                if tool_button(ui, Icon::Export, "Gerbers", true).on_hover_text("Write Gerber and drill files (Ctrl+E)").clicked() {
                    self.ask_export(ctx);
                }
            });
            ui.separator();
            App::group(ui, "Edit", |ui| {
                let (undo, redo, picked) = (self.can_undo(), self.can_redo(), !self.sel.is_empty());
                if ui.add_enabled(undo, egui::Button::new("Undo")).on_hover_text("Ctrl+Z").clicked() {
                    self.undo();
                }
                if ui.add_enabled(redo, egui::Button::new("Redo")).on_hover_text("Ctrl+Shift+Z").clicked() {
                    self.redo();
                }
                if ui.add_enabled(picked, egui::Button::new("Copy")).on_hover_text("Ctrl+D").clicked() {
                    self.duplicate_selection();
                }
                if ui.add_enabled(picked, egui::Button::new("Delete")).on_hover_text("Delete").clicked() {
                    self.delete_selection();
                }
            });
            ui.separator();
            App::group(ui, "Pick", |ui| self.filter_chip(ui, true));
            ui.separator();
            App::group(ui, "Tools", |ui| self.tool_controls(ui, true));
        });
    }

    // -- The panels ----------------------------------------------------------

    pub fn left_dock(&mut self, ui: &mut Ui) {
        egui::Panel::bottom("left-tabs").resizable(false).show_separator_line(false).frame(Frame::new().fill(theme::WELL)).show(ui, |ui| {
            let tabs: Vec<(LeftTab, &str, Badge)> = LeftTab::ALL
                .into_iter()
                .map(|t| (t, t.name(), if t == LeftTab::Select && !self.filter.is_all() { Some((self.filter.kinds().len().to_string(), theme::YELLOW.gamma_multiply(0.85))) } else { None }))
                .collect();
            let mut tab = self.left_tab;
            tab_strip(ui, &mut tab, &tabs);
            self.left_tab = tab;
        });
        egui::CentralPanel::default().frame(Frame::new().fill(theme::PANEL).inner_margin(Margin::symmetric(10, 10))).show(ui, |ui| {
            ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| match self.left_tab {
                LeftTab::Layers => self.layers_page(ui),
                LeftTab::Parts => self.library_list(ui),
                LeftTab::Select => self.select_page(ui),
            });
        });
    }

    pub fn right_dock(&mut self, ui: &mut Ui) {
        egui::Panel::bottom("right-tabs").resizable(false).show_separator_line(false).frame(Frame::new().fill(theme::WELL)).show(ui, |ui| {
            let errors = self.errors();
            let tabs: Vec<(RightTab, &str, Badge)> = RightTab::ALL
                .into_iter()
                .map(|t| (t, t.name(), if t == RightTab::Checks && errors > 0 { Some((errors.to_string(), theme::RED)) } else { None }))
                .collect();
            let mut tab = self.right_tab;
            tab_strip(ui, &mut tab, &tabs);
            self.right_tab = tab;
        });
        egui::CentralPanel::default().frame(Frame::new().fill(theme::PANEL).inner_margin(Margin::symmetric(12, 12))).show(ui, |ui| {
            self.right_page(ui);
        });
    }

    /// Brings the Properties page forward, as picking something on the board does.
    pub fn show_properties(&mut self) {
        self.right_tab = RightTab::Properties;
    }
}

/// The keyboard shortcuts, by what they do.
const SHORTCUTS: &[(&str, &[(&str, &str)])] = &[
    ("File", &[("Ctrl+N", "New board"), ("Ctrl+O", "Open"), ("Ctrl+S", "Save"), ("Ctrl+Shift+S", "Save as"), ("Ctrl+E", "Write Gerbers")]),
    ("Edit", &[("Ctrl+Z", "Undo"), ("Ctrl+Shift+Z / Ctrl+Y", "Redo"), ("Ctrl+A", "Select all"), ("Ctrl+D", "Copy"), ("Delete", "Delete"), ("R", "Turn (Shift: the other way)"), ("L", "Put on the other side"), ("Esc", "Let go, or clear what's picked")]),
    ("Tools", &[("S", "Select and move"), ("W", "Track"), ("V", "Via"), ("O", "Board outline"), ("P", "Copper pour"), ("K", "Keepout"), ("Enter", "Close an outline, pour or keepout")]),
    ("Routing", &[("Space", "Flip the corner"), ("Shift+Space", "Corner style"), ("Shift+R", "When something's in the way"), ("V", "Via to the other side"), ("2", "Via here"), ("3", "Where the width comes from"), ("Backspace", "Take back a click"), ("Ctrl+click", "Finish the connection"), ("Esc", "End")]),
    ("Layers", &[("+  −  PgUp  PgDn", "Next layer: copper, then the two silkscreens"), ("1–6", "A copper layer (when not routing)")]),
    ("View", &[("F", "Fit the board"), ("X", "See it from underneath"), ("G", "Show or hide the grid"), ("Ctrl+[", "Left panel"), ("Ctrl+]", "Right panel"), ("Ctrl+Shift+R", "Ribbon of controls"), ("F1", "This list")]),
];
