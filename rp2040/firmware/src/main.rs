#![no_std]
#![no_main]

use core::fmt::Write;
use cortex_m_rt::entry;
use embedded_hal::digital::v2::OutputPin;
use panic_halt as _;
use rp2040_hal as hal;

pub mod sh2e;

use hal::clocks::Clock;
use hal::pac;
use usb_device::class_prelude::*;
use usb_device::prelude::*;
use usbd_serial::SerialPort;

#[link_section = ".boot2"]
#[no_mangle]
#[used]
pub static BOOT2_FIRMWARE: [u8; 256] = rp2040_boot2::BOOT_LOADER_W25Q080;

/// A real SH7059 flash image to boot, embedded at build time. Only with the `real-boot`
/// feature, and never committed: you supply your own image (see `flash/README.md`). No
/// firmware image ships in this repository.
#[cfg(feature = "real-boot")]
static ECU_FLASH: &[u8] = include_bytes!("../flash/ecu.bin");

/// Emulated internal RAM of the SH7059 (80 KB, 0xFFFE_8000..0xFFFF_C000).
static mut SH_RAM: [u8; 81920] = [0; 81920];

/// A unique `&mut` to [`SH_RAM`]. Single-core, bare-metal, and the slice is only ever used
/// sequentially (one bus at a time), so there is no aliasing. Going through a raw pointer
/// keeps clear of the `static_mut_refs` lint (and the hard error it becomes in edition 2024).
fn sh_ram() -> &'static mut [u8] {
    unsafe { &mut *core::ptr::addr_of_mut!(SH_RAM) }
}

/// Bus to run real ECU code or the synthetic benchmarks.
pub struct EcuBus {
    pub rom: &'static [u8],
    pub ram: &'static mut [u8],
    pub io_reads: u32,
    pub io_writes: u32,
}

impl EcuBus {
    pub fn new(rom: &'static [u8], ram: &'static mut [u8]) -> Self {
        Self {
            rom,
            ram,
            io_reads: 0,
            io_writes: 0,
        }
    }
}

impl sh2e::Bus for EcuBus {
    #[inline(always)]
    fn read8(&mut self, addr: u32) -> u8 {
        if (addr as usize) < self.rom.len() {
            self.rom[addr as usize]
        } else if addr >= 0xFFFE_8000 && addr < 0xFFFF_C000 {
            self.ram[(addr - 0xFFFE_8000) as usize]
        } else {
            self.io_reads += 1;
            // Canned returns so a boot does not get stuck polling a peripheral that this
            // bare-metal harness does not model. These are placeholders, not a peripheral
            // model: they let the reset path advance, nothing more.
            match addr {
                0xFFFF_F01C => 0x84, // NMI / status
                0xFFFF_F818 => 0x80, // A/D ADF flag
                0xFFFF_F858 => 0x08,
                0xFFFF_F859 => 0x0F,
                _ => 0,
            }
        }
    }

    #[inline(always)]
    fn read16(&mut self, addr: u32) -> u16 {
        let b0 = self.read8(addr) as u16;
        let b1 = self.read8(addr + 1) as u16;
        (b0 << 8) | b1
    }

    #[inline(always)]
    fn read32(&mut self, addr: u32) -> u32 {
        let b0 = self.read8(addr) as u32;
        let b1 = self.read8(addr + 1) as u32;
        let b2 = self.read8(addr + 2) as u32;
        let b3 = self.read8(addr + 3) as u32;
        (b0 << 24) | (b1 << 16) | (b2 << 8) | b3
    }

    #[inline(always)]
    fn write8(&mut self, addr: u32, value: u8) {
        if addr >= 0xFFFE_8000 && addr < 0xFFFF_C000 {
            self.ram[(addr - 0xFFFE_8000) as usize] = value;
        } else {
            self.io_writes += 1;
        }
    }

    #[inline(always)]
    fn write16(&mut self, addr: u32, value: u16) {
        self.write8(addr, (value >> 8) as u8);
        self.write8(addr + 1, value as u8);
    }

    #[inline(always)]
    fn write32(&mut self, addr: u32, value: u32) {
        self.write8(addr, (value >> 24) as u8);
        self.write8(addr + 1, (value >> 16) as u8);
        self.write8(addr + 2, (value >> 8) as u8);
        self.write8(addr + 3, value as u8);
    }

    #[inline(always)]
    fn fetch16(&mut self, addr: u32) -> u16 {
        if (addr as usize + 1) < self.rom.len() {
            let b0 = self.rom[addr as usize] as u16;
            let b1 = self.rom[addr as usize + 1] as u16;
            (b0 << 8) | b1
        } else {
            self.read16(addr)
        }
    }
}

/// Writes formatted text over USB CDC with `core::fmt`.
struct UsbWriter<'w, 'a, B: UsbBus> {
    serial: &'w mut SerialPort<'a, B>,
}

impl<'w, 'a, B: UsbBus> Write for UsbWriter<'w, 'a, B> {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let mut bytes = s.as_bytes();
        while !bytes.is_empty() {
            match self.serial.write(bytes) {
                Ok(0) => break,
                Ok(n) => bytes = &bytes[n..],
                Err(_) => break,
            }
        }
        Ok(())
    }
}

