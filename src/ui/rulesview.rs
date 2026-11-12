//! The design rules window, and the card in the Checks panel that shows the rule a
//! problem breaks with the controls for it.

use crate::pcb::model::*;
use crate::pcb::rulebook::{CATALOG, Group, RuleId, RuleInfo, SeenRule, Support};
use crate::ui::app::{App, Unit};
use crate::ui::panels::{card, field, heading, length};
use crate::ui::theme::{self, ACCENT, DIM, FAINT, FG, GREEN, RED, YELLOW, semibold};
use eframe::egui::{self, Align, RichText, ScrollArea, Ui};

/// A length that may be left unset: shown as 0 when it is, and set to 0 to unset it.
fn optional_length(ui: &mut Ui, unit: Unit, value: &mut Option<f64>) {
    let mut v = value.unwrap_or(0.0);
    if length(ui, unit, &mut v) {
        *value = (v > 0.0).then_some(v);
    }
}

fn support_color(s: Support) -> egui::Color32 {
    match s {
        Support::Checked => GREEN,
        Support::Applied => ACCENT,
        Support::Listed => FAINT,
    }
}

/// The settings of rule `id`, and whether it's on, for a board: the controls both the rules
/// window and the card for a problem show.
/// With a net, only the rules asked of its classes are shown.
pub fn rule_controls(ui: &mut Ui, board: &mut Board, id: RuleId, u: Unit, net: Option<u32>) {
    let info = id.info();
    ui.label(RichText::new(info.what).size(12.0).color(DIM));
    ui.add_space(4.0);
    match info.support {
        Support::Checked => {
            let mut on = board.rules.is_on(id);
            if ui.checkbox(&mut on, "Check this rule").changed() {
                board.rules.set_on(id, on);
            }
        }
        Support::Applied => {
            ui.label(RichText::new("Used when the board is made (pours, Gerbers); it isn't a check of its own.").size(11.5).color(FAINT));
        }
        Support::Listed => {
            ui.label(RichText::new("Read from Altium files and shown; Coppersmith doesn't check it yet.").size(11.5).color(FAINT));
        }
    }
    let r = &mut board.rules;
    match id {
        RuleId::Clearance => {
            field(ui, "Clearance", |ui| length(ui, u, &mut r.clearance));
            if !r.gaps.is_empty() {
                let table: Vec<String> = r.gaps.iter().map(|g| format!("{} – {}: {}", g.a.name(), g.b.name(), u.len(g.mm))).collect();
                ui.label(RichText::new(format!("and {} pairs of kinds of copper with their own gap", r.gaps.len())).size(11.5).color(FAINT)).on_hover_text(table.join("\n"));
            }
        }
        RuleId::Width => {
            field(ui, "Preferred", |ui| length(ui, u, &mut r.track));
            field(ui, "Least", |ui| length(ui, u, &mut r.min_track));
            field(ui, "Most (0: none)", |ui| length(ui, u, &mut r.max_track));
        }
        RuleId::RoutingVias => {
            field(ui, "Pad", |ui| length(ui, u, &mut r.via_dia));
            field(ui, "Hole", |ui| length(ui, u, &mut r.via_drill));
            field(ui, "Pad, least (0: none)", |ui| length(ui, u, &mut r.via_min_dia));
            field(ui, "Pad, most (0: none)", |ui| length(ui, u, &mut r.via_max_dia));
            field(ui, "Hole, least (0: none)", |ui| length(ui, u, &mut r.via_min_drill));
            field(ui, "Hole, most (0: none)", |ui| length(ui, u, &mut r.via_max_drill));
        }
        RuleId::HoleSize => {
            field(ui, "Least", |ui| length(ui, u, &mut r.min_drill));
            field(ui, "Most (0: none)", |ui| length(ui, u, &mut r.max_drill));
        }
        RuleId::MinimumAnnularRing => {
            field(ui, "Least ring", |ui| length(ui, u, &mut r.min_ring));
        }
        RuleId::HoleToHoleClearance => {
            field(ui, "Gap", |ui| length(ui, u, &mut r.hole_gap));
        }
        RuleId::BoardOutlineClearance => {
            field(ui, "Gap", |ui| length(ui, u, &mut r.edge_clearance));
        }
        RuleId::ComponentClearance => {
            field(ui, "Gap (0: only overlap)", |ui| length(ui, u, &mut r.component_gap));
        }
        RuleId::AcuteAngle => {
            field(ui, "Least angle", |ui| ui.add(egui::DragValue::new(&mut r.min_angle).speed(0.5).range(1.0..=179.0).suffix("°")));
        }
        RuleId::Length => {
            field(ui, "Least (0: none)", |ui| length(ui, u, &mut r.min_net_length));
            field(ui, "Most (0: none)", |ui| length(ui, u, &mut r.max_net_length));
        }
        RuleId::SolderMaskExpansion => {
            field(ui, "Expansion", |ui| length(ui, u, &mut r.mask_margin));
        }
        RuleId::NetClassWidth => {
            let layers: Vec<u8> = board.copper_layers();
            let keep: Option<Vec<String>> = net.map(|n| board.class_rules_for(n).iter().map(|r| r.name.clone()).collect());
            let mut any = false;
            for rule in board.class_rules.iter_mut().filter(|c| matches!(c.kind, ClassRuleKind::Width | ClassRuleKind::PairRouting) && keep.as_ref().is_none_or(|k| k.contains(&c.name))) {
                any = true;
                ui.add_space(4.0);
                let scope = match &rule.scope {
                    ClassScope::NetClass(n) => format!("net class {n}"),
                    ClassScope::PairClass(n) => format!("pair class {n}"),
                };
                ui.label(RichText::new(format!("{} · {scope}", rule.name)).font(semibold(12.5)));
                for l in rule.layers.iter_mut() {
                    let name = if l.layer == u8::MAX { "All layers".to_string() } else if layers.contains(&l.layer) { Layer::Cu(l.layer).name() } else { format!("Layer {}", l.layer) };
                    ui.label(RichText::new(name).size(12.0).color(FAINT));
                    field(ui, "Least", |ui| optional_length(ui, u, &mut l.min_width));
                    field(ui, "Preferred", |ui| optional_length(ui, u, &mut l.pref_width));
                    field(ui, "Most", |ui| optional_length(ui, u, &mut l.max_width));
                }
            }
            if !any {
                ui.label(RichText::new("This board has no width rules for classes of nets.").size(12.0).color(FAINT));
            }
        }
        RuleId::MatchedLengths => {
            let keep: Option<Vec<String>> = net.map(|n| board.class_rules_for(n).iter().map(|r| r.name.clone()).collect());
            let mut any = false;
            for rule in board.class_rules.iter_mut().filter(|c| c.kind == ClassRuleKind::MatchedLengths && keep.as_ref().is_none_or(|k| k.contains(&c.name))) {
                any = true;
                ui.add_space(4.0);
                let scope = match &rule.scope {
                    ClassScope::NetClass(n) => format!("net class {n}"),
                    ClassScope::PairClass(n) => format!("pair class {n}"),
                };
                ui.label(RichText::new(format!("{} · {scope}", rule.name)).font(semibold(12.5)));
                field(ui, "Tolerance", |ui| optional_length(ui, u, &mut rule.tolerance));
                if matches!(rule.scope, ClassScope::PairClass(_)) {
                    ui.checkbox(&mut rule.within_pairs, "The two nets of each pair");
                    ui.checkbox(&mut rule.between_pairs, "Each pair against the others");
                }
            }
            if !any {
                ui.label(RichText::new("This board has no matched-length rules.").size(12.0).color(FAINT));
            }
        }
        _ => {}
    }
    seen_in_file(ui, &board.rules.seen, id);
}

