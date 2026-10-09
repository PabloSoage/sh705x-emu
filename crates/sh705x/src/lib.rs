//! Command-line front end. The binary is a thin wrapper around [`run`], so every
//! command can be tested without spawning a process.

use board::Eeprom;
use sh2e::{Cpu, Vector};
use sh7059::{Region, Sh7059, region};
use std::fmt::Write as _;
use std::path::PathBuf;

/// Usage text.
pub const USAGE: &str = "\
usage: sh705x info  --flash <flash.bin> [--eeprom <eeprom.bin>]
       sh705x trace --flash <flash.bin> [--max <instructions>] [--force <addr:mask>]...
                    [--out <io-trace.txt>] [--no-address-errors] [--history]

commands:
  info    load the images, check them and show the reset vectors and reset state
  trace   run from reset with peripheral registers as plain storage, optionally forcing
          status bits as phase 0 did, and write every register access to --out as
          'pc R|W addr size value' (hex). Prints instructions, cycles and speed.
          --no-address-errors makes misaligned accesses go through, as in Ghidra's
          emulator; only for comparing with it. --history prints the steps before the
          first exception";

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
        "trace" => {
            let (mut flash, mut out, mut max, mut force) = (None, None, 3_000_000, Vec::new());
            let (mut address_errors, mut history) = (true, false);
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
                    "--max" => max = value.parse().map_err(|_| format!("bad --max '{value}'"))?,
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
            }))
        }
        other => Err(format!("unknown command '{other}'")),
    }
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
    let mut cpu = Cpu::new();
    cpu.address_errors = address_errors;
    cpu.power_on_reset(&mut chip);
    // The last steps before the first exception, to say where it came from.
    let mut recent = std::collections::VecDeque::with_capacity(32);
    let mut first_exception = None;
    let start = std::time::Instant::now();
    if !history && out.is_none() {
        // Nothing to record per step: let the core run on its own.
        cpu.run(&mut chip, max);
    }
    while cpu.instructions < max && !cpu.sleeping {
        chip.trace_pc = cpu.pc;
        if history && first_exception.is_none() {
            if recent.len() == 32 {
                recent.pop_front();
            }
            recent.push_back((cpu.instructions, cpu.clone()));
        }
        let before = cpu.exceptions;
        cpu.step(&mut chip);
        if cpu.exceptions != before && first_exception.is_none() {
            first_exception = Some(cpu.last_exception);
        }
    }
    let secs = start.elapsed().as_secs_f64();

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
