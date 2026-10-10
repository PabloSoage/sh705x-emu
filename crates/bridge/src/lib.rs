//! Connections from the emulated ECU to the outside world: the CAN bus as a diagnostic
//! tool sees it ([`link`]), ISO-TP ([`isotp`]), an ELM327 on top ([`elm`]) and a TCP
//! server for apps that speak to one ([`server`]), and a Scanmatik SM3 for opendash
//! ([`sm3`]).

pub mod elm;
pub mod isotp;
pub mod link;
pub mod server;
pub mod sm3;
