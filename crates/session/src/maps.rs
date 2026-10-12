//! The calibration maps the firmware reads, as it reads them.
//!
//! Denso firmware looks its maps up through a few interpolation routines (the "masters"),
//! one for 3D maps and one for 2D curves, which take the map's descriptor in R4 and its
//! inputs in FR4 (and FR5), and return the result in FR0. The descriptor says everything
//! needed to draw the map (resultados fase 7; `Denso_SH705x` 17 and 18):
//!
//! - 3D: `u16 nx, u16 ny, ptr X, ptr Y, ptr Z, u8 type @0x10, f32 scale @0x14,
//!   f32 offset @0x18`;
//! - 2D: `u16 n, u8 type @0x02, u8 -, ptr X, ptr Z, f32 scale @0x0C, f32 offset @0x10`;
//! - the axes are floats; the cells are of `type` (0x00 float, 0x04 u8, 0x08 u16, 0x0C s8,
//!   0x10 s16), worth `offset + raw × scale` (a float is taken as it is).
//!
//! [`find_masters`] finds the two routines in a flash image the way the map catalogue's
//! tools do: the call targets that most often get a valid descriptor in R4.
//! [`MapTrace`] probes them ([`sh7059::Probes`]: the machine does not stop) and gathers
//! every lookup: which map, from where, with what inputs, what it gave.

use crate::names::Names;
use crate::sites::{self, Site, Source};
use sh7059::{Call, Machine};
use std::collections::{BTreeMap, HashMap, VecDeque};

/// The interpolation routines of one firmware.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Masters {
    /// The 3D one.
    pub d3: Option<u32>,
    /// The 2D one.
    pub d2: Option<u32>,
}

/// How a map's cells are stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cell {
    /// IEEE single, taken as it is.
    Float,
    /// Unsigned byte.
    U8,
    /// Unsigned 16 bits.
    U16,
    /// Signed byte.
    S8,
    /// Signed 16 bits.
    S16,
}

impl Cell {
    /// The type byte of a descriptor.
    fn from_type(t: u8) -> Option<Self> {
        Some(match t {
            0x00 => Self::Float,
            0x04 => Self::U8,
            0x08 => Self::U16,
            0x0C => Self::S8,
            0x10 => Self::S16,
            _ => return None,
        })
    }

    /// Bytes per cell.
    pub fn bytes(self) -> u32 {
        match self {
            Self::U8 | Self::S8 => 1,
            Self::U16 | Self::S16 => 2,
            Self::Float => 4,
        }
    }

    /// Its name.
    pub fn name(self) -> &'static str {
        match self {
            Self::Float => "float",
            Self::U8 => "u8",
            Self::U16 => "u16",
            Self::S8 => "s8",
            Self::S16 => "s16",
        }
    }
}

/// A map, read from its descriptor.
#[derive(Clone, Debug, PartialEq)]
pub struct Map {
    /// The descriptor's address (what the firmware passes in R4).
    pub desc: u32,
    /// The X axis.
    pub x: Vec<f32>,
    /// The Y axis; empty for a 2D curve.
    pub y: Vec<f32>,
    /// The cells in physical units, row by row: `z[j * nx + i]` is at `x[i]`, `y[j]`.
    pub z: Vec<f32>,
    /// How the cells are stored.
    pub cell: Cell,
    /// Physical value = `offset + raw × scale` (1 and 0 for floats).
    pub scale: f32,
    /// See [`Map::scale`].
    pub offset: f32,
    /// Where the X axis is.
    pub x_at: u32,
    /// Where the Y axis is (0 for a 2D curve).
    pub y_at: u32,
    /// Where the cells are.
    pub z_at: u32,
}

/// Largest axis taken for a map: the biggest in the Astra's catalogue has 35.
const MAX_AXIS: u16 = 128;

/// Whether `a` can hold calibration data: the flash, or RAM.
fn plausible(a: u32) -> bool {
    (a as usize) < sh7059::ROM_SIZE || a >= sh7059::RAM_BASE
}

/// Big-endian reads from a memory seen a byte at a time.
struct Mem<'a>(&'a dyn Fn(u32) -> u8);

