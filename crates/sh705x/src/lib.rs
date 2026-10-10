//! Command-line front end. The binary is a thin wrapper around [`run`], so every
//! command can be tested without spawning a process.

use board::Eeprom;
use sh2e::{Cpu, Vector};
use sh7059::{Machine, Region, Sh7059, region};
use std::fmt::Write as _;
use std::path::PathBuf;

/// Usage text.
pub const USAGE: &str = "\
usage: sh705x info  --flash <flash.bin> [--eeprom <eeprom.bin>]
       sh705x disasm --flash <flash.bin> --from <addr> [--count <instructions>]
       sh705x obd   --flash <flash.bin> [--eeprom <eeprom.bin>] [--boot <instructions>]
                    [--adc <channel:code>]... [--rpm <rpm>] [--sensors on|idle|off]
                    [--sensor <key=value>]... [--maf-hz <hz>] [--peers abs|none]
                    <ELM327 command>...
       sh705x serve --flash <flash.bin> [--eeprom <eeprom.bin>] [--port <port>]
                    [--adc <channel:code>]... [--flat-out]
       sh705x sm3   --flash <flash.bin> [--eeprom <eeprom.bin>] [--bind <address>]
                    [--port <port>] [--adc <channel:code>]...
       sh705x bench --flash <flash.bin> [--eeprom <eeprom.bin>] [--rpm <rpm>]
                    [--seconds <s>] [--adc <channel:code>]...
       sh705x trace --flash <flash.bin> [--max <instructions>] [--force <addr:mask>]...
                    [--adc <channel:code>]... [--eeprom <eeprom.bin>]
                    [--eeprom-layout <blocks>[le]] [--eeprom-out <eeprom.bin>]
                    [--out <io-trace.txt>] [--no-address-errors] [--history]

commands:
  info    load the images, check them and show the reset vectors and reset state
  disasm  disassemble the flash from an address (hex), with PC-relative literals
  obd     boot the ECU (40 M instructions by default) and send it ELM327 commands;
          --rpm turns a 60-2 crank wheel (TI0A) and a cam (TI0B) at that speed;
          the analog sensors start as a warm engine with the ignition on (--sensors
          on), idling with --rpm (idle), or all at code 0 (off); --sensor sets one in
          physical units, as the ECU reports it (coolant=90, rail=30, battery=13.8...;
          an unknown key lists them all; maf is g/s, a square wave on TI0C); --adc
          sets a raw code and --maf-hz the MAF frequency on top of all that; the ABS
          sends its frame 0x1E9 on the bus unless --peers none
          ('0100', 'ATSH7E0', '1A90'...) over the emulated CAN bus, in virtual time;
          prints each answer as an ELM327 would
  serve   run the ECU in real time and serve an ELM327 on TCP (port 35000 by
          default) for diagnostic apps; --flat-out runs as fast as it can instead
          of following the wall clock
  sm3     run the ECU in real time behind an emulated Scanmatik SM3, for opendash:
          set opendash's adapter address to this machine (port 777 by default;
          binds 0.0.0.0 unless --bind says otherwise)
  bench   boot the ECU as obd does, then run --seconds (5 by default) of ECU time as
          fast as it goes, and print the real-time factor: ECU time over wall time,
          1.00 being just real time
  trace   run from reset with peripheral registers as plain storage, optionally forcing
          status bits as phase 0 did, and write every register access to --out as
          'pc R|W addr size value' (hex). Prints instructions, cycles and speed.
          --no-address-errors makes misaligned accesses go through, as in Ghidra's
          emulator; only for comparing with it. --history prints the steps before the
          first exception. --adc sets an analog input (AN0-31) to a 10-bit code.
          --eeprom serves an 8 KB dump from the four 93C86 on the board; --eeprom-layout
          says which 2 KB block is each chip (PJ3/SCI3, PJ4/SCI4, PD0/SCI4, PD1/SCI2),
          default 0123, high byte first unless 'le' follows; --eeprom-out saves the
          EEPROM as the run left it";

/// A parsed command line.
#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    /// Print the usage text.
    Help,
    /// Load and describe the images.
    Info {
        /// Path to the flash image.
        flash: PathBuf,
        /// Path to the EEPROM image, if any.
        eeprom: Option<PathBuf>,
    },
    /// Run from reset and trace register accesses.
    Trace(TraceOptions),
    /// Talk to the ECU as an ELM327, in virtual time.
    Obd(EcuOptions, Vec<String>),
    /// Serve an ELM327 on TCP, in real time.
    Serve(EcuOptions, u16, bool),
    /// Serve an emulated SM3 on TCP, in real time: options, address, port.
    Sm3(EcuOptions, String, u16),
    /// Measure the real-time factor over this many seconds of ECU time.
    Bench(EcuOptions, u32),
    /// Disassemble the flash.
    Disasm {
        /// Path to the flash image.
        flash: PathBuf,
        /// First address.
        from: u32,
        /// Instructions to show.
        count: u32,
    },
}

