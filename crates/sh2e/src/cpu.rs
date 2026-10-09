use crate::Bus;

/// T bit of SR.
pub const SR_T: u32 = 0x0000_0001;
/// S bit of SR (saturation for MAC).
pub const SR_S: u32 = 0x0000_0002;
/// Interrupt mask bits I3..I0 of SR.
pub const SR_I_MASK: u32 = 0x0000_00F0;
/// Q bit of SR (division).
pub const SR_Q: u32 = 0x0000_0100;
/// M bit of SR (division).
pub const SR_M: u32 = 0x0000_0200;
/// The SR bits that exist on the SH-2: M, Q, I3..I0, S, T. The rest read as 0.
pub const SR_MASK: u32 = 0x0000_03F3;

/// FPSCR after a reset (manual table 2.1).
pub const FPSCR_RESET: u32 = 0x0004_0001;

/// Exception vector numbers (manual table 6.3).
pub mod vector {
    /// General illegal instruction.
    pub const GENERAL_ILLEGAL: u32 = 4;
    /// Illegal instruction in a delay slot.
    pub const SLOT_ILLEGAL: u32 = 6;
    /// CPU address error.
    pub const CPU_ADDRESS_ERROR: u32 = 9;
    /// NMI.
    pub const NMI: u32 = 11;
    /// FPU exception.
    pub const FPU: u32 = 13;
}

/// Entries of the exception vector table used at reset (manual section 6).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Vector {
    /// Vector 0: initial PC after a power-on reset.
    PowerOnPc = 0,
    /// Vector 1: initial SP after a power-on reset.
    PowerOnSp = 1,
    /// Vector 2: initial PC after a manual reset.
    ManualPc = 2,
    /// Vector 3: initial SP after a manual reset.
    ManualSp = 3,
}

impl Vector {
    /// Address of the vector in the table, which sits at address 0 after a reset
    /// (VBR = 0).
    pub fn address(self) -> u32 {
        self as u32 * 4
    }
}

/// Behaviour the manual does not pin down, counted every time the firmware runs into it.
///
/// Each counter is a path implemented on the best available reading (noted where it is
/// implemented). A non-zero count after a run says the result may depend on it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Unverified {
    /// MAC.L or MAC.W with SR.S = 1 (saturating multiply-and-accumulate). The software
    /// manual contradicts itself here: for MAC.W its text sets MACH bit 0 on overflow and
    /// its code does not; for MAC.L its code uses only MACH bits 15..0. The text's ranges
    /// are what is implemented.
    pub mac_saturation: u64,
    /// One of the two FADD/FSUB cases of hardware manual 3.6 (MAX against an infinity),
    /// which that manual describes with an inconsistent sign; +INF is used for both.
    pub inf_max_quirk: u64,
}

/// Architectural state of an SH-2E, plus counters.
///
/// Floating-point registers hold raw IEEE-754 bit patterns, not `f32`: the FPU rounds
/// toward zero and uses the opposite quiet/signaling NaN convention from the host, so the
/// host FPU must not touch the values on the way.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cpu {
    /// General registers R0..R15; R15 is the stack pointer.
    pub r: [u32; 16],
    /// Status register.
    pub sr: u32,
    /// Global base register.
    pub gbr: u32,
    /// Vector base register.
    pub vbr: u32,
    /// Multiply-and-accumulate register, high.
    pub mach: u32,
    /// Multiply-and-accumulate register, low.
    pub macl: u32,
    /// Procedure register (return address).
    pub pr: u32,
    /// Program counter: the address of the next instruction to execute.
    pub pc: u32,
    /// Floating-point registers FR0..FR15, as raw bits.
    pub fr: [u32; 16],
    /// Floating-point communication register.
    pub fpul: u32,
    /// Floating-point status and control register.
    pub fpscr: u32,
    /// Asleep after SLEEP, until an interrupt is accepted.
    pub sleeping: bool,
    /// Instructions executed, delay slots included.
    pub instructions: u64,
    /// Estimated CPU cycles: the minimum counts of manual tables 2.12 to 2.19, with no
    /// wait states or pipeline contention.
    pub cycles: u64,
    /// Exceptions taken (resets excluded).
    pub exceptions: u64,
    /// Vector number of the last exception taken.
    pub last_exception: Option<u32>,
    /// Paths taken that the manual does not pin down.
    pub unverified: Unverified,
    /// Raise CPU address errors on misaligned data accesses (manual 6.3), as the chip
    /// does. Only for comparing with Ghidra's emulator, which does not model them, is
    /// this turned off: the access is then made at the misaligned address.
    pub address_errors: bool,
    /// The last instruction was one after which interrupts are not accepted.
    pub(crate) irq_block: bool,
    /// A data access of the current instruction was misaligned.
    pub(crate) addr_error: bool,
}

