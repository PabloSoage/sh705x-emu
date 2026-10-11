//! `sh705x touched`: which calibration maps the firmware looks up while it runs a
//! scenario, how often, at what inputs, and what they give.
//!
//! Denso firmware reads its maps through a few interpolation routines that take the
//! map's descriptor in R4 and the inputs as floats in FR4 (and FR5 for a 3D map), and
//! return the result in FR0. With a breakpoint on each routine and another, one-shot, on
//! the return address, every lookup is caught with its inputs and its result. Which
//! routines those are depends on the firmware: they are given with --masters (found
//! with the map catalogue's tools), and a catalogue in CSV can be given to name each
//! descriptor's size and data address. With a second image, the maps whose data differ
//! there are marked: of what a calibration changes, what this scenario reads.

use crate::{EcuOptions, boot};
use sh7059::Stop;
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::PathBuf;

/// Options of `sh705x touched`.
#[derive(Debug, PartialEq, Eq)]
pub struct TouchedOptions {
    /// The ECU and the scenario (speed, sensors).
    pub ecu: EcuOptions,
    /// Interpolation routines: (address, dimensions of their maps, 2 or 3).
    pub masters: Vec<(u32, u8)>,
    /// Map catalogues (CSV with Struct_Addr, Size_X, [Size_Y,] Data_Addr columns).
    pub catalog: Vec<PathBuf>,
    /// Label files (the transferred-tags format: `ref dims | struct data ax.. | tags`),
    /// naming each map by its struct address.
    pub labels: Vec<PathBuf>,
    /// ECU time to watch, milliseconds.
    pub millis: u64,
    /// A second image: which of the maps looked up have other data there.
    pub compare: Option<PathBuf>,
}

/// What one map saw.
#[derive(Clone, Debug, Default)]
struct Use {
    dims: u8,
    calls: u64,
    x: Range,
    y: Range,
    out: Range,
}

#[derive(Clone, Copy, Debug)]
struct Range {
    min: f32,
    max: f32,
    last: f32,
    seen: bool,
}

impl Default for Range {
    fn default() -> Self {
        Self {
            min: f32::INFINITY,
            max: f32::NEG_INFINITY,
            last: f32::NAN,
            seen: false,
        }
    }
}

impl Range {
    fn add(&mut self, v: f32) {
        if v.is_finite() {
            self.min = self.min.min(v);
            self.max = self.max.max(v);
        }
        self.last = v;
        self.seen = true;
    }

    fn show(&self) -> String {
        if !self.seen {
            return "-".into();
        }
        if self.min == self.max {
            format!("{}", self.min)
        } else {
            format!("{} .. {}", self.min, self.max)
        }
    }
}

/// A catalogue entry: size and data address.
struct Entry {
    size: String,
    data: u32,
    /// Bytes of data: cells times the inferred width (2 when the catalogue gives none).
    bytes: usize,
}

/// Reads label files in the transferred-tags format: one map per line,
/// `ref  dims | struct  data  axX axY corrZ | tag; tag; ...  (candidates ...)`. Keyed by
/// the struct address, which is the descriptor the firmware passes in R4. The readable
/// name is the tags with the trailing `(candidates ...)` note dropped; the first entry
/// wins when a struct is listed more than once.
fn read_labels(paths: &[PathBuf]) -> Result<BTreeMap<u32, String>, String> {
    let mut out = BTreeMap::new();
    for p in paths {
        let text = std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()))?;
        for line in text.lines() {
            let mut parts = line.splitn(3, '|');
            let (Some(_left), Some(mid), Some(tags)) = (parts.next(), parts.next(), parts.next())
            else {
                continue;
            };
            let Some(struct_addr) = mid
                .split_whitespace()
                .next()
                .and_then(|h| u32::from_str_radix(h, 16).ok())
            else {
                continue;
            };
            let name = tags.split("(candidates").next().unwrap_or(tags).trim();
            if !name.is_empty() {
                out.entry(struct_addr).or_insert_with(|| name.to_string());
            }
        }
    }
    Ok(out)
}

