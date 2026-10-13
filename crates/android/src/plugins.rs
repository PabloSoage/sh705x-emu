//! The plugins, as the app reads them: the ones running (from the ECU's thread), the
//! library in the app's own storage, and its sources. Everything is plain text, one item
//! a line and tab-separated, as the other screens get theirs. A source's token goes in and
//! is kept, but is never handed back.

use crate::{Live, Order, lock};
use plugins::{Kind, Library, Source};

/// Fields joined by tabs, with tabs and line ends in them made spaces.
fn row(fields: &[&str]) -> String {
    fields
        .iter()
        .map(|f| f.replace(['\t', '\n', '\r'], " "))
        .collect::<Vec<_>>()
        .join("\t")
}

impl Live {
    /// The plugins running: per plugin a line `P index name version file heard sent
    /// error`, then a line `L text` per log line; and `N text` for what the last load did.
    pub fn plugins_text(&self) -> String {
        let (st, note) = lock(&self.plugins).clone();
        let mut out = Vec::new();
        for (i, s) in st.iter().enumerate() {
            out.push(row(&[
                "P",
                &i.to_string(),
                &s.manifest.name,
                &s.manifest.version,
                &s.file,
                &s.heard.to_string(),
                &s.sent.to_string(),
                s.error.as_deref().unwrap_or(""),
            ]));
            for l in s.log.iter().rev().take(20).rev() {
                out.push(row(&["L", l]));
            }
        }
        if let Some(n) = note {
            out.push(row(&["N", &n]));
        }
        out.join("\n")
    }

    /// Loads the plugins `dir`'s library loads at power-on; what could not be read.
    pub fn load_enabled(&self, dir: &str) -> String {
        let lib = Library::new(dir);
        let mut bad = Vec::new();
        for f in lib.enabled() {
            match lib.read(&f) {
                Ok(w) => self.send(Order::LoadPlugin(w, f)),
                Err(e) => bad.push(e),
            }
        }
        bad.join("\n")
    }
}

/// The plugins installed in `dir`: `file name version enabled description error` a line
/// (`enabled` 1 or 0; the manifest's fields empty and `error` set if it does not read).
pub fn installed(dir: &str) -> String {
    let lib = Library::new(dir);
    let on = lib.enabled();
    lib.installed()
        .into_iter()
        .map(|(file, m)| {
            let enabled = if on.contains(&file) { "1" } else { "0" };
            match m {
                Ok(m) => row(&[&file, &m.name, &m.version, enabled, &m.description, ""]),
                Err(e) => row(&[&file, "", "", enabled, "", &e]),
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Installs `wasm` as `file` in `dir`: what happened.
pub fn add(dir: &str, file: &str, wasm: &[u8]) -> String {
    match Library::new(dir).add(file, wasm) {
        Ok(m) => format!("{file}: installed ({} {})", m.name, m.version),
        Err(e) => format!("{file}: {e}"),
    }
}

/// Removes `file` from `dir`: empty, or what went wrong.
pub fn remove(dir: &str, file: &str) -> String {
    Library::new(dir).remove(file).err().unwrap_or_default()
}

/// Loads `file` at power-on, or not: empty, or what went wrong.
pub fn set_enabled(dir: &str, file: &str, on: bool) -> String {
    Library::new(dir)
        .set_enabled(file, on)
        .err()
        .unwrap_or_default()
}

/// The module of `file` in `dir`.
pub fn read(dir: &str, file: &str) -> Result<Vec<u8>, String> {
    Library::new(dir).read(file)
}

/// The sources in `dir`: `name kind location ref token` a line, `token` 1 if it has one
/// (the token itself stays on disk).
pub fn sources(dir: &str) -> String {
    Library::new(dir)
        .sources()
        .iter()
        .map(|s| {
            row(&[
                &s.name,
                s.kind.name(),
                &s.location,
                &s.reference,
                if s.token.is_empty() { "0" } else { "1" },
            ])
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Adds a source to `dir`: empty, or what is wrong with it.
pub fn add_source(
    dir: &str,
    name: &str,
    kind: &str,
    location: &str,
    reference: &str,
    token: &str,
) -> String {
    let Some(kind) = Kind::from_name(kind) else {
        return format!("'{kind}' is not a kind of source");
    };
    if name.trim().is_empty() || location.trim().is_empty() {
        return "a source needs a name and a location".into();
    }
    let lib = Library::new(dir);
    let mut s = lib.sources();
    s.push(Source {
        name: name.trim().into(),
        kind,
        location: location.trim().into(),
        reference: if reference.trim().is_empty() {
            "main".into()
        } else {
            reference.trim().into()
        },
        token: token.trim().into(),
    });
    lib.save_sources(&s).err().unwrap_or_default()
}

/// Removes source `index` from `dir`: empty, or what went wrong.
pub fn remove_source(dir: &str, index: usize) -> String {
    let lib = Library::new(dir);
    let mut s = lib.sources();
    if index < s.len() {
        s.remove(index);
    }
    lib.save_sources(&s).err().unwrap_or_default()
}

/// Source `index`'s plugins, `file name version description` a line; or one line `!`
/// and why not. Goes to the network: not on the main thread.
pub fn index(dir: &str, index: usize) -> String {
    let Some(s) = Library::new(dir).sources().into_iter().nth(index) else {
        return "!no such source".into();
    };
    match s.index() {
        Ok(e) => e
            .iter()
            .map(|e| row(&[&e.file, &e.name, &e.version, &e.description]))
            .collect::<Vec<_>>()
            .join("\n"),
        Err(e) => format!("!{e}"),
    }
}

/// Installs `file` from source `index` into `dir`: what happened. Goes to the network.
pub fn install(dir: &str, index: usize, file: &str) -> String {
    let lib = Library::new(dir);
    let Some(s) = lib.sources().into_iter().nth(index) else {
        return "no such source".into();
    };
    match lib.install(&s, file) {
        Ok(m) => format!("{file}: installed ({} {})", m.name, m.version),
        Err(e) => format!("{file}: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sources go in with their token and come back without it.
    #[test]
    fn a_token_is_kept_and_never_handed_back() {
        let dir = std::env::temp_dir().join(format!("sh705x-app-{}", std::process::id()));
        let d = dir.to_string_lossy().into_owned();
        assert_eq!(add_source(&d, "mine", "GitHub", "me/p", "", "s3cret"), "");
        assert!(add_source(&d, "x", "FTP", "a", "", "").contains("not a kind"));
        let s = sources(&d);
        assert_eq!(s, "mine\tGitHub\tme/p\tmain\t1");
        assert!(!s.contains("s3cret"));
        assert_eq!(Library::new(&dir).sources()[0].token, "s3cret");
        assert_eq!(remove_source(&d, 0), "");
        assert_eq!(sources(&d), "");
        assert!(installed(&d).is_empty());
        assert!(add(&d, "bad.wasm", b"x").contains("not a WebAssembly"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
