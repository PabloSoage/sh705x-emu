//! The plugins installed, and the places they come from.
//!
//! A library is a directory:
//!
//! ```text
//! plugins/       the plugins, one .wasm each
//! enabled.txt    the ones loaded at power-on, one file name a line
//! sources.json   the sources, with their tokens
//! ```
//!
//! A source is a repository or a web directory with a `plugins.txt` index at its root:
//! one plugin a line, tab-separated, file then name, version and description, `#` for
//! comments. The files sit beside it. As in opendash, each source keeps its own
//! credential, so a token that reads one cannot reach another:
//!
//! - **a forge** (GitHub, GitLab, Gitea) over HTTPS with a token scoped to one repository;
//! - **any HTTPS server**, with a bearer token if it wants one.
//!
//! Tokens are kept as written in `sources.json`, in the user's own directory.

use crate::manifest::Manifest;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Largest file fetched from a source.
pub const MAX_FETCH: u64 = 16 << 20;

/// What kind of host a source is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// `owner/repo` on github.com, through its contents API.
    GitHub,
    /// `group/project` on gitlab.com, through its files API.
    GitLab,
    /// A Gitea repository: its web address (`https://host/owner/repo`).
    Gitea,
    /// A web directory: its address.
    Https,
}

impl Kind {
    /// Every kind, in the order a front end lists them.
    pub const ALL: [Kind; 4] = [Kind::GitHub, Kind::GitLab, Kind::Gitea, Kind::Https];

    /// Its name, as kept in `sources.json` and shown.
    pub fn name(self) -> &'static str {
        match self {
            Kind::GitHub => "GitHub",
            Kind::GitLab => "GitLab",
            Kind::Gitea => "Gitea",
            Kind::Https => "HTTPS",
        }
    }

    /// The kind called `name`.
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|k| k.name().eq_ignore_ascii_case(name))
    }
}

/// Where plugins come from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Source {
    /// What the user calls it.
    pub name: String,
    /// What kind of host it is.
    pub kind: Kind,
    /// `owner/repo` for GitHub and GitLab, an address for Gitea and HTTPS.
    pub location: String,
    /// The branch or tag (forges only).
    pub reference: String,
    /// Empty for a public source.
    pub token: String,
}

/// Percent-encodes everything but unreserved characters.
fn encode(s: &str) -> String {
    s.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

impl Source {
    /// The address of the file `path` in the source.
    pub fn url_for(&self, path: &str) -> String {
        let base = self.location.trim().trim_end_matches('/');
        match self.kind {
            Kind::GitHub => format!(
                "https://api.github.com/repos/{base}/contents/{path}?ref={}",
                encode(&self.reference)
            ),
            Kind::GitLab => format!(
                "https://gitlab.com/api/v4/projects/{}/repository/files/{}/raw?ref={}",
                encode(base),
                encode(path),
                encode(&self.reference)
            ),
            Kind::Gitea => format!("{base}/raw/branch/{}/{path}", self.reference),
            Kind::Https => format!("{base}/{path}"),
        }
    }

    /// The headers the host wants.
    pub fn headers(&self) -> Vec<(&'static str, String)> {
        let t = self.token.trim();
        let mut h = Vec::new();
        match self.kind {
            Kind::GitHub => {
                h.push(("Accept", "application/vnd.github.raw".to_string()));
                h.push(("X-GitHub-Api-Version", "2022-11-28".to_string()));
                if !t.is_empty() {
                    h.push(("Authorization", format!("Bearer {t}")));
                }
            }
            Kind::GitLab if !t.is_empty() => h.push(("PRIVATE-TOKEN", t.to_string())),
            Kind::Gitea if !t.is_empty() => h.push(("Authorization", format!("token {t}"))),
            Kind::Https if !t.is_empty() => h.push(("Authorization", format!("Bearer {t}"))),
            _ => {}
        }
        h.push(("User-Agent", "sh705x-emu".to_string()));
        h
    }

    /// The file `path` of the source.
    pub fn fetch(&self, path: &str) -> Result<Vec<u8>, String> {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(30)))
            .build()
            .into();
        let url = self.url_for(path);
        let mut req = agent.get(&url);
        for (k, v) in self.headers() {
            req = req.header(k, v);
        }
        let mut resp = req.call().map_err(|e| match e {
            ureq::Error::StatusCode(401 | 403) => format!(
                "{}: refused ({e}); is the token right, and allowed to read this repository?",
                self.name
            ),
            ureq::Error::StatusCode(404) => format!(
                "{}: no {path} there (404); a private repository without a token looks the same",
                self.name
            ),
            e => format!("{}: {e}", self.name),
        })?;
        resp.body_mut()
            .with_config()
            .limit(MAX_FETCH)
            .read_to_vec()
            .map_err(|e| format!("{}: {e}", self.name))
    }

    /// The plugins the source lists.
    pub fn index(&self) -> Result<Vec<Entry>, String> {
        let raw = self.fetch("plugins.txt")?;
        Ok(read_index(&String::from_utf8_lossy(&raw)))
    }

    fn to_json(&self) -> Value {
        json!({
            "name": self.name,
            "kind": self.kind.name(),
            "location": self.location,
            "ref": self.reference,
            "token": self.token,
        })
    }

    fn from_json(v: &Value) -> Option<Self> {
        let s = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
        Some(Self {
            name: s("name")?,
            kind: Kind::from_name(&s("kind")?)?,
            location: s("location")?,
            reference: s("ref").unwrap_or_else(|| "main".into()),
            token: s("token").unwrap_or_default(),
        })
    }
}

