//! Microwire serial EEPROMs (93C86 in x16 organisation: 1,024 words of 16 bits) on a
//! clocked synchronous SCI channel.
//!
//! Instructions, after any number of leading zeros, a start bit and two opcode bits
//! (10 address bits for x16):
//!
//! | Instruction | Bits | Effect |
//! |---|---|---|
//! | READ | `1 10 A9–A0` | a dummy 0, then D15–D0; with the clock running, the next words follow |
//! | WRITE | `1 01 A9–A0 D15–D0` | programs a word when CS goes low |
//! | ERASE | `1 11 A9–A0` | sets a word to H'FFFF when CS goes low |
//! | EWEN | `1 00 11xxxxxxxx` | enables writing and erasing |
//! | EWDS | `1 00 00xxxxxxxx` | disables them (the state at power-on) |
//! | ERAL | `1 00 10xxxxxxxx` | erases every word when CS goes low |
//! | WRAL | `1 00 01xxxxxxxx D15–D0` | writes every word when CS goes low |
//!
//! DI is latched on the rising edge of the clock, and DO changes just after it. The SCI
//! latches its receive data on the rising edge too, so it sees DO one bit late, unless
//! the firmware inverts the serial clock pin with PLIR (SH7059 manual 22.3.25), which it
//! does during the data phase of a read. The bus therefore hands back DO as it is after
//! each edge when the clock is inverted, and as it was before the edge when not.
//!
//! Programming is instantaneous: after a write, DO shows "ready" (1) as soon as CS goes
//! high again. A real 93C86 takes a few milliseconds [SUP]. With no chip driving it, the
//! DO line reads 1, as if pulled up [SUP].

use crate::status::StatusChip;
use sh7059::sci::Peer;

/// Words in a 93C86 in x16 organisation.
pub const WORDS: usize = 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    /// Waiting for the start bit.
    Idle,
    /// Collecting the opcode and address bits after the start bit.
    Command { bits: u32, n: u32 },
    /// Shifting a word out of DO.
    Read { addr: u16, word: u16, left: u32 },
    /// Collecting the data of a WRITE (Some address) or WRAL (None).
    Data {
        addr: Option<u16>,
        bits: u16,
        n: u32,
    },
    /// Instruction complete: ignore the clock until CS goes low.
    Done,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Program {
    Write(u16, u16),
    WriteAll(u16),
    Erase(u16),
    EraseAll,
}

/// One 93C86.
#[derive(Clone, Debug)]
pub struct Eeprom93C86 {
    /// The contents.
    pub words: Vec<u16>,
    cs: bool,
    state: State,
    /// DO after the last edge: None while high impedance.
    out: Option<bool>,
    write_enabled: bool,
    pending: Option<Program>,
    /// A programming cycle has run since CS last went high: DO shows the ready status.
    status: bool,
    /// Words shifted out in full.
    pub reads: u64,
    /// Programming cycles carried out.
    pub programs: u64,
    /// Programming instructions refused because writing was disabled.
    pub refused: u64,
}

impl Eeprom93C86 {
    /// A chip holding `words` (padded with H'FFFF or cut to [`WORDS`]).
    pub fn new(words: &[u16]) -> Self {
        let mut w = vec![0xFFFF; WORDS];
        let n = words.len().min(WORDS);
        w[..n].copy_from_slice(&words[..n]);
        Self {
            words: w,
            cs: false,
            state: State::Idle,
            out: None,
            write_enabled: false,
            pending: None,
            status: false,
            reads: 0,
            programs: 0,
            refused: 0,
        }
    }

    /// A chip loaded from a 2 KB image, two bytes per word, high byte first if
    /// `big_endian`.
    pub fn from_bytes(bytes: &[u8], big_endian: bool) -> Self {
        let words: Vec<u16> = bytes
            .chunks(2)
            .map(|b| {
                let (x, y) = (b[0], *b.get(1).unwrap_or(&0xFF));
                if big_endian {
                    u16::from_be_bytes([x, y])
                } else {
                    u16::from_le_bytes([x, y])
                }
            })
            .collect();
        Self::new(&words)
    }

    /// The contents as bytes, in the same layout as [`Eeprom93C86::from_bytes`].
    pub fn to_bytes(&self, big_endian: bool) -> Vec<u8> {
        self.words
            .iter()
            .flat_map(|w| {
                if big_endian {
                    w.to_be_bytes()
                } else {
                    w.to_le_bytes()
                }
            })
            .collect()
    }

