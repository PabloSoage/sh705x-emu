# sh705x-emu on the RP2040

A bare-metal port of the emulator's SH-2E core to a Raspberry Pi RP2040 (Arm Cortex-M0+,
`thumbv6m-none-eabi`, `#![no_std]`). The same `sh2e` core the desktop emulator uses runs on
a ~2 € microcontroller, reporting its results over USB CDC. It is a feasibility and
throughput study, not a product.

## What it is

- The `sh2e` core under [`firmware/src/sh2e/`](firmware/src/sh2e/) is the emulator's core,
  carried over unchanged except for module paths and one no_std adjustment (`f32::trunc` is
  unavailable, and the float-to-int cast already rounds toward zero, so the cast stands in
  for it — same result). Decode, execution, exceptions and the FPU are the full core.
- [`firmware/src/main.rs`](firmware/src/main.rs) is the RP2040 harness: a bus, a USB serial
  console, and three benchmarks.

## Resources

| | SH7059 needs | RP2040 (YD-RP2040) |
|---|---|---|
| Flash | 1.5 MB image | 16 MB external QSPI |
| RAM | 80 KB (`0xFFFE_8000..0xFFFF_C000`) | 264 KB internal SRAM |
| I/O registers | 16 KB | mapped in SRAM |

The 80 KB of emulated RAM sits in internal SRAM with room to spare; a flash image lives in
external QSPI flash (execute-in-place).

## Measured

On a YD-RP2040 at the stock 125 MHz, over USB CDC, timed with the 1 µs hardware timer.
Throughput is instructions ÷ microseconds (MIPS):

| benchmark | result |
|---|---|
| integer ALU + branches | 0.69 MIPS (500 000 instructions in 725 ms) |
| single-precision FPU (software, round-toward-zero) | 0.10 MIPS (100 000 instructions in 1023 ms) |
| real SH7059 boot¹ | 50 000 instructions in 84 ms, reset PC `0x00000C20` |

¹ Only with `--features real-boot` and your own image. These are **not** cycle-accurate or
wall-clock comparisons against the real ECU: the SH7059 runs at ~40 MHz, and the early boot
window is dominated by polling peripherals this harness does not model (the console prints
the I/O-read share). Treat the figures as instruction throughput of this core on this chip.

## Build and run

```
rustup target add thumbv6m-none-eabi
cargo install elf2uf2-rs            # once

# default: synthetic benchmarks only, no firmware image needed
cargo build --release
elf2uf2-rs -d target/thumbv6m-none-eabi/release/sh705x-rp2040

# optional: also boot a real SH7059 image you own (see firmware/flash/)
cargo build --release --features real-boot
```

Hold BOOTSEL while plugging the board in so it mounts as a drive, then run `elf2uf2-rs -d`.
Open the USB serial port (115200, any value — CDC ignores it); the benchmarks run 3 s after
boot, or on any character you send.

## Firmware images

No firmware image ships here. The `real-boot` feature reads one from
`firmware/flash/ecu.bin`, which is git-ignored; supply your own dump of a chip you own.
