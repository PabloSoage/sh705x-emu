# 🚀 sh705x-emu Beta v0.4.1 — a recorded drive against the firmware, and standard names

A `replay` command that drives the firmware through every operating point of a recorded
route and says how far each of its setpoints lands from what the car asked for; the SAE
name for every standard OBD-II PID; and `maps` grids that say at a glance what they hold.

> **Context:** three additions on top of 0.4.0, checked against the Astra J's own flash
> (A17DTJ, Denso, SH7059, NCLSW420AX36) and a 224 000-sample drive recorded from the car.
>
> **Deliverables:**
> - `sh705x-v0.4.1-beta-windows-x86_64.zip`: `sh705x-gui.exe` and `sh705x.exe`, which
>   carry their own C runtime.
> - `app-arm64-v8a-release.apk` and `app-x86_64-release.apk`: versionCode 7, versionName
>   0.4.1. Signed with the same key as 0.3.x and 0.4.0, so they install over them.
>
> **No firmware comes with any of them.**

---

## 1. `sh705x replay`

```sh
python tools/route_cells.py <route.csv.gz> --out cells.csv
sh705x replay --flash <flash.bin> --eeprom <eeprom.bin> --cells cells.csv
```

- **`tools/route_cells.py`** (standard library only) bins a recorded route by engine speed
  and pedal travel and keeps the median of each input and setpoint per cell. It turns the
  pedal's sensor 1 into travel with the ECU's own calibration (19.6 % at rest, 70 % at full).
- **`replay`** sets each cell's pedal, coolant, intake air, boost, barometric pressure and
  air mass, lets the firmware settle, and compares five setpoints with the recorded ones:
  desired rail pressure, desired boost, VGT, desired air mass and torque. The cells run in
  parallel, as `maps` does.

## 2. Standard PID names

- A parameter profile is a generic list and can hang another module's label on a low
  identifier. For `0x00`–`0xFF` the emulator now uses the fixed SAE J1979 name instead
  (`0x000C` → "Engine speed", `0x000B` → "Intake manifold absolute pressure").
- Higher identifiers keep the profile's name. It shows wherever variables are named: the
  maps window on Windows and the maps tab on Android.

## 3. `maps` says what a grid holds

- Under each setpoint: empty, all zero, constant at a value, or the range it spans. "The
  same at every point" now carries the value, so a grid of zeros reads as such.

## 4. What was measured

On the Astra's flash, a route of 59 cells (223 979 samples, Coruña–Santiago–Cangas):

| setpoint | median \|error\| | p90 \|error\| |
|---|---|---|
| desired rail pressure | 1.89 MPa | 32.3 MPa |
| desired air mass | 2.08 g/s | 8.30 g/s |
| desired boost | 15.6 kPa | 56.9 kPa |
| VGT | 30.2 % | 48.6 % |
| torque | 27.0 N·m | 80.0 N·m |

Rail pressure and air mass follow the car. Boost, VGT and torque depend on the fuel
injected, which is 0 in the emulator without injection, so they do not match yet.

---

## 5. Upgrading

- **Windows:** unzip over the old folder.
- **Android:** install over 0.4.0 (or let the app's update detector find it); same key,
  same data.

---

*Checked against an Opel Astra J 1.7 CDTI (A17DTJ) in the emulator. Windows programs built
by the release workflow from the tagged commit, checked to import no Visual C++ runtime.
Release APKs built from the tagged commit and signed with the same key as 0.3.x and 0.4.0
(certificate SHA-256 `5ff00c0a…f515c2`).*
