//! The ECU board around the microcontroller: the serial EEPROM, the other chips on the
//! synchronous serial ports, and which pin carries which sensor or actuator signal.
//!
//! The EEPROM is four 93C86 Microwire chips on the synchronous serial ports
//! ([`microwire`]); [`wire_eeproms`] connects them as on the Denso board of the Astra J.
//!
//! The status chips that watch the output stages share SCI3 and SCI4 ([`status`],
//! [`wire_status`]). On SCI0 sits the injector driver [MED]: the firmware's module for it
//! (`0x61xxx`) is the one that reports P0201-P0204, P2147/P2148 and P0611. It writes four
//! registers from the calibration (`0x113AC`: register number on PK3-PK4, the byte on
//! SCI0), at power-on and again when its state changes, and reads back one byte: bit 0
//! (even registers) or 1 (odd) is a fault, heeded while an analog reading (`FFFEB874`) is
//! at 340 codes or more (`0x61C0C`). Nothing
//! modelled answers 0, no fault; it matters once there is injection.

pub mod microwire;
pub mod status;

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
/// first. The order of the last two is checked [MED]: a record the firmware loads at boot
/// (`FFFF68AC`, behind DID 0x1234) is a float and its complement, `43D40000 BC2BFFFF`
/// (424.0), at offset 0x2AC of block 3, with the same record also at 0x620 of block 0.
/// With blocks 2 and 3 swapped it finds zeros there, not a value and its complement, and
/// loads 0 (the check itself not located). The byte order only makes it read 424.0 rather than
/// a meaningless float [DER].
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

/// Port A data register: PA11 high turns the throttle driver off (the firmware's
/// power-on test of it, `0x31566`) [MED].
pub const PADR: u32 = 0xFFFF_F726;
/// Port B data register: PB11 reads SCI4's receive line [MED].
pub const PBDR: u32 = 0xFFFF_F738;
/// Port B I/O register.
pub const PBIOR: u32 = 0xFFFF_F730;
/// Port J I/O register.
pub const PJIOR: u32 = 0xFFFF_F766;

/// Puts the status chips ([`status`]) on SCI3 and SCI4, beside any EEPROMs already
/// there, and the board's other inputs the firmware checks its outputs with:
/// - SCI3: the chip on PJ1 reports the throttle driver on in bit 6 while PA11 is low;
///   every other bit, and the chip on PJ0, read 0 [SUP];
/// - SCI4: the chips on PD5 and PD6 report no fault (0), the first bit on PB11;
/// - PJ13 and PJ15 are high: two output stages' fault lines, low on a fault (P1407 and
///   P0403) [MED].
///
/// On a channel with no EEPROM the data line reads low when no chip drives it, as it
/// did with nothing there ([`sh7059::sci::Fixed`]`(0)`).
pub fn wire_status(chip: &mut sh7059::Sh7059) {
    use microwire::Line;
    use sh7059::{InputPin, Level, Wire};
    use status::StatusChip;
    // (channel, msb first, [(chip select register, bit)], [(chip, bit, register, bit)])
    type Chips = &'static [(u32, u8)];
    type Bits = &'static [(usize, u8, u32, u8)];
    let plan: [(usize, bool, Chips, Bits); 2] = [
        (3, false, &[(PJDR, 1), (PJDR, 0)], &[(0, 6, PADR, 11)]),
        (4, true, &[(PDDR, 5), (PDDR, 6)], &[]),
    ];
    for (sci, msb_first, selects, bits) in plan {
        let fresh = chip.sci.ch[sci]
            .peer
            .as_any()
            .is_none_or(|a| !a.is::<microwire::MicrowireBus>());
        if fresh {
            let mut bus = microwire::MicrowireBus::new(Vec::new());
            bus.pull = false;
            chip.sci.ch[sci].peer = Box::new(bus);
        }
        let bus = chip.sci.ch[sci]
            .peer
            .as_any_mut()
            .and_then(|a| a.downcast_mut::<microwire::MicrowireBus>())
            .expect("a bus");
        let mut wires = Vec::new();
        for &(reg, bit) in selects {
            bus.status.push(StatusChip::new(0, msb_first, true));
            let line = bus.add_line(Line::Status(bus.status.len() - 1));
            wires.push((reg, bit, line));
        }
        for &(k, b, reg, bit) in bits {
            let line = bus.add_line(Line::StatusBit {
                chip: k,
                bit: b,
                inverted: true,
            });
            wires.push((reg, bit, line));
        }
        if fresh {
            // PL8IR, PL9IR invert SCK3, SCK4.
            wires.push((PLIR, 5 + sci as u8, 0));
        }
        for (reg, bit, line) in wires {
            chip.wire(Wire {
                reg,
                bit,
                sci,
                line,
            });
        }
    }
    for (dr, ior, bit, level) in [
        (PJDR, PJIOR, 13, Level::Fixed(true)),
        (PJDR, PJIOR, 15, Level::Fixed(true)),
        (PBDR, PBIOR, 11, Level::SciData(4)),
    ] {
        chip.inputs.push(InputPin {
            dr,
            ior,
            bit,
            level,
        });
    }
}

