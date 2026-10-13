//! Controller area network, two HCAN-II channels of 32 mailboxes (manual section 17).
//!
//! Channel `ch` has its registers at `FFFFD000 + 0x800 × ch`; everything lives in the
//! chip's register storage and this module acts on writes and on bus events:
//!
//! | Offset | Register | Modelled |
//! |---|---|---|
//! | `000` | MCR | reset request (MCR0), halt (MCR1), message priority (MCR2), CAN-bus endian (MCR4), sleep (MCR5) |
//! | `002` | GSR | read-only: bus off and warnings never, GSR2 (no TXPR), GSR3 (reset), GSR4 (halt/sleep) |
//! | `004`, `006` | BCR1, BCR0 | bit timing, which sets how long a frame takes |
//! | `008` | IRR | IRR0 and IRR3–7, 10–15 cleared by writing 1; IRR1, 2, 8, 9 follow RXPR, RFPR, TXACK/ABACK and UMSR under MBIMR |
//! | `00A` | IMR | masks |
//! | `020`–`05A` | TXPR, TXCR, TXACK, ABACK, RXPR, RFPR, MBIMR, UMSR | in pairs: register 1 holds mailboxes 31–16, register 0 holds 15–0 |
//! | `100 + 0x20 × N` | mailbox N | control (ID, RTR, IDE, NMC, MBC, DLC), data, LAFM |
//!
//! Transmission: of the mailboxes waiting in TXPR, the one with the lowest arbitration
//! field goes first (or the highest mailbox number with MCR2); it takes the frame's bits
//! at the bit rate of BCR, without stuff bits [EST], and then TXACK is set and the
//! frame goes to the outbox. Transmissions are never lost: there is no other node to
//! lose arbitration to, and every frame is acknowledged.
//!
//! Reception: a frame from the bus is matched from mailbox 31 down to 0 against the ID
//! of each receive mailbox, with the LAFM bits as don't-cares, and stored in the first
//! that matches. A mailbox whose RXPR or RFPR is still set keeps its message (NMC = 0) or
//! is overwritten (NMC = 1); UMSR is set either way.
//!
//! MBC = 011 is "setting prohibited" in the manual, yet the Astra's firmware uses it for
//! receive mailboxes; it is taken as receiving data frames [SUP].
//!
//! Not modelled: the HCAN-II timer (not supported on the SH7059 anyway), error counters
//! and the error states, bus-off, overload frames, the test modes, automatic
//! transmission on a remote frame (ATX), and DMAC activation from mailbox 0.

use crate::IO_BASE;
use std::collections::VecDeque;

/// Other nodes on a CAN bus, as seen from the ECU: their frames in time order.
pub trait CanSource: Send {
    /// The next frame and the cycle at which its last bit is through, without taking it.
    fn peek(&self) -> Option<(u64, Frame)>;
    /// Takes the next frame.
    fn take(&mut self) -> Option<(u64, Frame)>;
}

/// Frames a channel's [`Channel::tap`] keeps.
pub const TAP_KEPT: usize = 4096;

/// First HCAN register (MCR_0).
pub const BASE: u32 = 0xFFFF_D000;
/// One past the last HCAN register (end of channel 1's mailbox 31).
pub const END: u32 = 0xFFFF_DD00;
/// Distance between the two channels' registers.
pub const CHANNEL_STRIDE: u32 = 0x800;

const MCR: u32 = 0x000;
const GSR: u32 = 0x002;
const BCR1: u32 = 0x004;
const BCR0: u32 = 0x006;
const IRR: u32 = 0x008;
const IMR: u32 = 0x00A;
const TXPR: u32 = 0x020;
const TXCR: u32 = 0x028;
const TXACK: u32 = 0x030;
const ABACK: u32 = 0x038;
const RXPR: u32 = 0x040;
const RFPR: u32 = 0x048;
const MBIMR: u32 = 0x050;
const UMSR: u32 = 0x058;
const MAILBOXES: u32 = 0x100;

/// IRR bits cleared by writing 1.
const IRR_W1C: u16 = 0xFCF9;

