//! Standard names of the OBD-II / SAE J1979 service 01 PIDs (`0x00`-`0xFF`).
//!
//! A parameter profile's own catalogue can hang another module's label on one of these
//! low identifiers (it is a generic GM list), so for a PID in this range the emulator
//! prefers the SAE name, which is fixed and public, over the profile's. Only the engine
//! PIDs this ECU can answer are listed; an unlisted one keeps the profile's name.

/// The SAE J1979 name of service-01 PID `pid`, or `None` if it is not a standard PID this
/// table carries.
pub fn pid_name(pid: u32) -> Option<&'static str> {
    Some(match pid {
        0x00 => "PIDs supported [01-20]",
        0x01 => "Monitor status since DTCs cleared",
        0x02 => "Freeze DTC",
        0x03 => "Fuel system status",
        0x04 => "Calculated engine load",
        0x05 => "Engine coolant temperature",
        0x06 => "Short term fuel trim, bank 1",
        0x07 => "Long term fuel trim, bank 1",
        0x08 => "Short term fuel trim, bank 2",
        0x09 => "Long term fuel trim, bank 2",
        0x0A => "Fuel pressure",
        0x0B => "Intake manifold absolute pressure",
        0x0C => "Engine speed",
        0x0D => "Vehicle speed",
        0x0E => "Timing advance",
        0x0F => "Intake air temperature",
        0x10 => "Mass air flow rate",
        0x11 => "Throttle position",
        0x12 => "Commanded secondary air status",
        0x13 => "Oxygen sensors present (2 banks)",
        0x1C => "OBD standards this vehicle conforms to",
        0x1D => "Oxygen sensors present (4 banks)",
        0x1E => "Auxiliary input status",
        0x1F => "Run time since engine start",
        0x20 => "PIDs supported [21-40]",
        0x21 => "Distance traveled with MIL on",
        0x22 => "Fuel rail pressure (relative to manifold vacuum)",
        0x23 => "Fuel rail gauge pressure",
        0x2C => "Commanded EGR",
        0x2D => "EGR error",
        0x2E => "Commanded evaporative purge",
        0x2F => "Fuel tank level input",
        0x30 => "Warm-ups since codes cleared",
        0x31 => "Distance traveled since codes cleared",
        0x32 => "Evap system vapor pressure",
        0x33 => "Absolute barometric pressure",
        0x3C => "Catalyst temperature, bank 1 sensor 1",
        0x3D => "Catalyst temperature, bank 2 sensor 1",
        0x3E => "Catalyst temperature, bank 1 sensor 2",
        0x3F => "Catalyst temperature, bank 2 sensor 2",
        0x40 => "PIDs supported [41-60]",
        0x41 => "Monitor status this drive cycle",
        0x42 => "Control module voltage",
        0x43 => "Absolute load value",
        0x44 => "Commanded air-fuel equivalence ratio",
        0x45 => "Relative throttle position",
        0x46 => "Ambient air temperature",
        0x47 => "Absolute throttle position B",
        0x48 => "Absolute throttle position C",
        0x49 => "Accelerator pedal position D",
        0x4A => "Accelerator pedal position E",
        0x4B => "Accelerator pedal position F",
        0x4C => "Commanded throttle actuator",
        0x4D => "Time run with MIL on",
        0x4E => "Time since trouble codes cleared",
        0x51 => "Fuel type",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::pid_name;

    #[test]
    fn standard_pids_get_their_sae_name() {
        assert_eq!(pid_name(0x0C), Some("Engine speed"));
        assert_eq!(pid_name(0x05), Some("Engine coolant temperature"));
        assert_eq!(pid_name(0x33), Some("Absolute barometric pressure"));
        // Not a standard PID this table carries, and a $22 DID is far out of range.
        assert_eq!(pid_name(0x55), None);
        assert_eq!(pid_name(0x308A), None);
    }
}