impl Mem<'_> {
    fn u8(&self, a: u32) -> u8 {
        (self.0)(a)
    }
    fn u16(&self, a: u32) -> u16 {
        u16::from_be_bytes([self.u8(a), self.u8(a + 1)])
    }
    fn u32(&self, a: u32) -> u32 {
        (0..4).fold(0, |v, k| v << 8 | self.u8(a + k) as u32)
    }
    fn f32(&self, a: u32) -> f32 {
        f32::from_bits(self.u32(a))
    }
    /// `n` axis points at `a`, if they are finite and never decrease.
    fn axis(&self, a: u32, n: u16) -> Option<Vec<f32>> {
        let v: Vec<f32> = (0..n as u32).map(|k| self.f32(a + 4 * k)).collect();
        (v.iter().all(|x| x.is_finite()) && v.windows(2).all(|w| w[0] <= w[1])).then_some(v)
    }
    /// `n` cells of `cell` at `a`, in physical units.
    fn cells(&self, a: u32, n: u32, cell: Cell, scale: f32, offset: f32) -> Vec<f32> {
        (0..n)
            .map(|k| {
                let at = a + k * cell.bytes();
                let raw = match cell {
                    Cell::Float => return self.f32(at),
                    Cell::U8 => self.u8(at) as f32,
                    Cell::S8 => self.u8(at) as i8 as f32,
                    Cell::U16 => self.u16(at) as f32,
                    Cell::S16 => self.u16(at) as i16 as f32,
                };
                offset + raw * scale
            })
            .collect()
    }
}

impl Map {
    /// The 3D map whose descriptor is at `desc`, if it reads as one.
    pub fn read3d(read: &dyn Fn(u32) -> u8, desc: u32) -> Option<Self> {
        let m = Mem(read);
        let (nx, ny) = (m.u16(desc), m.u16(desc + 2));
        if !(1..=MAX_AXIS).contains(&nx) || !(1..=MAX_AXIS).contains(&ny) {
            return None;
        }
        let (px, py, pz) = (m.u32(desc + 4), m.u32(desc + 8), m.u32(desc + 12));
        let cell = Cell::from_type(m.u8(desc + 0x10))?;
        if ![px, py, pz].into_iter().all(plausible) {
            return None;
        }
        let (scale, offset) = if cell == Cell::Float {
            (1.0, 0.0)
        } else {
            (m.f32(desc + 0x14), m.f32(desc + 0x18))
        };
        if !scale.is_finite() || !offset.is_finite() {
            return None;
        }
        Some(Self {
            desc,
            x: m.axis(px, nx)?,
            y: m.axis(py, ny)?,
            z: m.cells(pz, nx as u32 * ny as u32, cell, scale, offset),
            cell,
            scale,
            offset,
            x_at: px,
            y_at: py,
            z_at: pz,
        })
    }

    /// The 2D curve whose descriptor is at `desc`, if it reads as one.
    pub fn read2d(read: &dyn Fn(u32) -> u8, desc: u32) -> Option<Self> {
        let m = Mem(read);
        let n = m.u16(desc);
        if !(1..=MAX_AXIS).contains(&n) {
            return None;
        }
        let cell = Cell::from_type(m.u8(desc + 2))?;
        let (px, pz) = (m.u32(desc + 4), m.u32(desc + 8));
        if !plausible(px) || !plausible(pz) {
            return None;
        }
        let (scale, offset) = if cell == Cell::Float {
            (1.0, 0.0)
        } else {
            (m.f32(desc + 0x0C), m.f32(desc + 0x10))
        };
        if !scale.is_finite() || !offset.is_finite() {
            return None;
        }
        Some(Self {
            desc,
            x: m.axis(px, n)?,
            y: Vec::new(),
            z: m.cells(pz, n as u32, cell, scale, offset),
            cell,
            scale,
            offset,
            x_at: px,
            y_at: 0,
            z_at: pz,
        })
    }

    /// 2 for a curve, 3 for a map.
    pub fn dims(&self) -> u8 {
        if self.y.is_empty() { 2 } else { 3 }
    }

