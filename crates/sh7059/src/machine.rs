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
        self.execute(level)
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
        if let Some(level) = self.resume.take()
            && !self.execute(level)
        {
            return;
        }
        let debugging = self.chip.trace.is_some() || self.chip.watch.is_some();
        while self.cpu.instructions < end {
            let before = self.cpu.instructions;
            let level = self.take_interrupt();
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
}
