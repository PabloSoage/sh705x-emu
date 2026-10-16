//! A J2534 (04.04) pass-thru DLL whose adapter is the emulator's SM3 (`sh705x sm3`).
//!
//! Register it as a pass-thru device (`register.ps1`) and J2534 applications on this
//! machine, or in a virtual machine that reaches it over the network, talk to the
//! emulated ECU as if through a real adapter on a car. The vendor's DLL cannot do this:
//! it pairs with the physical adapter over Wi-Fi Direct, Bluetooth or USB before it
//! opens its socket, and takes no address.
//!
//! Where the adapter is, in order: the `SH705X_SM3` environment variable, the line
//! `server=host:port` in `%ProgramData%\sh705x\j2534.ini`, or `127.0.0.1:777`. The
//! line `log=<file>` there writes every call and frame to that file.
//!
//! Protocols: CAN and ISO 15765 (both on the high-speed bus), with pass, block and
//! flow-control filters, extended addressing, frame padding, loopback and periodic
//! messages. This file is the C ABI and nothing else; [`device`] does the work.

#![allow(unsafe_code)]

pub mod device;

use device::{Device, Error, Filter, Msg};
use std::ffi::{c_char, c_void};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// The size of `PASSTHRU_MSG.Data`.
const DATA_MAX: usize = 4128;
/// The one device this DLL opens.
const DEVICE_ID: u32 = 1;

/// `PASSTHRU_MSG`.
#[repr(C)]
pub struct PassThruMsg {
    /// Protocol.
    pub protocol_id: u32,
    /// Receive status.
    pub rx_status: u32,
    /// Transmit flags.
    pub tx_flags: u32,
    /// Microseconds.
    pub timestamp: u32,
    /// Bytes used in `data`.
    pub data_size: u32,
    /// Where extra data starts.
    pub extra_data_index: u32,
    /// Identifier and payload.
    pub data: [u8; DATA_MAX],
}

/// `SCONFIG`.
#[repr(C)]
pub struct SConfig {
    /// Parameter.
    pub parameter: u32,
    /// Value.
    pub value: u32,
}

/// `SCONFIG_LIST`.
#[repr(C)]
pub struct SConfigList {
    /// Entries.
    pub num_of_params: u32,
    /// The first entry.
    pub config_ptr: *mut SConfig,
}

static DEVICE: Mutex<Option<Arc<Device>>> = Mutex::new(None);
static LAST_ERROR: Mutex<String> = Mutex::new(String::new());

fn status(r: Result<(), Error>, what: &str) -> i32 {
    match r {
        Ok(()) => 0,
        Err(e) => {
            if let Ok(mut s) = LAST_ERROR.lock() {
                *s = format!("{what}: {e:?}");
            }
            e as i32
        }
    }
}

fn with_device<T>(f: impl FnOnce(&Device) -> Result<T, Error>) -> Result<T, Error> {
    // Cloned out so a blocking call does not hold the lock against the others.
    let d = DEVICE.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let d = d.ok_or(Error::InvalidDeviceId)?;
    f(&d)
}

/// Where the adapter is, and the log file if any.
fn settings() -> (String, Option<String>) {
    let mut server = "127.0.0.1:777".to_string();
    let mut log = None;
    if let Ok(dir) = std::env::var("ProgramData") {
        let path = std::path::Path::new(&dir).join("sh705x").join("j2534.ini");
        if let Ok(text) = std::fs::read_to_string(path) {
            for line in text.lines() {
                match line.trim().split_once('=') {
                    Some(("server", v)) => server = v.trim().to_string(),
                    Some(("log", v)) => log = Some(v.trim().to_string()),
                    _ => {}
                }
            }
        }
    }
    if let Ok(v) = std::env::var("SH705X_SM3") {
        server = v;
    }
    (server, log)
}

fn to_msg(m: &PassThruMsg) -> Result<Msg, Error> {
    let n = m.data_size as usize;
    if n > DATA_MAX {
        return Err(Error::InvalidMsg);
    }
    Ok(Msg {
        protocol: m.protocol_id,
        rx_status: m.rx_status,
        tx_flags: m.tx_flags,
        timestamp: m.timestamp,
        data: m.data[..n].to_vec(),
    })
}

