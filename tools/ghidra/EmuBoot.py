# Phase 0 of the Denso emulator: run a raw SH705x flash from its reset vector in
# Ghidra's p-code emulator, with no peripherals at all, and log every access
# outside the ROM. Where it ends up polling is the list of peripherals to model.
#
# A loop 'mov.x @rA,rN / tst / bt|bf' on an I/O address that repeats for two
# windows is resolved automatically: the polled bits are set (bt) or cleared (bf)
# in every later read of that address, and the report lists each one.
#
# Args: <report.txt> [max instructions] [ADDR:MASK ...] [trace:<file>] [noauto]
#   ADDR:MASK sets bits MASK (hex) in every byte read at ADDR (hex), by hand.
#   trace:<file> writes every I/O access, untrimmed: "pc R|W addr size value" in hex,
#     with the pc of the instruction (of the branch, for a delay slot).
#   noauto turns off the automatic resolution of polls.
# Run it with emu_boot.ps1.
# @runtime Jython
from ghidra.app.emulator import EmulatorHelper, MemoryAccessFilter
from ghidra.pcode.memstate import MemoryFaultHandler
import re
import time

args = getScriptArgs()
OUT = args[0]
MAXN = int(args[1]) if len(args) > 1 else 3000000
FORCE = {}
TRACE = None
AUTO = True
for a in args[2:]:
    if a.startswith("trace:"):
        TRACE = open(a[len("trace:"):], "w")
    elif a == "noauto":
        AUTO = False
    else:
        k, v = a.split(":")
        FORCE[int(k, 16)] = int(v, 16)

mem = currentProgram.getMemory()
ROM_END = mem.getMaxAddress().getOffset() + 1
IO_START = 0xFFFFC000      # above the initial SP; refined from the log, not assumed


def rom32(a):
    return mem.getInt(toAddr(a)) & 0xFFFFFFFF


PC0, SP0 = rom32(0), rom32(4)
emu = EmulatorHelper(currentProgram)


class Faults(MemoryFaultHandler):
    def uninitializedRead(self, address, size, buf, bufOffset):
        return True          # memory nobody wrote reads as zero

    def unknownAddress(self, address, write):
        return True


emu.setMemoryFaultHandler(Faults())

state = {"pc": PC0, "n": 0}
io = {}            # addr -> [first n, first pc, reads, writes, set of values read, last written]
seq = []           # I/O events in order, consecutive repeats collapsed
ram = {"r": 0, "w": 0, "lo": None, "hi": None}
other = {}         # accesses outside ROM, RAM guess and I/O guess


def val(values, size):
    v = 0
    for i in range(size):
        v = (v << 8) | (values[i] & 0xFF)
    return v


def note(kind, off, size, v):
    pc = state["pc"]
    if TRACE is not None and off >= IO_START:
        TRACE.write("%X %s %X %d %X\n" % (pc, kind, off, size, v))
    if off >= IO_START:
        e = io.get(off)
        if e is None:
            e = io[off] = [state["n"], pc, 0, 0, set(), None]
        if kind == "R":
            e[2] += 1
            if len(e[4]) < 8:
                e[4].add(v)
        else:
            e[3] += 1
            e[5] = v
        ev = (pc, kind, off, size, v)
        if seq and seq[-1][0] == ev:
            seq[-1][1] += 1
        else:
            seq.append([ev, 1, state["n"]])
            if len(seq) > 20000:     # keep the first 4000 and the last 16000
                del seq[4000:8000]
                seq.insert(4000, [None, 0, state["n"]])
    elif 0xFFFE0000 <= off < IO_START:
        ram[kind.lower()] += 1
        ram["lo"] = off if ram["lo"] is None else min(ram["lo"], off)
        ram["hi"] = off if ram["hi"] is None else max(ram["hi"], off)
    else:
        e = other.setdefault(off, [state["n"], pc, kind, 0])
        e[3] += 1


class Spy(MemoryAccessFilter):
    def processRead(self, spc, off, size, values):
        if spc.getName() != "ram" or off < ROM_END:
            return
        for i in range(size):
            if (off + i) in FORCE or (off + i) in CLEAR:
                f = ((values[i] & 0xFF) | FORCE.get(off + i, 0)) & ~CLEAR.get(off + i, 0) & 0xFF
                values[i] = f - 256 if f > 127 else f
        note("R", off, size, val(values, size))

    def processWrite(self, spc, off, size, values):
        if spc.getName() != "ram" or off < ROM_END:
            return
        note("W", off, size, val(values, size))


emu.getEmulator().addMemoryAccessFilter(Spy())
emu.writeRegister(emu.getPCRegister(), PC0)
emu.writeRegister("r15", SP0)
emu.writeRegister("sr", 0xF0)
emu.writeRegister("vbr", 0)

CLEAR = {}          # addr -> bits cleared in every byte read (for "wait until 0" polls)
auto = []           # polls resolved automatically: (n, loop pc, addr, size, mask, set/clear)
lst = currentProgram.getListing()


def insn(p):
    if lst.getInstructionAt(toAddr(p)) is None:
        disassemble(toAddr(p))
    return lst.getInstructionAt(toAddr(p))


def reg(name):
    return int(emu.readRegister(name)) & 0xFFFFFFFF


