# 🚀 sh705x-emu Beta v0.3.2 — the condition 65 fault codes share

The Astra's condition 0, which 65 of its fault codes need before their results count, read
from the firmware and described on each code that needs it. On Windows and on Android.

> **Context:** one commit on top of 0.3.1, checked against the Astra J's own flash
> (A17DTJ, Denso, SH7059, NCLSW420AX36).
>
> **Theme:** 0.3.1 showed which of a code's conditions are met; this one says what the most
> common of them is. It is the ECU's go-ahead for its electrical tests: the ignition on, the
> battery in range and the start-up over. Without it, a low battery would read as sensor
> circuits out of range.
>
> **Deliverables:**
> - `sh705x-v0.3.2-beta-windows-x86_64.zip`: `sh705x-gui.exe` and `sh705x.exe`, which
>   carry their own C runtime.
> - `app-arm64-v8a-release.apk` and `app-x86_64-release.apk`: versionCode 5, versionName
>   0.3.2. Signed with the same key as 0.3.1, so they install over it.
>
> **No firmware comes with any of them.**

---

## 1. Condition 0

65 of the Astra's 202 codes need it, and 36 need nothing else. They are mostly the circuit
high and low tests of the sensors:

- the MAF, the coolant and intake temperatures;
- the pedal, the boost and barometric pressures;
- the rail pressure, the fuel temperature and level;
- the EGR position, the 5 V references;
- the exhaust temperatures, the DPF pressure sensor (P2455 among them).

The flag is set when all three of these hold:

1. **The battery between 10 and 16 V.** With 1 V of hysteresis: it comes in between 11 and
   15 V, and drops out under 10 or over 16. Measured: started at 9.5, 10.5, 15.5 or 16.5 V
   it is never met; at 12 or 14 V it is.
2. **The ignition on, and the start-up over:**
   - **The ignition:** input AN14 after a 4.01 divider, on above 4.5 V and off below 4.0,
     two readings alike in a row, held a few runs after it drops.
   - **The start-up:** 56 runs in a row, about 0.8 s, after the start-up routine sets
     port D's PD4.
3. **A third part that is always met on this calibration:** it waits for a flag or for the
   engine turning, but both its delays are calibrated to 0.

At idle, and with the ignition on and the engine still, it is met 1.06 s after power-on.

---

## 2. In the emulator

- **Each code that needs condition 0** says what it is, beside whether it is met now (as
  condition 1 already did for P0101).
- **Tests:**
  - it is met once the DPF fault has been provoked;
  - with the battery at 9.5 V it is never met.

---

## 3. Upgrading

- **Windows:** unzip over the old folder.
- **Android:** install over 0.3.1; same key, same data.

---

*Checked against an Opel Astra J 1.7 CDTI (A17DTJ): its flash at warm idle and with the
ignition on in the emulator, the battery set from 9.5 to 16.5 V, the flag's writes watched.
Windows programs built by the release workflow from the tagged commit, checked to import no
Visual C++ runtime. Release APKs built from the tagged commit and signed with the same key as
0.3.1 (certificate SHA-256 `5ff00c0a…f515c2`).*
