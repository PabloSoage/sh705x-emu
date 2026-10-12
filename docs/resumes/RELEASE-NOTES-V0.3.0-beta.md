# 🚀 sh705x-emu Beta v0.3.0 — fault codes live, and an oscilloscope on the pins

The calibration's fault codes with their tests running live, and an oscilloscope on the
ECU's pins: the inputs, the PWM outputs and the injector channel, held on an edge and
measured. On Windows and on Android.

> **Context:** two commits on top of 0.2.0, checked against the Astra J's own flash
> (A17DTJ, Denso, SH7059, NCLSW420AX36).
>
> **Theme:** 0.2.0 showed what the firmware reads; this one shows what it tests and what
> it drives. A fault code is no longer just a line on a scan tool: you see its test run,
> fail, and become the code the ECU reports, and the maps that test reads. And the PWM the
> firmware computes is no longer a percentage: it is a waveform on a time base.
>
> **Deliverables:**
> - `sh705x-v0.3.0-beta-windows-x86_64.zip`: `sh705x-gui.exe` and `sh705x.exe`, which
>   carry their own C runtime.
> - `app-arm64-v8a-release.apk` and `app-x86_64-release.apk`: versionCode 3, versionName
>   0.3.0. Signed with the same key as 0.2.0, so they install over it.
>
> **No firmware comes with any of them.**

---

## 1. The headline

| | 0.2.0 | 0.3.0 |
|---|---|---|
| Fault codes | the ones the ECU reports, over OBD | **all 202**, enabled or not, each test live |
| A code's test | — | how often it runs, how often it fails, where, which maps it reads |
| Stopping on a code | — | **"Stop at its next test"** |
| The ECU's outputs | duty percentages in the engine view | **waveforms**, on 10 µs to 50 ms a division |
| Measuring a pin | — | frequency, period and duty per channel, an edge trigger, a cursor |

---

## 2. Fault codes, live

- **From the flash:** on the Astra's calibration the table holds 202 codes (194 enabled).
  Each comes with:
  - its tables A, B and C (A and C both 0 disable a code, as a workshop file does);
  - what the ECU does while it is active (raising the idle, among others).
- **Their tests, probed:** every test reports through one routine, with the code's index
  and its result (0 passed, 1 failed). The emulator probes it alongside the maps and
  counts, per code, the tests, the failures, the last result and where it was tested
  from.
  - At idle 193 of the 202 are tested.
  - With the DPF sensor at the top of its range, P2455's test starts failing and the ECU
    reports the code.
- **Where each is tested:** the function that reports it, read from the code, and the maps
  that function reads, each a link to the map. For example, P0101 (MAF range or
  performance) reads two.
- **"Read the codes"** asks the ECU for its confirmed (03) and pending (07) codes.
- **"Stop at its next test"** stops where the firmware next reports that code's test.
- **Descriptions:** the emulator brings them for the codes this project has met, in its
  own words, and your own file can add the rest.

---

## 3. An oscilloscope on the pins

- **Inputs** (crank, cam, MAF): every edge at the cycle the chip took it.
- **PWM outputs** of ATU-II channels 6 and 7 (SCV, throttle, VGT, swirl flaps, EGR): drawn
  from the timer's counter, period and duty. Exact between register changes, which are
  placed when the emulator next looks (half a millisecond at most in real time).
- **The one-shot outputs** of channel 8, where the injectors are: each pulse, exact.
- **Controls:**
  - time base from 10 µs to 50 ms a division;
  - a trigger on either edge of any channel (auto, normal or single, with how much comes
    before it);
  - hold, and a cursor that reads the time from the trigger;
  - each channel's frequency, period and duty.

**At idle on the Astra:**

| Signal | Measured | Check |
|---|---|---|
| Crank | 750 Hz, 50 % | 60 teeth at 750 rpm |
| SCV | 250 Hz, 53.5–54.8 % | the duty its registers give |
| VGT | 300 Hz | — |
| EGR opening drive | 200 Hz | — |

---

## 4. On Windows and on Android

- **Windows:** "Scope" opens the oscilloscope, and the maps window has a second page,
  "Fault codes".
- **Android:** two new tabs, "Faults" and "Scope". The scope's traces come first and the
  controls below them.

---

## 5. The rest

- **Tests for the new parts:**
  - the code table and its reports against what this project found by hand;
  - a provoked fault seen tested, reported and stopped at;
  - PWM traces and their measures, on a synthetic waveform and on the Astra's SCV;
  - the windows and the app's frames drawn and read.
- **A bug the tests caught:** a PWM period that had started before cycle 0 overflowed. It
  is worked out signed now.
- **CI** builds and tests both, as before.

---

## 6. Upgrading

- **Windows:** unzip over the old folder.
- **Android:** install over 0.2.0; same key, same data.

---

*Checked against an Opel Astra J 1.7 CDTI (A17DTJ): its flash at warm idle in the emulator,
with a fault provoked on the DPF sensor. Windows programs built by the release workflow from
the tagged commit, checked to import no Visual C++ runtime. Release APKs built from the tagged
commit and signed with the same key as 0.2.0 (certificate SHA-256 `5ff00c0a…f515c2`).*