fn from_msg(m: &Msg, out: &mut PassThruMsg) {
    let n = m.data.len().min(DATA_MAX);
    out.protocol_id = m.protocol;
    out.rx_status = m.rx_status;
    out.tx_flags = m.tx_flags;
    out.timestamp = m.timestamp;
    out.data_size = n as u32;
    out.extra_data_index = n as u32;
    out.data[..n].copy_from_slice(&m.data[..n]);
}

/// # Safety
/// `device_id` must be valid for a write.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn PassThruOpen(_name: *const c_void, device_id: *mut u32) -> i32 {
    if device_id.is_null() {
        return status(Err(Error::NullParameter), "PassThruOpen");
    }
    let mut d = DEVICE.lock().unwrap_or_else(|e| e.into_inner());
    if d.is_some() {
        return status(Err(Error::DeviceInUse), "PassThruOpen");
    }
    let (server, log) = settings();
    let log = log.and_then(|p| {
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(p)
            .ok()
    });
    match Device::open(&server, log) {
        Ok(dev) => {
            *d = Some(Arc::new(dev));
            // SAFETY: checked non-null; the caller owns the pointee.
            unsafe { *device_id = DEVICE_ID };
            0
        }
        Err(e) => status(Err(e), &format!("PassThruOpen {server}")),
    }
}

/// Closes the device.
#[unsafe(no_mangle)]
pub extern "system" fn PassThruClose(device_id: u32) -> i32 {
    if device_id != DEVICE_ID {
        return status(Err(Error::InvalidDeviceId), "PassThruClose");
    }
    let dev = DEVICE.lock().unwrap_or_else(|e| e.into_inner()).take();
    status(dev.map(drop).ok_or(Error::InvalidDeviceId), "PassThruClose")
}

/// # Safety
/// `channel_id` must be valid for a write.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn PassThruConnect(
    device_id: u32,
    protocol_id: u32,
    _flags: u32,
    baud: u32,
    channel_id: *mut u32,
) -> i32 {
    if channel_id.is_null() {
        return status(Err(Error::NullParameter), "PassThruConnect");
    }
    if device_id != DEVICE_ID {
        return status(Err(Error::InvalidDeviceId), "PassThruConnect");
    }
    match with_device(|d| d.connect(protocol_id, baud)) {
        Ok(ch) => {
            // SAFETY: checked non-null.
            unsafe { *channel_id = ch };
            0
        }
        Err(e) => status(Err(e), "PassThruConnect"),
    }
}

/// Closes a channel.
#[unsafe(no_mangle)]
pub extern "system" fn PassThruDisconnect(channel_id: u32) -> i32 {
    status(
        with_device(|d| d.disconnect(channel_id)),
        "PassThruDisconnect",
    )
}

/// # Safety
/// `msgs` must point to `*num_msgs` messages and `num_msgs` be valid for read and write.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn PassThruReadMsgs(
    channel_id: u32,
    msgs: *mut PassThruMsg,
    num_msgs: *mut u32,
    timeout: u32,
) -> i32 {
    if msgs.is_null() || num_msgs.is_null() {
        return status(Err(Error::NullParameter), "PassThruReadMsgs");
    }
    // SAFETY: checked non-null.
    let want = unsafe { *num_msgs } as usize;
    let (got, r) = match with_device(|d| {
        Ok(d.read(channel_id, want, Duration::from_millis(u64::from(timeout))))
    }) {
        Ok(x) => x,
        Err(e) => (vec![], Err(e)),
    };
    for (k, m) in got.iter().enumerate() {
        // SAFETY: k < want, the caller's array length.
        from_msg(m, unsafe { &mut *msgs.add(k) });
    }
    // SAFETY: checked non-null.
    unsafe { *num_msgs = got.len() as u32 };
    status(r, "PassThruReadMsgs")
}