/// The ECU to run for `obd` and `serve`.
#[derive(Debug, PartialEq, Eq)]
pub struct EcuOptions {
    /// Path to the flash image.
    pub flash: PathBuf,
    /// EEPROM dump to serve.
    pub eeprom: Option<PathBuf>,
    /// Where each chip is in the dump.
    pub layout: board::Layout,
    /// A/D inputs: channel and 10-bit code.
    pub adc: Vec<(usize, u16)>,
    /// Instructions to run before talking to it.
    pub boot: u64,
    /// Engine speed of the simulated crank and cam wheels, if the engine turns.
    pub rpm: Option<u32>,
    /// The sensors' starting state: "on" (ignition on, engine still), "idle", or "off"
    /// (every input at code 0); `None` for "idle" with --rpm and "on" without.
    pub sensors: Option<String>,
    /// Sensors set by key, `key=value` in physical units.
    pub sensor: Vec<String>,
    /// Frequency of the MAF's square wave on TI0C, over what the sensors give.
    pub maf_hz: Option<u32>,
    /// Other nodes on the bus: "abs" (the default) or "none".
    pub peers: String,
}

/// AN14 code that wakes the ECU: the firmware reads its ignition input there and stays
/// silent on CAN below code 230 (measured). The board's divider is not known, so this
/// is not a voltage [SUP].
pub const IGNITION_ON: u16 = 600;

/// Parses `--adc channel:code`.
fn parse_adc(value: &str) -> Result<(usize, u16), String> {
    let bad = || format!("--adc wants channel:code (0-31:0-1023), got '{value}'");
    let (c, v) = value.split_once(':').ok_or_else(bad)?;
    let c: usize = c.parse().map_err(|_| bad())?;
    let v: u16 = v.parse().map_err(|_| bad())?;
    if c > 31 || v > 1023 {
        return Err(bad());
    }
    Ok((c, v))
}

/// A command's own options: takes the option and the word after it, and says whether
/// it was one (and so took its value, if it has one).
type ExtraOption<'a> = dyn FnMut(&str, Option<&String>) -> Result<bool, String> + 'a;

/// Parses the options shared by `obd` and `serve`; the rest go back as arguments.
fn parse_ecu(
    cmd: &str,
    rest: &[String],
    extra: &mut ExtraOption,
) -> Result<(EcuOptions, Vec<String>), String> {
    let mut o = EcuOptions {
        flash: PathBuf::new(),
        eeprom: None,
        layout: board::DEFAULT_LAYOUT,
        adc: vec![(14, IGNITION_ON)],
        boot: 40_000_000,
        rpm: None,
        sensors: None,
        sensor: Vec::new(),
        maf_hz: None,
        peers: "abs".into(),
    };
    let (mut flash, mut args) = (None, Vec::new());
    let mut it = rest.iter().peekable();
    while let Some(opt) = it.next() {
        if !opt.starts_with("--") {
            args.push(opt.clone());
            continue;
        }
        if extra(opt, it.peek().copied())? {
            if it.peek().is_some_and(|v| !v.starts_with("--")) {
                it.next();
            }
            continue;
        }
        let value = it.next().ok_or_else(|| format!("{opt} needs a value"))?;
        match opt.as_str() {
            "--flash" => flash = Some(PathBuf::from(value)),
            "--eeprom" => o.eeprom = Some(PathBuf::from(value)),
            "--eeprom-layout" => o.layout = parse_layout(value)?,
            "--adc" => {
                let (c, v) = parse_adc(value)?;
                o.adc.retain(|&(k, _)| k != c);
                o.adc.push((c, v));
            }
            "--boot" => o.boot = value.parse().map_err(|_| format!("bad --boot '{value}'"))?,
            "--rpm" => o.rpm = Some(value.parse().map_err(|_| format!("bad --rpm '{value}'"))?),
            "--maf-hz" => {
                o.maf_hz = Some(
                    value
                        .parse()
                        .map_err(|_| format!("bad --maf-hz '{value}'"))?,
                )
            }
            "--peers" => {
                if !["abs", "none"].contains(&value.as_str()) {
                    return Err(format!("--peers is abs or none, not '{value}'"));
                }
                o.peers = value.clone();
            }
            "--sensors" => {
                if !["on", "idle", "off"].contains(&value.as_str()) {
                    return Err(format!("--sensors is on, idle or off, not '{value}'"));
                }
                o.sensors = Some(value.clone());
            }
            "--sensor" => {
                engine::sensors::parse(value)?;
                o.sensor.push(value.clone());
            }
            other => return Err(format!("unknown option '{other}'")),
        }
    }
    o.flash = flash.ok_or_else(|| format!("{cmd} needs --flash <flash.bin>"))?;
    Ok((o, args))
}

