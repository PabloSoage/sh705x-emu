//! The ECU board around the microcontroller: the serial EEPROM, the other chips on the
//! synchronous serial ports, and which pin carries which sensor or actuator signal.
//!
//! The EEPROM is four 93C86 Microwire chips on the synchronous serial ports
//! ([`microwire`]); [`wire_eeproms`] connects them as on the Denso board of the Astra J.

pub mod microwire;

use std::fmt;

/// Largest EEPROM image accepted: 64 KB, well above the serial EEPROMs fitted to these ECUs.
pub const EEPROM_MAX: usize = 0x1_0000;

/// An EEPROM image that cannot be used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EepromError {
    /// The image is empty, not a power of two, or larger than [`EEPROM_MAX`].
    BadSize {
        /// Bytes in the image given.
        actual: usize,
    },
}

impl fmt::Display for EepromError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EepromError::BadSize { actual } => write!(
                f,
                "EEPROM image is {actual} bytes; expected a power of two up to {EEPROM_MAX}"
            ),
        }
    }
}

impl std::error::Error for EepromError {}

/// The contents of the board's serial EEPROM.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Eeprom {
    data: Vec<u8>,
}

impl Eeprom {
    /// An EEPROM holding `image`. Serial EEPROMs come in power-of-two sizes.
    pub fn new(image: &[u8]) -> Result<Self, EepromError> {
        let n = image.len();
        if n == 0 || !n.is_power_of_two() || n > EEPROM_MAX {
            return Err(EepromError::BadSize { actual: n });
        }
        Ok(Self {
            data: image.to_vec(),
        })
    }

    /// An EEPROM of `size` bytes in the erased state (all 0xFF), for running without a dump.
    pub fn erased(size: usize) -> Result<Self, EepromError> {
        Self::new(&vec![0xFF; size])
    }

    /// Size in bytes.
    pub fn len(&self) -> usize {
        self.data.len()
    }

    /// Always false: an EEPROM has at least one byte.
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    /// The contents.
    pub fn data(&self) -> &[u8] {
        &self.data
    }
}

/// Port J data register: PJ3 and PJ4 select two of the EEPROMs.
pub const PJDR: u32 = 0xFFFF_F76C;
/// Port D data register: PD0 and PD1 select the other two.
pub const PDDR: u32 = 0xFFFF_F746;
/// Port L invert register: PL7IR, PL8IR and PL9IR invert SCK2, SCK3 and SCK4.
pub const PLIR: u32 = 0xFFFF_F75C;

/// One of the board's EEPROM chips: where it is connected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EepromChip {
    /// SCI channel.
    pub sci: usize,
    /// Port data register of its chip select.
    pub cs_reg: u32,
    /// Bit of its chip select in that register.
    pub cs_bit: u8,
}

/// The four 93C86 of the Astra J's Denso board, as the firmware drives them (traced in
/// the emulator): two pairs read side by side, (SCI3, SCI4) and (SCI4, SCI2).
pub const ASTRA_EEPROMS: [EepromChip; 4] = [
    EepromChip {
        sci: 3,
        cs_reg: PJDR,
        cs_bit: 3,
    },
    EepromChip {
        sci: 4,
        cs_reg: PJDR,
        cs_bit: 4,
    },
    EepromChip {
        sci: 4,
        cs_reg: PDDR,
        cs_bit: 0,
    },
    EepromChip {
        sci: 2,
        cs_reg: PDDR,
        cs_bit: 1,
    },
];

/// How a dump of all the chips is laid out: which 2 KB block holds each chip of
/// [`ASTRA_EEPROMS`], and the byte order of the words.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Layout {
    /// Block of the dump for each chip, in the order of [`ASTRA_EEPROMS`].
    pub blocks: [usize; 4],
    /// High byte first.
    pub big_endian: bool,
}

/// The layout used unless told otherwise: chips in [`ASTRA_EEPROMS`] order, high byte
/// first. Not checked against the real board yet [SUP].
pub const DEFAULT_LAYOUT: Layout = Layout {
    blocks: [0, 1, 2, 3],
    big_endian: true,
};