/// SH-2 machine code for the integer benchmark (loops, arithmetic, conditional branches).
/// Reset vector 0: PC = 0x00000008; vector 1: SP = 0xFFFFBFA0.
static SH2_INT_BENCH: &[u8] = &[
    0x00, 0x00, 0x00, 0x08, // Vector 0: PC
    0xFF, 0xFF, 0xBF, 0xA0, // Vector 1: SP
    // 0x00000008:
    0xE0, 0x01, // mov #1, r0
    0xE1, 0x01, // mov #1, r1
    0xD2, 0x03, // mov.l @(12, pc), r2 -> load iteration count
    // Loop (0x0000000E):
    0x63, 0x03, // mov r0, r3
    0x33, 0x1C, // add r1, r3
    0x60, 0x13, // mov r1, r0
    0x61, 0x33, // mov r3, r1
    0x42, 0x10, // dt r2 (decrement r2, set T when r2 == 0)
    0x8B, 0xFA, // bf 0x0000000E (-6 words = 12 bytes back)
    0x00, 0x09, // nop
    0x00, 0x1B, // sleep
    0x00, 0x00, 0xC3, 0x50, // Literal: 50,000 iterations
];

/// SH-2E machine code for the FPU benchmark (FADD, FMUL, FSUB).
static SH2_FPU_BENCH: &[u8] = &[
    0x00, 0x00, 0x00, 0x08, // Vector 0: PC
    0xFF, 0xFF, 0xBF, 0xA0, // Vector 1: SP
    // 0x00000008:
    0xD0, 0x05, // mov.l @(literal_float, pc), r0
    0x40, 0x5A, // lds r0, fpul
    0xF0, 0x2D, // float fpul, fr0
    0xF1, 0x0D, // fsts fpul, fr1
    0xD2, 0x04, // mov.l @(literal_count, pc), r2 -> 20000 iterations
    // Loop (0x00000012):
    0xF0, 0x10, // fadd fr1, fr0
    0xF0, 0x12, // fmul fr1, fr0
    0xF0, 0x11, // fsub fr1, fr0
    0x42, 0x10, // dt r2
    0x8B, 0xFA, // bf 0x00000012
    0x00, 0x09, // nop
    0x00, 0x1B, // sleep
    0x3F, 0x80, 0x00, 0x00, // Literal: 1.0f (0x3F800000)
    0x00, 0x00, 0x4E, 0x20, // Literal: 20,000 iterations (x4 ops/iter = 80k FPU ops)
];

/// Throughput of a run: instructions / microseconds = MIPS (millions of instructions
/// per second), since the RP2040 timer counts microseconds.
fn mips(instructions: u64, micros: u64) -> f32 {
    if micros > 0 {
        instructions as f32 / micros as f32
    } else {
        0.0
    }
}