/// A CAN frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Frame {
    /// Identifier: 11 bits, or 29 bits if `extended`.
    pub id: u32,
    /// 29-bit identifier.
    pub extended: bool,
    /// Remote frame.
    pub remote: bool,
    /// Data length code, 0 to 8.
    pub dlc: u8,
    /// Data bytes, the first `dlc` of them in use.
    pub data: [u8; 8],
}

impl Frame {
    /// A standard data frame.
    pub fn standard(id: u32, data: &[u8]) -> Self {
        let mut d = [0u8; 8];
        let n = data.len().min(8);
        d[..n].copy_from_slice(&data[..n]);
        Self {
            id: id & 0x7FF,
            extended: false,
            remote: false,
            dlc: n as u8,
            data: d,
        }
    }

    /// Bits on the bus without stuff bits, interframe space included: 47 + 8 × bytes for
    /// a standard data frame, 67 + 8 × bytes for an extended one (CAN 2.0).
    pub fn bits(&self) -> u64 {
        let bytes = if self.remote {
            0
        } else {
            self.dlc.min(8) as u64
        };
        (if self.extended { 67 } else { 47 }) + 8 * bytes
    }

    /// The arbitration field as one number, lower meaning higher priority: the 11-bit
    /// base ID, then SRR/RTR and IDE, then the 18-bit extension.
    fn arbitration(&self) -> u64 {
        let (base, ext) = if self.extended {
            (self.id >> 18, self.id & 0x3_FFFF)
        } else {
            (self.id, 0)
        };
        // Standard: RTR then IDE = 0. Extended: SRR = 1, IDE = 1, then the extension and RTR.
        let (b1, b2) = if self.extended {
            (1, 1)
        } else {
            (self.remote as u64, 0)
        };
        ((base as u64) << 21)
            | (b1 << 20)
            | (b2 << 19)
            | ((ext as u64) << 1)
            | (self.extended && self.remote) as u64
    }
}

/// One HCAN-II channel's state that is not in its registers.
#[derive(Clone, Debug, Default)]
pub struct Channel {
    /// The mailbox being transmitted and the cycle it ends.
    sending: Option<(usize, u64)>,
    /// Frames transmitted, with the cycle each ended.
    pub outbox: VecDeque<(u64, Frame)>,
    /// A second copy of the frames transmitted, while some tool wants one of its own (the
    /// outbox is drained by the diagnostic link): at most [`TAP_KEPT`], oldest dropped.
    pub tap: Option<VecDeque<(u64, Frame)>>,
    /// Frames received and stored, rejected for want of a mailbox, and lost to NMC = 0.
    pub received: u64,
    /// Frames from the bus that no mailbox took.
    pub unmatched: u64,
    /// Frames kept out by an unread message (NMC = 0).
    pub overruns: u64,
}

/// Both HCAN-II channels.
#[derive(Clone, Debug, Default)]
pub struct Hcan {
    /// HCAN0 and HCAN1.
    pub ch: [Channel; 2],
}

fn base(ch: usize) -> u32 {
    BASE + CHANNEL_STRIDE * ch as u32
}

fn r16(io: &[u8], a: u32) -> u16 {
    let o = (a - IO_BASE) as usize;
    u16::from_be_bytes([io[o], io[o + 1]])
}

fn w16(io: &mut [u8], a: u32, v: u16) {
    let o = (a - IO_BASE) as usize;
    io[o..o + 2].copy_from_slice(&v.to_be_bytes());
}

/// A register pair as one 32-bit mask, bit N for mailbox N.
fn r32(io: &[u8], ch: usize, reg: u32) -> u32 {
    let b = base(ch) + reg;
    ((r16(io, b) as u32) << 16) | r16(io, b + 2) as u32
}

fn w32(io: &mut [u8], ch: usize, reg: u32, v: u32) {
    let b = base(ch) + reg;
    w16(io, b, (v >> 16) as u16);
    w16(io, b + 2, v as u16);
}

fn mailbox(ch: usize, n: usize) -> u32 {
    base(ch) + MAILBOXES + 0x20 * n as u32
}