/// Options of the `trace` command.
#[derive(Debug, PartialEq, Eq)]
pub struct TraceOptions {
    /// Path to the flash image.
    pub flash: PathBuf,
    /// Instructions to run, delay slots included.
    pub max: u64,
    /// Register bytes and the bits forced to 1 in every read of them.
    pub force: Vec<(u32, u8)>,
    /// Where to write the register access trace.
    pub out: Option<PathBuf>,
    /// Raise CPU address errors (off only to compare with Ghidra).
    pub address_errors: bool,
    /// Keep the last steps and print them at the first exception (slower).
    pub history: bool,
    /// A/D inputs: channel and 10-bit code.
    pub adc: Vec<(usize, u16)>,
    /// EEPROM dump to serve.
    pub eeprom: Option<PathBuf>,
    /// Where each chip is in the dump.
    pub layout: board::Layout,
    /// Where to save the EEPROM at the end.
    pub eeprom_out: Option<PathBuf>,
}

fn hex(s: &str) -> Result<u32, String> {
    u32::from_str_radix(s.trim_start_matches("0x"), 16).map_err(|_| format!("not hex: '{s}'"))
}

/// Parses the arguments that follow the program name.
pub fn parse(args: &[String]) -> Result<Command, String> {
    let Some((cmd, rest)) = args.split_first() else {
        return Ok(Command::Help);
    };
    match cmd.as_str() {
        "-h" | "--help" | "help" => Ok(Command::Help),
        "info" => {
            let (mut flash, mut eeprom) = (None, None);
            let mut it = rest.iter();
            while let Some(opt) = it.next() {
                let slot = match opt.as_str() {
                    "--flash" => &mut flash,
                    "--eeprom" => &mut eeprom,
                    other => return Err(format!("unknown option '{other}'")),
                };
                let value = it.next().ok_or_else(|| format!("{opt} needs a path"))?;
                *slot = Some(PathBuf::from(value));
            }
            let flash = flash.ok_or("info needs --flash <flash.bin>")?;
            Ok(Command::Info { flash, eeprom })
        }
        "obd" => {
            let (o, args) = parse_ecu("obd", rest, &mut |_, _| Ok(false))?;
            if args.is_empty() {
                return Err("obd needs at least one ELM327 command, like 0100".into());
            }
            Ok(Command::Obd(o, args))
        }
        "serve" => {
            let (mut port, mut flat_out) = (35000u16, false);
            let (o, args) = parse_ecu("serve", rest, &mut |opt, value| match opt {
                "--port" => {
                    let v = value.ok_or("--port needs a value")?;
                    port = v.parse().map_err(|_| format!("bad --port '{v}'"))?;
                    Ok(true)
                }
                "--flat-out" => {
                    flat_out = true;
                    Ok(true)
                }
                _ => Ok(false),
            })?;
            if let Some(a) = args.first() {
                return Err(format!("serve takes no argument '{a}'"));
            }
            Ok(Command::Serve(o, port, flat_out))
        }
        "sm3" => {
            let (mut bind, mut port) = ("0.0.0.0".to_string(), 777u16);
            let (o, args) = parse_ecu("sm3", rest, &mut |opt, value| match opt {
                "--port" => {
                    let v = value.ok_or("--port needs a value")?;
                    port = v.parse().map_err(|_| format!("bad --port '{v}'"))?;
                    Ok(true)
                }
                "--bind" => {
                    bind = value.ok_or("--bind needs a value")?.clone();
                    Ok(true)
                }
                _ => Ok(false),
            })?;
            if let Some(a) = args.first() {
                return Err(format!("sm3 takes no argument '{a}'"));
            }
            Ok(Command::Sm3(o, bind, port))
        }
        "bench" => {
            let mut seconds = 5u32;
            let (o, args) = parse_ecu("bench", rest, &mut |opt, value| match opt {
                "--seconds" => {
                    let v = value.ok_or("--seconds needs a value")?;
                    seconds = v.parse().map_err(|_| format!("bad --seconds '{v}'"))?;
                    Ok(true)
                }
                _ => Ok(false),
            })?;
            if let Some(a) = args.first() {
                return Err(format!("bench takes no argument '{a}'"));
            }
            Ok(Command::Bench(o, seconds))
        }
        "disasm" => {
            let (mut flash, mut from, mut count) = (None, None, 32);
            let mut it = rest.iter();
            while let Some(opt) = it.next() {
                let value = it.next().ok_or_else(|| format!("{opt} needs a value"))?;
                match opt.as_str() {
                    "--flash" => flash = Some(PathBuf::from(value)),
                    "--from" => from = Some(hex(value)?),
                    "--count" => {
                        count = value
                            .parse()
                            .map_err(|_| format!("bad --count '{value}'"))?
                    }
                    other => return Err(format!("unknown option '{other}'")),
                }
            }
            Ok(Command::Disasm {
                flash: flash.ok_or("disasm needs --flash <flash.bin>")?,
                from: from.ok_or("disasm needs --from <addr>")? & !1,
                count,
            })
        }
        "trace" => {
            let (mut flash, mut out, mut max, mut force) = (None, None, 3_000_000, Vec::new());
            let (mut address_errors, mut history, mut adc) = (true, false, Vec::new());
            let (mut eeprom, mut eeprom_out, mut layout) = (None, None, board::DEFAULT_LAYOUT);
            let mut it = rest.iter();
            while let Some(opt) = it.next() {
                match opt.as_str() {
                    "--no-address-errors" => {
                        address_errors = false;
                        continue;
                    }
                    "--history" => {
                        history = true;
                        continue;
                    }
                    _ => {}
                }
                let value = it.next().ok_or_else(|| format!("{opt} needs a value"))?;
                match opt.as_str() {
                    "--flash" => flash = Some(PathBuf::from(value)),
                    "--out" => out = Some(PathBuf::from(value)),
                    "--eeprom" => eeprom = Some(PathBuf::from(value)),
                    "--eeprom-out" => eeprom_out = Some(PathBuf::from(value)),
                    "--eeprom-layout" => layout = parse_layout(value)?,
                    "--max" => max = value.parse().map_err(|_| format!("bad --max '{value}'"))?,
                    "--adc" => {
                        let bad =
                            || format!("--adc wants channel:code (0-31:0-1023), got '{value}'");
                        let (c, v) = value.split_once(':').ok_or_else(bad)?;
                        let c: usize = c.parse().map_err(|_| bad())?;
                        let v: u16 = v.parse().map_err(|_| bad())?;
                        if c > 31 || v > 1023 {
                            return Err(bad());
                        }
                        adc.push((c, v));
                    }
                    "--force" => {
                        let (a, m) = value
                            .split_once(':')
                            .ok_or_else(|| format!("--force wants addr:mask, got '{value}'"))?;
                        let mask = hex(m)?;
                        if mask > 0xFF {
                            return Err(format!("--force mask is one byte: '{value}'"));
                        }
                        force.push((hex(a)?, mask as u8));
                    }
                    other => return Err(format!("unknown option '{other}'")),
                }
            }
            let flash = flash.ok_or("trace needs --flash <flash.bin>")?;
            Ok(Command::Trace(TraceOptions {
                flash,
                max,
                force,
                out,
                address_errors,
                history,
                adc,
                eeprom,
                layout,
                eeprom_out,
            }))
        }
        other => Err(format!("unknown command '{other}'")),
    }
}

