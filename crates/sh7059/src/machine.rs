//! The CPU and the chip together, on one clock: the CPU runs until the next timer event,
//! the chip takes the event, and pending interrupts are offered to the CPU between
//! instructions.

use crate::Sh7059;
use sh2e::Cpu;

/// Why [`Machine::run_until`] returned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stop {
    /// The instruction count was reached.
    Count,
    /// The PC reached a breakpoint (the instruction there has not run).
    Breakpoint,
    /// A watchpoint with `stop` set was hit (the instruction that hit it has run).
    Watch,
    /// The CPU is asleep with nothing left to wake it.
    Asleep,
}

/// A call to a probed routine: where from, with what, and what it gave back.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Call {
    /// The routine.
    pub routine: u32,
    /// The return address (PR on entry): the caller is just before it.
    pub ret: u32,
    /// R4 on entry (a Denso map routine's descriptor).
    pub r4: u32,
    /// FR4 and FR5 on entry (its inputs).
    pub fr4: f32,
    /// See [`Call::fr4`].
    pub fr5: f32,
    /// FR0 on return (its result).
    pub fr0: f32,
    /// Cycle of the entry.
    pub cycles: u64,
}

/// Routines watched without stopping: every call to one of them, with its registers on
/// entry and FR0 on return, is added to [`Probes::calls`] for the caller to take.
#[derive(Clone, Debug, Default)]
pub struct Probes {
    /// The routines' entry addresses.
    pub routines: Vec<u32>,
    /// Calls that have returned, oldest first; the caller drains them.
    pub calls: Vec<Call>,
    /// Calls not kept because [`Probes::calls`] was full.
    pub dropped: u64,
    /// Calls under way, innermost last.
    pending: Vec<Call>,
}

impl Probes {
    /// Calls kept before new ones are dropped, if nobody drains them.
    pub const KEPT: usize = 1 << 20;
    /// Calls under way kept: one that never returns (a jump out) is forgotten.
    const NESTED: usize = 64;
    /// Routines watched at most: the run loop compares the PC with each of them.
    pub const ROUTINES: usize = 4;

    /// Probes on `routines` (the first [`Probes::ROUTINES`] of them).
    pub fn new(mut routines: Vec<u32>) -> Self {
        routines.truncate(Self::ROUTINES);
        Self {
            routines,
            ..Self::default()
        }
    }

    /// The entries as the run loop compares them (unused slots never match: odd).
    fn entries(&self) -> [u32; Self::ROUTINES] {
        std::array::from_fn(|k| self.routines.get(k).copied().unwrap_or(u32::MAX))
    }

    /// The return the run loop waits for, if a call is under way.
    fn waiting(&self) -> u32 {
        self.pending.last().map_or(u32::MAX, |c| c.ret)
    }

    /// Looks at the instruction about to run: a return from a probed call, or an entry.
    #[inline]
    fn check(&mut self, cpu: &Cpu) {
        let pc = cpu.pc;
        if let Some(c) = self.pending.last()
            && c.ret == pc
        {
            let mut c = self.pending.pop().expect("just seen");
            c.fr0 = f32::from_bits(cpu.fr[0]);
            if self.calls.len() < Self::KEPT {
                self.calls.push(c);
            } else {
                self.dropped += 1;
            }
        }
        if self.routines.contains(&pc) {
            if self.pending.len() == Self::NESTED {
                self.pending.remove(0);
            }
            self.pending.push(Call {
                routine: pc,
                ret: cpu.pr,
                r4: cpu.r[4],
                fr4: f32::from_bits(cpu.fr[4]),
                fr5: f32::from_bits(cpu.fr[5]),
                fr0: f32::NAN,
                cycles: cpu.cycles,
            });
        }
    }
}

/// An SH7059 running its firmware.
pub struct Machine {
    /// The core.
    pub cpu: Cpu,
    /// The chip around it.
    pub chip: Sh7059,
    /// Interrupts taken, per vector.
    pub interrupts: Box<[u64; 256]>,
    /// PC breakpoints, checked by [`Machine::run_until`] only.
    pub breakpoints: Vec<u32>,
    /// Set when [`Machine::run_until`] stopped at a breakpoint after taking an
    /// interrupt: the request level seen then, so the next step executes without taking
    /// another interrupt first, as it would have without the stop.
    resume: Option<u32>,
    /// Routines watched without stopping, if any.
    pub probes: Option<Box<Probes>>,
}

impl Machine {
    /// Powers the chip on: the CPU takes its reset vectors from the flash.
    pub fn new(mut chip: Sh7059, address_errors: bool) -> Self {
        let mut cpu = Cpu::new();
        cpu.address_errors = address_errors;
        cpu.power_on_reset(&mut chip);
        Self {
            cpu,
            chip,
            interrupts: Box::new([0; 256]),
            breakpoints: Vec::new(),
            resume: None,
            probes: None,
        }
    }

