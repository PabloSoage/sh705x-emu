//! The status chips on the clocked serial channels: 8-bit shift registers that the
//! firmware reads to check its output stages, sharing the channel with the EEPROMs.
//!
//! Found in the Astra's firmware (resultados fase 9):
//! - **SCI3, chip selects PJ1 and PJ0**: one byte each, read every task tick (`0xB82A`)
//!   into `FFFEB742` and `FFFEB743`. Bit 6 of the first is the throttle driver: the
//!   firmware turns it off for eight ticks after power-on (PA11 high) and expects to see
//!   the bit go low and come back (P060E) [MED].
//!   Bit 1 of the second (PJ0) is the main relay, checked against P0686/P0687 (`0x66D0E`)
//!   [MED: read in the code].
//! - **SCI4, chip selects PD5 and PD6**: one fault bit per output stage (`0x126D4`), read
//!   most significant bit first, the first of them off the receive line (PB11) before the
//!   clock runs, so bit k of the byte a chip sends lands in bit k - 1 of the firmware's.
//!   A set bit is a fault, and each has a pair of circuit codes (`0x1258C` and its
//!   callers). In the bits a chip sends:
//!
//!   | Chip | Bit | Output stage | Codes |
//!   |---|---|---|---|
//!   | PD5 | `0x04` | fan 1 | P0691/P0692 |
//!   | PD5 | `0x08` | fan 3 | P0695/P0696 |
//!   | PD5 | `0x10` | EGR cooler bypass | P245C/P245D [MED: sets P245C] |
//!   | PD5 | `0x40` | glow plug module | P0383/P0384 |
//!   | PD6 | `0x04` | A/C clutch relay | P0646/P0647 |
//!   | PD6 | `0x08` | fuel heater | P2688/P2689 |
//!   | PD6 | `0x40` | fan 2 | P0693/P0694 |
//!   | PD6 | `0x80` | throttle motor | P2102/P2103 [MED: sets P2103] |
//!
//!   The six not measured set nothing in 8 s idling (glow plugs with a cold engine, fan 1
//!   with a hot one and the A/C also for 20 s): their checks want something not modelled
//!   [SUP]. The other bits have no reader.
//!
//! What the chips are is not known: the model is the least that gives the firmware what
//! it reads [SUP].

/// One status chip: loads its byte when selected and shifts it out, one bit a clock, for
/// eight clocks; then it lets go of the data line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatusChip {
    /// The byte it reports.
    pub value: u8,
    /// Shifts the most significant bit out first.
    pub msb_first: bool,
    /// Its chip select is active low.
    pub active_low: bool,
    selected: bool,
    reg: u8,
    left: u8,
    /// Times it was selected.
    pub reads: u64,
}

impl StatusChip {
    /// A chip reporting `value`.
    pub fn new(value: u8, msb_first: bool, active_low: bool) -> Self {
        Self {
            value,
            msb_first,
            active_low,
            selected: false,
            reg: 0,
            left: 0,
            reads: 0,
        }
    }

    /// Its chip select pin changed level.
    pub fn select(&mut self, high: bool) {
        let on = high != self.active_low;
        if on && !self.selected {
            self.reg = self.value;
            self.left = 8;
            self.reads += 1;
        }
        if !on {
            self.left = 0;
        }
        self.selected = on;
    }

    /// The bit it drives on the data line, if it drives it.
    pub fn output(&self) -> Option<bool> {
        let bit = if self.msb_first {
            self.reg & 0x80 != 0
        } else {
            self.reg & 1 != 0
        };
        (self.left > 0).then_some(bit)
    }

    /// A clock edge: the next bit comes out.
    pub fn clock(&mut self) {
        if self.left > 0 {
            self.reg = if self.msb_first {
                self.reg << 1
            } else {
                self.reg >> 1
            };
            self.left -= 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shifts_its_byte_once_per_selection() {
        let mut c = StatusChip::new(0b1000_0001, false, true);
        assert_eq!(c.output(), None, "not selected");
        c.select(false);
        let mut bits = Vec::new();
        for _ in 0..9 {
            bits.push(c.output());
            c.clock();
        }
        let one = Some(true);
        let zero = Some(false);
        assert_eq!(bits, [one, zero, zero, zero, zero, zero, zero, one, None]);
        // Staying selected does not load it again; a new selection does.
        c.select(false);
        assert_eq!(c.output(), None);
        c.select(true);
        c.select(false);
        assert_eq!((c.output(), c.reads), (one, 2));
    }

    #[test]
    fn most_significant_bit_first() {
        let mut c = StatusChip::new(0b0100_0000, true, false);
        c.select(true);
        assert_eq!(c.output(), Some(false));
        c.clock();
        assert_eq!(c.output(), Some(true));
    }
}
