//! The maps window: every calibration map the firmware reads, as it reads it. A list of
//! the maps looked up (how often, the last inputs and result), and the one chosen drawn
//! whole, from its own descriptor, with where the firmware is on it now and where it
//! has just been: as a heat map, a surface to turn, a table, or a curve. Its call sites
//! open in the code view, and the ECU can stop at its next lookup.

use eframe::egui::{
    self, Align2, Color32, FontId, Pos2, Rect, RichText, Sense, Shape, Stroke, Vec2,
};
use session::Watch;
use session::maps::{Found, Map, MapTrace, Seen};
use session::names::{Name, Profile, read_labels, read_profiles};
use session::sites::Source;
use std::collections::BTreeMap;

/// How the list is ordered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Sort {
    /// The last looked up first.
    Recent,
    /// The most looked up first.
    Calls,
    /// By descriptor address.
    Address,
}

/// Which page of the window is shown.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Page {
    /// The maps.
    #[default]
    Maps,
    /// The fault codes.
    Faults,
}

/// How a map is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mode {
    /// Cells coloured by value (a curve: its line).
    Heat,
    /// A surface to turn (a curve: its line).
    Surface,
    /// The numbers.
    Table,
    /// This flash's numbers against the compared file's.
    Diff,
}

/// A second flash file the maps are compared with, as calibration editors do with an
/// original and a modified file. Read from disk on request and kept only in memory.
pub(crate) struct Other {
    name: String,
    flash: Vec<u8>,
}

/// How one map reads in the compared file.
struct Diff {
    map: Map,
    /// Cells whose value differs.
    changed: usize,
    /// The largest difference, compared file minus this one, by size.
    largest: f32,
}

/// Reads `base`'s descriptor from the compared file. `None` when the map does not live
/// in the flash (its cells are in RAM) or does not read the same shape there.
fn compare(other: &Other, desc: u32, base: &Map) -> Option<Diff> {
    let flash = &other.flash;
    let in_flash = |a: u32| (a as usize) < flash.len();
    if !in_flash(base.z_at) || !in_flash(base.x_at) {
        return None;
    }
    let read = |a: u32| flash.get(a as usize).copied().unwrap_or(0);
    let map = if base.dims() == 3 {
        Map::read3d(&read, desc)
    } else {
        Map::read2d(&read, desc)
    }?;
    if map.nx() != base.nx() || map.ny() != base.ny() {
        return None;
    }
    let (mut changed, mut largest) = (0, 0.0f32);
    for (a, b) in base.z.iter().zip(&map.z) {
        let d = b - a;
        if d != 0.0 && !(a.is_nan() && b.is_nan()) {
            changed += 1;
            if d.abs() > largest.abs() {
                largest = d;
            }
        }
    }
    Some(Diff {
        map,
        changed,
        largest,
    })
}

/// What the viewer has chosen.
pub struct MapView {
    pub(crate) selected: Option<u32>,
    filter: String,
    sort: Sort,
    pub(crate) mode: Mode,
    /// The view follows the map looked up last.
    follow: bool,
    yaw: f32,
    pitch: f32,
    /// Profiles read from an opendash profile file, and the one chosen.
    profiles: Vec<Profile>,
    profile: usize,
    /// What the last file read said.
    note: Option<String>,
    /// The page shown.
    pub(crate) page: Page,
    /// The fault codes page.
    pub(crate) faults: crate::faultview::FaultView,
    /// The file compared with, if any.
    other: Option<Other>,
    /// Each map as read in the compared file, worked out once.
    diffs: BTreeMap<u32, Option<Diff>>,
    /// The cell chosen for editing: map, column, row.
    cell: Option<(u32, usize, usize)>,
    /// What is typed for it.
    cell_text: String,
    /// The cells changed in this session, by map.
    edited: BTreeMap<u32, std::collections::BTreeSet<usize>>,
}

impl MapView {
    /// The map `desc` in the compared file, worked out on first use.
    fn diff(&mut self, desc: u32, base: &Map) -> Option<&Diff> {
        let other = self.other.as_ref()?;
        self.diffs
            .entry(desc)
            .or_insert_with(|| compare(other, desc, base))
            .as_ref()
    }
}

impl Default for MapView {
    fn default() -> Self {
        Self {
            selected: None,
            filter: String::new(),
            sort: Sort::Recent,
            mode: Mode::Heat,
            follow: false,
            yaw: -0.6,
            pitch: 0.55,
            profiles: Vec::new(),
            profile: 0,
            note: None,
            page: Page::Maps,
            faults: Default::default(),
            other: None,
            diffs: BTreeMap::new(),
            cell: None,
            cell_text: String::new(),
            edited: BTreeMap::new(),
        }
    }
}

/// A map in the browser: found in the whole-ROM catalogue, looked up at run time, or
/// both. The catalogue carries the static map and whether the firmware references it; the
/// trace carries the live lookups.
#[derive(Default)]
struct Entry<'a> {
    /// From the whole-ROM scan, if the descriptor was found there.
    found: Option<&'a Found>,
    /// From the run-time trace, if the firmware looked it up.
    seen: Option<&'a Seen>,
}

impl Entry<'_> {
    /// The decoded map: the run-time one first (it reads the live RAM pointers), else the
    /// catalogue's.
    fn map(&self) -> Option<&Map> {
        self.seen
            .and_then(|s| s.map.as_ref())
            .or(self.found.map(|f| &f.map))
    }

    /// Whether the firmware reads it, when the catalogue found it.
    fn referenced(&self) -> Option<bool> {
        self.found.map(|f| f.referenced)
    }

    /// Lookups at run time (0 for a map never reached).
    fn calls(&self) -> u64 {
        self.seen.map_or(0, |s| s.calls)
    }

    /// The map as stored in the flash, for comparing with another file: the catalogue's
    /// (the run-time one may read cells the firmware copied to RAM).
    fn stored(&self) -> Option<&Map> {
        self.found.map(|f| &f.map).or(self.map())
    }
}

