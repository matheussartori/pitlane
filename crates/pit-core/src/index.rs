//! The repository index: every tracked source file's symbols (with lines and a one-line signature)
//! and imports, parsed with tree-sitter. Kept in `~/.pitlane/index/`, never in the repository, and
//! refreshed incrementally: a file is parsed again only when its git blob changes.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tree_sitter::{Language, Node, Parser};

use crate::Repo;

/// Bump when what is extracted changes: every file is parsed again.
const VERSION: u32 = 1;
/// Files larger than this are skipped (bundles, fixtures, generated code).
const MAX_BYTES: u64 = 400_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Lang {
    Ts,
    Tsx,
    Js,
    Py,
    Rust,
}

impl Lang {
    pub fn of(path: &str) -> Option<Self> {
        let ext = path.rsplit_once('.')?.1;
        Some(match ext {
            "ts" | "mts" | "cts" => Self::Ts,
            "tsx" => Self::Tsx,
            "js" | "mjs" | "cjs" | "jsx" => Self::Js,
            "py" => Self::Py,
            "rs" => Self::Rust,
            _ => return None,
        })
    }

    fn language(self) -> Language {
        match self {
            Self::Ts => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            Self::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
            Self::Js => tree_sitter_javascript::LANGUAGE.into(),
            Self::Py => tree_sitter_python::LANGUAGE.into(),
            Self::Rust => tree_sitter_rust::LANGUAGE.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Symbol {
    pub name: String,
    /// fn, method, class, interface, type, enum, const, struct, trait, impl, mod.
    pub kind: String,
    /// 1-based, inclusive.
    pub line: u32,
    pub end: u32,
    /// The declaration's first line, trimmed.
    pub sig: String,
    #[serde(default)]
    pub exported: bool,
    /// The class, impl or trait it belongs to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileEntry {
    pub path: String,
    pub blob: String,
    pub lang: Lang,
    pub lines: u32,
    pub symbols: Vec<Symbol>,
    /// Import specifiers as written (`./util`, `react`, `crate::git`, `os.path`).
    pub imports: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Index {
    pub version: u32,
    pub root: PathBuf,
    pub head: String,
    pub files: Vec<FileEntry>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct IndexStats {
    pub files: usize,
    pub symbols: usize,
    pub parsed: usize,
    pub reused: usize,
    pub skipped: usize,
    pub ms: u64,
}

/// Where a repository's index lives.
pub fn path_for(pitlane_home: &Path, root: &Path) -> PathBuf {
    let key: String = root.to_string_lossy().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    pitlane_home.join("index").join(format!("{}.json", key.trim_matches('-')))
}

pub fn load(pitlane_home: &Path, root: &Path) -> Option<Index> {
    let text = std::fs::read_to_string(path_for(pitlane_home, root)).ok()?;
    serde_json::from_str::<Index>(&text).ok().filter(|i| i.version == VERSION)
}

/// Brings the index up to date with the main worktree's tracked files and returns it.
pub fn update(repo: &Repo, pitlane_home: &Path) -> Result<(Index, IndexStats)> {
    let t = Instant::now();
    let old = load(pitlane_home, &repo.root).unwrap_or_default();
    let mut by_path: HashMap<String, FileEntry> = old.files.into_iter().map(|f| (f.path.clone(), f)).collect();
    let head = repo.git.run(&["rev-parse", "HEAD"]).unwrap_or_default();

    // `ls-files -s`: mode, blob, stage, path. The worktree may differ from the index; edited files
    // are hashed again below so the index follows what is on disk.
    let modified: std::collections::HashSet<String> = repo.git.lines(&["ls-files", "-m"])?.into_iter().collect();
    let mut todo = Vec::new();
    let mut files = Vec::new();
    let mut stats = IndexStats::default();
    for l in repo.git.lines(&["ls-files", "-s"])? {
        let Some((meta, path)) = l.split_once('\t') else { continue };
        let blob = meta.split_whitespace().nth(1).unwrap_or_default().to_owned();
        let Some(lang) = Lang::of(path) else { continue };
        if path.ends_with(".d.ts") || path.contains("node_modules/") || path.contains(".min.") {
            continue;
        }
        let blob = if modified.contains(path) { format!("{blob}+{}", mtime(&repo.root.join(path))) } else { blob };
        match by_path.remove(path) {
            Some(f) if f.blob == blob => {
                stats.reused += 1;
                files.push(f);
            }
            _ => todo.push((path.to_owned(), blob, lang)),
        }
    }

    // Parse in parallel: one parser per thread.
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).min(12);
    let chunk = todo.len().div_ceil(threads).max(1);
    let root = repo.root.clone();
    let parsed: Vec<Option<FileEntry>> = std::thread::scope(|s| {
        let handles: Vec<_> = todo
            .chunks(chunk)
            .map(|part| {
                let root = &root;
                s.spawn(move || {
                    let mut parser = Parser::new();
                    part.iter().map(|(path, blob, lang)| parse_file(&mut parser, root, path, blob, *lang)).collect::<Vec<_>>()
                })
            })
            .collect();
        handles.into_iter().flat_map(|h| h.join().unwrap_or_default()).collect()
    });
    for p in parsed {
        match p {
            Some(f) => {
                stats.parsed += 1;
                files.push(f);
            }
            None => stats.skipped += 1,
        }
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    stats.files = files.len();
    stats.symbols = files.iter().map(|f| f.symbols.len()).sum();
    let index = Index { version: VERSION, root: repo.root.clone(), head, files };
    let path = path_for(pitlane_home, &repo.root);
    crate::fsutil::write_atomic(&path, serde_json::to_vec(&index)?).context("could not save the index")?;
    stats.ms = t.elapsed().as_millis() as u64;
    Ok((index, stats))
}

fn mtime(p: &Path) -> u64 {
    std::fs::metadata(p).and_then(|m| m.modified()).ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs())
}

fn parse_file(parser: &mut Parser, root: &Path, path: &str, blob: &str, lang: Lang) -> Option<FileEntry> {
    let full = root.join(path);
    if std::fs::metadata(&full).ok()?.len() > MAX_BYTES {
        return None;
    }
    let text = std::fs::read_to_string(&full).ok()?;
    let lines = text.lines().count() as u32;
    // Minified or generated: very long average lines.
    if lines > 0 && text.len() as u32 / lines > 300 {
        return None;
    }
    let (symbols, imports) = extract(parser, &text, lang)?;
    Some(FileEntry { path: path.to_owned(), blob: blob.to_owned(), lang, lines, symbols, imports })
}

/// Symbols and imports of one source text.
pub fn extract(parser: &mut Parser, text: &str, lang: Lang) -> Option<(Vec<Symbol>, Vec<String>)> {
    parser.set_language(&lang.language()).ok()?;
    let tree = parser.parse(text, None)?;
    let mut x = Extractor { src: text.as_bytes(), lang, symbols: Vec::new(), imports: Vec::new() };
    x.walk(tree.root_node(), None, false, 0);
    Some((x.symbols, x.imports))
}

struct Extractor<'a> {
    src: &'a [u8],
    lang: Lang,
    symbols: Vec<Symbol>,
    imports: Vec<String>,
}

impl Extractor<'_> {
    fn text(&self, n: Node) -> &str {
        n.utf8_text(self.src).unwrap_or_default()
    }

    fn field(&self, n: Node, f: &str) -> Option<String> {
        n.child_by_field_name(f).map(|c| self.text(c).to_owned())
    }

    fn push(&mut self, n: Node, name: String, kind: &str, exported: bool, parent: Option<&str>) {
        let first = self.text(n).lines().next().unwrap_or_default().trim();
        let sig: String = first.trim_end_matches('{').trim_end().chars().take(160).collect();
        self.symbols.push(Symbol {
            name,
            kind: kind.to_owned(),
            line: n.start_position().row as u32 + 1,
            end: n.end_position().row as u32 + 1,
            sig,
            exported,
            parent: parent.map(str::to_owned),
        });
    }

    fn walk(&mut self, n: Node, parent: Option<&str>, exported: bool, depth: u32) {
        if depth > 6 {
            return;
        }
        let mut cur = n.walk();
        let kids: Vec<Node> = n.named_children(&mut cur).collect();
        for c in kids {
            match self.lang {
                Lang::Ts | Lang::Tsx | Lang::Js => self.js(c, parent, exported, depth),
                Lang::Py => self.py(c, parent, depth),
                Lang::Rust => self.rust(c, parent, depth),
            }
        }
    }

    fn js(&mut self, c: Node, parent: Option<&str>, exported: bool, depth: u32) {
        match c.kind() {
            "export_statement" => {
                if let Some(src) = c.child_by_field_name("source") {
                    self.imports.push(self.text(src).trim_matches(['"', '\'', '`']).to_owned());
                }
                self.walk(c, parent, true, depth + 1);
            }
            "import_statement" => {
                if let Some(src) = c.child_by_field_name("source") {
                    self.imports.push(self.text(src).trim_matches(['"', '\'', '`']).to_owned());
                }
            }
            "function_declaration" | "generator_function_declaration" => {
                if let Some(name) = self.field(c, "name") {
                    self.push(c, name, "fn", exported, parent);
                }
            }
            "class_declaration" | "abstract_class_declaration" => {
                if let Some(name) = self.field(c, "name") {
                    self.push(c, name.clone(), "class", exported, parent);
                    if let Some(body) = c.child_by_field_name("body") {
                        self.walk(body, Some(&name), false, depth + 1);
                    }
                }
            }
            "method_definition" | "abstract_method_signature" => {
                if let Some(name) = self.field(c, "name") {
                    self.push(c, name, "method", false, parent);
                }
            }
            "interface_declaration" => {
                if let Some(name) = self.field(c, "name") {
                    self.push(c, name, "interface", exported, parent);
                }
            }
            "type_alias_declaration" => {
                if let Some(name) = self.field(c, "name") {
                    self.push(c, name, "type", exported, parent);
                }
            }
            "enum_declaration" => {
                if let Some(name) = self.field(c, "name") {
                    self.push(c, name, "enum", exported, parent);
                }
            }
            "internal_module" | "module" => {
                if let Some(name) = self.field(c, "name") {
                    self.push(c, name.clone(), "mod", exported, parent);
                    if let Some(body) = c.child_by_field_name("body") {
                        self.walk(body, Some(&name), exported, depth + 1);
                    }
                }
            }
            // Top-level `const x = ...`, functions as arrows included.
            "lexical_declaration" | "variable_declaration" if parent.is_none() => {
                let mut cur = c.walk();
                let decls: Vec<Node> = c.named_children(&mut cur).filter(|d| d.kind() == "variable_declarator").collect();
                for d in decls {
                    let Some(name) = d.child_by_field_name("name").filter(|n| n.kind() == "identifier").map(|n| self.text(n).to_owned()) else { continue };
                    let is_fn = d.child_by_field_name("value").is_some_and(|v| matches!(v.kind(), "arrow_function" | "function_expression" | "function"));
                    let kind = if is_fn { "fn" } else { "const" };
                    self.push(c, name, kind, exported, parent);
                }
            }
            _ => {}
        }
    }

    fn py(&mut self, c: Node, parent: Option<&str>, depth: u32) {
        match c.kind() {
            "import_statement" | "import_from_statement" => {
                if let Some(m) = self.field(c, "module_name") {
                    self.imports.push(m);
                } else if let Some(m) = self.field(c, "name") {
                    self.imports.push(m);
                }
            }
            "decorated_definition" => {
                if let Some(d) = c.child_by_field_name("definition") {
                    self.py(d, parent, depth);
                }
            }
            "function_definition" => {
                if let Some(name) = self.field(c, "name") {
                    let exported = !name.starts_with('_');
                    self.push(c, name, if parent.is_some() { "method" } else { "fn" }, exported, parent);
                }
            }
            "class_definition" => {
                if let Some(name) = self.field(c, "name") {
                    self.push(c, name.clone(), "class", !name.starts_with('_'), parent);
                    if let Some(body) = c.child_by_field_name("body") {
                        self.walk(body, Some(&name), false, depth + 1);
                    }
                }
            }
            _ => {}
        }
    }

    fn rust(&mut self, c: Node, parent: Option<&str>, depth: u32) {
        let public = || {
            let mut cur = c.walk();
            c.children(&mut cur).any(|k| k.kind() == "visibility_modifier")
        };
        let kind = match c.kind() {
            "use_declaration" => {
                if let Some(a) = c.child_by_field_name("argument") {
                    self.imports.push(self.text(a).to_owned());
                }
                return;
            }
            "function_item" | "function_signature_item" => {
                if parent.is_some() {
                    "method"
                } else {
                    "fn"
                }
            }
            "struct_item" => "struct",
            "enum_item" => "enum",
            "trait_item" => "trait",
            "type_item" => "type",
            "const_item" | "static_item" => "const",
            "macro_definition" => "macro",
            "mod_item" => "mod",
            "impl_item" => {
                let ty = self.field(c, "type").unwrap_or_default();
                let name = match self.field(c, "trait") {
                    Some(t) => format!("{t} for {ty}"),
                    None => ty.clone(),
                };
                self.push(c, name, "impl", false, parent);
                if let Some(body) = c.child_by_field_name("body") {
                    self.walk(body, Some(&ty), false, depth + 1);
                }
                return;
            }
            _ => return,
        };
        let Some(name) = self.field(c, "name") else { return };
        let exported = public();
        self.push(c, name.clone(), kind, exported, parent);
        if matches!(kind, "trait" | "mod")
            && let Some(body) = c.child_by_field_name("body")
        {
            // Tests stay out of the map: they are found by path, not by symbol.
            if !(kind == "mod" && name == "tests") {
                self.walk(body, Some(&name), false, depth + 1);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(text: &str, lang: Lang) -> (Vec<String>, Vec<String>) {
        let (s, i) = extract(&mut Parser::new(), text, lang).unwrap();
        (s.iter().map(|s| format!("{}:{}:{}{}", s.kind, s.name, s.line, if s.exported { "+" } else { "" })).collect(), i)
    }

    #[test]
    fn typescript() {
        let src = "import { a } from './util';\nexport { b } from \"./b\";\n\nexport function quota(x: number): string {\n  return '';\n}\nexport const limit = (n: number) => n;\nconst MAX = 3;\nexport interface Lane { branch: string }\nexport type Id = string;\nexport class Kart extends Base {\n  drift(angle: number) {}\n}\nenum Tier { A }\n";
        let (s, i) = names(src, Lang::Ts);
        assert_eq!(i, ["./util", "./b"]);
        assert_eq!(s, ["fn:quota:4+", "fn:limit:7+", "const:MAX:8", "interface:Lane:9+", "type:Id:10+", "class:Kart:11+", "method:drift:12", "enum:Tier:14"]);
        let (s, _) = names("export default function App() { return <div/> }\n", Lang::Tsx);
        assert_eq!(s, ["fn:App:1+"]);
    }

    #[test]
    fn python_and_rust() {
        let (s, i) = names("import os\nfrom a.b import c\n\n@dec\ndef run(x):\n    pass\n\nclass Tool:\n    def _hidden(self):\n        pass\n", Lang::Py);
        assert_eq!(i, ["os", "a.b"]);
        assert_eq!(s, ["fn:run:5+", "class:Tool:8+", "method:_hidden:9"]);
        let rs = "use crate::git::Git;\npub struct Repo { a: u8 }\nimpl Repo {\n    pub fn open() {}\n}\nimpl Drop for Repo { fn drop(&mut self) {} }\nfn helper() {}\n#[cfg(test)]\nmod tests { fn t() {} }\n";
        let (s, i) = names(rs, Lang::Rust);
        assert_eq!(i, ["crate::git::Git"]);
        assert_eq!(s, ["struct:Repo:2+", "impl:Repo:3", "method:open:4+", "impl:Drop for Repo:6", "method:drop:6", "fn:helper:7", "mod:tests:9"]);
    }
}
