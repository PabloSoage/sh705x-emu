//! The engine view: the crank and cam wheels turning under their sensors, the four
//! cylinders, and strip charts in ECU time of what the firmware asks for and what the
//! plants give back, with the crank and cam signals and the ATU-II's one-shot pulses
//! (the injectors' outputs) under them. Slowed down or a tooth at a time, the fast moves
//! show: the EGR's drive, the teeth, a pulse.

use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, Sense, Shape, Stroke, Vec2};
use session::scope::{Sample, Scope, TRACES};

/// The chart panels: title and traces (indexes into [`TRACES`]).
const PANELS: [(&str, &[usize]); 9] = [
    ("Engine speed (rpm)", &[0]),
    ("EGR (%)", &[1, 2, 3]),
    ("Boost (kPa)", &[4, 5]),
    ("VGT and SCV commands (%)", &[6, 9]),
    ("SCV current (mA)", &[7, 8]),
    ("Rail pressure (MPa)", &[10, 11]),
    ("Fresh air (g/s)", &[12]),
    ("Fuel asked (mm³)", &[13]),
    ("Torque asked (N·m)", &[14]),
];

/// Time windows offered, ms of ECU time.
const WINDOWS: [f64; 6] = [10.0, 50.0, 200.0, 1000.0, 5000.0, 20000.0];

/// Cam pulses over the 720° cycle, and their width (engine::wheel's defaults).
const CAM: [f32; 5] = [0.0, 180.0, 360.0, 540.0, 690.0];
const CAM_WIDTH: f32 = 10.0;
/// Firing top dead centres of cylinders 1, 2, 3, 4 over 720°, firing order 1-3-4-2, with
/// cylinder 1 put at 0°: where it really is relative to the gap is not known [SUP].
const TDC: [f32; 4] = [0.0, 540.0, 180.0, 360.0];

const COLORS: [Color32; 3] = [
    Color32::from_rgb(110, 170, 255),
    Color32::from_rgb(255, 160, 60),
    Color32::from_rgb(120, 220, 120),
];

/// What the viewer has chosen.
pub struct Settings {
    /// Index into [`WINDOWS`].
    pub(crate) window: usize,
    /// Which panels show.
    panels: [bool; PANELS.len()],
    /// Show the crank and cam signals.
    teeth: bool,
    /// Show the one-shot pulses.
    pulses: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            window: 2,
            panels: [true, true, true, false, false, true, false, false, false],
            teeth: true,
            pulses: true,
        }
    }
}

