//! The fault codes (DTCs) of a calibration, as its flash holds them, and their tests.
//!
//! In the Astra's calibration (NCLSW420AX36, resultados fases 9, 10 and 13) the codes are a
//! table of 202 words, the same index into three tables A, B and C that say whether each is
//! enabled (A = 0 and C = 0 disables it; B alone does not), and into three tables of masks
//! that say what the ECU does while it is active (raise the idle, cut the EGR...). Every
//! test reports its result through one routine, with the code's index in R4 and the result
//! in R5: [`report_sites`] finds the places that report each index, [`function_around`]
//! the function they are in, and [`maps_in`] the maps that function reads.

use crate::names::ASTRA;
use sh7059::Call;
use std::collections::BTreeMap;
use std::sync::Arc;

/// Where one calibration keeps its fault codes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Layout {
    /// Codes.
    pub count: usize,
    /// The codes, one word each.
    pub codes: u32,
    /// Tables A, B and C, one word per code.
    pub enable: [u32; 3],
    /// The three tables of reaction masks, one word per code.
    pub reactions: [u32; 3],
    /// The routine every test reports through: index in R4, result in R5.
    pub reporter: u32,
}

/// The Astra J's (resultados fases 9, 10 and 13).
pub const ASTRA_LAYOUT: Layout = Layout {
    count: 202,
    codes: 0x0012_40C4,
    enable: [0x0012_374C, 0x0012_38E0, 0x0012_3A74],
    reactions: [0x0012_4258, 0x0012_43EC, 0x0012_4580],
    reporter: 0x0003_378A,
};

/// The layout for calibration `id`, if known.
pub fn layout(id: Option<&str>) -> Option<Layout> {
    (id == Some(ASTRA)).then_some(ASTRA_LAYOUT)
}

/// One fault code.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Dtc {
    /// Its index in the tables.
    pub index: u16,
    /// The two bytes the ECU reports.
    pub raw: u16,
    /// Tables A, B and C.
    pub enable: [u16; 3],
    /// Its reaction masks.
    pub reactions: [u16; 3],
}

impl Dtc {
    /// The code as a garage reads it: P0400, U0121.
    pub fn code(&self) -> String {
        code(self.raw)
    }

    /// Enabled: A or C not 0 (fase 10).
    pub fn enabled(&self) -> bool {
        self.enable[0] != 0 || self.enable[2] != 0
    }
}

/// Two bytes as a code: the top two bits the system (P, C, B, U), then four digits.
pub fn code(raw: u16) -> String {
    format!(
        "{}{}{:03X}",
        ['P', 'C', 'B', 'U'][(raw >> 14) as usize],
        (raw >> 12) & 3,
        raw & 0xFFF
    )
}

/// What one active code makes the ECU do, by bit of its masks, where known (fase 9 §7).
pub fn reactions_known(m: [u16; 3]) -> Vec<&'static str> {
    let mut out = Vec::new();
    if m[2] & 0x0002 != 0 {
        out.push("raises the idle (1000 rpm)");
    }
    out
}

/// The fault codes in `flash` as `l` lays them out.
pub fn read_table(flash: &[u8], l: &Layout) -> Vec<Dtc> {
    (0..l.count as u32)
        .map(|k| Dtc {
            index: k as u16,
            raw: word(flash, l.codes + 2 * k),
            enable: l.enable.map(|t| word(flash, t + 2 * k)),
            reactions: l.reactions.map(|t| word(flash, t + 2 * k)),
        })
        .collect()
}

fn word(flash: &[u8], a: u32) -> u16 {
    let a = a as usize;
    flash
        .get(a..a + 2)
        .map_or(0, |b| u16::from_be_bytes([b[0], b[1]]))
}

fn long(flash: &[u8], a: u32) -> u32 {
    (word(flash, a) as u32) << 16 | word(flash, a + 2) as u32
}

/// Instructions read back from a call for the loads of its target and R4.
const LOOK_BACK: u32 = 16;

