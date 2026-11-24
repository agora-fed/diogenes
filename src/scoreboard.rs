//! The scoreboard: counts per column and the result of the count.

use serde::Serialize;

/// The number of seats in the Chamber of Deputies.
pub const SEATS: i64 = 513;
/// The default threshold: absolute majority of the seats.
pub const DEFAULT_THRESHOLD: i64 = 257;

/// The counts of the fixed columns.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct Counts {
    /// Deputies in the "yes" column.
    pub yes: i64,
    /// Deputies in the "no" column.
    pub no: i64,
    /// Deputies in the "disputed" column.
    pub disputed: i64,
    /// Deputies in the "absent" column.
    pub absent: i64,
    /// Deputies in the "chair" column.
    pub chair: i64,
    /// Deputies with no position, and deputies in custom columns.
    pub undecided: i64,
}

/// The result of a count.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Outcome {
    /// "Yes" reached the threshold.
    Approved {
        /// Votes above the threshold.
        margin: i64,
    },
    /// "Yes" can no longer reach the threshold.
    Rejected,
    /// The count is open.
    Open {
        /// Votes that "yes" still needs.
        missing: i64,
        /// True when "yes" plus "disputed" reaches the threshold.
        reachable: bool,
    },
}

/// Compute the outcome. `total` is the number of deputies in office.
#[must_use]
pub fn outcome(counts: Counts, threshold: i64, total: i64) -> Outcome {
    if counts.yes >= threshold {
        Outcome::Approved {
            margin: counts.yes - threshold,
        }
    } else if total - counts.no < threshold {
        Outcome::Rejected
    } else {
        Outcome::Open {
            missing: threshold - counts.yes,
            reachable: counts.yes + counts.disputed >= threshold,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approved_when_yes_reaches_threshold() {
        let c = Counts {
            yes: 300,
            ..Counts::default()
        };
        assert_eq!(outcome(c, 257, SEATS), Outcome::Approved { margin: 43 });
    }

    #[test]
    fn rejected_when_no_blocks_threshold() {
        let c = Counts {
            no: 257,
            ..Counts::default()
        };
        assert_eq!(outcome(c, 257, SEATS), Outcome::Rejected);
    }

    #[test]
    fn open_reports_missing_and_reach() {
        let c = Counts {
            yes: 200,
            disputed: 60,
            ..Counts::default()
        };
        assert_eq!(
            outcome(c, 257, SEATS),
            Outcome::Open {
                missing: 57,
                reachable: true
            }
        );
        let c = Counts {
            yes: 200,
            disputed: 10,
            ..Counts::default()
        };
        assert_eq!(
            outcome(c, 257, SEATS),
            Outcome::Open {
                missing: 57,
                reachable: false
            }
        );
    }

    #[test]
    fn threshold_edge_is_approved() {
        let c = Counts {
            yes: 257,
            ..Counts::default()
        };
        assert_eq!(outcome(c, 257, SEATS), Outcome::Approved { margin: 0 });
    }
}