/// MBC of mailbox `n`.
fn mbc(io: &[u8], ch: usize, n: usize) -> u16 {
    (r16(io, mailbox(ch, n) + 4) >> 8) & 7
}

/// Whether mailbox `n` transmits (MBC = 000 or 001) and receives data frames.
fn transmits(io: &[u8], ch: usize, n: usize) -> bool {
    n != 0 && matches!(mbc(io, ch, n), 0 | 1)
}

fn receives_data(io: &[u8], ch: usize, n: usize) -> bool {
    matches!(mbc(io, ch, n), 2..=5)
}

fn receives_remote(io: &[u8], ch: usize, n: usize) -> bool {
    matches!(mbc(io, ch, n), 1 | 2 | 4)
}

/// The ID, IDE and RTR set in mailbox `n` (or its LAFM, at offset 0x10).
fn id_fields(io: &[u8], at: u32) -> (u32, bool, bool) {
    let c0 = r16(io, at);
    let c1 = r16(io, at + 2);
    let std = ((c0 >> 4) & 0x7FF) as u32;
    let ext = (((c0 & 3) as u32) << 16) | c1 as u32;
    (std << 18 | ext, c0 & 4 != 0, c0 & 8 != 0)
}

impl Hcan {
    /// Initial values after a power-on reset (manual 17.4).
    pub fn reset(&mut self, io: &mut [u8]) {
        for ch in 0..2 {
            Self::reset_channel(io, ch);
            self.ch[ch] = Channel::default();
        }
    }

    /// The registers after a reset request: MCR0 and GSR3 set, IRR0 set, everything masked.
    fn reset_channel(io: &mut [u8], ch: usize) {
        let b = base(ch);
        let mcr = r16(io, b + MCR) & !0x0022 | 0x0001;
        w16(io, b + MCR, mcr);
        w16(io, b + GSR, 0x000C);
        w16(io, b + IRR, 0x0001);
        w16(io, b + IMR, 0xFFFF);
        for reg in [TXPR, TXCR, TXACK, ABACK, RXPR, RFPR, UMSR] {
            w32(io, ch, reg, 0);
        }
        w32(io, ch, MBIMR, 0xFFFF_FFFF);
    }

    /// CPU cycles per bit from BCR: (BRP + 1) × (TSEG1 + TSEG2 + 3) Pφ periods, Pφ = φ/4.
    pub fn cycles_per_bit(io: &[u8], ch: usize) -> u64 {
        let bcr1 = r16(io, base(ch) + BCR1);
        let brp = (r16(io, base(ch) + BCR0) & 0xFF) as u64;
        let tseg1 = ((bcr1 >> 12) & 0xF) as u64;
        let tseg2 = ((bcr1 >> 8) & 7) as u64;
        4 * (brp + 1) * (tseg1 + tseg2 + 3)
    }

    /// The channel takes part in the bus: out of reset, not halted, not asleep.
    fn active(io: &[u8], ch: usize) -> bool {
        r16(io, base(ch) + MCR) & 0x0023 == 0
    }