/// # Safety
/// `msgs` must point to `*num_msgs` messages and `num_msgs` be valid for read and write.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn PassThruWriteMsgs(
    channel_id: u32,
    msgs: *const PassThruMsg,
    num_msgs: *mut u32,
    _timeout: u32,
) -> i32 {
    if msgs.is_null() || num_msgs.is_null() {
        return status(Err(Error::NullParameter), "PassThruWriteMsgs");
    }
    // SAFETY: checked non-null.
    let n = unsafe { *num_msgs } as usize;
    let mut done = 0u32;
    let mut r = Ok(());
    for k in 0..n {
        // SAFETY: k < n, the caller's array length.
        let m = match to_msg(unsafe { &*msgs.add(k) }) {
            Ok(m) => m,
            Err(e) => {
                r = Err(e);
                break;
            }
        };
        if let Err(e) = with_device(|d| d.write(channel_id, &m)) {
            r = Err(e);
            break;
        }
        done += 1;
    }
    // SAFETY: checked non-null.
    unsafe { *num_msgs = done };
    status(r, "PassThruWriteMsgs")
}

/// # Safety
/// `msg` must point to a message and `msg_id` be valid for a write.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn PassThruStartPeriodicMsg(
    channel_id: u32,
    msg: *const PassThruMsg,
    msg_id: *mut u32,
    interval: u32,
) -> i32 {
    if msg.is_null() || msg_id.is_null() {
        return status(Err(Error::NullParameter), "PassThruStartPeriodicMsg");
    }
    // SAFETY: checked non-null.
    let r = to_msg(unsafe { &*msg }).and_then(|m| {
        with_device(|d| d.start_periodic(channel_id, m, Duration::from_millis(u64::from(interval))))
    });
    match r {
        Ok(id) => {
            // SAFETY: checked non-null.
            unsafe { *msg_id = id };
            0
        }
        Err(e) => status(Err(e), "PassThruStartPeriodicMsg"),
    }
}

/// Stops a periodic message.
#[unsafe(no_mangle)]
pub extern "system" fn PassThruStopPeriodicMsg(channel_id: u32, msg_id: u32) -> i32 {
    status(
        with_device(|d| d.stop_periodic(channel_id, msg_id)),
        "PassThruStopPeriodicMsg",
    )
}

fn bytes_of(m: *const PassThruMsg) -> Option<(Vec<u8>, u32)> {
    if m.is_null() {
        return None;
    }
    // SAFETY: checked non-null; the caller passes a message.
    let m = unsafe { &*m };
    let n = (m.data_size as usize).min(DATA_MAX);
    Some((m.data[..n].to_vec(), m.tx_flags))
}

/// # Safety
/// The message pointers must be null or point to messages; `filter_id` valid for a write.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn PassThruStartMsgFilter(
    channel_id: u32,
    filter_type: u32,
    mask: *const PassThruMsg,
    pattern: *const PassThruMsg,
    flow_control: *const PassThruMsg,
    filter_id: *mut u32,
) -> i32 {
    let (Some((mask, flags)), Some((pattern, _))) = (bytes_of(mask), bytes_of(pattern)) else {
        return status(Err(Error::NullParameter), "PassThruStartMsgFilter");
    };
    if filter_id.is_null() {
        return status(Err(Error::NullParameter), "PassThruStartMsgFilter");
    }
    let flow = bytes_of(flow_control).map(|x| x.0).unwrap_or_default();
    let f = Filter {
        kind: filter_type,
        mask,
        pattern,
        flow,
        tx_flags: flags,
    };
    match with_device(|d| d.start_filter(channel_id, f)) {
        Ok(id) => {
            // SAFETY: checked non-null.
            unsafe { *filter_id = id };
            0
        }
        Err(e) => status(Err(e), "PassThruStartMsgFilter"),
    }
}

/// Removes a filter.
#[unsafe(no_mangle)]
pub extern "system" fn PassThruStopMsgFilter(channel_id: u32, filter_id: u32) -> i32 {
    status(
        with_device(|d| d.stop_filter(channel_id, filter_id)),
        "PassThruStopMsgFilter",
    )
}

/// Programming voltage: there is no pin to drive.
#[unsafe(no_mangle)]
pub extern "system" fn PassThruSetProgrammingVoltage(
    _device_id: u32,
    _pin: u32,
    _voltage: u32,
) -> i32 {
    status(Err(Error::NotSupported), "PassThruSetProgrammingVoltage")
}

fn put(dst: *mut c_char, s: &str) {
    if dst.is_null() {
        return;
    }
    let b = s.as_bytes();
    let n = b.len().min(79);
    // SAFETY: J2534 buffers are 80 bytes; n + 1 <= 80.
    unsafe {
        std::ptr::copy_nonoverlapping(b.as_ptr(), dst.cast::<u8>(), n);
        *dst.add(n) = 0;
    }
}