/// `0123`, `2301le`...: the dump block of each chip, then `le` for low byte first.
fn parse_layout(v: &str) -> Result<board::Layout, String> {
    let bad = || format!("--eeprom-layout wants four distinct digits 0-3, maybe then 'le': '{v}'");
    let (digits, le) = match v.strip_suffix("le") {
        Some(d) => (d, true),
        None => (v, false),
    };
    let b: Vec<usize> = digits
        .chars()
        .map(|c| c.to_digit(4).map(|d| d as usize))
        .collect::<Option<_>>()
        .ok_or_else(bad)?;
    let mut seen = [false; 4];
    if b.len() != 4 || b.iter().any(|&d| std::mem::replace(&mut seen[d], true)) {
        return Err(bad());
    }
    Ok(board::Layout {
        blocks: [b[0], b[1], b[2], b[3]],
        big_endian: !le,
    })
}

fn read(path: &PathBuf) -> Result<Vec<u8>, String> {
    std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))
}

fn region_name(addr: u32) -> &'static str {
    match region(addr) {
        Region::Rom(_) => "flash ROM",
        Region::Ram(_) => "RAM",
        Region::Io(_) => "peripheral registers",
        Region::Unmapped => "UNMAPPED",
    }
}

/// Runs a command and returns what it prints.
pub fn run(cmd: &Command) -> Result<String, String> {
    match cmd {
        Command::Help => Ok(USAGE.to_string()),
        Command::Info { flash, eeprom } => info(flash, eeprom.as_ref()),
        Command::Trace(opts) => trace(opts),
        Command::Disasm { flash, from, count } => disasm(flash, *from, *count),
        Command::Obd(o, args) => obd(o, args),
        Command::Serve(o, port, flat_out) => serve(o, *port, *flat_out),
        Command::Sm3(o, bind, port) => sm3(o, bind, *port),
        Command::Bench(o, seconds) => bench(o, *seconds),
    }
}

