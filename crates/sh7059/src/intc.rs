//! Interrupt controller (manual section 7): priorities from the IPR registers and the
//! highest pending source. Only the sources of the modelled peripherals are listed.
//!
//! IPRA..IPRL are at `FFFFED00..FFFFED17`, 16 bits each, four 4-bit fields; they live in
//! the chip's register storage and are read from there.

/// First IPR register (IPRA).
pub const IPR_BASE: u32 = 0xFFFF_ED00;
/// One past the last IPR register (IPRL).
pub const IPR_END: u32 = 0xFFFF_ED18;
/// ICR: bit 15 (NMIL) reads the NMI pin level.
pub const ICR: u32 = 0xFFFF_ED18;

/// An interrupt source: its vector and where its priority is (manual table 7.3).
#[derive(Clone, Copy, Debug)]
pub struct Source {
    /// Vector number.
    pub vector: u32,
    /// Index of its IPR register: 0 = IPRA ... 11 = IPRL.
    pub ipr: usize,
    /// Bit position of its 4-bit field in that register.
    pub shift: u32,
}

/// CMTI0: vector 188, IPRJ bits 11..8.
pub const CMTI0: Source = Source {
    vector: 188,
    ipr: 9,
    shift: 8,
};
/// CMTI1: vector 192, IPRJ bits 7..4.
pub const CMTI1: Source = Source {
    vector: 192,
    ipr: 9,
    shift: 4,
};

/// ADI0: vector 190, IPRJ bits 11..8 (after CMTI0 and ADT0 within the field).
pub const ADI0: Source = Source {
    vector: 190,
    ipr: 9,
    shift: 8,
};
/// ADI1: vector 194, IPRJ bits 7..4 (after CMTI1 and ADT1).
pub const ADI1: Source = Source {
    vector: 194,
    ipr: 9,
    shift: 4,
};
/// ADI2: vector 196, IPRJ bits 3..0.
pub const ADI2: Source = Source {
    vector: 196,
    ipr: 9,
    shift: 0,
};

/// DEI0-3: vectors 72, 74, 76, 78; DEI0 and DEI1 share IPRC bits 15..12, DEI2 and DEI3
/// bits 11..8.
pub const DEI: [Source; 4] = [
    Source {
        vector: 72,
        ipr: 2,
        shift: 12,
    },
    Source {
        vector: 74,
        ipr: 2,
        shift: 12,
    },
    Source {
        vector: 76,
        ipr: 2,
        shift: 8,
    },
    Source {
        vector: 78,
        ipr: 2,
        shift: 8,
    },
];

/// HCAN0 and HCAN1: ERS, OVR, RM and SLE, vectors 220-223 in IPRL bits 11..8 and
/// 228-231 in IPRL bits 3..0.
pub const HCAN: [[Source; 4]; 2] = {
    let mut t = [[Source {
        vector: 0,
        ipr: 11,
        shift: 0,
    }; 4]; 2];
    let mut k = 0;
    while k < 4 {
        t[0][k] = Source {
            vector: 220 + k as u32,
            ipr: 11,
            shift: 8,
        };
        t[1][k] = Source {
            vector: 228 + k as u32,
            ipr: 11,
            shift: 0,
        };
        k += 1;
    }
    t
};

/// The priority level of `src` given the 24 IPR bytes.
pub fn level(ipr: &[u8], src: Source) -> u32 {
    let r = u16::from_be_bytes([ipr[2 * src.ipr], ipr[2 * src.ipr + 1]]);
    ((r >> src.shift) & 0xF) as u32
}

/// The highest-priority request among `requests`: (level, vector), or (0, 0) for none.
/// Level 0 never interrupts. Ties go to the source listed first, which follows the
/// default order of table 7.3.
pub fn highest(ipr: &[u8], requests: &[(Source, bool)]) -> (u32, u32) {
    let mut best = (0, 0);
    for &(src, on) in requests {
        if on {
            offer(ipr, src, &mut best);
        }
    }
    best
}

/// Makes `src`, which is requesting, the pending one if its level beats `best`. Offered in
/// table 7.3 order, ties stay with the earlier source.
#[inline(always)]
pub fn offer(ipr: &[u8], src: Source, best: &mut (u32, u32)) {
    let l = level(ipr, src);
    if l > best.0 {
        *best = (l, src.vector);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iprj_as_the_firmware_leaves_it() {
        let mut ipr = [0u8; 24];
        ipr[18..20].copy_from_slice(&0x0999u16.to_be_bytes()); // IPRJ
        assert_eq!(level(&ipr, CMTI0), 9);
        assert_eq!(level(&ipr, CMTI1), 9);
        assert_eq!(highest(&ipr, &[(CMTI0, false), (CMTI1, true)]), (9, 192));
        assert_eq!(highest(&ipr, &[(CMTI0, true), (CMTI1, true)]), (9, 188));
        // Within one IPR field, table 7.3 order: CMTI0 before ADI0.
        assert_eq!(highest(&ipr, &[(CMTI0, true), (ADI0, true)]), (9, 188));
        assert_eq!(highest(&ipr, &[(ADI0, true), (ADI1, true)]), (9, 190));
        assert_eq!(highest(&ipr, &[(ADI2, true)]), (9, 196));
        assert_eq!(highest(&ipr, &[(ADI1, true)]), (9, 194));
        ipr[18..20].copy_from_slice(&0x0000u16.to_be_bytes());
        assert_eq!(highest(&ipr, &[(CMTI1, true)]), (0, 0), "level 0 is masked");
    }
}
