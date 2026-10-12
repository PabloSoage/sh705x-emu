//! The oscilloscope window: the ECU's pins as waveforms (inputs, PWM outputs, one-shot
//! outputs), on a time base of 10 µs to 50 ms a division, held still on an edge of any of
//! them, with each channel's frequency and duty measured.

use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, RichText, Sense, Stroke, Vec2};
use session::osc::{CHANNELS, Osc, measure};

/// Microseconds a division.
const TIMEBASES: [f64; 12] = [
    10.0, 20.0, 50.0, 100.0, 200.0, 500.0, 1000.0, 2000.0, 5000.0, 10_000.0, 20_000.0, 50_000.0,
];
/// Divisions across.
const DIVS: f64 = 10.0;
/// CPU cycles a microsecond.
const PER_US: f64 = 80.0;

/// How the trigger holds the picture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    /// On the last trigger, or running free without one.
    Auto,
    /// Only on a trigger: the last picture stays until the next.
    Normal,
    /// The first trigger, then held.
    Single,
}

/// What the viewer has chosen.
pub struct OscView {
    /// Channels shown, by [`CHANNELS`] index.
    shown: [bool; CHANNELS.len()],
    timebase: usize,
    trigger: usize,
    rising: bool,
    mode: Mode,
    /// Fraction of the window before the trigger.
    pre: f64,
    /// Held: the picture does not move.
    held: bool,
    /// The window shown last (cycles), for Normal and Single and while held.
    window: Option<(u64, u64, Option<u64>)>,
}

impl Default for OscView {
    fn default() -> Self {
        let mut shown = [false; CHANNELS.len()];
        // Crank, cam, SCV, VGT, EGR close and open, injector 1.
        for k in [0, 1, 3, 7, 9, 10, 11] {
            shown[k] = true;
        }
        Self {
            shown,
            timebase: 6,
            trigger: 0,
            rising: true,
            mode: Mode::Auto,
            pre: 0.2,
            held: false,
            window: None,
        }
    }
}

/// A time in µs as a scope prints it.
fn time(us: f64) -> String {
    let a = us.abs();
    if a >= 1000.0 {
        format!("{:.3} ms", us / 1000.0)
    } else {
        format!("{us:.1} µs")
    }
}

/// Lane colours.
const COLOURS: [Color32; 6] = [
    Color32::from_rgb(255, 220, 90),
    Color32::from_rgb(110, 200, 255),
    Color32::from_rgb(255, 120, 200),
    Color32::from_rgb(120, 230, 140),
    Color32::from_rgb(255, 160, 80),
    Color32::from_rgb(190, 160, 255),
];

