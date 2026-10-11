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
pub mod atu;
pub mod cmt;
pub mod debug;
pub mod dmac;
pub mod hcan;
pub mod intc;
mod machine;
pub mod pins;
pub mod sci;

pub use machine::{Machine, Stop};
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

/// Frames of other nodes kept in [`Sh7059::heard`] for a listener that does not read
/// them; later ones are dropped.
pub const HEARD_KEPT: usize = 4096;

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

/// A port pin wired to the peer on an SCI channel: a chip select, or a serial clock
/// inversion bit. The peer hears every change of level through [`sci::Peer::pin`].
///
/// The level is the bit as written in the register; whether the pin is set as an output
/// with that function is not checked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Wire {
    /// The 16-bit port register holding the pin: a data register (PxDR) or PLIR.
    pub reg: u32,
    /// Bit of the pin in that register.
    pub bit: u8,
    /// SCI channel whose peer hears it.
    pub sci: usize,
    /// Line number passed to the peer.
    pub line: usize,
}

/// A pin the board drives into an I/O port. While the firmware has it as an input (its
/// bit clear in the port's IOR), a read of the port's data register returns its [`Level`]
/// there, not what was last written; as an output it reads back as written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InputPin {
    /// The 16-bit port data register (PxDR).
    pub dr: u32,
    /// The port's I/O register (PxIOR): a set bit makes the pin an output.
    pub ior: u32,
    /// Bit of the pin in both.
    pub bit: u8,
    /// The level the board puts on it.
    pub level: Level,
}

/// What drives an [`InputPin`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    /// A fixed level: true is high.
    Fixed(bool),
    /// The data line from the peer of this SCI channel ([`sci::Peer::data_out`]): the
    /// pin is wired to the channel's receive line and read as a port bit.
    SciData(usize),
}

/// Most wires a chip takes.
pub const MAX_WIRES: usize = 16;

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
    Atu,
    Dmac,
    Hcan,
}

