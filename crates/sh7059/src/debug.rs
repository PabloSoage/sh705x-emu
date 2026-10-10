//! Debugging aids: memory watchpoints, checked on every CPU or DMAC access to flash,
//! RAM and registers while [`crate::Sh7059::watch`] is set. Breakpoints and stepping are
//! in [`crate::Machine::run_until`].

/// An address range to watch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Watchpoint {
    /// First address.
    pub start: u32,
    /// One past the last address.
    pub end: u32,
    /// Report reads.
    pub read: bool,
    /// Report writes.
    pub write: bool,
    /// Stop [`crate::Machine::run_until`] when hit.
    pub stop: bool,
}

/// One watched access.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WatchHit {
    /// Instruction that made it (of the branch, for a delay slot).
    pub pc: u32,
    /// Address accessed.
    pub addr: u32,
    /// Bytes: 1, 2 or 4.
    pub size: u8,
    /// Value read or written.
    pub value: u32,
    /// A write.
    pub write: bool,
    /// The CPU instruction count when it happened.
    pub instruction: u64,
}

/// The watchpoints and what they caught.
#[derive(Clone, Debug, Default)]
pub struct Watch {
    /// What to watch.
    pub points: Vec<Watchpoint>,
    /// Accesses caught, oldest first; at most `limit` are kept, and later ones counted in
    /// `dropped`.
    pub hits: Vec<WatchHit>,
    /// Most hits kept.
    pub limit: usize,
    /// Hits not kept because `hits` was full.
    pub dropped: u64,
    /// A watchpoint with `stop` was hit since this was last cleared.
    pub stopped: bool,
}

impl Watch {
    /// Watches `points`, keeping up to `limit` hits.
    pub fn new(points: Vec<Watchpoint>, limit: usize) -> Self {
        Self {
            points,
            limit,
            ..Self::default()
        }
    }

    /// Records the access if a watchpoint covers it.
    #[inline]
    pub(crate) fn check(
        &mut self,
        pc: u32,
        instruction: u64,
        addr: u32,
        bytes: &[u8],
        write: bool,
    ) {
        let end = addr.wrapping_add(bytes.len() as u32);
        let Some(p) = self
            .points
            .iter()
            .find(|p| (if write { p.write } else { p.read }) && addr < p.end && p.start < end)
        else {
            return;
        };
        self.stopped |= p.stop;
        if self.hits.len() < self.limit {
            let value = bytes.iter().fold(0u32, |v, b| (v << 8) | *b as u32);
            self.hits.push(WatchHit {
                pc,
                addr,
                size: bytes.len() as u8,
                value,
                write,
                instruction,
            });
        } else {
            self.dropped += 1;
        }
    }
}