/// Draws the view of `scope` as of ECU time `now_us`.
pub fn show(ui: &mut egui::Ui, scope: &Scope, set: &mut Settings, now_us: f64) {
    ui.horizontal_wrapped(|ui| {
        ui.label("Window");
        egui::ComboBox::from_id_salt("window")
            .selected_text(window_name(WINDOWS[set.window]))
            .show_ui(ui, |ui| {
                for (k, w) in WINDOWS.iter().enumerate() {
                    ui.selectable_value(&mut set.window, k, window_name(*w));
                }
            });
        ui.separator();
        ui.checkbox(&mut set.teeth, "crank/cam");
        ui.checkbox(&mut set.pulses, "pulses");
        for (k, (title, _)) in PANELS.iter().enumerate() {
            let short = title.split(" (").next().unwrap_or(title);
            ui.checkbox(&mut set.panels[k], short);
        }
    });
    ui.separator();
    let crank = scope
        .samples
        .back()
        .map(|s| s.crank)
        .filter(|a| a.is_finite());
    let t1 = now_us;
    let t0 = t1 - WINDOWS[set.window] * 1000.0;
    let injecting = |k: u8| {
        scope
            .pulses
            .iter()
            .rev()
            .take(64)
            .any(|&(o, a, b)| o == k && a <= t1 && b >= t1 - 200.0)
    };
    ui.horizontal_top(|ui| {
        ui.vertical(|ui| {
            wheels(ui, crank);
            cylinders(ui, crank, &injecting);
            let ever = scope.pulses.iter().any(|&(o, _, _)| (8..12).contains(&o));
            ui.add(
                egui::Label::new(
                    egui::RichText::new(format!(
                        "Cylinder phase assumed: cylinder 1 at 0°, firing 1-3-4-2.\n{}",
                        if ever {
                            "Injector pulses: TO8I–L."
                        } else {
                            "No injector pulses (TO8I–L): the immobilizer\ncuts injection, and it is not emulated."
                        }
                    ))
                    .small()
                    .weak(),
                )
                .wrap(),
            );
        });
        ui.separator();
        egui::ScrollArea::vertical().show(ui, |ui| ui.vertical(|ui| {
            let first = scope.samples.partition_point(|s| s.us < t0).saturating_sub(1);
            let visible: Vec<Sample> = scope.samples.range(first..).copied().collect();
            let width = ui.available_width();
            let left = ui.cursor().left() + 70.0;
            let right = ui.cursor().left() + width - 8.0;
            let x = |t: f64| left + ((t - t0) / (t1 - t0)) as f32 * (right - left);
            let hover = ui
                .ctx()
                .pointer_hover_pos()
                .filter(|p| p.x >= left && p.x <= right && ui.clip_rect().contains(*p))
                .map(|p| t0 + (p.x - left) as f64 / (right - left) as f64 * (t1 - t0));
            let at = |t: f64| -> Option<&Sample> {
                let i = visible.partition_point(|s| s.us < t);
                visible.get(i.min(visible.len().saturating_sub(1)))
            };
            let value_time = hover.unwrap_or(t1);
            ui.label(
                egui::RichText::new(format!(
                    "{:.3} ms of ECU time{}",
                    value_time / 1000.0,
                    if hover.is_some() { " (pointer)" } else { "" }
                ))
                .monospace(),
            );
            if set.teeth {
                teeth(ui, &visible, width, &x, t0, t1, hover);
            }
            if set.pulses {
                pulses(ui, scope, width, &x, t0, t1, hover);
            }
            for (k, (title, traces)) in PANELS.iter().enumerate() {
                if set.panels[k] {
                    let now = at(value_time);
                    strip(ui, title, traces, &visible, now, width, &x, hover);
                }
            }
            axis(ui, width, &x, t0, t1);
        }));
    });
}

fn window_name(ms: f64) -> String {
    if ms >= 1000.0 {
        format!("{} s", ms / 1000.0)
    } else {
        format!("{ms} ms")
    }
}

