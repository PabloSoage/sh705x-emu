//! `sh705x catalog`: every map and curve whose descriptor sits anywhere in the flash,
//! read at run time or not — the exhaustive catalogue, the way WinOLS walks a file.
//!
//! `maps` and `touched` show the maps the firmware looks up while running. This walks the
//! whole ROM through [`session::maps::catalog`] and keeps every descriptor that parses and
//! validates as a 3D map or a 2D curve, each marked referenced (a master call loads its
//! descriptor into R4) or unreferenced (it parses but nothing reads it: an unused
//! calibration, or a false positive). Reads a file; boots nothing.

use session::maps::{self, Found};
use std::fmt::Write as _;
use std::path::PathBuf;

/// Options of `sh705x catalog`.
#[derive(Debug, PartialEq, Eq)]
pub struct CatalogOptions {
    /// Path to the flash image.
    pub flash: PathBuf,
    /// Where to write the catalogue as CSV, if asked.
    pub out: Option<PathBuf>,
    /// Keep only 2D curves (`Some(2)`), only 3D maps (`Some(3)`), or both (`None`).
    pub dims: Option<u8>,
    /// Keep only the maps the firmware does not read.
    pub unreferenced: bool,
}

/// A one-line count of a set of found maps: 3D/2D and referenced/unreferenced.
fn summary(found: &[&Found]) -> String {
    let n3 = found.iter().filter(|f| f.map.dims() == 3).count();
    let refd = found.iter().filter(|f| f.referenced).count();
    format!(
        "{} maps — {} 3D, {} 2D; {} referenced, {} unreferenced",
        found.len(),
        n3,
        found.len() - n3,
        refd,
        found.len() - refd
    )
}

/// Runs `sh705x catalog`.
pub fn catalog(o: &CatalogOptions) -> Result<String, String> {
    let flash = std::fs::read(&o.flash).map_err(|e| format!("{}: {e}", o.flash.display()))?;
    let all = maps::catalog(&flash);
    let all_refs: Vec<&Found> = all.iter().collect();

    let mut found: Vec<&Found> = all
        .iter()
        .filter(|f| o.dims.is_none_or(|d| f.map.dims() == d))
        .filter(|f| !o.unreferenced || !f.referenced)
        .collect();
    found.sort_by_key(|f| f.map.desc);

    let mut out = String::new();
    writeln!(
        out,
        "{}: whole-ROM scan, {} (every descriptor in ROM, read or not)",
        o.flash.display(),
        summary(&all_refs)
    )
    .unwrap();

    if let Some(path) = &o.out {
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
        std::fs::write(path, s).map_err(|e| format!("{}: {e}", path.display()))?;
        writeln!(out, "{} rows written to {}", found.len(), path.display()).unwrap();
        return Ok(out);
    }

    writeln!(
        out,
        "\n  descriptor  dims   nx×ny   type        scale      offset   ref  data"
    )
    .unwrap();
    const SHOWN: usize = 60;
    for f in found.iter().take(SHOWN) {
        let m = &f.map;
        writeln!(
            out,
            "  0x{:06X}   {}D   {:>3}×{:<3}  {:>5}  {:>10.4}  {:>10.3}  {:>3}  0x{:06X}",
            m.desc,
            m.dims(),
            m.nx(),
            if m.dims() == 3 { m.ny() } else { 1 },
            m.cell.name(),
            m.scale,
            m.offset,
            if f.referenced { "yes" } else { "no" },
            m.z_at
        )
        .unwrap();
    }
    if found.len() > SHOWN {
        writeln!(
            out,
            "  … and {} more (use --out <csv> for all)",
            found.len() - SHOWN
        )
        .unwrap();
    }
    writeln!(out, "\nshown: {}", summary(&found)).unwrap();
    Ok(out)
}
