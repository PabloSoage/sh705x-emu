//! The ELM327 over TCP: one app at a time, each with a fresh adapter, all on the same
//! running ECU.

use crate::elm::Elm327;
use crate::link::RealTimeEcu;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};

/// Serves apps on `listener` until it fails. Each connection gets a fresh adapter;
/// `log` is told about connections and every command.
pub fn serve(
    listener: TcpListener,
    ecu: &mut RealTimeEcu,
    log: &mut dyn FnMut(&str),
) -> std::io::Result<()> {
    for stream in listener.incoming() {
        let stream = stream?;
        let peer = stream
            .peer_addr()
            .map(|a| a.to_string())
            .unwrap_or_default();
        log(&format!("connected: {peer}"));
        if let Err(e) = session(stream, ecu, log) {
            log(&format!("{peer}: {e}"));
        }
        log(&format!("disconnected: {peer}"));
    }
    Ok(())
}

/// One app's session: commands end with CR (LF is ignored); answers end with the prompt.
pub fn session(
    stream: TcpStream,
    ecu: &mut RealTimeEcu,
    log: &mut dyn FnMut(&str),
) -> std::io::Result<()> {
    stream.set_nodelay(true)?;
    let mut out = stream.try_clone()?;
    let mut input = BufReader::new(stream);
    let mut elm = Elm327::new();
    out.write_all(b"\r>")?;
    let mut buf = Vec::new();
    loop {
        buf.clear();
        if input.read_until(b'\r', &mut buf)? == 0 {
            return Ok(());
        }
        let line: String = String::from_utf8_lossy(&buf)
            .chars()
            .filter(|&c| c != '\r' && c != '\n')
            .collect();
        ecu.flush();
        let answer = elm.line(&line, ecu);
        let shown = answer
            .trim_end_matches('>')
            .trim()
            .replace(['\r', '\n'], " | ");
        log(&format!("> {line}  < {shown}"));
        out.write_all(answer.as_bytes())?;
    }
}