/// The value register `r` is given just before `at` by a constant load: `MOV #imm`
/// (with an `EXTU.B` after it, unsigned), `MOV.W @(disp,PC)` or `MOV.L @(disp,PC)`; or the
/// delay slot `slot` sets it.
fn constant(flash: &[u8], at: u32, r: u16, slot: Option<u32>) -> Option<u32> {
    let decode = |a: u32| -> Option<Option<u32>> {
        let op = word(flash, a);
        if (op >> 8) & 0xF != r {
            return None;
        }
        Some(match op >> 12 {
            0xE => {
                let v = (op & 0xFF) as i8 as i32;
                // MOV #imm,Rr; EXTU.B Rr,Rr: an index past 127.
                let next = word(flash, a + 2);
                Some(if v < 0 && next == 0x600C | r << 8 | r << 4 {
                    (op & 0xFF) as u32
                } else {
                    v as u32
                })
            }
            0x9 => Some(word(flash, (a & !1) + 4 + (op & 0xFF) as u32 * 2) as i16 as i32 as u32),
            0xD => Some(long(flash, (a & !3) + 4 + (op & 0xFF) as u32 * 4)),
            // Anything else that writes Rr.
            0x6 | 0x3 | 0x7 | 0x5 => None,
            _ => return None,
        })
    };
    if let Some(s) = slot
        && let Some(v) = decode(s)
    {
        return v;
    }
    for k in 1..=LOOK_BACK {
        let Some(a) = at.checked_sub(2 * k) else {
            break;
        };
        let op = word(flash, a);
        // EXTU.B Rr,Rr after the load is part of it.
        if op == 0x600C | r << 8 | r << 4 {
            continue;
        }
        if let Some(v) = decode(a) {
            return v;
        }
    }
    None
}

/// The target of the call at `a` (`JSR @Rm`, `JMP @Rm` or `BSR`), if its register is a
/// constant.
fn call_target(flash: &[u8], a: u32) -> Option<u32> {
    let op = word(flash, a);
    if op & 0xF0FF == 0x400B || op & 0xF0FF == 0x402B {
        constant(flash, a, (op >> 8) & 0xF, None)
    } else if op & 0xF000 == 0xB000 {
        let d = ((op & 0xFFF) << 4) as i16 >> 4;
        Some(a.wrapping_add(4).wrapping_add((2 * d as i32) as u32))
    } else {
        None
    }
}

/// The places in `flash` that report a fault code through `reporter`, by index: every
/// call (or jump) to it with a constant in R4.
pub fn report_sites(flash: &[u8], reporter: u32, count: usize) -> BTreeMap<u16, Vec<u32>> {
    let mut out: BTreeMap<u16, Vec<u32>> = BTreeMap::new();
    for a in (0..flash.len().saturating_sub(4) as u32).step_by(2) {
        if call_target(flash, a) != Some(reporter) {
            continue;
        }
        if let Some(i) = constant(flash, a, 4, Some(a + 2))
            && (i as usize) < count
        {
            out.entry(i as u16).or_default().push(a);
        }
    }
    out
}

/// The function `at` is in, roughly: from just after the return before it to the end of
/// the return after it (a `RTS`, or a `JMP` whose delay slot restores PR).
pub fn function_around(flash: &[u8], at: u32) -> (u32, u32) {
    let ends = |a: u32| {
        let op = word(flash, a);
        op == 0x000B || (op & 0xF0FF == 0x402B && word(flash, a + 2) == 0x4F26)
    };
    let mut start = at;
    while start >= 2 && !ends(start - 2) {
        start -= 2;
    }
    // Past the delay slot of the return before.
    start = (start + 2).min(at);
    let mut end = at;
    while (end as usize) + 4 < flash.len() && !ends(end) {
        end += 2;
    }
    (start, end + 4)
}

/// The maps the code in `range` reads through `masters`: every call to one of them with a
/// constant descriptor in R4.
pub fn maps_in(flash: &[u8], range: (u32, u32), masters: [u32; 2]) -> Vec<u32> {
    let mut out = Vec::new();
    for a in (range.0..range.1).step_by(2) {
        if call_target(flash, a).is_some_and(|t| masters.contains(&t))
            && let Some(d) = constant(flash, a, 4, Some(a + 2))
            && !out.contains(&d)
        {
            out.push(d);
        }
    }
    out
}

/// What the ECU has done with one code while traced.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Tested {
    /// Tests reported.
    pub tests: u64,
    /// Of them, failed (R5 = 1).
    pub fails: u64,
    /// The last result (R5: 0 passed, 1 failed).
    pub last: u32,
    /// Cycle of the last test.
    pub at: u64,
    /// Tests per return address seen (the caller of the reporter, or, when the test
    /// jumps to it, the caller of the test).
    pub callers: BTreeMap<u32, u64>,
}

/// The fault codes of a calibration, and what its tests do while traced.
#[derive(Clone, Debug)]
pub struct DtcTrace {
    /// Where the calibration keeps them.
    pub layout: Layout,
    /// The table.
    pub table: Vec<Dtc>,
    /// The places that report each index with a constant, read from the flash.
    pub reports: BTreeMap<u16, Vec<u32>>,
    /// Per index, what its tests have done.
    pub tested: BTreeMap<u16, Tested>,
    /// The codes the ECU last answered as confirmed (mode 03).
    pub confirmed: Vec<u16>,
    /// The codes the ECU last answered as pending (mode 07).
    pub pending: Vec<u16>,
    /// Descriptions by code (P0400) from the user's own file; [`describe`] has the ones
    /// that come with the emulator.
    pub texts: BTreeMap<String, String>,
    flash: Arc<Vec<u8>>,
}