/// Sets the byte status chip `index` reports on channel `sci` (in [`wire_status`] order:
/// SCI3 PJ1, PJ0; SCI4 PD5, PD6). On SCI4 a set bit is an output stage's fault.
pub fn set_status(
    chip: &mut sh7059::Sh7059,
    sci: usize,
    index: usize,
    value: u8,
) -> Result<(), String> {
    chip.sci
        .ch
        .get_mut(sci)
        .and_then(|c| c.peer.as_any_mut())
        .and_then(|a| a.downcast_mut::<microwire::MicrowireBus>())
        .and_then(|bus| bus.status.get_mut(index))
        .map(|s| s.value = value)
        .ok_or_else(|| format!("no status chip {index} on SCI{sci}"))
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

    #[test]
    fn the_throttle_status_bit_follows_pa11() {
        use sh2e::Bus;
        let mut chip = sh7059::Sh7059::new(&vec![0; sh7059::ROM_SIZE]).unwrap();
        wire_status(&mut chip);
        // Read the chip on PJ1 (SCI3) as the firmware does: select it alone, one byte.
        let read = |chip: &mut sh7059::Sh7059, pa11: bool| {
            chip.write16(PJDR, 0x0003);
            chip.write16(PADR, if pa11 { 0x0800 } else { 0 });
            chip.write16(PJDR, 0x0001);
            chip.sci.ch[3].peer.exchange(0xFF, false)
        };
        assert_eq!(read(&mut chip, false), 0x40, "driver on");
        assert_eq!(
            read(&mut chip, true),
            0x00,
            "driver off for the power-on test"
        );
        assert_eq!(read(&mut chip, false), 0x40);
        // The fault lines read high (no fault) while the firmware has them as inputs.
        assert_eq!(chip.read16(PJDR) & 0xA000, 0xA000);
    }

    #[test]
    fn a_status_chip_reports_the_byte_it_is_set_to() {
        let mut chip = sh7059::Sh7059::new(&vec![0; sh7059::ROM_SIZE]).unwrap();
        wire_status(&mut chip);
        set_status(&mut chip, 3, 1, 0x5A).unwrap();
        // The chip on PJ0 alone.
        sh2e::Bus::write16(&mut chip, PJDR, 0x0003);
        sh2e::Bus::write16(&mut chip, PJDR, 0x0002);
        assert_eq!(chip.sci.ch[3].peer.exchange(0xFF, false), 0x5A);
        assert!(
            set_status(&mut chip, 4, 2, 0xFF).is_err(),
            "two chips on SCI4"
        );
        assert!(set_status(&mut chip, 0, 0, 0xFF).is_err(), "none on SCI0");
    }
}