/// What the window asks the rest of the interface to do.
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    /// Show the code at this address.
    GoTo(u32),
    /// Stop at the next lookup of this map, or no longer.
    Break(Option<u32>),
    /// Name the variables: the sensors, and a profile's parameters if given.
    Name(Option<Profile>),
    /// Map labels to add, by descriptor.
    Labels(BTreeMap<u32, Name>),
    /// Stop at the next test of this fault code, or no longer.
    BreakTest(Option<u16>),
    /// Ask the ECU for its codes.
    ReadCodes,
    /// Descriptions of fault codes to add, by code.
    Texts(BTreeMap<String, String>),
    /// Write these bytes into the emulated ECU (flash or RAM): a changed cell.
    Edit(Vec<(u32, u8)>),
    /// Put back the loaded flash's bytes: (where, from where in the flash, how many).
    Revert(Vec<(u32, u32, u32)>),
    /// Save the flash as it is now, edits included.
    SaveFlash,
}

/// The bytes that store `v` in one of `map`'s cells, if it fits its format.
pub(crate) fn encode(map: &Map, v: f32) -> Result<Vec<u8>, String> {
    use session::maps::Cell;
    if !v.is_finite() {
        return Err("not a number".into());
    }
    let (lo, hi, bytes) = match map.cell {
        Cell::Float => return Ok(v.to_bits().to_be_bytes().to_vec()),
        Cell::U8 => (0.0, 255.0, 1),
        Cell::S8 => (-128.0, 127.0, 1),
        Cell::U16 => (0.0, 65535.0, 2),
        Cell::S16 => (-32768.0, 32767.0, 2),
    };
    if map.scale == 0.0 {
        return Err("the cells have no scale".into());
    }
    let raw = ((v - map.offset) / map.scale).round();
    if raw < lo || raw > hi {
        return Err(format!(
            "out of range: {} to {}",
            num(map.offset + lo * map.scale),
            num(map.offset + hi * map.scale)
        ));
    }
    let r = raw as i64;
    Ok(if bytes == 1 {
        vec![r as u8]
    } else {
        (r as u16).to_be_bytes().to_vec()
    })
}

/// Lookups shown as the trail of where the firmware has been on a map.
const TRAIL: usize = 64;
/// Lookups listed as the latest.
const LATEST: usize = 14;
/// Characters of a map's name shown in the list.
const ROW_NAME: usize = 14;

/// A short number: few decimals for big values.
fn num(v: f32) -> String {
    if !v.is_finite() {
        return "-".into();
    }
    let a = v.abs();
    if a >= 1000.0 {
        format!("{v:.0}")
    } else if a >= 100.0 {
        format!("{v:.1}")
    } else if a >= 1.0 {
        format!("{v:.2}")
    } else {
        format!("{v:.3}")
    }
}

/// A colour for `t` in 0..1: blue, teal, green, yellow, red.
fn heat(t: f32) -> Color32 {
    const STOPS: [(f32, [u8; 3]); 5] = [
        (0.0, [40, 60, 160]),
        (0.25, [30, 150, 170]),
        (0.5, [70, 180, 80]),
        (0.75, [230, 200, 50]),
        (1.0, [210, 60, 40]),
    ];
    let t = if t.is_finite() {
        t.clamp(0.0, 1.0)
    } else {
        0.0
    };
    let k = STOPS.iter().position(|s| s.0 >= t).unwrap_or(4).max(1);
    let ((t0, a), (t1, b)) = (STOPS[k - 1], STOPS[k]);
    let f = (t - t0) / (t1 - t0);
    let mix = |i: usize| (a[i] as f32 + (b[i] as f32 - a[i] as f32) * f) as u8;
    Color32::from_rgb(mix(0), mix(1), mix(2))
}

/// The lowest and highest cell.
fn span(map: &Map) -> (f32, f32) {
    let (lo, hi) = map
        .z
        .iter()
        .fold((f32::INFINITY, f32::NEG_INFINITY), |(l, h), &z| {
            (l.min(z), h.max(z))
        });
    if hi > lo {
        (lo, hi)
    } else {
        (lo - 1.0, hi + 1.0)
    }
}

/// Draws the window; returns what it asks for.
pub fn show(
    ui: &mut egui::Ui,
    trace: &MapTrace,
    v: &mut MapView,
    watch: Option<Watch>,
    catalog: &[Found],
) -> Vec<Action> {
    let mut actions = Vec::new();
    let map_break = match watch {
        Some(Watch::Map(d)) => Some(d),
        _ => None,
    };
    ui.horizontal(|ui| {
        ui.selectable_value(&mut v.page, Page::Maps, RichText::new("Maps").strong());
        ui.selectable_value(
            &mut v.page,
            Page::Faults,
            RichText::new("Fault codes").strong(),
        )
        .on_hover_text("The calibration's fault codes, their tests live, and the maps they read");
    });
    if v.page == Page::Faults {
        let mut go = None;
        crate::faultview::show(ui, trace, &mut v.faults, watch, &mut actions, &mut go);
        if let Some(d) = go {
            v.page = Page::Maps;
            v.selected = Some(d);
            v.follow = false;
        }
        return actions;
    }
    if v.follow
        && let Some(c) = trace.recent.back()
    {
        v.selected = Some(c.r4);
    }
    let m = trace.masters;
    // Every map in the browser: the whole-ROM catalogue, the run-time trace, or both.
    let mut entries: BTreeMap<u32, Entry> = BTreeMap::new();
    for f in catalog {
        entries.entry(f.map.desc).or_default().found = Some(f);
    }
    for (&d, s) in &trace.seen {
        entries.entry(d).or_default().seen = Some(s);
    }
    let referenced = catalog.iter().filter(|f| f.referenced).count();
    ui.horizontal_wrapped(|ui| {
        ui.label(format!(
            "{} maps found ({} referenced, {} unreferenced) · {} read · {} lookups · \
             routines: 3D {}, 2D {}",
            catalog.len(),
            referenced,
            catalog.len() - referenced,
            trace.seen.len(),
            trace.total,
            m.d3.map_or("not found".into(), |a| format!("{a:#06X}")),
            m.d2.map_or("not found".into(), |a| format!("{a:#06X}")),
        ));
        if trace.dropped > 0 {
            ui.colored_label(Color32::YELLOW, format!("{} lookups lost", trace.dropped));
        }
    });
    if entries.is_empty() {
        ui.label(
            "No maps found in this flash: it does not store its calibration the Denso way \
             this viewer knows.",
        );
        return actions;
    }
    names_row(ui, trace, v, &mut actions);
    ui.separator();
    ui.horizontal_top(|ui| {
        ui.vertical(|ui| {
            ui.set_width(360.0);
            list(ui, trace, v, &entries);
            ui.separator();
            latest(ui, trace, v);
        });
        ui.separator();
        ui.vertical(
            |ui| match v.selected.and_then(|d| entries.get(&d).map(|e| (d, e))) {
                Some((desc, e)) => detail(ui, trace, desc, e, v, map_break, &mut actions),
                None => {
                    ui.label(
                        "Choose a map on the left (dim rows are unreferenced: found in the \
                         flash but not read), or follow the latest lookup.",
                    );
                }
            },
        );
    });
    actions
}