/// The crank wheel (60-4) and the cam wheel turning under their sensors (the marks at the
/// top), as seen from the front.
fn wheels(ui: &mut egui::Ui, crank: Option<f32>) {
    let (resp, p) = ui.allocate_painter(Vec2::splat(230.0), Sense::hover());
    let c = resp.rect.center();
    let a = crank.unwrap_or(0.0);
    let polar = |deg: f32, r: f32| {
        let t = (deg - 90.0).to_radians();
        c + Vec2::new(t.cos(), t.sin()) * r
    };
    let line = Color32::from_gray(120);
    p.circle_stroke(c, 100.0, Stroke::new(1.0, line));
    // Tooth k reaches the sensor (top) when the crank is at k·6°.
    for k in 0..56 {
        let at = k as f32 * 6.0 - a.rem_euclid(360.0);
        p.line_segment(
            [polar(at, 100.0), polar(at, 110.0)],
            Stroke::new(3.0, Color32::LIGHT_GRAY),
        );
    }
    p.circle_stroke(c, 62.0, Stroke::new(1.0, line));
    for &cam in &CAM {
        // The cam turns at half speed.
        let from = (cam - a) / 2.0;
        let pts: Vec<Pos2> = (0..=6)
            .map(|i| polar(from + i as f32 * CAM_WIDTH / 2.0 / 6.0, 70.0))
            .collect();
        p.add(Shape::line(
            pts,
            Stroke::new(5.0, Color32::from_rgb(255, 200, 80)),
        ));
    }
    let sensor = Color32::from_rgb(255, 90, 90);
    p.line_segment(
        [polar(0.0, 113.0), polar(0.0, 99.0)],
        Stroke::new(2.0, sensor),
    );
    p.line_segment(
        [polar(0.0, 76.0), polar(0.0, 58.0)],
        Stroke::new(2.0, sensor),
    );
    let text = match crank {
        Some(a) => {
            let tooth = (a.rem_euclid(360.0) / 6.0) as u32;
            format!(
                "{a:.1}° of 720\n{}",
                if tooth < 56 {
                    format!("tooth {tooth}")
                } else {
                    "gap".into()
                }
            )
        }
        None => "stopped".into(),
    };
    p.text(
        c,
        Align2::CENTER_CENTER,
        text,
        FontId::monospace(12.0),
        Color32::LIGHT_GRAY,
    );
    p.text(
        resp.rect.left_top(),
        Align2::LEFT_TOP,
        "crank 60-4 / cam",
        FontId::proportional(11.0),
        line,
    );
}

/// The four cylinders: piston height and stroke from the crank angle, and the injector
/// lit while its output (TO8I–L) pulses.
fn cylinders(ui: &mut egui::Ui, crank: Option<f32>, injecting: &dyn Fn(u8) -> bool) {
    let (resp, p) = ui.allocate_painter(Vec2::new(230.0, 120.0), Sense::hover());
    let r = resp.rect;
    let w = r.width() / 4.0;
    for (i, &tdc) in TDC.iter().enumerate() {
        let x = r.left() + w * i as f32;
        let bore = Rect::from_min_size(
            Pos2::new(x + 10.0, r.top() + 18.0),
            Vec2::new(w - 20.0, 70.0),
        );
        p.rect_stroke(
            bore,
            2.0,
            Stroke::new(1.0, Color32::GRAY),
            egui::StrokeKind::Inside,
        );
        let phase = crank.map(|a| (a - tdc).rem_euclid(720.0));
        if let Some(ph) = phase {
            // 0 at top dead centre, 1 at bottom.
            let down = (1.0 - ph.to_radians().cos()) / 2.0;
            let y = bore.top() + 4.0 + down * (bore.height() - 20.0);
            p.rect_filled(
                Rect::from_min_size(
                    Pos2::new(bore.left() + 2.0, y),
                    Vec2::new(bore.width() - 4.0, 14.0),
                ),
                2.0,
                Color32::from_gray(170),
            );
            let stroke = match ph {
                x if x < 180.0 => "power",
                x if x < 360.0 => "exhaust",
                x if x < 540.0 => "intake",
                _ => "compr.",
            };
            p.text(
                Pos2::new(bore.center().x, bore.bottom() + 12.0),
                Align2::CENTER_CENTER,
                stroke,
                FontId::proportional(10.0),
                Color32::LIGHT_GRAY,
            );
        }
        let lit = injecting(8 + i as u8);
        p.circle_filled(
            Pos2::new(bore.center().x, r.top() + 9.0),
            5.0,
            if lit {
                Color32::from_rgb(80, 200, 255)
            } else {
                Color32::from_gray(60)
            },
        );
        p.text(
            Pos2::new(bore.center().x + 12.0, r.top() + 9.0),
            Align2::LEFT_CENTER,
            format!("{}", i + 1),
            FontId::proportional(10.0),
            Color32::GRAY,
        );
    }
}