impl Default for Cpu {
    fn default() -> Self {
        Self::new()
    }
}

impl Cpu {
    /// A core with every register at zero. Call [`Cpu::power_on_reset`] before running.
    pub fn new() -> Self {
        Self {
            r: [0; 16],
            sr: 0,
            gbr: 0,
            vbr: 0,
            mach: 0,
            macl: 0,
            pr: 0,
            pc: 0,
            fr: [0; 16],
            fpul: 0,
            fpscr: 0,
            sleeping: false,
            instructions: 0,
            cycles: 0,
            exceptions: 0,
            last_exception: None,
            unverified: Unverified::default(),
            address_errors: true,
            irq_block: false,
            addr_error: false,
        }
    }

    /// Power-on reset, as in manual table 2.1: PC and R15 from vectors 0 and 1,
    /// SR.I = 0xF, VBR = 0 and FPSCR = H'00040001.
    ///
    /// The manual leaves every other register undefined. Here they are zero, so that a run
    /// is reproducible; firmware that reads one before writing it is a finding to report.
    pub fn power_on_reset<B: Bus>(&mut self, bus: &mut B) {
        let address_errors = self.address_errors;
        *self = Self::new();
        self.address_errors = address_errors;
        self.pc = bus.read32(Vector::PowerOnPc.address());
        self.r[15] = bus.read32(Vector::PowerOnSp.address());
        self.sr = SR_I_MASK;
        self.vbr = 0;
        self.fpscr = FPSCR_RESET;
    }

    /// The T bit as 0 or 1.
    #[inline(always)]
    pub fn t(&self) -> u32 {
        self.sr & SR_T
    }

    #[inline(always)]
    pub(crate) fn set_t(&mut self, t: bool) {
        self.sr = (self.sr & !SR_T) | t as u32;
    }

    /// Current interrupt mask level, 0 to 15.
    pub fn interrupt_mask(&self) -> u32 {
        (self.sr & SR_I_MASK) >> 4
    }

    /// Exception processing for address errors, interrupts and instructions (manual
    /// 6.1.2): SR and then `saved_pc` are pushed on the stack pointed to by R15, and
    /// execution continues at the address in vector `vector` of the table at VBR.
    pub(crate) fn raise<B: Bus>(&mut self, bus: &mut B, vector: u32, saved_pc: u32) {
        let sp = self.r[15].wrapping_sub(4);
        bus.write32(sp, self.sr);
        let sp = sp.wrapping_sub(4);
        bus.write32(sp, saved_pc);
        self.r[15] = sp;
        self.pc = bus.read32(self.vbr.wrapping_add(vector * 4));
        self.exceptions += 1;
        self.last_exception = Some(vector);
    }

    /// Whether an interrupt of `level` (1 to 15, or 16 for NMI) would be accepted now.
    ///
    /// Not after an instruction that blocks interrupts (manual table 6.10), and not at or
    /// below the SR mask, NMI excepted.
    pub fn accepts_interrupt(&self, level: u32) -> bool {
        !self.irq_block && (level >= 16 || level > self.interrupt_mask())
    }

