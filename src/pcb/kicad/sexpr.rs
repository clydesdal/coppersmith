//! KiCad's files are S-expressions: `(name arg arg (child …) …)`, with strings in
//! double quotes. This reads one into a tree and offers the lookups the importers need.

#[derive(Clone, Debug, PartialEq)]
pub enum Sx {
    /// A bare word or number.
    Atom(String),
    /// A quoted string.
    Str(String),
    List(Vec<Sx>),
}

/// Reads the first expression in the text (a file is one list).
pub fn parse(text: &str) -> Result<Sx, String> {
    let b = text.as_bytes();
    let mut i = 0;
    // A stack of the lists being built, so a deeply nested file can't overflow the call stack.
    let mut stack: Vec<Vec<Sx>> = Vec::new();
    let mut root: Option<Sx> = None;
    while i < b.len() {
        match b[i] {
            b' ' | b'\t' | b'\r' | b'\n' => i += 1,
            b'(' => {
                stack.push(Vec::new());
                i += 1;
            }
            b')' => {
                let done = stack.pop().ok_or("an unmatched )")?;
                let node = Sx::List(done);
                match stack.last_mut() {
                    Some(parent) => parent.push(node),
                    None => {
                        root = Some(node);
                        break;
                    }
                }
                i += 1;
            }
            b'"' => {
                i += 1;
                let mut s: Vec<u8> = Vec::new();
                loop {
                    let c = *b.get(i).ok_or("a string that never ends")?;
                    i += 1;
                    match c {
                        b'"' => break,
                        b'\\' => {
                            let e = *b.get(i).ok_or("a string that never ends")?;
                            i += 1;
                            s.push(match e {
                                b'n' => b'\n',
                                b't' => b'\t',
                                other => other,
                            });
                        }
                        other => s.push(other),
                    }
                }
                let s = String::from_utf8_lossy(&s).to_string();
                match stack.last_mut() {
                    Some(parent) => parent.push(Sx::Str(s)),
                    None => return Err("text outside a list".into()),
                }
            }
            _ => {
                let start = i;
                while i < b.len() && !matches!(b[i], b' ' | b'\t' | b'\r' | b'\n' | b'(' | b')' | b'"') {
                    i += 1;
                }
                let atom = String::from_utf8_lossy(&b[start..i]).to_string();
                match stack.last_mut() {
                    Some(parent) => parent.push(Sx::Atom(atom)),
                    None => return Err("text outside a list".into()),
                }
            }
        }
    }
    if !stack.is_empty() {
        return Err("a ( that is never closed".into());
    }
    root.ok_or_else(|| "nothing to read".to_string())
}

impl Sx {
    /// The first word of a list: `footprint` of `(footprint "R" …)`.
    pub fn head(&self) -> Option<&str> {
        match self {
            Sx::List(v) => match v.first() {
                Some(Sx::Atom(a)) => Some(a),
                _ => None,
            },
            _ => None,
        }
    }

    /// What follows the head.
    pub fn args(&self) -> &[Sx] {
        match self {
            Sx::List(v) if !v.is_empty() => &v[1..],
            _ => &[],
        }
    }

    /// The text of a word or a string.
    pub fn text(&self) -> Option<&str> {
        match self {
            Sx::Atom(s) | Sx::Str(s) => Some(s),
            Sx::List(_) => None,
        }
    }

    pub fn num(&self) -> Option<f64> {
        self.text().and_then(|t| t.parse().ok())
    }

    /// The first child list with this head.
    pub fn get(&self, name: &str) -> Option<&Sx> {
        self.args().iter().find(|c| c.head() == Some(name))
    }

    /// Every child list with this head.
    pub fn all<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Sx> + 'a {
        self.args().iter().filter(move |c| c.head() == Some(name))
    }

    pub fn str_arg(&self, i: usize) -> Option<&str> {
        self.args().get(i).and_then(|a| a.text())
    }

    pub fn num_arg(&self, i: usize) -> Option<f64> {
        self.args().get(i).and_then(|a| a.num())
    }

    /// `(name 1.5)` → 1.5, looking in the children.
    pub fn child_num(&self, name: &str) -> Option<f64> {
        self.get(name).and_then(|c| c.num_arg(0))
    }

    /// `(name "x")` or `(name x)` → "x".
    pub fn child_str(&self, name: &str) -> Option<&str> {
        self.get(name).and_then(|c| c.str_arg(0))
    }

    /// Whether a flag is on: `(name yes)`, `(name)` or a bare `name` among the children.
    pub fn flag(&self, name: &str) -> bool {
        if self.args().iter().any(|a| matches!(a, Sx::Atom(s) if s == name)) {
            return true;
        }
        match self.get(name) {
            Some(c) => !matches!(c.str_arg(0), Some("no") | Some("false")),
            None => false,
        }
    }

    /// `(xy x y)` points of a `(pts …)` child.
    pub fn points(&self) -> Vec<(f64, f64)> {
        self.get("pts")
            .map(|p| p.all("xy").filter_map(|xy| Some((xy.num_arg(0)?, xy.num_arg(1)?))).collect())
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tree_is_read_with_strings_numbers_and_flags() {
        let t = parse("(kicad_pcb (version 20240108) (net 1 \"GND \\\"a\\\"\") (locked) (pts (xy 1 2) (xy -3.5 4)) hide)").unwrap();
        assert_eq!(t.head(), Some("kicad_pcb"));
        assert_eq!(t.child_num("version"), Some(20240108.0));
        assert_eq!(t.get("net").unwrap().str_arg(1), Some("GND \"a\""));
        assert!(t.flag("locked") && t.flag("hide") && !t.flag("absent"));
        assert_eq!(t.points(), vec![(1.0, 2.0), (-3.5, 4.0)]);
        assert_eq!(t.all("net").count(), 1);
    }

    #[test]
    fn a_flag_set_to_no_is_off() {
        let t = parse("(zone (locked no) (hide yes))").unwrap();
        assert!(!t.flag("locked") && t.flag("hide"));
    }

    #[test]
    fn broken_text_is_an_error_not_a_panic() {
        assert!(parse("(a (b)").is_err());
        assert!(parse(")").is_err());
        assert!(parse("(a \"never ends)").is_err());
        assert!(parse("").is_err());
        assert!(parse("word").is_err());
    }

    #[test]
    fn deeply_nested_text_does_not_overflow() {
        // Deep enough for any real file; the parser itself doesn't recurse.
        let n = 1_500;
        let text = format!("{}x{}", "(a ".repeat(n), ")".repeat(n));
        assert!(parse(&text).is_ok());
    }
}