/// # Safety
/// Each pointer must be null or an 80-byte buffer.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn PassThruReadVersion(
    _device_id: u32,
    firmware: *mut c_char,
    dll: *mut c_char,
    api: *mut c_char,
) -> i32 {
    put(firmware, "sh705x emulated SM3");
    put(dll, env!("CARGO_PKG_VERSION"));
    put(api, "04.04");
    0
}

/// # Safety
/// `text` must be null or an 80-byte buffer.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn PassThruGetLastError(text: *mut c_char) -> i32 {
    if text.is_null() {
        return Error::NullParameter as i32;
    }
    put(
        text,
        &LAST_ERROR.lock().map(|s| s.clone()).unwrap_or_default(),
    );
    0
}

const GET_CONFIG: u32 = 0x01;
const SET_CONFIG: u32 = 0x02;
const READ_VBATT: u32 = 0x03;
const CLEAR_TX_BUFFER: u32 = 0x07;
const CLEAR_RX_BUFFER: u32 = 0x08;
const CLEAR_PERIODIC_MSGS: u32 = 0x09;
const CLEAR_MSG_FILTERS: u32 = 0x0A;
const CLEAR_FUNCT_MSG_LOOKUP_TABLE: u32 = 0x0B;
const ADD_TO_FUNCT_MSG_LOOKUP_TABLE: u32 = 0x0C;
const DELETE_FROM_FUNCT_MSG_LOOKUP_TABLE: u32 = 0x0D;
const READ_PROG_VOLTAGE: u32 = 0x0E;

/// # Safety
/// `input` and `output` must be what J2534 specifies for `ioctl_id`.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn PassThruIoctl(
    channel_id: u32,
    ioctl_id: u32,
    input: *mut c_void,
    output: *mut c_void,
) -> i32 {
    let r = match ioctl_id {
        GET_CONFIG | SET_CONFIG => {
            if input.is_null() {
                Err(Error::NullParameter)
            } else {
                // SAFETY: J2534 passes an SCONFIG_LIST for these.
                let list = unsafe { &*(input as *const SConfigList) };
                let mut r = Ok(());
                for k in 0..list.num_of_params as usize {
                    if list.config_ptr.is_null() {
                        r = Err(Error::NullParameter);
                        break;
                    }
                    // SAFETY: k < the list's count.
                    let c = unsafe { &mut *list.config_ptr.add(k) };
                    let one = if ioctl_id == GET_CONFIG {
                        with_device(|d| d.get_config(channel_id, c.parameter)).map(|v| c.value = v)
                    } else {
                        with_device(|d| d.set_config(channel_id, c.parameter, c.value))
                    };
                    if let Err(e) = one {
                        r = Err(e);
                        break;
                    }
                }
                r
            }
        }
        READ_VBATT | READ_PROG_VOLTAGE => {
            if output.is_null() {
                Err(Error::NullParameter)
            } else {
                let v = if ioctl_id == READ_VBATT {
                    with_device(|d| d.battery_mv())
                } else {
                    Ok(0)
                };
                v.map(|mv| {
                    // SAFETY: J2534 passes a u32 for these.
                    unsafe { *(output as *mut u32) = mv }
                })
            }
        }
        CLEAR_TX_BUFFER => with_device(|_| Ok(())),
        CLEAR_RX_BUFFER => with_device(|d| d.clear_rx(channel_id)),
        CLEAR_PERIODIC_MSGS => with_device(|d| d.clear_periodic(channel_id)),
        CLEAR_MSG_FILTERS => with_device(|d| d.clear_filters(channel_id)),
        CLEAR_FUNCT_MSG_LOOKUP_TABLE
        | ADD_TO_FUNCT_MSG_LOOKUP_TABLE
        | DELETE_FROM_FUNCT_MSG_LOOKUP_TABLE => Err(Error::NotSupported),
        _ => Err(Error::InvalidIoctlId),
    };
    let _ = with_device(|d| {
        d.log(&format!("ioctl ch{channel_id} {ioctl_id:#x} -> {r:?}"));
        Ok(())
    });
    status(r, "PassThruIoctl")
}