    /// Takes an interrupt if it can be accepted (see [`Cpu::accepts_interrupt`]): pushes SR
    /// and PC, sets the SR mask to the interrupt's level (15 for NMI) and jumps through
    /// `vector`. Wakes the core from SLEEP. Returns whether it was taken.
    pub fn interrupt<B: Bus>(&mut self, bus: &mut B, level: u32, vector: u32) -> bool {
        if !self.accepts_interrupt(level) {
            return false;
        }
        let pc = self.pc;
        self.raise(bus, vector, pc);
        self.sr = (self.sr & !SR_I_MASK) | (level.min(15) << 4);
        self.sleeping = false;
        true
    }

    // Data accesses. A misaligned word or longword access raises a CPU address error
    // once the instruction finishes (manual 6.3); the access itself is dropped and a read
    // returns 0.

    #[inline(always)]
    pub(crate) fn rd8<B: Bus>(&mut self, bus: &mut B, a: u32) -> u8 {
        bus.read8(a)
    }

    #[inline(always)]
    pub(crate) fn rd16<B: Bus>(&mut self, bus: &mut B, a: u32) -> u16 {
        if a & 1 != 0 && self.address_errors {
            self.addr_error = true;
            return 0;
        }
        bus.read16(a)
    }

    #[inline(always)]
    pub(crate) fn rd32<B: Bus>(&mut self, bus: &mut B, a: u32) -> u32 {
        if a & 3 != 0 && self.address_errors {
            self.addr_error = true;
            return 0;
        }
        bus.read32(a)
    }

    #[inline(always)]
    pub(crate) fn wr8<B: Bus>(&mut self, bus: &mut B, a: u32, v: u32) {
        bus.write8(a, v as u8);
    }

    #[inline(always)]
    pub(crate) fn wr16<B: Bus>(&mut self, bus: &mut B, a: u32, v: u32) {
        if a & 1 != 0 && self.address_errors {
            self.addr_error = true;
            return;
        }
        bus.write16(a, v as u16);
    }

    #[inline(always)]
    pub(crate) fn wr32<B: Bus>(&mut self, bus: &mut B, a: u32, v: u32) {
        if a & 3 != 0 && self.address_errors {
            self.addr_error = true;
            return;
        }
        bus.write32(a, v);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Flat;

    fn table(pc: u32, sp: u32) -> Flat {
        let mut m = Flat::new(64);
        m.write32(0, pc);
        m.write32(4, sp);
        m.write32(8, 0xDEAD_BEEF);
        m.write32(12, 0xCAFE_F00D);
        m
    }

    #[test]
    fn power_on_reset_takes_pc_and_sp_from_vectors_0_and_1() {
        let mut cpu = Cpu::new();
        cpu.power_on_reset(&mut table(0x0000_1234, 0x8765_4320));
        assert_eq!(cpu.pc, 0x0000_1234);
        assert_eq!(cpu.r[15], 0x8765_4320);
    }

    #[test]
    fn power_on_reset_sets_the_documented_initial_values() {
        let mut cpu = Cpu::new();
        cpu.sr = 0xFFFF_FFFF;
        cpu.vbr = 0x0010_0000;
        cpu.r[3] = 7;
        cpu.power_on_reset(&mut table(0x100, 0x200));
        assert_eq!(cpu.sr, SR_I_MASK);
        assert_eq!(cpu.vbr, 0);
        assert_eq!(cpu.fpscr, 0x0004_0001);
        assert_eq!(cpu.r[3], 0, "undefined registers start at zero");
    }

    #[test]
    fn vector_addresses() {
        assert_eq!(Vector::PowerOnPc.address(), 0);
        assert_eq!(Vector::PowerOnSp.address(), 4);
        assert_eq!(Vector::ManualPc.address(), 8);
        assert_eq!(Vector::ManualSp.address(), 12);
    }
}
