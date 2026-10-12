//! Advanced timer unit II (manual section 11): the part the Astra's firmware uses
//! without an engine attached.
//!
//! Modelled:
//! - prescalers PSCR1–4 (φ' = Pφ / (PSCR + 1)) and start registers TSTR1–3;
//! - channel 0: the 32-bit free-running counter TCNT0 on φ' of PSCR1, its overflow flag
//!   OVF0 and interrupt OVI0 (the firmware times its diagnostic protocol on TCNT0); and
//!   its input captures: an edge on TI0A–TI0D that TIOR0 selects copies TCNT0 into
//!   ICR0A–ICR0D and sets ICF0A–ICF0D, at the edge's own cycle;
//! - channel 10, the tooth part: TCNT10A, 32 bits on the φ' of PSCR4 while STR10 is set;
//!   on each TI10 edge that TCR10 selects, TCNT10A goes to ICR10A and restarts at 1,
//!   TCNT10B counts the edge (setting CMF10B when it counts on from OCR10B), ICF10A is
//!   set, and ICR10A shifted by the TIOR10 multiplier goes to RLD10C unless RLDEN;
//!   OCR10A against TCNT10A (CMF10A); interrupts CMI10A, CMI10B and ICI10A;
//! - channel 10, the multiplied clock (11.3.12): TCNT10C counts RLD10C down on the
//!   channel's clock and reloads, giving the multiplied clock AGCK1; each TI10 edge
//!   reloads it when RLD10C is loaded (RLDEN clear), so AGCK1 divides each tooth
//!   interval by the TIOR10 multiplier. TCNT10G counts AGCK1 and restarts at 0 on each
//!   TI10 edge; with IO10G = 001 it is compared with GR10G (CMF10G, interrupt CMI10G).
//!   With IREG set, CMF10G is set at the match but its interrupt is requested only from
//!   the next TI10 edge on ([`Atu::held10`]);
//! - channel 8, one-shot pulses started by software: setting a DSTR bit starts DCNT8x
//!   counting down on the clock TCR8 selects; TO8x is high until it reaches 0, when the
//!   DSTR bit clears, OSF8x is set (interrupt OSI8x) and the pulse is logged in
//!   [`Atu::pulses`];
//! - offset one-shot pulses started by channel 2 (11.3.6): when TCNT2B leaves OCR2x
//!   (x = A..H) and TCNR connects DCNT8I..P to it, that down-counter starts as if its
//!   DSTR bit had been set, after loading RLDR8 if RLDENR enables the reload and RLDR8
//!   is not 0; a counter at 0 or already running does not start;
//! - channel 2B: TCNT2B and its eight output compare registers OCR2A–H (CMF2A–H,
//!   OVF2B, interrupts CMI2A–H and OVI2B);
//! - channels 3 to 5: TCNT3–5, general registers used as output compare (IMF, counter
//!   clearing with CCI), overflow (OVF), interrupts IMI and OVI;
//! - channels 6 and 7, PWM timers: each of the eight TCNTs counts 1 to CYLR; on reaching
//!   CYLR it sets CMF, starts again at 1 and BFR is copied to DTR. The output is high from
//!   the start of a cycle until TCNT reaches DTR (11.3.9);
//! - every TSR, modelled channel or not: flags are cleared by writing 0 and never set by
//!   software;
//! - flag timing (11.4.1): a compare match is signalled "in the last state of the match",
//!   when TCNT leaves the matched value, and an overflow when it leaves H'FFFF. So a
//!   counter cleared on a match runs 0..=GR (period GR + 1), a PWM timer runs 1..=CYLR
//!   (period CYLR), and a counter started on its compare value matches on the first count.
//!
//! Not modelled yet: input capture on channels 1 to 5 (their GRs), the capture of
//! ICR0D on a channel 10 compare match, the interval interrupts of channel 0 (ITVRR),
//! the corrected angle clock of channel 10 (TCNT10D–F, TCCLR10, AGCKM) and its noise
//! canceler, one-shot pulses started by channel 1 or by the GRs of channel 2 (TCNR, TRGMDR),
//! the termination of one-shot pulses by compare matches (OTR),
//! channels 1, 2A, 8, 9, 10 and 11, the PWM mode of
//! channels 3 to 5 (TMDR), the complementary PWM mode of channel 6 (PMDR), and the timing
//! of flag setting within one count.
//!
//! The registers live in the chip's register storage; this module keeps only what moves
//! with time (the counters) and acts on the storage at events and writes.

use crate::IO_BASE;

/// Every timer status register (manual 11.1.3).
pub const TSR: [u32; 12] = [
    0xFFFF_F42C,
    0xFFFF_F45E,
    0xFFFF_F460,
    0xFFFF_F62C,
    0xFFFF_F62E,
    0xFFFF_F480,
    0xFFFF_F522,
    0xFFFF_F5A2,
    0xFFFF_F66A,
    0xFFFF_F69E,
    0xFFFF_F6E8,
    0xFFFF_F5CA,
];

/// First ATU-II register.
pub const BASE: u32 = 0xFFFF_F400;
/// One past the last ATU-II register.
pub const END: u32 = 0xFFFF_F700;

const TSTR2: u32 = 0xFFFF_F400; // channels 6 and 7
const TCNT0: u32 = 0xFFFF_F430;
const TSR0: u32 = 0xFFFF_F42C;
const TIER0: u32 = 0xFFFF_F42E;
const TIOR0: u32 = 0xFFFF_F42A;
const DCNT8: u32 = 0xFFFF_F640;
const DSTR: u32 = 0xFFFF_F666;
const TCR8: u32 = 0xFFFF_F668;
const TSR8: u32 = 0xFFFF_F66A;
const TIER8: u32 = 0xFFFF_F66C;
const RLDR8: u32 = 0xFFFF_F660;
const TCNR: u32 = 0xFFFF_F662;
const RLDENR: u32 = 0xFFFF_F66E;
/// Pulses kept in [`Atu::pulses`].
const PULSES_KEPT: usize = 4096;
const TCNT10A: u32 = 0xFFFF_F6C0;
const TCNT10B: u32 = 0xFFFF_F6C4;
const ICR10A: u32 = 0xFFFF_F6D0;
const OCR10A: u32 = 0xFFFF_F6D4;
const OCR10B: u32 = 0xFFFF_F6D8;
const RLD10C: u32 = 0xFFFF_F6DA;
const TIOR10: u32 = 0xFFFF_F6E2;
const TCNT10C: u32 = 0xFFFF_F6C6;
const TCNT10G: u32 = 0xFFFF_F6CE;
const GR10G: u32 = 0xFFFF_F6DC;
const TCR10: u32 = 0xFFFF_F6E4;
const TSR10: u32 = 0xFFFF_F6E8;
const TIER10: u32 = 0xFFFF_F6EA;
/// ICR0A, ICR0B, ICR0C, ICR0D (manual table 11.3: D sits below TCNT0).
const ICR0: [u32; 4] = [0xFFFF_F434, 0xFFFF_F438, 0xFFFF_F43C, 0xFFFF_F420];
const TSTR1: u32 = 0xFFFF_F401; // channels 0 to 5 and 10
const PSCR: [u32; 4] = [0xFFFF_F404, 0xFFFF_F406, 0xFFFF_F408, 0xFFFF_F40A];

const CH2: u32 = 0xFFFF_F600;
const TSR2B: u32 = 0xFFFF_F62E;
const TIER2B: u32 = 0xFFFF_F632;
const TSR3: u32 = 0xFFFF_F480;
const TIER3: u32 = 0xFFFF_F482;
const CH345: [u32; 3] = [0xFFFF_F4A0, 0xFFFF_F4C0, 0xFFFF_F4E0];
const CH67: [u32; 2] = [0xFFFF_F500, 0xFFFF_F580];

#[inline]
fn r8(io: &[u8], a: u32) -> u8 {
    io[(a - IO_BASE) as usize]
}

