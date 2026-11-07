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
  --read-altium FILE            read an Altium or Circuit Studio library,
                                board, schematic sheet or project and say
                                what's in it; a project's schematic netlist
                                is checked against its board
  --zoom X,Y,WIDTH              start looking at this spot, WIDTH mm across
  --select REFDES               start with this part selected
  --layout WORDS                open with this layout, such as 'ribbon, no-left,
                                right:checks, filter:tracks+vias'
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
        Some(path) if pcb::altium::is_board(path) => Ok(pcb::altium::read_board(path)?.board),
        Some(path) => {
            let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
            model::Board::load(&text)
        }
        None => Ok(sample::board()),
    }
}

/// `--read-altium`: says what's in a library, board, sheet or project.
fn read_altium_design(path: &std::path::Path) -> Result<(), String> {
    use pcb::altium;
    if altium::is_board(path) {
        let got = altium::read_board(path)?;
        print_board(&got);
        return Ok(());
    }
    if altium::is_sheet(path) {
        let sheet = altium::read_sheet(path)?;
        print_sheet(&sheet);
        let nets = altium::schdoc::netlist(std::slice::from_ref(&sheet));
        println!("{} nets", nets.len());
        for (name, pins) in &nets {
            println!("  {name}: {}", pins.join(" "));
        }
        return Ok(());
    }
    if altium::is_project(path) {
        let docs = altium::project_documents(path)?;
        let mut sheets = Vec::new();
        let mut board = None;
        for d in &docs {
            if altium::is_sheet(d) {
                let s = altium::read_sheet(d)?;
                print_sheet(&s);
                sheets.push(s);
            } else if altium::is_board(d) && board.is_none() {
                let b = altium::read_board(d)?;
                print_board(&b);
                board = Some(b);
            }
        }
        let nets = altium::schdoc::netlist(&sheets);
        println!("Schematic netlist: {} nets", nets.len());
        if let Some(b) = board {
            let (checked, problems) = altium::compare_nets(&nets, &b.board);
            println!("Against the board: {checked} nets checked, {} disagree", problems.len());
            for p in &problems {
                println!("  {p}");
            }
        }
        return Ok(());
    }
    // A library.
    let got = altium::read(path)?;
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
    Ok(())
}

fn print_board(got: &pcb::altium::pcbdoc::ImportedBoard) {
    let b = &got.board;
    let bb = b.outline_bbox();
    println!(
        "Board {}: {:.2} × {:.2} mm, {} copper layers, {} parts, {} nets, {} tracks, {} vias",
        b.name,
        bb.w(),
        bb.h(),
        b.copper,
        b.footprints.len(),
        b.nets.len(),
        b.tracks.len(),
        b.vias.len()
    );
    let r = &b.rules;
    println!(
        "  rules: clearance {} mm, track {} mm (least {}), via {}/{} mm, hole least {}, ring least {}",
        r.clearance, r.track, r.min_track, r.via_dia, r.via_drill, r.min_drill, r.min_ring
    );
    for v in &b.pours {
        let what = if v.plane { "plane" } else { "pour" };
        let conn = match v.connect {
            model::Connect::Direct => "solid".to_string(),
            model::Connect::Relief => format!("relief {}/{} mm", v.spoke, v.gap),
        };
        println!(
            "  {what} {} on {}: {:.1} mm² in {} piece{}, clearance {} mm, edge {} mm, pads {conn}",
            b.net_name(v.net),
            model::Layer::Cu(v.layer).name().to_lowercase(),
            v.copper_area(),
            v.fill.len(),
            if v.fill.len() == 1 { "" } else { "s" },
            v.clearance,
            v.edge
        );
    }
    if !b.fills.is_empty() {
        println!("  {} fill{}", b.fills.len(), if b.fills.len() == 1 { "" } else { "s" });
    }
    for n in &got.notes {
        println!("  {n}");
    }
}

fn print_sheet(s: &pcb::altium::schdoc::Sheet) {
    println!("Sheet {}: {} parts, {} wires, {} names", s.name, s.parts.len(), s.wires.len(), s.names.len());
    for p in &s.parts {
        println!("  {:<8} {:<24} {:<20} {} pins  [{}]", p.designator, p.lib_ref, p.value, p.pins.len(), p.footprint);
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
                return read_altium_design(&path);
            }
            "--read-altium-lib" => return read_altium_design(&PathBuf::from(value("--read-altium-lib")?)),
            "--write-sample" => {
                let path = PathBuf::from(value("--write-sample")?);
                return std::fs::write(&path, sample::board().save()).map_err(|e| format!("{}: {e}", path.display()));
            }
            "--screenshot" => launch.screenshot = Some(PathBuf::from(value("--screenshot")?)),
            "--play" => launch.play = Some(value("--play")?),
            "--select" => launch.select = Some(value("--select")?),
            "--layout" => launch.layout = Some(value("--layout")?),
            "--library" => launch.library = Some(PathBuf::from(value("--library")?)),
            "--schematic" => launch.schematic = Some(PathBuf::from(value("--schematic")?)),
            "--zoom" => {
                let v = numbers(&value("--zoom")?, 3, "--zoom")?;
                launch.zoom = Some((p(v[0], v[1]), v[2]));
            }
            other if other.starts_with("--") => return Err(format!("I don't know {other}")),
            path => launch.path = Some(PathBuf::from(path)),
        }
    }

    if gerbers.is_some() || check {
        let t = std::time::Instant::now();
        let mut board = load(&launch.path)?;
        eprintln!("[time] load {:?}", t.elapsed());
        let t = std::time::Instant::now();
        board.repour();
        eprintln!("[time] repour {:?}", t.elapsed());
        let t = std::time::Instant::now();
        let cu = copper::analyse(&board);
        eprintln!("[time] analyse {:?} ({} items, {} parts)", t.elapsed(), cu.items.len(), cu.items.iter().map(|c| c.parts.len()).sum::<usize>());
        eprintln!("[count] tracks {} vias {} fps {} pours {} fills {} keepouts {} (on parts {})", board.tracks.len(), board.vias.len(), board.footprints.len(), board.pours.len(), board.fills.len(), board.keepouts.len(), board.footprints.iter().map(|f| f.keepouts.len()).sum::<usize>());
        let t = std::time::Instant::now();
        let found = drc::check(&board, &cu);
        eprintln!("[time] drc {:?}", t.elapsed());
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