    /// Takes a write to the HCAN registers. `before` holds the bytes from `addr` before
    /// it; the storage already holds the written ones.
    pub fn load(&mut self, now: u64, addr: u32, before: &[u8], io: &mut [u8]) {
        let ch = ((addr - BASE) / CHANNEL_STRIDE) as usize;
        let b = base(ch);
        let off = addr - b;
        let n = before.len() as u32;
        let hit = |reg: u32, len: u32| off < reg + len && reg < off + n;
        // The 16-bit value a register had before the write.
        let old16 = |reg: u32, io: &[u8]| {
            let mut v = [0u8; 2];
            for (i, x) in v.iter_mut().enumerate() {
                let a = b + reg + i as u32;
                *x = if (addr..addr + n).contains(&a) {
                    before[(a - addr) as usize]
                } else {
                    io[(a - IO_BASE) as usize]
                };
            }
            u16::from_be_bytes(v)
        };
        let old32 =
            |reg: u32, io: &[u8]| ((old16(reg, io) as u32) << 16) | old16(reg + 2, io) as u32;
        if hit(MCR, 2) {
            let mcr = r16(io, b + MCR);
            let was = old16(MCR, io);
            if mcr & 1 != 0 && was & 1 == 0 {
                Self::reset_channel(io, ch);
                self.ch[ch].sending = None;
            }
            // GSR3 follows MCR0 (joining the bus after 11 recessive bits is taken as
            // immediate); GSR4 follows halt or sleep, both entered at once on an idle bus.
            let mut gsr = r16(io, b + GSR) & !0x0018;
            if mcr & 1 != 0 {
                gsr |= 0x0008;
            }
            if mcr & 0x0022 != 0 {
                gsr |= 0x0010;
            }
            w16(io, b + GSR, gsr);
            let entering = (mcr & !was) & 0x0022 != 0;
            if entering {
                let irr = r16(io, b + IRR) | 1;
                w16(io, b + IRR, irr);
            }
        }
        if hit(GSR, 2) {
            let v = old16(GSR, io);
            w16(io, b + GSR, v);
        }
        if hit(IRR, 2) {
            let (was, w) = (old16(IRR, io), r16(io, b + IRR));
            w16(io, b + IRR, was & !(w & IRR_W1C));
        }
        if hit(0x00C, 2) {
            let v = old16(0x00C, io);
            w16(io, b + 0x00C, v); // TEC/REC: read-only outside test mode
        }
        if hit(TXPR, 4) {
            let (was, w) = (old32(TXPR, io), r32(io, ch, TXPR));
            let mut set = w & !1;
            for m in 1..32 {
                if set & (1 << m) != 0 && !transmits(io, ch, m) {
                    set &= !(1 << m);
                }
            }
            w32(io, ch, TXPR, was | set);
        }
        if hit(TXCR, 4) {
            let w = r32(io, ch, TXCR) & !1;
            w32(io, ch, TXCR, 0);
            let txpr = r32(io, ch, TXPR);
            let sending = self.ch[ch].sending.map(|(m, _)| m);
            // A mailbox already on the bus finishes; the others are cancelled at once.
            let cancel = w & txpr & !sending.map_or(0, |m| 1 << m);
            if cancel != 0 {
                w32(io, ch, TXPR, txpr & !cancel);
                let ab = r32(io, ch, ABACK) | cancel;
                w32(io, ch, ABACK, ab);
            }
        }
        for reg in [TXACK, ABACK, RXPR, RFPR, UMSR] {
            if hit(reg, 4) {
                let (was, w) = (old32(reg, io), r32(io, ch, reg));
                w32(io, ch, reg, was & !w);
            }
        }
        Self::derive_irr(io, ch);
        self.start(now, io, ch);
    }

    /// IRR1, IRR2, IRR8 and IRR9 from RXPR, RFPR, TXACK/ABACK and UMSR, under MBIMR; and
    /// GSR2 from TXPR.
    fn derive_irr(io: &mut [u8], ch: usize) {
        let open = !r32(io, ch, MBIMR);
        let any = |reg| r32(io, ch, reg) & open != 0;
        let mut irr = r16(io, base(ch) + IRR) & !0x0306;
        if any(RXPR) {
            irr |= 0x0002;
        }
        if any(RFPR) {
            irr |= 0x0004;
        }
        if (r32(io, ch, TXACK) | r32(io, ch, ABACK)) & open != 0 {
            irr |= 0x0100;
        }
        if any(UMSR) {
            irr |= 0x0200;
        }
        w16(io, base(ch) + IRR, irr);
        let mut gsr = r16(io, base(ch) + GSR) & !0x0004;
        if r32(io, ch, TXPR) == 0 {
            gsr |= 0x0004;
        }
        w16(io, base(ch) + GSR, gsr);
    }

    /// The frame mailbox `n` holds, for transmission.
    fn frame_of(io: &[u8], ch: usize, n: usize) -> Frame {
        let at = mailbox(ch, n);
        let (id, ide, rtr) = id_fields(io, at);
        let dlc = (r16(io, at + 4) & 0xF).min(8) as u8;
        let mut data = [0u8; 8];
        let o = (at + 8 - IO_BASE) as usize;
        data.copy_from_slice(&io[o..o + 8]);
        if r16(io, base(ch) + MCR) & 0x10 != 0 {
            for p in data.chunks_mut(2) {
                p.swap(0, 1);
            }
        }
        Frame {
            id: if ide { id } else { id >> 18 },
            extended: ide,
            remote: rtr,
            dlc,
            data,
        }
    }

