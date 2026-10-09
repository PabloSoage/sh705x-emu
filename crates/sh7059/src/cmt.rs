//! Compare match timer, two channels (manual section 14).
//!
//! Registers (16-bit, accessible as 8, 16 or 32 bits):
//!
//! | Address | Register | Bits |
//! |---|---|---|
//! | `FFFFF710` | CMSTR | STR1 (1), STR0 (0) |
//! | `FFFFF712`, `FFFFF718` | CMCSR0/1 | CMF (7), CMIE (6), CKS1..0 (1..0): Pφ/8, /32, /128, /512 |
//! | `FFFFF714`, `FFFFF71A` | CMCNT0/1 | counter |
//! | `FFFFF716`, `FFFFF71C` | CMCOR0/1 | compare value |
//!
//! CMCNT counts up on the selected clock; when it matches CMCOR, CMF is set and CMCNT
//! is cleared, so the period is CMCOR + 1 counts. CMF can only be cleared by writing 0.
//!
//! Counters are not ticked: each channel keeps the count it had at a reference time and
//! the cycle of its next match, and its value is computed when read.

/// First register address.
pub const BASE: u32 = 0xFFFF_F710;
/// One past the last register address.
pub const END: u32 = 0xFFFF_F71E;

const CMF: u16 = 0x80;
const CMIE: u16 = 0x40;

/// One CMT channel.
#[derive(Clone, Debug, Default)]
pub struct Channel {
    /// Counting (the channel's STR bit).
    pub running: bool,
    /// CMCSR.
    pub csr: u16,
    /// CMCOR.
    pub cor: u16,
    /// CMCNT at `base_cycle`.
    cnt: u16,
    /// CPU cycle at which the count was `cnt`.
    base_cycle: u64,
}

impl Channel {
    /// CPU cycles per count: Pφ is φ/4 (manual 5.1), divided by 8, 32, 128 or 512.
    fn cycles_per_count(&self) -> u64 {
        4 * [8, 32, 128, 512][(self.csr & 3) as usize]
    }

    /// Counts until the next match from count `c`.
    fn counts_to_match(&self, c: u16) -> u64 {
        if c <= self.cor {
            (self.cor - c) as u64 + 1
        } else {
            0x1_0000 - c as u64 + self.cor as u64 + 1
        }
    }

    /// The counter value at `now`, without side effects. Matches between `base_cycle`
    /// and `now` are taken by [`Cmt::advance`] before this is called.
    fn count_at(&self, now: u64) -> u16 {
        if !self.running {
            return self.cnt;
        }
        let elapsed = (now - self.base_cycle) / self.cycles_per_count();
        let to_match = self.counts_to_match(self.cnt);
        if elapsed < to_match {
            self.cnt.wrapping_add(elapsed as u16)
        } else {
            ((elapsed - to_match) % (self.cor as u64 + 1)) as u16
        }
    }

    /// CPU cycle of the next match, if counting.
    fn next_match(&self) -> Option<u64> {
        self.running
            .then(|| self.base_cycle + self.counts_to_match(self.cnt) * self.cycles_per_count())
    }

    /// Re-anchors the count at `now`, so a register change takes effect from here.
    fn resync(&mut self, now: u64) {
        self.cnt = self.count_at(now);
        self.base_cycle = now - (now - self.base_cycle) % self.cycles_per_count();
    }

    /// Whether this channel requests its interrupt.
    pub fn requesting(&self) -> bool {
        self.csr & (CMF | CMIE) == CMF | CMIE
    }
}

/// The two-channel compare match timer.
#[derive(Clone, Debug, Default)]
pub struct Cmt {
    /// Channels 0 and 1.
    pub ch: [Channel; 2],
    /// Compare matches since power-on, per channel.
    pub matches: [u64; 2],
}

impl Cmt {
    /// Takes every match up to `now`: sets CMF and moves the reference forward.
    pub fn advance(&mut self, now: u64) {
        for (i, c) in self.ch.iter_mut().enumerate() {
            while let Some(t) = c.next_match() {
                if t > now {
                    break;
                }
                c.csr |= CMF;
                c.cnt = 0;
                c.base_cycle = t;
                self.matches[i] += 1;
            }
        }
    }

    /// The earliest cycle at which something will happen.
    pub fn next_event(&self) -> u64 {
        self.ch
            .iter()
            .filter_map(Channel::next_match)
            .min()
            .unwrap_or(u64::MAX)
    }

