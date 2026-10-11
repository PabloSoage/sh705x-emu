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
| 1 | SH-2E core (integer and FPU) and memory map, in Rust | done |
| 2 | Interrupt controller and on-chip peripherals (ATU-II, CMT, WDT, DMAC, SCI, A/D, HCAN) | in progress: INTC, CMT, DMAC, SCI, A/D, HCAN and most of the ATU-II done; the rest needs the engine |
| 3 | The Denso board: EEPROM, external chips, pin-to-sensor map | in progress: the four EEPROMs served; 25 of the 32 analog inputs named, the MAF found on a timer input |
| 4 | Diagnostics over the emulated CAN bus | done for reading: OBD and GMLAN through an ELM327 on TCP and an emulated Scanmatik SM3 |
| 5 | Simulated engine | in progress: crank and cam wheels, sensors in physical units, the ABS on the bus; no injection yet |
| 6 | Validation against a real ECU, with an FPGA-driven bench | — |
| 7 | Calibration comparison and map-access tracing | started: setpoints over a speed × pedal grid, two images compared; the maps looked up in a scenario |
| 8 | The ECU on an FPGA (SH-2 soft core with an SH-2E FPU) | — |

### Phase 0 in short

On the Astra dump, Ghidra's p-code emulator runs from reset to the firmware's
background loop, with interrupts enabled, in 2.44 M instructions and with no
peripheral modelled. On the way it busy-waits on a hardware status bit 19 times: the NMI
pin, SCI transmit status, DMA end, A/D end and ATU-II compare flags. Beyond that point
nothing happens without timer interrupts. Ghidra executes about 37,000 instructions per
second, which makes it a reference oracle rather than an emulator.

### Where it stands

- **Core:** every SH-2E instruction, delay slots, exceptions and interrupts, and an FPU
  that rounds toward zero exactly. Checked against Ghidra's disassembler on all 65,536
  codes, against an exact rational oracle for the FPU, and by mutation testing.
- **The Astra's boot** makes the same 196,669 register accesses as Ghidra's emulator,
  from reset to the background loop.
- **With the CMT and the interrupt controller**, the firmware takes its 1 ms tick and
  starts its periodic work: A/D conversions, CAN mailboxes, serial traffic.
- **With the DMAC, SCI, A/D and ATU-II modelled**, the firmware boots and runs with no
  status bit forced: it passes its timer self-tests, fills RAM by DMA, and takes compare
  match interrupts from the ATU-II as it runs.
- **The board's EEPROM** is four 93C86 Microwire chips on the synchronous serial ports,
  each with its own chip select; the firmware reads all four at boot. A PCMflash dump
  can be served as it is.
- **On CAN,** with the ignition input up, the firmware broadcasts its periodic GMLAN
  frames and answers diagnostic requests: OBD modes 01, 03 and 09 (the VIN, over
  ISO-TP), and GMLAN service 1A. The VIN it returns is the car's.
- **The engine turns:** a 60-4 crank wheel and a 4+1 cam wheel on the ATU-II's input
  captures, and the ECU reports the speed it is set to, steadily from idle to 4500 rpm.
  The sync tooth has to come 15 to 45 crank degrees before a main tooth for the
  firmware to keep in sync; where it is on the real engine is not known.
- **Sensors in physical units:** for each analog input, the curve the firmware itself
  applies, measured on the emulated ECU, turns a value (coolant 90 degC, rail 35 MPa,
  boost 180 kPa) into the code that makes the ECU report it. The boost sensor is
  ratiometric to one of the two 5 V references, which the firmware checks. The MAF is
  a frequency on TI0C. The defaults are the car's own values at warm idle.
- **Other nodes on the bus:** of the 20 identifiers the firmware listens to, the ABS
  frame is the one whose absence it reports (U0121); it is sent by default.
- **Debugging:** memory watchpoints, PC breakpoints and single stepping in the core,
  and a disassembler.
- **Speed:** with the EEPROM, CAN and the engine turning, the emulated ECU runs 1.26 to
  1.36 times faster than real time (about 90 M instructions per second, i7-10870H);
  the SH7059 runs at 80 MHz, about 69 M instructions per second of ECU time. Flash is
  decoded once, and flash and RAM accesses take a direct path.