    /// Starts the next transmission if the bus is free and something waits.
    fn start(&mut self, now: u64, io: &[u8], ch: usize) {
        if self.ch[ch].sending.is_some() || !Self::active(io, ch) {
            return;
        }
        let txpr = r32(io, ch, TXPR);
        if txpr == 0 {
            return;
        }
        let by_number = r16(io, base(ch) + MCR) & 4 != 0;
        let pick = (1..32)
            .rev()
            .filter(|&m| txpr & (1 << m) != 0)
            .min_by_key(|&m| {
                if by_number {
                    (0, 31 - m as u64)
                } else {
                    // Ties (same arbitration field) go to the higher mailbox, scanned first.
                    (Self::frame_of(io, ch, m).arbitration(), 31 - m as u64)
                }
            });
        if let Some(m) = pick {
            let bits = Self::frame_of(io, ch, m).bits();
            self.ch[ch].sending = Some((m, now + bits * Self::cycles_per_bit(io, ch)));
        }
    }

    /// The cycle the frame on the bus ends.
    pub fn next_event(&self) -> u64 {
        self.ch
            .iter()
            .filter_map(|c| c.sending.map(|(_, end)| end))
            .min()
            .unwrap_or(u64::MAX)
    }

    /// Ends the transmissions due by `now` and starts the next ones.
    pub fn advance(&mut self, now: u64, io: &mut [u8]) {
        for ch in 0..2 {
            while let Some((m, end)) = self.ch[ch].sending {
                if end > now {
                    break;
                }
                self.ch[ch].sending = None;
                let f = Self::frame_of(io, ch, m);
                self.ch[ch].outbox.push_back((end, f));
                if let Some(t) = self.ch[ch].tap.as_mut() {
                    if t.len() == TAP_KEPT {
                        t.pop_front();
                    }
                    t.push_back((end, f));
                }
                let txpr = r32(io, ch, TXPR) & !(1 << m);
                w32(io, ch, TXPR, txpr);
                let ack = r32(io, ch, TXACK) | (1 << m);
                w32(io, ch, TXACK, ack);
                Self::derive_irr(io, ch);
                self.start(end, io, ch);
            }
        }
    }

    /// A frame arrives from the bus on channel `ch`: it is stored in the first matching
    /// receive mailbox from 31 down to 0. Returns the mailbox, or None.
    pub fn deliver(&mut self, io: &mut [u8], ch: usize, f: &Frame) -> Option<usize> {
        if !Self::active(io, ch) {
            return None;
        }
        let want = if f.extended { f.id } else { f.id << 18 };
        let found = (0..32).rev().find(|&n| {
            let kind = if f.remote {
                receives_remote(io, ch, n)
            } else {
                receives_data(io, ch, n)
            };
            if !kind {
                return false;
            }
            let at = mailbox(ch, n);
            let (id, ide, _) = id_fields(io, at);
            let (mask, _, _) = id_fields(io, at + 0x10);
            // The standard part is always compared; the extension only for extended frames.
            let care = !mask & if f.extended { 0x1FFF_FFFF } else { 0x1FFC_0000 };
            ide == f.extended && (id ^ want) & care == 0
        });
        let Some(n) = found else {
            self.ch[ch].unmatched += 1;
            return None;
        };
        let flag = if f.remote { RFPR } else { RXPR };
        let pending = (r32(io, ch, RXPR) | r32(io, ch, RFPR)) & (1 << n) != 0;
        let at = mailbox(ch, n);
        let nmc = r16(io, at + 4) & 0x2000 != 0;
        if pending {
            let u = r32(io, ch, UMSR) | (1 << n);
            w32(io, ch, UMSR, u);
            if !nmc {
                self.ch[ch].overruns += 1;
                Self::derive_irr(io, ch);
                return Some(n);
            }
        }
        // Store the whole message: ID fields, DLC and data.
        let c0 = r16(io, at) & 0x8000
            | (((want >> 18) as u16 & 0x7FF) << 4)
            | (f.remote as u16) << 3
            | (f.extended as u16) << 2
            | ((want >> 16) & 3) as u16;
        w16(io, at, c0);
        w16(io, at + 2, want as u16);
        let c2 = r16(io, at + 4) & !0xF | f.dlc.min(8) as u16;
        w16(io, at + 4, c2);
        let mut data = f.data;
        if r16(io, base(ch) + MCR) & 0x10 != 0 {
            for p in data.chunks_mut(2) {
                p.swap(0, 1);
            }
        }
        let o = (at + 8 - IO_BASE) as usize;
        io[o..o + 8].copy_from_slice(&data);
        let v = r32(io, ch, flag) | (1 << n);
        w32(io, ch, flag, v);
        self.ch[ch].received += 1;
        Self::derive_irr(io, ch);
        Some(n)
    }

