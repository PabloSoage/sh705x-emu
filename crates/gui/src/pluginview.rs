//! The plugins window: the ones running, the ones installed, and the sources they come
//! from (a repository of your own, with its token, as in opendash). Fetching runs on its
//! own thread, so a slow source does not stop the window.

use crate::emu::Cmd;
use eframe::egui::{self, Color32, RichText};
use plugins::{Entry, Kind, Library, Source, Status};
use std::sync::mpsc::{Receiver, Sender, channel};

/// What a fetch on its thread comes back with.
enum Fetched {
    /// A source's index (the source's position).
    Index(usize, Result<Vec<Entry>, String>),
    /// A plugin installed from a source, or why not.
    Installed(String, Result<String, String>),
}

/// The window's state.
pub struct PluginView {
    lib: Option<Library>,
    sources: Vec<Source>,
    /// Per source, its index once listed.
    listed: Vec<Option<Result<Vec<Entry>, String>>>,
    form: Source,
    note: Option<String>,
    busy: usize,
    tx: Sender<Fetched>,
    rx: Receiver<Fetched>,
}

impl Default for PluginView {
    fn default() -> Self {
        let lib = Library::default_dir().map(Library::new);
        let sources = lib.as_ref().map(Library::sources).unwrap_or_default();
        let (tx, rx) = channel();
        Self {
            listed: vec![None; sources.len()],
            lib,
            sources,
            form: blank(),
            note: None,
            busy: 0,
            tx,
            rx,
        }
    }
}

fn blank() -> Source {
    Source {
        name: String::new(),
        kind: Kind::GitHub,
        location: String::new(),
        reference: "main".into(),
        token: String::new(),
    }
}

/// The plugins in the library loaded at power-on: (module, file) each, and what failed.
pub fn enabled() -> (Vec<(Vec<u8>, String)>, Vec<String>) {
    let Some(lib) = Library::default_dir().map(Library::new) else {
        return (Vec::new(), Vec::new());
    };
    let (mut ok, mut bad) = (Vec::new(), Vec::new());
    for f in lib.enabled() {
        match lib.read(&f) {
            Ok(w) => ok.push((w, f)),
            Err(e) => bad.push(e),
        }
    }
    (ok, bad)
}

impl PluginView {
    /// A window on the library `lib` rather than the user's own (tests, screenshots).
    #[cfg(test)]
    pub fn with_library(lib: Library) -> Self {
        let sources = lib.sources();
        Self {
            listed: vec![None; sources.len()],
            lib: Some(lib),
            sources,
            ..Self::default()
        }
    }

    fn save_sources(&mut self) {
        if let Some(lib) = &self.lib
            && let Err(e) = lib.save_sources(&self.sources)
        {
            self.note = Some(e);
        }
    }

    /// Takes what the fetching threads have finished.
    fn poll(&mut self) {
        while let Ok(f) = self.rx.try_recv() {
            self.busy = self.busy.saturating_sub(1);
            match f {
                Fetched::Index(i, r) => {
                    if let Some(slot) = self.listed.get_mut(i) {
                        *slot = Some(r);
                    }
                }
                Fetched::Installed(file, r) => {
                    self.note = Some(match r {
                        Ok(v) => format!("{file}: installed ({v})"),
                        Err(e) => format!("{file}: {e}"),
                    });
                }
            }
        }
    }
}

