# Disassembles every 16-bit code with Ghidra's SuperH language, as an independent oracle
# for the emulator's decoder. The program to run it on is all_codes.bin: each code
# 0x0000..0xFFFF stored big-endian at address 2 * code (see compare_decode.py).
#
# Args: <out.txt>   One line per code: "CODE<TAB>mnemonic<TAB>operands", or "CODE<TAB>-"
# when Ghidra does not decode it.
# @runtime Jython
from ghidra.app.util import PseudoDisassembler

out = open(getScriptArgs()[0], "w")
pd = PseudoDisassembler(currentProgram)
for code in range(0x10000):
    addr = toAddr(code * 2)
    try:
        ins = pd.disassemble(addr)
    except Exception:
        ins = None
    if ins is None:
        out.write("%04X\t-\n" % code)
        continue
    ops = ",".join(ins.getDefaultOperandRepresentation(i) for i in range(ins.getNumOperands()))
    out.write("%04X\t%s\t%s\n" % (code, ins.getMnemonicString().lower(), ops.lower()))
out.close()
print("DecodeAll: done")