    /// Interrupt requests of channel `ch`: ERS, OVR, RM and SLE (manual table 17.10).
    pub fn requests(io: &[u8], ch: usize) -> [bool; 4] {
        let pending = r16(io, base(ch) + IRR) & !r16(io, base(ch) + IMR);
        [
            pending & 0x0079 != 0, // IRR0, 3, 4, 5, 6
            pending & 0xFE80 != 0, // IRR7, 9 to 15
            pending & 0x0006 != 0, // IRR1, 2
            pending & 0x0100 != 0, // IRR8
        ]
    }
}

#[cfg(test)]
mod tests {
    use crate::{ROM_SIZE, Sh7059};
    use sh2e::Bus;

    use super::Frame;

    const B: u32 = super::BASE;

    fn chip() -> Sh7059 {
        let mut c = Sh7059::new(&vec![0u8; ROM_SIZE]).unwrap();
        c.write16(B + 0x004, 0x5200); // BCR1: TSEG1 = 5, TSEG2 = 2
        c.write16(B + 0x006, 0x0003); // BCR0: BRP = 3 -> 500 kbit/s at Pφ = 20 MHz
        c
    }

    fn mb(n: u32) -> u32 {
        B + 0x100 + 0x20 * n
    }

    #[test]
    fn power_on_state_and_leaving_reset() {
        let mut c = chip();
        assert_eq!(c.read16(B), 0x0001, "MCR0 set");
        assert_eq!(c.read16(B + 2), 0x000C, "GSR3, GSR2");
        assert_eq!(c.read16(B + 8), 0x0001, "IRR0");
        assert_eq!(c.read16(B + 0xA), 0xFFFF, "IMR");
        assert_eq!(c.read16(B + 0x52), 0xFFFF, "MBIMR0");
        c.write16(B, 0x0000);
        assert_eq!(c.read16(B + 2) & 0x0008, 0, "out of reset");
        c.write16(B + 8, 0x0001);
        assert_eq!(c.read16(B + 8), 0, "IRR0 cleared by writing 1");
        c.write16(B + 2, 0xFFFF);
        assert_eq!(c.read16(B + 2), 0x0004, "GSR is read-only");
    }

