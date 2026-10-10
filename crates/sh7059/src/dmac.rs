//! Direct memory access controller, four channels (manual section 10).
//!
//! Registers, all in the chip's register storage:
//!
//! | Address | Register | Notes |
//! |---|---|---|
//! | `FFFFECB0` | DMAOR | AE (2) and NMIF (1) cleared only by writing 0; DME (0) |
//! | `FFFFECC0 + 0x10 k` | SARk | source address |
//! | `FFFFECC4 + 0x10 k` | DARk | destination address |
//! | `FFFFECC8 + 0x10 k` | DMATCRk | 24-bit count; 0 means 2^24 |
//! | `FFFFECCC + 0x10 k` | CHCRk | RS4–0, SM, DM, TS, TM, IE, TE (cleared only by writing 0), DE |
//!
//! Modelled: auto-request transfers (RS = 11111) of bytes, words or longwords with fixed,
//! incrementing or decrementing addresses; TE and the DEI interrupts at the end; abort
//! when DE or DME is cleared; AE on a misaligned address. A channel transfers one unit
//! every [`CYCLES_PER_UNIT`] CPU cycles from the write that enabled it, and the units due
//! are carried out whenever the DMAC registers are accessed and when the last one is due.
//! The CPU is not stalled while the DMAC holds the bus.
//!
//! Not modelled: on-chip peripheral requests (ATU-II, SCI, A/D, HCAN; channels enabled
//! with one are counted in [`Dmac::peripheral_requests`] and do nothing), indirect address
//! mode of channel 3 and the source reload of channel 2, and channel priorities (the
//! channels run side by side).

use crate::IO_BASE;

/// First DMAC register (DMAOR).
pub const BASE: u32 = 0xFFFF_ECB0;
/// One past the last DMAC register (CHCR3).
pub const END: u32 = 0xFFFF_ED00;
/// DMA operation register.
pub const DMAOR: u32 = 0xFFFF_ECB0;

/// CPU cycles per transfer unit: a read and a write of on-chip memory, one state each.
/// The manual gives no figure for on-chip memory; this is an estimate [EST].
pub const CYCLES_PER_UNIT: u64 = 2;

const DME: u16 = 0x0001;
const NMIF: u16 = 0x0002;
const AE: u16 = 0x0004;
const DE: u32 = 0x0001;
const TE: u32 = 0x0002;
const IE: u32 = 0x0004;
const AUTO_REQUEST: u32 = 0x1F;

/// Address of channel `k`'s SAR; DAR, DMATCR and CHCR follow at +4, +8 and +0xC.
pub const fn sar(k: usize) -> u32 {
    0xFFFF_ECC0 + 0x10 * k as u32
}

fn r16(io: &[u8], a: u32) -> u16 {
    let o = (a - IO_BASE) as usize;
    u16::from_be_bytes([io[o], io[o + 1]])
}

fn r32(io: &[u8], a: u32) -> u32 {
    let o = (a - IO_BASE) as usize;
    u32::from_be_bytes([io[o], io[o + 1], io[o + 2], io[o + 3]])
}

fn w16(io: &mut [u8], a: u32, v: u16) {
    let o = (a - IO_BASE) as usize;
    io[o..o + 2].copy_from_slice(&v.to_be_bytes());
}

fn w32(io: &mut [u8], a: u32, v: u32) {
    let o = (a - IO_BASE) as usize;
    io[o..o + 4].copy_from_slice(&v.to_be_bytes());
}

/// Bits of CHCRk that exist: DI only on channel 3, RO only on channel 2.
fn chcr_mask(k: usize) -> u32 {
    0x001F_333F
        | match k {
            2 => 0x0100_0000,
            3 => 0x1000_0000,
            _ => 0,
        }
}

/// One transfer unit, as the chip has to carry it out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Unit {
    /// Source address.
    pub src: u32,
    /// Destination address.
    pub dst: u32,
    /// Bytes: 1, 2 or 4.
    pub size: u32,
}

/// A channel transferring: the cycle it started and the units done since.
#[derive(Clone, Copy, Debug)]
struct Run {
    start: u64,
    done: u64,
}

/// The DMA controller.
#[derive(Clone, Debug, Default)]
pub struct Dmac {
    run: [Option<Run>; 4],
    /// Channels enabled with an on-chip peripheral as the request source, which is not
    /// modelled: they never transfer.
    pub peripheral_requests: u64,
    /// Units transferred since power-on.
    pub transfers: u64,
}

