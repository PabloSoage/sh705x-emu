//! A/D converter: three modules, 10-bit, 32 channels (manual section 18).
//!
//! | Module | Channels | ADDR | ADCSR | ADCR |
//! |---|---|---|---|---|
//! | A/D0 | AN0–AN11 | `FFFFF800–F817` | `FFFFF818` | `FFFFF819` |
//! | A/D1 | AN12–AN23 | `FFFFF820–F837` | `FFFFF838` | `FFFFF839` |
//! | A/D2 | AN24–AN31 | `FFFFF840–F84F` | `FFFFF858` | `FFFFF859` |
//!
//! Setting ADCR.ADST converts the channels that ADCSR's mode and CH bits select
//! (single, or 4-, 8- or 12-channel scan). Each conversion takes 266 states with
//! CKS = 0 or 134 with CKS = 1 for the first channel, and 256 or 128 for each one after it
//! (manual 18.4.3), states being Pφ cycles. At the end every result is in its ADDR
//! (10 bits, left-aligned), ADF is set (an interrupt if ADIE), and ADST is cleared, or
//! the scan starts again in continuous mode.
//!
//! Simplifications: results are written together at the end of the round, not one by
//! one; external and ATU-II triggers (TRGE) are not modelled.

/// First register address.
pub const BASE: u32 = 0xFFFF_F800;
/// One past ADCR2.
pub const END: u32 = 0xFFFF_F85A;

const ADF: u8 = 0x80;
const ADIE: u8 = 0x40;
const ADST: u8 = 0x20;
const ADCS: u8 = 0x10;
const CKS: u8 = 0x40;

/// One A/D module.
#[derive(Clone, Debug)]
pub struct Module {
    /// ADCSR.
    pub csr: u8,
    /// ADCR.
    pub cr: u8,
    /// The cycle at which the running round ends.
    pub busy_until: Option<u64>,
}

/// The three modules and the analog inputs.
#[derive(Clone, Debug)]
pub struct Adc {
    /// A/D0, A/D1, A/D2.
    pub m: [Module; 3],
    /// ADDR0..ADDR31 as read (10-bit result in bits 15..6).
    pub addr: [u16; 32],
    /// The input on each channel, as a 10-bit code (0 = 0 V, 1023 = AVref).
    pub input: [u16; 32],
    /// Rounds completed, per module.
    pub rounds: [u64; 3],
}

impl Default for Adc {
    fn default() -> Self {
        Self {
            m: [
                Module {
                    csr: 0x00,
                    cr: 0x0F,
                    busy_until: None,
                },
                Module {
                    csr: 0x00,
                    cr: 0x0F,
                    busy_until: None,
                },
                Module {
                    csr: 0x08,
                    cr: 0x0F,
                    busy_until: None,
                },
            ],
            addr: [0; 32],
            input: [0; 32],
            rounds: [0; 3],
        }
    }
}

impl Adc {
    /// The channels module `k` converts with its current settings (manual 18.2.2, 18.2.4).
    pub fn channels(&self, k: usize) -> Vec<usize> {
        let csr = self.m[k].csr;
        let mode = (csr >> 4) & 3;
        if k == 2 {
            let ch = (csr & 7) as usize;
            let (g, i) = (ch / 4, ch % 4);
            let mut out: Vec<usize> = match mode {
                0 => vec![24 + ch],
                1 => (0..=i).map(|j| 24 + 4 * g + j).collect(),
                2 => (0..=i).flat_map(|j| [24 + j, 28 + j]).collect(),
                _ => Vec::new(),
            };
            out.sort_unstable();
            return out;
        }
        let first = 12 * k;
        let ch = (csr & 0xF) as usize;
        let (g, i) = (ch / 4, ch % 4);
        let mut out: Vec<usize> = match mode {
            0 => vec![first + ch],
            1 => (0..=i).map(|j| first + 4 * g + j).collect(),
            2 => (0..=i).flat_map(|j| [first + j, first + 4 + j]).collect(),
            _ => (0..=i)
                .flat_map(|j| [first + j, first + 4 + j, first + 8 + j])
                .collect(),
        };
        out.retain(|&c| c < first + 12);
        out.sort_unstable();
        out
    }

    /// CPU cycles for one round of module `k`: Pφ is φ/4.
    fn round_cycles(&self, k: usize) -> u64 {
        let n = self.channels(k).len().max(1) as u64;
        let (first, rest) = if self.m[k].cr & CKS != 0 {
            (134, 128)
        } else {
            (266, 256)
        };
        4 * (first + rest * (n - 1))
    }

    /// Ends every round due by `now`.
    pub fn advance(&mut self, now: u64) {
        for k in 0..3 {
            while let Some(end) = self.m[k].busy_until {
                if end > now {
                    break;
                }
                for c in self.channels(k) {
                    self.addr[c] = (self.input[c] & 0x3FF) << 6;
                }
                self.rounds[k] += 1;
                self.m[k].csr |= ADF;
                let continuous = self.m[k].csr & 0x30 != 0 && self.m[k].cr & ADCS != 0;
                if continuous {
                    self.m[k].busy_until = Some(end + self.round_cycles(k));
                } else {
                    self.m[k].cr &= !ADST;
                    self.m[k].busy_until = None;
                }
            }
        }
    }