def resolve_poll(pcs):
    """A tight loop 'mov.x @rA,rN ... tst rM,rN|#imm,r0 ... bt/bf' on an I/O address:
    give the polled bits the value that ends the loop. Returns a description or None."""
    load = test = br = None
    for p in pcs:
        s = str(insn(p)).lower()
        if re.match(r"mov\.[bwl] @r\d+\+?,r\d+$", s):
            load = (p, s)
        elif s.startswith("tst "):
            test = s
        elif s.startswith("bt ") or s.startswith("bf "):
            br = s[:2]
    if not (load and test and br):
        return None
    m = load[1]
    size = {"b": 1, "w": 2, "l": 4}[m[4]]
    src, dst = m.split()[1].split(",")
    addr = reg(src.strip("@").rstrip("+"))
    if addr < IO_START:
        return None
    a, b = test.split()[1].split(",")
    isreg = lambda t: re.match(r"r\d+$", t) is not None
    if not isreg(a):                       # tst #imm,r0 (Ghidra prints it without '#')
        mask = int(a.lstrip("#"), 16)
    else:
        mask = reg(b if a == dst else a)
    mask &= (1 << (8 * size)) - 1
    if not mask:
        return None
    target = FORCE if br == "bt" else CLEAR
    for i in range(size):
        byte = (mask >> (8 * (size - 1 - i))) & 0xFF
        if byte:
            target[addr + i] = target.get(addr + i, 0) | byte
    auto.append((state["n"], load[0], addr, size, mask, "set" if br == "bt" else "clear"))
    return True


WINDOW = 20000
window, loops, stop = {}, [], "instruction cap"
t0 = time.time()
while state["n"] < MAXN:
    pc = emu.getExecutionAddress().getOffset()
    state["pc"] = pc
    if pc >= ROM_END:
        stop = "PC left the ROM: 0x%08X" % pc
        break
    window[pc] = window.get(pc, 0) + 1
    if not emu.step(monitor):
        stop = "emulator error at 0x%08X: %s" % (pc, emu.getLastError())
        break
    state["n"] += 1
    if state["n"] % WINDOW == 0:
        if len(window) <= 48:
            pcs = tuple(sorted(window))
            if loops and loops[-1][1] == pcs:
                loops[-1][2] += 1
            else:
                loops.append([state["n"], pcs, 1])
            if AUTO and loops[-1][2] >= 2 and len(auto) < 200 and resolve_poll(pcs):
                loops[-1][2] = 0
            elif loops[-1][2] >= 5:
                stop = "stuck: same %d PCs for %d instructions" % (len(pcs), loops[-1][2] * WINDOW)
                break
        window = {}
dt = time.time() - t0
if TRACE is not None:
    TRACE.close()

f = open(OUT, "w")
w = lambda s="": f.write(s + "\n")
w("reset PC 0x%08X  SP 0x%08X  ROM end 0x%X" % (PC0, SP0, ROM_END))
w("executed %d instructions in %.1f s (%.0f/s); stop: %s" % (state["n"], dt, state["n"] / max(dt, 1e-9), stop))
w("last PC 0x%08X  forced: %s" % (state["pc"], ", ".join("%08X=%02X" % kv for kv in sorted(FORCE.items())) or "none"))
regs = ["r%d" % i for i in range(16)] + ["sr", "gbr", "vbr", "pr", "macl", "mach"]
w("registers: " + " ".join("%s=%08X" % (r, emu.readRegister(r)) for r in regs))
w()
w("RAM guess [0xFFFE0000, 0x%08X): %d reads, %d writes, touched 0x%s..0x%s" % (
    IO_START, ram["r"], ram["w"], "%08X" % ram["lo"] if ram["lo"] else "-", "%08X" % ram["hi"] if ram["hi"] else "-"))
w()
w("== I/O registers (>= 0x%08X), by first access ==" % IO_START)
w("addr      first_n    first_pc  reads  writes  read values        last written")
for a, e in sorted(io.items(), key=lambda kv: kv[1][0]):
    w("%08X  %9d  %08X  %5d  %6d  %-18s %s" % (a, e[0], e[1], e[2], e[3],
      ",".join("%X" % x for x in sorted(e[4])), "-" if e[5] is None else "%X" % e[5]))
w()
w("== other accesses (neither ROM nor RAM/I-O guess) ==")
for a, e in sorted(other.items()):
    w("%08X  first_n %d  pc %08X  %s x%d" % (a, e[0], e[1], e[2], e[3]))
w()
w("== tight loops (<= 48 distinct PCs in a %d-instruction window) ==" % WINDOW)
for n, pcs, k in loops:
    w("from n=%d, %d windows: %s" % (n - WINDOW, k, " ".join("%X" % p for p in pcs)))
w()
w("== polls resolved automatically (n, loop load pc, addr, size, mask, bits) ==")
for n, p, a, size, mask, how in auto:
    w("%9d  %08X  %08X  %d  %0*X  %s" % (n, p, a, size, size * 2, mask, how))
w()
if loops:
    w("== disassembly of the last tight loop ==")
    for p in loops[-1][1]:
        w("%08X  %s" % (p, insn(p)))
    w()
w("== I/O sequence (n, pc, R/W, addr, size, value, repeats) ==")
for ev, k, n in seq:
    if ev is None:
        w("   ... events dropped before n=%d ..." % n)
        continue
    pc, kind, a, size, v = ev
    w("%9d  %08X  %s  %08X  %d  %0*X%s" % (n, pc, kind, a, size, size * 2, v, "  x%d" % k if k > 1 else ""))
f.close()
print("EmuBoot: %d instructions, %s" % (state["n"], stop))
