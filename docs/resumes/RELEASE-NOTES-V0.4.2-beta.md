# 🚀 sh705x-emu Beta v0.4.2 — the replay drive, corrected; a whole-ROM map scan; and a J2534 door

A fix to `replay` so the turbo vanes follow the car instead of their stop; a scan of the
whole ROM for every map and curve; and a J2534 pass-thru DLL so GDS2 and other Windows
diagnostic tools talk to the emulated ECU as if through a real adapter.

> **Context:** five changes on top of 0.4.1, checked against the Astra J's own flash
> (A17DTJ, Denso, SH7059, NCLSW420AX36) and a drive recorded from the car.
>
> **Deliverables:**
> - `sh705x-v0.4.2-beta-windows-x86_64.zip`: `sh705x-gui.exe` and `sh705x.exe`, which
>   carry their own C runtime.
> - `app-arm64-v8a-release.apk` and `app-x86_64-release.apk`: versionCode 8, versionName
>   0.4.2. Signed with the same key as 0.3.x, 0.4.0 and 0.4.1, so they install over them.
> - `sh705x_j2534_x86.dll` and `sh705x_j2534_x64.dll` with `register.ps1`: the J2534
>   pass-thru DLL for 32-bit tools (GDS2) and 64-bit ones.
>
> **No firmware comes with any of them.**

---

## 1. `replay` drives the turbo right

- The plant builds manifold pressure as *resting pressure + the turbocharger's boost*.
  `replay` was feeding it the **recorded** manifold pressure as the resting value, so the
  boost was counted twice: the firmware saw too much pressure, opened the vanes to their
  stop, and the VGT command sat near 11 % against the car's ~58 %.
- It now feeds the **barometer** as the resting pressure, which is what "no boost" means.
  The vanes follow the car: the VGT median error drops from **30.2 % to 4.3 %**.

## 2. Comparing two images, and the per-cell table

- **`--compare <other.bin>`** replays the same route through a second flash and lists only
  the cells where the two images compute a different setpoint — rpm, pedal, setpoint,
  each image's value and the difference — or says they agree everywhere.
- **`--out <file.csv>`** writes the recorded and computed setpoint of every cell.
- Two more setpoints are read when the route logs them: **desired EGR** (`0x002C`) and the
  **EGR command** (`0x32A8`).
- **`tools/route_cells.py`** now groups the samples by polling round (a round ends when an
  identifier repeats or more than `--round-ms` has passed), so each cell is a real
  operating point rather than a stride of rows. Setpoint columns the route never logged
  are left empty.

## 3. `catalog`: every map in the ROM, read or not

- **`sh705x catalog`** (and the map browser in the GUI) scans the whole ROM for map and
  curve descriptors and lists them — axes, size, address — whether or not the firmware
  read them during the run. It finds the maps the live view cannot, because nothing has
  touched them yet.

## 4. `j2534`: GDS2 and other tools reach the emulated ECU

- A J2534 (04.04) pass-thru DLL whose "adapter" is the emulator's SM3 (`sh705x sm3`) over
  TCP. Registered with `register.ps1`, J2534 applications on the machine — or in a virtual
  machine that reaches it over the network — list it beside real adapters and talk CAN and
  ISO 15765 to the emulated ECU. The vendor's own DLL cannot: it pairs with the physical
  adapter first and takes no address.
- The DLL finds the SM3 at `SH705X_SM3`, or `server=host:port` in
  `%ProgramData%\sh705x\j2534.ini`, or `127.0.0.1:777`.

## 5. The SM3 opening handshake

- `sh705x sm3` now answers the opening exchange with the same lengths the car's Scanmatik
  gives, so tools that check them accept the emulated adapter.

## 6. The in-app updater lists the installers

- The Android updater checked for "install unknown apps" first and, without it, sent you to
  a settings screen — which some phones lock, dead-ending the update. It now offers the
  installer list first: **Install With Options** (through Shizuku) installs on its own
  authority and needs no such permission. The system installer is asked for only when it is
  the only option.

## 7. What was measured

On the Astra's flash, the Coruña–Santiago–Cangas route, 87 cells (558 494 samples):

| setpoint | median \|error\| | p90 \|error\| | constant-guess median |
|---|---|---|---|
| desired rail pressure | 3.03 MPa | 53.4 MPa | 28.05 MPa |
| desired air mass | 1.42 g/s | 8.30 g/s | 32.70 g/s |
| VGT | 4.32 % | 10.97 % | 9.02 % |
| desired boost | 17.50 kPa | 80.0 kPa | 57.69 kPa |
| torque | 35.0 N·m | 134.5 N·m | 102.25 N·m |

Rail pressure, air mass and now VGT land well inside a constant guess: the firmware
explains them. Boost and torque depend on the fuel injected, which is 0 in the emulator
without injection, so they do not match yet.

---

## 8. Upgrading

- **Windows:** unzip over the old folder.
- **Android:** install over 0.4.1; same key, same data. **Install this one by hand once:**
  phones on 0.4.1 or earlier carry the old in-app updater (§6), which cannot install it on
  a phone that locks "install unknown apps". Open the APK with Install With Options this
  time; from 0.4.2 on, the in-app updater works.
- **J2534:** place the two DLLs where you like and run `register.ps1` as administrator
  (see its header for the `-Dll32`/`-Dll64`/`-Server` arguments).

---

*Checked against an Opel Astra J 1.7 CDTI (A17DTJ) in the emulator. Windows programs built
by the release workflow from the tagged commit, checked to import no Visual C++ runtime.
Release APKs built from the tagged commit and signed with the same key as 0.3.x, 0.4.0 and
0.4.1 (certificate SHA-256 `5ff00c0a…f515c2`).*
