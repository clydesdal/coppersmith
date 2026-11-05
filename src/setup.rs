//! Installing Coppersmith: the program goes in ~/Applications beside the other
//! apps, with a launcher entry, an icon and a `coppersmith` terminal command.
//! Nothing here runs unless the person asks, and it stays inside their home.

use crate::icons;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const NAME: &str = "coppersmith";

fn home() -> Result<PathBuf, String> {
    std::env::var_os("HOME").map(PathBuf::from).ok_or("The home folder couldn't be found.".to_string())
}

fn program_path() -> Result<PathBuf, String> {
    Ok(home()?.join("Applications").join(NAME))
}

fn command_path() -> Result<PathBuf, String> {
    Ok(home()?.join(".local/bin").join(NAME))
}

fn share_dir() -> Result<PathBuf, String> {
    match std::env::var_os("XDG_DATA_HOME").filter(|v| !v.is_empty()) {
        Some(dir) => Ok(PathBuf::from(dir)),
        None => Ok(home()?.join(".local/share")),
    }
}

fn desktop_file() -> Result<PathBuf, String> {
    Ok(share_dir()?.join("applications").join(format!("{NAME}.desktop")))
}

fn icon_file(size: u32) -> Result<PathBuf, String> {
    Ok(share_dir()?.join(format!("icons/hicolor/{size}x{size}/apps/{NAME}.png")))
}

fn scalable_icon() -> Result<PathBuf, String> {
    Ok(share_dir()?.join(format!("icons/hicolor/scalable/apps/{NAME}.svg")))
}

fn make_room(path: &Path) -> Result<(), String> {
    match path.parent() {
        Some(dir) => std::fs::create_dir_all(dir).map_err(|e| format!("{} couldn't be made: {e}", dir.display())),
        None => Ok(()),
    }
}

/// True when this very program is the one in the Applications folder.
#[allow(dead_code)]
pub fn is_current() -> bool {
    let Ok(installed) = program_path() else { return false };
    let size = |p: &Path| std::fs::metadata(p).map(|m| m.len()).ok();
    std::env::current_exe()
        .is_ok_and(|exe| exe == installed || (size(&installed).is_some() && size(&exe) == size(&installed)))
}

fn write_pictures() -> Result<(), String> {
    for (size, bytes) in icons::PICTURES {
        let path = icon_file(size)?;
        make_room(&path)?;
        std::fs::write(&path, bytes).map_err(|e| format!("The icon couldn't be saved: {e}"))?;
    }
    let path = scalable_icon()?;
    make_room(&path)?;
    std::fs::write(&path, icons::SCALABLE).map_err(|e| format!("The icon couldn't be saved: {e}"))
}

/// Lets the launcher know, if the tools for that are around. It finds new
/// entries by itself in time, so nothing's wrong if they aren't.
fn refresh_launcher() {
    let Ok(share) = share_dir() else { return };
    let run = |tool: &str, args: &[&str], dir: PathBuf| {
        let _ = Command::new(tool).args(args).arg(dir).stdout(Stdio::null()).stderr(Stdio::null()).status();
    };
    run("update-desktop-database", &[], share.join("applications"));
    run("gtk-update-icon-cache", &["-q", "-t", "-f"], share.join("icons/hicolor"));
}

pub fn install() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| format!("The program couldn't find itself: {e}"))?;
    let program = program_path()?;
    make_room(&program)?;
    if exe != program {
        // Written beside it first, so a copy that's running isn't disturbed.
        let fresh = program.with_extension("new");
        std::fs::copy(&exe, &fresh).map_err(|e| format!("The program couldn't be copied: {e}"))?;
        std::fs::set_permissions(&fresh, std::fs::Permissions::from_mode(0o755)).map_err(|e| e.to_string())?;
        std::fs::rename(&fresh, &program).map_err(|e| format!("The program couldn't be put in place: {e}"))?;
    }

    let command = command_path()?;
    make_room(&command)?;
    // Only ever replaces a link, so it can't overwrite someone's own file.
    if command.is_symlink() {
        std::fs::remove_file(&command).map_err(|e| e.to_string())?;
    }
    if !command.exists() {
        symlink(&program, &command).map_err(|e| format!("The terminal command couldn't be made: {e}"))?;
    }

    write_pictures()?;
    let desktop = desktop_file()?;
    make_room(&desktop)?;
    let entry = format!(
        "[Desktop Entry]\nType=Application\nName=Coppersmith\n\
         Comment=Lay out printed circuit boards and write Gerbers\n\
         Exec=\"{}\" %F\nIcon={NAME}\nTerminal=false\nCategories=Development;Electronics;Engineering;\n\
         Keywords=pcb;layout;board;footprint;gerber;routing;\nStartupWMClass={NAME}\n",
        program.display()
    );
    std::fs::write(&desktop, entry).map_err(|e| format!("The launcher entry couldn't be made: {e}"))?;
    refresh_launcher();
    Ok(())
}

pub fn uninstall() -> Result<(), String> {
    let program = program_path()?;
    let command = command_path()?;
    let mut files = vec![program.clone(), desktop_file()?, scalable_icon()?];
    for (size, _) in icons::PICTURES {
        files.push(icon_file(size)?);
    }
    if command.is_symlink() && std::fs::read_link(&command).is_ok_and(|to| to == program) {
        files.push(command);
    }
    for file in files {
        match std::fs::remove_file(&file) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                return Err(format!("{} couldn't be removed: {e}", file.display()));
            }
            _ => {}
        }
    }
    refresh_launcher();
    Ok(())
}