    /// One step: take an interrupt if one is pending and acceptable, then execute one
    /// instruction (or a branch with its slot), or, asleep, skip to the next event.
    /// Returns false when asleep with nothing left to wake it.
    pub fn step(&mut self) -> bool {
        let level = match self.resume.take() {
            Some(l) => l,
            None => self.take_interrupt(),
        };
        self.probe();
        self.execute(level)
    }

    /// Lets the probes see the instruction about to run, if there are probes.
    #[inline(always)]
    fn probe(&mut self) {
        if let Some(p) = self.probes.as_deref_mut()
            && !self.cpu.sleeping
        {
            p.check(&self.cpu);
        }
    }

    /// The probes' entries and the return they wait for, for the run loop to compare the
    /// PC with: never matching without probes.
    fn probe_state(&self) -> ([u32; Probes::ROUTINES], u32) {
        self.probes
            .as_deref()
            .map_or(([u32::MAX; Probes::ROUTINES], u32::MAX), |p| {
                (p.entries(), p.waiting())
            })
    }

    /// The PC is at a probed entry or at the awaited return: the probes take it. Returns
    /// the return to wait for next.
    #[cold]
    #[inline(never)]
    fn probe_hit(&mut self) -> u32 {
        self.probe();
        self.probes.as_deref().map_or(u32::MAX, Probes::waiting)
    }

    /// Takes the pending interrupt if the CPU accepts it now. Returns the request level.
    #[inline(always)]
    fn take_interrupt(&mut self) -> u32 {
        let (level, vector) = self.chip.pending;
        if level != 0 && self.cpu.accepts_interrupt(level) {
            self.cpu.interrupt(&mut self.chip, level, vector);
            self.interrupts[vector as usize] += 1;
        }
        level
    }

    /// Executes one instruction, or a branch with its slot, or, asleep, skips to the next
    /// event; then takes the chip events due. `level` is the request level seen before.
    #[inline(always)]
    fn execute(&mut self, level: u32) -> bool {
        if self.cpu.sleeping {
            // Asleep with nothing to come, or with a request the mask will never let in
            // (no instruction runs to lower it): stuck for good, as the chip would be.
            if self.chip.next_event == u64::MAX || level != 0 {
                return false;
            }
            self.cpu.cycles = self.cpu.cycles.max(self.chip.next_event);
        } else {
            if self.chip.trace.is_some() || self.chip.watch.is_some() {
                self.chip.trace_pc = self.cpu.pc;
                self.chip.instruction = self.cpu.instructions;
            }
            self.chip.now = self.cpu.cycles;
            self.cpu.step(&mut self.chip);
        }
        if self.cpu.cycles >= self.chip.next_event {
            self.chip.now = self.cpu.cycles;
            self.chip.advance();
        }
        true
    }

    /// Runs until `count` more instructions have executed, or the CPU sleeps with no
    /// event left to wake it.
    pub fn run(&mut self, count: u64) {
        let end = self.cpu.instructions + count;
        let mut idle = 0u32;
        if let Some(level) = self.resume.take() {
            self.probe();
            if !self.execute(level) {
                return;
            }
        }
        let debugging = self.chip.trace.is_some() || self.chip.watch.is_some();
        let probing = self.probes.is_some();
        let (entries, mut waiting) = self.probe_state();
        // The entries' span, so most instructions are passed with one comparison.
        let low = entries.iter().copied().min().unwrap_or(u32::MAX);
        let span = entries
            .iter()
            .filter(|&&e| e != u32::MAX)
            .map(|&e| e - low)
            .max()
            .unwrap_or(0);
        while self.cpu.instructions < end {
            let before = self.cpu.instructions;
            let level = self.take_interrupt();
            if probing {
                let pc = self.cpu.pc;
                if pc == waiting || (pc.wrapping_sub(low) <= span && entries.contains(&pc)) {
                    waiting = self.probe_hit();
                }
            }
            if !debugging && !self.cpu.sleeping {
                // The common case, without the checks `execute` makes for the rest.
                self.chip.now = self.cpu.cycles;
                self.cpu.step(&mut self.chip);
                if self.cpu.cycles >= self.chip.next_event {
                    self.chip.now = self.cpu.cycles;
                    self.chip.advance();
                }
            } else if !self.execute(level) {
                break;
            }
            // Asleep through a million timer events without a wake-up: give up.
            idle = if self.cpu.instructions == before {
                idle + 1
            } else {
                0
            };
            if idle > 1_000_000 {
                break;
            }
        }
    }
}

