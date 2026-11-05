//! Everything that differs between Linux, macOS and Windows lives here, so the
//! rest of the program reads the same on all three.
//!
//! - Trackpad pinches: macOS and Windows hand them to the window library
//!   already; on Linux they're heard straight from the Wayland desktop.
//! - Folders: where your own footprints live, and how a folder is shown.
//! - Installing: Linux gets ~/Applications and a launcher entry; macOS and
//!   Windows are packaged by the scripts in `packaging/`.

use std::path::{Path, PathBuf};

#[cfg(target_os = "linux")]
mod install_linux;
#[cfg(target_os = "linux")]
mod pinch_wayland;

#[cfg(target_os = "linux")]
pub use pinch_wayland::Pinch;

/// Elsewhere the window library passes pinches on as zooms, so there's
/// nothing extra to listen to.
#[cfg(not(target_os = "linux"))]
#[derive(Default)]
pub struct Pinch;

#[cfg(not(target_os = "linux"))]
impl Pinch {
    #[allow(dead_code)]
    pub fn add(&self, _zoom: f32) {}
    pub fn take(&self) -> (f32, [f32; 2]) {
        (1.0, [0.0, 0.0])
    }
    pub fn stop(&mut self) {}
}

/// Starts listening for trackpad pinches on this window, where that's needed.
pub fn listen_for_pinches(pinch: &mut Pinch, cc: &eframe::CreationContext<'_>) {
    #[cfg(target_os = "linux")]
    {
        use raw_window_handle::{HasDisplayHandle, RawDisplayHandle};
        if let Ok(handle) = cc.display_handle()
            && let RawDisplayHandle::Wayland(wayland) = handle.as_raw()
        {
            pinch.listen(wayland.display.as_ptr(), &cc.egui_ctx);
        }
    }
    #[cfg(not(target_os = "linux"))]
    let _ = (pinch, cc);
}

/// Picture runs draw while the window is hidden. On Linux X11 keeps drawing
/// when the screen sleeps and Wayland doesn't, so prefer X11 when there is one.
pub fn prefer_drawing_while_hidden() {
    #[cfg(target_os = "linux")]
    if std::env::var_os("DISPLAY").is_some() {
        // Nothing else is running yet.
        unsafe { std::env::remove_var("WAYLAND_DISPLAY") };
    }
}

/// A Windows release build has no console of its own; when it's started from
/// a terminal, print there, so `--check` and `--gerbers` still answer.
pub fn attach_console() {
    #[cfg(windows)]
    {
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn AttachConsole(process: u32) -> i32;
        }
        const PARENT: u32 = u32::MAX;
        unsafe {
            AttachConsole(PARENT);
        }
    }
}

pub fn home() -> Option<PathBuf> {
    let var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    std::env::var_os(var).filter(|v| !v.is_empty()).map(PathBuf::from)
}

/// Where Coppersmith keeps your own files, like footprints.
pub fn data_dir() -> Option<PathBuf> {
    let base = if cfg!(target_os = "macos") {
        home()?.join("Library/Application Support")
    } else if cfg!(windows) {
        std::env::var_os("APPDATA").map(PathBuf::from).or_else(|| Some(home()?.join("AppData/Roaming")))?
    } else {
        match std::env::var_os("XDG_DATA_HOME").filter(|v| !v.is_empty()) {
            Some(dir) => PathBuf::from(dir),
            None => home()?.join(".local/share"),
        }
    };
    Some(base.join("coppersmith"))
}

/// Shows a folder in the computer's file manager.
pub fn open_folder(dir: &Path) -> Result<(), String> {
    let program = if cfg!(target_os = "macos") {
        "open"
    } else if cfg!(windows) {
        "explorer"
    } else {
        "xdg-open"
    };
    std::process::Command::new(program).arg(dir).spawn().map(|_| ()).map_err(|e| e.to_string())
}

/// Finds another program by name: on the PATH, or where apps are usually put.
pub fn find_program(name: &str) -> Option<PathBuf> {
    let file = if cfg!(windows) { format!("{name}.exe") } else { name.to_string() };
    let mut places: Vec<PathBuf> = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
    if let Some(home) = home() {
        places.push(home.join("Applications"));
    }
    if cfg!(target_os = "macos") {
        let title: String = name[..1].to_uppercase() + &name[1..];
        places.push(PathBuf::from(format!("/Applications/{title}.app/Contents/MacOS")));
    }
    places.into_iter().map(|dir| dir.join(&file)).find(|p| p.is_file())
}

pub fn install() -> Result<String, String> {
    #[cfg(target_os = "linux")]
    {
        install_linux::install()?;
        Ok("Coppersmith is installed. Find it in your app launcher, or run: coppersmith".into())
    }
    #[cfg(target_os = "macos")]
    {
        Err("On a Mac, run packaging/macos/make-app.sh and drag Coppersmith.app to Applications.".into())
    }
    #[cfg(windows)]
    {
        Err("On Windows, run packaging\\windows\\make-zip.ps1 and unzip it where you keep programs.".into())
    }
}

pub fn uninstall() -> Result<String, String> {
    #[cfg(target_os = "linux")]
    {
        install_linux::uninstall()?;
        Ok("Coppersmith has been removed. Your boards weren't touched.".into())
    }
    #[cfg(not(target_os = "linux"))]
    {
        Err("Delete Coppersmith where you put it; your boards aren't kept inside it.".into())
    }
}
