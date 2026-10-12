//! Names for what the maps read and give: variables in RAM, routines whose result is an
//! input, and the maps themselves.
//!
//! Some come with the emulator: what this project found in one calibration
//! ([`Names::builtin`], only when the flash is that calibration), and the sensors, found by
//! moving each one and seeing which variable follows ([`crate::Session::find_sensors`]).
//! The rest are loaded from the user's own files at run time and never ship with the
//! emulator: an opendash parameter profile, matched against RAM by asking the emulated
//! ECU for each identifier ([`crate::Session::find_identifiers`]), and map labels.

use std::collections::BTreeMap;

/// How a name was found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum How {
    /// Known for this calibration from this project's own work.
    Calibration,
    /// A sensor moved and this variable followed it.
    Sensor,
    /// An identifier of a parameter profile gave this variable's value, in two engine
    /// states (`sure`) or in one.
    Identifier {
        /// The identifier (a mode 01 PID below 0x100, else a $22 DID).
        pid: u32,
        /// Matched in two states that gave different values.
        sure: bool,
    },
    /// From a label file.
    Label,
}

/// A name, its unit, and how it was found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Name {
    /// What it is.
    pub text: String,
    /// Its unit, if known.
    pub unit: String,
    /// How it was found.
    pub how: How,
}

impl Name {
    fn new(text: &str, unit: &str, how: How) -> Self {
        Self {
            text: text.into(),
            unit: unit.into(),
            how,
        }
    }
}

/// Every name known.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Names {
    /// Variables, by address.
    pub vars: BTreeMap<u32, Name>,
    /// Routines whose result is an input, by entry address.
    pub routines: BTreeMap<u32, Name>,
    /// Maps, by descriptor.
    pub maps: BTreeMap<u32, Name>,
}

/// The Astra J's calibration, as its flash names itself.
pub const ASTRA: &str = "NCLSW420AX36";

impl Names {
    /// The names known for the calibration `id` (as found in the flash by
    /// [`calibration`]): this project's findings for the Astra J's NCLSW420AX36
    /// (`Denso_SH705x` 18, and resultados fases 8, 9 and 11); none for another.
    pub fn builtin(id: Option<&str>) -> Self {
        let mut n = Self::default();
        if id != Some(ASTRA) {
            return n;
        }
        let c = How::Calibration;
        for (a, text, unit) in [
            (0xFFFE_B7E4, "engine speed", "rpm"),
            (0xFFFE_FC4C, "torque asked", "N·m"),
            (0xFFFE_F8F4, "fuel asked per stroke", "mm³"),
            (0xFFFF_1370, "EGR asked", "%"),
            (0xFFFE_FE84, "boost asked", "kPa"),
            (0xFFFE_F9DC, "SCV current asked", "mA"),
            (0xFFFE_F924, "rail pressure asked", "MPa"),
            (0xFFFE_F9C0, "SCV command", "%"),
            (0xFFFE_FDDC, "VGT command", "%"),
        ] {
            n.vars.insert(a, Name::new(text, unit, c.clone()));
        }
        for (a, text) in [
            (0xD432, "engine speed"),
            (0x7640, "a calibration constant (offset + u8 × scale)"),
            (0x74C4, "min(a, b)"),
            (0x74B4, "max(a, b)"),
        ] {
            n.routines.insert(a, Name::new(text, "", c.clone()));
        }
        n
    }

    /// Adds `other`'s names; a name already there stays.
    pub fn merge(&mut self, other: Names) {
        for (a, v) in other.vars {
            self.vars.entry(a).or_insert(v);
        }
        for (a, v) in other.routines {
            self.routines.entry(a).or_insert(v);
        }
        for (a, v) in other.maps {
            self.maps.entry(a).or_insert(v);
        }
    }
}

/// The calibration a flash names itself, if it carries one of the known ones.
pub fn calibration(flash: &[u8]) -> Option<&'static str> {
    [ASTRA]
        .into_iter()
        .find(|id| flash.windows(id.len()).any(|w| w == id.as_bytes()))
}

/// One parameter of a profile: an identifier and how its bytes become a value.
#[derive(Clone, Debug, PartialEq)]
pub struct Parameter {
    /// Its name.
    pub name: String,
    /// The identifier: a mode 01 PID below 0x100, else a $22 DID.
    pub pid: u32,
    /// Bytes of the value.
    pub bytes: usize,
    /// Value = raw × `scale` + `offset`.
    pub scale: f64,
    /// See [`Parameter::scale`].
    pub offset: f64,
    /// Its unit.
    pub unit: String,
}

impl Parameter {
    /// The value of the bytes that follow the identifier in an answer.
    pub fn value(&self, data: &[u8]) -> Option<f64> {
        let b = data.get(..self.bytes)?;
        let raw = b.iter().fold(0u64, |v, &x| v << 8 | x as u64);
        Some(raw as f64 * self.scale + self.offset)
    }
}

