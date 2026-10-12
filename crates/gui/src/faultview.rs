//! The fault codes page of the maps window: the calibration's codes as its flash holds
//! them (enabled or not, what the ECU does while one is active), what their tests do
//! live, which the ECU reports, and for each one where it is tested and which maps that
//! test reads.

use crate::mapview::Action;
use eframe::egui::{self, Color32, RichText};
use session::Watch;
use session::dtcs::{Dtc, DtcTrace, reactions_known, read_texts};
use session::maps::MapTrace;

/// What the page has chosen.
#[derive(Default)]
pub struct FaultView {
    /// The code shown (its index).
    pub(crate) selected: Option<u16>,
    filter: String,
    failing: bool,
    disabled: bool,
    note: Option<String>,
}

/// The status of a code as the ECU last answered it.
fn status(d: &DtcTrace, x: &Dtc) -> Option<&'static str> {
    if d.confirmed.contains(&x.raw) {
        Some("confirmed")
    } else if d.pending.contains(&x.raw) {
        Some("pending")
    } else {
        None
    }
}

/// Draws the page. `go_map` is set to a map to show on the maps page.
pub fn show(
    ui: &mut egui::Ui,
    trace: &MapTrace,
    v: &mut FaultView,
    watch: Option<Watch>,
    actions: &mut Vec<Action>,
    go_map: &mut Option<u32>,
) {
    let Some(d) = &trace.dtcs else {
        ui.label(
            "The fault codes' tables are known for the Astra J's calibration (NCLSW420AX36) \
             only; this flash is another.",
        );
        return;
    };
    let tested: u64 = d.tested.values().map(|t| t.tests).sum();
    ui.horizontal_wrapped(|ui| {
        ui.label(format!(
            "{} codes · {} enabled · {} tested live, {tested} tests · reported: {} confirmed, {} pending",
            d.table.len(),
            d.table.iter().filter(|x| x.enabled()).count(),
            d.tested.len(),
            d.confirmed.len(),
            d.pending.len(),
        ));
        if ui
            .button("Read the codes")
            .on_hover_text("Ask the ECU for its confirmed (03) and pending (07) codes")
            .clicked()
        {
            actions.push(Action::ReadCodes);
        }
        if ui
            .button("Read descriptions…")
            .on_hover_text("Your own file: one code per line, then its text")
            .clicked()
            && let Some(path) = rfd::FileDialog::new().set_title("Code descriptions").pick_file()
        {
            match std::fs::read_to_string(&path) {
                Ok(t) => {
                    let texts = read_texts(&t);
                    v.note = Some(format!("{} descriptions read", texts.len()));
                    actions.push(Action::Texts(texts));
                }
                Err(e) => v.note = Some(e.to_string()),
            }
        }
        if let Some(n) = &v.note {
            ui.label(RichText::new(n).weak());
        }
    });
    ui.separator();
    ui.horizontal_top(|ui| {
        ui.vertical(|ui| {
            ui.set_width(470.0);
            list(ui, d, v);
        });
        ui.separator();
        ui.vertical(|ui| match v.selected {
            Some(i) if (i as usize) < d.table.len() => {
                detail(ui, trace, d, i, watch, actions, go_map)
            }
            _ => {
                ui.label("Choose a code on the left.");
            }
        });
    });
}

/// The codes, filtered.
fn list(ui: &mut egui::Ui, d: &DtcTrace, v: &mut FaultView) {
    ui.horizontal(|ui| {
        ui.label("Filter");
        ui.add(egui::TextEdit::singleline(&mut v.filter).desired_width(90.0))
            .on_hover_text("Part of the code or of its description");
        ui.checkbox(&mut v.failing, "failing")
            .on_hover_text("Codes whose last test failed");
        ui.checkbox(&mut v.disabled, "disabled")
            .on_hover_text("Codes the calibration disables (A = 0 and C = 0)");
    });
    let f = v.filter.trim().to_uppercase();
    let rows: Vec<&Dtc> = d
        .table
        .iter()
        .filter(|x| {
            let t = d.tested.get(&x.index);
            (f.is_empty()
                || x.code().contains(&f)
                || d.text(x.raw).is_some_and(|t| t.to_uppercase().contains(&f)))
                && (!v.failing || t.is_some_and(|t| t.last == 1))
                && (!v.disabled || !x.enabled())
        })
        .collect();
    let mono = egui::TextStyle::Monospace;
    let h = ui.text_style_height(&mono);
    egui::ScrollArea::vertical()
        .id_salt("fault_list")
        .max_height(620.0)
        .auto_shrink([false, false])
        .show_rows(ui, h, rows.len(), |ui, range| {
            for x in &rows[range] {
                let t = d.tested.get(&x.index);
                let tests = t.map_or("-".into(), |t| format!("{}/{}", t.fails, t.tests));
                let mut text = format!(
                    "{} {:>3} {} {:>11}",
                    x.code(),
                    x.index,
                    if x.enabled() { "on " } else { "OFF" },
                    tests
                );
                if let Some(s) = status(d, x) {
                    text.push_str(&format!(" {s}"));
                }
                if let Some(t) = d.text(x.raw) {
                    text.push_str("  ");
                    text.push_str(&t.chars().take(30).collect::<String>());
                }
                let mut rt = RichText::new(text).monospace();
                if status(d, x).is_some() {
                    rt = rt.color(Color32::from_rgb(255, 170, 80));
                } else if t.is_some_and(|t| t.last == 1) {
                    rt = rt.color(Color32::from_rgb(230, 210, 110));
                } else if !x.enabled() {
                    rt = rt.weak();
                }
                if ui
                    .selectable_label(v.selected == Some(x.index), rt)
                    .clicked()
                {
                    v.selected = Some(x.index);
                }
            }
        });
}

