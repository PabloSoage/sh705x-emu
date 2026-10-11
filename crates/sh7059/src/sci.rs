//! Serial communication interface, five channels, clocked synchronous mode
//! (manual section 15). The ECU's firmware uses SCI0, 2, 3 and 4 in this mode, polled,
//! to talk to other chips on the board.
//!
//! Registers per channel at `FFFFF000 + 8 × k`: SMR, BRR, SCR, TDR, SSR, RDR, SDCR.
//!
//! What is modelled:
//! - writing 0 to TDRE (after TDR) starts a byte when the transmitter is idle: TDR moves
//!   to the shift register, TDRE is set again at once and TEND cleared; a byte written
//!   while one is going out waits in TDR;
//! - a byte takes 8 bit times, B = Pφ / (4 × 4^n × (N + 1)) (manual 15.2.8, synchronous
//!   formula), with n from SMR.CKS and N from BRR;
//! - at the end of a byte with RE set, what the peer sent lands in RDR and sets RDRF
//!   (ORER instead, if RDRF was still 1); then the next byte goes out, or TEND is set;
//! - TDRE, RDRF, ORER, FER and PER are cleared by writing 0 (writing 1 keeps them);
//!   TEND is read-only and is cleared together with TDRE; TE = 0 sets TDRE and TEND.
//!
//! Not modelled: asynchronous mode, receive-only clocking (RE without TE), the SCI
//! interrupts (the firmware polls), and DMA. Data direction (SDCR.DIR) is passed to the
//! peer as is: the byte in TDR is what the firmware wrote.

/// First register address.
pub const BASE: u32 = 0xFFFF_F000;
/// One past the last register address of the five channels.
pub const END: u32 = 0xFFFF_F028;

const TDRE: u8 = 0x80;
const RDRF: u8 = 0x40;
const ORER: u8 = 0x20;
const TEND: u8 = 0x04;
const CLEARABLE: u8 = TDRE | RDRF | ORER | 0x10 | 0x08;
const TE: u8 = 0x20;
const RE: u8 = 0x10;

/// The chip at the other end of a channel.
pub trait Peer: Send {
    /// One byte each way: receives `tx` and returns the byte shifted back.
    fn exchange(&mut self, tx: u8, msb_first: bool) -> u8;

    /// A port pin wired to this peer (see [`crate::Wire`]) changed level. `line` is the
    /// number given in the wiring.
    fn pin(&mut self, _line: usize, _high: bool) {}

    /// The level of the data line from the peer between bytes: what a port pin wired to
    /// the receive line reads. Undriven, it is taken as high.
    fn data_out(&self) -> bool {
        true
    }

    /// The peer as `Any`, for a board model to get its own type back.
    fn as_any(&self) -> Option<&dyn std::any::Any> {
        None
    }

    /// The peer as mutable `Any`, for a board model to add to its own type.
    fn as_any_mut(&mut self) -> Option<&mut dyn std::any::Any> {
        None
    }
}

/// Nothing attached: every exchange returns the same byte.
pub struct Fixed(pub u8);

impl Peer for Fixed {
    fn exchange(&mut self, _tx: u8, _msb_first: bool) -> u8 {
        self.0
    }

    fn data_out(&self) -> bool {
        self.0 != 0
    }
}

/// One SCI channel.
pub struct Channel {
    /// SMR.
    pub smr: u8,
    /// BRR.
    pub brr: u8,
    /// SCR.
    pub scr: u8,
    /// TDR.
    pub tdr: u8,
    /// SSR.
    pub ssr: u8,
    /// RDR.
    pub rdr: u8,
    /// SDCR.
    pub sdcr: u8,
    /// The byte being shifted out, and the cycle it ends.
    shifting: Option<(u8, u64)>,
    /// What is on the other end.
    pub peer: Box<dyn Peer>,
    /// Bytes sent, for tracing.
    pub sent: Vec<u8>,
    /// Bytes received.
    pub received: Vec<u8>,
}

