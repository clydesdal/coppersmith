//! Coppersmith: lay out printed circuit boards, in Copperview's clothes.
//!
//! - `pcb`: the board and everything worked out from it; no windows, no OS.
//! - `ui`: the window.
//! - `platform`: what differs between Linux, macOS and Windows.

// A release build on Windows opens no console window behind the app.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod pcb;
mod platform;
mod ui;

use pcb::{copper, drc, gerber, model, sample};
use ui::app::{self, Launch};
use ui::icons;
use eframe::egui;
use pcb::geom::p;
use std::path::PathBuf;

const HELP: &str = "\
Coppersmith: lay out printed circuit boards.

  coppersmith                   open the window with the sample board
  coppersmith BOARD.cspcb       open a board
  coppersmith install           Linux: add to ~/Applications and the app launcher
  coppersmith uninstall         Linux: take it out again

Options:
  --gerbers DIR                 write the board's Gerber and drill files and exit
  --check                       print the rule check for the board and exit
  --write-sample FILE           save the sample board as a file and exit
  --read-altium FILE            read an Altium PcbLib, SchLib or IntLib and
                                say what's in it and what was left out
  --zoom X,Y,WIDTH              start looking at this spot, WIDTH mm across
  --select REFDES               start with this part selected
  --screenshot FILE.png         draw a few frames, save the picture, and exit
  --play STEPS                  with --screenshot: act out mouse and key steps,
                                such as 'click 600 400; key w; key ctrl+z'
";

fn numbers(text: &str, count: usize, name: &str) -> Result<Vec<f64>, String> {
    let values: Vec<f64> = text.split(',').filter_map(|t| t.trim().parse().ok()).collect();
    if values.len() == count { Ok(values) } else { Err(format!("{name} needs {count} numbers separated by commas")) }
}

fn load(path: &Option<PathBuf>) -> Result<model::Board, String> {
    match path {
        Some(path) => {
            let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
            model::Board::load(&text)
        }
        None => Ok(sample::board()),
    }
}

fn run() -> Result<(), String> {
    if std::env::args().len() > 1 {
        platform::attach_console();
    }
    let mut launch = Launch::default();
    let mut gerbers: Option<PathBuf> = None;
    let mut check = false;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = |name: &str| args.next().ok_or(format!("{name} needs a value"));
        match arg.as_str() {
            "-h" | "--help" | "help" => {
                print!("{HELP}");
                return Ok(());
            }
            "install" => {
                println!("{}", platform::install()?);
                return Ok(());
            }
            "uninstall" => {
                println!("{}", platform::uninstall()?);
                return Ok(());
            }
            "--gerbers" => gerbers = Some(PathBuf::from(value("--gerbers")?)),
            "--check" => check = true,
            "--read-altium" => {
                let path = PathBuf::from(value("--read-altium")?);
                let got = pcb::altium::read(&path)?;
                println!("{} footprints", got.footprints.len());
                for f in &got.footprints {
                    let holes = f.pads.iter().filter(|p| p.drill > 0.0).count();
                    println!("  {:<28} {:>3} pads ({holes} through) {}", f.name, f.pads.len(), f.description);
                }
                println!("{} symbols", got.symbols.len());
                for s in &got.symbols {
                    let fp = s.footprints.first().map(|f| format!(" -> {f}")).unwrap_or_default();
                    println!("  {:<28} {:>3} pins, {} part(s), {}?{fp}  {}", s.name, s.pins.len(), s.parts, s.prefix, s.description);
                }
                if !got.notes.is_empty() {
                    println!("Left out or approximated:");
                    for n in &got.notes {
                        println!("  {n}");
                    }
                }
                return Ok(());
            }
            "--write-sample" => {
                let path = PathBuf::from(value("--write-sample")?);
                return std::fs::write(&path, sample::board().save()).map_err(|e| format!("{}: {e}", path.display()));
            }
            "--screenshot" => launch.screenshot = Some(PathBuf::from(value("--screenshot")?)),
            "--play" => launch.play = Some(value("--play")?),
            "--select" => launch.select = Some(value("--select")?),
            "--zoom" => {
                let v = numbers(&value("--zoom")?, 3, "--zoom")?;
                launch.zoom = Some((p(v[0], v[1]), v[2]));
            }
            other if other.starts_with("--") => return Err(format!("I don't know {other}")),
            path => launch.path = Some(PathBuf::from(path)),
        }
    }

    if gerbers.is_some() || check {
        let board = load(&launch.path)?;
        let found = drc::check(&board, &copper::analyse(&board));
        if check {
            for prob in &found {
                println!("{:?}\t{}\t{}", prob.severity, prob.title, prob.detail);
            }
            println!("{} problems", found.len());
        }
        if let Some(dir) = gerbers {
            for path in gerber::write(&board, &dir)? {
                println!("{}", path.display());
            }
        }
        return Ok(());
    }

    let mut viewport = egui::ViewportBuilder::default()
        .with_title("Coppersmith")
        .with_app_id("coppersmith")
        .with_inner_size([1440.0, 900.0])
        .with_min_inner_size([640.0, 420.0]);
    if let Some((rgba, side)) = icons::picture_pixels(128) {
        viewport = viewport.with_icon(egui::IconData { rgba, width: side, height: side });
    }
    if launch.screenshot.is_some() {
        platform::prefer_drawing_while_hidden();
    }
    let mut options = eframe::NativeOptions { viewport, multisampling: 4, ..Default::default() };
    options.glow_options.vsync = launch.screenshot.is_none();
    eframe::run_native("Coppersmith", options, Box::new(move |cc| Ok(Box::new(app::App::new(cc, launch)))))
        .map_err(|e| format!("The window couldn't open: {e}"))
}

fn main() {
    if let Err(error) = run() {
        eprintln!("coppersmith: {error}");
        std::process::exit(1);
    }
}