    /// Cells along X.
    pub fn nx(&self) -> usize {
        self.x.len()
    }

    /// Cells along Y (1 for a curve).
    pub fn ny(&self) -> usize {
        self.y.len().max(1)
    }

    /// The cell at `i` along X and `j` along Y.
    pub fn at(&self, i: usize, j: usize) -> f32 {
        self.z[j * self.nx() + i]
    }

    /// Where `v` falls along `axis`, as a fractional index, held to the axis' ends.
    pub fn position(axis: &[f32], v: f32) -> f32 {
        let n = axis.len();
        if n < 2 || v <= axis[0] || v.is_nan() {
            return 0.0;
        }
        if v >= axis[n - 1] {
            return (n - 1) as f32;
        }
        let i = axis
            .partition_point(|&a| a <= v)
            .saturating_sub(1)
            .min(n - 2);
        let span = axis[i + 1] - axis[i];
        i as f32
            + if span > 0.0 {
                (v - axis[i]) / span
            } else {
                0.0
            }
    }

    /// The map at `x` (and `y`), interpolated linearly between its cells and held at its
    /// edges.
    pub fn interpolate(&self, x: f32, y: f32) -> f32 {
        let lerp = |a: f32, b: f32, t: f32| a + (b - a) * t;
        let split = |p: f32, n: usize| {
            let i = (p.floor() as usize).min(n.saturating_sub(2));
            (i, (p - i as f32).clamp(0.0, 1.0))
        };
        let (i, tx) = split(Self::position(&self.x, x), self.nx());
        let i1 = (i + 1).min(self.nx() - 1);
        if self.dims() == 2 {
            return lerp(self.at(i, 0), self.at(i1, 0), tx);
        }
        let (j, ty) = split(Self::position(&self.y, y), self.ny());
        let j1 = (j + 1).min(self.ny() - 1);
        lerp(
            lerp(self.at(i, j), self.at(i1, j), tx),
            lerp(self.at(i, j1), self.at(i1, j1), tx),
            ty,
        )
    }
}

/// Instructions searched back from a call for the loads that set its target and R4.
const LOOK_BACK: usize = 12;
/// Calls with a valid descriptor a routine needs to be taken for a master.
const MIN_CALLS: u32 = 20;

/// Finds the interpolation routines in `flash`: for every `JSR @Rm` (its target loaded
/// into Rm from a PC-relative literal) and every `BSR`, the literal loaded into R4 in the
/// delay slot or just before; the target that most often gets a valid 3D descriptor
/// there is the 3D master, and the other one that most often gets a valid 2D descriptor,
/// the 2D master.
pub fn find_masters(flash: &[u8]) -> Masters {
    let word = |a: usize| u16::from_be_bytes([flash[a], flash[a + 1]]);
    let literal = |a: usize, op: u16| {
        let at = (a & !3) + 4 + (op & 0xFF) as usize * 4;
        flash
            .get(at..at + 4)
            .map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    };
    // The literal loaded into register `r` by `MOV.L @(disp,PC),Rr`, looking back from
    // `a` (and at `slot` first, if given).
    let loaded = |a: usize, r: u16, slot: Option<usize>| {
        let is = |op: u16| op & 0xFF00 == 0xD000 | r << 8;
        if let Some(s) = slot
            && is(word(s))
        {
            return literal(s, word(s));
        }
        (1..=LOOK_BACK)
            .filter_map(|k| a.checked_sub(2 * k))
            .map(|b| (b, word(b)))
            .find(|&(_, op)| is(op))
            .and_then(|(b, op)| literal(b, op))
    };
    let read = |a: u32| flash.get(a as usize).copied().unwrap_or(0);
    let (mut c3, mut c2): (HashMap<u32, u32>, HashMap<u32, u32>) = Default::default();
    for a in (0..flash.len().saturating_sub(4)).step_by(2) {
        let op = word(a);
        let target = if op & 0xF0FF == 0x400B {
            loaded(a, (op >> 8) & 0xF, None)
        } else if op & 0xF000 == 0xB000 {
            let d = ((op & 0xFFF) << 4) as i16 >> 4;
            Some((a as i64 + 4 + 2 * d as i64) as u32)
        } else {
            continue;
        };
        let (Some(target), Some(desc)) = (target, loaded(a, 4, Some(a + 2))) else {
            continue;
        };
        if (desc as usize) >= flash.len() {
            continue;
        }
        if Map::read3d(&read, desc).is_some() {
            *c3.entry(target).or_default() += 1;
        }
        if Map::read2d(&read, desc).is_some() {
            *c2.entry(target).or_default() += 1;
        }
    }
    let best = |c: &HashMap<u32, u32>, not: Option<u32>| {
        c.iter()
            .filter(|&(&t, &n)| n >= MIN_CALLS && Some(t) != not)
            .max_by_key(|&(&t, &n)| (n, std::cmp::Reverse(t)))
            .map(|(&t, _)| t)
    };
    let d3 = best(&c3, None);
    Masters {
        d3,
        d2: best(&c2, d3),
    }
}