#[inline]
fn r16(io: &[u8], a: u32) -> u16 {
    let o = (a - IO_BASE) as usize;
    u16::from_be_bytes([io[o], io[o + 1]])
}

#[inline]
fn r32(io: &[u8], a: u32) -> u32 {
    ((r16(io, a) as u32) << 16) | r16(io, a + 2) as u32
}

#[inline]
fn w16(io: &mut [u8], a: u32, v: u16) {
    let o = (a - IO_BASE) as usize;
    io[o..o + 2].copy_from_slice(&v.to_be_bytes());
}

#[inline]
fn set16(io: &mut [u8], a: u32, bits: u16) {
    let v = r16(io, a) | bits;
    w16(io, a, v);
}

/// One counter: its value at `base` and how many CPU cycles each count takes (0 when
/// stopped or on an external clock).
#[derive(Clone, Copy, Debug, Default)]
pub struct Counter {
    /// Value at `base`.
    pub value: u16,
    /// CPU cycle of `value`.
    pub base: u64,
    /// CPU cycles per count; 0 = not counting.
    pub cpc: u64,
}

impl Counter {
    fn elapsed(&self, now: u64) -> u64 {
        (now - self.base).checked_div(self.cpc).unwrap_or(0)
    }
}

/// A PWM output of channel 6 or 7 as a waveform, in CPU cycles: high for `high` at the
/// start of each `period`, the periods starting at `origin` (and every `period` before and
/// after it).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Wave {
    /// Cycles per period (CYLR counts).
    pub period: u64,
    /// Cycles high at the start of each (DTR counts).
    pub high: u64,
    /// A cycle a period starts at, below `period`.
    pub origin: u64,
}

impl Wave {
    /// The level at cycle `t`.
    pub fn level(&self, t: u64) -> bool {
        (t + self.period - self.origin) % self.period < self.high
    }
}

/// Channel 0's 32-bit counter: its value at `base` and CPU cycles per count (0 when
/// stopped).
#[derive(Clone, Copy, Debug, Default)]
pub struct Counter32 {
    /// Value at `base`.
    pub value: u32,
    /// CPU cycle of `value`.
    pub base: u64,
    /// CPU cycles per count; 0 = not counting.
    pub cpc: u64,
}

impl Counter32 {
    fn elapsed(&self, now: u64) -> u64 {
        (now - self.base).checked_div(self.cpc).unwrap_or(0)
    }

    /// The value at cycle `at`, which may be before `base` (counting wraps).
    fn value_at(&self, at: u64) -> u32 {
        if self.cpc == 0 {
            return self.value;
        }
        if at >= self.base {
            self.value
                .wrapping_add(((at - self.base) / self.cpc) as u32)
        } else {
            self.value
                .wrapping_sub((self.base - at).div_ceil(self.cpc) as u32)
        }
    }

    /// The cycle it next leaves H'FFFFFFFF, if counting.
    fn overflow(&self) -> Option<u64> {
        (self.cpc != 0).then(|| self.base + ((0xFFFF_FFFF - self.value) as u64 + 1) * self.cpc)
    }
}

/// Channel 10's multiplied clock AGCK1 and the counter TCNT10G on it.
#[derive(Clone, Copy, Debug, Default)]
pub struct AngleClock {
    /// CPU cycles between AGCK1 pulses (RLD10C counts of the channel clock); 0 = none.
    pub period: u64,
    /// Cycle of a TCNT10C reload, from which the pulses follow every `period`.
    pub phase: u64,
    /// TCNT10G at cycle `from`.
    pub g: u16,
    /// Cycle from which TCNT10G counts the pulses.
    pub from: u64,
    /// CMF10G was set with IREG: its interrupt waits for the next TI10 edge.
    held: bool,
    /// The compare match with the current GR10G has been taken since `from`.
    matched: bool,
}

impl AngleClock {
    /// AGCK1 pulses in (a, b].
    fn pulses(&self, a: u64, b: u64) -> u64 {
        if self.period == 0 || b <= a {
            return 0;
        }
        let k = |t: u64| (t.max(self.phase) - self.phase) / self.period;
        k(b) - k(a)
    }

    /// TCNT10G at `now`.
    fn value(&self, now: u64) -> u16 {
        self.g.wrapping_add(self.pulses(self.from, now) as u16)
    }

    /// The cycle of the pulse that brings TCNT10G to `target`, if it counts.
    fn reaches(&self, target: u16) -> Option<u64> {
        if self.period == 0 {
            return None;
        }
        let n = target.wrapping_sub(self.g) as u64;
        let n = if n == 0 { 65536 } else { n };
        let first = self.from.max(self.phase);
        let k0 = (first - self.phase) / self.period;
        Some(self.phase + (k0 + n) * self.period)
    }
}

/// Which counter: 0 = TCNT2B, 1..=3 = TCNT3..5, 4..=7 = TCNT6A..D, 8..=11 = TCNT7A..D.
const N: usize = 12;

fn tcnt_addr(i: usize) -> u32 {
    match i {
        0 => CH2 + 2,
        1..=3 => CH345[i - 1],
        _ => CH67[(i - 4) / 4] + 2 * ((i - 4) % 4) as u32,
    }
}

/// The modelled ATU-II counters.
#[derive(Clone, Debug, Default)]
pub struct Atu {
    /// The twelve modelled 16-bit counters.
    pub cnt: [Counter; N],
    /// TCNT0.
    pub tcnt0: Counter32,
    /// TCNT10A.
    pub tcnt10a: Counter32,
    /// CMF10A has been set since TCNT10A last restarted or OCR10A changed.
    cmp10a_done: bool,
    /// TI10 edges counted.
    pub ti10_edges: u64,
    /// The multiplied clock AGCK1 and TCNT10G.
    pub agck: AngleClock,
    /// Channel 8 down-counters running: (start cycle, cycles per count, count at start).
    one_shot: [Option<(u64, u64, u16)>; 16],
    /// One-shot pulses that have ended: (output 0 = TO8A .. 15 = TO8P, start, end), the
    /// latest [`PULSES_KEPT`].
    pub pulses: std::collections::VecDeque<(u8, u64, u64)>,
    /// One-shot pulses ended since power-on.
    pub pulse_count: u64,
    /// Earliest event, CPU cycle.
    next: u64,
    /// Events taken: compare matches and overflows of channels 2B to 5, and cycle ends of
    /// channels 6 and 7.
    pub events: u64,
    /// Input captures taken.
    pub captures: u64,
}

impl Atu {
    /// Initial values that are not zero after a power-on reset (manual 11.1.3).
    pub fn reset(&mut self, io: &mut [u8]) {
        for a in (CH2 + 4..CH2 + 0x24).step_by(2) {
            w16(io, a, 0xFFFF); // GR2A-H, OCR2A-H
        }
        for &c in &CH345 {
            for k in 0..4 {
                w16(io, c + 2 + 2 * k, 0xFFFF); // GR
            }
        }
        for &c in &CH67 {
            for k in 0..4 {
                w16(io, c + 2 * k, 0x0001); // TCNT
                w16(io, c + 8 + 2 * k, 0xFFFF); // CYLR
                w16(io, c + 0x10 + 2 * k, 0xFFFF); // BFR
                w16(io, c + 0x18 + 2 * k, 0xFFFF); // DTR
            }
        }
        for (i, c) in self.cnt.iter_mut().enumerate() {
            c.value = r16(io, tcnt_addr(i));
        }
        // Channel 10 (11.2.26).
        w16(io, TCNT10A + 2, 1);
        self.tcnt10a.value = 1;
        w16(io, 0xFFFF_F6C6, 1); // TCNT10C
        w16(io, 0xFFFF_F6CC, 1); // TCNT10F
        w16(io, OCR10A, 0xFFFF);
        w16(io, OCR10A + 2, 0xFFFF);
        io[(OCR10B - IO_BASE) as usize] = 0xFF;
        w16(io, 0xFFFF_F6DC, 0xFFFF); // GR10G
        io[(0xFFFF_F6E0 - IO_BASE) as usize] = 0xFF; // NCR10
        self.next = u64::MAX;
    }

