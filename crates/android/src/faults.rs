//! The fault codes, as the app reads them: a summary, a list, and one code in words.
//! They are traced with the maps (see [`crate::maps`]).

use crate::{Live, lock};
use session::dtcs::{Dtc, DtcTrace, reactions_text};
use session::maps::MapTrace;

impl Live {
    /// Runs `f` on the codes being traced, if they are.
    fn with_codes<T>(&self, f: impl FnOnce(&MapTrace, &DtcTrace) -> T) -> Option<T> {
        let slot = lock(&self.maps).clone()?;
        let t = lock(&slot);
        let d = t.dtcs.as_ref()?;
        Some(f(&t, d))
    }

    /// The codes in one line; empty while they are not traced or not known.
    pub fn codes_status(&self) -> String {
        self.with_codes(|_, d| {
            let tests: u64 = d.tested.values().map(|t| t.tests).sum();
            format!(
                "{} codes · {} enabled · {} tested live, {tests} tests · reported: {} confirmed, {} pending",
                d.table.len(),
                d.table.iter().filter(|x| x.enabled()).count(),
                d.tested.len(),
                d.confirmed.len(),
                d.pending.len()
            )
        })
        .unwrap_or_default()
    }

    /// The codes `filter` matches (part of the code or its text), only those failing or
    /// disabled if asked: one per line, `index code enabled fails tests last status text`,
    /// tab-separated (`last` 1 failed, 0 passed, - untested; `status` confirmed, pending
    /// or empty).
    pub fn code_list(&self, filter: &str, failing: bool, disabled: bool) -> String {
        let f = filter.trim().to_uppercase();
        self.with_codes(|_, d| {
            d.table
                .iter()
                .filter(|x| {
                    let t = d.tested.get(&x.index);
                    (f.is_empty()
                        || x.code().contains(&f)
                        || d.text(x.raw).is_some_and(|t| t.to_uppercase().contains(&f)))
                        && (!failing || t.is_some_and(|t| t.last == 1))
                        && (!disabled || !x.enabled())
                })
                .map(|x| {
                    let t = d.tested.get(&x.index);
                    format!(
                        "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
                        x.index,
                        x.code(),
                        x.enabled() as u8,
                        t.map_or(0, |t| t.fails),
                        t.map_or(0, |t| t.tests),
                        t.map_or("-".into(), |t| t.last.to_string()),
                        status(d, x),
                        d.text(x.raw).unwrap_or_default()
                    )
                })
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
    }

    /// One code in words, one fact per line.
    pub fn code_text(&self, index: u16) -> String {
        self.with_codes(|t, d| {
            let Some(x) = d.table.get(index as usize) else {
                return String::new();
            };
            let mut l = vec![format!("{} · index {index}", x.code())];
            if let Some(text) = d.text(x.raw) {
                l.push(text);
            }
            let s = status(d, x);
            if !s.is_empty() {
                l.push(format!("The ECU reports it: {s}"));
            }
            let [a, b, c] = x.enable;
            l.push(format!(
                "{} · A {a:04X}, B {b:04X}, C {c:04X}",
                if x.enabled() {
                    "Enabled"
                } else {
                    "Disabled (A = 0 and C = 0)"
                }
            ));
            l.push(reactions_text(x));
            l.push(d.tested_text(index));
            l.extend(d.conditions_text(index));
            let m = t.masters;
            for mo in d.monitors(index, [m.d3.unwrap_or(u32::MAX), m.d2.unwrap_or(u32::MAX)]) {
                let maps: Vec<String> = mo.maps.iter().map(|d| format!("{d:06X}")).collect();
                l.push(format!(
                    "{} {:06X}, in the function at {:06X}{}",
                    if mo.live {
                        "seen tested from"
                    } else {
                        "reported at"
                    },
                    mo.site,
                    mo.range.0,
                    if maps.is_empty() {
                        ", which reads no map".into()
                    } else {
                        format!(", which reads {}", maps.join(", "))
                    }
                ));
            }
            l.join("\n")
        })
        .unwrap_or_default()
    }

    /// The maps a code's tests read, one hex descriptor per line.
    pub fn code_maps(&self, index: u16) -> String {
        self.with_codes(|t, d| {
            let m = t.masters;
            let mut out: Vec<u32> = d
                .monitors(index, [m.d3.unwrap_or(u32::MAX), m.d2.unwrap_or(u32::MAX)])
                .into_iter()
                .flat_map(|mo| mo.maps)
                .collect();
            out.sort_unstable();
            out.dedup();
            out.iter()
                .map(|d| format!("{d:06X}"))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
    }
}

/// Confirmed, pending, or empty.
fn status(d: &DtcTrace, x: &Dtc) -> &'static str {
    if d.confirmed.contains(&x.raw) {
        "confirmed"
    } else if d.pending.contains(&x.raw) {
        "pending"
    } else {
        ""
    }
}