fn read_catalog(paths: &[PathBuf]) -> Result<BTreeMap<u32, Entry>, String> {
    let mut out = BTreeMap::new();
    for p in paths {
        let text = std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()))?;
        let mut lines = text.lines();
        let head: Vec<&str> = lines.next().unwrap_or("").split(',').collect();
        let col = |name: &str| head.iter().position(|h| h.trim() == name);
        let (Some(st), Some(sx), Some(da)) = (col("Struct_Addr"), col("Size_X"), col("Data_Addr"))
        else {
            return Err(format!(
                "{}: wants Struct_Addr, Size_X and Data_Addr columns",
                p.display()
            ));
        };
        let sy = col("Size_Y");
        let wd = col("Inferred_Width");
        for l in lines {
            let c: Vec<&str> = l.split(',').collect();
            let hex = |k: usize| {
                c.get(k)
                    .and_then(|v| u32::from_str_radix(v.trim(), 16).ok())
            };
            let (Some(s), Some(d)) = (hex(st), hex(da)) else {
                continue;
            };
            let x = c.get(sx).map_or("?", |v| v.trim());
            let num = |v: &str| v.trim().parse::<usize>().ok();
            let (size, cells) = match sy.and_then(|k| c.get(k)) {
                Some(y) => (
                    format!("{x}x{}", y.trim()),
                    num(x).zip(num(y)).map(|(a, b)| a * b),
                ),
                None => (x.to_string(), num(x)),
            };
            let width = wd.and_then(|k| c.get(k)).and_then(|v| num(v)).unwrap_or(2);
            out.insert(
                s,
                Entry {
                    size,
                    data: d,
                    bytes: cells.unwrap_or(0) * width,
                },
            );
        }
    }
    Ok(out)
}