    /// CPU cycles per count of counter `i` from the current registers, or 0.
    fn cycles_per_count(io: &[u8], i: usize) -> u64 {
        let tstr1 = r8(io, TSTR1);
        let tstr2 = r8(io, TSTR2);
        let (running, pscr, sel) = match i {
            // TCNT2B: STR1B,2B (TSTR1 bit 3); TCR2B (F62A) CKSELB3-0.
            0 => (tstr1 & 0x08 != 0, PSCR[0], r8(io, 0xFFFF_F62A) & 0x0F),
            // TCNT3-5: STR3-5 (TSTR1 bits 4-6); TCR3-5 CKSEL3-0.
            1..=3 => (
                tstr1 & (0x10 << (i - 1)) != 0,
                PSCR[0],
                r8(io, CH345[i - 1] + 0x0C) & 0x0F,
            ),
            // TCNT6A-D, 7A-D: TSTR2 bits 0-7; TCR6A/7A (B: bits 6-4, A: 2-0), TCR6B/7B (D, C).
            _ => {
                let ch = (i - 4) / 4;
                let k = (i - 4) % 4;
                let tcr = r8(io, CH67[ch] + 0x21 - (k as u32 / 2));
                let sel = if k % 2 == 0 { tcr & 7 } else { (tcr >> 4) & 7 };
                (tstr2 & (1 << (i - 4)) != 0, PSCR[1 + ch], sel)
            }
        };
        if !running || sel > 5 {
            return 0;
        }
        let pscr = (r8(io, pscr) & 0x1F) as u64;
        (4 * (pscr + 1)) << sel
    }

    /// Brings every counter's value up to `now` with its current clock (call before the
    /// registers that set the clock change).
    pub fn resync(&mut self, now: u64) {
        for c in [&mut self.tcnt0, &mut self.tcnt10a] {
            if c.cpc != 0 {
                let n = c.elapsed(now);
                c.value = c.value.wrapping_add(n as u32);
                c.base += n * c.cpc;
            } else {
                c.base = now;
            }
        }
        for c in &mut self.cnt {
            if c.cpc != 0 {
                let n = c.elapsed(now);
                c.value = c.value.wrapping_add(n as u16);
                c.base += n * c.cpc;
            } else {
                c.base = now;
            }
        }
    }

    /// Writes the current counter values into the register storage, for a read.
    pub fn store(&self, now: u64, io: &mut [u8]) {
        for (k, run) in self.one_shot.iter().enumerate() {
            if let Some((start, cpc, n)) = *run {
                let done = ((now - start) / cpc).min(n as u64) as u16;
                w16(io, DCNT8 + 2 * k as u32, n - done);
            }
        }
        if self.agck.period != 0 {
            w16(io, TCNT10G, self.agck.value(now));
            let cpc = self.tcnt10a.cpc.max(1);
            let into = (now.max(self.agck.phase) - self.agck.phase) % self.agck.period;
            let left = (self.agck.period - into) / cpc;
            w16(io, TCNT10C, left.clamp(1, 0xFFFF) as u16);
        }
        for (c, a) in [(&self.tcnt0, TCNT0), (&self.tcnt10a, TCNT10A)] {
            let v = c.value.wrapping_add(c.elapsed(now) as u32);
            w16(io, a, (v >> 16) as u16);
            w16(io, a + 2, v as u16);
        }
        for (i, c) in self.cnt.iter().enumerate() {
            // Events up to `now` have been taken, so this does not cross a match or CYLR.
            w16(
                io,
                tcnt_addr(i),
                c.value.wrapping_add(c.elapsed(now) as u16),
            );
        }
    }

    /// Takes a write to the ATU-II registers. `before` holds the bytes from `addr` before
    /// it, the storage already holds the written ones. Call [`Atu::resync`] first.
    pub fn load(&mut self, now: u64, addr: u32, before: &[u8], io: &mut [u8]) {
        let n = before.len() as u32;
        // Status registers: written bits can only clear flags.
        for &t in &TSR {
            for b in 0..2u32 {
                let a = t + b;
                if (addr..addr + n).contains(&a) {
                    let old = before[(a - addr) as usize];
                    let o = (a - IO_BASE) as usize;
                    io[o] &= old;
                }
            }
        }
        // A counter written by software takes the new value from now.
        for (i, c) in self.cnt.iter_mut().enumerate() {
            let t = tcnt_addr(i);
            if t < addr + n && addr < t + 2 {
                c.value = r16(io, t);
                c.base = now;
            }
        }
        for (i, c) in self.cnt.iter_mut().enumerate() {
            c.cpc = Self::cycles_per_count(io, i);
        }
        if TCNT0 < addr + n && addr < TCNT0 + 4 {
            self.tcnt0.value = r32(io, TCNT0);
            self.tcnt0.base = now;
        }
        // TCNT0 counts on the φ' of PSCR1 only, while STR0 is set.
        self.tcnt0.cpc = if r8(io, TSTR1) & 1 != 0 {
            4 * ((r8(io, PSCR[0]) & 0x1F) as u64 + 1)
        } else {
            0
        };
        if TCNT10A < addr + n && addr < TCNT10A + 4 {
            self.tcnt10a.value = r32(io, TCNT10A);
            self.tcnt10a.base = now;
            self.cmp10a_done = false;
        }
        if OCR10A < addr + n && addr < OCR10A + 4 {
            self.cmp10a_done = false;
        }
        if TCNT10G < addr + n && addr < TCNT10G + 2 {
            self.agck.g = r16(io, TCNT10G);
            self.agck.from = now;
            self.agck.matched = false;
        } else if GR10G < addr + n && addr < GR10G + 2 || (addr..addr + n).contains(&TIOR10) {
            // A new compare value or mode: count from now, so a match already taken is
            // not taken again.
            self.agck.g = self.agck.value(now);
            self.agck.from = now;
            self.agck.matched = false;
        }
        if DSTR < addr + n && addr < DSTR + 2 {
            let old = u16::from_be_bytes([
                if addr <= DSTR {
                    before[(DSTR - addr) as usize]
                } else {
                    io[(DSTR - IO_BASE) as usize]
                },
                if addr <= DSTR + 1 && DSTR + 1 < addr + n {
                    before[(DSTR + 1 - addr) as usize]
                } else {
                    io[(DSTR + 1 - IO_BASE) as usize]
                },
            ]);
            let new = r16(io, DSTR);
            for k in 0..16 {
                let bit = 1u16 << k;
                if new & bit != 0 && old & bit == 0 {
                    let count = r16(io, DCNT8 + 2 * k as u32);
                    self.one_shot[k] = Some((now, Self::dcnt_cycles(io, k), count));
                } else if new & bit == 0 && old & bit != 0 {
                    // Stopped by software: the count stays where it was, no flag.
                    if let Some((start, cpc, c)) = self.one_shot[k].take() {
                        let done = ((now - start) / cpc).min(c as u64) as u16;
                        w16(io, DCNT8 + 2 * k as u32, c - done);
                    }
                }
            }
        }
        // TCNT10A counts on the φ' of PSCR4 while STR10 is set.
        self.tcnt10a.cpc = if r8(io, TSTR1) & 0x80 != 0 {
            4 * ((r8(io, PSCR[3]) & 0x1F) as u64 + 1)
        } else {
            0
        };
        self.schedule(io);
    }

    /// CPU cycles per count of down-counter DCNT8x (`k` = 0 for A .. 15 for P): the
    /// clock TCR8 selects, A-H in its low bits and I-P in its high bits.
    fn dcnt_cycles(io: &[u8], k: usize) -> u64 {
        let tcr = r8(io, TCR8);
        let sel = if k < 8 { tcr & 7 } else { (tcr >> 4) & 7 };
        (4 * ((r8(io, PSCR[0]) & 0x1F) as u64 + 1)) << sel.min(5)
    }