/// What the file said of the rules of this kind.
fn seen_in_file(ui: &mut Ui, seen: &[SeenRule], id: RuleId) {
    let mine: Vec<&SeenRule> = seen
        .iter()
        .filter(|s| match id {
            RuleId::NetClassWidth => s.kind.eq_ignore_ascii_case("Width") && s.scope.contains("Class"),
            RuleId::Width => s.kind.eq_ignore_ascii_case("Width") && !s.scope.contains("Class"),
            _ => s.rule == Some(id),
        })
        .collect();
    if mine.is_empty() {
        return;
    }
    ui.add_space(4.0);
    egui::CollapsingHeader::new(RichText::new(format!("In the file ({})", mine.len())).size(12.0).color(DIM)).id_salt(("seen", id)).show(ui, |ui| {
        for s in mine.iter().take(40) {
            let state = if s.enabled { "" } else { " · off" };
            ui.label(RichText::new(format!("{} · {}{state}", s.name, if s.scope.is_empty() { "All" } else { &s.scope })).size(12.0).color(FG));
            for (k, v) in &s.params {
                ui.label(RichText::new(format!("{k} = {v}")).font(theme::mono(11.0)).color(FAINT));
            }
        }
        if mine.len() > 40 {
            ui.label(RichText::new(format!("and {} more", mine.len() - 40)).size(11.5).color(FAINT));
        }
    });
}