fn trace(opts: &TraceOptions) -> Result<String, String> {
    let TraceOptions {
        flash,
        max,
        force,
        out,
        address_errors,
        history,
        adc,
        eeprom,
        layout,
        eeprom_out,
    } = opts;
    let (max, address_errors, history, out) = (*max, *address_errors, *history, out.as_ref());
    let image = read(flash)?;
    let mut chip = Sh7059::new(&image).map_err(|e| format!("{}: {e}", flash.display()))?;
    for &(a, m) in force.iter() {
        if !matches!(region(a), Region::Io(_)) {
            return Err(format!("--force {a:#010X}: not a peripheral register"));
        }
        chip.force_bits(a, m);
    }
    if out.is_some() {
        chip.trace = Some(Vec::new());
    }
    for &(c, v) in adc {
        chip.adc.input[c] = v;
    }
    if let Some(path) = eeprom {
        let e = Eeprom::new(&read(path)?).map_err(|e| format!("{}: {e}", path.display()))?;
        if e.len() != 4 * board::CHIP_BYTES {
            return Err(format!(
                "{}: {} bytes; the board has four 93C86, 8192 bytes",
                path.display(),
                e.len()
            ));
        }
        board::wire_eeproms(&mut chip, &e, *layout);
    }
    let mut m = Machine::new(chip, address_errors);
    // The last steps before the first exception, to say where it came from.
    let mut recent = std::collections::VecDeque::with_capacity(32);
    let mut first_exception = None;
    let start = std::time::Instant::now();
    if !history {
        m.run(max);
    }
    while m.cpu.instructions < max {
        if first_exception.is_none() {
            if recent.len() == 32 {
                recent.pop_front();
            }
            recent.push_back((m.cpu.instructions, m.cpu.clone()));
        }
        let before = m.cpu.exceptions;
        if !m.step() {
            break;
        }
        if m.cpu.exceptions != before && first_exception.is_none() {
            first_exception = Some(m.cpu.last_exception);
        }
    }
    let secs = start.elapsed().as_secs_f64();
    let Machine {
        cpu,
        mut chip,
        interrupts,
        ..
    } = m;

    let mut o = String::new();
    if let Some(v) = first_exception {
        writeln!(o, "first exception: vector {v:?}; the steps before it:").unwrap();
        for (n, c) in &recent {
            let code = sh2e::Bus::read16(&mut chip, c.pc);
            writeln!(
                o,
                "  {n:>9}  PC {:08X}  {code:04X} {:?}  R0-R7 {:08X?}  R15 {:08X}",
                c.pc,
                sh2e::decode::DECODE[code as usize],
                &c.r[..8],
                c.r[15]
            )
            .unwrap();
        }
    }
    writeln!(
        o,
        "executed {} instructions, {} cycles (minimum counts) in {:.3} s: {:.1} M instructions/s",
        cpu.instructions,
        cpu.cycles,
        secs,
        cpu.instructions as f64 / secs / 1e6
    )
    .unwrap();
    writeln!(
        o,
        "PC 0x{:08X}  SR 0x{:08X}  VBR 0x{:08X}  R15 0x{:08X}{}",
        cpu.pc,
        cpu.sr,
        cpu.vbr,
        cpu.r[15],
        if cpu.sleeping { "  (asleep)" } else { "" }
    )
    .unwrap();
    writeln!(
        o,
        "exceptions {} (last vector {:?}); unmapped accesses {}; flash writes {}",
        cpu.exceptions, cpu.last_exception, chip.unmapped_accesses, chip.rom_writes
    )
    .unwrap();
    writeln!(o, "unverified paths: {:?}", cpu.unverified).unwrap();
    let taken: Vec<String> = interrupts
        .iter()
        .enumerate()
        .filter(|(_, n)| **n > 0)
        .map(|(v, n)| format!("vector {v}: {n}"))
        .collect();
    for (k, c) in chip.sci.ch.iter().enumerate() {
        if !c.sent.is_empty() {
            let head: Vec<String> = c.sent.iter().take(16).map(|b| format!("{b:02X}")).collect();
            writeln!(
                o,
                "SCI{k}: {} bytes sent, {} received; first sent: {}",
                c.sent.len(),
                c.received.len(),
                head.join(" ")
            )
            .unwrap();
        }
    }
    writeln!(o, "A/D rounds {:?}", chip.adc.rounds).unwrap();
    for (k, c) in chip.hcan.ch.iter().enumerate() {
        let mut ids =
            std::collections::BTreeMap::<(u32, bool), (u64, u64, sh7059::hcan::Frame)>::new();
        for (t, f) in &c.outbox {
            let e = ids.entry((f.id, f.extended)).or_insert((0, *t, *f));
            e.0 += 1;
        }
        if ids.is_empty() && c.received == 0 {
            continue;
        }
        let list: Vec<String> = ids
            .iter()
            .map(|((id, ext), (n, first, f))| {
                format!(
                    "{}{:X} x{n} (first at cycle {first}: {})",
                    if *ext { "x" } else { "" },
                    id,
                    f.data[..f.dlc as usize]
                        .iter()
                        .map(|b| format!("{b:02X}"))
                        .collect::<Vec<_>>()
                        .join(" ")
                )
            })
            .collect();
        writeln!(
            o,
            "HCAN{k}: {} frames sent: {}; received {}, unmatched {}",
            c.outbox.len(),
            list.join(", "),
            c.received,
            c.unmatched
        )
        .unwrap();
    }
    if eeprom.is_some() {
        let s = board::eeprom_stats(&chip);
        writeln!(
            o,
            "EEPROM words read {:?}, programmed {:?}, refused {:?} (PJ3, PJ4, PD0, PD1)",
            s.iter().map(|x| x.0).collect::<Vec<_>>(),
            s.iter().map(|x| x.1).collect::<Vec<_>>(),
            s.iter().map(|x| x.2).collect::<Vec<_>>()
        )
        .unwrap();
        if let Some(path) = eeprom_out {
            let img = board::eeprom_image(&chip, *layout).expect("EEPROMs wired");
            std::fs::write(path, img).map_err(|e| format!("{}: {e}", path.display()))?;
            writeln!(o, "EEPROM saved to {}", path.display()).unwrap();
        }
    }
    writeln!(
        o,
        "DMA units {}; channels left on a peripheral request (not modelled) {}",
        chip.dmac.transfers, chip.dmac.peripheral_requests
    )
    .unwrap();
    writeln!(
        o,
        "interrupts taken: {}; CMT matches {:?}; sleeping {}",
        if taken.is_empty() {
            "none".to_string()
        } else {
            taken.join(", ")
        },
        chip.cmt.matches,
        cpu.sleeping
    )
    .unwrap();
    if let (Some(path), Some(t)) = (out, chip.trace.as_ref()) {
        let mut text = String::with_capacity(t.len() * 28);
        for a in t {
            writeln!(
                text,
                "{:X} {} {:X} {} {:X}",
                a.pc,
                if a.write { 'W' } else { 'R' },
                a.addr,
                a.size,
                a.value
            )
            .unwrap();
        }
        std::fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))?;
        writeln!(o, "register accesses {} -> {}", t.len(), path.display()).unwrap();
    }
    Ok(o)
}