    /// Channel 2's down-count start trigger for DCNT8x (`k` = 8..=15) at cycle `at`
    /// (11.3.6): DST8x is set and DCNT8x starts, after the reload, unless TCNR leaves it
    /// unconnected, it is running, or its count is 0.
    fn trigger_one_shot(&mut self, io: &mut [u8], k: usize, at: u64) {
        let bit = 1u16 << k;
        if r16(io, TCNR) & bit == 0 || self.one_shot[k].is_some() {
            return;
        }
        let dcnt = DCNT8 + 2 * k as u32;
        let reload = r16(io, RLDR8);
        if r8(io, RLDENR) & 0x80 != 0 && reload != 0 {
            w16(io, dcnt, reload);
        }
        let count = r16(io, dcnt);
        if count == 0 {
            return;
        }
        set16(io, DSTR, bit);
        self.one_shot[k] = Some((at, Self::dcnt_cycles(io, k), count));
    }

    /// The compare values counter `i` is watched for: OCR2A-H, the compare GRs of
    /// channels 3-5, or CYLR for a PWM timer.
    fn targets(io: &[u8], i: usize) -> ([u16; 8], usize) {
        let mut t = [0u16; 8];
        let mut n = 0;
        match i {
            0 => {
                for k in 0..8 {
                    t[n] = r16(io, CH2 + 0x14 + 2 * k);
                    n += 1;
                }
            }
            1..=3 => {
                for g in 0..4 {
                    if Self::gr_compare(io, i, g).is_some() {
                        t[n] = r16(io, CH345[i - 1] + 2 + 2 * g as u32);
                        n += 1;
                    }
                }
            }
            _ => {
                let (ch, k) = ((i - 4) / 4, (i - 4) % 4);
                t[0] = r16(io, CH67[ch] + 8 + 2 * k as u32);
                n = 1;
            }
        }
        (t, n)
    }

    /// Counts until counter `i` next leaves a compare value or H'FFFF (11.4.1), and the
    /// value it leaves; None if it does not count.
    fn counts_to_event(&self, io: &[u8], i: usize) -> Option<(u64, u16)> {
        let c = &self.cnt[i];
        if c.cpc == 0 {
            return None;
        }
        let (t, n) = Self::targets(io, i);
        let leave = |x: u16| x.wrapping_sub(c.value) as u64 + 1;
        let mut best = (leave(0xFFFF), 0xFFFF);
        for &g in &t[..n] {
            if leave(g) < best.0 {
                best = (leave(g), g);
            }
        }
        Some(best)
    }

    /// For channels 3-5 (i = 1..=3) and GR g (0 = A .. 3 = D): Some(counter clear) if the
    /// GR is an output compare register (TIOR IO = 001, 010 or 011), else None.
    fn gr_compare(io: &[u8], i: usize, g: usize) -> Option<bool> {
        let base = CH345[i - 1];
        // TIORxA (base + 0x0B): A in bits 3-0, B in 7-4; TIORxB (base + 0x0A): C, D.
        let tior = r8(io, if g < 2 { base + 0x0B } else { base + 0x0A });
        let nib = if g % 2 == 0 { tior & 0x0F } else { tior >> 4 };
        matches!(nib & 7, 1..=3).then_some(nib & 8 != 0)
    }

    fn schedule(&mut self, io: &[u8]) {
        self.next = (0..N)
            .filter_map(|i| {
                self.counts_to_event(io, i)
                    .map(|(d, _)| self.cnt[i].base + d * self.cnt[i].cpc)
            })
            .min()
            .unwrap_or(u64::MAX)
            .min(self.tcnt0.overflow().unwrap_or(u64::MAX))
            .min(self.cmp10a_at(io).unwrap_or(u64::MAX))
            .min(self.cmp10g_at(io).unwrap_or(u64::MAX))
            .min(
                self.one_shot
                    .iter()
                    .flatten()
                    .map(|&(s, cpc, n)| s + n as u64 * cpc)
                    .min()
                    .unwrap_or(u64::MAX),
            );
    }

    /// The cycle TCNT10G reaches GR10G, if it is compared (IO10G = 001) and has not.
    fn cmp10g_at(&self, io: &[u8]) -> Option<u64> {
        if r8(io, TIOR10) & 7 != 1 || self.agck.matched {
            return None;
        }
        self.agck.reaches(r16(io, GR10G))
    }

    /// The cycle TCNT10A reaches OCR10A, if it is counting towards it.
    fn cmp10a_at(&self, io: &[u8]) -> Option<u64> {
        let c = &self.tcnt10a;
        let target = r32(io, OCR10A);
        (c.cpc != 0 && !self.cmp10a_done && target >= c.value)
            .then(|| c.base + (target - c.value) as u64 * c.cpc)
    }

    /// An edge on TI10 at cycle `at` (TCR10 says which edges count).
    pub fn ti10_edge(&mut self, io: &mut [u8], at: u64, rising: bool) {
        let taken = match r8(io, TCR10) & 3 {
            1 => rising,
            2 => !rising,
            3 => true,
            _ => false,
        };
        if !taken {
            return;
        }
        self.ti10_edges += 1;
        // TCNT10B counts even with STR10 clear.
        let b = (TCNT10B - IO_BASE) as usize;
        if io[b] == r8(io, OCR10B) {
            set16(io, TSR10, 0x0004); // CMF10B
        }
        io[b] = io[b].wrapping_add(1);
        if self.tcnt10a.cpc == 0 {
            return;
        }
        let v = self.tcnt10a.value_at(at);
        w16(io, ICR10A, (v >> 16) as u16);
        w16(io, ICR10A + 2, v as u16);
        set16(io, TSR10, 0x0002); // ICF10A
        let tior = r8(io, TIOR10);
        if tior & 0x80 == 0 {
            let shift = 5 + ((tior >> 4) & 3) as u32; // x32, x64, x128, x256
            let rld = (v >> shift).min(0xFFFF) as u16;
            w16(io, RLD10C, rld);
            // TCNT10C takes the new value too and starts again: AGCK1 restarts here.
            self.agck.period = rld as u64 * self.tcnt10a.cpc;
            self.agck.phase = at;
        }
        // TCNT10G restarts at 0; a CMF10G held for this edge (IREG) may interrupt now.
        self.agck.held = false;
        self.agck.g = 0;
        self.agck.from = at;
        self.agck.matched = false;
        self.tcnt10a.value = 1;
        self.tcnt10a.base = at;
        self.cmp10a_done = false;
        self.schedule(io);
    }

    /// The earliest cycle at which a counter reaches a compare value, its cycle end or
    /// overflows.
    pub fn next_event(&self) -> u64 {
        self.next
    }

    /// Takes every event due by `now`, setting flags and moving BFR to DTR in `io`.
    pub fn advance(&mut self, now: u64, io: &mut [u8]) {
        if now < self.next {
            return;
        }
        for k in 0..16 {
            if let Some((start, cpc, n)) = self.one_shot[k] {
                let end = start + n as u64 * cpc;
                if end <= now {
                    self.one_shot[k] = None;
                    w16(io, DCNT8 + 2 * k as u32, 0);
                    let dstr = r16(io, DSTR) & !(1 << k);
                    w16(io, DSTR, dstr);
                    set16(io, TSR8, 1 << k);
                    self.events += 1;
                    self.pulse_count += 1;
                    if self.pulses.len() == PULSES_KEPT {
                        self.pulses.pop_front();
                    }
                    self.pulses.push_back((k as u8, start, end));
                }
            }
        }
        if let Some(t) = self.cmp10a_at(io)
            && t <= now
        {
            self.events += 1;
            self.cmp10a_done = true;
            set16(io, TSR10, 0x0001); // CMF10A
        }
        if let Some(t) = self.cmp10g_at(io)
            && t <= now
        {
            self.events += 1;
            self.agck.matched = true;
            set16(io, TSR10, 0x0008); // CMF10G
            self.agck.held = true;
        }
        while let Some(t) = self.tcnt0.overflow() {
            if t > now {
                break;
            }
            self.events += 1;
            self.tcnt0.value = 0;
            self.tcnt0.base = t;
            set16(io, TSR0, 0x0010); // OVF0
        }
        for i in 0..N {
            while let Some((d, x)) = self.counts_to_event(io, i) {
                let t = self.cnt[i].base + d * self.cnt[i].cpc;
                if t > now {
                    break;
                }
                self.events += 1;
                self.cnt[i].base = t;
                self.cnt[i].value = x.wrapping_add(1);
                self.on_leave(io, i, x);
            }
        }
        self.schedule(io);
    }