impl Machine {
    /// Like [`Machine::run`], but also stops at a breakpoint or a stopping watchpoint.
    /// A breakpoint at the PC it starts from does not stop it, so calling it again goes
    /// on past the breakpoint; `run_until(1)` steps one instruction (a branch and its
    /// delay slot count as two).
    pub fn run_until(&mut self, count: u64) -> Stop {
        let end = self.cpu.instructions + count;
        if self.breakpoints.is_empty() && self.chip.watch.is_none() {
            // Nothing to stop at: the fast loop, which stops only by count or asleep.
            self.run(count);
            return if self.cpu.instructions >= end {
                Stop::Count
            } else {
                Stop::Asleep
            };
        }
        let mut idle = 0u32;
        let mut first = true;
        while self.cpu.instructions < end {
            let level = match self.resume.take() {
                Some(l) => l,
                None => self.take_interrupt(),
            };
            if !first && !self.cpu.sleeping && self.breakpoints.contains(&self.cpu.pc) {
                self.resume = Some(level);
                return Stop::Breakpoint;
            }
            first = false;
            self.probe();
            let before = self.cpu.instructions;
            if !self.execute(level) {
                return Stop::Asleep;
            }
            if let Some(w) = self.chip.watch.as_mut()
                && w.stopped
            {
                w.stopped = false;
                return Stop::Watch;
            }
            idle = if self.cpu.instructions == before {
                idle + 1
            } else {
                0
            };
            if idle > 1_000_000 {
                return Stop::Asleep;
            }
        }
        Stop::Count
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ROM_SIZE;
    use crate::Stop;

    /// A program that starts CMT1 with interrupts at level 9, then sleeps; the handler
    /// counts in R8, clears CMF and returns.
    fn program() -> Vec<u8> {
        let mut f = vec![0u8; ROM_SIZE];
        let put16 =
            |f: &mut Vec<u8>, a: usize, v: u16| f[a..a + 2].copy_from_slice(&v.to_be_bytes());
        let put32 =
            |f: &mut Vec<u8>, a: usize, v: u32| f[a..a + 4].copy_from_slice(&v.to_be_bytes());
        put32(&mut f, 0, 0x1000); // reset PC
        put32(&mut f, 4, 0xFFFF_BFF0); // reset SP
        put32(&mut f, 192 * 4, 0x2000); // CMTI1 handler (VBR = 0)
        let pool = 0x1100u32;
        for (k, v) in [0xFFFF_F718u32, 0xFFFF_F71C, 0xFFFF_F710, 0xFFFF_ED12]
            .iter()
            .enumerate()
        {
            put32(&mut f, (pool + 4 * k as u32) as usize, *v);
        }
        // MOV.L @(disp,PC),Rn for the four addresses into R1..R4.
        let mut code: Vec<u16> = Vec::new();
        for k in 0..4u32 {
            let addr = 0x1000 + 2 * code.len() as u32;
            let disp = (pool + 4 * k - ((addr + 4) & !3)) / 4;
            code.push(0xD000 | ((k as u16 + 1) << 8) | disp as u16);
        }
        code.extend([
            0xE063, 0x2201, // MOV #99,R0; MOV.W R0,@R2   CMCOR1 = 99
            0xE040, 0x2101, // MOV #0x40,R0; MOV.W R0,@R1 CMCSR1: CMIE, Pφ/8
            0xE090, 0x600C, 0x2401, // MOV #0x90,R0; EXTU.B; MOV.W R0,@R4  IPRJ = 0x0090
            0xE002, 0x2301, // MOV #2,R0; MOV.W R0,@R3    CMSTR: start channel 1
            0xE000, 0x400E, // MOV #0,R0; LDC R0,SR       unmask
            0x001B, 0xAFFD, 0x0009, // loop: SLEEP; BRA loop; NOP
        ]);
        for (i, c) in code.iter().enumerate() {
            put16(&mut f, 0x1000 + 2 * i, *c);
        }
        // Handler: ADD #1,R8; MOV.W @R1,R0; MOV #0x40,R0; MOV.W R0,@R1; RTE; NOP
        for (i, c) in [0x7801u16, 0x6011, 0xE040, 0x2101, 0x002B, 0x0009]
            .iter()
            .enumerate()
        {
            put16(&mut f, 0x2000 + 2 * i, *c);
        }
        f
    }

    #[test]
    fn a_cmt_interrupt_wakes_the_cpu_every_period() {
        let mut m = Machine::new(Sh7059::new(&program()).unwrap(), true);
        // 100 counts at Pφ/8 = 3200 CPU cycles per period.
        // Bounded, so a broken interrupt path fails instead of sleeping forever.
        for _ in 0..1_000_000 {
            if m.cpu.r[8] >= 50 {
                break;
            }
            assert!(m.step());
        }
        assert_eq!(m.cpu.r[8], 50, "handler runs");
        assert_eq!(m.chip.cmt.matches[1], 50);
        let start = m.cpu.cycles - 50 * 3200;
        assert!(
            start < 100,
            "50 periods after a setup of a few dozen cycles: {start}"
        );
        assert_eq!(m.interrupts[192], 50);
        assert_eq!(m.cpu.r[8], 50, "the handler ran once per match");
        assert_eq!(m.cpu.exceptions, 50);
        assert_eq!(
            m.cpu.interrupt_mask(),
            9,
            "inside the handler, at its level"
        );
        for _ in 0..5 {
            m.step(); // the rest of the handler and its RTE
        }
        assert_eq!(m.cpu.interrupt_mask(), 0, "RTE restored the mask");
    }

    #[test]
    fn breakpoints_and_watchpoints_stop_run_until() {
        let mut m = Machine::new(Sh7059::new(&program()).unwrap(), true);
        // The handler's first instruction.
        m.breakpoints.push(0x2000);
        assert_eq!(m.run_until(1_000_000), Stop::Breakpoint);
        assert_eq!(m.cpu.pc, 0x2000);
        assert_eq!(m.cpu.r[8], 0, "stopped before the handler ran");
        // Resuming from the breakpoint goes past it, to the next time.
        assert_eq!(m.run_until(1_000_000), Stop::Breakpoint);
        assert_eq!(m.cpu.r[8], 1);
        // One step.
        let n = m.cpu.instructions;
        assert_eq!(m.run_until(1), Stop::Count);
        assert_eq!(m.cpu.instructions, n + 1);
        assert_eq!(m.cpu.pc, 0x2002);
        m.breakpoints.clear();
        // A stopping watchpoint on the write to CMCSR1 in the handler.
        m.chip.watch = Some(Box::new(crate::debug::Watch::new(
            vec![crate::debug::Watchpoint {
                start: 0xFFFF_F718,
                end: 0xFFFF_F71A,
                read: false,
                write: true,
                stop: true,
            }],
            10,
        )));
        assert_eq!(m.run_until(1_000_000), Stop::Watch);
        let w = m.chip.watch.as_ref().unwrap();
        assert_eq!(w.hits.len(), 1);
        assert_eq!(
            (w.hits[0].pc, w.hits[0].value, w.hits[0].write),
            (0x2006, 0x40, true)
        );
        assert_eq!(m.cpu.pc, 0x2008, "the writing instruction has run");
        // Reads are not reported by a write-only watchpoint.
        m.chip.watch.as_mut().unwrap().points[0].stop = false;
        assert_eq!(m.run_until(20_000), Stop::Count);
        assert!(m.chip.watch.as_ref().unwrap().hits.iter().all(|h| h.write));
    }

    #[test]
    fn a_masked_interrupt_is_not_taken() {
        let mut f = program();
        f[0x1000 + 2 * 14..0x1000 + 2 * 14 + 2].copy_from_slice(&0x0009u16.to_be_bytes()); // no LDC: SR.I stays 15
        let mut m = Machine::new(Sh7059::new(&f).unwrap(), true);
        m.run(1000);
        assert_eq!(m.interrupts[192], 0);
        assert!(
            m.cpu.sleeping,
            "asleep for good: the request can never get in"
        );
    }

    #[test]
    fn a_probe_sees_the_call_and_its_result_without_stopping() {
        let mut f = vec![0u8; ROM_SIZE];
        let put16 =
            |f: &mut Vec<u8>, a: usize, v: u16| f[a..a + 2].copy_from_slice(&v.to_be_bytes());
        f[0..4].copy_from_slice(&0x1000u32.to_be_bytes()); // reset PC
        f[4..8].copy_from_slice(&0xFFFF_BFF0u32.to_be_bytes()); // reset SP
        for (a, op) in [
            (0x1000, 0xD403), // MOV.L @(0x1010),R4
            (0x1002, 0xB002), // BSR 0x100A
            (0x1004, 0x0009), // NOP (slot)
            (0x1006, 0xAFFE), // BRA 0x1006: spin here
            (0x1008, 0x0009), // NOP (slot)
            (0x100A, 0xF09D), // FLDI1 FR0
            (0x100C, 0x000B), // RTS
            (0x100E, 0x0009), // NOP (slot)
        ] {
            put16(&mut f, a, op);
        }
        f[0x1010..0x1014].copy_from_slice(&0x0009_DADCu32.to_be_bytes());
        let mut m = Machine::new(Sh7059::new(&f).unwrap(), true);
        m.probes = Some(Box::new(Probes::new(vec![0x100A])));
        assert_eq!(m.run_until(40), Stop::Count);
        let p = m.probes.as_ref().unwrap();
        assert_eq!(p.calls.len(), 1, "{:?}", p.calls);
        let c = p.calls[0];
        assert_eq!((c.routine, c.ret, c.r4), (0x100A, 0x1006, 0x0009_DADC));
        assert_eq!(c.fr0, 1.0);
    }
}