/// One line of a source's index.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// The file, beside the index.
    pub file: String,
    /// The plugin's name.
    pub name: String,
    /// Its version.
    pub version: String,
    /// What it does.
    pub description: String,
}

/// Reads an index: `file<TAB>name<TAB>version<TAB>description`, `#` for comments. Lines
/// without a plugin file name are left out.
pub fn read_index(text: &str) -> Vec<Entry> {
    text.lines()
        .map(str::trim_end)
        .filter(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#'))
        .filter_map(|l| {
            let mut f = l.split('\t').map(str::trim);
            let file = f.next()?.to_string();
            valid_file(&file).ok()?;
            Some(Entry {
                file,
                name: f.next().unwrap_or_default().to_string(),
                version: f.next().unwrap_or_default().to_string(),
                description: f.next().unwrap_or_default().to_string(),
            })
        })
        .collect()
}

/// A plugin's file name is a plain name ending in `.wasm`: letters, digits, `.`, `_` and
/// `-`, nothing that leaves the directory.
pub fn valid_file(name: &str) -> Result<(), String> {
    let ok = name.ends_with(".wasm")
        && name.len() > 5
        && !name.starts_with('.')
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b));
    if ok {
        Ok(())
    } else {
        Err(format!(
            "'{name}' is not a plugin file name (letters, digits, . _ -, ending in .wasm)"
        ))
    }
}

/// The plugins installed in a directory, and the sources.
#[derive(Clone, Debug)]
pub struct Library {
    /// The directory.
    pub dir: PathBuf,
}

impl Library {
    /// The library in `dir` (made when first written to).
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// The usual place on this computer: `%APPDATA%\sh705x` on Windows,
    /// `$XDG_CONFIG_HOME/sh705x` or `~/.config/sh705x` elsewhere.
    pub fn default_dir() -> Option<PathBuf> {
        let var = |k: &str| {
            std::env::var_os(k)
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
        };
        if cfg!(windows) {
            var("APPDATA").map(|d| d.join("sh705x"))
        } else {
            var("XDG_CONFIG_HOME")
                .or_else(|| var("HOME").map(|h| h.join(".config")))
                .map(|d| d.join("sh705x"))
        }
    }

    fn plugins(&self) -> PathBuf {
        self.dir.join("plugins")
    }

    /// The plugins installed, by file name, with their manifests (or why not).
    pub fn installed(&self) -> Vec<(String, Result<Manifest, String>)> {
        let Ok(rd) = std::fs::read_dir(self.plugins()) else {
            return Vec::new();
        };
        let mut out: Vec<_> = rd
            .flatten()
            .filter_map(|e| {
                let file = e.file_name().to_string_lossy().into_owned();
                valid_file(&file).ok()?;
                let m = std::fs::read(e.path())
                    .map_err(|e| e.to_string())
                    .and_then(|b| Manifest::of(&b));
                Some((file, m))
            })
            .collect();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    /// The module of an installed plugin.
    pub fn read(&self, file: &str) -> Result<Vec<u8>, String> {
        valid_file(file)?;
        std::fs::read(self.plugins().join(file)).map_err(|e| format!("{file}: {e}"))
    }

    /// Installs the module `wasm` as `file`, after checking its manifest; replaces one of
    /// the same name.
    pub fn add(&self, file: &str, wasm: &[u8]) -> Result<Manifest, String> {
        valid_file(file)?;
        let m = Manifest::of(wasm)?;
        std::fs::create_dir_all(self.plugins()).map_err(|e| e.to_string())?;
        std::fs::write(self.plugins().join(file), wasm).map_err(|e| format!("{file}: {e}"))?;
        Ok(m)
    }

    /// Installs `file` from `source`.
    pub fn install(&self, source: &Source, file: &str) -> Result<Manifest, String> {
        valid_file(file)?;
        let wasm = source.fetch(file)?;
        self.add(file, &wasm)
    }

    /// Removes an installed plugin.
    pub fn remove(&self, file: &str) -> Result<(), String> {
        valid_file(file)?;
        self.set_enabled(file, false)?;
        match std::fs::remove_file(self.plugins().join(file)) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.to_string()),
            _ => Ok(()),
        }
    }

    /// The plugins loaded at power-on.
    pub fn enabled(&self) -> Vec<String> {
        std::fs::read_to_string(self.dir.join("enabled.txt"))
            .unwrap_or_default()
            .lines()
            .map(str::trim)
            .filter(|l| valid_file(l).is_ok())
            .map(str::to_string)
            .collect()
    }

    /// Loads `file` at power-on, or not.
    pub fn set_enabled(&self, file: &str, on: bool) -> Result<(), String> {
        let mut e = self.enabled();
        e.retain(|f| f != file);
        if on {
            e.push(file.to_string());
        }
        write(&self.dir, "enabled.txt", &(e.join("\n") + "\n"))
    }

    /// The sources.
    pub fn sources(&self) -> Vec<Source> {
        std::fs::read(self.dir.join("sources.json"))
            .ok()
            .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
            .and_then(|v| v.as_array().cloned())
            .unwrap_or_default()
            .iter()
            .filter_map(Source::from_json)
            .collect()
    }

    /// Keeps `sources`, replacing those before.
    pub fn save_sources(&self, sources: &[Source]) -> Result<(), String> {
        let v = Value::Array(sources.iter().map(Source::to_json).collect());
        let text = serde_json::to_string_pretty(&v).map_err(|e| e.to_string())?;
        write(&self.dir, "sources.json", &text)
    }
}

