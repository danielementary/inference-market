/// What a faulty worker (`--faulty`) does with successive jobs, in a loop. The rotation restarts
/// from `Slow` on every new connection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    /// Streams at half the advertised throughput, so the SLA is missed.
    Slow,
    /// Accepts the job and never sends anything.
    NoStart,
    /// Drops the connection after `CRASH_AFTER` tokens, without `Done`.
    Crash,
}

pub const CRASH_AFTER: u64 = 5;

impl Fault {
    const CYCLE: [Fault; 3] = [Fault::Slow, Fault::NoStart, Fault::Crash];

    /// Fault for the `index`-th job of a connection.
    pub fn for_job(index: usize) -> Fault {
        Self::CYCLE[index % Self::CYCLE.len()]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotates() {
        let seen: Vec<_> = (0..4).map(Fault::for_job).collect();
        assert_eq!(
            seen,
            [Fault::Slow, Fault::NoStart, Fault::Crash, Fault::Slow]
        );
    }
}