impl Dmac {
    /// Takes a write to the DMAC registers. `before` holds the bytes from `addr` before
    /// it; the storage already holds the written ones.
    pub fn load(&mut self, now: u64, addr: u32, before: &[u8], io: &mut [u8]) {
        let n = before.len() as u32;
        let old = |a: u32, io: &[u8]| {
            let mut b = [0u8; 4];
            for (i, x) in b.iter_mut().enumerate() {
                let ai = a + i as u32;
                *x = if (addr..addr + n).contains(&ai) {
                    before[(ai - addr) as usize]
                } else {
                    io[(ai - IO_BASE) as usize]
                };
            }
            b
        };
        if (addr..addr + n).any(|a| (DMAOR..DMAOR + 2).contains(&a)) {
            let was = u16::from_be_bytes([old(DMAOR, io)[0], old(DMAOR, io)[1]]);
            let v = r16(io, DMAOR) & 7;
            // AE and NMIF: writing 1 keeps them as they were, writing 0 clears them.
            w16(io, DMAOR, (v & DME) | (v & was & (AE | NMIF)));
        }
        for k in 0..4 {
            let tcr = sar(k) + 8;
            if (addr..addr + n).any(|a| (tcr..tcr + 4).contains(&a)) {
                let v = r32(io, tcr) & 0x00FF_FFFF;
                w32(io, tcr, v);
            }
            let chcr = sar(k) + 0xC;
            if (addr..addr + n).any(|a| (chcr..chcr + 4).contains(&a)) {
                let was = u32::from_be_bytes(old(chcr, io));
                let v = r32(io, chcr) & chcr_mask(k);
                w32(io, chcr, (v & !TE) | (v & was & TE));
            }
        }
        self.check(now, io);
    }

    /// Starts channels that have become enabled and stops those that no longer are.
    pub fn check(&mut self, now: u64, io: &[u8]) {
        let dmaor = r16(io, DMAOR);
        let master = dmaor & DME != 0 && dmaor & (AE | NMIF) == 0;
        for k in 0..4 {
            let chcr = r32(io, sar(k) + 0xC);
            let on = master && chcr & DE != 0 && chcr & TE == 0;
            let auto = (chcr >> 16) & 0x1F == AUTO_REQUEST;
            match (on, self.run[k].is_some()) {
                (true, false) if auto => {
                    self.run[k] = Some(Run {
                        start: now,
                        done: 0,
                    })
                }
                (true, false) => {
                    if (chcr >> 16) & 0x1F != 0 {
                        self.peripheral_requests += 1;
                    }
                    // Mark it as seen, so the count is once per enable, with nothing due.
                    self.run[k] = Some(Run {
                        start: u64::MAX,
                        done: 0,
                    });
                }
                (false, true) => self.run[k] = None,
                _ => {}
            }
        }
    }

    /// Transfers left on channel `k`: DMATCR, where 0 means 2^24.
    fn remaining(io: &[u8], k: usize) -> u64 {
        match r32(io, sar(k) + 8) {
            0 => 1 << 24,
            n => n as u64,
        }
    }

    /// Whether no channel is transferring.
    pub fn idle(&self) -> bool {
        self.run
            .iter()
            .all(|r| r.is_none_or(|r| r.start == u64::MAX))
    }

    /// Units channel `k` should have transferred by `now` and has not.
    pub fn due(&self, k: usize, now: u64, io: &[u8]) -> u64 {
        match self.run[k] {
            Some(r) if r.start <= now => {
                ((now - r.start) / CYCLES_PER_UNIT - r.done).min(Self::remaining(io, k))
            }
            _ => 0,
        }
    }

    /// The next unit of channel `k`: addresses and size from its registers, or None with
    /// AE set if an address is misaligned or the size is the prohibited one.
    pub fn unit(&self, k: usize, io: &mut [u8]) -> Option<Unit> {
        let chcr = r32(io, sar(k) + 0xC);
        let ts = (chcr >> 4) & 3;
        let (src, dst) = (r32(io, sar(k)), r32(io, sar(k) + 4));
        let size = 1 << ts;
        if ts == 3 || src % size != 0 || dst % size != 0 {
            let v = r16(io, DMAOR) | AE;
            w16(io, DMAOR, v);
            return None;
        }
        Some(Unit { src, dst, size })
    }

