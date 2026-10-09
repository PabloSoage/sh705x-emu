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

pub mod adc;
pub mod cmt;
pub mod intc;
mod machine;
pub mod sci;

pub use machine::Machine;
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

/// H-UDI instruction register SDIR; the status register SDSR follows it.
pub const HUDI_SDIR: u32 = 0xFFFF_F7C0;

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
#[inline(always)]
pub fn region(addr: u32) -> Region {
    // RAM first: most data accesses go there.
    if (RAM_BASE..IO_BASE).contains(&addr) {
        Region::Ram((addr - RAM_BASE) as usize)
    } else if addr < ROM_BASE + ROM_SIZE as u32 {
        Region::Rom((addr - ROM_BASE) as usize)
    } else if addr >= IO_BASE {
        Region::Io((addr - IO_BASE) as usize)
    } else {
        Region::Unmapped
    }
}

/// One access to the peripheral register area, for tracing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IoAccess {
    /// Address of the instruction that made it (of the branch, for a delay slot), as set
    /// in [`Sh7059::trace_pc`].
    pub pc: u32,
    /// A write, or a read.
    pub write: bool,
    /// Register address.
    pub addr: u32,
    /// Access size in bytes: 1, 2 or 4.
    pub size: u8,
    /// Value read or written.
    pub value: u32,
}

/// An access of `n` bytes that would run past the end of its region is unmapped. Only a
/// misaligned access can do that, and the CPU raises an address error before making one
/// unless that is turned off.
#[inline(always)]
fn fit(r: Region, n: usize) -> Region {
    match r {
        Region::Rom(o) if o + n > ROM_SIZE => Region::Unmapped,
        Region::Ram(o) if o + n > RAM_SIZE => Region::Unmapped,
        Region::Io(o) if o + n > IO_SIZE => Region::Unmapped,
        r => r,
    }
}

/// The modelled peripherals, by register block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Periph {
    Cmt,
    Sci,
    Adc,
}

impl Periph {
    /// Offset of the block in the register storage, and its length.
    fn span(self) -> (usize, usize) {
        let (b, e) = match self {
            Periph::Cmt => (cmt::BASE, cmt::END),
            Periph::Sci => (sci::BASE, sci::END),
            Periph::Adc => (adc::BASE, adc::END),
        };
        ((b - IO_BASE) as usize, (e - b) as usize)
    }
}

