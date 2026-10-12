//! The maps window: every calibration map the firmware reads, as it reads it. A list of
//! the maps looked up (how often, the last inputs and result), and the one chosen drawn
//! whole, from its own descriptor, with where the firmware is on it now and where it
//! has just been: as a heat map, a surface to turn, a table, or a curve. Its call sites
//! open in the code view, and the ECU can stop at its next lookup.

use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, Sense, Shape, Stroke, Vec2};
use session::maps::{Map, MapTrace, Seen};
use session::names::{How, Name, Profile, read_labels, read_profiles};
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

/// How a map is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mode {
    /// Cells coloured by value (a curve: its line).
    Heat,
    /// A surface to turn (a curve: its line).
    Surface,
    /// The numbers.
    Table,
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
        }
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
    map_break: Option<u32>,
) -> Vec<Action> {
    let mut actions = Vec::new();
    if v.follow
        && let Some(c) = trace.recent.back()
    {
        v.selected = Some(c.r4);
    }
    let m = trace.masters;
    ui.horizontal_wrapped(|ui| {
        ui.label(format!(
            "{} maps read · {} lookups · interpolation routines: 3D {}, 2D {}",
            trace.seen.len(),
            trace.total,
            m.d3.map_or("not found".into(), |a| format!("{a:#06X}")),
            m.d2.map_or("not found".into(), |a| format!("{a:#06X}")),
        ));
        if trace.dropped > 0 {
            ui.colored_label(Color32::YELLOW, format!("{} lookups lost", trace.dropped));
        }
    });
    if m.d3.is_none() && m.d2.is_none() {
        ui.label(
            "No interpolation routine found in this flash: it does not read its maps the \
             Denso way this viewer knows.",
        );
        return actions;
    }
    names_row(ui, trace, v, &mut actions);
    ui.separator();
    ui.horizontal_top(|ui| {
        ui.vertical(|ui| {
            ui.set_width(360.0);
            list(ui, trace, v);
            ui.separator();
            latest(ui, trace, v);
        });
        ui.separator();
        ui.vertical(|ui| {
            let seen = v.selected.and_then(|d| trace.seen.get(&d).map(|s| (d, s)));
            match seen {
                Some((desc, s)) => detail(ui, trace, desc, s, v, map_break, &mut actions),
                None => {
                    ui.label("Choose a map on the left, or follow the latest lookup.");
                }
            }
        });
    });
    actions
}

/// The maps looked up, filtered and ordered.
fn list(ui: &mut egui::Ui, trace: &MapTrace, v: &mut MapView) {
    ui.horizontal(|ui| {
        ui.label("Filter");
        ui.add(egui::TextEdit::singleline(&mut v.filter).desired_width(90.0))
            .on_hover_text("Part of the descriptor's address (hex), of a name, or 2D / 3D");
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
    let f = v.filter.trim().to_uppercase();
    let mut rows: Vec<(u32, &Seen)> = trace
        .seen
        .iter()
        .filter(|(d, s)| {
            let dims = s
                .map
                .as_ref()
                .map_or("", |m| if m.dims() == 3 { "3D" } else { "2D" });
            f.is_empty()
                || format!("{d:06X}").contains(&f)
                || dims == f
                || title(trace, **d).is_some_and(|t| t.to_uppercase().contains(&f))
        })
        .map(|(&d, s)| (d, s))
        .collect();
    match v.sort {
        Sort::Recent => rows.sort_by_key(|(_, s)| std::cmp::Reverse(s.at)),
        Sort::Calls => rows.sort_by_key(|(_, s)| std::cmp::Reverse(s.calls)),
        Sort::Address => {}
    }
    let mono = egui::TextStyle::Monospace;
    let h = ui.text_style_height(&mono);
    egui::ScrollArea::vertical()
        .id_salt("map_list")
        .max_height(380.0)
        .auto_shrink([false, false])
        .show_rows(ui, h, rows.len(), |ui, range| {
            for (d, s) in &rows[range] {
                let size = s.map.as_ref().map_or("?".into(), |m| {
                    if m.dims() == 3 {
                        format!("{}×{}", m.nx(), m.ny())
                    } else {
                        format!("{}", m.nx())
                    }
                });
                let mut text = format!("{d:06X} {size:>6} {:>8} → {}", s.calls, num(s.last.2));
                if let Some(t) = title(trace, *d) {
                    // One line a map: the name cut short (whole in the detail).
                    text.push_str("  ");
                    let mut t: String = t.chars().take(ROW_NAME).collect();
                    if t.chars().count() == ROW_NAME {
                        t.push('…');
                    }
                    text.push_str(&t);
                }
                let r = ui.selectable_label(
                    v.selected == Some(*d),
                    egui::RichText::new(text).monospace(),
                );
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

/// One map: what it is, where it is read from, and the map itself.
fn detail(
    ui: &mut egui::Ui,
    trace: &MapTrace,
    desc: u32,
    s: &Seen,
    v: &mut MapView,
    map_break: Option<u32>,
    actions: &mut Vec<Action>,
) {
    let Some(map) = &s.map else {
        ui.label(format!(
            "Descriptor {desc:06X}: it does not read as a map ({} lookups).",
            s.calls
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
        s.calls
    ));
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
    let now = (x, y, out);
    match (v.mode, map.dims()) {
        (Mode::Table, _) => table(ui, map, now),
        (_, 2) => curve(ui, map, now, &trail),
        (Mode::Surface, _) => surface(ui, map, now, v),
        _ => heat_map(ui, map, now, &trail),
    }
}

/// What a map is called: its label, else what its result is stored as, if named.
fn title(trace: &MapTrace, desc: u32) -> Option<String> {
    if let Some(n) = trace.names.maps.get(&desc) {
        return Some(n.text.clone());
    }
    trace.seen.get(&desc)?.sites.keys().find_map(|r| {
        let a = trace.sites.get(r)?.out?;
        trace
            .names
            .vars
            .get(&a)
            .map(|n| format!("gives {}", n.text))
    })
}

/// A variable: its name if known (with a ? if matched in one state only), and its address.
fn var_name(trace: &MapTrace, a: u32) -> String {
    match trace.names.vars.get(&a) {
        Some(n) => {
            let unsure = matches!(n.how, How::Identifier { sure: false, .. });
            let unit = if n.unit.is_empty() {
                String::new()
            } else {
                format!(", {}", n.unit)
            };
            format!(
                "{}{unit}{} ({a:08X})",
                n.text,
                if unsure { "?" } else { "" }
            )
        }
        None => format!("{a:08X}"),
    }
}

/// Where an input comes from; another map is a link to it.
fn source_link(ui: &mut egui::Ui, trace: &MapTrace, src: Source, v: &mut MapView) {
    match src {
        Source::Ram(a) => {
            ui.label(egui::RichText::new(var_name(trace, a)).strong());
        }
        Source::Map(d) => {
            let t = title(trace, d).map_or(String::new(), |t| format!(" ({t})"));
            if ui.link(format!("map {d:06X}{t}")).clicked() {
                v.selected = Some(d);
                v.follow = false;
            }
        }
        Source::Call(t) => {
            ui.label(match trace.names.routines.get(&t) {
                Some(n) => format!("{} (routine {t:06X})", n.text),
                None => format!("routine {t:06X}"),
            });
        }
        Source::Unknown => {
            ui.label("computed in the caller");
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

/// The numbers, with the cells the last lookup interpolated between marked.
fn table(ui: &mut egui::Ui, map: &Map, now: (f32, f32, f32)) {
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
                        ui.label(t);
                    }
                    ui.end_row();
                }
            });
        });
}