    /// The earliest cycle at which a round ends.
    pub fn next_event(&self) -> u64 {
        self.m
            .iter()
            .filter_map(|m| m.busy_until)
            .min()
            .unwrap_or(u64::MAX)
    }

    /// Whether module `k` requests its interrupt (ADF and ADIE).
    pub fn requesting(&self, k: usize) -> bool {
        self.m[k].csr & (ADF | ADIE) == ADF | ADIE
    }

    /// Writes the register values into `regs` (the bytes from [`BASE`] to [`END`]).
    pub fn store(&self, regs: &mut [u8]) {
        for (c, v) in self.addr.iter().enumerate() {
            let o = match c {
                0..=11 => 2 * c,
                12..=23 => 0x20 + 2 * (c - 12),
                _ => 0x40 + 2 * (c - 24),
            };
            regs[o..o + 2].copy_from_slice(&v.to_be_bytes());
        }
        for (k, o) in [0x18usize, 0x38, 0x58].into_iter().enumerate() {
            regs[o] = self.m[k].csr;
            regs[o + 1] = self.m[k].cr;
        }
    }

    /// Takes a write: `before` and `after` are the register bytes around it. ADDR is
    /// read-only; ADF only goes from 1 to 0; setting ADST starts a round.
    pub fn load(&mut self, now: u64, before: &[u8], after: &[u8]) {
        for (k, o) in [0x18usize, 0x38, 0x58].into_iter().enumerate() {
            let m = &mut self.m[k];
            let (csr_b, csr_a) = (before[o], after[o]);
            let fixed = if k == 2 { 0x08 } else { 0x00 };
            m.csr = (csr_a & !ADF & !fixed) | (csr_b & csr_a & ADF) | fixed;
            let (cr_b, cr_a) = (before[o + 1], after[o + 1]);
            m.cr = (cr_a & 0xF0) | 0x0F;
            if cr_a & ADST != 0 && cr_b & ADST == 0 {
                let len = self.round_cycles(k);
                self.m[k].busy_until = Some(now + len);
            } else if cr_a & ADST == 0 {
                self.m[k].busy_until = None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_selection_as_the_firmware_uses_it() {
        let mut a = Adc::default();
        a.m[0].csr = 0x13; // 4-channel scan, group 0, CH = 3
        assert_eq!(a.channels(0), [0, 1, 2, 3]);
        a.m[0].csr = 0x33; // 12-channel scan, CH = 3
        assert_eq!(a.channels(0), (0..12).collect::<Vec<_>>());
        a.m[1].csr = 0x13;
        assert_eq!(a.channels(1), [12, 13, 14, 15]);
        a.m[2].csr = 0x5B; // ADIE, 4-channel scan, CH = 3 (bit 3 reads 1)
        assert_eq!(a.channels(2), [24, 25, 26, 27]);
        a.m[2].csr = 0x6B; // 8-channel scan
        assert_eq!(a.channels(2), (24..32).collect::<Vec<_>>());
        a.m[0].csr = 0x05; // single, AN5
        assert_eq!(a.channels(0), [5]);
    }

    fn regs(a: &Adc) -> Vec<u8> {
        let mut r = vec![0; (END - BASE) as usize];
        a.store(&mut r);
        r
    }

    fn write(a: &mut Adc, now: u64, off: usize, v: u8) {
        let before = regs(a);
        let mut after = before.clone();
        after[off] = v;
        a.load(now, &before, &after);
    }

    #[test]
    fn a_single_cycle_scan_converts_and_flags() {
        let mut a = Adc::default();
        a.input[2] = 0x3FF;
        a.input[3] = 512;
        write(&mut a, 0, 0x18, 0x13 | ADF);
        assert_eq!(a.m[0].csr & ADF, 0, "software cannot set ADF");
        write(&mut a, 0, 0x18, 0x13);
        write(&mut a, 100, 0x19, 0x20); // ADST
        let len = 4 * (266 + 3 * 256);
        assert_eq!(a.next_event(), 100 + len);
        a.advance(100 + len - 1);
        assert_eq!(a.m[0].csr & ADF, 0);
        a.advance(100 + len);
        assert_eq!(a.m[0].csr & ADF, ADF);
        assert_eq!(a.m[0].cr & ADST, 0, "single-cycle scan clears ADST");
        assert_eq!(a.addr[2], 0xFFC0);
        assert_eq!(a.addr[3], 512 << 6);
        assert!(!a.requesting(0), "ADIE off");
        write(&mut a, len + 200, 0x18, 0x13 | ADF); // writing 1 keeps ADF
        assert_eq!(a.m[0].csr & ADF, ADF);
        write(&mut a, len + 200, 0x18, 0x13); // writing 0 clears it
        assert_eq!(a.m[0].csr & ADF, 0);
        write(&mut a, len + 200, 0x00, 0x12); // ADDR is read-only
        assert_eq!(a.addr[0], 0);
    }

    #[test]
    fn adi2_is_requested_when_enabled() {
        let mut a = Adc::default();
        write(&mut a, 0, 0x58, 0x5B);
        write(&mut a, 0, 0x59, 0x60); // CKS = 1, ADST
        a.advance(4 * (134 + 3 * 128));
        assert!(a.requesting(2));
        assert_eq!(regs(&a)[0x58], 0xDB, "bit 3 reads 1");
    }
}