    #[test]
    fn transmit_in_priority_order_and_acknowledge() {
        let mut c = chip();
        c.write16(B, 0);
        // The lower ID sits in the lower mailbox, so ID order and mailbox order differ.
        for (n, id) in [(5u32, 0x101u16), (9, 0x7E8)] {
            c.write16(mb(n), id << 4);
            c.write16(mb(n) + 4, 0x0008); // MBC = 000, DLC 8
            c.write16(mb(n) + 8, 0x0102 + n as u16);
        }
        c.write16(B + 0x22, (1 << 5) | (1 << 9)); // TXPR0
        assert_eq!(c.read16(B + 2) & 4, 0, "GSR2: transmission waiting");
        let bit = super::Hcan::cycles_per_bit(&c.io, 0);
        assert_eq!(bit, 4 * 4 * 10, "2 µs at φ = 80 MHz");
        assert_eq!(c.next_event, 111 * bit, "47 + 64 bits");
        c.now = c.next_event;
        c.advance();
        let (t, f) = c.hcan.ch[0].outbox.pop_front().unwrap();
        assert_eq!(
            (t, f.id, f.data[0], f.data[1]),
            (111 * bit, 0x101, 0x01, 0x07),
            "lowest ID first"
        );
        assert_eq!(c.read16(B + 0x32), 1 << 5, "TXACK0");
        assert_eq!(c.read16(B + 0x22), 1 << 9, "the other still waits");
        c.now = c.next_event;
        c.advance();
        assert_eq!(c.hcan.ch[0].outbox.pop_front().unwrap().1.id, 0x7E8);
        assert_eq!(c.read16(B + 2) & 4, 4, "GSR2: nothing waiting");
        // IRR8 only through MBIMR, the interrupt only through IMR and IPRL.
        assert_eq!(c.read16(B + 8) & 0x0100, 0, "masked by MBIMR");
        c.write16(B + 0x52, !(1 << 9));
        assert_eq!(c.read16(B + 8) & 0x0100, 0x0100, "IRR8");
        c.write16(0xFFFF_ED16, 0x0700); // IPRL: HCAN0 at level 7
        assert_eq!(c.pending, (0, 0), "IMR8 still set");
        c.write16(B + 0xA, !0x0100);
        assert_eq!(c.pending, (7, 223), "SLE0");
        c.write16(B + 0x32, 1 << 9); // clear TXACK
        assert_eq!(c.read16(B + 8) & 0x0100, 0);
        assert_eq!(c.pending, (0, 0));
    }

    #[test]
    fn cancel_a_waiting_transmission() {
        let mut c = chip();
        c.write16(B, 0);
        for n in [3u32, 4] {
            c.write16(mb(n), 0x100 << 4);
            c.write16(mb(n) + 4, 0x0001);
        }
        c.write16(B + 0x22, 0x0018);
        // Mailbox 4 has the same ID and goes first (higher number); cancel both.
        c.write16(B + 0x2A, 0x0018);
        assert_eq!(
            c.read16(B + 0x22),
            1 << 4,
            "the one on the bus cannot be cancelled"
        );
        assert_eq!(c.read16(B + 0x3A), 1 << 3, "ABACK0");
        assert_eq!(c.read16(B + 0x2A), 0, "TXCR reads back 0");
    }

    #[test]
    fn receive_with_mask_overrun_and_overwrite() {
        let mut c = chip();
        c.write16(B, 0);
        // Mailbox 13: 0x7DF, receive (MBC = 010). Mailbox 20: 0x7E0 with LAFM on the low
        // 4 bits and NMC = 1. Mailbox 2 uses the MBC = 011 the firmware writes.
        c.write16(mb(13), 0x7DF << 4);
        c.write16(mb(13) + 4, 0x0208);
        c.write16(mb(20), 0x7E0 << 4);
        c.write16(mb(20) + 4, 0x2208);
        c.write16(mb(20) + 0x10, 0x00F << 4);
        c.write16(mb(2), 0x123 << 4);
        c.write16(mb(2) + 4, 0x0308);
        c.write16(B + 0x22, 1 << 13);
        assert_eq!(
            c.read16(B + 0x22),
            0,
            "TXPR cannot be set for a receive mailbox"
        );
        let f = Frame::standard(0x7DF, &[0x02, 0x01, 0x00]);
        assert_eq!(c.can_deliver(0, &f), Some(13));
        assert_eq!(c.read16(mb(13) + 4) & 0xF, 3, "DLC stored");
        assert_eq!(c.read16(mb(13) + 8), 0x0201);
        assert_eq!(c.read16(B + 0x42), 1 << 13, "RXPR0");
        assert_eq!(
            c.can_deliver(0, &Frame::standard(0x7E5, &[9])),
            Some(20),
            "LAFM"
        );
        assert_eq!(
            c.read16(mb(20)) >> 4,
            0x7E5,
            "the received ID replaces the set one"
        );
        assert_eq!(c.can_deliver(0, &Frame::standard(0x123, &[1])), Some(2));
        assert_eq!(c.can_deliver(0, &Frame::standard(0x7E1, &[])), Some(20));
        assert_eq!(c.read16(mb(20)) >> 4, 0x7E1, "NMC = 1: overwritten");
        assert_eq!(c.read16(B + 0x58), 1 << 4, "UMSR1 bit for mailbox 20");
        assert_eq!(c.can_deliver(0, &Frame::standard(0x7DF, &[0xAA])), Some(13));
        assert_eq!(
            c.read16(mb(13) + 8) >> 8,
            0x02,
            "NMC = 0: the old message stays"
        );
        assert_eq!(c.can_deliver(0, &Frame::standard(0x555, &[])), None);
        assert_eq!(c.hcan.ch[0].unmatched, 1);
        // RM0 through MBIMR, IMR and IPRL.
        c.write16(0xFFFF_ED16, 0x0500);
        c.write16(B + 0x52, !(1 << 13));
        c.write16(B + 0xA, !0x0002);
        assert_eq!(c.pending, (5, 222), "RM0");
        c.write16(B + 0x42, 1 << 13);
        c.write16(B + 0x40, 0xFFFF);
        assert_eq!(c.pending, (0, 0));
    }