/// Lookups kept for the live view, newest last.
const RECENT: usize = 4096;

/// What one map has seen.
#[derive(Clone, Debug)]
pub struct Seen {
    /// The map, if its descriptor reads as one.
    pub map: Option<Map>,
    /// Lookups.
    pub calls: u64,
    /// The last lookup: X, Y (NaN for a curve) and the result.
    pub last: (f32, f32, f32),
    /// Cycle of the last lookup.
    pub at: u64,
    /// Lookups per call site (the return address; the call is the instruction 4 bytes
    /// before).
    pub sites: BTreeMap<u32, u64>,
    /// Lookups whose result differs from interpolating the decoded map by more than a
    /// thousandth of the map's span: a check on the decoding.
    pub mismatches: u64,
}

/// Every lookup of the masters, gathered per map.
#[derive(Clone, Debug, Default)]
pub struct MapTrace {
    /// The routines probed.
    pub masters: Masters,
    /// Per descriptor.
    pub seen: BTreeMap<u32, Seen>,
    /// The latest lookups, oldest first.
    pub recent: VecDeque<Call>,
    /// All lookups.
    pub total: u64,
    /// Lookups lost because nobody took them in time.
    pub dropped: u64,
    /// Each call site (by return address), read from its code when first seen.
    pub sites: HashMap<u32, Site>,
    /// What is known by name.
    pub names: Names,
    /// The fault codes and their tests, where the calibration's layout is known.
    pub dtcs: Option<crate::dtcs::DtcTrace>,
}

impl MapTrace {
    /// Starts probing `masters` on `m`.
    pub fn attach(m: &mut Machine, masters: Masters) -> Self {
        let routines = [masters.d3, masters.d2].into_iter().flatten().collect();
        m.probes = Some(Box::new(sh7059::Probes::new(routines)));
        Self {
            masters,
            ..Self::default()
        }
    }

    /// What a map is called: its label, else what its result is stored as, if named.
    pub fn title(&self, desc: u32) -> Option<String> {
        if let Some(n) = self.names.maps.get(&desc) {
            return Some(n.text.clone());
        }
        self.seen.get(&desc)?.sites.keys().find_map(|r| {
            let a = self.sites.get(r)?.out?;
            self.names.vars.get(&a).map(|n| format!("gives {}", n.text))
        })
    }