- **The engine runs on the firmware's torque** (`sh705x plant`): a simulated crank turns
  with the torque the firmware asks for, and its idle governor settles at 750 rpm, 12 N·m
  and 7.0 mm³ a stroke, as the car does at warm idle. The SCV current loop and the EGR,
  through an air-mass loop, close against plant models from power-on: the EGR settles a
  third open for 5.2 g/s of fresh air, as in the car.
- **The turbocharger:** the VGT (7A) and the boost the car makes for each speed and fuel;
  the firmware's boost loop closes on it (`sh705x plant --sensor pedal=30`).
- **The board's output monitors:** the status chips the firmware reads on SCI3 and SCI4
  and two fault lines on port J are modelled, so its output stage checks pass: no fault
  is pending at idle, with or without the EEPROM.
- **Not yet:** the ECU injects nothing while the immobilizer, whose other side is not
  emulated, keeps it shut; with the car's EEPROM it also raises the idle to 1000 rpm for
  it (P160D). The swirl flaps have no plant yet. What can be
  modelled without injection is, from the firmware's own demands.

## Layout

| Path | Contents |
|---|---|
| `crates/sh2e` | SH-2E core: registers, reset, and the `Bus` trait it runs against |
| `crates/sh7059` | The SH7059: address map, flash, RAM, register area, peripherals (INTC, CMT, DMAC, SCI, A/D, ATU-II, HCAN-II), debugging aids, and `Machine`, which runs CPU and chip on one clock |
| `crates/board` | The ECU board: the four 93C86 EEPROMs, the output status chips and fault lines, and how they are wired |
| `crates/engine` | The simulated engine: crank and cam wheels, sensors with the firmware's A/D curves, the MAF, periodic frames of other CAN nodes |
| `crates/bridge` | The CAN bus as a diagnostic tool sees it, in virtual or real time; ISO-TP; an ELM327; a TCP server; an emulated Scanmatik SM3 |
| `crates/sh705x` | Command-line front end |
| `crates/gui` | `sh705x-gui`: registers, code with breakpoints, memory, CAN, interrupts, the engine (speed and sensors in physical units) and an ELM327 console, live (egui) |
| `tools/bench.py` | Interleaved speed comparison of builds |
| `tools/gen_curves.py` | Generates `crates/engine/src/curves.rs` from a sweep of the A/D inputs |
| `tools/ghidra/EmuBoot.py` | Phase 0: runs a raw flash image from its reset vector in Ghidra's p-code emulator, logs every I/O access and resolves status-bit polls automatically |
| `tools/ghidra/emu_boot.ps1` | Runs `EmuBoot.py` headless in a throwaway project |

## Building and running

```sh
cargo build --release
cargo run --release -p sh705x -- info --flash <flash.bin> --eeprom <eeprom.bin>
```

`info` loads both images, checks their sizes, prints the reset vectors with the region
each one points into, and the CPU state after a power-on reset. `trace` runs the
firmware and writes every peripheral register access; `disasm` disassembles the flash;
see `sh705x help`.

### Talking to the emulated ECU

```sh
# Boot it and send ELM327 commands, in virtual time (deterministic, no wall clock):
sh705x obd --flash <flash.bin> --eeprom <eeprom.bin> 0100 0902 ATSH7E0 1A92

# Run it in real time and serve an ELM327 on 127.0.0.1:35000 for diagnostic apps:
sh705x serve --flash <flash.bin> --eeprom <eeprom.bin>
```