    #[test]
    fn a_remote_frame_sets_rfpr_and_requests_rm() {
        let mut c = chip();
        c.write16(B, 0);
        c.write16(mb(6), 0x200 << 4);
        c.write16(mb(6) + 4, 0x0400); // MBC = 100: receives data and remote frames
        let mut f = Frame::standard(0x200, &[]);
        f.remote = true;
        assert_eq!(c.can_deliver(0, &f), Some(6));
        assert_eq!(c.read16(B + 0x4A), 1 << 6, "RFPR0");
        assert_eq!(c.read16(B + 0x42), 0, "not RXPR");
        assert_eq!(c.read16(mb(6)) & 8, 8, "RTR stored");
        c.write16(0xFFFF_ED16, 0x0300);
        c.write16(B + 0x52, !(1 << 6));
        c.write16(B + 0xA, !0x0004);
        assert_eq!(c.pending, (3, 222), "RM0 from IRR2");
    }

    #[test]
    fn nothing_moves_in_reset_or_halt() {
        let mut c = chip();
        c.write16(mb(13), 0x7DF << 4);
        c.write16(mb(13) + 4, 0x0208);
        assert_eq!(
            c.can_deliver(0, &Frame::standard(0x7DF, &[])),
            None,
            "in reset"
        );
        c.write16(B, 0x0002);
        assert_eq!(c.read16(B + 2) & 0x0010, 0x0010, "GSR4: halted");
        assert_eq!(c.read16(B + 8) & 1, 1, "IRR0 on entering halt");
        assert_eq!(
            c.can_deliver(0, &Frame::standard(0x7DF, &[])),
            None,
            "halted"
        );
        c.write16(B, 0x0000);
        assert_eq!(c.can_deliver(0, &Frame::standard(0x7DF, &[])), Some(13));
    }

    #[test]
    fn extended_frames_and_channel_1() {
        let mut c = chip();
        let b1 = B + 0x800;
        c.write16(b1, 0);
        c.write16(b1 + 0x100 + 0x20 * 4, (0x3u16 << 4) | 0x4 | 0x3);
        c.write16(b1 + 0x100 + 0x20 * 4 + 2, 0xFFFE);
        c.write16(b1 + 0x100 + 0x20 * 4 + 4, 0x0208);
        let id = (0x3 << 18) | 0x3_FFFE;
        let f = Frame {
            id,
            extended: true,
            remote: false,
            dlc: 2,
            data: [1, 2, 0, 0, 0, 0, 0, 0],
        };
        assert_eq!(c.can_deliver(1, &f), Some(4));
        assert_eq!(
            c.can_deliver(1, &Frame::standard(0x3, &[])),
            None,
            "IDE must match"
        );
        assert_eq!(c.can_deliver(0, &f), None, "channel 0 is still in reset");
        assert_eq!(f.bits(), 67 + 16);
    }
}
