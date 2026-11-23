//! The firmware releases this project knows, and where each keeps the variables the plants
//! and the front ends read.
//!
//! The RAM layout moves between releases: the Astra's addresses read on another one give
//! unrelated values, or zeros. So a release is recognised by the name its own code carries,
//! and one that is not known has no addresses at all.

/// One firmware release.
#[derive(Debug, PartialEq, Eq)]
pub struct Software {
    /// Its name, as the flash carries it in its code.
    pub id: &'static str,
    /// Where it keeps its variables.
    pub ram: Ram,
}

/// Where a release keeps the variables read here: floats in RAM.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ram {
    /// Engine speed, rpm.
    pub rpm: u32,
    /// Net torque, N·m (what 0x1A2D reports).
    pub torque: u32,
    /// The fuel per stroke it asks for, mm³.
    pub quantity: u32,
    /// The idle speed it aims at, rpm.
    pub idle_target: u32,
    /// The EGR position it asks for, % (0x002C).
    pub egr_asked: u32,
    /// The boost it asks for, kPa (0x328A).
    pub boost_asked: u32,
    /// The SCV current it asks for, mA.
    pub scv_asked: u32,
    /// The rail pressure it asks for, MPa (0x308A).
    pub rail_asked: u32,
    /// The SCV command, % (0x323D).
    pub scv_command: u32,
    /// The VGT command, % (0x2041).
    pub vgt_command: u32,
}

/// The releases known, the project's own first.
///
/// `NCLSW420AX36` is the Astra J's (A17DTJ), where every variable was found by its
/// behaviour [MED]. The others were derived from it [DER]: for every load of a variable's
/// address in its code, the place with the same instructions around it in the other image,
/// and the address loaded there. Every load that could be matched gave the same answer.
/// Torque, rail, boost and VGT were then checked against what the ECU itself reports on
/// diagnostics at seven operating points.
///
/// The exception is the torque, kept twice: where it is computed, and where 0x1A2D takes it
/// from. AZ03's second copy was found by the code that answers 0x1A2D; AZ07 has none, so
/// the first is used there.
pub const KNOWN: &[Software] = &[
    Software {
        id: "NCLSW420AX36",
        ram: AX,
    },
    Software {
        id: "NCLSW440AX39",
        ram: AX,
    },
    Software {
        id: "NCLNP000AZ03",
        ram: Ram {
            rpm: 0xFFFE_B944,
            torque: 0xFFFE_FDE0,
            quantity: 0xFFFE_FA88,
            idle_target: 0xFFFE_FCF4,
            egr_asked: 0xFFFF_1510,
            boost_asked: 0xFFFF_0024,
            scv_asked: 0xFFFE_FB70,
            rail_asked: 0xFFFE_FAB8,
            scv_command: 0xFFFE_FB54,
            vgt_command: 0xFFFE_FF7C,
        },
    },
    Software {
        id: "NCLNP030AZ07",
        ram: Ram {
            rpm: 0xFFFE_B944,
            torque: 0xFFFE_FDC8,
            quantity: 0xFFFE_FA88,
            idle_target: 0xFFFE_FCF4,
            egr_asked: 0xFFFF_1508,
            boost_asked: 0xFFFF_0020,
            scv_asked: 0xFFFE_FB70,
            rail_asked: 0xFFFE_FAB8,
            scv_command: 0xFFFE_FB54,
            vgt_command: 0xFFFE_FF78,
        },
    },
];

/// The layout AX36 and AX39 share.
const AX: Ram = Ram {
    rpm: 0xFFFE_B7E4,
    torque: 0xFFFE_FC4C,
    quantity: 0xFFFE_F8F4,
    idle_target: 0xFFFE_FB60,
    egr_asked: 0xFFFF_1370,
    boost_asked: 0xFFFE_FE84,
    scv_asked: 0xFFFE_F9DC,
    rail_asked: 0xFFFE_F924,
    scv_command: 0xFFFE_F9C0,
    vgt_command: 0xFFFE_FDDC,
};

/// The release of `flash`, if it is one of those known.
pub fn identify(flash: &[u8]) -> Option<&'static Software> {
    KNOWN
        .iter()
        .find(|s| flash.windows(s.id.len()).any(|w| w == s.id.as_bytes()))
}

/// [`identify`], for the flash of a machine.
pub fn of(m: &sh7059::Machine) -> Option<&'static Software> {
    let flash: Vec<u8> = (0..sh7059::ROM_SIZE as u32)
        .map(|a| m.chip.peek(a))
        .collect();
    identify(&flash)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_flash_is_known_by_the_name_it_carries() {
        for s in KNOWN {
            let mut flash = vec![0xFF; 64];
            flash[10..22].copy_from_slice(s.id.as_bytes());
            assert_eq!(identify(&flash), Some(s));
        }
        assert_eq!(identify(&[0xFF; 64]), None);
        assert_eq!(identify(&[]), None);
    }
}