For [opendash](https://github.com/PabloSoage/opendash), which talks to a Scanmatik SM3
rather than an ELM327, there is an emulated SM3: run

```sh
sh705x sm3 --flash <flash.bin> --eeprom <eeprom.bin>
```

and set opendash's adapter address to this machine's IP (port 777), on the same
network.

To watch it run:

```sh
cargo run --release -p gui -- --flash <flash.bin> --eeprom <eeprom.bin>
```

All of them turn the ignition input on (`--adc 14:600`); without it the firmware stays
silent on CAN. Frames from the tool reach the ECU after their time on a 500 kbit/s bus,
as on the car: the firmware drops a flow control that comes back faster than that.

The engine and its sensors:

```sh
# Idling at 750 rpm, sensors at the car's warm idle values, the ABS on the bus:
sh705x obd --flash <flash.bin> --eeprom <eeprom.bin> --rpm 750 010C 0105 07

# Any sensor in physical units (an unknown key lists them), or a raw A/D code:
sh705x serve --flash <flash.bin> --eeprom <eeprom.bin> --rpm 2000 --sensor coolant=90 --sensor maf=40

# How fast it runs against real time, with the engine turning:
sh705x bench --flash <flash.bin> --eeprom <eeprom.bin> --rpm 800
```

`--sensors off` leaves every analog input at code 0 and `--peers none` takes the ABS
off the bus.

### Comparing calibrations

```sh
# What the ECU asks for over a grid of engine speed and pedal, and where a second image
# differs: fuel injected and asked for, torque delivered, desired rail pressure, SCV current (measured)
# and duty command, desired air flow and boost, VGT vane command, desired EGR and
# throttle, load.
sh705x maps --flash <original.bin> --compare <modified.bin> --eeprom <eeprom.bin>
```

And which maps the firmware reads in a scenario, through its interpolation routines
(found with the map catalogue's Ghidra tools), with their inputs and results, marking
those a second image changes. With `--labels` each map is also named in words (VNT duty
cycle, target air mass, torque-to-IQ, rail pressure limiters...):

```sh
sh705x touched --flash <original.bin> --compare <modified.bin> --eeprom <eeprom.bin>     --rpm 2500 --sensor pedal=60 --masters 3d=7128,2d=70B4 --catalog <maps-3d.csv> --labels <tags.txt>
```

Each point of `maps` holds the engine at that speed, as on a dyno, while the ECU boots
with the plants around it (EGR, fresh air, SCV, turbocharger), and reads the setpoints
once their loops have settled. They come from the calibration's maps, so a changed map
shows where the engine is on it. As long as the ECU does not inject, the fuel it injects
reads 0; the fuel it asks for, read from RAM, is the number a fuel map change moves.

### Actuators that run without injection

Two of the ECU's loops regulate whatever the fuel cut does: the suction control valve's
current (ATU-II output 6A, read back on AN1) and the EGR valve's position (an H-bridge
on 7D and 7C, read back on AN10). `actuators` powers the ECU on with the engine held at
idle speed, drives a physical plant with the firmware's duties and returns the plant's
output to those inputs every millisecond, so the firmware's own regulators close against
it, and traces the result with the VGT command (7A):

```sh
sh705x actuators --flash <flash.bin> --eeprom <eeprom.bin> --millis 4000
```

The SCV current loop holds its own setpoint (2250 mA asked, 2249 measured). With the
engine held, the idle governor asks for a lot of fuel and the firmware shuts the EGR, as
the car does under load; `plant` lets the speed go and shows the EGR loop at idle.

### The engine on the firmware's own torque

With the immobiliser shut no injector fires, but the firmware still works out the fuel it
wants and the torque that fuel would give. `plant` turns a simulated crankshaft with that
torque, the crank wheel following its speed tooth by tooth, so the firmware's idle
governor and pedal map close their loop through the simulation. Nothing in the firmware
is changed, and nothing here makes a real engine run.

```sh
# Idle (the default), or rev it in neutral with --sensor pedal=30; --load in N·m
sh705x plant --flash <flash.bin> --millis 15000
```

At warm idle it settles at 750 rpm, 12 N·m and 7.0 mm³ a stroke, with the EGR 33 % open
and 5.2 g/s of fresh air: what the car reports standing still. With the car's EEPROM
the firmware also checks the immobilizer, and its reaction raises the idle to 1000 rpm;
the trace shows the target it aims at.

### Tests

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
- **Built to be watched.** The core will expose pause, single step and breakpoints, so
  a graphical front end can show registers, peripherals and the engine live.
- **Reference:** *SH-2E SH7059 F-ZTAT, SH7058S F-ZTAT Hardware Manual*, Renesas
  REJ09B0177-0500, Rev. 5.00.

## Firmware

**No firmware is, or will be, in this repository.** ECU flash and EEPROM images are
copyrighted by their manufacturers; bring your own dump. The `.gitignore` refuses the
usual image formats.

This project is for understanding and testing ECUs. It is not a tool for disabling
emission control systems.