#[inline(always)]
fn periph_at(addr: u32) -> Option<Periph> {
    if (sci::BASE..sci::END).contains(&addr) {
        Some(Periph::Sci)
    } else if (cmt::BASE..cmt::END).contains(&addr) {
        Some(Periph::Cmt)
    } else if (adc::BASE..adc::END).contains(&addr) {
        Some(Periph::Adc)
    } else {
        None
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
    /// Bits set in every read of each register byte, as the phase 0 tool forced them to
    /// get past status polls. All zero by default.
    io_force: Box<[u8]>,
    /// Accesses to the register area, when tracing is on.
    pub trace: Option<Vec<IoAccess>>,
    /// The instruction address recorded with each traced access; the caller sets it
    /// before every step.
    pub trace_pc: u32,
    /// Accesses to unmapped space since power-on: reads return 0, writes are dropped.
    pub unmapped_accesses: u64,
    /// Writes to the flash ROM since power-on, which are dropped. The firmware only
    /// writes flash through the flash controller's programming sequence.
    pub rom_writes: u64,
    /// The current CPU cycle, which timers read; the machine sets it before each step.
    pub now: u64,
    /// Compare match timer.
    pub cmt: cmt::Cmt,
    /// Serial communication interface.
    pub sci: sci::Sci,
    /// A/D converter.
    pub adc: adc::Adc,
    /// Highest pending interrupt: (level, vector), or (0, 0) for none.
    pub pending: (u32, u32),
    /// CPU cycle of the next timer event.
    pub next_event: u64,
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
            io_force: vec![0; IO_SIZE].into_boxed_slice(),
            trace: None,
            trace_pc: 0,
            unmapped_accesses: 0,
            rom_writes: 0,
            now: 0,
            cmt: cmt::Cmt::default(),
            sci: sci::Sci::default(),
            adc: adc::Adc::default(),
            pending: (0, 0),
            next_event: u64::MAX,
        })
        .map(|mut chip| {
            // ICR.NMIL reads the NMI pin; the boot loader waits for it to be high. How the
            // board drives the pin is not known: it is taken as high [SUP].
            chip.io_force[(intc::ICR - IO_BASE) as usize] |= 0x80;
            chip.reset_registers();
            chip
        })
    }

    /// The flash image as loaded.
    pub fn rom(&self) -> &[u8] {
        &self.rom
    }

    /// Sets `mask` in every later read of the register byte at `addr`, which must be in
    /// the peripheral area. This reproduces the phase 0 tool, which forced status bits to
    /// get past polls on peripherals that are not modelled yet.
    pub fn force_bits(&mut self, addr: u32, mask: u8) {
        match region(addr) {
            Region::Io(o) => self.io_force[o] |= mask,
            _ => panic!("force_bits: {addr:#010X} is not a peripheral register"),
        }
    }

    fn read<const N: usize>(&mut self, addr: u32) -> [u8; N] {
        let mut out = [0; N];
        match fit(region(addr), N) {
            Region::Rom(o) => out.copy_from_slice(&self.rom[o..o + N]),
            Region::Ram(o) => out.copy_from_slice(&self.ram[o..o + N]),
            Region::Io(o) => {
                match periph_at(addr) {
                    Some(Periph::Sci) => {
                        let (b, len) = Periph::Sci.span();
                        let k = ((addr - sci::BASE) / 8) as usize;
                        self.sci.store_channel(k, &mut self.io[b..b + len]);
                    }
                    Some(p) => self.store_regs(p),
                    None => {}
                }
                for (i, b) in out.iter_mut().enumerate() {
                    *b = self.io[o + i] | self.io_force[o + i];
                }
                self.record(false, addr, &out);
            }
            Region::Unmapped => self.unmapped_accesses += 1,
        }
        out
    }

    fn write<const N: usize>(&mut self, addr: u32, bytes: [u8; N]) {
        match fit(region(addr), N) {
            Region::Rom(_) => self.rom_writes += 1,
            Region::Ram(o) => self.ram[o..o + N].copy_from_slice(&bytes),
            Region::Io(o) => {
                if let Some(p) = periph_at(addr) {
                    self.store_regs(p);
                    let (b, len) = p.span();
                    let mut before = [0u8; 96];
                    before[..len].copy_from_slice(&self.io[b..b + len]);
                    self.io[o..o + N].copy_from_slice(&bytes);
                    let (before, after) = (&before[..len], &self.io[b..b + len]);
                    match p {
                        Periph::Cmt => self.cmt.load(self.now, before, after),
                        Periph::Sci => self.sci.load(self.now, before, after),
                        Periph::Adc => self.adc.load(self.now, before, after),
                    }
                    self.store_regs(p);
                    self.update_interrupts();
                } else {
                    self.io[o..o + N].copy_from_slice(&bytes);
                    if (intc::IPR_BASE..intc::IPR_END).contains(&addr) {
                        self.update_interrupts();
                    }
                }
                self.record(true, addr, &bytes);
            }
            Region::Unmapped => self.unmapped_accesses += 1,
        }
    }

    /// Initial values of the modelled registers that are not zero after a power-on reset.
    fn reset_registers(&mut self) {
        // H-UDI: SDIR = H'E000 and SDSR = H'0F01 on the SH7059F (manual table 20.2 and
        // its errata, which give H'5001 for the SH7058SF and H'0B01 for the SH7058). They
        // are set by TRST, not by reset; the board is taken to assert TRST at power-up
        // [SUP]. The boot loader reads SDSR to tell the chip variant.
        let h = (HUDI_SDIR - IO_BASE) as usize;
        self.io[h..h + 4].copy_from_slice(&[0xE0, 0x00, 0x0F, 0x01]);
        self.cmt.ch[0].cor = 0xFFFF;
        self.cmt.ch[1].cor = 0xFFFF;
        for p in [Periph::Cmt, Periph::Sci, Periph::Adc] {
            self.store_regs(p);
        }
    }

    /// Copies a peripheral model's registers into the register storage before a read.
    fn store_regs(&mut self, p: Periph) {
        let (b, len) = p.span();
        let regs = &mut self.io[b..b + len];
        match p {
            Periph::Cmt => self.cmt.store(self.now, regs),
            Periph::Sci => self.sci.store(regs),
            Periph::Adc => self.adc.store(regs),
        }
    }

    /// Takes the peripheral events due by `now` and refreshes the pending interrupt and
    /// the next event.
    pub fn advance(&mut self) {
        self.cmt.advance(self.now);
        self.sci.advance(self.now);
        self.adc.advance(self.now);
        self.update_interrupts();
    }

    fn update_interrupts(&mut self) {
        let i = (intc::IPR_BASE - IO_BASE) as usize;
        self.pending = intc::highest(
            &self.io[i..i + 24],
            &[
                (intc::CMTI0, self.cmt.ch[0].requesting()),
                (intc::ADI0, self.adc.requesting(0)),
                (intc::CMTI1, self.cmt.ch[1].requesting()),
                (intc::ADI1, self.adc.requesting(1)),
                (intc::ADI2, self.adc.requesting(2)),
            ],
        );
        self.next_event = self
            .cmt
            .next_event()
            .min(self.sci.next_event())
            .min(self.adc.next_event());
    }

    #[inline(always)]
    fn record(&mut self, write: bool, addr: u32, bytes: &[u8]) {
        if let Some(t) = self.trace.as_mut() {
            let value = bytes.iter().fold(0u32, |v, b| (v << 8) | *b as u32);
            t.push(IoAccess {
                pc: self.trace_pc,
                write,
                addr,
                size: bytes.len() as u8,
                value,
            });
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

    /// Instruction fetch: almost always from flash, so that case skips the region match.
    #[inline(always)]
    fn fetch16(&mut self, addr: u32) -> u16 {
        let a = addr as usize;
        if a + 1 < ROM_SIZE {
            u16::from_be_bytes([self.rom[a], self.rom[a + 1]])
        } else {
            self.read16(addr)
        }
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
    fn hudi_reads_as_an_sh7059f() {
        let mut c = chip();
        assert_eq!(c.read16(HUDI_SDIR), 0xE000);
        assert_eq!(c.read16(HUDI_SDIR + 2), 0x0F01, "SDSR of the SH7059F");
    }

    #[test]
    fn peripherals_answer_through_the_bus() {
        let mut c = chip();
        // SCI3 SSR starts at H'84; A/D2 ADCSR2 at H'08 and ADCR2 at H'0F.
        assert_eq!(c.read8(0xFFFF_F01C), 0x84);
        assert_eq!(c.read8(0xFFFF_F858), 0x08);
        assert_eq!(c.read8(0xFFFF_F859), 0x0F);
        // A/D0: convert AN0 once, read the result after the conversion time.
        c.adc.input[0] = 0x155;
        c.write8(0xFFFF_F818, 0x00);
        c.write8(0xFFFF_F819, 0x20);
        assert!(c.next_event < u64::MAX);
        c.now = c.next_event;
        c.advance();
        assert_eq!(c.read8(0xFFFF_F818) & 0x80, 0x80, "ADF");
        assert_eq!(c.read16(0xFFFF_F800), 0x155 << 6);
    }

    #[test]
    fn forced_bits_and_tracing() {
        let mut c = chip();
        c.force_bits(0xFFFF_ED18, 0x80);
        c.trace = Some(Vec::new());
        c.trace_pc = 0x1234;
        c.write16(0xFFFF_ED18, 0x0001);
        assert_eq!(c.read16(0xFFFF_ED18), 0x8001);
        let _ = c.read32(0xFFFE_8000); // RAM: not traced
        let t = c.trace.unwrap();
        assert_eq!(t.len(), 2);
        assert_eq!(
            t[1],
            IoAccess {
                pc: 0x1234,
                write: false,
                addr: 0xFFFF_ED18,
                size: 2,
                value: 0x8001
            }
        );
    }

    #[test]
    fn the_cpu_resets_from_the_flash_vectors() {
        let mut c = chip();
        let mut cpu = sh2e::Cpu::new();
        cpu.power_on_reset(&mut c);
        assert_eq!(cpu.pc, 0x0000_1000);
    }
}