    /// Chip select. Going low ends the instruction and starts any programming cycle.
    pub fn select(&mut self, high: bool) {
        if self.cs && !high {
            if let Some(p) = self.pending.take() {
                self.program(p);
            }
            self.state = State::Idle;
            self.out = None;
        } else if !self.cs && high {
            self.state = State::Idle;
            self.out = self.status.then_some(true);
        }
        self.cs = high;
    }

    fn program(&mut self, p: Program) {
        if !self.write_enabled {
            self.refused += 1;
            return;
        }
        match p {
            Program::Write(a, d) => self.words[a as usize] = d,
            Program::WriteAll(d) => self.words.fill(d),
            Program::Erase(a) => self.words[a as usize] = 0xFFFF,
            Program::EraseAll => self.words.fill(0xFFFF),
        }
        self.programs += 1;
        self.status = true;
    }

    /// DO now: None while not driven.
    pub fn output(&self) -> Option<bool> {
        if self.cs { self.out } else { None }
    }

    /// One rising clock edge with DI = `di`.
    pub fn clock(&mut self, di: bool) {
        if !self.cs {
            return;
        }
        self.state = match self.state {
            State::Idle if di => {
                self.out = None;
                self.status = false;
                State::Command { bits: 0, n: 0 }
            }
            State::Idle => State::Idle,
            State::Command { bits, n } => {
                let bits = (bits << 1) | di as u32;
                if n + 1 < 12 {
                    State::Command { bits, n: n + 1 }
                } else {
                    self.decode(bits)
                }
            }
            State::Read { addr, word, left } => {
                let bit = (word >> (left - 1)) & 1 != 0;
                self.out = Some(bit);
                if left > 1 {
                    State::Read {
                        addr,
                        word,
                        left: left - 1,
                    }
                } else {
                    // Sequential read: the next word follows without a dummy bit.
                    let next = (addr + 1) % WORDS as u16;
                    self.reads += 1;
                    State::Read {
                        addr: next,
                        word: self.words[next as usize],
                        left: 16,
                    }
                }
            }
            State::Data { addr, bits, n } => {
                let bits = (bits << 1) | di as u16;
                if n + 1 < 16 {
                    State::Data {
                        addr,
                        bits,
                        n: n + 1,
                    }
                } else {
                    self.pending = Some(match addr {
                        Some(a) => Program::Write(a, bits),
                        None => Program::WriteAll(bits),
                    });
                    State::Done
                }
            }
            State::Done => State::Done,
        };
    }

    /// The opcode and address are in: act on them.
    fn decode(&mut self, bits: u32) -> State {
        let addr = (bits & 0x3FF) as u16;
        match bits >> 10 {
            0b10 => {
                // The dummy 0 comes out with the last address bit.
                self.out = Some(false);
                State::Read {
                    addr,
                    word: self.words[addr as usize],
                    left: 16,
                }
            }
            0b01 => State::Data {
                addr: Some(addr),
                bits: 0,
                n: 0,
            },
            0b11 => {
                self.pending = Some(Program::Erase(addr));
                State::Done
            }
            _ => match addr >> 8 {
                0b11 => {
                    self.write_enabled = true;
                    State::Done
                }
                0b00 => {
                    self.write_enabled = false;
                    State::Done
                }
                0b10 => {
                    self.pending = Some(Program::EraseAll);
                    State::Done
                }
                _ => State::Data {
                    addr: None,
                    bits: 0,
                    n: 0,
                },
            },
        }
    }
}

/// What a pin wired to a [`MicrowireBus`] does there.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Line {
    /// The serial clock inversion bit (PLIR) of the channel.
    ClockInversion,
    /// The chip select of `chips[i]`.
    Eeprom(usize),
    /// The chip select of `status[i]`.
    Status(usize),
    /// Sets bit `bit` of the byte `status[chip]` reports, to the pin's level, or to its
    /// opposite if `inverted`.
    StatusBit {
        /// Which status chip.
        chip: usize,
        /// Which bit of its byte.
        bit: u8,
        /// The bit is the opposite of the pin.
        inverted: bool,
    },
}