    /// Counter `i` has just left value `v`: flags, clearing, cycle end. Its value is
    /// already `v + 1`; a clearing match sets it to 0 (or 1 for a PWM timer).
    fn on_leave(&mut self, io: &mut [u8], i: usize, v: u16) {
        let overflow = v == 0xFFFF;
        match i {
            0 => {
                if overflow {
                    set16(io, TSR2B, 0x0100);
                }
                let at = self.cnt[0].base;
                for k in 0..8 {
                    if r16(io, CH2 + 0x14 + 2 * k) == v {
                        set16(io, TSR2B, 1 << k);
                        self.trigger_one_shot(io, 8 + k as usize, at);
                    }
                }
            }
            1..=3 => {
                let shift = 5 * (i - 1) as u16; // ch3 bits 0-4, ch4 5-9, ch5 10-14
                if overflow {
                    set16(io, TSR3, 1 << (shift + 4));
                }
                let mut clear = false;
                for g in 0..4 {
                    if let Some(cci) = Self::gr_compare(io, i, g)
                        && r16(io, CH345[i - 1] + 2 + 2 * g as u32) == v
                    {
                        set16(io, TSR3, 1 << (shift + g as u16));
                        clear |= cci;
                    }
                }
                if clear {
                    self.cnt[i].value = 0;
                }
            }
            _ => {
                let (ch, k) = ((i - 4) / 4, (i - 4) % 4);
                let c = CH67[ch];
                if v == r16(io, c + 8 + 2 * k as u32) {
                    set16(io, c + 0x22, 1 << k); // CMF in TSR6/7
                    let bfr = r16(io, c + 0x10 + 2 * k as u32);
                    w16(io, c + 0x18 + 2 * k as u32, bfr); // DTR <- BFR
                    self.cnt[i].value = 1;
                }
            }
        }
    }

    /// An edge on TI0A–TI0D (`input` 0..=3) at cycle `at`: if TIOR0 selects that edge,
    /// TCNT0 at that cycle goes to the ICR0 register and its ICF0 flag is set. Returns
    /// whether it was captured.
    pub fn capture(&mut self, io: &mut [u8], at: u64, input: usize, rising: bool) -> bool {
        let sel = (r8(io, TIOR0) >> (2 * input)) & 3;
        let taken = match sel {
            1 => rising,
            2 => !rising,
            3 => true,
            _ => false,
        };
        if taken {
            let v = self.tcnt0.value_at(at);
            w16(io, ICR0[input], (v >> 16) as u16);
            w16(io, ICR0[input] + 2, v as u16);
            set16(io, TSR0, 1 << input);
            self.captures += 1;
        }
        taken
    }

    /// TSR10 flags whose interrupt is held back: CMF10G until the TI10 edge after its
    /// match when IREG is set (TIER10 bit 4).
    pub fn held10(&self, io: &[u8]) -> u16 {
        if self.agck.held && r16(io, TIER10) & 0x10 != 0 {
            0x0008
        } else {
            0
        }
    }

    /// The PWM output of channel 6 or 7 (`ch` = 0 or 1), timer `k` (0 = A .. 3 = D), as a
    /// waveform, if that timer is running: the counter runs from 0 to CYLR, the output is
    /// high below DTR (as [`Atu::duty`] reads it).
    pub fn wave(&self, io: &[u8], ch: usize, k: usize) -> Option<Wave> {
        let c = &self.cnt[4 + 4 * ch + k];
        if c.cpc == 0 {
            return None;
        }
        let regs = CH67[ch];
        let cylr = r16(io, regs + 8 + 2 * k as u32).max(1) as u64;
        let dtr = (r16(io, regs + 0x18 + 2 * k as u32) as u64).min(cylr);
        let period = cylr * c.cpc;
        // The counter was 0 at base - value × cpc (before power-on, maybe: taken modulo).
        let zero = (c.base as i128 - c.value as i128 * c.cpc as i128).rem_euclid(period as i128);
        Some(Wave {
            period,
            high: dtr * c.cpc,
            origin: zero as u64,
        })
    }

    /// The PWM output of channel 6 or 7 (`ch` = 0 or 1), timer `k` (0 = A .. 3 = D), as a
    /// duty fraction DTR / CYLR, if that timer is running.
    pub fn duty(&self, io: &[u8], ch: usize, k: usize) -> Option<f64> {
        let i = 4 + 4 * ch + k;
        if self.cnt[i].cpc == 0 {
            return None;
        }
        let c = CH67[ch];
        let cylr = r16(io, c + 8 + 2 * k as u32).max(1) as f64;
        let dtr = r16(io, c + 0x18 + 2 * k as u32) as f64;
        Some((dtr / cylr).min(1.0))
    }
}

/// Status and enable register pairs of the modelled channels.
const FLAG_PAIRS: [(u32, u32); 7] = [
    (TSR8, TIER8),
    (TSR0, TIER0),
    (TSR10, TIER10),
    (TSR2B, TIER2B),
    (TSR3, TIER3),
    (CH67[0] + 0x22, CH67[0] + 0x24),
    (CH67[1] + 0x22, CH67[1] + 0x24),
];

/// Calls `f` with (vector, IPR index, shift) of every requesting source, in vector order;
/// `held10` masks TSR10 flags that may not interrupt yet ([`Atu::held10`]). Most of the
/// time no flag is both set and enabled, and that is all it checks.
#[inline]
pub fn for_each_request(io: &[u8], held10: u16, mut f: impl FnMut(u32, usize, u32)) {
    if FLAG_PAIRS
        .iter()
        .all(|&(s, e)| r16(io, s) & r16(io, e) == 0)
    {
        return;
    }
    for (v, ipr, shift, on) in requests(io, held10) {
        if on {
            f(v, ipr, shift);
        }
    }
}

