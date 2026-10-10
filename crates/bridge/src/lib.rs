//! Connections from the emulated ECU to the outside world: the CAN bus as a diagnostic
//! tool sees it ([`link`]), ISO-TP ([`isotp`]), an ELM327 on top ([`elm`]) and a TCP
//! server for apps that speak to one ([`server`]).

pub mod elm;
pub mod isotp;
pub mod link;
pub mod server;