/// The chips sharing one SCI channel: EEPROMs and status chips ([`StatusChip`]), each
/// with its own chip select, and the channel's serial clock inversion bit.
///
/// Pins are wired by line number: as built by [`MicrowireBus::new`], 0 is the clock
/// inversion (PLIR bit) and 1 + i the chip select of `chips[i]`; [`MicrowireBus::add_line`]
/// adds more.
#[derive(Clone, Debug)]
pub struct MicrowireBus {
    /// The EEPROMs on this channel.
    pub chips: Vec<Eeprom93C86>,
    /// The status chips on it.
    pub status: Vec<StatusChip>,
    /// The level of the data line when no chip drives it: high, pulled up, on the board.
    pub pull: bool,
    lines: Vec<Line>,
    inverted: bool,
}

impl Default for MicrowireBus {
    fn default() -> Self {
        Self::new(Vec::new())
    }
}

impl MicrowireBus {
    /// A bus with these chips.
    pub fn new(chips: Vec<Eeprom93C86>) -> Self {
        let lines = std::iter::once(Line::ClockInversion)
            .chain((0..chips.len()).map(Line::Eeprom))
            .collect();
        Self {
            chips,
            status: Vec::new(),
            pull: true,
            lines,
            inverted: false,
        }
    }

    /// Adds a line and returns its number, to wire a pin to.
    pub fn add_line(&mut self, line: Line) -> usize {
        self.lines.push(line);
        self.lines.len() - 1
    }

    /// The data line: any driving chip pulls it low with a 0; undriven, it is at
    /// [`MicrowireBus::pull`].
    fn line(&self) -> bool {
        let mut driven = self
            .chips
            .iter()
            .filter_map(Eeprom93C86::output)
            .chain(self.status.iter().filter_map(StatusChip::output))
            .peekable();
        if driven.peek().is_none() {
            return self.pull;
        }
        driven.all(|b| b)
    }
}

impl Peer for MicrowireBus {
    fn exchange(&mut self, tx: u8, msb_first: bool) -> u8 {
        let mut rx = 0u8;
        for i in 0..8 {
            let shift = if msb_first { 7 - i } else { i };
            let di = (tx >> shift) & 1 != 0;
            let before = self.line();
            for c in &mut self.chips {
                c.clock(di);
            }
            for s in &mut self.status {
                s.clock();
            }
            let bit = if self.inverted { self.line() } else { before };
            rx |= (bit as u8) << shift;
        }
        rx
    }

    fn data_out(&self) -> bool {
        self.line()
    }

    fn as_any(&self) -> Option<&dyn std::any::Any> {
        Some(self)
    }

    fn as_any_mut(&mut self) -> Option<&mut dyn std::any::Any> {
        Some(self)
    }