/// Draws the window.
pub fn show(ui: &mut egui::Ui, osc: &Osc, v: &mut OscView) {
    ui.horizontal_wrapped(|ui| {
        ui.label("Time base");
        egui::ComboBox::from_id_salt("osc_tb")
            .selected_text(format!("{}/div", time(TIMEBASES[v.timebase])))
            .show_ui(ui, |ui| {
                for (k, tb) in TIMEBASES.iter().enumerate() {
                    ui.selectable_value(&mut v.timebase, k, format!("{}/div", time(*tb)));
                }
            });
        ui.separator();
        ui.label("Trigger");
        egui::ComboBox::from_id_salt("osc_trig")
            .selected_text(CHANNELS[v.trigger].1)
            .show_ui(ui, |ui| {
                for (k, (_, name)) in CHANNELS.iter().enumerate() {
                    ui.selectable_value(&mut v.trigger, k, *name);
                }
            });
        ui.selectable_value(&mut v.rising, true, "rising");
        ui.selectable_value(&mut v.rising, false, "falling");
        for (m, name) in [
            (Mode::Auto, "auto"),
            (Mode::Normal, "normal"),
            (Mode::Single, "single"),
        ] {
            if ui.selectable_label(v.mode == m, name).clicked() {
                v.mode = m;
                v.window = None;
            }
        }
        ui.add(egui::Slider::new(&mut v.pre, 0.0..=1.0).text("before"))
            .on_hover_text("How much of the window is before the trigger");
        ui.toggle_value(&mut v.held, "⏸ hold");
    });
    ui.horizontal_wrapped(|ui| {
        ui.label("Channels");
        for (k, (_, name)) in CHANNELS.iter().enumerate() {
            ui.checkbox(&mut v.shown[k], *name);
        }
    });
    ui.separator();

    // The window: on the trigger, or running free, or as it was.
    let span = (TIMEBASES[v.timebase] * DIVS * PER_US) as u64;
    let before = (span as f64 * v.pre) as u64;
    let hold = v.held || (v.mode == Mode::Single && v.window.is_some());
    if !hold {
        match osc.trigger(CHANNELS[v.trigger].0, v.rising, before, span - before) {
            Some(t) => v.window = Some((t - before, t - before + span, Some(t))),
            None if v.mode == Mode::Auto => {
                let t1 = osc.now;
                v.window = Some((t1.saturating_sub(span), t1, None));
            }
            None => {}
        }
    }
    let Some((t0, t1, trig)) = v.window else {
        ui.label("Waiting for a trigger…");
        return;
    };
    // Keep the time base when it changes while held.
    let (t0, t1) = if t1 - t0 == span {
        (t0, t1)
    } else {
        let at = trig.unwrap_or(t1);
        (at.saturating_sub(before), at.saturating_sub(before) + span)
    };

    let shown: Vec<usize> = (0..CHANNELS.len()).filter(|&k| v.shown[k]).collect();
    let lane = 46.0;
    let size = Vec2::new(
        ui.available_width().max(400.0),
        lane * shown.len().max(1) as f32 + 24.0,
    );
    let (resp, p) = ui.allocate_painter(size, Sense::hover());
    let r = resp.rect;
    let plot = Rect::from_min_max(r.min + Vec2::new(130.0, 0.0), r.max - Vec2::new(4.0, 22.0));
    p.rect_filled(plot, 0.0, Color32::from_rgb(10, 14, 12));
    let grid = Stroke::new(1.0, Color32::from_rgb(30, 50, 40));
    for d in 0..=DIVS as usize {
        let x = plot.left() + plot.width() * d as f32 / DIVS as f32;
        p.line_segment(
            [Pos2::new(x, plot.top()), Pos2::new(x, plot.bottom())],
            grid,
        );
    }
    let x = |t: u64| plot.left() + (t.saturating_sub(t0)) as f32 / span as f32 * plot.width();
    let font = FontId::proportional(11.0);
    let mut rows = Vec::new();
    for (n, &k) in shown.iter().enumerate() {
        let (ch, name) = CHANNELS[k];
        let colour = COLOURS[n % COLOURS.len()];
        let top = plot.top() + n as f32 * lane + 8.0;
        let (hi, lo) = (top + 4.0, top + lane - 14.0);
        p.line_segment(
            [
                Pos2::new(plot.left(), top + lane - 4.0),
                Pos2::new(plot.right(), top + lane - 4.0),
            ],
            grid,
        );
        p.text(
            Pos2::new(r.left(), (hi + lo) / 2.0),
            Align2::LEFT_CENTER,
            name,
            font.clone(),
            colour,
        );
        let t = osc.trace(ch, t0, t1);
        let y = |l: bool| if l { hi } else { lo };
        let mut pts = vec![Pos2::new(plot.left(), y(t.initial))];
        let mut level = t.initial;
        for &(at, l) in &t.edges {
            let ex = x(at);
            pts.push(Pos2::new(ex, y(level)));
            pts.push(Pos2::new(ex, y(l)));
            level = l;
        }
        pts.push(Pos2::new(plot.right(), y(level)));
        p.add(egui::Shape::line(pts, Stroke::new(1.6, colour)));
        rows.push((name, colour, measure(&t, t0, t1), t.edges.len()));
    }
    if let Some(t) = trig {
        let tx = x(t);
        p.line_segment(
            [Pos2::new(tx, plot.top()), Pos2::new(tx, plot.bottom())],
            Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 255, 255, 90)),
        );
        p.text(
            Pos2::new(tx + 3.0, plot.top() + 2.0),
            Align2::LEFT_TOP,
            "T",
            font.clone(),
            Color32::WHITE,
        );
    }
    // The time axis, from the trigger (or the window's end).
    let zero = trig.unwrap_or(t1);
    for d in 0..=DIVS as usize {
        let t = t0 + span * d as u64 / DIVS as u64;
        let us = (t as f64 - zero as f64) / PER_US;
        p.text(
            Pos2::new(
                plot.left() + plot.width() * d as f32 / DIVS as f32,
                plot.bottom() + 4.0,
            ),
            Align2::CENTER_TOP,
            time(us),
            FontId::proportional(10.0),
            Color32::GRAY,
        );
    }
    if let Some(h) = resp.hover_pos()
        && plot.contains(h)
    {
        p.line_segment(
            [Pos2::new(h.x, plot.top()), Pos2::new(h.x, plot.bottom())],
            Stroke::new(1.0, Color32::from_rgba_unmultiplied(120, 220, 255, 110)),
        );
        let t = t0 as f64 + (h.x - plot.left()) as f64 / plot.width() as f64 * span as f64;
        resp.on_hover_text(time((t - zero as f64) / PER_US));
    }
    ui.add_space(4.0);
    egui::Grid::new("osc_measures")
        .striped(true)
        .show(ui, |ui| {
            for h in ["channel", "frequency", "period", "duty", "edges"] {
                ui.label(RichText::new(h).strong());
            }
            ui.end_row();
            for (name, colour, m, edges) in rows {
                ui.colored_label(colour, name);
                match m {
                    Some(m) => {
                        ui.label(format!("{:.2} Hz", PER_US * 1e6 / m.period));
                        ui.label(time(m.period / PER_US));
                        ui.label(format!("{:.1} %", m.duty * 100.0));
                    }
                    None => {
                        for _ in 0..3 {
                            ui.label("-");
                        }
                    }
                }
                ui.label(edges.to_string());
                ui.end_row();
            }
        });
}