/// The maps found, filtered and ordered: referenced and unreferenced, read or not.
fn list(ui: &mut egui::Ui, trace: &MapTrace, v: &mut MapView, entries: &BTreeMap<u32, Entry>) {
    ui.horizontal(|ui| {
        ui.label("Filter");
        ui.add(egui::TextEdit::singleline(&mut v.filter).desired_width(90.0))
            .on_hover_text(
                "Part of the descriptor's address (hex), of a name, 2D / 3D, REF / UNREF, or \
                 DIFF (the maps the compared file changes)",
            );
        egui::ComboBox::from_id_salt("map_sort")
            .selected_text(match v.sort {
                Sort::Recent => "latest",
                Sort::Calls => "most read",
                Sort::Address => "address",
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut v.sort, Sort::Recent, "latest");
                ui.selectable_value(&mut v.sort, Sort::Calls, "most read");
                ui.selectable_value(&mut v.sort, Sort::Address, "address");
            });
        ui.checkbox(&mut v.follow, "follow")
            .on_hover_text("Show the map looked up last: stepping, each lookup as it comes");
    });
    if v.other.is_some() {
        for (&d, e) in entries {
            if let Some(m) = e.stored() {
                v.diff(d, m);
            }
        }
    }
    let changed = |v: &MapView, d: u32| {
        v.diffs
            .get(&d)
            .and_then(|x| x.as_ref())
            .map_or(0, |x| x.changed)
    };
    let f = v.filter.trim().to_uppercase();
    let mut rows: Vec<(u32, &Entry)> = entries
        .iter()
        .filter(|(d, e)| {
            let dims = e
                .map()
                .map_or("", |m| if m.dims() == 3 { "3D" } else { "2D" });
            f.is_empty()
                || format!("{d:06X}").contains(&f)
                || dims == f
                || (f == "REF" && e.referenced() == Some(true))
                || (f == "UNREF" && e.referenced() == Some(false))
                || (f == "DIFF" && changed(v, **d) > 0)
                || title(trace, **d).is_some_and(|t| t.to_uppercase().contains(&f))
        })
        .map(|(&d, e)| (d, e))
        .collect();
    match v.sort {
        Sort::Recent => rows.sort_by_key(|(_, e)| std::cmp::Reverse(e.seen.map_or(0, |s| s.at))),
        Sort::Calls => rows.sort_by_key(|(_, e)| std::cmp::Reverse(e.calls())),
        Sort::Address => {}
    }
    let mono = egui::TextStyle::Monospace;
    let h = ui.text_style_height(&mono);
    egui::ScrollArea::vertical()
        .id_salt("map_list")
        .max_height(380.0)
        .auto_shrink([false, false])
        .show_rows(ui, h, rows.len(), |ui, range| {
            for (d, e) in &rows[range] {
                let size = e.map().map_or("?".into(), |m| {
                    if m.dims() == 3 {
                        format!("{}×{}", m.nx(), m.ny())
                    } else {
                        format!("{}", m.nx())
                    }
                });
                let last = e.seen.map_or(f32::NAN, |s| s.last.2);
                let mut text = format!("{d:06X} {size:>6} {:>8} → {}", e.calls(), num(last));
                let n = changed(v, *d);
                if n > 0 {
                    text.push_str(&format!("  Δ{n}"));
                }
                if let Some(t) = title(trace, *d) {
                    // One line a map: the name cut short (whole in the detail).
                    text.push_str("  ");
                    let mut t: String = t.chars().take(ROW_NAME).collect();
                    if t.chars().count() == ROW_NAME {
                        t.push('…');
                    }
                    text.push_str(&t);
                }
                let mut rt = egui::RichText::new(text).monospace();
                // Unreferenced maps (found but not read by the firmware) are dimmed.
                if e.referenced() == Some(false) {
                    rt = rt.weak();
                }
                let r = ui.selectable_label(v.selected == Some(*d), rt);
                if r.clicked() {
                    v.selected = Some(*d);
                    v.follow = false;
                }
            }
        });
}

/// The latest lookups, newest first.
fn latest(ui: &mut egui::Ui, trace: &MapTrace, v: &mut MapView) {
    ui.label(egui::RichText::new("Latest lookups").strong());
    for c in trace.recent.iter().rev().take(LATEST) {
        let ms = c.cycles as f64 / 80_000.0;
        let ins = if c.fr5.is_finite() && Some(c.routine) == trace.masters.d3 {
            format!("{}, {}", num(c.fr4), num(c.fr5))
        } else {
            num(c.fr4)
        };
        let text = format!("{ms:10.3} ms {:06X} ({ins}) → {}", c.r4, num(c.fr0));
        if ui
            .selectable_label(
                v.selected == Some(c.r4),
                egui::RichText::new(text).monospace(),
            )
            .clicked()
        {
            v.selected = Some(c.r4);
            v.follow = false;
        }
    }
}

