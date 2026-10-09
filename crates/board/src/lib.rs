//! The ECU board around the microcontroller: the serial EEPROM, the other chips on the
//! synchronous serial ports, and which pin carries which sensor or actuator signal.
//!
//! For now this only holds the EEPROM image. The protocol it is read with is not known
//! yet: the board model serves it once that is worked out.

use std::fmt;

/// Largest EEPROM image accepted: 64 KB, well above the serial EEPROMs fitted to these ECUs.
pub const EEPROM_MAX: usize = 0x1_0000;

/// An EEPROM image that cannot be used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EepromError {
    /// The image is empty, not a power of two, or larger than [`EEPROM_MAX`].
    BadSize {
        /// Bytes in the image given.
        actual: usize,
    },
}

impl fmt::Display for EepromError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EepromError::BadSize { actual } => write!(
                f,
                "EEPROM image is {actual} bytes; expected a power of two up to {EEPROM_MAX}"
            ),
        }
    }
}

impl std::error::Error for EepromError {}

/// The contents of the board's serial EEPROM.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Eeprom {
    data: Vec<u8>,
}

impl Eeprom {
    /// An EEPROM holding `image`. Serial EEPROMs come in power-of-two sizes.
    pub fn new(image: &[u8]) -> Result<Self, EepromError> {
        let n = image.len();
        if n == 0 || !n.is_power_of_two() || n > EEPROM_MAX {
            return Err(EepromError::BadSize { actual: n });
        }
        Ok(Self {
            data: image.to_vec(),
        })
    }

    /// An EEPROM of `size` bytes in the erased state (all 0xFF), for running without a dump.
    pub fn erased(size: usize) -> Result<Self, EepromError> {
        Self::new(&vec![0xFF; size])
    }

    /// Size in bytes.
    pub fn len(&self) -> usize {
        self.data.len()
    }

    /// Always false: an EEPROM has at least one byte.
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    /// The contents.
    pub fn data(&self) -> &[u8] {
        &self.data
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_power_of_two_sizes() {
        assert_eq!(Eeprom::new(&[0; 8192]).unwrap().len(), 8192);
        assert_eq!(Eeprom::erased(512).unwrap().data()[0], 0xFF);
    }

    #[test]
    fn rejects_other_sizes() {
        for n in [0, 8191, 0x2_0000] {
            assert_eq!(
                Eeprom::new(&vec![0; n]),
                Err(EepromError::BadSize { actual: n })
            );
        }
    }
}
