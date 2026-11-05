//! Coppersmith: lay out printed circuit boards, in Copperview's clothes.

mod app;
mod copper;
mod drc;
mod font;
mod geom;
mod gerber;
mod icons;
mod library;
mod model;
mod panels;
mod pinch;
mod play;
mod sample;
mod setup;
mod theme;
mod view;

use app::Launch;
use eframe::egui;
use geom::p;
use std::path::PathBuf;

const HELP: &str = "\
Coppersmith: lay out printed circuit boards.

  coppersmith                   open the window with the sample board
  coppersmith BOARD.cspcb       open a board
  coppersmith install           add to the Applications folder and app launcher
  coppersmith uninstall         take it out again

Options:
  --gerbers DIR                 write the board's Gerber and drill files and exit
  --check                       print the rule check for the board and exit
  --write-sample FILE           save the sample board as a file and exit
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
                setup::install()?;
                println!("Coppersmith is installed. Find it in your app launcher, or run: coppersmith");
                return Ok(());
            }
            "uninstall" => {
                setup::uninstall()?;
                println!("Coppersmith has been removed. Your boards weren't touched.");
                return Ok(());
            }
            "--gerbers" => gerbers = Some(PathBuf::from(value("--gerbers")?)),
            "--check" => check = true,
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
    if launch.screenshot.is_some() && std::env::var_os("DISPLAY").is_some() {
        unsafe { std::env::remove_var("WAYLAND_DISPLAY") };
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