impl DtcTrace {
    /// The codes of `flash`, laid out as `layout`.
    pub fn new(flash: Arc<Vec<u8>>, layout: Layout) -> Self {
        Self {
            layout,
            table: read_table(&flash, &layout),
            reports: report_sites(&flash, layout.reporter, layout.count),
            tested: BTreeMap::new(),
            confirmed: Vec::new(),
            pending: Vec::new(),
            texts: BTreeMap::new(),
            flash,
        }
    }

    /// Takes one call to the reporter.
    pub fn add(&mut self, c: &Call) {
        if c.r4 as usize >= self.layout.count {
            return;
        }
        let t = self.tested.entry(c.r4 as u16).or_default();
        t.tests += 1;
        if c.r5 == 1 {
            t.fails += 1;
        }
        t.last = c.r5;
        t.at = c.cycles;
        *t.callers.entry(c.ret).or_default() += 1;
    }

    /// The code's text: the user's, else the emulator's, else none.
    pub fn text(&self, raw: u16) -> Option<String> {
        let c = code(raw);
        self.texts
            .get(&c)
            .cloned()
            .or_else(|| describe(&c).map(str::to_string))
    }

    /// Each place that reports `index`, the function it is in, and the maps that function
    /// reads (through `masters`): the places found in the flash, then the callers seen
    /// live.
    pub fn monitors(&self, index: u16, masters: [u32; 2]) -> Vec<Monitor> {
        let mut out: Vec<Monitor> = Vec::new();
        let mut add = |site: u32, live: bool| {
            let range = function_around(&self.flash, site);
            if out.iter().any(|m| m.range == range) {
                return;
            }
            out.push(Monitor {
                site,
                range,
                maps: maps_in(&self.flash, range, masters),
                live,
            });
        };
        for &a in self.reports.get(&index).into_iter().flatten() {
            add(a, false);
        }
        let callers = self.tested.get(&index).map(|t| t.callers.keys());
        for &ret in callers.into_iter().flatten() {
            add(ret.wrapping_sub(4), true);
        }
        out
    }
}

/// A place a code's test reports from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Monitor {
    /// The call (or jump) to the reporter; for one seen live, the call before the return
    /// address.
    pub site: u32,
    /// The function around it.
    pub range: (u32, u32),
    /// The maps that function reads.
    pub maps: Vec<u32>,
    /// Seen live rather than read from the flash.
    pub live: bool,
}

/// Descriptions of codes this project has met (resultados fases 9, 10, 13), in its own
/// words.
pub fn describe(code: &str) -> Option<&'static str> {
    Some(match code {
        "P0016" => "crankshaft and camshaft positions do not agree",
        "P0101" => "mass air flow sensor range or performance",
        "P0102" => "mass air flow sensor reads low",
        "P0103" => "mass air flow sensor reads high",
        "P0116" | "P0117" | "P0118" => "engine coolant temperature sensor",
        "P0133" => "oxygen sensor slow to respond",
        "P0201" | "P0202" | "P0203" | "P0204" => "injector circuit",
        "P0217" => "engine over temperature",
        "P0383" | "P0384" => "glow plug control module",
        "P0400" => "EGR flow",
        "P0404" => "EGR control range or performance",
        "P0545" | "P0546" => "exhaust gas temperature sensor 1",
        "P0611" => "fuel injector control module",
        "P0646" | "P0647" => "A/C compressor relay",
        "P0652" => "sensor reference voltage B low",
        "P0686" | "P0687" => "ECM main power relay",
        "P0691" | "P0692" => "cooling fan 1 control",
        "P0693" | "P0694" => "cooling fan 2 control",
        "P0695" | "P0696" => "cooling fan 3 control",
        "P060E" => "throttle position performance",
        "P160D" => "immobilizer: the ECU is not authorized",
        "P2002" | "P2003" => "particulate filter efficiency",
        "P2032" | "P2033" => "exhaust gas temperature sensor 2",
        "P2102" | "P2103" => "throttle actuator motor",
        "P2138" => "accelerator pedal sensors disagree",
        "P2147" | "P2148" => "injector supply voltage",
        "P2262" => "boost pressure not detected",
        "P242F" => "particulate filter restriction (ash)",
        "P244A" | "P244B" => "particulate filter differential pressure",
        "P2454" | "P2455" => "particulate filter differential pressure sensor",
        "P245C" | "P245D" => "EGR cooler bypass control",
        "P2688" | "P2689" => "fuel heater control",
        "U0121" => "lost communication with the ABS",
        _ => return None,
    })
}