/// The sensors `o` describes; `--adc` and `--maf-hz` go on top of these.
fn sensors(o: &EcuOptions) -> Result<engine::sensors::Sensors, String> {
    use engine::sensors::Sensors;
    let mode = o
        .sensors
        .as_deref()
        .unwrap_or(if o.rpm.is_some() { "idle" } else { "on" });
    let mut s = match mode {
        "off" => Sensors { values: Vec::new() },
        "idle" => Sensors::idle(),
        _ => Sensors::ignition_on(),
    };
    for kv in &o.sensor {
        let (k, v) = engine::sensors::parse(kv)?;
        s.set(k, v)?;
    }
    Ok(s)
}

/// Builds and boots the ECU described by `o`.
fn boot(o: &EcuOptions) -> Result<Machine, String> {
    let image = read(&o.flash)?;
    let mut chip = Sh7059::new(&image).map_err(|e| format!("{}: {e}", o.flash.display()))?;
    if let Some(path) = &o.eeprom {
        let e = Eeprom::new(&read(path)?).map_err(|e| format!("{}: {e}", path.display()))?;
        if e.len() != 4 * board::CHIP_BYTES {
            return Err(format!(
                "{}: {} bytes; the board has four 93C86, 8192 bytes",
                path.display(),
                e.len()
            ));
        }
        board::wire_eeproms(&mut chip, &e, o.layout);
    }
    let sensors = sensors(o)?;
    for (c, v) in sensors.codes() {
        chip.adc.input[c] = v;
    }
    for &(c, v) in &o.adc {
        chip.adc.input[c] = v;
    }
    let mut pins = sh7059::pins::Merge::default();
    if let Some(rpm) = o.rpm {
        pins.0
            .push(Box::new(engine::wheel::Wheels::new(rpm as f64, 0)));
    }
    if let Some(hz) = o.maf_hz.map(f64::from).or(sensors.maf_hz()) {
        pins.0.push(Box::new(sh7059::pins::SquareWave::new(
            sh7059::pins::Pin::Ti0(2),
            engine::sensors::MAF_KEY,
            hz,
            engine::wheel::CYCLES_PER_S,
            0,
        )));
    }
    if !pins.0.is_empty() {
        chip.set_stimulus(Box::new(pins));
    }
    if o.peers == "abs" {
        chip.set_can_source(Box::new(engine::bus::abs()));
    }
    let mut m = Machine::new(chip, true);
    m.run(o.boot);
    Ok(m)
}