fn write(dir: &Path, name: &str, text: &str) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    std::fs::write(dir.join(name), text).map_err(|e| format!("{name}: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(kind: Kind, location: &str, token: &str) -> Source {
        Source {
            name: "s".into(),
            kind,
            location: location.into(),
            reference: "main".into(),
            token: token.into(),
        }
    }

    /// The same addresses and headers opendash uses for its catalogues.
    #[test]
    fn each_kind_of_source_is_addressed_as_its_host_wants() {
        let g = source(Kind::GitHub, "me/plugins", "t0k");
        assert_eq!(
            g.url_for("a.wasm"),
            "https://api.github.com/repos/me/plugins/contents/a.wasm?ref=main"
        );
        assert!(
            g.headers()
                .contains(&("Authorization", "Bearer t0k".into()))
        );
        assert!(
            g.headers()
                .contains(&("Accept", "application/vnd.github.raw".into()))
        );
        let l = source(Kind::GitLab, "me/plugins", "t0k");
        assert_eq!(
            l.url_for("a.wasm"),
            "https://gitlab.com/api/v4/projects/me%2Fplugins/repository/files/a.wasm/raw?ref=main"
        );
        assert!(l.headers().contains(&("PRIVATE-TOKEN", "t0k".into())));
        let t = source(Kind::Gitea, "https://git.home/me/plugins/", "t0k");
        assert_eq!(
            t.url_for("a.wasm"),
            "https://git.home/me/plugins/raw/branch/main/a.wasm"
        );
        assert!(t.headers().contains(&("Authorization", "token t0k".into())));
        let h = source(Kind::Https, "https://nas.home/p", "");
        assert_eq!(h.url_for("plugins.txt"), "https://nas.home/p/plugins.txt");
        assert!(!h.headers().iter().any(|(k, _)| *k == "Authorization"));
    }

    #[test]
    fn an_index_is_read_and_unsafe_names_left_out() {
        let e = read_index(
            "# file\tname\tversion\tdescription\n\
             node.wasm\tA node\t1.0.0\tAnswers on the bus\n\
             ../evil.wasm\tx\t1\t\n\
             \n\
             bare.wasm\n",
        );
        assert_eq!(e.len(), 2);
        assert_eq!(e[0].name, "A node");
        assert_eq!(e[1].file, "bare.wasm");
        assert!(valid_file("a/b.wasm").is_err());
        assert!(valid_file("a.exe").is_err());
    }

    /// Installs, enables, keeps sources and removes, on disk.
    #[test]
    fn a_library_keeps_its_plugins_and_sources() {
        let dir = std::env::temp_dir().join(format!("sh705x-lib-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let lib = Library::new(&dir);
        let wasm = wat::parse_str(
            r#"(module (@custom "sh705x-plugin" "{\"name\":\"n\",\"version\":\"2\",\"api\":1}"))"#,
        )
        .unwrap();
        assert_eq!(lib.add("n.wasm", &wasm).unwrap().version, "2");
        assert!(lib.add("x.wasm", b"junk").is_err());
        lib.set_enabled("n.wasm", true).unwrap();
        assert_eq!(lib.enabled(), ["n.wasm"]);
        assert_eq!(lib.installed().len(), 1);
        let s = vec![source(Kind::GitHub, "me/p", "secret")];
        lib.save_sources(&s).unwrap();
        assert_eq!(lib.sources(), s);
        lib.remove("n.wasm").unwrap();
        assert!(lib.installed().is_empty() && lib.enabled().is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