/// Runs `sh705x touched`.
pub fn touched(o: &TouchedOptions) -> Result<String, String> {
    let catalog = read_catalog(&o.catalog)?;
    let labels = read_labels(&o.labels)?;
    let images = match &o.compare {
        None => None,
        Some(p) => {
            let a = std::fs::read(&o.ecu.flash)
                .map_err(|e| format!("{}: {e}", o.ecu.flash.display()))?;
            let b = std::fs::read(p).map_err(|e| format!("{}: {e}", p.display()))?;
            if a.len() != b.len() {
                return Err(format!("{}: not the size of the first image", p.display()));
            }
            Some((a, b))
        }
    };
    // Whether a catalogued map's data differs between the images.
    let differs = |e: &Entry| -> Option<bool> {
        let (a, b) = images.as_ref()?;
        let r = e.data as usize..e.data as usize + e.bytes;
        (e.bytes > 0 && r.end <= a.len()).then(|| a[r.clone()] != b[r])
    };
    let mut m = boot(&o.ecu)?;
    let masters: BTreeMap<u32, u8> = o.masters.iter().copied().collect();
    m.breakpoints = masters.keys().copied().collect();
    let end = m.cpu.cycles + o.millis * bridge::link::CYCLES_PER_US * 1000;
    let mut uses: BTreeMap<u32, Use> = BTreeMap::new();
    // Lookups under way: (return address, descriptor), innermost last.
    let mut pending: Vec<(u32, u32)> = Vec::new();
    let mut lookups = 0u64;
    while m.cpu.cycles < end {
        match m.run_until(1_000_000) {
            Stop::Count => {}
            Stop::Breakpoint => {
                let pc = m.cpu.pc;
                let f = |k: usize| f32::from_bits(m.cpu.fr[k]);
                if let Some(&dims) = masters.get(&pc) {
                    let desc = m.cpu.r[4];
                    let u = uses.entry(desc).or_default();
                    u.dims = dims;
                    u.calls += 1;
                    u.x.add(f(4));
                    if dims == 3 {
                        u.y.add(f(5));
                    }
                    lookups += 1;
                    pending.push((m.cpu.pr, desc));
                    if !m.breakpoints.contains(&m.cpu.pr) {
                        m.breakpoints.push(m.cpu.pr);
                    }
                } else if let Some(k) = pending.iter().rposition(|&(ret, _)| ret == pc) {
                    let (_, desc) = pending.remove(k);
                    if let Some(u) = uses.get_mut(&desc) {
                        u.out.add(f(0));
                    }
                    if !masters.contains_key(&pc) && !pending.iter().any(|&(r, _)| r == pc) {
                        m.breakpoints.retain(|&b| b != pc);
                    }
                }
            }
            Stop::Watch => {}
            Stop::Asleep => break,
        }
        m.chip.hcan.ch[0].outbox.clear();
        m.chip.heard.clear();
    }
    let mut rows: Vec<(&u32, &Use)> = uses.iter().collect();
    rows.sort_by(|a, b| b.1.calls.cmp(&a.1.calls).then(a.0.cmp(b.0)));
    let mut out = String::new();
    writeln!(
        out,
        "{} lookups of {} maps in {} ms of ECU time{}",
        lookups,
        uses.len(),
        o.millis,
        if catalog.is_empty() {
            String::new()
        } else {
            format!(
                "; {} of them in the catalogue's {}",
                uses.keys().filter(|d| catalog.contains_key(d)).count(),
                catalog.len()
            )
        }
    )
    .unwrap();
    if images.is_some() {
        let used: Vec<String> = uses
            .keys()
            .filter(|d| catalog.get(d).and_then(differs) == Some(true))
            .map(|d| format!("{d:08X}"))
            .collect();
        let all = catalog
            .values()
            .filter(|e| differs(e) == Some(true))
            .count();
        writeln!(
            out,
            "{all} catalogued maps have other data in the second image; {} of them were looked \
             up here, marked *: {}",
            used.len(),
            if used.is_empty() {
                "none".to_string()
            } else {
                used.join(" ")
            }
        )
        .unwrap();
    }
    if !labels.is_empty() {
        let named = uses.keys().filter(|d| labels.contains_key(d)).count();
        writeln!(
            out,
            "{named} of them are named by the {} labels given",
            labels.len()
        )
        .unwrap();
    }
    writeln!(
        out,
        "{:>10} {:>3} {:>8} {:>7} {:>10}  x / y inputs  ->  result",
        "descriptor", "dim", "calls", "size", "data"
    )
    .unwrap();
    for (desc, u) in rows {
        let (size, data) = catalog
            .get(desc)
            .map_or(("?".to_string(), "?".to_string()), |e| {
                let mark = if differs(e) == Some(true) { "*" } else { "" };
                (e.size.clone(), format!("{mark}{:08X}", e.data))
            });
        let inputs = if u.dims == 3 {
            format!("{} / {}", u.x.show(), u.y.show())
        } else {
            u.x.show()
        };
        let name = labels.get(desc).map_or(String::new(), |n| format!("  {n}"));
        writeln!(
            out,
            "  {desc:08X} {:>3} {:>8} {size:>7} {data:>10}  {inputs}  ->  {}{name}",
            format!("{}D", u.dims),
            u.calls,
            u.out.show()
        )
        .unwrap();
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::read_labels;

    #[test]
    fn labels_are_keyed_by_the_struct_address() {
        // The transferred-tags format: `ref dims | struct data ax ax corr | tags (note)`.
        let text = "\
000449C4   16x11 | 0009DE14   0011B660    0.000  0.000  1.000 | Turbo; VNT Duty Cycle  (candidates 4, margin 0.48)
0003C88C   17x35 | 0009BC30   000E8100    0.000  0.000  1.000 | Injection; Fuel Table  (candidates 1, margin inf)
a comment line without bars is ignored
0004827A   20x20 | 0009B8E8   000E2B20    0.000  0.000  0.993 | Injection; Airflow Meter 1  (candidates 28)
000482A8   20x20 | 0009B8E8   000E2B20    0.000  0.000  0.993 | Injection; Airflow Meter 2  (candidates 28)
";
        let dir = std::env::temp_dir().join(format!("sh705x-labels-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("tags.txt");
        std::fs::write(&p, text).unwrap();
        let labels = read_labels(&[p]).unwrap();
        assert_eq!(labels.len(), 3, "three distinct structs");
        assert_eq!(labels[&0x0009_DE14], "Turbo; VNT Duty Cycle");
        assert_eq!(labels[&0x0009_BC30], "Injection; Fuel Table");
        // A struct listed twice keeps the first name; the (candidates ...) note is dropped.
        assert_eq!(labels[&0x0009_B8E8], "Injection; Airflow Meter 1");
    }
}
