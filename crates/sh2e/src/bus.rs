/// The CPU's view of memory and on-chip peripherals.
///
/// Addresses are physical and data is big-endian, as on the chip. Reads take
/// `&mut self` because reading a peripheral register can have side effects (clearing a
/// status flag, popping a receive buffer).
///
/// Alignment is the CPU's business: it raises an address error before calling the bus
/// with a misaligned word or longword address.
pub trait Bus {
    /// Reads one byte.
    fn read8(&mut self, addr: u32) -> u8;
    /// Reads a 16-bit word from an even address.
    fn read16(&mut self, addr: u32) -> u16;
    /// Reads a 32-bit longword from an address that is a multiple of 4.
    fn read32(&mut self, addr: u32) -> u32;
    /// Writes one byte.
    fn write8(&mut self, addr: u32, value: u8);
    /// Writes a 16-bit word to an even address.
    fn write16(&mut self, addr: u32, value: u16);
    /// Writes a 32-bit longword to an address that is a multiple of 4.
    fn write32(&mut self, addr: u32, value: u32);

    /// Fetches an instruction from an even address. Separate from [`Bus::read16`] so an
    /// implementation can give code fetches a fast path, and so instruction fetches do not
    /// trigger peripheral read side effects in tracing.
    #[inline(always)]
    fn fetch16(&mut self, addr: u32) -> u16 {
        self.read16(addr)
    }
}
