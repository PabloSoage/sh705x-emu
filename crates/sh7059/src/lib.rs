//! The Renesas SH7059 microcontroller around an SH-2E core.
//!
//! Address map in on-chip ROM enabled mode (manual REJ09B0177-0500, table 9.4):
//!
//! | Range | Contents |
//! |---|---|
//! | `0x0000_0000..0x0018_0000` | on-chip flash ROM, 1.5 MB |
//! | `0x0020_0000..0x0100_0000` | external CS0–CS3 spaces (not fitted on the ECUs seen so far) |
//! | `0xFFFE_8000..0xFFFF_C000` | on-chip RAM, 80 KB |
//! | `0xFFFF_C000..=0xFFFF_FFFF` | on-chip peripheral registers, 16 KB |
//!
//! Table 9.4 prints the RAM start as H'FFFF8000, which would be 16 KB, not the 80 KB it
//! states; the same table reserves everything up to H'FFFE7FFF, and the flash chapter
//! gives H'FFFE8000–H'FFFFBFFF. That is the range used here.

use sh2e::Bus;
use std::fmt;

/// Start of the on-chip flash ROM.
pub const ROM_BASE: u32 = 0x0000_0000;
/// Size of the on-chip flash ROM: 1.5 MB.
pub const ROM_SIZE: usize = 0x18_0000;
/// Start of the on-chip RAM.
pub const RAM_BASE: u32 = 0xFFFE_8000;
/// Size of the on-chip RAM: 80 KB.
pub const RAM_SIZE: usize = 0x1_4000;
/// Start of the on-chip peripheral register area.
pub const IO_BASE: u32 = 0xFFFF_C000;
/// Size of the on-chip peripheral register area: 16 KB.
pub const IO_SIZE: usize = 0x4000;

/// A flash image that cannot be loaded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImageError {
    /// The image is not the size of the SH7059's flash.
    WrongSize {
        /// Bytes in the image given.
        actual: usize,
    },
}

impl fmt::Display for ImageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ImageError::WrongSize { actual } => write!(
                f,
                "flash image is {actual} bytes; an SH7059 flash is {ROM_SIZE} bytes (1.5 MB)"
            ),
        }
    }
}

impl std::error::Error for ImageError {}

/// The region an address falls in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Region {
    /// On-chip flash ROM, with the offset into it.
    Rom(usize),
    /// On-chip RAM, with the offset into it.
    Ram(usize),
    /// On-chip peripheral registers, with the offset from [`IO_BASE`].
    Io(usize),
    /// External or reserved space: nothing answers.
    Unmapped,
}

/// Which region holds `addr`.
pub fn region(addr: u32) -> Region {
    if addr < ROM_BASE + ROM_SIZE as u32 {
        Region::Rom((addr - ROM_BASE) as usize)
    } else if (RAM_BASE..IO_BASE).contains(&addr) {
        Region::Ram((addr - RAM_BASE) as usize)
    } else if addr >= IO_BASE {
        Region::Io((addr - IO_BASE) as usize)
    } else {
        Region::Unmapped
    }
}

/// The chip: flash, RAM and the peripheral register area.
///
/// Peripheral registers are plain storage for now: they read back what was written.
/// Each peripheral model replaces its part of that area as it is written.
pub struct Sh7059 {
    rom: Box<[u8]>,
    ram: Box<[u8]>,
    io: Box<[u8]>,
    /// Accesses to unmapped space since power-on: reads return 0, writes are dropped.
    pub unmapped_accesses: u64,
    /// Writes to the flash ROM since power-on, which are dropped. The firmware only
    /// writes flash through the flash controller's programming sequence.
    pub rom_writes: u64,
}

impl Sh7059 {
    /// A chip with the given flash image; RAM and registers start at zero.
    pub fn new(flash: &[u8]) -> Result<Self, ImageError> {
        if flash.len() != ROM_SIZE {
            return Err(ImageError::WrongSize {
                actual: flash.len(),
            });
        }
        Ok(Self {
            rom: flash.into(),
            ram: vec![0; RAM_SIZE].into_boxed_slice(),
            io: vec![0; IO_SIZE].into_boxed_slice(),
            unmapped_accesses: 0,
            rom_writes: 0,
        })
    }

    /// The flash image as loaded.
    pub fn rom(&self) -> &[u8] {
        &self.rom
    }