impl App {
    /// The card in the Checks panel for the problem picked: which rule it breaks, and the
    /// controls of that rule.
    pub fn rule_card(&mut self, ui: &mut Ui) {
        let Some(prob) = self.focus.and_then(|i| self.problems.get(i)).cloned() else { return };
        let (u, id) = (self.unit, prob.rule);
        let info = id.info();
        let net = prob.items.iter().find_map(|i| if let crate::pcb::copper::Item::Track(t) = i { self.doc.track(*t).map(|t| t.net) } else { None });
        let mut open_all = false;
        card(ui, |ui| {
            ui.horizontal(|ui| {
                let dot = if prob.severity == crate::pcb::drc::Severity::Error { RED } else { YELLOW };
                let (rect, _) = ui.allocate_exact_size(egui::vec2(12.0, 16.0), egui::Sense::hover());
                ui.painter().circle_filled(rect.center(), 4.0, dot);
                heading(ui, &prob.title);
            });
            ui.label(RichText::new(&prob.detail).size(12.0).color(DIM));
            ui.add_space(6.0);
            ui.separator();
            ui.add_space(4.0);
            ui.label(RichText::new("Rule broken").size(11.5).color(FAINT));
            ui.label(RichText::new(info.name).font(semibold(14.0)));
            let altium = if info.altium.is_empty() { String::new() } else { format!(" · Altium {}", info.altium) };
            ui.label(RichText::new(format!("{}{altium}", info.group.name())).size(11.5).color(FAINT));
            ui.add_space(4.0);
            rule_controls(ui, &mut self.doc, id, u, net);
            ui.add_space(6.0);
            if ui.button("All the design rules…").clicked() {
                open_all = true;
            }
        });
        if open_all {
            self.show_rules = true;
            self.rules_focus = Some(id);
        }
    }

    /// The window of every design rule: what Coppersmith checks and what it only shows, each
    /// with its controls, grouped as Altium groups them.
    pub fn rules_window(&mut self, ctx: &egui::Context) {
        if !self.show_rules {
            return;
        }
        let mut open = true;
        let (u, focus) = (self.unit, self.rules_focus.take());
        let before = self.doc.clone();
        egui::Window::new("Design rules").open(&mut open).resizable(true).default_size([560.0, 640.0]).show(ctx, |ui| {
            let (checked, applied, listed) = {
                let n = |s: Support| CATALOG.iter().filter(|r| r.support == s).count();
                (n(Support::Checked), n(Support::Applied), n(Support::Listed))
            };
            let on = CATALOG.iter().filter(|r| r.support == Support::Checked && self.doc.rules.is_on(r.id)).count();
            ui.label(
                RichText::new(format!("{on} of {checked} checks on · {applied} applied when the board is made · {listed} read from Altium files and shown")).size(12.0).color(DIM),
            );
            ui.add_space(6.0);
            ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                for group in Group::ALL {
                    let rules: Vec<&RuleInfo> = CATALOG.iter().filter(|r| r.group == group).collect();
                    let checks = rules.iter().filter(|r| r.support == Support::Checked).count();
                    let title = format!("{} · {}", group.name(), if checks > 0 { format!("{checks} checked") } else { "shown only".to_string() });
                    let has_focus = focus.is_some_and(|f| rules.iter().any(|r| r.id == f));
                    let header = egui::CollapsingHeader::new(RichText::new(title).font(semibold(14.0))).id_salt(("rule-group", group.name())).default_open(checks > 0 || has_focus);
                    let header = if has_focus { header.open(Some(true)) } else { header };
                    header.show(ui, |ui| {
                        for info in rules {
                            let state = if info.support == Support::Checked {
                                if self.doc.rules.is_on(info.id) { "on" } else { "off" }
                            } else {
                                info.support.name()
                            };
                            let color = if info.support == Support::Checked && !self.doc.rules.is_on(info.id) { RED } else { support_color(info.support) };
                            let label = RichText::new(format!("{}   {state}", info.name)).size(13.0);
                            let mut h = egui::CollapsingHeader::new(label.color(if color == RED { DIM } else { FG })).id_salt(("rule", info.id));
                            if focus == Some(info.id) {
                                h = h.open(Some(true));
                            }
                            let shown = h.show(ui, |ui| {
                                rule_controls(ui, &mut self.doc, info.id, u, None);
                                ui.add_space(4.0);
                            });
                            if focus == Some(info.id) {
                                shown.header_response.scroll_to_me(Some(Align::Center));
                            }
                        }
                    });
                }
            });
        });
        self.show_rules = open;
        // A change made here is one undo step, as in the panels.
        if self.doc != before {
            self.remember(before);
        }
    }
}
