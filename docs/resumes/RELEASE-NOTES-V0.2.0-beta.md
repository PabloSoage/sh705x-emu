# 🚀 sh705x-emu Beta v0.2.0 — the calibration maps, as the firmware reads them

Every map the ECU reads, drawn whole from its own descriptor, with where the firmware is
on it right now, where each of its inputs comes from and where its result goes: on
Windows and on Android, live, in slow motion, or a lookup at a time.

> **Context:** four commits on top of 0.1.0, checked against the Astra J's own flash
> (A17DTJ, Denso, SH7059, NCLSW420AX36).
>
> **Theme:** 0.1.0 let you watch the engine; this one opens the calibration. A remap tool
> shows a map as a table; here you see the firmware read it: which cell, from which line
> of code, with which variables, and what it does with the result.
>
> **Deliverables:**
> - `sh705x-v0.2.0-beta-windows-x86_64.zip`: `sh705x-gui.exe` and `sh705x.exe`, which
>   carry their own C runtime.
> - `app-arm64-v8a-release.apk` and `app-x86_64-release.apk`: versionCode 2, versionName
>   0.2.0. Signed with the same key as 0.1.0, so they install over it.
>
> **No firmware comes with any of them**, and no catalogue of names either: those you
> load from your own files.

---

## 1. The headline

| | 0.1.0 | 0.2.0 |
|---|---|---|
| Calibration maps | listed by `sh705x touched`, on the command line | **drawn live** in the window and the app: heat map, surface, table, curve |
| Where the firmware is on a map | — | the cell it reads, now, and the trail of its last lookups |
| Where a value comes from | — | each input traced to a variable, a routine or another map |
| Names | — | the sensors found by moving them; an opendash profile read at run time |
| Stopping on a map | a breakpoint by hand | **"Stop at its next lookup"** |

---

## 2. Every lookup, without stopping the ECU

Denso firmware reads its maps through a few interpolation routines that take the map's
descriptor in R4 and its inputs in FR4 and FR5, and return the result in FR0. The
emulator now:

- **finds those routines in the flash by itself**, by counting which call targets get a
  valid descriptor: on the Astra, `0x7128` (3D) and `0x70B4` (2D), the same the map
  catalogue's Ghidra tools found;
- **probes them without stopping the machine**: every call is recorded with its
  registers on the way in and FR0 on the way out. With the maps open it runs about 6 %
  slower (1.23 → 1.16 times real time, best of six runs each); closed, the run loop is
  the same as before;
- **reads each map from its own descriptor**, in physical units: sizes, axes, the cell
  type (float, u8, u16, s8, s16) and the scale and offset it carries.

**Checked:** at idle the Astra reads **594 maps**, all decoded, and on all **18 352
lookups** of half a second the result the firmware gets is exactly the decoded map's
interpolation.

---

## 3. Where each value comes from

For each place a map is read from, the code around the call is read: back from the call
to where each input register was loaded, and forward to where the result is stored. An
input is a variable in RAM, the result of a routine (the engine speed), or the result of
another map, which becomes a link to it; and each map lists the maps that read what it
gives.

**Checked live:** stopped at 5000 real lookups across 616 call sites, **2617 of the 2625
variables named as an input hold exactly the value the firmware passes** (99.7 %). The
others are routine results (mostly the engine speed) or other maps: the chain from
"fuel → torque" (`09CEB8`) into the next map shows up on its own.

---

## 4. Names

- **With the emulator:** what this project found in the Astra's calibration (the engine
  speed routine, the torque and fuel asked, the setpoints of the engine view), used only
  when the flash is that calibration.
- **"Find the sensors":** every sensor is moved one way and the other for a moment, and
  the variables that follow it in all three states take its name: 16 sensors on the
  Astra. The 5 V references and the oil switch are left alone, and the engine is put
  back: it idles where it did and **no fault is left behind** (checked).
- **"Read an opendash profile…":** a parameter profile (opendash's `profiles.txt`); the
  emulated ECU is asked for each identifier in three engine states, and the variable that
  holds the answer every time takes its name. Checked against two this project found
  another way: *Desired Fuel Rail Pressure* lands on `FFFEF924` and *Torque Delivered* on
  `FFFEFC4C`. Parameters that read 0 without injection cannot be placed.
- **"Read map labels…":** labels by descriptor.

Profiles and labels are read from your disk at run time and **never kept with the
emulator**: such catalogues are often someone else's data.

---

## 5. On Windows

"Maps" opens a window with the list (filter by address, name or 2D/3D; follow the latest
lookup), the map in four views, its call sites as links into the code view, and "Stop at
its next lookup", which runs the ECU and stops where the firmware next reads that map, to
go on an instruction or a tooth at a time.

## 6. On Android

A "Maps" tab beside the engine: the same list, the map as a heat map (tap a cell for its
value), a surface turned with a finger, a table or a curve, where each value comes from,
the names, and "Stop at its next lookup". The maps are traced only while the tab is open.

---

## 7. The rest

- **Tests** for the new parts: the probes on a hand-written program; descriptors read and
  refused; the masters, the decoding and the call sites checked against the Astra's
  flash; a stop at one map passing all the others; naming that leaves no fault; the map
  window drawn in every view; the app's map layout on the JVM.
- **CI** builds and tests both, as before.
- **The repository is public**, under GPL-3.0-or-later.

---

## 8. Upgrading

- **Windows:** unzip over the old folder.
- **Android:** install over 0.1.0; same key, same data.

---

*Checked against an Opel Astra J 1.7 CDTI (A17DTJ): its flash, at warm idle in the
emulator, and its own diagnostic answers for the profile's identifiers. Windows programs
built by the release workflow from the tagged commit, checked to import no Visual C++
runtime. Release APKs built from the tagged commit and signed with the same key as 0.1.0
(certificate SHA-256 `5ff00c0a…f515c2`).*