/// A named selection of parameters for one module.
#[derive(Clone, Debug, PartialEq)]
pub struct Profile {
    /// Its name.
    pub name: String,
    /// The module it is for.
    pub module: String,
    /// Its parameters.
    pub parameters: Vec<Parameter>,
}

/// `(X(0)*a)+b` or `(X(0)*a)-b`, as the catalogues write a formula.
fn formula(f: &str) -> Option<(f64, f64)> {
    let f = f.trim().strip_prefix("(X(0)*")?;
    let (a, rest) = f.split_once(')')?;
    let b: f64 = match rest.trim() {
        "" => 0.0,
        r if r.starts_with('+') => r[1..].trim().parse().ok()?,
        r if r.starts_with('-') => -r[1..].trim().parse::<f64>().ok()?,
        _ => return None,
    };
    Some((a.trim().parse().ok()?, b))
}

/// Reads the profiles in an opendash profile file: blocks that start with
/// `opendash-preset 1`, then `module`, `name`, and one `cat:name|pid|bytes|formula|unit`
/// row per parameter. Rows it cannot read are skipped.
pub fn read_profiles(text: &str) -> Vec<Profile> {
    let mut out: Vec<Profile> = Vec::new();
    for line in text.lines() {
        let line = line.trim_end();
        if line.trim() == "opendash-preset 1" {
            out.push(Profile {
                name: String::new(),
                module: String::new(),
                parameters: Vec::new(),
            });
            continue;
        }
        let Some(p) = out.last_mut() else { continue };
        if let Some(v) = line.strip_prefix("module\t") {
            p.module = v.trim().into();
        } else if let Some(v) = line.strip_prefix("name\t") {
            p.name = v.trim().into();
        } else if let Some(row) = line.strip_prefix("cat:") {
            let c: Vec<&str> = row.split('|').collect();
            if let [name, pid, bytes, f, unit] = c[..]
                && let (Ok(pid), Ok(bytes), Some((scale, offset))) =
                    (pid.trim().parse(), bytes.trim().parse(), formula(f))
            {
                p.parameters.push(Parameter {
                    name: name.trim().into(),
                    pid,
                    bytes,
                    scale,
                    offset,
                    unit: unit.trim().into(),
                });
            }
        }
    }
    out.retain(|p| !p.parameters.is_empty());
    out
}

/// Reads map labels in the transferred-tags format (`Denso_SH705x` 17): one map per line,
/// `ref dims | struct data axX axY corrZ | tag; tag; ...  (candidates ...)`, keyed by the
/// struct (the descriptor). The first label of a descriptor wins.
pub fn read_labels(text: &str) -> BTreeMap<u32, Name> {
    let mut out = BTreeMap::new();
    for line in text.lines() {
        let mut parts = line.splitn(3, '|');
        let (Some(_), Some(mid), Some(tags)) = (parts.next(), parts.next(), parts.next()) else {
            continue;
        };
        let Some(desc) = mid
            .split_whitespace()
            .next()
            .and_then(|h| u32::from_str_radix(h, 16).ok())
        else {
            continue;
        };
        let name = tags.split("(candidates").next().unwrap_or(tags).trim();
        if !name.is_empty() {
            out.entry(desc)
                .or_insert_with(|| Name::new(name, "", How::Label));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_profile_reads_as_opendash_writes_it() {
        let text = "# comment\nopendash-preset 1\nmodule\tEngine Control Module\nname\tAstra J A17DTJ\n\
                    cat:Desired Fuel Rail Pressure|12426|2|(X(0)*0.01)+0|MPa\n\
                    cat:ECT Sensor|5|1|(X(0)*1)-40|°C\ncat:broken|x|1|?|\n";
        let p = read_profiles(text);
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].name, "Astra J A17DTJ");
        assert_eq!(p[0].parameters.len(), 2);
        let rail = &p[0].parameters[0];
        assert_eq!(
            (rail.pid, rail.bytes, rail.unit.as_str()),
            (12426, 2, "MPa")
        );
        assert_eq!(rail.value(&[0x09, 0x60]), Some(24.0));
        assert_eq!(p[0].parameters[1].value(&[117]), Some(77.0));
    }

    #[test]
    fn labels_read_by_descriptor() {
        let l = read_labels(
            "ref dims | struct data | tags\n000449C4   16x11 | 0009DE14   0011B660    0.000 | Turbo; VNT Duty Cycle  (candidates 4, margin 0.48)\n",
        );
        assert_eq!(l[&0x9DE14].text, "Turbo; VNT Duty Cycle");
    }

    #[test]
    fn builtin_names_are_only_for_their_calibration() {
        assert!(Names::builtin(Some(ASTRA)).vars.contains_key(&0xFFFE_B7E4));
        assert!(Names::builtin(None).vars.is_empty());
        let mut flash = vec![0u8; 64];
        flash[10..22].copy_from_slice(ASTRA.as_bytes());
        assert_eq!(calibration(&flash), Some(ASTRA));
        assert_eq!(calibration(&[0u8; 64]), None);
    }
}