fn run_benchmarks<B: UsbBus>(serial: &mut SerialPort<'_, B>, timer: &hal::Timer, base_mhz: u32) {
    let mut w = UsbWriter { serial };
    let _ = writeln!(w, "\r\n==========================================================");
    let _ = writeln!(w, "  sh705x-emu SH705x core on Raspberry Pi RP2040 (bare-metal)");
    let _ = writeln!(w, "==========================================================");
    let _ = writeln!(w, "Board: YD-RP2040 (RP2040, dual Arm Cortex-M0+)");
    let _ = writeln!(w, "Target: thumbv6m-none-eabi | System clock: {} MHz", base_mhz);

    // TEST 1: integer ALU and control flow.
    let _ = writeln!(w, "\r\n[1/3] Integer benchmark (ALU + branches)...");
    let ram = sh_ram();
    ram.fill(0);
    let mut bus = EcuBus::new(SH2_INT_BENCH, ram);
    let mut cpu = sh2e::Cpu::new();
    cpu.power_on_reset(&mut bus);
    let t0 = timer.get_counter().ticks();
    let n1 = cpu.run(&mut bus, 500_000);
    let dt1 = timer.get_counter().ticks() - t0;
    let _ = writeln!(w, "  -> instructions: {}", n1);
    let _ = writeln!(w, "  -> elapsed: {} us ({} ms)", dt1, dt1 / 1000);
    let _ = writeln!(w, "  -> throughput: {:.2} MIPS", mips(n1, dt1));
    let _ = writeln!(w, "  -> emulated SH-2 cycles (min, no wait states): {}", cpu.cycles);

    // TEST 2: single-precision FPU, round-toward-zero in software.
    let _ = writeln!(w, "\r\n[2/3] FPU benchmark (FADD, FMUL, FSUB)...");
    let ram = sh_ram();
    ram.fill(0);
    let mut bus = EcuBus::new(SH2_FPU_BENCH, ram);
    let mut cpu = sh2e::Cpu::new();
    cpu.power_on_reset(&mut bus);
    let t0 = timer.get_counter().ticks();
    let n2 = cpu.run(&mut bus, 200_000);
    let dt2 = timer.get_counter().ticks() - t0;
    let _ = writeln!(w, "  -> instructions: {}", n2);
    let _ = writeln!(w, "  -> elapsed: {} us ({} ms)", dt2, dt2 / 1000);
    let _ = writeln!(w, "  -> throughput: {:.2} MIPS", mips(n2, dt2));

    // TEST 3: boot a real SH7059 flash image. Only with `real-boot`, and only with an image
    // you supplied; nothing ships here.
    #[cfg(feature = "real-boot")]
    {
        let _ = writeln!(w, "\r\n[3/3] Booting a real SH7059 flash image ({} bytes)...", ECU_FLASH.len());
        let ram = sh_ram();
        ram.fill(0);
        let mut bus = EcuBus::new(ECU_FLASH, ram);
        let mut cpu = sh2e::Cpu::new();
        cpu.power_on_reset(&mut bus);
        let _ = writeln!(w, "  -> reset PC (vector 0): 0x{:08X}", cpu.pc);
        let _ = writeln!(w, "  -> reset SP (vector 1): 0x{:08X}", cpu.r[15]);
        let t0 = timer.get_counter().ticks();
        let n3 = cpu.run(&mut bus, 50_000);
        let dt3 = timer.get_counter().ticks() - t0;
        let _ = writeln!(w, "  -> instructions: {}", n3);
        let _ = writeln!(w, "  -> elapsed: {} us ({} ms)", dt3, dt3 / 1000);
        let _ = writeln!(w, "  -> throughput: {:.2} MIPS", mips(n3, dt3));
        let _ = writeln!(w, "  -> PC after the run: 0x{:08X}", cpu.pc);
        let _ = writeln!(w, "  -> I/O reads / writes: {} / {}", bus.io_reads, bus.io_writes);
        // The early reset path spends much of its time polling peripherals this harness does
        // not model (the I/O reads above), so this throughput is not representative of steady
        // firmware work. And note it is instruction throughput only: the SH7059 runs at
        // ~40 MHz but this is not a cycle-accurate or wall-clock comparison.
        let io_share = if n3 > 0 { bus.io_reads as u64 * 100 / n3 } else { 0 };
        let _ = writeln!(w, "  -> note: {}% of these instructions were peripheral-polling I/O reads", io_share);
    }
    #[cfg(not(feature = "real-boot"))]
    {
        let _ = writeln!(w, "\r\n[3/3] Real-firmware boot: disabled.");
        let _ = writeln!(w, "  -> build with --features real-boot and your own image (flash/README.md).");
    }

    let _ = writeln!(w, "\r\n(send any character over the serial port to run again)");
    let _ = writeln!(w, "==========================================================\r\n");
}

#[entry]
fn main() -> ! {
    let mut pac = pac::Peripherals::take().unwrap();
    let _core = pac::CorePeripherals::take().unwrap();

    let mut watchdog = hal::Watchdog::new(pac.WATCHDOG);

    // Standard 125 MHz. Overclocking (and the voltage change it needs) is out of scope here.
    let clocks = hal::clocks::init_clocks_and_plls(
        12_000_000,
        pac.XOSC,
        pac.CLOCKS,
        pac.PLL_SYS,
        pac.PLL_USB,
        &mut pac.RESETS,
        &mut watchdog,
    )
    .ok()
    .unwrap();

    let sio = hal::Sio::new(pac.SIO);
    let pins = hal::gpio::Pins::new(
        pac.IO_BANK0,
        pac.PADS_BANK0,
        sio.gpio_bank0,
        &mut pac.RESETS,
    );

    let mut led = pins.gpio25.into_push_pull_output();
    let _ = led.set_high();

    let timer = hal::Timer::new(pac.TIMER, &mut pac.RESETS, &clocks);

    let usb_bus = UsbBusAllocator::new(hal::usb::UsbBus::new(
        pac.USBCTRL_REGS,
        pac.USBCTRL_DPRAM,
        clocks.usb_clock,
        true,
        &mut pac.RESETS,
    ));

    let mut serial = SerialPort::new(&usb_bus);
    let mut usb_dev = UsbDeviceBuilder::new(&usb_bus, UsbVidPid(0x2E8A, 0x000A))
        .device_class(2) // CDC
        .build();

    let mut last_blink = timer.get_counter().ticks();
    let mut led_state = true;
    let mut benchmarks_done = false;

    let base_mhz = clocks.system_clock.freq().to_MHz();

    loop {
        if !usb_dev.poll(&mut [&mut serial]) {
            let now = timer.get_counter().ticks();
            if now - last_blink > 500_000 {
                last_blink = now;
                led_state = !led_state;
                if led_state {
                    let _ = led.set_high();
                } else {
                    let _ = led.set_low();
                }
            }
        }

        let mut buf = [0u8; 64];
        let has_input = matches!(serial.read(&mut buf), Ok(count) if count > 0);

        if (!benchmarks_done && timer.get_counter().ticks() > 3_000_000) || has_input {
            benchmarks_done = true;
            let _ = led.set_high();
            run_benchmarks(&mut serial, &timer, base_mhz);
        }
    }
}