/// Reads descriptions from a file: one per line, a code, a tab or spaces, its text.
pub fn read_texts(text: &str) -> BTreeMap<String, String> {
    text.lines()
        .filter_map(|l| {
            let (c, t) = l.trim().split_once(['\t', ' '])?;
            let c = c.trim().to_uppercase();
            (c.len() == 5 && "PCBU".contains(&c[..1])).then(|| (c, t.trim().to_string()))
        })
        .collect()
}

/// The codes in a mode 03 or 07 answer (the service byte, a count, then two bytes each).
pub fn codes_in(answer: &[u8]) -> Vec<u16> {
    answer
        .get(2..)
        .unwrap_or_default()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|b| u16::from_be_bytes(*b))
        .filter(|&c| c != 0)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flash() -> Option<Vec<u8>> {
        std::fs::read(std::env::var("SH705X_FLASH").ok()?).ok()
    }

    #[test]
    fn descriptions_and_answers_read() {
        let t = read_texts("P0400\tEGR flow, mine\nnonsense\nU0121  the ABS is gone\n");
        assert_eq!(t["P0400"], "EGR flow, mine");
        assert_eq!(t["U0121"], "the ABS is gone");
        assert_eq!(t.len(), 2);
        assert_eq!(
            codes_in(&[0x43, 0x02, 0x04, 0x00, 0x21, 0x03]),
            [0x0400, 0x2103]
        );
        assert!(codes_in(&[0x47, 0x00]).is_empty());
    }

    #[test]
    fn codes_read_as_the_ecu_reports_them() {
        assert_eq!(code(0x0400), "P0400");
        assert_eq!(code(0x2103), "P2103");
        assert_eq!(code(0xC121), "U0121");
        assert_eq!(code(0x160D), "P160D");
    }

    /// The Astra's table: the indices this project has seen (P060E at 0x6F, P160D at 197,
    /// resultados fase 9), and the reports found where fase 9 found them.
    #[test]
    fn the_astra_table_and_its_reports() {
        let Some(f) = flash() else { return };
        let t = read_table(&f, &ASTRA_LAYOUT);
        assert_eq!(t.len(), 202);
        assert_eq!(t[0x6F].code(), "P060E");
        assert_eq!(t[197].code(), "P160D");
        let enabled = t.iter().filter(|d| d.enabled()).count();
        let reporting = report_sites(&f, ASTRA_LAYOUT.reporter, 202);
        eprintln!(
            "{enabled} of 202 enabled; {} codes reported from {} places",
            reporting.len(),
            reporting.values().map(Vec::len).sum::<usize>()
        );
        // P060E: 0x5DA4C's function reports it (fase 9).
        let p060e = &reporting[&0x6F];
        assert!(
            p060e.iter().any(|&a| {
                let (s, e) = function_around(&f, a);
                (s..e).contains(&0x5DA4C)
            }),
            "{p060e:x?}"
        );
    }

    /// Traced on the Astra: the tests run, a provoked fault fails its test and becomes a
    /// code the ECU reports, and a stop at that code's next test lands on it.
    #[test]
    fn a_provoked_fault_is_seen_tested_reported_and_stopped_at() {
        let Some(f) = flash() else { return };
        let mut m = crate::machine(&f, None, board::DEFAULT_LAYOUT).unwrap();
        crate::Engine::idle().apply(&mut m);
        let mut s = crate::Session::new(m, crate::Engine::idle());
        s.trace_maps(true);
        for _ in 0..1000 {
            s.run(40_000);
        }
        let t = s.maps.clone().unwrap();
        let p2455 = {
            let g = t.lock().unwrap();
            let d = g.dtcs.as_ref().expect("the Astra's codes");
            assert!(d.tested.len() > 150, "{}", d.tested.len());
            d.table.iter().position(|x| x.code() == "P2455").unwrap() as u16
        };
        assert_eq!(
            t.lock().unwrap().dtcs.as_ref().unwrap().tested[&p2455].fails,
            0
        );
        // The DPF sensor at the top of its range.
        s.m.chip.adc.input[11] = 1023;
        for _ in 0..12_000 {
            s.run(40_000);
        }
        s.read_codes();
        {
            let g = t.lock().unwrap();
            let d = g.dtcs.as_ref().unwrap();
            assert!(d.tested[&p2455].fails > 0);
            let raw = d.table[p2455 as usize].raw;
            assert!(
                d.confirmed.contains(&raw) || d.pending.contains(&raw),
                "{:x?} {:x?}",
                d.confirmed,
                d.pending
            );
            let ms = d.monitors(p2455, [0x7128, 0x70B4]);
            eprintln!("P2455 monitors: {ms:x?}");
            assert!(!ms.is_empty());
        }
        s.break_on_test(Some(p2455));
        assert_eq!(s.run(80_000_000), sh7059::Stop::Breakpoint);
        assert_eq!(s.m.cpu.pc, ASTRA_LAYOUT.reporter);
        assert_eq!(s.m.cpu.r[4], p2455 as u32);
    }
}
