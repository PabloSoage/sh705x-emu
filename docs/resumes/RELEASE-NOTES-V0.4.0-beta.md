# 🚀 sh705x-emu Beta v0.4.0 — plugins beside the ECU, and an app that updates itself

WebAssembly plugins that ride the ECU's CAN bus, read the memory they declare and write
RAM, each in a sandbox with a budget per call — installed from a file or from your own
repository, public or private, exactly as opendash does. And an Android app that finds its
own new releases on GitHub and installs them.

> **Context:** the plugin host and the update detector on top of 0.3.2, checked against the
> Astra J's own flash (A17DTJ, Denso, SH7059, NCLSW420AX36) and on an Android emulator.
>
> **Why plugins:** the emulator's CAN bus already lets other nodes answer the ECU
> ([ADR-0005](https://github.com/PabloSoage/sh705x-emu)). A plugin is that node, made
> installable: it hears the frames the ECU sends and puts frames back on the bus, so you
> can stand in for a module the car talks to without touching the emulator's own code
> ([ADR-0008](https://github.com/PabloSoage/sh705x-emu)). Anything specific to a car stays
> in a private plugin repository, out of this one.
>
> **Deliverables:**
> - `sh705x-v0.4.0-beta-windows-x86_64.zip`: `sh705x-gui.exe` and `sh705x.exe`, which
>   carry their own C runtime.
> - `app-arm64-v8a-release.apk` and `app-x86_64-release.apk`: versionCode 6, versionName
>   0.4.0. Signed with the same key as 0.3.x, so they install over it.
>
> **No firmware comes with any of them.**

---

## 1. The plugins

- **Format:** a `.wasm` carrying a JSON manifest in its `sh705x-plugin` section. It runs on
  `wasmi`, with 20 million units of fuel per call, so a plugin that loops forever is stopped
  and the rest of the machine goes on.
- **API 1:**
  - `send`, `read`, `write`, `log` and `now_us`;
  - `init`, `on_frame` (every frame the ECU sends) and `tick` (every `tick_us`).
- **Sandbox:** a plugin reads only the memory its manifest names (up to 64 KiB) and writes
  only RAM (`FFFE_8000..FFFF_C000`); any write outside is refused and logged.
- **Sources:** install from a file, or from GitHub, GitLab, Gitea or a plain HTTPS index —
  with a per-source token for private repositories, the same URLs and headers as opendash.
- **SDK and example:** `plugins/sdk` (the `sh705x-plugin` crate, a `manifest!` macro and
  `read`/`write`/`log`/`send` helpers) and `plugins/abs-node`, which stands in for the ABS:
  it sends `0x1E9` every 21.4 ms, and with it the Astra stops reporting U0121 (no ABS).

## 2. On each platform

- **Windows:** a Plugins window — load, enable, see each plugin's log and errors, add a
  source and install from it.
- **Android:** a Plugins tab (the 5th), the same, plus loading the enabled plugins at
  power-on. Needs the network permission for remote sources.

## 3. The app updates itself

- Ported from opendash: at start-up (you can turn it off) and by hand, under a new
  **Updates** section, it asks GitHub for the newest release, compares versions by number,
  downloads the APK for the phone's ABI and hands it to the installer.
- It never interrupts: the dialog does not appear while the ECU is running.

## 4. What was measured

- With the Astra's flash and EEPROM, no node, 8 s of idle: U0121 is reported. With the
  `abs-node` plugin: no U0121; it hears 3250 frames in 5 s and reads 750 rpm.
- A write outside RAM is refused and logged; a plugin looping in `tick` is stopped, the
  others keep running.
- A private GitHub source serves its index with a token and gives a helpful 404 without one.
- On the Android emulator: install from a file, from an HTTPS source, load at power-on
  across a power cycle; the update detector found a newer release, downloaded its APK and
  requested the install permission.

---

## 5. Upgrading

- **Windows:** unzip over the old folder.
- **Android:** install over 0.3.x; same key, same data.

---

*Checked against an Opel Astra J 1.7 CDTI (A17DTJ) in the emulator, and on a Pixel Android
emulator (API 36, x86_64). Windows programs built by the release workflow from the tagged
commit, checked to import no Visual C++ runtime. Release APKs built from the tagged commit
and signed with the same key as 0.3.x (certificate SHA-256 `5ff00c0a…f515c2`).*