/// Draws the window. `cmds` takes the loads and unloads.
pub fn show(
    ui: &mut egui::Ui,
    running: &[Status],
    load_note: Option<&str>,
    v: &mut PluginView,
    cmds: &Sender<Cmd>,
) {
    v.poll();
    if v.busy > 0 {
        ui.ctx().request_repaint();
    }
    ui.label(
        "A plugin is one .wasm file that runs in a sandbox beside the ECU: it hears the \
         frames the ECU sends on its CAN bus and can send its own, reads the memory its \
         manifest names and can write RAM.",
    );
    ui.separator();
    ui.heading("Running");
    if running.is_empty() {
        ui.label(RichText::new("None.").weak());
    }
    for (i, st) in running.iter().enumerate() {
        ui.horizontal_wrapped(|ui| {
            ui.label(
                RichText::new(format!("{} {}", st.manifest.name, st.manifest.version)).strong(),
            );
            ui.label(
                RichText::new(format!(
                    "({}) · heard {}, sent {}",
                    st.file, st.heard, st.sent
                ))
                .weak(),
            );
            if ui.button("Unload").clicked() {
                let _ = cmds.send(Cmd::UnloadPlugin(i));
            }
        });
        if let Some(e) = &st.error {
            ui.colored_label(Color32::from_rgb(240, 110, 90), e);
        }
        if !st.log.is_empty() {
            egui::ScrollArea::vertical()
                .id_salt(("plugin_log", i))
                .max_height(90.0)
                .stick_to_bottom(true)
                .show(ui, |ui| {
                    for l in &st.log {
                        ui.monospace(l);
                    }
                });
        }
    }
    if let Some(n) = load_note {
        ui.label(RichText::new(n).weak());
    }
    ui.separator();
    let Some(lib) = v.lib.clone() else {
        ui.heading("Installed");
        ui.label("No place to keep plugins: APPDATA is not set.");
        return;
    };
    ui.heading("Installed").on_hover_text(format!(
        "In {}: plugins\\ holds them, enabled.txt the ones loaded at power-on, sources.json \
         the sources and their tokens",
        lib.dir.display()
    ));
    let enabled = lib.enabled();
    for (file, m) in lib.installed() {
        ui.horizontal_wrapped(|ui| match &m {
            Ok(m) => {
                ui.label(RichText::new(format!("{} {}", m.name, m.version)).strong());
                ui.label(RichText::new(&file).weak());
                if ui.button("Load now").clicked() {
                    match lib.read(&file) {
                        Ok(w) => {
                            let _ = cmds.send(Cmd::LoadPlugin(w, file.clone()));
                        }
                        Err(e) => v.note = Some(e),
                    }
                }
                let mut on = enabled.contains(&file);
                if ui
                    .checkbox(&mut on, "At power-on")
                    .on_hover_text("Load it whenever an ECU is powered on")
                    .changed()
                    && let Err(e) = lib.set_enabled(&file, on)
                {
                    v.note = Some(e);
                }
                if ui.button("Remove").clicked()
                    && let Err(e) = lib.remove(&file)
                {
                    v.note = Some(e);
                }
                if !m.description.is_empty() {
                    ui.label(&m.description);
                }
            }
            Err(e) => {
                ui.label(&file);
                ui.colored_label(Color32::from_rgb(240, 110, 90), e);
                if ui.button("Remove").clicked() {
                    let _ = lib.remove(&file);
                }
            }
        });
    }
    if ui.button("Install from a file…").clicked()
        && let Some(path) = rfd::FileDialog::new()
            .set_title("A plugin")
            .add_filter("WebAssembly", &["wasm"])
            .pick_file()
    {
        let file = path
            .file_name()
            .map(|f| f.to_string_lossy().into_owned())
            .unwrap_or_default();
        v.note = Some(
            match std::fs::read(&path)
                .map_err(|e| e.to_string())
                .and_then(|w| lib.add(&file, &w))
            {
                Ok(m) => format!("{file}: installed ({} {})", m.name, m.version),
                Err(e) => format!("{file}: {e}"),
            },
        );
    }
    if let Some(n) = &v.note {
        ui.label(RichText::new(n).weak());
    }
    ui.separator();
    ui.heading("Sources");
    ui.label(
        RichText::new(
            "A repository or web directory with a plugins.txt index (file, name, version, \
             description, tab-separated) and the files beside it. Each keeps its own token.",
        )
        .weak(),
    );
    let mut drop = None;
    for i in 0..v.sources.len() {
        let s = v.sources[i].clone();
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(&s.name).strong());
            let at = if matches!(s.kind, Kind::GitHub | Kind::GitLab | Kind::Gitea) {
                format!("{} {} @ {}", s.kind.name(), s.location, s.reference)
            } else {
                format!("{} {}", s.kind.name(), s.location)
            };
            ui.label(RichText::new(at).weak());
            ui.label(if s.token.is_empty() {
                "public"
            } else {
                "with a token"
            });
            if ui.button("List").clicked() {
                v.busy += 1;
                let tx = v.tx.clone();
                std::thread::spawn(move || {
                    let _ = tx.send(Fetched::Index(i, s.index()));
                });
            }
            if ui.button("Remove").clicked() {
                drop = Some(i);
            }
        });
        match v.listed.get(i).cloned().flatten() {
            Some(Ok(entries)) => {
                if entries.is_empty() {
                    ui.label(RichText::new("    Its index lists nothing.").weak());
                }
                for e in entries {
                    ui.horizontal_wrapped(|ui| {
                        ui.add_space(16.0);
                        ui.label(format!("{} {}", e.name, e.version));
                        ui.label(RichText::new(&e.file).weak());
                        if ui.button("Install").clicked() {
                            v.busy += 1;
                            let (tx, src, lib, file) = (
                                v.tx.clone(),
                                v.sources[i].clone(),
                                lib.clone(),
                                e.file.clone(),
                            );
                            std::thread::spawn(move || {
                                let r = lib
                                    .install(&src, &file)
                                    .map(|m| format!("{} {}", m.name, m.version));
                                let _ = tx.send(Fetched::Installed(file, r));
                            });
                        }
                        if !e.description.is_empty() {
                            ui.label(&e.description);
                        }
                    });
                }
            }
            Some(Err(e)) => {
                ui.colored_label(Color32::from_rgb(240, 110, 90), format!("    {e}"));
            }
            None => {}
        }
    }
    if let Some(i) = drop {
        v.sources.remove(i);
        v.listed.remove(i);
        v.save_sources();
    }
    if v.busy > 0 {
        ui.label(RichText::new("Fetching…").weak());
    }
    ui.add_space(6.0);
    egui::CollapsingHeader::new("Add a source")
        .default_open(v.sources.is_empty())
        .show(ui, |ui| {
            egui::Grid::new("new_source").num_columns(2).show(ui, |ui| {
                let f = &mut v.form;
                ui.label("Name");
                ui.text_edit_singleline(&mut f.name);
                ui.end_row();
                ui.label("Kind");
                egui::ComboBox::from_id_salt("source_kind")
                    .selected_text(f.kind.name())
                    .show_ui(ui, |ui| {
                        for k in Kind::ALL {
                            ui.selectable_value(&mut f.kind, k, k.name());
                        }
                    });
                ui.end_row();
                ui.label("Location");
                ui.add(
                    egui::TextEdit::singleline(&mut f.location).hint_text(match f.kind {
                        Kind::GitHub | Kind::GitLab => "owner/repository",
                        Kind::Gitea => "https://host/owner/repository",
                        Kind::Https => "https://host/directory",
                    }),
                );
                ui.end_row();
                if f.kind != Kind::Https {
                    ui.label("Branch or tag");
                    ui.text_edit_singleline(&mut f.reference);
                    ui.end_row();
                }
                ui.label("Token");
                ui.add(
                    egui::TextEdit::singleline(&mut f.token)
                        .password(true)
                        .hint_text("empty for a public source"),
                )
                .on_hover_text(
                    "Scope it to read this one repository. It is kept in sources.json, in \
                     your own profile.",
                );
                ui.end_row();
            });
            let f = &v.form;
            let ready = !f.name.trim().is_empty() && !f.location.trim().is_empty();
            if ui.add_enabled(ready, egui::Button::new("Add")).clicked() {
                let mut s = v.form.clone();
                s.name = s.name.trim().to_string();
                s.location = s.location.trim().to_string();
                s.token = s.token.trim().to_string();
                v.sources.push(s);
                v.listed.push(None);
                v.form = blank();
                v.save_sources();
            }
        });
}