/// One code: what it is, what the calibration and the ECU do with it, and where it is
/// tested.
fn detail(
    ui: &mut egui::Ui,
    trace: &MapTrace,
    d: &DtcTrace,
    i: u16,
    watch: Option<Watch>,
    actions: &mut Vec<Action>,
    go_map: &mut Option<u32>,
) {
    let x = &d.table[i as usize];
    ui.heading(format!("{} · index {i}", x.code()));
    if let Some(t) = d.text(x.raw) {
        ui.label(RichText::new(t).strong());
    }
    if let Some(s) = status(d, x) {
        ui.colored_label(
            Color32::from_rgb(255, 170, 80),
            format!("The ECU reports it: {s}"),
        );
    }
    let [a, b, c] = x.enable;
    ui.label(format!(
        "{} by the calibration · A {a:04X}, B {b:04X}, C {c:04X}",
        if x.enabled() {
            "Enabled"
        } else {
            "Disabled (A = 0 and C = 0)"
        }
    ));
    let [r1, r2, r3] = x.reactions;
    let known = reactions_known(x.reactions);
    ui.label(format!(
        "While active: masks {r1:04X} {r2:04X} {r3:04X}{}",
        if known.is_empty() {
            String::new()
        } else {
            format!(" · {}", known.join(", "))
        }
    ));
    match d.tested.get(&i) {
        Some(t) => {
            ui.label(format!(
                "Tested {} times while traced, failed {}; the last test {}",
                t.tests,
                t.fails,
                if t.last == 1 { "failed" } else { "passed" }
            ));
        }
        None => {
            ui.label("Not tested while traced: its test needs other conditions.");
        }
    }
    ui.horizontal(|ui| {
        if watch == Some(Watch::Test(i)) {
            if ui.button("Stopping at its next test · cancel").clicked() {
                actions.push(Action::BreakTest(None));
            }
        } else if ui
            .button("Stop at its next test")
            .on_hover_text("Run, and stop where the firmware next reports this code's test")
            .clicked()
        {
            actions.push(Action::BreakTest(Some(i)));
        }
    });
    ui.separator();
    let m = trace.masters;
    let monitors = d.monitors(i, [m.d3.unwrap_or(u32::MAX), m.d2.unwrap_or(u32::MAX)]);
    if monitors.is_empty() {
        ui.label(
            "No place reports it with a constant index, and its test has not been seen: the \
             index is worked out at run time.",
        );
    }
    for mo in monitors {
        ui.horizontal_wrapped(|ui| {
            ui.label(if mo.live {
                "Seen tested from"
            } else {
                "Reported at"
            });
            if ui
                .link(format!("{:06X}", mo.site))
                .on_hover_text("Open in the code view")
                .clicked()
            {
                actions.push(Action::GoTo(mo.site));
            }
            ui.label("in the function at");
            if ui.link(format!("{:06X}", mo.range.0)).clicked() {
                actions.push(Action::GoTo(mo.range.0));
            }
            if mo.maps.is_empty() {
                ui.label("· reads no map");
            } else {
                ui.label("· reads");
                for desc in &mo.maps {
                    let t = trace
                        .title(*desc)
                        .map_or(String::new(), |t| format!(" ({t})"));
                    if ui.link(format!("map {desc:06X}{t}")).clicked() {
                        *go_map = Some(*desc);
                    }
                }
            }
        });
    }
}
