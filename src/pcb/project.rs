//! A project: the schematics, boards and libraries that go together.
//!
//! An Altium or Circuit Studio project file (`.PrjPcb`, `.CSPrjPcb`) lists its
//! documents; with none, the project is whatever of those kinds lies in the
//! folder of the file being worked on. Only what Coppersmith can open is kept
//! (project outputs, reports and the like are left out).

use std::path::{Path, PathBuf};

#[derive(Clone, Copy, PartialEq, Eq, Debug, PartialOrd, Ord)]
pub enum FileKind {
    Schematic,
    Board,
    Library,
}

impl FileKind {
    pub fn of(path: &Path) -> Option<FileKind> {
        let ext = path.extension()?.to_string_lossy().to_lowercase();
        match ext.as_str() {
            "schdoc" | "csschdoc" | "cssch" | "kicad_sch" => Some(FileKind::Schematic),
            "pcbdoc" | "cspcbdoc" | "cspcb" | "kicad_pcb" => Some(FileKind::Board),
            "pcblib" | "schlib" | "intlib" | "kicad_mod" | "kicad_sym" => Some(FileKind::Library),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            FileKind::Schematic => "Schematics",
            FileKind::Board => "Boards",
            FileKind::Library => "Libraries",
        }
    }
}

/// A project file (`.PrjPcb`, `.CSPrjPcb`).
pub fn is_project_file(path: &Path) -> bool {
    path.extension().is_some_and(|e| e.eq_ignore_ascii_case("prjpcb") || e.eq_ignore_ascii_case("csprjpcb") || e.eq_ignore_ascii_case("kicad_pro"))
}

#[derive(Clone, Debug, PartialEq)]
pub struct ProjectFile {
    pub path: PathBuf,
    pub kind: FileKind,
    /// Listed in the project but not there.
    pub missing: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Project {
    pub name: String,
    /// The project file, if it came from one.
    pub file: Option<PathBuf>,
    pub dir: PathBuf,
    pub files: Vec<ProjectFile>,
}

fn sorted(mut files: Vec<ProjectFile>) -> Vec<ProjectFile> {
    files.sort_by(|a, b| a.kind.cmp(&b.kind).then_with(|| a.path.file_name().map(|n| n.to_ascii_lowercase()).cmp(&b.path.file_name().map(|n| n.to_ascii_lowercase()))));
    files.dedup_by(|a, b| a.path == b.path);
    files
}

impl Project {
    /// A project from its file: the documents it lists, paths Windows-style
    /// and relative to the file.
    pub fn from_file(path: &Path) -> Result<Project, String> {
        // A KiCad project lists nothing: it's the files beside it.
        if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("kicad_pro")) {
            let dir = path.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new(".")).to_path_buf();
            let mut project = Project::from_folder(&dir);
            project.name = path.file_stem().unwrap_or_default().to_string_lossy().to_string();
            project.file = Some(path.to_path_buf());
            return Ok(project);
        }
        let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let text = String::from_utf8_lossy(&bytes);
        let dir = path.parent().unwrap_or(Path::new(".")).to_path_buf();
        let mut files = Vec::new();
        let mut in_doc = false;
        for line in text.lines() {
            let line = line.trim();
            if line.starts_with('[') {
                in_doc = line.starts_with("[Document");
            } else if in_doc && let Some(p) = line.strip_prefix("DocumentPath=") {
                let full = dir.join(p.replace('\\', "/"));
                if let Some(kind) = FileKind::of(&full) {
                    let missing = !full.exists();
                    files.push(ProjectFile { path: full, kind, missing });
                }
            }
        }
        Ok(Project { name: path.file_stem().unwrap_or_default().to_string_lossy().to_string(), file: Some(path.to_path_buf()), dir, files: sorted(files) })
    }

    /// The openable files in a folder, as a project named for the folder.
    pub fn from_folder(dir: &Path) -> Project {
        let mut files = Vec::new();
        if let Ok(entries) = std::fs::read_dir(dir) {
            for e in entries.flatten() {
                let path = e.path();
                if path.is_file()
                    && let Some(kind) = FileKind::of(&path)
                {
                    files.push(ProjectFile { path, kind, missing: false });
                }
            }
        }
        let name = dir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "Project".into());
        Project { name, file: None, dir: dir.to_path_buf(), files: sorted(files) }
    }

    /// The project a file belongs to: the project file in its folder if there
    /// is one that lists it, else the folder.
    pub fn around(path: &Path) -> Project {
        let dir = path.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new(".")).to_path_buf();
        if let Ok(entries) = std::fs::read_dir(&dir) {
            let mut projects: Vec<PathBuf> = entries.flatten().map(|e| e.path()).filter(|p| is_project_file(p)).collect();
            projects.sort();
            for pf in projects {
                if let Ok(p) = Project::from_file(&pf)
                    && p.contains(path)
                {
                    return p;
                }
            }
        }
        Project::from_folder(&dir)
    }

    pub fn contains(&self, path: &Path) -> bool {
        self.files.iter().any(|f| same_file(&f.path, path))
    }

    pub fn of_kind(&self, kind: FileKind) -> impl Iterator<Item = &ProjectFile> {
        self.files.iter().filter(move |f| f.kind == kind)
    }
}

