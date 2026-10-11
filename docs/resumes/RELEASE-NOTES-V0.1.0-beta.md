# 🚀 sh705x-emu Beta v0.1.0 — an engine ECU's own firmware, running without the car

The first release: an emulator for Denso engine ECUs built on the Renesas SH705x, which
runs the ECU's own firmware, unmodified, on a simulated engine, on Windows and on Android.

> **Context:** the first target is the ECU of an Opel Astra J 1.7 CDTI (A17DTJ, Denso,
> SH7059, software NCLSW420AX36). Everything below was checked against that car's own
> flash and EEPROM, and where the car could answer, against the car.
>
> **Theme:** from a CPU core to an engine that idles. The firmware boots, talks
> diagnostics on its CAN bus, closes its own loops through simulated actuators, and holds
> the engine at idle on the torque it asks for, as the car does.
>
> **Deliverables:**
> - `sh705x-v0.1.0-beta-windows-x86_64.zip`: `sh705x-gui.exe` (the window) and
>   `sh705x.exe` (the command line). They carry their own C runtime: they run on any
>   64-bit Windows with nothing else installed.
> - `app-arm64-v8a-release.apk` (phones) and `app-x86_64-release.apk` (the Android
>   emulator, x86 devices): versionCode 1, versionName 0.1.0, minSdk 26, English and
>   Spanish. Signed with the same key as opendash and Rustify.
>
> **No firmware comes with any of them.** Bring your own flash dump (1.5 MB) and, if you
> have it, the EEPROM dump (8 KB).

---

## 1. The headline

| | What it does | Checked against |
|---|---|---|
| Core | Every SH-2E instruction, the FPU rounding toward zero as the chip does | Ghidra's disassembler on all 65 536 codes, an exact rational oracle, mutation testing |
| Boot | The Astra's flash from reset to its background loop | the same 196 669 register accesses as Ghidra's emulator |
| Diagnostics | OBD and GMLAN over its emulated CAN bus | the car: 231 recorded answers come back with the same shape, 108 byte for byte |
| Engine | Idles on the firmware's own torque | the car at warm idle: 750 rpm, 12 N·m, 7.0 mm³, EGR a third open, 5.2 g/s of air |
| Speed | 1.26 to 1.36 times faster than real time | i7-10870H, with the EEPROM, CAN and the engine turning |

---

## 2. The ECU, emulated

- **The chip.** The SH-2E core, and the on-chip peripherals the firmware uses: the
  interrupt controller, timers (CMT), DMA, serial ports, the A/D converter, HCAN-II and
  the ATU-II's input captures, tooth channel, one-shot pulses and PWM outputs. The
  firmware boots with no status bit forced.
- **The board.** The four 93C86 EEPROMs, with the order of their blocks proven from a
  record that only fits one way; the status chips that watch the output stages, so no
  fault is pending at idle; the injector driver on SCI0. A PCMflash dump is served as it
  is.
- **The car around it.** A 60-4 crank wheel and a 4+1 cam wheel, every sensor in
  physical units through the curve the firmware itself applies, the MAF as a frequency,
  and the other nodes on the bus (the ABS, and the rest of the car standing still).

---

## 3. An engine that idles on its own

Plants for the suction control valve, the EGR valve and the fresh air it leaves, and the
turbocharger close the firmware's own loops. With them, a simulated crankshaft turns on
the torque the firmware asks for, and its idle governor settles where the car does:

| | Emulator | The car, warm idle |
|---|---|---|
| Engine speed | 750 rpm | 750 rpm |
| Torque / fuel asked | 12.0 N·m / 7.02 mm³ | 12.0 N·m / 7.0 mm³ |
| EGR / fresh air | 33 % open / 5.2 g/s | a third open / 5.2 g/s |
| SCV current | 2249 mA for 2250 asked | on its setpoint |

---

## 4. Diagnostics, from your usual tools

- **ELM327 over TCP** (`sh705x serve`): any diagnostic app that talks to an ELM327 over
  Wi-Fi can be pointed at the emulator.
- **An emulated Scanmatik SM3** (`sh705x sm3`): opendash connects to it as it would to
  the real adapter.
- **A console** in the window, in virtual time.

OBD modes 01, 03 and 09 (the VIN, over ISO-TP) and GMLAN service 1A answer as the car
does. Faults can be provoked: an output stage reporting a short (`--status`), a sensor out
of range (`--adc`).

---

## 5. Comparing calibrations

- **`sh705x maps`** runs the same speed × pedal grid against two flash images, on the
  plants, and shows where the second differs: fuel and torque, rail pressure, SCV, air
  flow, boost, VGT, EGR, throttle, load.
- **`sh705x touched`** lists the maps the firmware reads in a scenario, with their inputs
  and results, named in words, marking those a second image changes.

---

## 6. Watching it work

- **The window.** It opens on a load screen: choose the images with the system's dialog,
  type their paths, or drop the files onto it. Then registers, the code around the PC
  with breakpoints, memory, CAN traffic, interrupts, sensors as sliders, and the console.
- **Step by step.** An instruction, a crank tooth or an input edge at a time, and slow
  motion down to a thousandth of real time.
- **The engine view.** The crank and cam wheels turning under their sensors, the four
  cylinders, and charts in ECU time of what the firmware asks for against what the plants
  give back. Each column keeps its minimum and maximum, so a move faster than a pixel
  still shows: the EGR drive stepping every few milliseconds is there at full speed.

---

## 7. On Android

The same core, in Kotlin and Compose: the engine view on a phone.

- **Loading.** Pick the dumps with the system's file picker. The app asks for no
  permissions and keeps the dumps in memory only.
- **Running.** "Idle on its torque" starts the engine as on the desktop; slow motion,
  "Next tooth" and "Next edge" work the same.
- **Theme.** It starts in a dark theme; light, or the system's, is a tap away.

On the Android emulator (x86_64, software GPU) it holds 750 rpm at 0.66 times real time.
**How fast it runs on a real phone has not been measured yet.**

---

## 8. What it does not do, and why

- **It does not inject.** The ECU will not inject until the immobilizer authorizes it,
  and that side is not emulated, by choice: the emulator shows what the ECU does
  without that authorization, as with a key it does not recognise. The rail pressure is
  therefore the sensor as set, not a combustion, and with the car's EEPROM the idle rises
  to 1000 rpm (P160D).
- **The cylinders' phase is assumed.** Where cylinder 1's top dead centre falls relative
  to the gap is not known; the engine view says so.
- **Not yet:** checking it against a real ECU on an FPGA-driven bench, and the ECU itself
  on an FPGA.

---

## 9. Installing

- **Windows:** unzip and run `sh705x-gui.exe`. `sh705x.exe help` lists the command line.
- **Android:** install the APK for your device. It is the first version, so there is
  nothing to update from.
- **From source:** `cargo build --release`; the Android app with `./gradlew
  :app:assembleRelease` in `android/`, with the NDK and cargo-ndk (see the README).

Licensed GPL-3.0-or-later.

---

*Checked against an Opel Astra J 1.7 CDTI (A17DTJ): its flash and EEPROM dumps, a
diagnostic sweep of the car's engine ECU and its GMLAN identifiers recorded from the car,
and its live values at warm idle. Windows programs built by the release workflow from the
tagged commit, checked to import no Visual C++ runtime. Release APKs built from the tagged
commit, signed with opendash's and Rustify's key (certificate SHA-256
`5ff00c0a…f515c2`, the same in all three), installed and started on the Android
emulator.*