fn bench(o: &EcuOptions, seconds: u32) -> Result<String, String> {
    let t = std::time::Instant::now();
    let mut m = boot(o)?;
    let booted = t.elapsed().as_secs_f64();
    let (c0, i0) = (m.cpu.cycles, m.cpu.instructions);
    let target = c0 + seconds as u64 * bridge::link::CYCLES_PER_US * 1_000_000;
    let t = std::time::Instant::now();
    while m.cpu.cycles < target {
        // As RealTimeEcu does: about 100 µs at a time, the bus emptied in between.
        m.run(8_000);
        m.chip.hcan.ch[0].outbox.clear();
    }
    let wall = t.elapsed().as_secs_f64();
    let ecu = (m.cpu.cycles - c0) as f64 / (bridge::link::CYCLES_PER_US as f64 * 1e6);
    let instructions = m.cpu.instructions - i0;
    Ok(format!(
        "booted in {booted:.2} s; then {ecu:.3} s of ECU time in {wall:.3} s: real-time \
         factor {:.3}; {} instructions, {:.1} M instructions/s\n",
        ecu / wall,
        instructions,
        instructions as f64 / wall / 1e6
    ))
}

fn obd(o: &EcuOptions, args: &[String]) -> Result<String, String> {
    let mut m = boot(o)?;
    let mut link = bridge::link::VirtualLink::new(&mut m, 0);
    let mut elm = bridge::elm::Elm327::new();
    elm.echo = false;
    let mut out = String::new();
    for a in args {
        let answer = elm.line(a, &mut link);
        let text = answer
            .trim_end_matches('>')
            .trim_end()
            .replace('\r', "\n    ");
        writeln!(out, "{a}\n    {text}").unwrap();
    }
    Ok(out)
}

fn serve(o: &EcuOptions, port: u16, flat_out: bool) -> Result<String, String> {
    let m = boot(o)?;
    let listener = std::net::TcpListener::bind(("127.0.0.1", port))
        .map_err(|e| format!("port {port}: {e}"))?;
    eprintln!(
        "ELM327 on 127.0.0.1:{port}; ECU running {}",
        if flat_out { "flat out" } else { "in real time" }
    );
    let mut ecu = bridge::link::RealTimeEcu::spawn(m, 0, !flat_out);
    report_speed(ecu.stats.clone());
    bridge::server::serve(listener, &mut ecu, &mut |s| eprintln!("{s}"))
        .map_err(|e| e.to_string())?;
    Ok(String::new())
}

/// Prints, every ten seconds, how the emulation keeps up with the wall clock.
fn report_speed(stats: std::sync::Arc<bridge::link::Stats>) {
    use std::sync::atomic::Ordering::Relaxed;
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(std::time::Duration::from_secs(10));
            let c = stats.cycles.load(Relaxed) as f64;
            let w = stats.wall_us.load(Relaxed) as f64;
            let lag = stats.worst_lag_us.load(Relaxed);
            if w > 0.0 {
                eprintln!(
                    "real-time factor {:.2}, worst lag {:.1} ms",
                    c / bridge::link::CYCLES_PER_US as f64 / w,
                    lag as f64 / 1000.0
                );
            }
        }
    });
}