    /// Writes the current register values into `regs` (the 14 bytes from [`BASE`]).
    pub fn store(&self, now: u64, regs: &mut [u8]) {
        let str_bits = self.ch[0].running as u16 | (self.ch[1].running as u16) << 1;
        let put = |regs: &mut [u8], off: usize, v: u16| {
            regs[off..off + 2].copy_from_slice(&v.to_be_bytes());
        };
        put(regs, 0x0, str_bits);
        for (k, c) in self.ch.iter().enumerate() {
            let o = 2 + 6 * k;
            put(regs, o, c.csr);
            put(regs, o + 2, c.count_at(now));
            put(regs, o + 4, c.cor);
        }
    }

    /// Takes a write: `before` and `after` are the 14 register bytes around it.
    pub fn load(&mut self, now: u64, before: &[u8], after: &[u8]) {
        let get = |r: &[u8], off: usize| u16::from_be_bytes([r[off], r[off + 1]]);
        let str_bits = get(after, 0);
        for (k, c) in self.ch.iter_mut().enumerate() {
            let o = 2 + 6 * k;
            c.resync(now);
            let old_csr = get(before, o);
            let new_csr = get(after, o);
            // CMF: writing 1 keeps it, writing 0 clears it; software cannot set it.
            let cmf = old_csr & new_csr & CMF;
            c.csr = (new_csr & (CMIE | 3)) | cmf;
            if get(before, o + 2) != get(after, o + 2) {
                c.cnt = get(after, o + 2);
                c.base_cycle = now;
            }
            c.cor = get(after, o + 4);
            let run = str_bits & (1 << k) != 0;
            if run && !c.running {
                c.base_cycle = now;
            }
            c.running = run;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn regs(cmt: &Cmt, now: u64) -> Vec<u8> {
        let mut r = vec![0; 14];
        cmt.store(now, &mut r);
        r
    }

    fn write16(cmt: &mut Cmt, now: u64, off: usize, v: u16) {
        let before = regs(cmt, now);
        let mut after = before.clone();
        after[off..off + 2].copy_from_slice(&v.to_be_bytes());
        cmt.load(now, &before, &after);
    }

    #[test]
    fn channel_1_as_the_firmware_sets_it_up_ticks_every_2500_counts() {
        let mut c = Cmt::default();
        write16(&mut c, 0, 0xC, 0x09C3); // CMCOR1 = 2499
        write16(&mut c, 0, 0x8, 0x00C0); // CMCSR1: CMIE, Pφ/8 (CMF written as 1: ignored)
        assert_eq!(c.ch[1].csr, CMIE, "software cannot set CMF");
        write16(&mut c, 0, 0x0, 0x0002); // CMSTR: start channel 1
        let period = 2500 * 8 * 4; // counts x Pφ/8 x (φ/Pφ)
        assert_eq!(c.next_event(), period);
        c.advance(period - 1);
        assert!(!c.ch[1].requesting());
        assert_eq!(
            u16::from_be_bytes([regs(&c, period - 1)[0xA], regs(&c, period - 1)[0xB]]),
            2499
        );
        c.advance(period);
        assert!(c.ch[1].requesting());
        assert_eq!(c.matches[1], 1);
        assert_eq!(regs(&c, period)[0xA..0xC], [0, 0], "cleared on match");
        // Clearing CMF by writing 0.
        write16(&mut c, period + 10, 0x8, 0x0040);
        assert!(!c.ch[1].requesting());
        assert_eq!(c.next_event(), 2 * period);
        // Several periods at once.
        c.advance(10 * period + 5);
        assert_eq!(c.matches[1], 10);
    }

    #[test]
    fn stopped_channels_hold_their_count() {
        let mut c = Cmt::default();
        write16(&mut c, 0, 0x6, 100); // CMCOR0
        write16(&mut c, 0, 0x4, 7); // CMCNT0
        assert_eq!(c.next_event(), u64::MAX);
        assert_eq!(regs(&c, 1_000_000)[0x4..0x6], [0, 7]);
        write16(&mut c, 1000, 0x0, 1);
        // 94 counts to the match at Pφ/8: 94 * 32 cycles.
        assert_eq!(c.next_event(), 1000 + 94 * 32);
    }
}