    fn read<const N: usize>(&mut self, addr: u32) -> [u8; N] {
        let mut out = [0; N];
        match region(addr) {
            Region::Rom(o) => out.copy_from_slice(&self.rom[o..o + N]),
            Region::Ram(o) => out.copy_from_slice(&self.ram[o..o + N]),
            Region::Io(o) => out.copy_from_slice(&self.io[o..o + N]),
            Region::Unmapped => self.unmapped_accesses += 1,
        }
        out
    }

    fn write<const N: usize>(&mut self, addr: u32, bytes: [u8; N]) {
        match region(addr) {
            Region::Rom(_) => self.rom_writes += 1,
            Region::Ram(o) => self.ram[o..o + N].copy_from_slice(&bytes),
            Region::Io(o) => self.io[o..o + N].copy_from_slice(&bytes),
            Region::Unmapped => self.unmapped_accesses += 1,
        }
    }
}

impl Bus for Sh7059 {
    fn read8(&mut self, addr: u32) -> u8 {
        self.read::<1>(addr)[0]
    }
    fn read16(&mut self, addr: u32) -> u16 {
        u16::from_be_bytes(self.read(addr))
    }
    fn read32(&mut self, addr: u32) -> u32 {
        u32::from_be_bytes(self.read(addr))
    }
    fn write8(&mut self, addr: u32, value: u8) {
        self.write(addr, [value]);
    }
    fn write16(&mut self, addr: u32, value: u16) {
        self.write(addr, value.to_be_bytes());
    }
    fn write32(&mut self, addr: u32, value: u32) {
        self.write(addr, value.to_be_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chip() -> Sh7059 {
        let mut flash = vec![0u8; ROM_SIZE];
        flash[0..4].copy_from_slice(&0x0000_1000u32.to_be_bytes());
        flash[ROM_SIZE - 2..].copy_from_slice(&[0xAB, 0xCD]);
        Sh7059::new(&flash).unwrap()
    }

    #[test]
    fn rejects_an_image_of_the_wrong_size() {
        let err = Sh7059::new(&[0; 0x10_0000]).err().unwrap();
        assert_eq!(err, ImageError::WrongSize { actual: 0x10_0000 });
    }

    #[test]
    fn region_boundaries() {
        assert_eq!(region(0), Region::Rom(0));
        assert_eq!(region(0x0017_FFFF), Region::Rom(ROM_SIZE - 1));
        assert_eq!(region(0x0018_0000), Region::Unmapped);
        assert_eq!(region(0xFFFE_7FFF), Region::Unmapped);
        assert_eq!(region(0xFFFE_8000), Region::Ram(0));
        assert_eq!(region(0xFFFF_BFFF), Region::Ram(RAM_SIZE - 1));
        assert_eq!(region(0xFFFF_C000), Region::Io(0));
        assert_eq!(region(0xFFFF_FFFF), Region::Io(IO_SIZE - 1));
    }

    #[test]
    fn rom_reads_big_endian_and_ignores_writes() {
        let mut c = chip();
        assert_eq!(c.read32(0), 0x0000_1000);
        assert_eq!(c.read16(0x0017_FFFE), 0xABCD);
        c.write32(0, 0xFFFF_FFFF);
        assert_eq!(c.read32(0), 0x0000_1000);
        assert_eq!(c.rom_writes, 1);
    }

    #[test]
    fn ram_and_io_read_back_what_was_written() {
        let mut c = chip();
        c.write32(0xFFFF_BFFC, 0x1122_3344);
        assert_eq!(c.read8(0xFFFF_BFFC), 0x11);
        assert_eq!(c.read16(0xFFFF_BFFE), 0x3344);
        c.write16(0xFFFF_ED18, 0x8000);
        assert_eq!(c.read16(0xFFFF_ED18), 0x8000);
    }

    #[test]
    fn unmapped_space_reads_zero_and_is_counted() {
        let mut c = chip();
        c.write32(0x0020_0000, 1);
        assert_eq!(c.read32(0x0020_0000), 0);
        assert_eq!(c.unmapped_accesses, 2);
    }

    #[test]
    fn the_cpu_resets_from_the_flash_vectors() {
        let mut c = chip();
        let mut cpu = sh2e::Cpu::new();
        cpu.power_on_reset(&mut c);
        assert_eq!(cpu.pc, 0x0000_1000);
    }
}
