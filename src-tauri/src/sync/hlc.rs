//! Hybrid Logical Clock (HLC) — a monotonic, causally-aware timestamp that stays sensible even when
//! two devices' wall clocks disagree. Each local write gets a fresh HLC; remote writes advance our
//! clock past theirs. Last-writer-wins compares the *encoded* HLC string lexicographically, with the
//! node id as a deterministic tiebreaker so two devices never disagree about which write "won".
//!
//! Encoding: `{wall:016x}-{counter:08x}-{node}`. Fixed-width hex means lexicographic string order
//! equals numeric (wall, counter) order — so a plain `>` on the strings is a correct total order.

/// The mutable clock state we persist (in `sync_self`) between writes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HlcState {
    pub wall: u64,    // last physical time we observed (ms since epoch)
    pub counter: u32, // logical counter, bumped when wall doesn't advance
}

impl HlcState {
    /// Stamp a new *local* event. `now_ms` is the current physical wall clock.
    pub fn tick(&mut self, now_ms: u64) -> (u64, u32) {
        let prev = self.wall;
        self.wall = self.wall.max(now_ms);
        self.counter = if self.wall == prev { self.counter + 1 } else { 0 };
        (self.wall, self.counter)
    }

    /// Advance our clock after *receiving* a remote HLC, so our next local write orders after it.
    /// Returns the resulting (wall, counter) for our clock.
    pub fn observe(&mut self, now_ms: u64, remote_wall: u64, remote_counter: u32) -> (u64, u32) {
        let prev_wall = self.wall;
        let prev_counter = self.counter;
        let l = prev_wall.max(remote_wall).max(now_ms);
        self.counter = if l == prev_wall && l == remote_wall {
            prev_counter.max(remote_counter) + 1
        } else if l == prev_wall {
            prev_counter + 1
        } else if l == remote_wall {
            remote_counter + 1
        } else {
            0
        };
        self.wall = l;
        (self.wall, self.counter)
    }
}

/// Encode an HLC to its sortable string form. `node` is this device's id (any short ascii token).
pub fn encode(wall: u64, counter: u32, node: &str) -> String {
    format!("{wall:016x}-{counter:08x}-{node}")
}

/// Decode an HLC string back to (wall, counter, node). Returns None if malformed.
pub fn decode(s: &str) -> Option<(u64, u32, String)> {
    let mut parts = s.splitn(3, '-');
    let wall = u64::from_str_radix(parts.next()?, 16).ok()?;
    let counter = u32::from_str_radix(parts.next()?, 16).ok()?;
    let node = parts.next()?.to_string();
    Some((wall, counter, node))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tick_is_monotonic_even_when_clock_stalls() {
        let mut s = HlcState::default();
        let a = s.tick(1000);
        let b = s.tick(1000); // same physical ms → counter bumps
        let c = s.tick(1000);
        let d = s.tick(2000); // physical advances → counter resets
        assert_eq!(a, (1000, 0));
        assert_eq!(b, (1000, 1));
        assert_eq!(c, (1000, 2));
        assert_eq!(d, (2000, 0));
    }

    #[test]
    fn tick_handles_a_clock_that_goes_backwards() {
        let mut s = HlcState::default();
        let _ = s.tick(5000);
        // Physical clock jumps backwards: HLC must not regress.
        let b = s.tick(3000);
        assert_eq!(b, (5000, 1));
    }

    #[test]
    fn encoded_order_matches_logical_order() {
        // Lexicographic string compare must equal (wall, counter) numeric order.
        let older = encode(1000, 5, "nodeA");
        let newer_counter = encode(1000, 6, "nodeA");
        let newer_wall = encode(2000, 0, "nodeA");
        assert!(newer_counter > older);
        assert!(newer_wall > newer_counter);
        // Node id only breaks ties at equal (wall, counter).
        assert!(encode(1000, 5, "nodeB") > encode(1000, 5, "nodeA"));
    }

    #[test]
    fn observe_advances_past_remote() {
        let mut s = HlcState { wall: 1000, counter: 0 };
        // Receive a remote write from the future (their clock is ahead).
        let (w, c) = s.observe(900, 5000, 3);
        assert_eq!((w, c), (5000, 4));
        // A subsequent local tick orders strictly after what we received.
        let local = s.tick(900);
        assert!(encode(local.0, local.1, "me") > encode(5000, 3, "them"));
    }

    #[test]
    fn decode_roundtrips() {
        let s = encode(123456, 7, "abc123");
        assert_eq!(decode(&s), Some((123456, 7, "abc123".to_string())));
        assert_eq!(decode("garbage"), None);
    }
}