/// The same file, whichever way its path is written.
pub fn same_file(a: &Path, b: &Path) -> bool {
    a == b || matches!((a.canonicalize(), b.canonicalize()), (Ok(x), Ok(y)) if x == y)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata/altium").join(name)
    }

    #[test]
    fn kinds_are_told_by_extension_in_any_case() {
        assert_eq!(FileKind::of(Path::new("a/01 MAIN.SchDoc")), Some(FileKind::Schematic));
        assert_eq!(FileKind::of(Path::new("x.CSPcbDoc")), Some(FileKind::Board));
        assert_eq!(FileKind::of(Path::new("Ezdisplay.SCHLIB")), Some(FileKind::Library));
        assert_eq!(FileKind::of(Path::new("report.html")), None);
        assert_eq!(FileKind::of(Path::new("noext")), None);
        assert!(is_project_file(Path::new("a.PrjPcb")) && is_project_file(Path::new("b.CSPRJPCB")));
    }

    #[test]
    fn a_project_file_lists_its_documents_and_leaves_out_the_outputs() {
        let p = Project::from_file(&data("TestFixtureISSI.PrjPcb")).unwrap();
        assert_eq!(p.name, "TestFixtureISSI");
        let names: Vec<String> = p.files.iter().map(|f| f.path.file_name().unwrap().to_string_lossy().to_string()).collect();
        // Schematics first, then boards, then libraries.
        assert_eq!(names, ["Sheet1.SchDoc", "TestFixtureISSIPCB.CSPcbDoc", "fixturelib.PcbLib", "fixturelib.SchLib"], "{names:?}");
        assert!(p.files.iter().all(|f| !f.missing), "the test files are all there");
        assert_eq!(p.of_kind(FileKind::Library).count(), 2);
    }

    #[test]
    fn a_file_belongs_to_the_project_that_lists_it_else_to_its_folder() {
        // Listed by the project file in the same folder.
        let p = Project::around(&data("Sheet1.SchDoc"));
        assert_eq!(p.name, "TestFixtureISSI");
        assert!(p.file.is_some() && p.contains(&data("fixturelib.SchLib")));
        // Not listed (Sheet2 isn't in the project): the folder, which has everything.
        let p = Project::around(&data("Sheet2.SchDoc"));
        assert_eq!(p.name, "altium");
        assert!(p.file.is_none() && p.contains(&data("Sheet2.SchDoc")) && p.contains(&data("DAL_LIB.PcbLib")));
        assert!(p.files.len() >= 8);
    }

    #[test]
    fn a_missing_document_is_flagged_not_dropped() {
        let dir = std::env::temp_dir().join(format!("coppersmith-project-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("p.PrjPcb"), "[Document1]\r\nDocumentPath=gone.SchDoc\r\n[Document2]\r\nDocumentPath=sub\\here.PcbDoc\r\n").unwrap();
        let p = Project::from_file(&dir.join("p.PrjPcb")).unwrap();
        assert_eq!(p.files.len(), 2);
        assert!(p.files.iter().all(|f| f.missing));
        assert_eq!(p.files[1].path, dir.join("sub/here.PcbDoc"), "backslashes become folders");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