/// One map: what it is, where it is read from, and the map itself. The map comes from the
/// catalogue, the run-time trace, or both; its lookups, sites and trail are shown only
/// when the firmware has reached it.
fn detail(
    ui: &mut egui::Ui,
    trace: &MapTrace,
    desc: u32,
    e: &Entry,
    v: &mut MapView,
    map_break: Option<u32>,
    actions: &mut Vec<Action>,
) {
    let Some(map) = e.map() else {
        ui.label(format!(
            "Descriptor {desc:06X}: it does not read as a map ({} lookups).",
            e.calls()
        ));
        return;
    };
    ui.heading(if map.dims() == 3 {
        format!("Map {desc:06X} · {}×{}", map.nx(), map.ny())
    } else {
        format!("Curve {desc:06X} · {} points", map.nx())
    });
    if let Some(t) = title(trace, desc) {
        ui.label(egui::RichText::new(t).strong());
    }
    match e.referenced() {
        Some(true) => {
            ui.label(
                egui::RichText::new(
                    "referenced · the firmware loads this descriptor before a \
                     master call",
                )
                .weak(),
            );
        }
        Some(false) => {
            ui.colored_label(
                Color32::from_rgb(200, 170, 80),
                "unreferenced · no master call loads this descriptor: an unused calibration, \
                 or a false positive of the scan",
            );
        }
        None => {}
    }
    let stored = if map.cell == session::maps::Cell::Float {
        "float".to_string()
    } else {
        format!(
            "{} × {} + {}",
            map.cell.name(),
            num(map.scale),
            num(map.offset)
        )
    };
    ui.label(format!(
        "cells {stored} at {:06X} · X at {:06X}{} · {} lookups",
        map.z_at,
        map.x_at,
        if map.dims() == 3 {
            format!(" · Y at {:06X}", map.y_at)
        } else {
            String::new()
        },
        e.calls()
    ));
    if let Some(name) = v.other.as_ref().map(|o| o.name.clone()) {
        let base = e.stored().unwrap_or(map);
        match v.diff(desc, base) {
            Some(d) if d.changed > 0 => {
                ui.colored_label(
                    Color32::from_rgb(230, 150, 60),
                    format!(
                        "{name}: {} of {} cells differ · largest change {}{}",
                        d.changed,
                        base.z.len(),
                        if d.largest > 0.0 { "+" } else { "" },
                        num(d.largest)
                    ),
                );
            }
            Some(_) => {
                ui.label(egui::RichText::new(format!("{name}: the same")).weak());
            }
            None => {
                ui.label(
                    egui::RichText::new(format!(
                        "{name}: not comparable (its cells are not in the flash, or the \
                         descriptor reads differently there)"
                    ))
                    .weak(),
                );
            }
        }
    }
    if let Some(s) = e.seen {
        let (x, y, out) = s.last;
        ui.label(
            egui::RichText::new(if map.dims() == 3 {
                format!(
                    "last lookup: X {}, Y {}, result {}",
                    num(x),
                    num(y),
                    num(out)
                )
            } else {
                format!("last lookup: X {}, result {}", num(x), num(out))
            })
            .strong(),
        );
        if s.mismatches > 0 {
            ui.colored_label(
                Color32::YELLOW,
                format!(
                    "{} lookups gave something else than this map interpolated: \
                     the decoding may be off",
                    s.mismatches
                ),
            );
        }
        // Each call site: where its inputs come from and where its result goes.
        for (&ret, &n) in &s.sites {
            let site = trace.sites.get(&ret);
            ui.horizontal_wrapped(|ui| {
                if ui
                    .link(format!("{:06X}", ret.wrapping_sub(4)))
                    .on_hover_text(format!("{n} lookups from here; open in the code view"))
                    .clicked()
                {
                    actions.push(Action::GoTo(ret.wrapping_sub(4)));
                }
                let Some(site) = site else { return };
                let inputs: &[(&str, Source)] = if map.dims() == 3 {
                    &[("X", site.x), ("Y", site.y)]
                } else {
                    &[("X", site.x)]
                };
                for &(axis, src) in inputs {
                    ui.label(format!("{axis} from"));
                    source_link(ui, trace, src, v);
                }
                ui.label(match site.out {
                    Some(a) => format!("· result into {}", var_name(trace, a)),
                    None => "· result used in the caller".into(),
                });
            });
        }
        let readers = trace.readers(desc);
        if !readers.is_empty() {
            ui.horizontal_wrapped(|ui| {
                ui.label("Its result is read by");
                for d in readers {
                    if ui.link(format!("map {d:06X}")).clicked() {
                        v.selected = Some(d);
                        v.follow = false;
                    }
                }
            });
        }
    } else {
        ui.label(
            egui::RichText::new(
                "Not looked up in this run — stop at its next lookup, or drive \
                 the engine onto it.",
            )
            .weak(),
        );
    }
    ui.horizontal(|ui| {
        if map_break == Some(desc) {
            if ui.button("Stopping at its next lookup · cancel").clicked() {
                actions.push(Action::Break(None));
            }
        } else if ui
            .button("Stop at its next lookup")
            .on_hover_text("Run, and stop where the firmware next reads this map")
            .clicked()
        {
            actions.push(Action::Break(Some(desc)));
        }
        ui.separator();
        ui.selectable_value(
            &mut v.mode,
            Mode::Heat,
            if map.dims() == 3 { "Heat map" } else { "Curve" },
        );
        if map.dims() == 3 {
            ui.selectable_value(&mut v.mode, Mode::Surface, "Surface");
        }
        ui.selectable_value(&mut v.mode, Mode::Table, "Table");
        ui.separator();
        if ui
            .button("Copy")
            .on_hover_text("The map as text with tabs, to paste into a spreadsheet")
            .clicked()
        {
            ui.ctx().copy_text(as_text(map, '\t'));
        }
        if ui.button("Save CSV…").clicked()
            && let Some(path) = rfd::FileDialog::new()
                .set_title("Save the map")
                .set_file_name(format!("map_{desc:06X}.csv"))
                .save_file()
            && let Err(e) = std::fs::write(&path, as_text(map, ','))
        {
            v.note = Some(e.to_string());
        }
        if v.other.is_some() {
            ui.selectable_value(&mut v.mode, Mode::Diff, "Differences")
                .on_hover_text("Each cell as this flash has it → as the compared file has it");
        } else if v.mode == Mode::Diff {
            v.mode = Mode::Table;
        }
    });
    // Where the firmware has been on this map, oldest first.
    let trail: Vec<(f32, f32, f32)> = trace
        .recent
        .iter()
        .rev()
        .filter(|c| c.r4 == desc)
        .take(TRAIL)
        .map(|c| (c.fr4, c.fr5, c.fr0))
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    let now = e.seen.map_or((f32::NAN, f32::NAN, f32::NAN), |s| s.last);
    match (v.mode, map.dims()) {
        (Mode::Diff, _) => {
            let base = e.stored().unwrap_or(map);
            match v.diff(desc, base) {
                Some(d) => diff_table(ui, base, &d.map),
                None => {
                    ui.label("Not comparable with the other file.");
                }
            }
        }
        (Mode::Table, _) => {
            editor(ui, desc, e, map, v, actions);
            let sel = v.cell.filter(|c| c.0 == desc).map(|c| (c.1, c.2));
            let edited = v.edited.get(&desc).cloned().unwrap_or_default();
            if let Some((i, j)) = table(ui, map, now, sel, &edited) {
                v.cell = Some((desc, i, j));
                v.cell_text = map.at(i, j).to_string();
            }
        }
        (_, 2) => curve(ui, map, now, &trail),
        (Mode::Surface, _) => surface(ui, map, now, v),
        _ => heat_map(ui, map, now, &trail),
    }
}

