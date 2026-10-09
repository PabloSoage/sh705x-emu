# sh705x-emu

An emulator for engine control units built on the Renesas SH705x microcontrollers
(SH-2E core). It runs the ECU's own firmware, unmodified, against modelled on-chip
peripherals and a simulated engine, so the ECU can be studied without the car.

The first target is a Denso ECU with an SH7059 (Opel Astra J, A17DTJ engine).

## Goals

- **Diagnostics:** talk to the emulated ECU over its emulated CAN bus with ordinary
  diagnostic tools, starting with an ELM327 bridge.
- **Calibration testing:** run the same scenario against two flash images and compare
  what the ECU does.
- **Your own ECU:** load a flash dump (1.5 MB) and an EEPROM dump (8 KB) of a specific
  unit.
- **Engine behaviour:** close the loop with a simulated engine — crankshaft and camshaft
  signals, sensors, actuators — and watch injection, EGR, boost and rail pressure respond.

## Status

| Phase | What | State |
|---|---|---|
| 0 | Boot in Ghidra's p-code emulator: find where the firmware waits on hardware | done |
| 1 | SH-2E core (integer and FPU) and memory map, in Rust | next |
| 2 | Interrupt controller and on-chip peripherals (ATU-II, CMT, WDT, DMAC, SCI, A/D, HCAN) | — |
| 3 | The Denso board: EEPROM, external chips, pin-to-sensor map | — |
| 4 | Diagnostics over the emulated CAN bus | — |
| 5 | Simulated engine | — |
| 6 | Validation against a real ECU, with an FPGA-driven bench | — |
| 7 | Calibration comparison and map-access tracing | — |
| 8 | The ECU on an FPGA (SH-2 soft core with an SH-2E FPU) | — |

### Phase 0 in short

On the Astra dump, Ghidra's p-code emulator runs from reset to the firmware's
background loop, with interrupts enabled, in 2.44 M instructions and with no
peripheral modelled. On the way it busy-waits on a hardware status bit 19 times: the NMI
pin, SCI transmit status, DMA end, A/D end and ATU-II compare flags. Beyond that point
nothing happens without timer interrupts. Ghidra executes about 37,000 instructions per
second, which makes it a reference oracle rather than an emulator.

## Layout

| Path | Contents |
|---|---|
| `crates/sh2e` | SH-2E core: registers, reset, and the `Bus` trait it runs against |
| `crates/sh7059` | The SH7059: address map, flash, RAM and the peripheral register area |
| `crates/board` | The ECU board: EEPROM image now; external chips and pin map later |
| `crates/engine` | The simulated engine (phase 5, empty) |
| `crates/bridge` | Virtual CAN bus and diagnostic tool bridges (phase 4, empty) |
| `crates/sh705x` | Command-line front end |
| `tools/ghidra/EmuBoot.py` | Phase 0: runs a raw flash image from its reset vector in Ghidra's p-code emulator, logs every I/O access and resolves status-bit polls automatically |
| `tools/ghidra/emu_boot.ps1` | Runs `EmuBoot.py` headless in a throwaway project |

## Building and running

```sh
cargo build --release
cargo run --release -p sh705x -- info --flash <flash.bin> --eeprom <eeprom.bin>
```

`info` loads both images, checks their sizes, prints the reset vectors with the region
each one points into, and the CPU state after a power-on reset.

```sh
cargo test --all
cargo clippy --all-targets -- -D warnings
cargo fmt --all --check
```

Tests that need a real dump read it from `SH705X_FLASH` (and `SH705X_EEPROM`) and are
skipped when those are not set.

The phase 0 tool needs Ghidra 12 (`GHIDRA_INSTALL_DIR` or `-Ghidra`):

```powershell
.\tools\ghidra\emu_boot.ps1 -Flash <flash.bin> -Out boot.report.txt -Max 15000000
```

## Design notes

- **Own interpreter, in Rust.** No existing emulator models the SH-2E: its single-precision
  FPU rounds toward zero only, which neither the host FPU nor Ghidra does by default.
- **Real time is the target.** The emulated ECU should run at least as fast as the real
  one: the SH7059 runs at up to 80 MHz, so up to 80 million instructions per second.
  The core is built for that from the start: the bus is a generic parameter (no dynamic
  dispatch per access), flash and RAM sit on a fast path apart from the peripherals,
  and instructions will be pre-decoded. A phase 8 FPGA port is the route to hard real
  time.
- **Virtual time.** Scenarios run on an event queue and are reproducible bit for bit;
  wall-clock pacing is only used when a real tool is attached to a bridge.
- **Built to be watched.** The core exposes pause, single step and breakpoints, so a
  graphical front end can show registers, peripherals and the engine live.
- **Reference:** *SH-2E SH7059 F-ZTAT, SH7058S F-ZTAT Hardware Manual*, Renesas
  REJ09B0177-0500, Rev. 5.00.

## Firmware

**No firmware is, or will be, in this repository.** ECU flash and EEPROM images are
copyrighted by their manufacturers; bring your own dump. The `.gitignore` refuses the
usual image formats.

This project is for understanding and testing ECUs. It is not a tool for disabling
emission control systems.