/// One strip chart: the traces over the window, scaled to what they span.
#[allow(clippy::too_many_arguments)]
fn strip(
    ui: &mut egui::Ui,
    title: &str,
    traces: &[usize],
    visible: &[Sample],
    now: Option<&Sample>,
    width: f32,
    x: &dyn Fn(f64) -> f32,
    hover: Option<f64>,
) {
    ui.horizontal_wrapped(|ui| {
        ui.label(egui::RichText::new(title).strong());
        for (n, &k) in traces.iter().enumerate() {
            let v = now.map_or(f32::NAN, |s| s.v[k]);
            ui.label(
                egui::RichText::new(format!(
                    "■ {} {} {}",
                    TRACES[k].name,
                    value(v),
                    TRACES[k].unit
                ))
                .color(COLORS[n])
                .small(),
            );
        }
    });
    let (resp, p) = ui.allocate_painter(Vec2::new(width, 70.0), Sense::hover());
    let r = resp.rect;
    let (mut lo, mut hi) = (f32::INFINITY, f32::NEG_INFINITY);
    for s in visible {
        for &k in traces {
            if s.v[k].is_finite() {
                lo = lo.min(s.v[k]);
                hi = hi.max(s.v[k]);
            }
        }
    }
    if !lo.is_finite() {
        p.text(
            r.center(),
            Align2::CENTER_CENTER,
            "nothing to show",
            FontId::proportional(11.0),
            Color32::GRAY,
        );
        return;
    }
    if hi - lo < 1e-3 {
        (lo, hi) = (lo - 1.0, hi + 1.0);
    }
    let pad = (hi - lo) * 0.08;
    let (lo, hi) = (lo - pad, hi + pad);
    let plot = Rect::from_x_y_ranges(r.left() + 70.0..=r.right() - 8.0, r.top()..=r.bottom());
    p.rect_filled(plot, 0.0, Color32::from_gray(22));
    let y = |v: f32| plot.bottom() - (v - lo) / (hi - lo) * plot.height();
    for (v, a) in [
        (hi - pad, Align2::RIGHT_TOP),
        (lo + pad, Align2::RIGHT_BOTTOM),
    ] {
        p.text(
            Pos2::new(plot.left() - 4.0, y(v)),
            a,
            value(v),
            FontId::monospace(10.0),
            Color32::GRAY,
        );
    }
    if lo < 0.0 && hi > 0.0 {
        p.line_segment(
            [
                Pos2::new(plot.left(), y(0.0)),
                Pos2::new(plot.right(), y(0.0)),
            ],
            Stroke::new(1.0, Color32::from_gray(60)),
        );
    }
    for (n, &k) in traces.iter().enumerate() {
        // Per pixel column, the first, lowest and highest points: fast moves stay visible.
        let mut pts: Vec<Pos2> = Vec::new();
        let mut col: Option<(i32, f32, f32)> = None;
        let flush = |pts: &mut Vec<Pos2>, c: (i32, f32, f32)| {
            pts.push(Pos2::new(c.0 as f32, y(c.1)));
            if c.2 != c.1 {
                pts.push(Pos2::new(c.0 as f32, y(c.2)));
            }
        };
        for s in visible {
            let v = s.v[k];
            if !v.is_finite() {
                continue;
            }
            let px = x(s.us).clamp(plot.left(), plot.right()) as i32;
            col = match col {
                Some((c, a, b)) if c == px => Some((c, a.min(v), b.max(v))),
                Some(c) => {
                    flush(&mut pts, c);
                    Some((px, v, v))
                }
                None => Some((px, v, v)),
            };
        }
        if let Some(c) = col {
            flush(&mut pts, c);
        }
        if pts.len() > 1 {
            p.add(Shape::line(pts, Stroke::new(1.5, COLORS[n])));
        }
    }
    cursor(&p, plot, x, hover);
}