/// What a map is called (see [`MapTrace::title`]).
fn title(trace: &MapTrace, desc: u32) -> Option<String> {
    trace.title(desc)
}

/// A variable by name (see [`MapTrace::var_name`]).
fn var_name(trace: &MapTrace, a: u32) -> String {
    trace.var_name(a)
}

/// Where an input comes from; another map is a link to it.
fn source_link(ui: &mut egui::Ui, trace: &MapTrace, src: Source, v: &mut MapView) {
    match src {
        Source::Map(d) => {
            if ui.link(trace.source_text(src)).clicked() {
                v.selected = Some(d);
                v.follow = false;
            }
        }
        Source::Ram(_) => {
            ui.label(egui::RichText::new(trace.source_text(src)).strong());
        }
        _ => {
            ui.label(trace.source_text(src));
        }
    }
}

/// What is known by name, and the ways to know more.
fn names_row(ui: &mut egui::Ui, trace: &MapTrace, v: &mut MapView, actions: &mut Vec<Action>) {
    let n = &trace.names;
    ui.horizontal_wrapped(|ui| {
        ui.label(format!(
            "Names: {} variables, {} routines, {} maps",
            n.vars.len(),
            n.routines.len(),
            n.maps.len()
        ));
        if ui
            .button("Find the sensors")
            .on_hover_text(
                "Move every sensor for a moment and see which variables follow (about 3 s \
                 of ECU time)",
            )
            .clicked()
        {
            actions.push(Action::Name(None));
        }
        if ui
            .button("Read an opendash profile…")
            .on_hover_text(
                "A parameter profile (opendash's profiles.txt): the ECU is asked for each \
                 identifier and the variable holding the answer takes its name. Read from \
                 your disk; nothing is kept with the emulator.",
            )
            .clicked()
            && let Some(path) = rfd::FileDialog::new()
                .set_title("opendash parameter profiles")
                .pick_file()
        {
            match std::fs::read_to_string(&path) {
                Ok(t) => {
                    v.profiles = read_profiles(&t);
                    v.profile = 0;
                    v.note = Some(format!("{} profiles read", v.profiles.len()));
                }
                Err(e) => v.note = Some(e.to_string()),
            }
        }
        if !v.profiles.is_empty() {
            let k = v.profile.min(v.profiles.len() - 1);
            egui::ComboBox::from_id_salt("profile")
                .selected_text(v.profiles[k].name.clone())
                .show_ui(ui, |ui| {
                    for (i, p) in v.profiles.iter().enumerate() {
                        ui.selectable_value(&mut v.profile, i, &p.name);
                    }
                });
            if ui.button("Name from it").clicked() {
                actions.push(Action::Name(Some(v.profiles[k].clone())));
            }
        }
        if ui
            .button("Read map labels…")
            .on_hover_text("Labels by descriptor, in the transferred-tags format")
            .clicked()
            && let Some(path) = rfd::FileDialog::new().set_title("Map labels").pick_file()
        {
            match std::fs::read_to_string(&path) {
                Ok(t) => {
                    let l = read_labels(&t);
                    v.note = Some(format!("{} labels read", l.len()));
                    actions.push(Action::Labels(l));
                }
                Err(e) => v.note = Some(e.to_string()),
            }
        }
        if let Some(o) = &v.other {
            let n = v.diffs.values().flatten().filter(|d| d.changed > 0).count();
            ui.label(
                egui::RichText::new(format!("comparing with {} · {n} maps differ", o.name))
                    .color(Color32::from_rgb(230, 150, 60)),
            );
            if ui
                .button("Stop comparing")
                .on_hover_text("Forget the other file")
                .clicked()
            {
                v.other = None;
                v.diffs.clear();
            }
        } else if ui
            .button("Compare with a flash…")
            .on_hover_text(
                "Another dump of this ECU (a modified file, another car): every map is read \
                 from it with the same descriptor and the differing cells are counted. Filter \
                 DIFF lists the maps it changes. Read from your disk; nothing is kept.",
            )
            .clicked()
            && let Some(path) = rfd::FileDialog::new()
                .set_title("Flash to compare with")
                .pick_file()
        {
            match std::fs::read(&path) {
                Ok(flash) => {
                    let name = path
                        .file_name()
                        .map_or("other file".into(), |n| n.to_string_lossy().into_owned());
                    v.other = Some(Other { name, flash });
                    v.diffs.clear();
                }
                Err(e) => v.note = Some(e.to_string()),
            }
        }
        if let Some(note) = &v.note {
            ui.label(egui::RichText::new(note).weak());
        }
    });
}