impl Default for Channel {
    fn default() -> Self {
        Self {
            smr: 0,
            brr: 0xFF,
            scr: 0,
            tdr: 0xFF,
            ssr: 0x84,
            rdr: 0,
            sdcr: 0xF2,
            shifting: None,
            peer: Box::new(Fixed(0)),
            sent: Vec::new(),
            received: Vec::new(),
        }
    }
}

impl Channel {
    /// CPU cycles per byte in synchronous mode: 8 bits of 4 × 4^n × (N + 1) Pφ cycles,
    /// with Pφ = φ/4.
    pub fn cycles_per_byte(&self) -> u64 {
        let n = (self.smr & 3) as u32;
        8 * 4 * 4u64.pow(n) * (self.brr as u64 + 1) * 4
    }

    fn start(&mut self, now: u64) {
        self.shifting = Some((self.tdr, now + self.cycles_per_byte()));
        self.ssr |= TDRE;
        self.ssr &= !TEND;
    }

    fn finish(&mut self, end: u64) {
        let (tx, _) = self.shifting.take().expect("finish without a byte");
        self.sent.push(tx);
        let rx = self.peer.exchange(tx, self.sdcr & 0x08 != 0);
        if self.scr & RE != 0 {
            if self.ssr & RDRF != 0 {
                self.ssr |= ORER;
            } else {
                self.rdr = rx;
                self.ssr |= RDRF;
                self.received.push(rx);
            }
        }
        if self.ssr & TDRE == 0 && self.scr & TE != 0 {
            self.start(end);
        } else {
            self.ssr |= TEND;
        }
    }
}

/// The five SCI channels.
#[derive(Default)]
pub struct Sci {
    /// SCI0..SCI4.
    pub ch: [Channel; 5],
}

impl Sci {
    /// Ends every byte due by `now`.
    pub fn advance(&mut self, now: u64) {
        for c in &mut self.ch {
            while let Some((_, end)) = c.shifting {
                if end > now {
                    break;
                }
                c.finish(end);
            }
        }
    }

    /// The earliest cycle at which a byte ends.
    pub fn next_event(&self) -> u64 {
        self.ch
            .iter()
            .filter_map(|c| c.shifting.map(|(_, end)| end))
            .min()
            .unwrap_or(u64::MAX)
    }

    /// Writes the register values into `regs` (the 40 bytes from [`BASE`]).
    pub fn store(&self, regs: &mut [u8]) {
        for k in 0..self.ch.len() {
            self.store_channel(k, regs);
        }
    }

    /// Writes channel `k`'s registers into `regs` (the 40 bytes from [`BASE`]). Reads
    /// only need this: the firmware polls SSR far more than anything else.
    #[inline(always)]
    pub fn store_channel(&self, k: usize, regs: &mut [u8]) {
        let c = &self.ch[k];
        regs[8 * k..8 * k + 7].copy_from_slice(&[c.smr, c.brr, c.scr, c.tdr, c.ssr, c.rdr, c.sdcr]);
    }

