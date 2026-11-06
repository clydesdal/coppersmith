//! Acts out mouse and key steps through the same path real input takes, so a
//! check run can prove that clicking really works.

use crate::ui::app::App;
use crate::ui::view::PLAY_FROM;
use eframe::egui::{self, Event, Key, Modifiers, MouseWheelUnit, PointerButton, pos2, vec2};

#[derive(Clone, Debug)]
pub enum Step {
    /// A spot on the board in millimetres, turned into a spot on the screen
    /// when it's played.
    Board(Box<Step>),
    Move(f32, f32),
    Down(f32, f32, PointerButton),
    Up(f32, f32, PointerButton),
    Scroll(f32),
    Type(String),
    Key(Key, Modifiers),
    /// Keys held down until let go, like Ctrl for Ctrl+click.
    Hold(Modifiers),
    /// Saves the board to a file, for checks to read back.
    SaveBoard(String),
    Wait,
}

fn key_of(text: &str) -> Option<(Key, Modifiers)> {
    let mut m = Modifiers::NONE;
    let mut name = text;
    for part in text.split('+') {
        match part.to_lowercase().as_str() {
            "ctrl" => m.ctrl = true,
            "shift" => m.shift = true,
            "alt" => m.alt = true,
            _ => name = part,
        }
    }
    m.command = m.ctrl;
    let title: String = name.chars().take(1).flat_map(char::to_uppercase).chain(name.chars().skip(1)).collect();
    let key = Key::from_name(name).or_else(|| Key::from_name(&name.to_uppercase())).or_else(|| Key::from_name(&title))?;
    Some((key, m))
}

pub fn parse(text: &str) -> Vec<Step> {
    let mut steps = Vec::new();
    for action in text.split(';') {
        let words: Vec<&str> = action.split_whitespace().collect();
        let num = |i: usize| words.get(i).and_then(|t| t.parse::<f32>().ok()).unwrap_or(0.0);
        let (x, y) = (num(1), num(2));
        let press = |steps: &mut Vec<Step>, b: PointerButton| {
            steps.extend([Step::Move(x, y), Step::Wait, Step::Down(x, y, b), Step::Wait, Step::Up(x, y, b), Step::Wait]);
        };
        let (word, board) = match words.first().copied() {
            Some(w) if w.starts_with('b') && ["bmove", "bclick", "brclick", "bdclick", "bdrag", "bdown", "bup"].contains(&w) => (Some(&w[1..]), true),
            other => (other, false),
        };
        let first = steps.len();
        match word {
            Some("move") => steps.extend([Step::Move(x, y), Step::Wait]),
            // A press and a let-go on their own, for keys in the middle of a drag.
            Some("down") => steps.extend([Step::Move(x, y), Step::Wait, Step::Down(x, y, PointerButton::Primary), Step::Wait]),
            Some("up") => steps.extend([Step::Move(x, y), Step::Wait, Step::Up(x, y, PointerButton::Primary), Step::Wait]),
            Some("click") => press(&mut steps, PointerButton::Primary),
            Some("rclick") => press(&mut steps, PointerButton::Secondary),
            Some("dclick") => {
                press(&mut steps, PointerButton::Primary);
                steps.pop();
                steps.extend([Step::Down(x, y, PointerButton::Primary), Step::Up(x, y, PointerButton::Primary), Step::Wait]);
            }
            Some("drag") => {
                let (x2, y2) = (num(3), num(4));
                steps.extend([Step::Move(x, y), Step::Wait, Step::Down(x, y, PointerButton::Primary), Step::Wait]);
                for i in 1..=6 {
                    let t = i as f32 / 6.0;
                    steps.extend([Step::Move(x + (x2 - x) * t, y + (y2 - y) * t), Step::Wait]);
                }
                steps.extend([Step::Up(x2, y2, PointerButton::Primary), Step::Wait]);
            }
            Some("scroll") => steps.extend([Step::Move(x, y), Step::Wait, Step::Scroll(num(3)), Step::Wait]),
            Some("type") => steps.extend([Step::Type(words[1..].join(" ")), Step::Wait]),
            Some("key") => {
                if let Some((key, m)) = words.get(1).and_then(|k| key_of(k)) {
                    steps.extend([Step::Key(key, m), Step::Wait]);
                }
            }
            Some("hold") => {
                let m = words.get(1).and_then(|k| key_of(&format!("{k}+A"))).map(|(_, m)| m).unwrap_or(Modifiers::NONE);
                steps.extend([Step::Hold(m), Step::Wait]);
            }
            Some("saveboard") => {
                if let Some(path) = words.get(1) {
                    steps.extend([Step::SaveBoard(path.to_string()), Step::Wait]);
                }
            }
            Some("release") => steps.extend([Step::Hold(Modifiers::NONE), Step::Wait]),
            Some("wait") => steps.extend(std::iter::repeat_n(Step::Wait, x as usize)),
            _ => {}
        }
        if board {
            for s in &mut steps[first..] {
                if !matches!(s, Step::Wait) {
                    *s = Step::Board(Box::new(s.clone()));
                }
            }
        }
    }
    steps.reverse();
    steps
}

impl App {
    pub fn play_step(&mut self, raw: &mut egui::RawInput) {
        if self.frames < PLAY_FROM {
            return;
        }
        let Some(mut step) = self.play.pop() else { return };
        if let Step::Board(inner) = step {
            let tr = self.tr(self.canvas_rect);
            let at = |x: f32, y: f32| {
                let q = tr.pt(crate::pcb::geom::p(x as f64, y as f64));
                (q.x, q.y)
            };
            step = match *inner {
                Step::Move(x, y) => {
                    let (x, y) = at(x, y);
                    Step::Move(x, y)
                }
                Step::Down(x, y, b) => {
                    let (x, y) = at(x, y);
                    Step::Down(x, y, b)
                }
                Step::Up(x, y, b) => {
                    let (x, y) = at(x, y);
                    Step::Up(x, y, b)
                }
                other => other,
            };
        }
        let button = |x: f32, y: f32, button: PointerButton, pressed: bool| Event::PointerButton {
            pos: pos2(x, y),
            button,
            pressed,
            modifiers: Modifiers::NONE,
        };
        match step {
            Step::Board(_) => {}
            Step::Move(x, y) => raw.events.push(Event::PointerMoved(pos2(x, y))),
            Step::Down(x, y, b) => raw.events.push(button(x, y, b, true)),
            Step::Up(x, y, b) => raw.events.push(button(x, y, b, false)),
            Step::Scroll(amount) => raw.events.push(Event::MouseWheel {
                unit: MouseWheelUnit::Point,
                delta: vec2(0.0, amount),
                phase: egui::TouchPhase::Move,
                source: egui::MouseWheelSource::Unknown,
                modifiers: Modifiers::NONE,
            }),
            Step::Type(text) => raw.events.push(Event::Text(text)),
            Step::Key(key, modifiers) => {
                raw.events.push(Event::ModifiersChanged(modifiers));
                for pressed in [true, false] {
                    raw.events.push(Event::Key { key, physical_key: None, pressed, repeat: false, modifiers });
                }
                raw.events.push(Event::ModifiersChanged(Modifiers::NONE));
            }
            Step::Hold(m) => raw.events.push(Event::ModifiersChanged(m)),
            Step::SaveBoard(path) => {
                let _ = std::fs::write(&path, self.doc.save());
            }
            Step::Wait => {}
        }
    }
}