/// The cells coloured by value, with where the firmware is and has been.
fn heat_map(ui: &mut egui::Ui, map: &Map, now: (f32, f32, f32), trail: &[(f32, f32, f32)]) {
    let avail = ui.available_size();
    let size = Vec2::new(avail.x.max(300.0), (avail.y - 10.0).clamp(240.0, 560.0));
    let (resp, p) = ui.allocate_painter(size, Sense::hover());
    let r = resp.rect;
    let plot = Rect::from_min_max(r.min + Vec2::new(56.0, 6.0), r.max - Vec2::new(8.0, 28.0));
    let (nx, ny) = (map.nx(), map.ny());
    let (cw, ch) = (plot.width() / nx as f32, plot.height() / ny as f32);
    let (lo, hi) = span(map);
    for j in 0..ny {
        for i in 0..nx {
            let z = map.at(i, j);
            let cell = Rect::from_min_size(
                plot.min + Vec2::new(i as f32 * cw, j as f32 * ch),
                Vec2::new(cw, ch),
            );
            p.rect_filled(cell.shrink(0.5), 0.0, heat((z - lo) / (hi - lo)));
            if cw > 34.0 && ch > 14.0 {
                p.text(
                    cell.center(),
                    Align2::CENTER_CENTER,
                    num(z),
                    FontId::monospace(10.0),
                    Color32::BLACK,
                );
            }
        }
    }
    let font = FontId::proportional(10.0);
    let grey = Color32::GRAY;
    let every = |n: usize, room: f32| ((n as f32 * 34.0 / room).ceil() as usize).max(1);
    for i in (0..nx).step_by(every(nx, plot.width())) {
        p.text(
            Pos2::new(plot.left() + (i as f32 + 0.5) * cw, plot.bottom() + 4.0),
            Align2::CENTER_TOP,
            num(map.x[i]),
            font.clone(),
            grey,
        );
    }
    for j in (0..ny).step_by(every(ny, plot.height() * 2.0)) {
        p.text(
            Pos2::new(plot.left() - 4.0, plot.top() + (j as f32 + 0.5) * ch),
            Align2::RIGHT_CENTER,
            num(map.y[j]),
            font.clone(),
            grey,
        );
    }
    p.text(
        Pos2::new(plot.right(), plot.bottom() + 16.0),
        Align2::RIGHT_TOP,
        "X, across",
        font.clone(),
        grey,
    );
    p.text(
        Pos2::new(r.left(), plot.top()),
        Align2::LEFT_TOP,
        "Y, down",
        font,
        grey,
    );
    let at = |x: f32, y: f32| {
        Pos2::new(
            plot.left() + (Map::position(&map.x, x) + 0.5) * cw,
            plot.top() + (Map::position(&map.y, y) + 0.5) * ch,
        )
    };
    let pts: Vec<Pos2> = trail.iter().map(|&(x, y, _)| at(x, y)).collect();
    if pts.len() > 1 {
        p.add(Shape::line(
            pts,
            Stroke::new(1.5, Color32::from_white_alpha(140)),
        ));
    }
    if now.0.is_finite() {
        let c = at(now.0, now.1);
        p.circle_stroke(c, 7.0, Stroke::new(2.5, Color32::WHITE));
        p.circle_filled(c, 2.5, Color32::WHITE);
    }
    if let Some(h) = resp.hover_pos()
        && plot.contains(h)
    {
        let i = (((h.x - plot.left()) / cw) as usize).min(nx - 1);
        let j = (((h.y - plot.top()) / ch) as usize).min(ny - 1);
        resp.on_hover_text(format!(
            "X {} · Y {}: {}",
            num(map.x[i]),
            num(map.y[j]),
            num(map.at(i, j))
        ));
    }
}

/// A curve: its points, where the firmware is and has been.
fn curve(ui: &mut egui::Ui, map: &Map, now: (f32, f32, f32), trail: &[(f32, f32, f32)]) {
    let avail = ui.available_size();
    let size = Vec2::new(avail.x.max(300.0), (avail.y - 10.0).clamp(220.0, 480.0));
    let (resp, p) = ui.allocate_painter(size, Sense::hover());
    let r = resp.rect;
    let plot = Rect::from_min_max(r.min + Vec2::new(56.0, 8.0), r.max - Vec2::new(10.0, 24.0));
    p.rect_filled(plot, 0.0, Color32::from_gray(22));
    let (lo, hi) = span(map);
    let (x0, x1) = (map.x[0], *map.x.last().expect("a curve has points"));
    let xs = if x1 > x0 { x1 - x0 } else { 1.0 };
    let pos = |x: f32, z: f32| {
        Pos2::new(
            plot.left() + (x.clamp(x0, x1) - x0) / xs * plot.width(),
            plot.bottom() - (z - lo) / (hi - lo) * plot.height(),
        )
    };
    let pts: Vec<Pos2> = (0..map.nx()).map(|i| pos(map.x[i], map.at(i, 0))).collect();
    p.add(Shape::line(
        pts.clone(),
        Stroke::new(2.0, Color32::from_rgb(110, 170, 255)),
    ));
    for q in &pts {
        p.circle_filled(*q, 2.5, Color32::from_rgb(110, 170, 255));
    }
    let font = FontId::proportional(10.0);
    for (k, &x) in map.x.iter().enumerate() {
        if k % ((map.nx() / 12).max(1)) == 0 {
            p.text(
                Pos2::new(pos(x, lo).x, plot.bottom() + 4.0),
                Align2::CENTER_TOP,
                num(x),
                font.clone(),
                Color32::GRAY,
            );
        }
    }
    for z in [lo, (lo + hi) / 2.0, hi] {
        p.text(
            Pos2::new(plot.left() - 4.0, pos(x0, z).y),
            Align2::RIGHT_CENTER,
            num(z),
            font.clone(),
            Color32::GRAY,
        );
    }
    let trail: Vec<Pos2> = trail.iter().map(|&(x, _, z)| pos(x, z)).collect();
    for q in &trail {
        p.circle_filled(*q, 2.0, Color32::from_white_alpha(120));
    }
    if now.0.is_finite() {
        let c = pos(now.0, now.2);
        p.line_segment(
            [Pos2::new(c.x, plot.top()), Pos2::new(c.x, plot.bottom())],
            Stroke::new(1.0, Color32::from_white_alpha(90)),
        );
        p.circle_stroke(c, 7.0, Stroke::new(2.5, Color32::WHITE));
    }
}

