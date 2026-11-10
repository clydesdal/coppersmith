//! The files opened lately, kept between runs in a plain text file (one path a
//! line, newest first) in the user's settings folder.

use std::path::{Path, PathBuf};

/// How many are kept.
pub const MAX: usize = 15;

/// Where the list lives: the platform's settings folder, if there is one.
fn file() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let dir = if cfg!(target_os = "macos") {
        home?.join("Library/Application Support/Coppersmith")
    } else if cfg!(windows) {
        PathBuf::from(std::env::var_os("APPDATA")?).join("Coppersmith")
    } else {
        std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).or_else(|| home.map(|h| h.join(".config")))?.join("coppersmith")
    };
    Some(dir.join("recent.txt"))
}

/// The saved list (not in tests, which shouldn't read or change a person's own).
pub fn load() -> Vec<PathBuf> {
    if cfg!(test) {
        return Vec::new();
    }
    let Some(text) = file().and_then(|f| std::fs::read_to_string(f).ok()) else { return Vec::new() };
    text.lines().filter(|l| !l.trim().is_empty()).map(PathBuf::from).take(MAX).collect()
}

pub fn save(list: &[PathBuf]) {
    if cfg!(test) {
        return;
    }
    let Some(f) = file() else { return };
    if let Some(dir) = f.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let text: String = list.iter().map(|p| format!("{}\n", p.display())).collect();
    let _ = std::fs::write(f, text);
}

/// Puts a file first, once, and keeps no more than `MAX`.
pub fn push(list: &mut Vec<PathBuf>, path: &Path) {
    list.retain(|p| p != path);
    list.insert(0, path.to_path_buf());
    list.truncate(MAX);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_newest_is_first_and_nothing_is_listed_twice() {
        let mut l = Vec::new();
        for n in ["a", "b", "c"] {
            push(&mut l, Path::new(n));
        }
        assert_eq!(l, [PathBuf::from("c"), PathBuf::from("b"), PathBuf::from("a")]);
        push(&mut l, Path::new("a"));
        assert_eq!(l, [PathBuf::from("a"), PathBuf::from("c"), PathBuf::from("b")]);
    }

    #[test]
    fn only_the_latest_are_kept() {
        let mut l = Vec::new();
        for n in 0..MAX + 5 {
            push(&mut l, Path::new(&format!("f{n}")));
        }
        assert_eq!(l.len(), MAX);
        assert_eq!(l[0], PathBuf::from(format!("f{}", MAX + 4)));
    }
}
