//! A plugin's manifest: JSON in the custom section `sh705x-plugin` of its module.
//!
//! ```json
//! {"name": "A node", "version": "1.0.0", "api": 1,
//!  "description": "What it does",
//!  "reads": [["FFFE8000", "FFFE8100"]],
//!  "tick_us": 1000}
//! ```
//!
//! `reads` are ranges (start, end exclusive, hexadecimal) of the ECU's memory the plugin
//! can `read`; `tick_us` how often its `tick` is called, in ECU time.

use serde_json::Value;

/// The custom section that holds the manifest.
pub const SECTION: &str = "sh705x-plugin";

/// Bytes of memory a plugin may read at most, over all its ranges.
pub const MAX_READ: u32 = 0x1_0000;

/// What a plugin says about itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Manifest {
    /// Its name.
    pub name: String,
    /// Its version.
    pub version: String,
    /// The API it was written for.
    pub api: u32,
    /// What it does.
    pub description: String,
    /// The memory it reads: start, end exclusive.
    pub reads: Vec<(u32, u32)>,
    /// How often its `tick` is called, microseconds of ECU time.
    pub tick_us: u64,
}

impl Manifest {
    /// The manifest of the module `wasm`.
    pub fn of(wasm: &[u8]) -> Result<Self, String> {
        let json = custom_section(wasm, SECTION).ok_or_else(|| {
            if wasm.starts_with(b"\0asm") {
                format!("no '{SECTION}' section: not a plugin for this emulator")
            } else {
                "not a WebAssembly module".to_string()
            }
        })?;
        Self::parse(json)
    }

    /// Reads the JSON of a manifest.
    pub fn parse(json: &[u8]) -> Result<Self, String> {
        let v: Value =
            serde_json::from_slice(json).map_err(|e| format!("its manifest is not JSON: {e}"))?;
        let text = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
        let name = text("name").ok_or("its manifest has no name")?;
        let version = text("version").ok_or("its manifest has no version")?;
        let api = v
            .get("api")
            .and_then(Value::as_u64)
            .ok_or("its manifest has no api")? as u32;
        if api != crate::API {
            return Err(format!(
                "written for API {api}; this emulator offers {}",
                crate::API
            ));
        }
        let mut reads = Vec::new();
        for r in v
            .get("reads")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default()
        {
            let pair = r
                .as_array()
                .filter(|p| p.len() == 2)
                .ok_or("a range in 'reads' is not [start, end]")?;
            let hex = |x: &Value| {
                x.as_str()
                    .and_then(|s| u32::from_str_radix(s.trim_start_matches("0x"), 16).ok())
                    .ok_or_else(|| format!("'{x}' in 'reads' is not a hexadecimal address"))
            };
            let (s, e) = (hex(&pair[0])?, hex(&pair[1])?);
            if e <= s {
                return Err(format!("the range {s:08X}..{e:08X} in 'reads' is empty"));
            }
            reads.push((s, e));
        }
        let total: u64 = reads.iter().map(|&(s, e)| (e - s) as u64).sum();
        if total > MAX_READ as u64 {
            return Err(format!(
                "it reads {total} bytes; at most {MAX_READ} are given"
            ));
        }
        Ok(Self {
            name,
            version,
            api,
            description: text("description").unwrap_or_default(),
            reads,
            tick_us: v
                .get("tick_us")
                .and_then(Value::as_u64)
                .unwrap_or(1000)
                .max(100),
        })
    }
}

/// An unsigned LEB128 number at the start of `b`, and its length.
fn leb(b: &[u8]) -> Option<(usize, usize)> {
    let mut v: usize = 0;
    for (k, &x) in b.iter().enumerate().take(5) {
        v |= ((x & 0x7F) as usize) << (7 * k);
        if x & 0x80 == 0 {
            return Some((v, k + 1));
        }
    }
    None
}

/// The contents of the custom section `name` of the module `wasm`, if it has one.
pub fn custom_section<'a>(wasm: &'a [u8], name: &str) -> Option<&'a [u8]> {
    if wasm.get(..8)? != b"\0asm\x01\0\0\0" {
        return None;
    }
    let mut p = 8;
    while p < wasm.len() {
        let id = wasm[p];
        let (size, n) = leb(&wasm[p + 1..])?;
        let start = p + 1 + n;
        let end = start.checked_add(size)?;
        if end > wasm.len() {
            return None;
        }
        if id == 0 {
            let (len, m) = leb(&wasm[start..end])?;
            let s = start + m;
            let e = s.checked_add(len)?;
            if e <= end && &wasm[s..e] == name.as_bytes() {
                return Some(&wasm[e..end]);
            }
        }
        p = end;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_manifest_is_read_and_checked() {
        let m = Manifest::parse(
            br#"{"name":"n","version":"1","api":1,"reads":[["FFFE8000","FFFE8010"]]}"#,
        )
        .unwrap();
        assert_eq!(m.reads, [(0xFFFE_8000, 0xFFFE_8010)]);
        assert_eq!(m.tick_us, 1000);
        assert!(Manifest::parse(br#"{"name":"n","version":"1","api":2}"#).is_err());
        assert!(Manifest::parse(br#"{"name":"n","api":1}"#).is_err());
        assert!(
            Manifest::parse(br#"{"name":"n","version":"1","api":1,"reads":[["0","20000"]]}"#)
                .is_err()
        );
        assert!(Manifest::of(b"not wasm").is_err());
    }
}