    fn pin(&mut self, line: usize, high: bool) {
        match self.lines.get(line) {
            Some(Line::ClockInversion) => self.inverted = high,
            Some(&Line::Eeprom(i)) => self.chips[i].select(high),
            Some(&Line::Status(i)) => self.status[i].select(high),
            Some(&Line::StatusBit {
                chip,
                bit,
                inverted,
            }) => {
                let s = &mut self.status[chip];
                if high != inverted {
                    s.value |= 1 << bit;
                } else {
                    s.value &= !(1 << bit);
                }
            }
            None => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs one CS-framed transaction of `tx` bytes, MSB first, with the clock inverted
    /// from byte `invert_from` on, as the firmware does for the data phase of a read.
    fn transact(bus: &mut MicrowireBus, cs: usize, tx: &[u8], invert_from: usize) -> Vec<u8> {
        bus.pin(cs, true);
        let mut rx = Vec::new();
        for (i, &b) in tx.iter().enumerate() {
            bus.pin(0, i >= invert_from);
            rx.push(bus.exchange(b, true));
        }
        bus.pin(0, false);
        bus.pin(cs, false);
        rx
    }

    fn bus() -> MicrowireBus {
        let a: Vec<u16> = (0..WORDS as u16).map(|i| 0xA000 | i).collect();
        let b: Vec<u16> = (0..WORDS as u16).map(|i| 0xB000 | i).collect();
        MicrowireBus::new(vec![Eeprom93C86::new(&a), Eeprom93C86::new(&b)])
    }

    #[test]
    fn a_read_as_the_firmware_does_it() {
        let mut bus = bus();
        // 000 1 10 A9-A0 = 0x18 0x05 + 0x3 << 8: address 0x305.
        let rx = transact(&mut bus, 1, &[0x1B, 0x05, 0x00, 0x00], 2);
        assert_eq!(&rx[2..], &[0xA3, 0x05], "word 0x305 of the first chip");
        let rx = transact(&mut bus, 2, &[0x18, 0x07, 0x00, 0x00], 2);
        assert_eq!(&rx[2..], &[0xB0, 0x07], "the second chip on its own select");
        assert_eq!(rx[1] & 1, 1, "DO not driven yet, seen without inversion");
        let rx = transact(&mut bus, 2, &[0x18, 0x07, 0x00, 0x00], 1);
        assert_eq!(
            rx[1] & 1,
            0,
            "the dummy 0 comes out with the last address bit"
        );
    }

    #[test]
    fn without_the_clock_inversion_the_data_is_one_bit_late() {
        let mut bus = bus();
        let rx = transact(&mut bus, 1, &[0x18, 0x05, 0x00, 0x00], 4);
        // Seen one bit late: the dummy 0, then D15-D1.
        assert_eq!(u16::from_be_bytes([rx[2], rx[3]]), 0xA005 >> 1);
    }

    #[test]
    fn sequential_read_and_wraparound() {
        let mut bus = bus();
        let rx = transact(&mut bus, 1, &[0x1B, 0xFF, 0, 0, 0, 0], 2);
        assert_eq!(&rx[2..], &[0xA3, 0xFF, 0xA0, 0x00]);
        assert_eq!(bus.chips[0].reads, 2);
    }

    #[test]
    fn writing_needs_ewen_and_happens_when_cs_goes_low() {
        let mut bus = bus();
        // WRITE 0x002 = 0x1234: 000 1 01 0000000010 + data.
        transact(&mut bus, 1, &[0x14, 0x02, 0x12, 0x34], 9);
        assert_eq!(
            bus.chips[0].words[2], 0xA002,
            "refused: writing disabled at power-on"
        );
        assert_eq!(bus.chips[0].refused, 1);
        transact(&mut bus, 1, &[0x13, 0x00], 9); // EWEN
        transact(&mut bus, 1, &[0x14, 0x02, 0x12, 0x34], 9);
        assert_eq!(bus.chips[0].words[2], 0x1234);
        assert_eq!(
            bus.chips[1].words[2], 0xB002,
            "the other chip was not selected"
        );
        // ERASE 0x003; then EWDS and a write that must not happen.
        transact(&mut bus, 1, &[0x1C, 0x03], 9);
        assert_eq!(bus.chips[0].words[3], 0xFFFF);
        transact(&mut bus, 1, &[0x10, 0x00], 9); // EWDS
        transact(&mut bus, 1, &[0x14, 0x04, 0x00, 0x00], 9);
        assert_eq!(bus.chips[0].words[4], 0xA004);
        assert_eq!(bus.chips[0].programs, 2);
    }

    #[test]
    fn after_programming_do_shows_ready() {
        let mut bus = bus();
        transact(&mut bus, 1, &[0x13, 0x00], 9);
        transact(&mut bus, 1, &[0x14, 0x02, 0x12, 0x34], 9);
        bus.pin(1, true);
        assert_eq!(bus.chips[0].output(), Some(true), "ready");
        bus.pin(1, false);
        assert_eq!(bus.chips[0].output(), None);
    }

    #[test]
    fn eral_and_wral() {
        let mut bus = bus();
        transact(&mut bus, 2, &[0x13, 0x00], 9); // EWEN
        transact(&mut bus, 2, &[0x11, 0x00, 0x5A, 0xA5], 9); // WRAL 0x5AA5
        assert!(bus.chips[1].words.iter().all(|&w| w == 0x5AA5));
        transact(&mut bus, 2, &[0x12, 0x00], 9); // ERAL
        assert!(bus.chips[1].words.iter().all(|&w| w == 0xFFFF));
        assert_eq!(bus.chips[0].words[0], 0xA000);
    }

    #[test]
    fn bytes_round_trip_in_both_orders() {
        let img: Vec<u8> = (0..2048u32).map(|i| (i * 7) as u8).collect();
        for be in [true, false] {
            let c = Eeprom93C86::from_bytes(&img, be);
            assert_eq!(c.to_bytes(be), img);
        }
        assert_eq!(
            Eeprom93C86::from_bytes(&[0x12, 0x34], true).words[0],
            0x1234
        );
        assert_eq!(
            Eeprom93C86::from_bytes(&[0x12, 0x34], false).words[0],
            0x3412
        );
    }
}
