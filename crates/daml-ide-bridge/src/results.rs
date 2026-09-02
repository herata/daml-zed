//! The single Markdown file the editor keeps open.
//!
//! One file rather than one per script: the editor cannot be told to open a
//! document, so every extra file is another manual open. With a single pane the
//! reader opens it once, splits it beside the source, and every subsequent
//! "Show script results" lands there.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::markdown;

pub struct Pane {
    path: PathBuf,
    root: PathBuf,
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    /// The script the pane is currently showing. Several virtual resources can
    /// re-render at once, and without this the pane would flip between them.
    current: Option<String>,
    titles: HashMap<String, String>,
}

impl Pane {
    pub fn new(root: impl Into<PathBuf>, path: Option<&str>) -> Self {
        let root = root.into();
        let path = match path {
            Some(path) => PathBuf::from(path),
            None => root.join(".daml/ide/script-results.md"),
        };
        Self {
            path,
            root,
            state: Mutex::new(State::default()),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The pane switches to this script immediately, before the server has
    /// rendered anything, so a click always produces visible feedback.
    pub fn show(&self, uri: &str, title: &str) -> io::Result<()> {
        {
            let mut state = self.state.lock().unwrap();
            state.current = Some(uri.to_string());
            state.titles.insert(uri.to_string(), title.to_string());
        }
        self.write(&format!(
            "# {title}\n\n`{}`\n\n_Running…_\n",
            self.source_of(uri)
        ))
    }

    /// Ignores renders for anything but the script the pane is showing.
    pub fn update(&self, uri: &str, html: &str) -> io::Result<bool> {
        let title = {
            let state = self.state.lock().unwrap();
            if state.current.as_deref() != Some(uri) {
                return Ok(false);
            }
            state
                .titles
                .get(uri)
                .cloned()
                .unwrap_or_else(|| "Script results".to_string())
        };
        self.write(&markdown::render(html, &title, &self.source_of(uri)))?;
        Ok(true)
    }

    /// The source file the script lives in, relative to the project when it is
    /// inside it, because an absolute path is noise in a heading.
    fn source_of(&self, uri: &str) -> String {
        let Some(file) = query_value(uri, "file") else {
            return uri.to_string();
        };
        let path = PathBuf::from(&file);
        path.strip_prefix(&self.root)
            .map(|relative| relative.display().to_string())
            .unwrap_or(file)
    }

    /// Written through a temporary file so the editor never reloads a half
    /// written document.
    fn write(&self, contents: &str) -> io::Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let temporary = self.path.with_extension("md.tmp");
        fs::write(&temporary, contents)?;
        fs::rename(&temporary, &self.path)
    }
}

/// `daml://compiler?file=%2Fa%2Fb.daml&top-level-decl=setup`
fn query_value(uri: &str, key: &str) -> Option<String> {
    let query = uri.split_once('?')?.1;
    query
        .split('&')
        .find_map(|pair| pair.strip_prefix(&format!("{key}=")))
        .map(percent_decode)
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => match u8::from_str_radix(&text[i + 1..i + 3], 16) {
                Ok(byte) => {
                    out.push(byte);
                    i += 3;
                }
                Err(_) => {
                    out.push(bytes[i]);
                    i += 1;
                }
            },
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            byte => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    const HTML: &str = include_str!("../tests/fixtures/script-result.html");

    fn pane() -> (Pane, tempdir::TempDir) {
        let dir = tempdir::TempDir::new();
        (Pane::new(dir.path(), None), dir)
    }

    /// A directory that removes itself, so the tests need no dependency.
    mod tempdir {
        use std::path::{Path, PathBuf};
        pub struct TempDir(PathBuf);
        impl TempDir {
            pub fn new() -> Self {
                let path = std::env::temp_dir().join(format!(
                    "daml-ide-bridge-test-{}-{:?}",
                    std::process::id(),
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_nanos()
                ));
                std::fs::create_dir_all(&path).unwrap();
                Self(path)
            }
            pub fn path(&self) -> &Path {
                &self.0
            }
        }
        impl Drop for TempDir {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
    }

    #[test]
    fn decodes_the_source_out_of_the_virtual_uri() {
        let pane = Pane::new("/project", None);
        assert_eq!(
            pane.source_of("daml://compiler?file=%2Fproject%2Ftest%2Fa.daml&top-level-decl=setup"),
            "test/a.daml"
        );
    }

    #[test]
    fn keeps_an_absolute_source_outside_the_project() {
        let pane = Pane::new("/project", None);
        assert_eq!(
            pane.source_of("daml://compiler?file=%2Felsewhere%2Fa.daml"),
            "/elsewhere/a.daml"
        );
    }

    #[test]
    fn defaults_to_the_projects_build_directory() {
        assert!(Pane::new("/project", None)
            .path()
            .ends_with(".daml/ide/script-results.md"));
        assert_eq!(
            Pane::new("/project", Some("/tmp/out.md")).path(),
            Path::new("/tmp/out.md")
        );
    }

    #[test]
    fn shows_a_placeholder_before_the_render_arrives() {
        let (pane, _dir) = pane();
        pane.show("daml://compiler?file=%2Fa.daml", "Script: setup")
            .unwrap();
        let written = std::fs::read_to_string(pane.path()).unwrap();
        assert!(written.contains("# Script: setup"));
        assert!(written.contains("Running"));
    }

    #[test]
    fn a_render_replaces_the_placeholder() {
        let (pane, _dir) = pane();
        let uri = "daml://compiler?file=%2Fa.daml";
        pane.show(uri, "Script: setup").unwrap();
        assert!(pane.update(uri, HTML).unwrap());
        let written = std::fs::read_to_string(pane.path()).unwrap();
        assert!(written.contains("## Transactions"));
        assert!(written.contains("| #2:1 | active |"));
        assert!(!written.contains("Running"));
    }

    #[test]
    fn a_render_for_another_script_does_not_steal_the_pane() {
        let (pane, _dir) = pane();
        pane.show("daml://compiler?file=%2Fa.daml", "Script: a")
            .unwrap();
        assert!(!pane.update("daml://compiler?file=%2Fb.daml", HTML).unwrap());
        assert!(std::fs::read_to_string(pane.path())
            .unwrap()
            .contains("Script: a"));
    }

    #[test]
    fn switching_scripts_switches_the_pane() {
        let (pane, _dir) = pane();
        let a = "daml://compiler?file=%2Fa.daml";
        let b = "daml://compiler?file=%2Fb.daml";
        pane.show(a, "Script: a").unwrap();
        pane.update(a, HTML).unwrap();
        pane.show(b, "Script: b").unwrap();
        assert!(pane.update(b, HTML).unwrap());
        assert!(std::fs::read_to_string(pane.path())
            .unwrap()
            .contains("Script: b"));
    }

    #[test]
    fn no_temporary_file_is_left_behind() {
        let (pane, _dir) = pane();
        pane.show("daml://compiler?file=%2Fa.daml", "Script: setup")
            .unwrap();
        assert!(!pane.path().with_extension("md.tmp").exists());
    }
}
