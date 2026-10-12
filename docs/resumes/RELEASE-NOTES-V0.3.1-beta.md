# 🚀 sh705x-emu Beta v0.3.1 — why a failing test is not yet a fault

The fault codes page now tells the tests the ECU counted from those it threw away, and
which of a code's conditions are met. The fault code tables are also found in the flash
from its code rather than looked up by calibration. On Windows and on Android.

> **Context:** one commit on top of 0.3.0, checked against the Astra J's own flash
> (A17DTJ, Denso, SH7059, NCLSW420AX36).
>
> **Theme:** in 0.3.0, P0101's test failed at every run at idle and the code never went
> pending. This version shows why: the ECU only counts a code's results while its
> conditions are met, and P0101's need 1500 to 4000 rpm among others.
>
> **Deliverables:**
> - `sh705x-v0.3.1-beta-windows-x86_64.zip`: `sh705x-gui.exe` and `sh705x.exe`, which
>   carry their own C runtime.
> - `app-arm64-v8a-release.apk` and `app-x86_64-release.apk`: versionCode 4, versionName
>   0.3.1. Signed with the same key as 0.3.0, so they install over it.
>
> **No firmware comes with any of them.**

---

## 1. The headline

| | 0.3.0 | 0.3.1 |
|---|---|---|
| A code's tests | how many ran and failed | and **how many the ECU counted** |
| Its conditions | — | **each one, and whether it is met now** |
| The tables | known for the Astra's calibration only | **found from the code**, on any flash laid out the same way |

---

## 2. Counted or thrown away

- **The conditions:** every test reports through one routine. That routine only passes the
  result on to the one that counts it (the routine that reads tables A, B and C) when the
  code's conditions are met. A code lists up to three, each a flag in RAM.
- **What the emulator shows:** it probes the counting routine as well, so each code shows
  how many of its results counted, and its conditions as they are now: met or not.
- **P0101 at idle:**
  - its test compares the air per stroke, brought to 25 °C and 101.325 kPa, with two maps
    over engine speed;
  - at idle it is under the lower limit and the test fails; at 2000 rpm it passes;
  - its results only count while condition 1 is met: 1500 to 4000 rpm, five other
    quantities under their limits and condition 0 met, 125 times in a row;
  - so at idle it fails and is never counted.

---

## 3. The tables, found

The fault code tables are no longer a constant for one calibration:

- **The counting routine:** a function that loads three tables of n words one after
  another (A, B and C).
- **The reporter:** the routine that calls it, called from many places with a constant
  index under n.
- **The codes:** the table of n words that looks like codes and is loaded from the most
  places.
- **The reactions and the conditions:** tables loaded by routines the reporter calls.

On the Astra's four dumps (the original, two workshop files and a later read) it finds
exactly what this project found by hand, in about 56 ms. It still does with half the codes
disabled. **No other calibration has been tried:** on one laid out differently it may find
nothing, and the page says so.

---

## 4. The rest

- **Screenshots:** the faults page on Windows and on Android, and the Android
  oscilloscope with its time axis as the code now draws it.
- **Tests:**
  - the layout found on the Astra's flash, also with codes disabled, and none on a flash
    without the code;
  - P0101 failing at idle, uncounted, condition 1 not met and 2 met;
  - P2455 counted once its fault is provoked.
- **CI** builds and tests both, as before.

---

## 5. Upgrading

- **Windows:** unzip over the old folder.
- **Android:** install over 0.3.0; same key, same data.

---

*Checked against an Opel Astra J 1.7 CDTI (A17DTJ): its flash at warm idle in the emulator,
and at 2000 and 2500 rpm with the plants held. Windows programs built by the release workflow
from the tagged commit, checked to import no Visual C++ runtime. Release APKs built from the
tagged commit and signed with the same key as 0.3.0 (certificate SHA-256 `5ff00c0a…f515c2`).*