    /// Takes a write: `before` and `after` are the 40 register bytes around it.
    pub fn load(&mut self, now: u64, before: &[u8], after: &[u8]) {
        for (k, c) in self.ch.iter_mut().enumerate() {
            let (b, a) = (&before[8 * k..8 * k + 7], &after[8 * k..8 * k + 7]);
            if a == b {
                continue;
            }
            c.smr = a[0];
            c.brr = a[1];
            c.tdr = a[3];
            c.sdcr = (a[6] & 0x08) | 0xF2;
            // SSR: the clearable flags only go from 1 to 0; TEND and MPB are read-only.
            // TEND needs no handling of its own here: clearing TDRE with TE set either
            // starts a byte (which clears TEND) or finds one going out (TEND already 0),
            // and with TE clear TEND is held at 1.
            c.ssr = (c.ssr & !CLEARABLE) | (c.ssr & a[4] & CLEARABLE) | (a[4] & 0x01);
            c.scr = a[2];
            if c.scr & TE == 0 {
                c.ssr |= TDRE | TEND;
            } else if c.ssr & TDRE == 0 && c.shifting.is_none() {
                c.start(now);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Echo(Vec<u8>);
    impl Peer for Echo {
        fn exchange(&mut self, tx: u8, _: bool) -> u8 {
            self.0.push(tx);
            tx ^ 0xFF
        }
    }

    fn write(sci: &mut Sci, now: u64, k: usize, off: usize, v: u8) {
        let mut before = vec![0; 40];
        sci.store(&mut before);
        let mut after = before.clone();
        after[8 * k + off] = v;
        sci.load(now, &before, &after);
    }

    #[test]
    fn a_synchronous_exchange_as_the_firmware_does_it() {
        let mut sci = Sci::default();
        sci.ch[3].peer = Box::new(Echo(Vec::new()));
        write(&mut sci, 0, 3, 0, 0x80); // SMR: synchronous, Pφ
        write(&mut sci, 0, 3, 1, 4); // BRR = 4: 1 Mbit/s at Pφ = 20 MHz
        write(&mut sci, 0, 3, 2, 0x30); // SCR: TE, RE
        write(&mut sci, 10, 3, 3, 0x13); // TDR
        assert_eq!(
            sci.ch[3].ssr & (TDRE | TEND),
            TDRE | TEND,
            "nothing until TDRE is cleared"
        );
        write(&mut sci, 10, 3, 4, 0x84 & !TDRE); // clear TDRE: go
        assert_eq!(
            sci.ch[3].ssr & (TDRE | TEND),
            TDRE,
            "TDR taken at once; transmitting"
        );
        let per_byte = 8 * 4 * 5 * 4; // 8 bits x 4 x (N + 1) Pφ, x4 for φ
        assert_eq!(sci.ch[3].cycles_per_byte(), per_byte);
        assert_eq!(sci.next_event(), 10 + per_byte);
        sci.advance(10 + per_byte);
        assert_eq!(sci.ch[3].ssr & (TDRE | TEND | RDRF), TDRE | TEND | RDRF);
        assert_eq!(sci.ch[3].rdr, 0x13 ^ 0xFF);
        assert_eq!(sci.ch[3].sent, [0x13]);
    }

    #[test]
    fn a_second_byte_waits_and_an_unread_one_overruns() {
        let mut sci = Sci::default();
        write(&mut sci, 0, 2, 2, 0x30);
        write(&mut sci, 0, 2, 3, 1);
        write(&mut sci, 0, 2, 4, 0x04); // clear TDRE: byte 1 out
        write(&mut sci, 5, 2, 3, 2);
        write(&mut sci, 5, 2, 4, 0x04); // clear TDRE again: byte 2 waits in TDR
        assert_eq!(sci.ch[2].ssr & TDRE, 0);
        let t = sci.ch[2].cycles_per_byte();
        sci.advance(t);
        assert_eq!(
            sci.ch[2].ssr & (TDRE | RDRF | TEND),
            TDRE | RDRF,
            "byte 2 started"
        );
        sci.advance(2 * t);
        assert_eq!(sci.ch[2].ssr & ORER, ORER, "RDRF still set: overrun");
        assert_eq!(sci.ch[2].sent, [1, 2]);
    }

    #[test]
    fn flags_are_only_cleared_by_writing_zero() {
        let mut sci = Sci::default();
        write(&mut sci, 0, 0, 4, 0xFF);
        assert_eq!(
            sci.ch[0].ssr, 0x85,
            "writing 1 sets no flag; MPBT (bit 0) is R/W"
        );
        write(&mut sci, 0, 0, 2, 0x20);
        write(&mut sci, 0, 0, 4, 0x00);
        assert_eq!(sci.ch[0].ssr & TEND, 0, "TEND cleared with TDRE");
        write(&mut sci, 1, 0, 2, 0x00);
        assert_eq!(
            sci.ch[0].ssr & (TDRE | TEND),
            TDRE | TEND,
            "TE = 0 sets both"
        );
    }
}