/// Interrupt requests of the modelled channels: (vector, IPR index, shift, requesting).
pub fn requests(io: &[u8], held10: u16) -> [(u32, usize, u32, bool); 58] {
    let tsr2b = r16(io, TSR2B);
    let tier2b = r16(io, TIER2B);
    let tsr3 = r16(io, TSR3);
    let tier3 = r16(io, TIER3);
    let on2 = |bit: u16| tsr2b & tier2b & bit != 0;
    let on3 = |bit: u16| tsr3 & tier3 & bit != 0;
    let pwm = |ch: usize, k: u16| {
        let c = CH67[ch];
        r16(io, c + 0x22) & r16(io, c + 0x24) & (1 << k) != 0
    };
    // IPRE = index 4, IPRF = 5, IPRG = 6 (manual table 7.3).
    let mut out = [(0, 0, 0, false); 58];
    let mut n = 0;
    let mut push = |v: u32, ipr: usize, shift: u32, on: bool| {
        out[n] = (v, ipr, shift, on);
        n += 1;
    };
    let tsr0 = r16(io, TSR0) & r16(io, TIER0);
    // ICI0A, ICI0B in IPRC 3-0; ICI0C, ICI0D in IPRD 15-12; OVI0 in IPRD 11-8.
    push(84, 2, 0, tsr0 & 1 != 0);
    push(86, 2, 0, tsr0 & 2 != 0);
    push(88, 3, 12, tsr0 & 4 != 0);
    push(90, 3, 12, tsr0 & 8 != 0);
    push(92, 3, 8, tsr0 & 0x10 != 0);
    for k in 0..4 {
        push(108 + k, 4, 8, on2(1 << k)); // CMI2A-D
    }
    for k in 4..8 {
        push(108 + k, 4, 4, on2(1 << k)); // CMI2E-H
    }
    push(116, 4, 0, on2(0x0100)); // OVI2B (shared with OVI2A)
    for (ch, (imi, ovi, shift_imi, shift_ovi, ipr)) in [
        (120, 124, 12, 8, 5),
        (128, 132, 4, 0, 5),
        (136, 140, 12, 8, 6),
    ]
    .into_iter()
    .enumerate()
    {
        let s = 5 * ch as u16;
        for g in 0..4 {
            push(imi + g as u32, ipr, shift_imi, on3(1 << (s + g)));
        }
        push(ovi, ipr, shift_ovi, on3(1 << (s + 4)));
    }
    for k in 0..4 {
        push(144 + k as u32, 6, 4, pwm(0, k)); // CMI6A-D
    }
    for k in 0..4 {
        push(148 + k as u32, 6, 0, pwm(1, k)); // CMI7A-D
    }
    // Channel 8: OSI8A-P, vectors 152-167, four to a field of IPRH.
    let t8 = r16(io, TSR8) & r16(io, TIER8);
    for k in 0..16u32 {
        push(152 + k, 7, 12 - 4 * (k / 4), t8 & (1 << k) != 0);
    }
    // Channel 10: CMI10A, CMI10B in IPRI 7-4; ICI10A and CMI10G share vector 180, IPRI 3-0.
    let t10 = r16(io, TSR10) & r16(io, TIER10) & !held10;
    push(176, 8, 4, t10 & 1 != 0);
    push(178, 8, 4, t10 & 4 != 0);
    push(180, 8, 0, t10 & 0xA != 0);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::IO_SIZE;

    fn io() -> (Atu, Vec<u8>) {
        let mut io = vec![0u8; IO_SIZE];
        let mut a = Atu::default();
        a.reset(&mut io);
        (a, io)
    }

    fn write16(a: &mut Atu, io: &mut [u8], now: u64, addr: u32, v: u16) {
        a.resync(now);
        a.store(now, io);
        let o = (addr - IO_BASE) as usize;
        let before = io[o..o + 2].to_vec();
        io[o..o + 2].copy_from_slice(&v.to_be_bytes());
        a.load(now, addr, &before, io);
    }

    fn write8(a: &mut Atu, io: &mut [u8], now: u64, addr: u32, v: u8) {
        a.resync(now);
        a.store(now, io);
        let o = (addr - IO_BASE) as usize;
        let before = vec![io[o]];
        io[o] = v;
        a.load(now, addr, &before, io);
    }

    #[test]
    fn pwm_timer_cycle_flag_and_buffer_transfer() {
        let (mut a, mut io) = io();
        write8(&mut a, &mut io, 0, 0xFFFF_F406, 4); // PSCR2: Pφ/5
        write8(&mut a, &mut io, 0, 0xFFFF_F521, 0x11); // TCR6A: A and B at φ'/2
        write16(&mut a, &mut io, 0, 0xFFFF_F508, 0x1F40); // CYLR6A = 8000
        write16(&mut a, &mut io, 0, 0xFFFF_F510, 0x03E8); // BFR6A = 1000
        write8(&mut a, &mut io, 0, TSTR2, 0x01); // start 6A
        let cpc = 4 * 5 * 2; // φ/Pφ x (PSCR + 1) x 2
        assert_eq!(a.cnt[4].cpc, cpc);
        // From 1, TCNT leaves 8000 after 8000 counts.
        assert_eq!(a.next_event(), 8000 * cpc);
        a.store(7999 * cpc, &mut io);
        assert_eq!(r16(&io, 0xFFFF_F500), 8000, "at CYLR, not yet matched");
        a.advance(8000 * cpc, &mut io);
        a.store(8000 * cpc, &mut io);
        assert_eq!(r16(&io, 0xFFFF_F500), 1, "back to 1");
        assert_eq!(r16(&io, 0xFFFF_F522) & 1, 1, "CMF6A");
        assert_eq!(r16(&io, 0xFFFF_F518), 0x03E8, "BFR moved to DTR");
        assert_eq!(a.duty(&io, 0, 0), Some(1000.0 / 8000.0));
        // Then every 8000 counts: 1, 2, ..., 8000.
        assert_eq!(a.next_event(), 16000 * cpc);
        // Clear CMF6A by writing 0; writing 1 does not set it.
        write16(&mut a, &mut io, 8000 * cpc + 1, 0xFFFF_F522, 0x000E);
        assert_eq!(r16(&io, 0xFFFF_F522), 0);
        write16(&mut a, &mut io, 8000 * cpc + 2, 0xFFFF_F522, 0x000F);
        assert_eq!(r16(&io, 0xFFFF_F522), 0, "software cannot set a flag");
    }

    #[test]
    fn the_boot_self_test_finds_its_flags() {
        // The boot loader writes CYLR = 1 and TCNT = 1, starts the timer and waits for CMF:
        // already on the compare value, it matches on the first count.
        let (mut a, mut io) = io();
        write16(&mut a, &mut io, 0, 0xFFFF_F50A, 0x0001); // CYLR6B
        write16(&mut a, &mut io, 0, 0xFFFF_F502, 0x0001); // TCNT6B
        write8(&mut a, &mut io, 0, TSTR2, 0x02);
        let cpc = a.cnt[5].cpc;
        assert_eq!(cpc, 4, "PSCR2 = 0, CKSEL = 0: one count per Pphi");
        assert_eq!(a.next_event(), cpc);
        a.advance(cpc, &mut io);
        assert_eq!(r16(&io, 0xFFFF_F522) & 2, 2);
    }

    #[test]
    fn channel_3_output_compare_with_clearing_and_overflow() {
        let (mut a, mut io) = io();
        write8(&mut a, &mut io, 0, 0xFFFF_F4AC, 0x01); // TCR3: φ'/2 (PSCR1 = 0: Pφ/1)
        write16(&mut a, &mut io, 0, 0xFFFF_F4A8, 100); // GR3D
        write8(&mut a, &mut io, 0, 0xFFFF_F4AA, 0x90); // TIOR3B: GR3D compare (001), CCI
        write16(&mut a, &mut io, 0, TIER3, 0x0008); // IME3D
        write8(&mut a, &mut io, 0, TSTR1, 0x10); // STR3
        let cpc = 4 * 2;
        assert_eq!(a.next_event(), 101 * cpc, "leaves 100 on the 101st count");
        a.advance(101 * cpc, &mut io);
        assert_eq!(r16(&io, TSR3) & 0x0008, 0x0008, "IMF3D");
        assert!(
            requests(&io, 0).iter().any(|r| r.0 == 123 && r.3),
            "IMI3D requested"
        );
        assert_eq!(a.cnt[1].value, 0, "cleared on compare match");
        assert_eq!(a.next_event(), 202 * cpc, "period GR + 1");
        assert_eq!(r16(&io, TSR3) & 0x0010, 0, "clearing is not an overflow");
        // Without CCI it runs on to overflow.
        write8(&mut a, &mut io, 101 * cpc, 0xFFFF_F4AA, 0x10);
        a.advance(101 * cpc + 0x1_0000 * cpc, &mut io);
        assert_eq!(r16(&io, TSR3) & 0x0010, 0x0010, "OVF3");
    }

    #[test]
    fn capture_registers_do_not_compare() {
        let (mut a, mut io) = io();
        write16(&mut a, &mut io, 0, 0xFFFF_F4A2, 100); // GR3A, which would match first
        write8(&mut a, &mut io, 0, 0xFFFF_F4AB, 0x77); // TIOR3A: A and B capture
        write8(&mut a, &mut io, 0, TSTR1, 0x10);
        // Only the overflow is an event.
        assert_eq!(a.next_event(), 0x1_0000 * 4);
    }

    #[test]
    fn an_external_clock_does_not_count_without_its_pin() {
        let (mut a, mut io) = io();
        write8(&mut a, &mut io, 0, 0xFFFF_F4AC, 0x09); // TCR3: external clock
        write8(&mut a, &mut io, 0, TSTR1, 0x10);
        assert_eq!(a.cnt[1].cpc, 0);
        assert_eq!(a.next_event(), u64::MAX);
    }

    #[test]
    fn interrupt_sources_and_their_priority_fields() {
        let (_, io) = io();
        let r = requests(&io, 0);
        let find = |v: u32| r.iter().find(|x| x.0 == v).map(|x| (x.1, x.2)).unwrap();
        // Table 7.3: CMI2E-H in IPRE 7-4, IMI3 in IPRF 15-12, IMI4 in IPRF 7-4,
        // IMI5 in IPRG 15-12, CMI6 in IPRG 7-4, CMI7 in IPRG 3-0.
        assert_eq!(find(108), (4, 8));
        assert_eq!(find(114), (4, 4));
        assert_eq!(find(116), (4, 0));
        assert_eq!(find(123), (5, 12));
        assert_eq!(find(124), (5, 8));
        assert_eq!(find(130), (5, 4));
        assert_eq!(find(137), (6, 12));
        assert_eq!(find(145), (6, 4));
        assert_eq!(find(151), (6, 0));
    }

    #[test]
    fn every_set_and_enabled_flag_is_offered_in_vector_order() {
        let (_, mut io) = io();
        for (s, e, bits) in [
            (TSR2B, TIER2B, 0x0101),
            (TSR3, TIER3, 0x0010),
            (0xFFFF_F522, 0xFFFF_F524, 0x0008),
            (0xFFFF_F5A2, 0xFFFF_F5A4, 0x0001),
        ] {
            w16(&mut io, s, bits);
            w16(&mut io, e, bits);
        }
        let mut got = Vec::new();
        for_each_request(&io, 0, |v, _, _| got.push(v));
        assert_eq!(got, [108, 116, 124, 147, 148]);
        w16(&mut io, TSR0, 0x0011);
        w16(&mut io, TIER0, 0x0011);
        got.clear();
        for_each_request(&io, 0, |v, _, _| got.push(v));
        assert_eq!(&got[..2], [84, 92], "ICI0A and OVI0 first");
        w16(&mut io, TIER3, 0);
        got.clear();
        for_each_request(&io, 0, |v, _, _| got.push(v));
        assert_eq!(
            got,
            [84, 92, 108, 116, 147, 148],
            "a set flag without its enable is not offered"
        );
    }

    #[test]
    fn tcnt0_counts_on_pscr1_and_overflows() {
        let (mut a, mut io) = io();
        write8(&mut a, &mut io, 0, 0xFFFF_F404, 4); // PSCR1: φ' = Pφ / 5
        write8(&mut a, &mut io, 0, TSTR1, 0x01); // STR0
        assert_eq!(a.tcnt0.cpc, 20, "4 MHz at φ = 80 MHz: 4000 counts per ms");
        a.store(80_000, &mut io);
        assert_eq!(r32(&io, TCNT0), 4000);
        // Written by software, it goes on from there.
        a.resync(80_000);
        a.store(80_000, &mut io);
        let o = (TCNT0 - IO_BASE) as usize;
        let before = io[o..o + 4].to_vec();
        io[o..o + 4].copy_from_slice(&0xFFFF_FFF0u32.to_be_bytes());
        a.load(80_000, TCNT0, &before, &mut io);
        assert_eq!(
            a.next_event(),
            80_000 + 16 * 20,
            "leaves H'FFFFFFFF after 16 counts"
        );
        w16(&mut io, TIER0, 0x0010);
        a.advance(80_000 + 16 * 20, &mut io);
        assert_eq!(r16(&io, TSR0), 0x0010, "OVF0");
        assert!(requests(&io, 0).iter().any(|r| r.0 == 92 && r.3), "OVI0");
        a.store(80_000 + 18 * 20, &mut io);
        assert_eq!(r32(&io, TCNT0), 2);
        // Stopping freezes it.
        write8(&mut a, &mut io, 80_000 + 18 * 20, TSTR1, 0x00);
        a.store(1_000_000, &mut io);
        assert_eq!(r32(&io, TCNT0), 2);
    }

    #[test]
    fn channel_8_one_shot_pulses() {
        let (mut a, mut io) = io();
        write8(&mut a, &mut io, 0, 0xFFFF_F404, 4); // PSCR1: 20 cycles a count
        write8(&mut a, &mut io, 0, TCR8, 0x10); // A-H on phi', I-P on phi'/2
        write16(&mut a, &mut io, 0, DCNT8, 5); // DCNT8A
        write16(&mut a, &mut io, 0, DCNT8 + 2 * 9, 3); // DCNT8J
        write16(&mut a, &mut io, 100, DSTR, 0x0201);
        assert_eq!(
            a.next_event(),
            100 + 5 * 20,
            "8A ends first: 5 counts of 20"
        );
        a.store(140, &mut io);
        assert_eq!(r16(&io, DCNT8 + 2 * 9), 2, "counting down");
        a.advance(100 + 5 * 20, &mut io);
        assert_eq!(r16(&io, TSR8), 0x0001, "OSF8A");
        assert_eq!(a.next_event(), 100 + 3 * 40, "then 8J: 3 counts of 40");
        a.advance(100 + 3 * 40, &mut io);
        assert_eq!(r16(&io, TSR8), 0x0201, "OSF8A and OSF8J");
        assert_eq!(r16(&io, DSTR), 0, "DSTR clears itself");
        assert_eq!(r16(&io, DCNT8), 0);
        assert_eq!(a.pulse_count, 2);
        assert!(a.pulses.contains(&(0, 100, 200)));
        assert!(a.pulses.contains(&(9, 100, 220)));
        w16(&mut io, TIER8, 0x0200);
        assert!(
            requests(&io, 0)
                .iter()
                .any(|r| r.0 == 161 && r.1 == 7 && r.2 == 4 && r.3),
            "OSI8J"
        );
        // A count stopped by software stays where it was and sets no flag.
        write16(&mut a, &mut io, 300, TSR8, 0);
        write16(&mut a, &mut io, 300, DCNT8, 10);
        write16(&mut a, &mut io, 300, DSTR, 0x0001);
        write16(&mut a, &mut io, 300 + 4 * 20, DSTR, 0x0000);
        assert_eq!(r16(&io, DCNT8), 6);
        a.advance(10_000, &mut io);
        assert_eq!(r16(&io, TSR8), 0);
        assert_eq!(a.pulse_count, 2);
    }

    #[test]
    fn ti10_rising_edges_when_selected() {
        let (mut a, mut io) = io();
        write8(&mut a, &mut io, 0, 0xFFFF_F6E4, 0x01); // TCR10: rising edges
        a.ti10_edge(&mut io, 10, false);
        assert_eq!(a.ti10_edges, 0);
        a.ti10_edge(&mut io, 20, true);
        assert_eq!(a.ti10_edges, 1);
    }

    #[test]
    fn channel_0_captures_tcnt0_at_the_edge() {
        let (mut a, mut io) = io();
        write8(&mut a, &mut io, 0, 0xFFFF_F404, 4); // PSCR1: 20 cycles a count
        write8(&mut a, &mut io, 0, TSTR1, 0x01);
        write8(&mut a, &mut io, 0, TIOR0, 0x16); // A falling, B rising, C rising
        // The counter is brought up to date at 10 000; an edge from before that still
        // captures the count it had then.
        a.resync(10_000);
        assert!(!a.capture(&mut io, 2_000, 0, true), "A wants falling edges");
        assert!(a.capture(&mut io, 2_000, 0, false));
        assert_eq!(r32(&io, ICR0[0]), 100);
        assert!(a.capture(&mut io, 12_000, 1, true));
        assert_eq!(r32(&io, ICR0[1]), 600);
        assert!(!a.capture(&mut io, 12_000, 3, true), "D disabled");
        assert_eq!(r16(&io, TSR0), 0x0003, "ICF0A, ICF0B");
        w16(&mut io, TIER0, 0x0001);
        assert!(requests(&io, 0).iter().any(|r| r.0 == 84 && r.3), "ICI0A");
    }

    #[test]
    fn channel_10_measures_tooth_periods() {
        let (mut a, mut io) = io();
        write8(&mut a, &mut io, 0, 0xFFFF_F6E4, 0x02); // TCR10: falling edges
        write8(&mut a, &mut io, 0, 0xFFFF_F6E2, 0x10); // TIOR10: x64
        write8(&mut a, &mut io, 0, TSTR1, 0x80); // STR10; PSCR4 = 0: one count per Pphi
        assert_eq!(a.tcnt10a.cpc, 4);
        a.ti10_edge(&mut io, 400, true);
        assert_eq!(a.ti10_edges, 0, "rising edge not selected");
        a.ti10_edge(&mut io, 400, false);
        assert_eq!(r32(&io, ICR10A), 101, "from 1, 100 counts");
        assert_eq!(io[(TCNT10B - IO_BASE) as usize], 1);
        assert_eq!(r16(&io, TSR10), 0x0002, "ICF10A");
        a.resync(20_000);
        a.ti10_edge(&mut io, 400 + 4 * 6400, false);
        assert_eq!(
            r32(&io, ICR10A),
            6401,
            "counted from 1 again at the last edge"
        );
        assert_eq!(r16(&io, RLD10C), 6401 >> 6, "reload: period over 64");
        // OCR10B = 1: counting on from 1 sets CMF10B.
        io[(OCR10B - IO_BASE) as usize] = 2;
        a.ti10_edge(&mut io, 30_000, false);
        assert_eq!(r16(&io, TSR10) & 4, 4, "CMF10B");
        // OCR10A as a timeout on the next tooth.
        w16(&mut io, OCR10A, 0);
        w16(&mut io, OCR10A + 2, 1000);
        a.cmp10a_done = false;
        a.schedule(&io);
        assert_eq!(a.next_event(), 30_000 + 4 * 999);
        a.advance(30_000 + 4 * 999, &mut io);
        assert_eq!(r16(&io, TSR10) & 1, 1, "CMF10A");
        w16(&mut io, TIER10, 0x0003);
        assert!(requests(&io, 0).iter().any(|r| r.0 == 176 && r.3));
        assert!(requests(&io, 0).iter().any(|r| r.0 == 180 && r.3));
    }

    #[test]
    fn tcnt2b_compares_with_every_ocr() {
        let (mut a, mut io) = io();
        write8(&mut a, &mut io, 0, 0xFFFF_F62A, 0x01); // TCR2B: φ'/2
        write16(&mut a, &mut io, 0, 0xFFFF_F620, 0x0010); // OCR2G
        write16(&mut a, &mut io, 0, TIER2B, 0x0040); // CME2G
        write8(&mut a, &mut io, 0, TSTR1, 0x08); // STR2B
        a.advance(0x11 * 8, &mut io);
        assert_eq!(r16(&io, TSR2B), 0x0040);
        assert!(
            requests(&io, 0).iter().any(|r| r.0 == 114 && r.3),
            "CMI2G requested"
        );
        // A read sees the count.
        a.store(0x11 * 8 + 5 * 8, &mut io);
        assert_eq!(r16(&io, 0xFFFF_F602), 0x16);
    }

    #[test]
    fn a_channel_2_compare_starts_a_connected_one_shot() {
        let run = |tcnr: u16, dcnt: u16, reload: Option<u16>| {
            let (mut a, mut io) = io();
            write16(&mut a, &mut io, 0, 0xFFFF_F614, 10); // OCR2A
            write16(&mut a, &mut io, 0, 0xFFFF_F650, dcnt); // DCNT8I
            write16(&mut a, &mut io, 0, TCNR, tcnr);
            if let Some(r) = reload {
                write16(&mut a, &mut io, 0, RLDR8, r);
                write8(&mut a, &mut io, 0, RLDENR, 0x80);
            }
            write8(&mut a, &mut io, 0, TSTR1, 0x08); // TCNT2B on Pphi: 4 cycles a count
            // TCNT2B leaves 10 after 11 counts.
            a.advance(43, &mut io);
            assert_eq!(r16(&io, DSTR), 0, "not yet");
            a.advance(44, &mut io);
            (a, io)
        };
        let (mut a, mut io) = run(0x0100, 5, None);
        assert_eq!(r16(&io, DSTR), 0x0100, "DST8I set by the compare match");
        a.advance(44 + 5 * 4, &mut io);
        assert_eq!(
            a.pulses.back(),
            Some(&(8, 44, 64)),
            "TO8I high for five counts"
        );
        assert_eq!(r16(&io, TSR8), 0x0100, "OSF8I");
        let (_, io) = run(0x0200, 5, None);
        assert_eq!(r16(&io, DSTR), 0, "DCNT8I not connected");
        let (_, io) = run(0x0100, 0, None);
        assert_eq!(r16(&io, DSTR), 0, "a count of 0 does not start");
        let (mut a, mut io) = run(0x0100, 5, Some(7));
        a.advance(44 + 7 * 4, &mut io);
        assert_eq!(a.pulses.back(), Some(&(8, 44, 72)), "RLDR8 reloaded first");
    }

    #[test]
    fn tcnt10g_finds_the_gap_and_ireg_holds_the_interrupt() {
        let gap = |ireg: bool| {
            let (mut a, mut io) = io();
            write8(&mut a, &mut io, 0, TCR10, 0x01); // rising edges
            write8(&mut a, &mut io, 0, TIOR10, 0x11); // x64, GR10G compare
            write16(&mut a, &mut io, 0, GR10G, 160);
            write16(&mut a, &mut io, 0, TIER10, if ireg { 0x18 } else { 0x08 });
            write8(&mut a, &mut io, 0, TSTR1, 0x80); // TCNT10A on Pphi
            // Teeth every 25 600 cycles (6400 counts): AGCK1 every 6400 / 64 = 100
            // counts, 400 cycles, 64 to a tooth, so TCNT10G never reaches 160...
            for t in [0, 25_600, 51_200] {
                a.ti10_edge(&mut io, t, true);
                a.advance(t, &mut io);
            }
            assert_eq!(r16(&io, TSR10) & 8, 0);
            // ... until a gap of three pitches: 160 pulses after the last tooth.
            a.advance(115_199, &mut io);
            assert_eq!(r16(&io, TSR10) & 8, 0, "159 pulses");
            a.advance(115_200, &mut io);
            assert_eq!(r16(&io, TSR10) & 8, 8, "CMF10G at the match");
            (a, io)
        };
        let cmi10g =
            |a: &Atu, io: &[u8]| requests(io, a.held10(io)).iter().any(|r| r.0 == 180 && r.3);
        let (a, io) = gap(false);
        assert!(cmi10g(&a, &io), "without IREG, at once");
        let (mut a, mut io) = gap(true);
        assert!(!cmi10g(&a, &io), "with IREG, held");
        a.ti10_edge(&mut io, 128_000, true);
        assert!(cmi10g(&a, &io), "until the tooth after the gap");
        // The gap's interval (three pitches) is the new AGCK1 period: 1200 cycles.
        a.store(128_000 + 4000, &mut io);
        assert_eq!(r16(&io, TCNT10G), 3, "restarted at that tooth");
        // Written by software, it counts on from there until the next tooth.
        write16(&mut a, &mut io, 133_000, TCNT10G, 50);
        a.store(133_000 + 1200, &mut io);
        assert_eq!(r16(&io, TCNT10G), 51);
        a.ti10_edge(&mut io, 135_000, true);
        a.store(135_000, &mut io);
        assert_eq!(r16(&io, TCNT10G), 0);
    }
}
