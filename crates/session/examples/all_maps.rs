//! Every map and curve whose descriptor sits anywhere in the flash — whether the
//! firmware reads it at run time or not. The exhaustive catalogue, the way WinOLS
//! walks a file, so the emulator serves as an analysis tool and not only as a run.
//!
//! `maps` and `touched` list the maps the firmware looks up while running (through
//! the interpolation masters). This walks the whole ROM instead and keeps every
//! descriptor that parses and validates as a 3D map or a 2D curve, including the
//! ones nothing calls.
//!
//!   cargo run -p session --release --example all_maps -- <flash.bin> [--csv <out.csv>]
//!
//! Reuses `session::maps::Map::{read3d, read2d}` for the descriptor format and its
//! checks (axes finite and non-decreasing, pointers in range, valid cell type). On
//! top of those it keeps a candidate only when its axes and cells live in ROM, its
//! axes strictly increase (which drops the all-zero axes a RAM pointer would read as
//! and other flat false positives), and its cells fit the image. Deduped by the
//! (X, Y, Z) addresses, so two descriptors onto the same data count once.
//!
//! Reads a file; touches no hardware and boots nothing.

use session::maps::Map;
use std::collections::BTreeSet;
use std::fmt::Write as _;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let Some(path) = args.get(1).filter(|a| !a.starts_with("--")) else {
        eprintln!("uso: all_maps <flash.bin> [--csv <out.csv>]");
        std::process::exit(2);
    };
    let csv_out = args
        .iter()
        .position(|a| a == "--csv")
        .and_then(|i| args.get(i + 1))
        .cloned();

    let flash = std::fs::read(path).unwrap_or_else(|e| {
        eprintln!("{path}: {e}");
        std::process::exit(1);
    });
    let rom = flash.len() as u32;
    let read = move |a: u32| *flash.get(a as usize).unwrap_or(&0);

    let in_rom = |a: u32| a < rom;
    let strictly_up = |ax: &[f32]| {
        ax.len() >= 2 && ax.first().copied().unwrap_or(f32::NAN) < ax.last().copied().unwrap_or(f32::NAN)
    };

    // Every 4-aligned offset is tried as a descriptor: the masters get its address
    // from a PC-relative literal, so a descriptor is word-aligned and its u32 axis
    // and data pointers are naturally aligned.
    let mut maps: Vec<Map> = Vec::new();
    let mut seen: BTreeSet<(u32, u32, u32)> = BTreeSet::new();
    for d in (0..rom).step_by(4) {
        // 3D first (a Y axis to place it), then 2D at the same spot.
        let cand = Map::read3d(&read, d)
            .filter(|m| m.y_at != 0 && !m.y.is_empty())
            .or_else(|| Map::read2d(&read, d));
        let Some(m) = cand else { continue };

        if !in_rom(m.x_at) || !in_rom(m.z_at) || (m.dims() == 3 && !in_rom(m.y_at)) {
            continue;
        }
        let z_bytes = m.z.len() as u32 * m.cell.bytes();
        if m.z_at.checked_add(z_bytes).is_none_or(|end| end > rom) {
            continue;
        }
        if !strictly_up(&m.x) || (m.dims() == 3 && !strictly_up(&m.y)) {
            continue;
        }
        if !seen.insert((m.x_at, m.y_at, m.z_at)) {
            continue;
        }
        maps.push(m);
    }

    maps.sort_by_key(|m| m.desc);
    let n3 = maps.iter().filter(|m| m.dims() == 3).count();
    let n2 = maps.len() - n3;
    println!(
        "{}: {} maps in the calibration — {} 3D, {} 2D (every descriptor in ROM, read or not)",
        path,
        maps.len(),
        n3,
        n2
    );

    if let Some(out) = csv_out {
        let mut s = String::from("desc,dims,nx,ny,type,scale,offset,x_at,y_at,z_at\n");
        for m in &maps {
            writeln!(
                s,
                "0x{:06X},{},{},{},{},{},{},0x{:06X},0x{:06X},0x{:06X}",
                m.desc,
                m.dims(),
                m.nx(),
                if m.dims() == 3 { m.ny() } else { 0 },
                m.cell.name(),
                m.scale,
                m.offset,
                m.x_at,
                m.y_at,
                m.z_at
            )
            .unwrap();
        }
        std::fs::write(&out, s).unwrap_or_else(|e| {
            eprintln!("{out}: {e}");
            std::process::exit(1);
        });
        println!("escrito {out}");
    } else {
        println!("\n  descriptor   dims   nx×ny   type   escala      offset    datos");
        for m in maps.iter().take(40) {
            println!(
                "  0x{:06X}    {}D   {:>3}×{:<3}  {:>5}  {:>9.4}  {:>9.3}  0x{:06X}",
                m.desc,
                m.dims(),
                m.nx(),
                if m.dims() == 3 { m.ny() } else { 1 },
                m.cell.name(),
                m.scale,
                m.offset,
                m.z_at
            );
        }
        if maps.len() > 40 {
            println!("  … y {} más (usa --csv para todos)", maps.len() - 40);
        }
    }
}