/// The crank and cam signals over the window, rebuilt from the crank angle of the
/// samples (each tooth high for the first half of its pitch, as `engine::wheel` makes it).
#[allow(clippy::too_many_arguments)]
fn teeth(
    ui: &mut egui::Ui,
    visible: &[Sample],
    width: f32,
    x: &dyn Fn(f64) -> f32,
    t0: f64,
    t1: f64,
    hover: Option<f64>,
) {
    ui.label(egui::RichText::new("Crank (TI0A) and cam (TI0B)").strong());
    let (resp, p) = ui.allocate_painter(Vec2::new(width, 46.0), Sense::hover());
    let r = resp.rect;
    let plot = Rect::from_x_y_ranges(x(t0)..=x(t1), r.top()..=r.bottom());
    p.rect_filled(plot, 0.0, Color32::from_gray(22));
    let lanes = [
        (plot.top() + 4.0, plot.top() + 20.0, Color32::LIGHT_GRAY),
        (
            plot.top() + 26.0,
            plot.top() + 42.0,
            Color32::from_rgb(255, 200, 80),
        ),
    ];
    for (n, label) in ["crank", "cam"].iter().enumerate() {
        p.text(
            Pos2::new(plot.left() - 4.0, (lanes[n].0 + lanes[n].1) / 2.0),
            Align2::RIGHT_CENTER,
            *label,
            FontId::proportional(10.0),
            Color32::GRAY,
        );
    }
    // Teeth turned over the window; too many to draw one by one, a band.
    let turned: f32 = visible
        .windows(2)
        .filter(|w| w[0].crank.is_finite() && w[1].crank.is_finite())
        .map(|w| (w[1].crank - w[0].crank).rem_euclid(720.0))
        .sum();
    if turned / 6.0 > plot.width() / 3.0 {
        for (top, bottom, color) in lanes {
            p.rect_filled(
                Rect::from_x_y_ranges(plot.x_range(), top..=bottom),
                0.0,
                color.gamma_multiply(0.35),
            );
        }
        p.text(
            plot.center(),
            Align2::CENTER_CENTER,
            "too many teeth to draw: shorten the window",
            FontId::proportional(11.0),
            Color32::WHITE,
        );
        return;
    }
    let high = |crank_deg: f32| -> (bool, bool) {
        let a = crank_deg.rem_euclid(720.0);
        let tooth = (a.rem_euclid(360.0) / 6.0) as u32;
        let crank = tooth < 56 && a.rem_euclid(6.0) < 3.0;
        let cam = CAM.iter().any(|&c| (a - c).rem_euclid(720.0) < CAM_WIDTH);
        (crank, cam)
    };
    // Between two samples the angle moves at a steady rate; walk it a quarter tooth at a
    // time and draw the levels.
    let mut was: [Option<bool>; 2] = [None; 2];
    for w in visible.windows(2) {
        let (a0, a1) = (w[0].crank, w[1].crank);
        if !a0.is_finite() || !a1.is_finite() {
            continue;
        }
        let span = (a1 - a0).rem_euclid(720.0);
        let steps = ((span / 1.5).ceil() as usize).max(1);
        for i in 0..steps {
            let f = i as f64 / steps as f64;
            let g = (i + 1) as f64 / steps as f64;
            let ta = w[0].us + (w[1].us - w[0].us) * f;
            let tb = w[0].us + (w[1].us - w[0].us) * g;
            if tb < t0 || ta > t1 {
                continue;
            }
            let (c, k) = high(a0 + span * f as f32 + 0.01);
            let xs = x(ta).max(plot.left())..=x(tb).min(plot.right());
            for (n, (on, (top, bottom, color))) in [c, k].into_iter().zip(lanes).enumerate() {
                let yy = if on { top } else { bottom };
                // The edge, where the level changes.
                if was[n].is_some_and(|w| w != on) {
                    p.line_segment(
                        [Pos2::new(*xs.start(), top), Pos2::new(*xs.start(), bottom)],
                        Stroke::new(1.5, color),
                    );
                }
                was[n] = Some(on);
                p.line_segment(
                    [Pos2::new(*xs.start(), yy), Pos2::new(*xs.end(), yy)],
                    Stroke::new(1.5, color),
                );
            }
        }
    }
    cursor(&p, plot, x, hover);
}