    /// After channel `k` has carried out one unit: addresses and count updated, TE at the
    /// end.
    pub fn unit_done(&mut self, k: usize, io: &mut [u8]) {
        let chcr = r32(io, sar(k) + 0xC);
        let size = 1u32 << ((chcr >> 4) & 3);
        let step = |mode: u32, a: u32| match mode {
            1 => a.wrapping_add(size),
            2 => a.wrapping_sub(size),
            _ => a,
        };
        let src = step((chcr >> 12) & 3, r32(io, sar(k)));
        let dst = step((chcr >> 8) & 3, r32(io, sar(k) + 4));
        w32(io, sar(k), src);
        w32(io, sar(k) + 4, dst);
        let tcr = r32(io, sar(k) + 8).wrapping_sub(1) & 0x00FF_FFFF;
        w32(io, sar(k) + 8, tcr);
        if tcr == 0 {
            w32(io, sar(k) + 0xC, chcr | TE);
        }
        if let Some(r) = self.run[k].as_mut() {
            r.done += 1;
        }
        self.transfers += 1;
    }

    /// The cycle the earliest running channel ends, so its TE is set on time.
    pub fn next_event(&self, io: &[u8]) -> u64 {
        (0..4)
            .filter_map(|k| {
                self.run[k]
                    .filter(|r| r.start != u64::MAX)
                    .map(|r| r.start + (r.done + Self::remaining(io, k)) * CYCLES_PER_UNIT)
            })
            .min()
            .unwrap_or(u64::MAX)
    }

    /// Whether channel `k` is requesting its DEI interrupt: TE and IE.
    pub fn requesting(io: &[u8], k: usize) -> bool {
        let chcr = r32(io, sar(k) + 0xC);
        chcr & (TE | IE) == TE | IE
    }
}

#[cfg(test)]
mod tests {
    use crate::{ROM_SIZE, Sh7059};
    use sh2e::Bus;

    fn chip() -> Sh7059 {
        Sh7059::new(&vec![0u8; ROM_SIZE]).unwrap()
    }

    #[test]
    fn the_boot_ram_fill() {
        // As the firmware does it: SAR = DAR, source fixed, destination incrementing,
        // longwords, burst, auto-request; then it polls CHCR0 for TE.
        let mut c = chip();
        c.write32(0xFFFE_A7F0, 0x1234_5678);
        c.write16(0xFFFF_ED04, 0xF000); // IPRC: DEI0 at level 15, so only IE holds it back
        c.write16(super::DMAOR, 0x0001);
        c.write32(0xFFFF_ECC0, 0xFFFE_A7F0);
        c.write32(0xFFFF_ECC4, 0xFFFE_A7F0);
        c.write32(0xFFFF_ECC8, 0xFF00_0064);
        assert_eq!(c.read32(0xFFFF_ECC8), 100, "DMATCR is 24 bits");
        c.write32(0xFFFF_ECCC, 0x001F_012A);
        assert_eq!(c.read32(0xFFFF_ECCC), 0x001F_0128, "software cannot set TE");
        c.write32(0xFFFF_ECCC, 0x001F_0129);
        assert_eq!(c.read32(0xFFFF_ECCC) & 2, 0, "not done at once");
        assert_eq!(c.next_event, 100 * super::CYCLES_PER_UNIT);
        c.now = 40 * super::CYCLES_PER_UNIT;
        assert_eq!(
            c.read32(0xFFFF_ECC8),
            60,
            "units carried out as time passes"
        );
        c.write16(super::DMAOR, 0x0001);
        assert_eq!(
            c.next_event,
            100 * super::CYCLES_PER_UNIT,
            "the end does not move"
        );
        c.now = 99 * super::CYCLES_PER_UNIT;
        assert_eq!(c.read32(0xFFFF_ECCC) & 2, 0, "no TE before the last unit");
        c.now = c.next_event;
        c.advance();
        assert_eq!(c.read32(0xFFFF_ECCC), 0x001F_012B, "TE");
        assert_eq!(c.read32(0xFFFF_ECC8), 0);
        assert_eq!(c.read32(0xFFFF_ECC4), 0xFFFE_A7F0 + 400);
        assert_eq!(c.read32(0xFFFF_ECC0), 0xFFFE_A7F0, "source fixed");
        for a in (0xFFFE_A7F0..0xFFFE_A7F0 + 400).step_by(4) {
            assert_eq!(c.read32(a), 0x1234_5678);
        }
        assert_eq!(c.read32(0xFFFE_A7F0 + 400), 0, "not one more");
        assert_eq!(c.dmac.transfers, 100);
        assert_eq!(c.pending, (0, 0), "IE clear: no DEI");
        // Writing 1 to TE keeps it; writing 0 clears it.
        c.write32(0xFFFF_ECCC, 0x001F_012B);
        assert_eq!(c.read32(0xFFFF_ECCC) & 2, 2);
        c.write32(0xFFFF_ECCC, 0x001F_0128);
        assert_eq!(c.read32(0xFFFF_ECCC), 0x001F_0128);
    }