fn sm3(o: &EcuOptions, bind: &str, port: u16) -> Result<String, String> {
    let m = boot(o)?;
    let listener =
        std::net::TcpListener::bind((bind, port)).map_err(|e| format!("{bind}:{port}: {e}"))?;
    eprintln!("emulated SM3 on {bind}:{port}; point opendash's adapter address here");
    let mut ecu = bridge::link::RealTimeEcu::spawn(m, 0, true);
    report_speed(ecu.stats.clone());
    bridge::sm3::serve(listener, &mut ecu, &mut |s| eprintln!("{s}")).map_err(|e| e.to_string())?;
    Ok(String::new())
}

fn disasm(flash: &PathBuf, from: u32, count: u32) -> Result<String, String> {
    let image = read(flash)?;
    let mut chip = Sh7059::new(&image).map_err(|e| format!("{}: {e}", flash.display()))?;
    let mut o = String::new();
    for k in 0..count {
        let pc = from.wrapping_add(2 * k);
        if !matches!(region(pc), Region::Rom(_)) {
            break;
        }
        let code = sh2e::Bus::read16(&mut chip, pc);
        let text = sh2e::disasm::disassemble(code, pc);
        let note = match sh2e::disasm::literal(code, pc) {
            Some((a, 4)) => format!("  ; = H'{:08X}", sh2e::Bus::read32(&mut chip, a)),
            Some((a, _)) => format!("  ; = H'{:04X}", sh2e::Bus::read16(&mut chip, a)),
            None => String::new(),
        };
        writeln!(o, "{pc:08X}  {code:04X}  {text}{note}").unwrap();
    }
    Ok(o)
}

fn info(flash: &PathBuf, eeprom: Option<&PathBuf>) -> Result<String, String> {
    let image = read(flash)?;
    let mut chip = Sh7059::new(&image).map_err(|e| format!("{}: {e}", flash.display()))?;
    let mut out = String::new();
    let o = &mut out;

    writeln!(o, "flash   {}  {} bytes", flash.display(), image.len()).unwrap();
    writeln!(o, "vectors").unwrap();
    for (v, what) in [
        (Vector::PowerOnPc, "power-on reset PC"),
        (Vector::PowerOnSp, "power-on reset SP"),
        (Vector::ManualPc, "manual reset PC  "),
        (Vector::ManualSp, "manual reset SP  "),
    ] {
        let value = sh2e::Bus::read32(&mut chip, v.address());
        writeln!(
            o,
            "  {}  {what}  0x{value:08X}  {}",
            v as u32,
            region_name(value)
        )
        .unwrap();
    }

    let mut cpu = Cpu::new();
    cpu.power_on_reset(&mut chip);
    writeln!(
        o,
        "reset   PC 0x{:08X}  R15 0x{:08X}  SR 0x{:08X}  VBR 0x{:08X}  FPSCR 0x{:08X}",
        cpu.pc, cpu.r[15], cpu.sr, cpu.vbr, cpu.fpscr
    )
    .unwrap();

    let mut problems = Vec::new();
    if !matches!(region(cpu.pc), Region::Rom(_)) || cpu.pc % 2 != 0 {
        problems.push(format!(
            "reset PC 0x{:08X} is not an even flash address",
            cpu.pc
        ));
    }
    // The stack grows down: the initial SP may be the address just past the end of RAM.
    if !matches!(region(cpu.r[15].wrapping_sub(4)), Region::Ram(_)) || cpu.r[15] % 4 != 0 {
        problems.push(format!(
            "reset SP 0x{:08X} does not point into RAM",
            cpu.r[15]
        ));
    }

    if let Some(path) = eeprom {
        let e = Eeprom::new(&read(path)?).map_err(|e| format!("{}: {e}", path.display()))?;
        let erased = e.data().iter().filter(|&&b| b == 0xFF).count();
        writeln!(
            o,
            "eeprom  {}  {} bytes, {} erased (0xFF)",
            path.display(),
            e.len(),
            erased
        )
        .unwrap();
    } else {
        writeln!(o, "eeprom  none").unwrap();
    }

    if problems.is_empty() {
        writeln!(o, "check   ok").unwrap();
        Ok(out)
    } else {
        Err(format!("{out}{}", problems.join("\n")))
    }
}