impl Periph {
    /// Offset of the block in the register storage, and its length.
    fn span(self) -> (usize, usize) {
        let (b, e) = match self {
            Periph::Cmt => (cmt::BASE, cmt::END),
            Periph::Sci => (sci::BASE, sci::END),
            Periph::Adc => (adc::BASE, adc::END),
            Periph::Atu => (atu::BASE, atu::END),
            Periph::Dmac => (dmac::BASE, dmac::END),
            Periph::Hcan => (hcan::BASE, hcan::END),
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
    } else if (atu::BASE..atu::END).contains(&addr) {
        Some(Periph::Atu)
    } else if (dmac::BASE..dmac::END).contains(&addr) {
        Some(Periph::Dmac)
    } else if (hcan::BASE..hcan::END).contains(&addr) {
        Some(Periph::Hcan)
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
    /// The flash decoded, one entry per halfword, for instruction fetches. The flash
    /// never changes while running (writes to it are ignored), so neither does this;
    /// anything that ever changes `rom` must rebuild it.
    decoded: Box<[sh2e::Decoded]>,
    ram: Box<[u8]>,
    io: Box<[u8]>,
    /// Bits set in every read of each register byte, as the phase 0 tool forced them to
    /// get past status polls. All zero by default.
    io_force: Box<[u8]>,
    /// Pins the board drives into the I/O ports ([`InputPin`]). A pin not listed reads
    /// back what was last written to its data register.
    pub inputs: Vec<InputPin>,
    /// Accesses to the register area, when tracing is on.
    pub trace: Option<Vec<IoAccess>>,
    /// The instruction address recorded with each traced or watched access; the
    /// machine sets it before every step while tracing or watching.
    pub trace_pc: u32,
    /// The CPU's instruction count, recorded with each watched access.
    pub instruction: u64,
    /// Memory watchpoints, when set.
    pub watch: Option<Box<debug::Watch>>,
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
    /// Advanced timer unit II (the channels modelled so far).
    pub atu: atu::Atu,
    /// DMA controller.
    pub dmac: dmac::Dmac,
    /// CAN controllers.
    pub hcan: hcan::Hcan,
    /// Set while the DMAC carries out units, so its own accesses do not re-enter it.
    dma_busy: bool,
    /// Port pins wired to SCI peers.
    wires: Vec<Wire>,
    /// What drives the input pins from outside, if anything.
    pub stimulus: Option<Box<dyn pins::Stimulus>>,
    /// Other nodes on the CAN bus of HCAN channel 0, if any.
    pub can_source: Option<Box<dyn hcan::CanSource>>,
    /// Frames the nodes of [`Sh7059::can_source`] have put on the bus, for tools that
    /// listen to it; whoever reads them empties it.
    pub heard: Vec<(u64, hcan::Frame)>,
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
            decoded: flash
                .chunks_exact(2)
                .map(|w| sh2e::predecode(u16::from_be_bytes([w[0], w[1]])))
                .collect(),
            ram: vec![0; RAM_SIZE].into_boxed_slice(),
            io: vec![0; IO_SIZE].into_boxed_slice(),
            io_force: vec![0; IO_SIZE].into_boxed_slice(),
            inputs: Vec::new(),
            trace: None,
            trace_pc: 0,
            instruction: 0,
            watch: None,
            unmapped_accesses: 0,
            rom_writes: 0,
            now: 0,
            cmt: cmt::Cmt::default(),
            sci: sci::Sci::default(),
            adc: adc::Adc::default(),
            atu: atu::Atu::default(),
            dmac: dmac::Dmac::default(),
            hcan: hcan::Hcan::default(),
            dma_busy: false,
            wires: Vec::new(),
            stimulus: None,
            can_source: None,
            heard: Vec::new(),
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

    /// Wires a port pin to the peer of an SCI channel, which hears its current level at
    /// once and every change after. Panics past [`MAX_WIRES`] or if the register is not a
    /// peripheral register.
    pub fn wire(&mut self, w: Wire) {
        assert!(self.wires.len() < MAX_WIRES, "at most {MAX_WIRES} wires");
        assert!(
            matches!(region(w.reg), Region::Io(o) if o + 2 <= IO_SIZE) && w.bit < 16,
            "wire: {:#010X} bit {} is not a 16-bit peripheral register bit",
            w.reg,
            w.bit
        );
        self.wires.push(w);
        let level = self.pin_level(&w);
        self.sci.ch[w.sci].peer.pin(w.line, level);
    }

    fn pin_level(&self, w: &Wire) -> bool {
        let o = (w.reg - IO_BASE) as usize;
        (u16::from_be_bytes([self.io[o], self.io[o + 1]]) >> w.bit) & 1 != 0
    }

    /// Stores a write to a plain register, telling SCI peers about wired pins it changes.
    fn write_plain<const N: usize>(&mut self, addr: u32, o: usize, bytes: [u8; N]) {
        let touched = self
            .wires
            .iter()
            .any(|w| w.reg < addr + N as u32 && addr < w.reg + 2);
        if !touched {
            self.io[o..o + N].copy_from_slice(&bytes);
            return;
        }
        let mut old = [false; MAX_WIRES];
        for (l, w) in old.iter_mut().zip(&self.wires) {
            *l = self.pin_level(w);
        }
        self.io[o..o + N].copy_from_slice(&bytes);
        for (i, was) in old.into_iter().enumerate().take(self.wires.len()) {
            let w = self.wires[i];
            let level = self.pin_level(&w);
            if level != was {
                self.sci.ch[w.sci].peer.pin(w.line, level);
            }
        }
    }

    /// The byte at `addr` as stored, for a debugger: no peripheral is brought up to date,
    /// nothing is traced or watched. Counters read as they were last stored. Unmapped
    /// space reads 0.
    pub fn peek(&self, addr: u32) -> u8 {
        match region(addr) {
            Region::Rom(o) => self.rom[o],
            Region::Ram(o) => self.ram[o],
            Region::Io(o) => self.io[o] | self.io_force[o],
            Region::Unmapped => 0,
        }
    }

    /// The PWM output of ATU-II channel 6 or 7 (`ch` = 0 or 1), timer `k` (0 = A .. 3 =
    /// D), as a duty fraction, if that timer is running.
    pub fn pwm_duty(&self, ch: usize, k: usize) -> Option<f64> {
        self.atu.duty(&self.io, ch, k)
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

    /// Puts the levels of the board's input pins into a read of `out.len()` bytes at
    /// `addr`.
    fn drive_inputs(&self, addr: u32, out: &mut [u8]) {
        for p in &self.inputs {
            let ior = (p.ior - IO_BASE) as usize;
            if u16::from_be_bytes([self.io[ior], self.io[ior + 1]]) >> p.bit & 1 != 0 {
                continue;
            }
            // The high byte of a register comes first.
            let at = p.dr + u32::from(p.bit < 8);
            if let Some(b) = at.checked_sub(addr).and_then(|i| out.get_mut(i as usize)) {
                let m = 1u8 << (p.bit % 8);
                let high = match p.level {
                    Level::Fixed(h) => h,
                    Level::SciData(k) => self.sci.ch[k].peer.data_out(),
                };
                *b = if high { *b | m } else { *b & !m };
            }
        }
    }

    #[inline(always)]
    fn read<const N: usize>(&mut self, addr: u32) -> [u8; N] {
        // Flash and RAM straight away when nothing watches: most data accesses.
        if self.watch.is_none() {
            let rom = addr.wrapping_sub(ROM_BASE) as usize;
            if rom + N <= ROM_SIZE {
                return self.rom[rom..rom + N].try_into().expect("N bytes");
            }
            let ram = addr.wrapping_sub(RAM_BASE) as usize;
            if ram + N <= RAM_SIZE {
                return self.ram[ram..ram + N].try_into().expect("N bytes");
            }
        }
        self.read_any(addr)
    }

    fn read_any<const N: usize>(&mut self, addr: u32) -> [u8; N] {
        let mut out = [0; N];
        match fit(region(addr), N) {
            Region::Rom(o) => out.copy_from_slice(&self.rom[o..o + N]),
            Region::Ram(o) => out.copy_from_slice(&self.ram[o..o + N]),
            Region::Io(o) => {
                match periph_at(addr) {
                    Some(Periph::Atu) => self.atu.store(self.now, &mut self.io),
                    Some(Periph::Dmac) => self.dma_catch_up(),
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
                if !self.inputs.is_empty() {
                    self.drive_inputs(addr, &mut out);
                }
                self.record(false, addr, &out);
            }
            Region::Unmapped => self.unmapped_accesses += 1,
        }
        if let Some(w) = self.watch.as_mut() {
            w.check(self.trace_pc, self.instruction, addr, &out, false);
        }
        out
    }

    #[inline(always)]
    fn write<const N: usize>(&mut self, addr: u32, bytes: [u8; N]) {
        if self.watch.is_none() {
            let ram = addr.wrapping_sub(RAM_BASE) as usize;
            if ram + N <= RAM_SIZE {
                self.ram[ram..ram + N].copy_from_slice(&bytes);
                return;
            }
        }
        self.write_any(addr, bytes)
    }

    fn write_any<const N: usize>(&mut self, addr: u32, bytes: [u8; N]) {
        if let Some(w) = self.watch.as_mut() {
            w.check(self.trace_pc, self.instruction, addr, &bytes, true);
        }
        match fit(region(addr), N) {
            Region::Rom(_) => self.rom_writes += 1,
            Region::Ram(o) => self.ram[o..o + N].copy_from_slice(&bytes),
            Region::Io(o) => {
                if periph_at(addr) == Some(Periph::Atu) {
                    self.atu.resync(self.now);
                    self.atu.store(self.now, &mut self.io);
                    let mut before = [0u8; N];
                    before.copy_from_slice(&self.io[o..o + N]);
                    self.io[o..o + N].copy_from_slice(&bytes);
                    self.atu.load(self.now, addr, &before, &mut self.io);
                    self.update_interrupts();
                } else if periph_at(addr) == Some(Periph::Hcan) {
                    let mut before = [0u8; N];
                    before.copy_from_slice(&self.io[o..o + N]);
                    self.io[o..o + N].copy_from_slice(&bytes);
                    self.hcan.load(self.now, addr, &before, &mut self.io);
                    self.update_interrupts();
                } else if periph_at(addr) == Some(Periph::Dmac) {
                    self.dma_catch_up();
                    let mut before = [0u8; N];
                    before.copy_from_slice(&self.io[o..o + N]);
                    self.io[o..o + N].copy_from_slice(&bytes);
                    self.dmac.load(self.now, addr, &before, &mut self.io);
                    self.update_interrupts();
                } else if let Some(p) = periph_at(addr) {
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
                        Periph::Atu | Periph::Dmac | Periph::Hcan => {
                            unreachable!("handled above")
                        }
                    }
                    self.store_regs(p);
                    self.update_interrupts();
                } else {
                    self.write_plain(addr, o, bytes);
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
        self.atu.reset(&mut self.io);
        self.hcan.reset(&mut self.io);
    }

    /// Copies a peripheral model's registers into the register storage before a read.
    fn store_regs(&mut self, p: Periph) {
        let (b, len) = p.span();
        match p {
            Periph::Atu => return self.atu.store(self.now, &mut self.io),
            Periph::Dmac => return self.dma_catch_up(),
            Periph::Hcan => return,
            _ => {}
        }
        let regs = &mut self.io[b..b + len];
        match p {
            Periph::Cmt => self.cmt.store(self.now, regs),
            Periph::Sci => self.sci.store(regs),
            Periph::Adc => self.adc.store(regs),
            Periph::Atu | Periph::Dmac | Periph::Hcan => unreachable!(),
        }
    }

    /// Takes the peripheral events due by `now` and refreshes the pending interrupt and
    /// the next event.
    pub fn advance(&mut self) {
        self.take_edges();
        self.cmt.advance(self.now);
        self.sci.advance(self.now);
        self.adc.advance(self.now);
        self.atu.advance(self.now, &mut self.io);
        self.dma_catch_up();
        self.hcan.advance(self.now, &mut self.io);
        self.take_frames();
        self.update_interrupts();
    }

    /// Connects other nodes to the CAN bus of HCAN channel 0, replacing any before.
    pub fn set_can_source(&mut self, s: Box<dyn hcan::CanSource>) {
        self.can_source = Some(s);
        self.update_interrupts();
    }

    /// Delivers the frames of other nodes whose last bit is through by `now`.
    fn take_frames(&mut self) {
        let Some(s) = self.can_source.as_mut() else {
            return;
        };
        while let Some((at, _)) = s.peek() {
            if at > self.now {
                break;
            }
            let (at, f) = s.take().expect("peeked");
            self.hcan.deliver(&mut self.io, 0, &f);
            if self.heard.len() < HEARD_KEPT {
                self.heard.push((at, f));
            }
        }
    }

    /// Connects what drives the input pins, replacing what did before.
    pub fn set_stimulus(&mut self, s: Box<dyn pins::Stimulus>) {
        self.stimulus = Some(s);
        self.update_interrupts();
    }

    /// Changes a setting of what drives the input pins (see [`pins::Stimulus::set`])
    /// from now on.
    pub fn stimulus_set(&mut self, key: &str, value: f64) {
        if let Some(s) = self.stimulus.as_mut() {
            s.set(key, value, self.now);
            self.update_interrupts();
        }
    }

    /// The next edge on the input pins, not taken yet.
    pub fn next_edge(&self) -> Option<pins::Edge> {
        self.stimulus.as_ref()?.peek()
    }

    /// Where the crankshaft driving the input pins is at cycle `now`, degrees over 720
    /// (see [`pins::Stimulus::crank_angle`]).
    pub fn crank_angle(&self, now: u64) -> Option<f64> {
        self.stimulus.as_ref()?.crank_angle(now)
    }

    /// Takes the input pin edges due by `now`, each at its own cycle.
    fn take_edges(&mut self) {
        let Some(s) = self.stimulus.as_mut() else {
            return;
        };
        while let Some(e) = s.peek() {
            if e.at > self.now {
                break;
            }
            let e = s.take().expect("peeked");
            match e.pin {
                pins::Pin::Ti0(k) => {
                    self.atu.capture(&mut self.io, e.at, k as usize & 3, e.high);
                }
                pins::Pin::Ti10 => self.atu.ti10_edge(&mut self.io, e.at, e.high),
            }
        }
    }

    /// A frame arrives from the CAN bus on HCAN channel `ch`. Returns the mailbox that
    /// took it, if any.
    pub fn can_deliver(&mut self, ch: usize, frame: &hcan::Frame) -> Option<usize> {
        let r = self.hcan.deliver(&mut self.io, ch, frame);
        self.update_interrupts();
        r
    }

    /// Carries out the DMA transfer units due by `now`.
    fn dma_catch_up(&mut self) {
        if self.dma_busy || self.dmac.idle() {
            return;
        }
        self.dma_busy = true;
        for k in 0..4 {
            for _ in 0..self.dmac.due(k, self.now, &self.io) {
                let Some(u) = self.dmac.unit(k, &mut self.io) else {
                    // Address error: every channel stops.
                    self.dmac.check(self.now, &self.io);
                    break;
                };
                match u.size {
                    1 => {
                        let v = self.read::<1>(u.src);
                        self.write(u.dst, v);
                    }
                    2 => {
                        let v = self.read::<2>(u.src);
                        self.write(u.dst, v);
                    }
                    _ => {
                        let v = self.read::<4>(u.src);
                        self.write(u.dst, v);
                    }
                }
                self.dmac.unit_done(k, &mut self.io);
            }
        }
        self.dmac.check(self.now, &self.io);
        self.dma_busy = false;
    }

    fn update_interrupts(&mut self) {
        let i = (intc::IPR_BASE - IO_BASE) as usize;
        let (io, ipr) = (&self.io, &self.io[i..i + 24]);
        // Offered in vector order, which is the default priority order of table 7.3.
        let mut best = (0, 0);
        for (k, dei) in intc::DEI.into_iter().enumerate() {
            if dmac::Dmac::requesting(io, k) {
                intc::offer(ipr, dei, &mut best);
            }
        }
        atu::for_each_request(io, self.atu.held10(io), |vector, ipr_index, shift| {
            let src = intc::Source {
                vector,
                ipr: ipr_index,
                shift,
            };
            intc::offer(ipr, src, &mut best);
        });
        for (src, on) in [
            (intc::CMTI0, self.cmt.ch[0].requesting()),
            (intc::ADI0, self.adc.requesting(0)),
            (intc::CMTI1, self.cmt.ch[1].requesting()),
            (intc::ADI1, self.adc.requesting(1)),
            (intc::ADI2, self.adc.requesting(2)),
        ] {
            if on {
                intc::offer(ipr, src, &mut best);
            }
        }
        for ch in 0..2 {
            for (k, on) in hcan::Hcan::requests(io, ch).into_iter().enumerate() {
                if on {
                    intc::offer(ipr, intc::HCAN[ch][k], &mut best);
                }
            }
        }
        self.pending = best;
        self.next_event = self
            .cmt
            .next_event()
            .min(self.sci.next_event())
            .min(self.adc.next_event())
            .min(self.atu.next_event())
            .min(self.dmac.next_event(&self.io))
            .min(self.hcan.next_event())
            .min(
                self.stimulus
                    .as_ref()
                    .and_then(|s| s.peek())
                    .map_or(u64::MAX, |e| e.at),
            )
            .min(
                self.can_source
                    .as_ref()
                    .and_then(|s| s.peek())
                    .map_or(u64::MAX, |(at, _)| at),
            );
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

    /// Instruction fetch with its decoding: from the decoded flash when it is there.
    #[inline(always)]
    fn fetch_decoded(&mut self, addr: u32) -> (u16, sh2e::Decoded) {
        let a = addr as usize;
        if a + 1 < ROM_SIZE {
            (
                u16::from_be_bytes([self.rom[a], self.rom[a + 1]]),
                self.decoded[a >> 1],
            )
        } else {
            let code = self.read16(addr);
            (code, sh2e::predecode(code))
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
    fn atu_requests_reach_the_interrupt_controller_in_priority_order() {
        let mut c = chip();
        c.write16(0xFFFF_ED0A, 0x5000); // IPRF: IMI3A-D at level 5
        c.write16(0xFFFF_ED0C, 0x0050); // IPRG: CMI6A-D at level 5
        c.write8(0xFFFF_F4AC, 0x01); // TCR3: φ'/2
        c.write16(0xFFFF_F4A8, 100); // GR3D
        c.write8(0xFFFF_F4AA, 0x10); // TIOR3B: GR3D compare
        c.write16(0xFFFF_F482, 0x0008); // TIER3: IME3D
        c.write8(0xFFFF_F401, 0x10); // TSTR1: STR3
        assert_eq!(c.pending, (0, 0));
        c.now = c.next_event;
        c.advance();
        assert_eq!(c.pending, (5, 123), "IMI3D");
        // CMI6A at the same level: the lower vector (IMI3D) stays first.
        c.write16(0xFFFF_F508, 1); // CYLR6A
        c.write16(0xFFFF_F524, 0x0001); // TIER6: CME6A
        c.write8(0xFFFF_F400, 0x01); // TSTR2: 6A
        c.now = c.next_event;
        c.advance();
        assert_eq!(c.read16(0xFFFF_F522) & 1, 1, "CMF6A");
        assert_eq!(c.pending, (5, 123));
        // A higher level wins whatever the vector.
        c.write16(0xFFFF_ED0C, 0x0060);
        assert_eq!(c.pending, (6, 144), "CMI6A");
        // Clearing IMF3D and CMF6A leaves nothing.
        c.write16(0xFFFF_F480, 0x0000);
        c.write16(0xFFFF_F522, 0x0000);
        assert_eq!(c.pending, (0, 0));
    }

    /// Records pin changes, shared with the test.
    struct Pins(std::sync::Arc<std::sync::Mutex<Vec<(usize, bool)>>>);
    impl sci::Peer for Pins {
        fn exchange(&mut self, _: u8, _: bool) -> u8 {
            0
        }
        fn pin(&mut self, line: usize, high: bool) {
            self.0.lock().unwrap().push((line, high));
        }
    }

    #[test]
    fn wired_pins_reach_the_sci_peer_on_every_change() {
        let mut c = chip();
        let log = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        c.sci.ch[3].peer = Box::new(Pins(log.clone()));
        c.write16(0xFFFF_F76C, 0x0008); // PJDR: PJ3 high before wiring
        let w = |bit, line| Wire {
            reg: 0xFFFF_F76C,
            bit,
            sci: 3,
            line,
        };
        c.wire(w(3, 1));
        c.wire(w(4, 2));
        assert_eq!(
            *log.lock().unwrap(),
            [(1, true), (2, false)],
            "levels at wiring"
        );
        log.lock().unwrap().clear();
        c.write16(0xFFFF_F76C, 0x0018); // PJ4 up
        c.write8(0xFFFF_F76D, 0x10); // byte write: PJ3 down
        c.write8(0xFFFF_F76C, 0x00); // high byte: no wired pin changes
        c.write16(0xFFFF_F76C, 0x0010); // same levels: nothing
        assert_eq!(*log.lock().unwrap(), [(2, true), (1, false)]);
    }

    #[test]
    fn the_cpu_resets_from_the_flash_vectors() {
        let mut c = chip();
        let mut cpu = sh2e::Cpu::new();
        cpu.power_on_reset(&mut c);
        assert_eq!(cpu.pc, 0x0000_1000);
    }

    #[test]
    fn an_input_pin_reads_the_board_while_it_is_an_input() {
        // Port J: PJDR at F76C, PJIOR at F766.
        let (dr, ior) = (0xFFFF_F76C, 0xFFFF_F766);
        let mut c = chip();
        c.inputs.push(InputPin {
            dr,
            ior,
            bit: 13,
            level: Level::Fixed(true),
        });
        c.inputs.push(InputPin {
            dr,
            ior,
            bit: 2,
            level: Level::SciData(3),
        });
        c.write16(dr, 0x0004);
        // PJ13 reads high whatever was written; PJ2 follows SCI3's data line, low with
        // nothing there (sci::Fixed(0)).
        assert_eq!(c.read16(dr), 0x2000);
        assert_eq!(c.read8(dr), 0x20, "the high byte alone");
        // As outputs, both read back what was written.
        c.write16(ior, 0x2004);
        assert_eq!(c.read16(dr), 0x0004);
    }
}