/// Bytes per chip.
pub const CHIP_BYTES: usize = 2 * microwire::WORDS;

/// Puts the EEPROMs on the chip's SCI channels, loaded from `image` (four 2 KB blocks)
/// as `layout` says, with their chip selects and serial clock inversion bits wired.
/// Channels with no EEPROM keep their peer.
pub fn wire_eeproms(chip: &mut sh7059::Sh7059, image: &Eeprom, layout: Layout) {
    assert_eq!(image.len(), 4 * CHIP_BYTES, "four 93C86: 8 KB");
    for sci in 0..5 {
        let on: Vec<(usize, &EepromChip)> = ASTRA_EEPROMS
            .iter()
            .enumerate()
            .filter(|(_, c)| c.sci == sci)
            .collect();
        if on.is_empty() {
            continue;
        }
        let chips = on
            .iter()
            .map(|&(i, _)| {
                let b = layout.blocks[i] * CHIP_BYTES;
                microwire::Eeprom93C86::from_bytes(
                    &image.data()[b..b + CHIP_BYTES],
                    layout.big_endian,
                )
            })
            .collect();
        chip.sci.ch[sci].peer = Box::new(microwire::MicrowireBus::new(chips));
        chip.wire(sh7059::Wire {
            reg: PLIR,
            // PL7IR, PL8IR, PL9IR for SCK2, SCK3, SCK4.
            bit: 5 + sci as u8,
            sci,
            line: 0,
        });
        for (line, &(_, c)) in on.iter().enumerate() {
            chip.wire(sh7059::Wire {
                reg: c.cs_reg,
                bit: c.cs_bit,
                sci,
                line: 1 + line,
            });
        }
    }
}

/// The EEPROM contents as they are now in the chip's peers, laid out as `layout`; None
/// if [`wire_eeproms`] was not used.
pub fn eeprom_image(chip: &sh7059::Sh7059, layout: Layout) -> Option<Vec<u8>> {
    let mut out = vec![0u8; 4 * CHIP_BYTES];
    let mut seen = [0usize; 5];
    for (i, c) in ASTRA_EEPROMS.iter().enumerate() {
        let bus = chip.sci.ch[c.sci]
            .peer
            .as_any()?
            .downcast_ref::<microwire::MicrowireBus>()?;
        let e = bus.chips.get(seen[c.sci])?;
        seen[c.sci] += 1;
        let b = layout.blocks[i] * CHIP_BYTES;
        out[b..b + CHIP_BYTES].copy_from_slice(&e.to_bytes(layout.big_endian));
    }
    Some(out)
}

/// Words read, programming cycles and refused programming instructions of each chip, in
/// [`ASTRA_EEPROMS`] order; empty if [`wire_eeproms`] was not used.
pub fn eeprom_stats(chip: &sh7059::Sh7059) -> Vec<(u64, u64, u64)> {
    let mut out = Vec::new();
    let mut seen = [0usize; 5];
    for c in &ASTRA_EEPROMS {
        let Some(bus) = chip.sci.ch[c.sci]
            .peer
            .as_any()
            .and_then(|a| a.downcast_ref::<microwire::MicrowireBus>())
        else {
            return Vec::new();
        };
        if let Some(e) = bus.chips.get(seen[c.sci]) {
            out.push((e.reads, e.programs, e.refused));
        }
        seen[c.sci] += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_power_of_two_sizes() {
        assert_eq!(Eeprom::new(&[0; 8192]).unwrap().len(), 8192);
        assert_eq!(Eeprom::erased(512).unwrap().data()[0], 0xFF);
    }

    #[test]
    fn rejects_other_sizes() {
        for n in [0, 8191, 0x2_0000] {
            assert_eq!(
                Eeprom::new(&vec![0; n]),
                Err(EepromError::BadSize { actual: n })
            );
        }
    }
}
