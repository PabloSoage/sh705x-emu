# Plugins

A plugin is one WebAssembly module that the emulator runs beside the ECU, in a sandbox.
It can:

- hear every frame the ECU sends on its CAN bus (HCAN0), and put frames of its own on
  it, as another node of the car would;
- read the parts of the ECU's memory its manifest names, and write RAM;
- log lines for the user.

It cannot reach the network, the disk or anything else on the host. Every call has a
budget of work, so a plugin that loops forever is stopped and set aside, and the ECU
goes on.

The same `.wasm` runs in the Windows program and in the Android app. Plugins are
installed from a file, or from a source: a repository of your own (GitHub, GitLab,
Gitea) with a token scoped to it, or any HTTPS directory. Each source keeps its own
token, as opendash does with its catalogues. A plugin can live in a private repository
and never be published.

| Here | What |
|---|---|
| `sdk/` | `sh705x-plugin`: the emulator's imports as Rust functions, `Frame`, and the `manifest!` macro |
| `abs-node/` | An example: the ABS frame 0x1E9 every 21.4 ms (what the built-in `abs` peer sends), and a log line every five seconds with the frames the ECU has sent and its engine speed |
| `../crates/plugins` | The host side: the API, the sandbox, the library and the sources |

---

## Writing one in Rust

```toml
# Cargo.toml
[lib]
crate-type = ["cdylib"]

[dependencies]
sh705x-plugin = { git = "https://github.com/PabloSoage/sh705x-emu" }
```

```rust
use sh705x_plugin::{Frame, log, manifest, send};

manifest!(r#"{"name": "My node", "version": "0.1.0", "api": 1, "tick_us": 10000}"#);

#[unsafe(no_mangle)]
pub extern "C" fn init() {
    log("on the bus");
}

#[unsafe(no_mangle)]
pub extern "C" fn on_frame(id: i32, flags: i32, len: i32, data: i64) {
    let f = Frame::from_args(id, flags, len, data);
    if f.id == 0x7E8 {
        // Answer 2 ms later.
        send(&Frame::standard(0x7E9, f.bytes()), 2000);
    }
}
```

```sh
rustup target add wasm32-unknown-unknown
cargo build --release --target wasm32-unknown-unknown
# target/wasm32-unknown-unknown/release/<name>.wasm is the plugin
```

Any language that compiles to WebAssembly works: the contract is the imports and exports
below, and the manifest in a custom section.

### The manifest

JSON in the custom section `sh705x-plugin` (`manifest!` puts it there):

| Field | |
|---|---|
| `name`, `version` | required |
| `api` | required: `1` |
| `description` | shown beside it |
| `reads` | ranges of memory it may `read`: `[["FFFE8000", "FFFE8100"]]`, start and end (exclusive), hexadecimal; 64 KiB in all at most |
| `tick_us` | how often `tick` is called, µs of ECU time; 1000 if not given, 100 at least |

### API 1

Imports, from the module `sh705x`:

| Import | |
|---|---|
| `send(id: i32, flags: i32, len: i32, data: i64, delay_us: i64)` | puts a frame on the bus `delay_us` from now |
| `read(addr: i32) -> i32` | a byte of the memory `reads` covers, as of this call; -1 elsewhere |
| `write(addr: i32, value: i32)` | writes a byte of RAM once the call returns; outside RAM it is refused and logged |
| `log(ptr: i32, len: i32)` | a line of UTF-8 from the module's memory |
| `now_us() -> i64` | ECU time since power-on, µs |

Exports, each optional:

| Export | Called |
|---|---|
| `init()` | once, when loaded |
| `on_frame(id: i32, flags: i32, len: i32, data: i64)` | for every frame the ECU sends |
| `tick(now_us: i64)` | every `tick_us` |
| `memory` | needed for `log` |

A frame's `flags` are bit 0 extended (29-bit) identifier and bit 1 remote frame. `data`
holds the bytes little-endian, byte 0 in the low 8 bits.

### Timing

- **When it runs:** a plugin runs after each stretch the emulator runs, about 0.5 ms of
  ECU time at full speed.
- **What it hears:** the frames the ECU sent in that stretch, in order.
- **When its frames arrive:** at the end of the stretch they fall due in, so up to one
  stretch late.
- **The ELM327 console:** its exchanges run the ECU on their own, and plugins do not run
  during them.

---

## Publishing them from a repository

Put a `plugins.txt` at the root of the repository, or of the web directory, and the
`.wasm` files beside it:

```text
# file	name	version	description
my-node.wasm	My node	0.1.0	Answers on the bus
```

The fields are tab-separated, and `#` starts a comment.

Then add the source in the program's Plugins window, or the app's Plugins tab:

| Kind | Location | Token |
|---|---|---|
| GitHub | `owner/repository` | a fine-grained token with read access to its contents |
| GitLab | `group/project` (on gitlab.com) | `read_repository` |
| Gitea | `https://host/owner/repository` | read access |
| HTTPS | `https://host/directory` | sent as a bearer token, if given |

**Where tokens are kept:** in `sources.json` beside the plugins, as written. That is
`%APPDATA%\sh705x` on Windows, and the app's own storage on Android, which the app
never hands back to the screen.