/// The ATU-II's channel 8 one-shot pulses in the window, a lane per output that pulsed,
/// and always the injectors' four (TO8I–L).
fn pulses(
    ui: &mut egui::Ui,
    scope: &Scope,
    width: f32,
    x: &dyn Fn(f64) -> f32,
    t0: f64,
    t1: f64,
    hover: Option<f64>,
) {
    let shown: Vec<&(u8, f64, f64)> = scope
        .pulses
        .iter()
        .filter(|(_, a, b)| *b >= t0 && *a <= t1)
        .collect();
    let mut outputs: Vec<u8> = vec![8, 9, 10, 11];
    for (o, _, _) in &shown {
        if !outputs.contains(o) {
            outputs.push(*o);
        }
    }
    outputs.sort();
    ui.label(egui::RichText::new("One-shot pulses, channel 8 (TO8I–L: injectors)").strong());
    let lane = 12.0;
    let (resp, p) = ui.allocate_painter(
        Vec2::new(width, lane * outputs.len() as f32 + 4.0),
        Sense::hover(),
    );
    let r = resp.rect;
    let plot = Rect::from_x_y_ranges(x(t0)..=x(t1), r.top()..=r.bottom());
    p.rect_filled(plot, 0.0, Color32::from_gray(22));
    for (n, &o) in outputs.iter().enumerate() {
        let top = plot.top() + 2.0 + n as f32 * lane;
        let name = if (8..12).contains(&o) {
            format!("inj {}", o - 7)
        } else {
            format!("TO8{}", (b'A' + o) as char)
        };
        p.text(
            Pos2::new(plot.left() - 4.0, top + lane / 2.0),
            Align2::RIGHT_CENTER,
            name,
            FontId::proportional(10.0),
            Color32::GRAY,
        );
        for &&(k, a, b) in shown.iter().filter(|(k, _, _)| *k == o) {
            let _ = k;
            let xa = x(a).max(plot.left());
            let xb = x(b).min(plot.right()).max(xa + 1.0);
            p.rect_filled(
                Rect::from_x_y_ranges(xa..=xb, top + 1.0..=top + lane - 2.0),
                0.0,
                Color32::from_rgb(80, 200, 255),
            );
        }
    }
    cursor(&p, plot, x, hover);
}

/// The time axis under the charts.
fn axis(ui: &mut egui::Ui, width: f32, x: &dyn Fn(f64) -> f32, t0: f64, t1: f64) {
    let (resp, p) = ui.allocate_painter(Vec2::new(width, 16.0), Sense::hover());
    let r = resp.rect;
    for i in 0..=4 {
        let t = t0 + (t1 - t0) * i as f64 / 4.0;
        let align = match i {
            0 => Align2::LEFT_TOP,
            4 => Align2::RIGHT_TOP,
            _ => Align2::CENTER_TOP,
        };
        p.text(
            Pos2::new(x(t), r.top()),
            align,
            format!("{:.1} ms", t / 1000.0),
            FontId::monospace(10.0),
            Color32::GRAY,
        );
    }
}

/// The pointer's time, as a line across a chart.
fn cursor(p: &egui::Painter, plot: Rect, x: &dyn Fn(f64) -> f32, hover: Option<f64>) {
    if let Some(t) = hover {
        let xx = x(t);
        p.line_segment(
            [Pos2::new(xx, plot.top()), Pos2::new(xx, plot.bottom())],
            Stroke::new(1.0, Color32::from_gray(150)),
        );
    }
}

fn value(v: f32) -> String {
    if !v.is_finite() {
        "—".into()
    } else if v.abs() >= 100.0 {
        format!("{v:.0}")
    } else {
        format!("{v:.2}")
    }
}
