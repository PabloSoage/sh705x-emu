//! Every map and curve whose descriptor sits anywhere in the flash — whether the
//! firmware reads it at run time or not. The exhaustive catalogue, the way WinOLS walks a
//! file, so the emulator serves as an analysis tool and not only as a run.
//!
//! `maps` and `touched` list the maps the firmware looks up while running (through the
//! interpolation masters). This walks the whole ROM instead and keeps every descriptor
//! that parses and validates as a 3D map or a 2D curve, including the ones nothing calls,
//! each marked referenced (the firmware reads it) or not.
//!
//!   cargo run -p session --release --example all_maps -- <flash.bin> [--csv <out.csv>]
//!
//! A thin wrapper around `session::maps::catalog`, which the `sh705x catalog` command
//! also drives. Reads a file; touches no hardware and boots nothing.

use session::maps::catalog;
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

    let found = catalog(&flash);
    let n3 = found.iter().filter(|f| f.map.dims() == 3).count();
    let n2 = found.len() - n3;
    let refd = found.iter().filter(|f| f.referenced).count();
    println!(
        "{}: {} maps in the calibration — {} 3D, {} 2D; {} referenced, {} unreferenced \
         (every descriptor in ROM, read or not)",
        path,
        found.len(),
        n3,
        n2,
        refd,
        found.len() - refd
    );

    if let Some(out) = csv_out {
        let mut s = String::from("desc,dims,nx,ny,type,scale,offset,referenced,x_at,y_at,z_at\n");
        for f in &found {
            let m = &f.map;
            writeln!(
                s,
                "0x{:06X},{},{},{},{},{},{},{},0x{:06X},0x{:06X},0x{:06X}",
                m.desc,
                m.dims(),
                m.nx(),
                if m.dims() == 3 { m.ny() } else { 0 },
                m.cell.name(),
                m.scale,
                m.offset,
                f.referenced,
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
        println!("\n  descriptor   dims   nx×ny   type   escala      offset   ref  datos");
        for f in found.iter().take(40) {
            let m = &f.map;
            println!(
                "  0x{:06X}    {}D   {:>3}×{:<3}  {:>5}  {:>9.4}  {:>9.3}  {:>3}  0x{:06X}",
                m.desc,
                m.dims(),
                m.nx(),
                if m.dims() == 3 { m.ny() } else { 1 },
                m.cell.name(),
                m.scale,
                m.offset,
                if f.referenced { "yes" } else { "no" },
                m.z_at
            );
        }
        if found.len() > 40 {
            println!("  … y {} más (usa --csv para todos)", found.len() - 40);
        }
    }
}