/// The map as a surface, to turn by dragging.
fn surface(ui: &mut egui::Ui, map: &Map, now: (f32, f32, f32), v: &mut MapView) {
    let avail = ui.available_size();
    let size = Vec2::new(avail.x.max(300.0), (avail.y - 10.0).clamp(260.0, 560.0));
    let (resp, p) = ui.allocate_painter(size, Sense::drag());
    let d = resp.drag_delta();
    v.yaw += d.x * 0.01;
    v.pitch = (v.pitch + d.y * 0.01).clamp(0.05, 1.45);
    let r = resp.rect;
    p.rect_filled(r, 0.0, Color32::from_gray(18));
    let (nx, ny) = (map.nx(), map.ny());
    let (lo, hi) = span(map);
    let (cy, sy, cp, sp) = (v.yaw.cos(), v.yaw.sin(), v.pitch.cos(), v.pitch.sin());
    let scale = r.width().min(r.height() * 1.4) * 0.42;
    // A point of the unit box (x, y across, z up) on the screen, and its depth.
    let project = |x: f32, y: f32, z: f32| {
        let (x, y, z) = (x - 0.5, y - 0.5, (z - 0.5) * 0.6);
        let (rx, ry) = (x * cy - y * sy, x * sy + y * cy);
        let (sy2, depth) = (ry * sp - z * cp, ry * cp + z * sp);
        (r.center() + Vec2::new(rx, sy2) * scale, depth)
    };
    let fx = |i: usize| {
        if nx > 1 {
            i as f32 / (nx - 1) as f32
        } else {
            0.5
        }
    };
    let fy = |j: usize| {
        if ny > 1 {
            j as f32 / (ny - 1) as f32
        } else {
            0.5
        }
    };
    let fz = |z: f32| (z - lo) / (hi - lo);
    // Each cell: its depth, its corners on the screen, its colour shaded by how it
    // faces the light.
    let mut quads: Vec<(f32, [Pos2; 4], Color32)> = Vec::new();
    for j in 0..ny.saturating_sub(1) {
        for i in 0..nx.saturating_sub(1) {
            let corners = [(i, j), (i + 1, j), (i + 1, j + 1), (i, j + 1)];
            let mut pts = [Pos2::ZERO; 4];
            let (mut depth, mut zsum) = (0.0, 0.0);
            for (k, &(a, b)) in corners.iter().enumerate() {
                let z = map.at(a, b);
                let (q, dpt) = project(fx(a), fy(b), fz(z));
                pts[k] = q;
                depth += dpt;
                zsum += z;
            }
            // The cell's slope along each axis, in the unit box, for the shading.
            let (dx, dy) = (fx(i + 1) - fx(i), fy(j + 1) - fy(j));
            let sx = (fz(map.at(i + 1, j)) - fz(map.at(i, j))) * 0.6 / dx.max(1e-6);
            let sy = (fz(map.at(i, j + 1)) - fz(map.at(i, j))) * 0.6 / dy.max(1e-6);
            let light = 1.0 / (1.0 + sx * sx + sy * sy).sqrt();
            let c = heat(fz(zsum / 4.0));
            let f = 0.55 + 0.45 * light;
            let shade = |v: u8| (v as f32 * f) as u8;
            quads.push((
                depth,
                pts,
                Color32::from_rgb(shade(c.r()), shade(c.g()), shade(c.b())),
            ));
        }
    }
    // Back to front, as one mesh: triangles are drawn in order and, unlike outlined
    // polygons, never feathered into spikes when a cell turns edge-on.
    quads.sort_by(|a, b| b.0.total_cmp(&a.0));
    let mut mesh = egui::Mesh::default();
    let edge = Color32::from_black_alpha(90);
    for (_, q, colour) in &quads {
        let base = mesh.vertices.len() as u32;
        for v in q {
            mesh.colored_vertex(*v, *colour);
        }
        mesh.add_triangle(base, base + 1, base + 2);
        mesh.add_triangle(base, base + 2, base + 3);
        for k in 0..4 {
            thin_line(&mut mesh, q[k], q[(k + 1) % 4], 0.35, edge);
        }
    }
    p.add(Shape::mesh(mesh));
    let font = FontId::proportional(10.0);
    let grey = Color32::GRAY;
    for (q, text) in [
        (
            project(0.0, 0.0, 0.0).0,
            format!("X {} · Y {}", num(map.x[0]), num(map.y[0])),
        ),
        (
            project(1.0, 0.0, 0.0).0,
            format!("X {}", num(*map.x.last().expect("axis"))),
        ),
        (
            project(0.0, 1.0, 0.0).0,
            format!("Y {}", num(*map.y.last().expect("axis"))),
        ),
    ] {
        p.text(q, Align2::CENTER_CENTER, text, font.clone(), grey);
    }
    p.text(
        r.left_top() + Vec2::new(6.0, 4.0),
        Align2::LEFT_TOP,
        format!("{} .. {} · drag to turn", num(lo), num(hi)),
        font,
        grey,
    );
    if now.0.is_finite() {
        let (px, py) = (
            Map::position(&map.x, now.0) / (nx.max(2) - 1) as f32,
            Map::position(&map.y, now.1) / (ny.max(2) - 1) as f32,
        );
        let (top, _) = project(px, py, fz(now.2));
        let (base, _) = project(px, py, 0.0);
        p.line_segment([base, top], Stroke::new(1.5, Color32::WHITE));
        p.circle_stroke(top, 6.0, Stroke::new(2.5, Color32::WHITE));
    }
}

/// A line `width` across from `a` to `b`, as two triangles of `mesh`.
fn thin_line(mesh: &mut egui::Mesh, a: Pos2, b: Pos2, width: f32, colour: Color32) {
    let d = b - a;
    let len = d.length();
    if len < 1e-3 {
        return;
    }
    let n = Vec2::new(-d.y, d.x) / len * width;
    let base = mesh.vertices.len() as u32;
    for v in [a + n, b + n, b - n, a - n] {
        mesh.colored_vertex(v, colour);
    }
    mesh.add_triangle(base, base + 1, base + 2);
    mesh.add_triangle(base, base + 2, base + 3);
}

/// The map as rows of text: the X axis on top and, for a map, the Y axis down the left.
fn as_text(map: &Map, sep: char) -> String {
    let mut out = String::new();
    let row = |out: &mut String, head: String, cells: Vec<f32>| {
        out.push_str(&head);
        for c in cells {
            out.push(sep);
            out.push_str(&c.to_string());
        }
        out.push('\n');
    };
    row(
        &mut out,
        if map.dims() == 3 { "Y\\X" } else { "X" }.into(),
        map.x.clone(),
    );
    for j in 0..map.ny() {
        let head = if map.dims() == 3 {
            map.y[j].to_string()
        } else {
            "Z".into()
        };
        row(
            &mut out,
            head,
            (0..map.nx()).map(|i| map.at(i, j)).collect(),
        );
    }
    out
}

