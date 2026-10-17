# 🚀 sh705x-emu Beta v0.4.3 — compare two flashes, change a map and watch the firmware use it; GDS2 sees live data

The map browser learns what calibration editors do: compare this flash with another file,
copy or save a map, and change a cell. The change goes into the emulated ECU, and its next
lookups use it. The J2534 DLL now delivers the frames GDS2's Data Display streams on.

> **Context:** four changes on top of 0.4.2, checked against the Astra J's own flash
> (A17DTJ, Denso, SH7059, NCLSW420AX36), and with GDS2 in a virtual machine.
>
> **Deliverables:**
> - `sh705x-v0.4.3-beta-windows-x86_64.zip`: `sh705x-gui.exe` and `sh705x.exe`, which
>   carry their own C runtime.
> - `app-arm64-v8a-release.apk` and `app-x86_64-release.apk`: versionCode 9, versionName
>   0.4.3. Signed with the same key as 0.3.x and 0.4.x, so they install over them.
> - `sh705x_j2534_x86.dll` and `sh705x_j2534_x64.dll` with `register.ps1`.
>
> **No firmware comes with any of them.**

---

## 1. Comparing with another flash

- **"Compare with a flash…"** reads a second image from your disk. Each map is read from
  the same descriptor in both files; maps whose cells live in RAM, or whose shape differs
  in the other file, are marked as not comparable.
- The list marks each map that differs with its changed-cell count (`Δ100`), and the
  filter **`DIFF`** keeps only those.
- **"Differences"** shows each cell as `this → other`, coloured by the size of the change,
  with the count and the largest change above.
- The other file is kept only in memory; "Stop comparing" drops it.

## 2. Copying and saving a map

- **"Copy"** puts the map on the clipboard as tab-separated text, axes included, ready for
  a spreadsheet. **"Save CSV…"** writes the same with commas.

## 3. Changing a map in the emulated ECU

- In **"Table"**, click a cell, type the value in physical units and press **Set** (or
  Enter). It is encoded in the map's own format: float, or 8/16 bits with the descriptor's
  scale and offset. A value out of range is refused, and the limits are shown.
- It is written to the emulated flash and, when the firmware reads that map from a RAM
  copy, to that copy too. The maps shown and the lookup checks are re-read, so no false
  "mismatch" appears.
- Changed cells are underlined. **"Revert this map"** puts back the loaded flash's bytes.
  **"Save flash as…"** writes the image with the changes. Its checksum is not recomputed:
  it is for the emulator and for study, not for an ECU.
- **Checked end to end** on the Astra's flash: with the boost target (`0x9DADC`) raised
  10 % while idling, each later lookup returns exactly the changed map's interpolation
  (test `a_changed_map_is_what_the_firmware_reads`, which runs when `SH705X_FLASH` is set).
- Only the emulator's memory changes. Nothing is sent to a car.

## 4. J2534: GDS2's Data Display streams live

- GDS2 opens an ISO 15765 channel with a flow-control filter for the ECU's replies and a
  **pass filter** for `0x5E8`, where the ECU streams the periodic data it was asked for.
  Those frames are single CAN frames outside any flow-control pair, and the DLL dropped
  them, so GDS2 reported lost communication. Raw frames that only a pass filter claims are
  now delivered. GDS2's Data Display then shows the emulated engine's parameters live, at
  1000 rpm as set in the emulator.

## 5. Smaller fixes

- A map the whole-ROM scan finds no master call for, but which the firmware does look up,
  is no longer called "unused" nor dimmed in the list. The scan misses it because the
  firmware reaches it another way.
- Reading a map from a descriptor that points near the end of the address space no longer
  overflows in debug builds.

---

## 6. Upgrading

- **Windows:** unzip over the old folder.
- **Android:** install over 0.4.2; same key, same data. The in-app updater offers it. The
  map changes in §1–§3 are in the Windows window only this time.
- **J2534:** replace the DLLs and run `register.ps1` again as administrator. The DLL file
  names did not change.

---

*Checked against an Opel Astra J 1.7 CDTI (A17DTJ) in the emulator and with GDS2. Windows
programs built by the release workflow from the tagged commit, checked to import no Visual
C++ runtime. Release APKs built from the tagged commit and signed with the same key as
0.3.x and 0.4.x (certificate SHA-256 `5ff00c0a…f515c2`).*
