//! Prints the decoder table: one line per 16-bit code, "CODE<TAB>Op".
//! Used by tools/compare_decode.py against Ghidra's disassembler.

fn main() {
    for code in 0..=0xFFFFu16 {
        println!("{code:04X}\t{:?}", sh2e::decode::DECODE[code as usize]);
    }
}