    /// A variable: its name if known (with a ? if matched in one state only), its unit,
    /// and its address.
    pub fn var_name(&self, a: u32) -> String {
        match self.names.vars.get(&a) {
            Some(n) => {
                let unsure = matches!(n.how, crate::names::How::Identifier { sure: false, .. });
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

    /// Where an input comes from, in words.
    pub fn source_text(&self, src: Source) -> String {
        match src {
            Source::Ram(a) => self.var_name(a),
            Source::Map(d) => {
                let t = self.title(d).map_or(String::new(), |t| format!(" ({t})"));
                format!("map {d:06X}{t}")
            }
            Source::Call(t) => match self.names.routines.get(&t) {
                Some(n) => format!("{} (routine {t:06X})", n.text),
                None => format!("routine {t:06X}"),
            },
            Source::Unknown => "computed in the caller".into(),
        }
    }

    /// The maps whose result an input of a lookup is, or that read a variable a map's
    /// result is stored in: who reads what `desc` gives (by the sites seen so far).
    pub fn readers(&self, desc: u32) -> Vec<u32> {
        let outs: Vec<u32> = self.seen[&desc]
            .sites
            .keys()
            .filter_map(|r| self.sites.get(r).and_then(|s| s.out))
            .collect();
        let mut out: Vec<u32> = self
            .seen
            .iter()
            .filter(|&(&d, s)| {
                d != desc
                    && s.sites.keys().any(|r| {
                        self.sites.get(r).is_some_and(|site| {
                            [site.x, site.y].iter().any(|src| match *src {
                                Source::Map(m) => m == desc,
                                Source::Ram(a) => outs.contains(&a),
                                _ => false,
                            })
                        })
                    })
            })
            .map(|(&d, _)| d)
            .collect();
        out.dedup();
        out
    }

    /// Takes the lookups `m` has made since the last time.
    pub fn take(&mut self, m: &mut Machine) {
        let Some(p) = m.probes.as_deref_mut() else {
            return;
        };
        let calls = std::mem::take(&mut p.calls);
        self.dropped = p.dropped;
        let chip = &m.chip;
        let read = |a: u32| chip.peek(a);
        if let Some(d) = self.dtcs.as_mut() {
            d.read_conditions(&read);
        }
        for c in calls {
            if let Some(d) = self.dtcs.as_mut()
                && (c.routine == d.layout.reporter || c.routine == d.layout.counter)
            {
                d.add(&c);
                continue;
            }
            self.total += 1;
            let three = Some(c.routine) == self.masters.d3;
            let s = self.seen.entry(c.r4).or_insert_with(|| Seen {
                map: if three {
                    Map::read3d(&read, c.r4)
                } else {
                    Map::read2d(&read, c.r4)
                },
                calls: 0,
                last: (f32::NAN, f32::NAN, f32::NAN),
                at: 0,
                sites: BTreeMap::new(),
                mismatches: 0,
            });
            s.calls += 1;
            s.last = (c.fr4, if three { c.fr5 } else { f32::NAN }, c.fr0);
            s.at = c.cycles;
            *s.sites.entry(c.ret).or_default() += 1;
            if !self.sites.contains_key(&c.ret) {
                let site = sites::read(
                    &read,
                    c.ret,
                    [
                        self.masters.d3.unwrap_or(u32::MAX),
                        self.masters.d2.unwrap_or(u32::MAX),
                    ],
                );
                self.sites.insert(c.ret, site);
            }
            if let Some(map) = &s.map {
                let (lo, hi) = map
                    .z
                    .iter()
                    .fold((f32::INFINITY, f32::NEG_INFINITY), |(l, h), &z| {
                        (l.min(z), h.max(z))
                    });
                let tolerance = ((hi - lo) * 1e-3).max(1e-4);
                if (map.interpolate(c.fr4, c.fr5) - c.fr0).abs() > tolerance {
                    s.mismatches += 1;
                }
            }
            if self.recent.len() == RECENT {
                self.recent.pop_front();
            }
            self.recent.push_back(c);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A memory of `bytes` at 0.
    fn mem(bytes: &[u8]) -> impl Fn(u32) -> u8 + '_ {
        |a| bytes.get(a as usize).copied().unwrap_or(0)
    }

    fn put(b: &mut [u8], a: usize, v: &[u8]) {
        b[a..a + v.len()].copy_from_slice(v);
    }

    /// A 3x2 u16 map at 0x100, scale 0.5, offset 10, axes at 0x200 and 0x300, cells at
    /// 0x400: z = 10 + 0.5 × (100 j + i).
    fn map3() -> Vec<u8> {
        let mut b = vec![0u8; 0x1000];
        put(&mut b, 0x100, &3u16.to_be_bytes());
        put(&mut b, 0x102, &2u16.to_be_bytes());
        put(&mut b, 0x104, &0x200u32.to_be_bytes());
        put(&mut b, 0x108, &0x300u32.to_be_bytes());
        put(&mut b, 0x10C, &0x400u32.to_be_bytes());
        b[0x110] = 0x08;
        put(&mut b, 0x114, &0.5f32.to_be_bytes());
        put(&mut b, 0x118, &10.0f32.to_be_bytes());
        for (k, x) in [0.0f32, 1000.0, 2000.0].iter().enumerate() {
            put(&mut b, 0x200 + 4 * k, &x.to_be_bytes());
        }
        for (k, y) in [0.0f32, 50.0].iter().enumerate() {
            put(&mut b, 0x300 + 4 * k, &y.to_be_bytes());
        }
        for j in 0..2u16 {
            for i in 0..3u16 {
                let at = 0x400 + 2 * (j * 3 + i) as usize;
                put(&mut b, at, &(100 * j + i).to_be_bytes());
            }
        }
        b
    }

    #[test]
    fn a_3d_descriptor_reads_in_physical_units() {
        let b = map3();
        let m = Map::read3d(&mem(&b), 0x100).unwrap();
        assert_eq!((m.nx(), m.ny(), m.dims(), m.cell), (3, 2, 3, Cell::U16));
        assert_eq!(m.x, [0.0, 1000.0, 2000.0]);
        assert_eq!(m.at(2, 1), 10.0 + 0.5 * 102.0);
        // Halfway along both axes, and held at the edges.
        assert_eq!(m.interpolate(500.0, 25.0), 10.0 + 0.5 * 50.5);
        assert_eq!(m.interpolate(-5.0, 99.0), m.at(0, 1));
        assert_eq!(Map::position(&m.x, 1500.0), 1.5);
        // Not a 2D curve.
        assert!(Map::read2d(&mem(&b), 0x100).is_none_or(|c| c.nx() == 3));
    }

    #[test]
    fn a_bad_descriptor_is_refused() {
        let mut b = map3();
        b[0x110] = 0x05; // no such type
        assert!(Map::read3d(&mem(&b), 0x100).is_none());
        let mut b = map3();
        put(&mut b, 0x204, &(-1.0f32).to_be_bytes()); // the axis goes back
        assert!(Map::read3d(&mem(&b), 0x100).is_none());
    }

    /// The flash in `SH705X_FLASH`, or `None` to skip.
    fn flash() -> Option<Vec<u8>> {
        std::env::var("SH705X_FLASH")
            .ok()
            .map(|p| std::fs::read(p).expect("SH705X_FLASH"))
    }

    #[test]
    fn the_astra_masters_are_found_in_its_flash() {
        let Some(f) = flash() else { return };
        let m = find_masters(&f);
        // The ones the map catalogue's tools found in Ghidra (resultados fase 7).
        assert_eq!(m.d3, Some(0x7128), "{m:x?}");
        assert_eq!(m.d2, Some(0x70B4), "{m:x?}");
    }

    #[test]
    fn at_idle_the_lookups_match_the_decoded_maps() {
        let Some(f) = flash() else { return };
        let mut m = crate::machine(&f, None, board::DEFAULT_LAYOUT).unwrap();
        let engine = crate::Engine::idle();
        engine.apply(&mut m);
        let mut t = MapTrace::attach(&mut m, find_masters(&f));
        // Half a second of ECU time.
        for _ in 0..1000 {
            m.run(40_000);
            t.take(&mut m);
        }
        let maps = t.seen.len();
        let decoded = t.seen.values().filter(|s| s.map.is_some()).count();
        let calls: u64 = t.seen.values().map(|s| s.calls).sum();
        let bad: u64 = t.seen.values().map(|s| s.mismatches).sum();
        eprintln!("{maps} maps, {decoded} decoded, {calls} lookups, {bad} off the decoding");
        assert!(maps > 300, "{maps}");
        assert!(decoded * 100 >= maps * 95, "{decoded} of {maps}");
        // The firmware's interpolation and ours agree on nearly every lookup.
        assert!(bad * 100 <= calls, "{bad} of {calls}");
        assert_eq!(t.dropped, 0);
    }

    #[test]
    fn the_variables_the_sites_name_hold_the_inputs_the_firmware_passes() {
        use crate::sites::{Source, read};
        let Some(f) = flash() else { return };
        let mut m = crate::machine(&f, None, board::DEFAULT_LAYOUT).unwrap();
        crate::Engine::idle().apply(&mut m);
        let masters = find_masters(&f);
        let both = [masters.d3.unwrap(), masters.d2.unwrap()];
        m.run(8_000_000);
        m.breakpoints = both.to_vec();
        let (mut named, mut held, mut inputs, mut sites) =
            (0, 0, 0, std::collections::HashSet::new());
        let (mut maps, mut calls) = (0, 0);
        for _ in 0..5000 {
            assert_eq!(m.run_until(10_000_000), sh7059::Stop::Breakpoint);
            let three = Some(m.cpu.pc) == masters.d3;
            let ret = m.cpu.pr;
            sites.insert(ret);
            let chip = &m.chip;
            let site = read(&|a| chip.peek(a), ret, both);
            let f32_at =
                |a: u32| f32::from_bits((0..4).fold(0, |v, k| v << 8 | chip.peek(a + k) as u32));
            for (src, fr) in [(site.x, 4), (site.y, 5)]
                .into_iter()
                .take(if three { 2 } else { 1 })
            {
                inputs += 1;
                match src {
                    Source::Map(_) => maps += 1,
                    Source::Call(_) => calls += 1,
                    _ => {}
                }
                if let Source::Ram(a) = src {
                    named += 1;
                    if f32_at(a).to_bits() == m.cpu.fr[fr] {
                        held += 1;
                    }
                }
            }
        }
        eprintln!(
            "{} sites; {inputs} inputs: {named} a variable ({held} holding the input), {maps} another map, {calls} a routine",
            sites.len()
        );
        assert!(held * 100 >= named * 97, "{held} of {named}");
    }

    #[test]
    fn a_map_break_stops_at_that_map_and_passes_the_others() {
        let Some(f) = flash() else { return };
        let mut m = crate::machine(&f, None, board::DEFAULT_LAYOUT).unwrap();
        let engine = crate::Engine::idle();
        engine.apply(&mut m);
        let mut s = crate::Session::new(m, engine);
        s.trace_maps(true);
        s.run(8_000_000);
        // The boost target (resultados fase 7): read, but not on every lookup.
        let want = 0x0009_DADC;
        let t = s.maps.clone().unwrap();
        assert!(t.lock().unwrap().seen.contains_key(&want));
        s.break_on_map(Some(want));
        let stop = s.run(80_000_000);
        assert_eq!(stop, sh7059::Stop::Breakpoint);
        assert_eq!(s.m.cpu.r[4], want);
        assert_eq!(Some(s.m.cpu.pc), s.masters().d3);
        // Going on, the lookup it stopped at is traced.
        let before = t.lock().unwrap().seen[&want].calls;
        s.break_on_map(None);
        s.run(1_000);
        assert!(t.lock().unwrap().seen[&want].calls > before);
    }
}

#[cfg(test)]
mod cost {
    #[test]
    #[ignore]
    fn probe_cost() {
        let f = std::fs::read(std::env::var("SH705X_FLASH").unwrap()).unwrap();
        let mut best = [0.0f64; 2];
        for round in 0..12 {
            let probing = round % 2 == 1;
            let mut m = crate::machine(&f, None, board::DEFAULT_LAYOUT).unwrap();
            crate::Engine::idle().apply(&mut m);
            let mut t = probing.then(|| super::MapTrace::attach(&mut m, super::find_masters(&f)));
            let start = std::time::Instant::now();
            let c0 = m.cpu.cycles;
            while m.cpu.cycles - c0 < 2 * 80_000_000 {
                m.run(40_000);
                if let Some(t) = t.as_mut() {
                    t.take(&mut m);
                }
            }
            let x = (m.cpu.cycles - c0) as f64 / 80e6 / start.elapsed().as_secs_f64();
            best[probing as usize] = best[probing as usize].max(x);
        }
        eprintln!(
            "best: without {:.2}x, with probes {:.2}x real time",
            best[0], best[1]
        );
    }
}