    #[test]
    fn decrementing_bytes_with_an_interrupt() {
        let mut c = chip();
        for i in 0..4u32 {
            c.write8(0xFFFE_8000 + i, i as u8 + 1);
        }
        c.write16(0xFFFF_ED04, 0x0A00); // IPRC: DEI2/3 at level 10
        c.write16(super::DMAOR, 0x0001);
        c.write32(0xFFFF_ECE0, 0xFFFE_8003);
        c.write32(0xFFFF_ECE4, 0xFFFE_8103);
        c.write32(0xFFFF_ECE8, 4);
        c.write32(0xFFFF_ECEC, 0x001F_2205); // SM, DM decrement; bytes; IE; DE
        c.now = c.next_event;
        c.advance();
        let got: Vec<u8> = (0..4).map(|i| c.read8(0xFFFE_8100 + i)).collect();
        assert_eq!(got, [1, 2, 3, 4]);
        assert_eq!(c.read32(0xFFFF_ECE0), 0xFFFE_7FFF);
        assert_eq!(c.pending, (10, 76), "DEI2");
    }

    #[test]
    fn clearing_de_aborts_and_dme_gates_everything() {
        let mut c = chip();
        c.write32(0xFFFF_ECC0, 0xFFFE_8000);
        c.write32(0xFFFF_ECC4, 0xFFFE_9000);
        c.write32(0xFFFF_ECC8, 50);
        c.write32(0xFFFF_ECCC, 0x001F_1101); // words, both incrementing
        assert_eq!(c.next_event, u64::MAX, "DME = 0: nothing starts");
        c.write16(super::DMAOR, 0x0001);
        c.now = 10 * super::CYCLES_PER_UNIT;
        c.write32(0xFFFF_ECCC, 0x001F_1100);
        assert_eq!(c.read32(0xFFFF_ECC8), 40, "ten units, then stopped");
        assert_eq!(c.next_event, u64::MAX);
        assert_eq!(c.read32(0xFFFF_ECCC) & 2, 0, "aborted: no TE");
    }

    #[test]
    fn a_misaligned_address_sets_ae() {
        let mut c = chip();
        c.write16(super::DMAOR, 0x0001);
        c.write32(0xFFFF_ECC0, 0xFFFE_8001);
        c.write32(0xFFFF_ECC4, 0xFFFE_9000);
        c.write32(0xFFFF_ECC8, 5);
        c.write32(0xFFFF_ECCC, 0x001F_0011);
        c.now = 100;
        c.advance();
        assert_eq!(c.read16(super::DMAOR), 0x0005, "AE");
        assert_eq!(c.dmac.transfers, 0);
        c.write16(super::DMAOR, 0x0007);
        assert_eq!(
            c.read16(super::DMAOR),
            0x0005,
            "writing 1 keeps AE and cannot set NMIF"
        );
        c.write16(super::DMAOR, 0x0001);
        assert_eq!(c.read16(super::DMAOR), 0x0001, "cleared by writing 0");
    }

    #[test]
    fn a_peripheral_request_is_counted_not_run() {
        let mut c = chip();
        c.write16(super::DMAOR, 0x0001);
        c.write32(0xFFFF_ECC8, 5);
        c.write32(0xFFFF_ECCC, 0xEFE3_CCC1); // RS = SCI1 transmit (00011), reserved bits set
        assert_eq!(c.dmac.peripheral_requests, 1);
        c.now = 1000;
        c.advance();
        assert_eq!(c.dmac.transfers, 0);
        assert_eq!(c.read32(0xFFFF_ECCC), 0x0003_0001, "reserved bits read 0");
    }
}