/// The numbers of two files side by side: each cell as `this → other`, on red where the
/// other file raises it and on blue where it lowers it, stronger the bigger the change.
fn diff_table(ui: &mut egui::Ui, base: &Map, other: &Map) {
    let largest = base
        .z
        .iter()
        .zip(&other.z)
        .map(|(a, b)| (b - a).abs())
        .filter(|d| d.is_finite())
        .fold(0.0f32, f32::max);
    egui::ScrollArea::both()
        .id_salt("map_diff")
        .max_height(560.0)
        .show(ui, |ui| {
            egui::Grid::new("map_diff_cells")
                .striped(true)
                .show(ui, |ui| {
                    ui.label(
                        egui::RichText::new(if base.dims() == 3 { "Y \\ X" } else { "X" })
                            .monospace(),
                    );
                    for x in &base.x {
                        ui.label(egui::RichText::new(num(*x)).monospace().strong());
                    }
                    ui.end_row();
                    for j in 0..base.ny() {
                        let head = if base.dims() == 3 {
                            num(base.y[j])
                        } else {
                            "Z".into()
                        };
                        ui.label(egui::RichText::new(head).monospace().strong());
                        for i in 0..base.nx() {
                            let (a, b) = (base.at(i, j), other.at(i, j));
                            let d = b - a;
                            let mut t = if d == 0.0 || !d.is_finite() {
                                egui::RichText::new(num(a)).monospace().weak()
                            } else {
                                egui::RichText::new(format!("{} → {}", num(a), num(b))).monospace()
                            };
                            if d != 0.0 && d.is_finite() && largest > 0.0 {
                                let k = (40.0 + 160.0 * d.abs() / largest) as u8;
                                let bg = if d > 0.0 {
                                    Color32::from_rgb(k, 30, 30)
                                } else {
                                    Color32::from_rgb(30, 40, k)
                                };
                                t = t.background_color(bg).color(Color32::WHITE);
                            }
                            ui.label(t);
                        }
                        ui.end_row();
                    }
                });
        });
}

/// Changing a cell of the map in the emulated ECU: its flash image and, when the firmware
/// reads the map from RAM, that copy too. Only the emulator's memory changes.
fn editor(
    ui: &mut egui::Ui,
    desc: u32,
    e: &Entry,
    map: &Map,
    v: &mut MapView,
    actions: &mut Vec<Action>,
) {
    // Where the cells are: in the flash, and where the firmware reads them if elsewhere.
    let stored = e.stored().unwrap_or(map);
    let mut places = vec![stored.z_at];
    if map.z_at != stored.z_at {
        places.push(map.z_at);
    }
    let width = map.cell.bytes();
    ui.horizontal_wrapped(|ui| {
        match v.cell.filter(|c| c.0 == desc) {
            Some((_, i, j)) => {
                ui.label(if map.dims() == 3 {
                    format!("Cell X {}, Y {}:", num(map.x[i]), num(map.y[j]))
                } else {
                    format!("Point X {}:", num(map.x[i]))
                });
                let r = ui.add(egui::TextEdit::singleline(&mut v.cell_text).desired_width(80.0));
                let enter = r.lost_focus() && ui.input(|k| k.key_pressed(egui::Key::Enter));
                if ui.button("Set").clicked() || enter {
                    let k = j * map.nx() + i;
                    match v.cell_text.trim().replace(',', ".").parse::<f32>() {
                        Ok(x) => match encode(map, x) {
                            Ok(bytes) => {
                                let mut writes = Vec::new();
                                for &at in &places {
                                    let a = at.wrapping_add(k as u32 * width);
                                    for (n, &b) in bytes.iter().enumerate() {
                                        writes.push((a.wrapping_add(n as u32), b));
                                    }
                                }
                                actions.push(Action::Edit(writes));
                                v.edited.entry(desc).or_default().insert(k);
                                v.diffs.remove(&desc);
                                v.note = None;
                            }
                            Err(why) => v.note = Some(why),
                        },
                        Err(_) => v.note = Some("not a number".into()),
                    }
                }
            }
            None => {
                ui.label(
                    egui::RichText::new("Click a cell to change it in the emulated ECU.").weak(),
                );
            }
        }
        if v.edited.contains_key(&desc) {
            ui.separator();
            if ui
                .button("Revert this map")
                .on_hover_text("Its cells as in the flash loaded")
                .clicked()
            {
                let len = map.z.len() as u32 * width;
                actions.push(Action::Revert(
                    places.iter().map(|&at| (at, stored.z_at, len)).collect(),
                ));
                v.edited.remove(&desc);
                v.diffs.remove(&desc);
            }
        }
        if !v.edited.is_empty() {
            ui.separator();
            if ui
                .button("Save flash as…")
                .on_hover_text(
                    "The flash with the changes, to your disk. Its checksum is not recomputed: \
                     for the emulator and for study, not for an ECU.",
                )
                .clicked()
            {
                actions.push(Action::SaveFlash);
            }
        }
    });
}

/// The numbers, with the cells the last lookup interpolated between marked, the chosen
/// cell outlined and the changed ones underlined. Returns the cell clicked.
fn table(
    ui: &mut egui::Ui,
    map: &Map,
    now: (f32, f32, f32),
    sel: Option<(usize, usize)>,
    edited: &std::collections::BTreeSet<usize>,
) -> Option<(usize, usize)> {
    let mut clicked = None;
    let near = |axis: &[f32], v: f32| {
        let p = Map::position(axis, v);
        let i = p.floor() as usize;
        (i, if p > i as f32 { i + 1 } else { i })
    };
    let (i0, i1) = near(&map.x, now.0);
    let (j0, j1) = if map.dims() == 3 {
        near(&map.y, now.1)
    } else {
        (0, 0)
    };
    let used =
        |i: usize, j: usize| now.0.is_finite() && (i == i0 || i == i1) && (j == j0 || j == j1);
    egui::ScrollArea::both()
        .id_salt("map_table")
        .max_height(560.0)
        .show(ui, |ui| {
            egui::Grid::new("map_cells").striped(true).show(ui, |ui| {
                ui.label(
                    egui::RichText::new(if map.dims() == 3 { "Y \\ X" } else { "X" }).monospace(),
                );
                for x in &map.x {
                    ui.label(egui::RichText::new(num(*x)).monospace().strong());
                }
                ui.end_row();
                for j in 0..map.ny() {
                    let head = if map.dims() == 3 {
                        num(map.y[j])
                    } else {
                        "Z".into()
                    };
                    ui.label(egui::RichText::new(head).monospace().strong());
                    for i in 0..map.nx() {
                        let mut t = egui::RichText::new(num(map.at(i, j))).monospace();
                        if used(i, j) {
                            t = t
                                .background_color(Color32::from_rgb(90, 80, 20))
                                .color(Color32::WHITE);
                        }
                        if edited.contains(&(j * map.nx() + i)) {
                            t = t.underline().color(Color32::from_rgb(230, 150, 60));
                        }
                        if sel == Some((i, j)) {
                            t = t.background_color(Color32::from_rgb(40, 90, 150));
                        }
                        if ui.add(egui::Label::new(t).sense(Sense::click())).clicked() {
                            clicked = Some((i, j));
                        }
                    }
                    ui.end_row();
                }
            });
        });
    clicked
}
