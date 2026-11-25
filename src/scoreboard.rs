//! The scoreboard: counts per column and the result of the count.

use serde::Serialize;

/// The number of seats in the Chamber of Deputies.
pub const SEATS: i64 = 513;
/// The default threshold: absolute majority of the seats. For the rule
/// [`Rule::Simple`] it is the quorum (RICD art. 183).
pub const DEFAULT_THRESHOLD: i64 = 257;
/// Three fifths of the seats: the threshold of a constitutional amendment (CF art. 60).
pub const PEC_THRESHOLD: i64 = 308;

/// How a project is approved.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, serde::Deserialize, sqlx::Type)]
#[serde(rename_all = "snake_case")]
#[sqlx(type_name = "text", rename_all = "snake_case")]
pub enum Rule {
    /// "Yes" must reach a fixed number of votes (257 for a PLP, 308 for a PEC).
    #[default]
    Fixed,
    /// "Yes" must be more than "no", with a quorum of present deputies (ordinary bills).
    Simple,
}

impl Rule {
    /// The rule and the threshold that the Chamber applies to a bill kind.
    /// `PEC` → 308 votes; `PLP` → 257 votes; other kinds → simple majority, quorum 257.
    #[must_use]
    pub fn for_bill_kind(kind: &str) -> (Self, i64) {
        match kind.trim().to_uppercase().as_str() {
            "PEC" => (Self::Fixed, PEC_THRESHOLD),
            "PLP" => (Self::Fixed, DEFAULT_THRESHOLD),
            _ => (Self::Simple, DEFAULT_THRESHOLD),
        }
    }
}

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
        /// Votes above the threshold. For [`Rule::Simple`]: votes of "yes" minus votes of "no".
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
        /// Present deputies that the quorum still needs. Always 0 for [`Rule::Fixed`].
        #[serde(default)]
        quorum_missing: i64,
    },
}

/// Compute the outcome with the rule of the project.
#[must_use]
pub fn outcome_with(rule: Rule, counts: Counts, threshold: i64, total: i64) -> Outcome {
    match rule {
        Rule::Fixed => outcome(counts, threshold, total),
        Rule::Simple => simple_outcome(counts, threshold),
    }
}

/// Compute the outcome of a simple majority with a quorum.
///
/// "Open" deputies are "disputed" and "undecided" (custom columns included). They can still
/// vote either way or stay away. "Absent" deputies do not vote. The chair counts for the
/// quorum and does not vote. A tie is not approval.
#[must_use]
pub fn simple_outcome(counts: Counts, quorum: i64) -> Outcome {
    let open = counts.disputed + counts.undecided;
    let present = counts.yes + counts.no + counts.chair;
    if present + open < quorum || counts.yes + open <= counts.no {
        return Outcome::Rejected;
    }
    if counts.yes > counts.no + open && present >= quorum {
        return Outcome::Approved {
            margin: counts.yes - counts.no,
        };
    }
    // "Yes" wins for sure when yes + k > no + (open - k).
    let missing = ((counts.no + open - counts.yes) / 2 + 1).max(0);
    let missing = if counts.yes > counts.no + open {
        0
    } else {
        missing
    };
    Outcome::Open {
        missing,
        reachable: counts.yes + counts.disputed > counts.no && present + counts.disputed >= quorum,
        quorum_missing: (quorum - present).max(0),
    }
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
            quorum_missing: 0,
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
                reachable: true,
                quorum_missing: 0
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
                reachable: false,
                quorum_missing: 0
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

    fn simple(yes: i64, no: i64, disputed: i64, undecided: i64, chair: i64) -> Counts {
        Counts {
            yes,
            no,
            disputed,
            undecided,
            chair,
            absent: SEATS - yes - no - disputed - undecided - chair,
        }
    }

    #[test]
    fn bill_kind_picks_the_rule() {
        assert_eq!(Rule::for_bill_kind("PEC"), (Rule::Fixed, 308));
        assert_eq!(Rule::for_bill_kind("plp "), (Rule::Fixed, 257));
        assert_eq!(Rule::for_bill_kind("PL"), (Rule::Simple, 257));
        assert_eq!(Rule::for_bill_kind("MPV"), (Rule::Simple, 257));
        assert_eq!(Rule::for_bill_kind("PDL"), (Rule::Simple, 257));
        assert_eq!(Rule::default(), Rule::Fixed);
    }

    #[test]
    fn simple_majority_of_a_real_roll_call_is_approved() {
        // Urgency of PL 4675/2025: 276 × 186, two abstentions, the chair.
        let c = simple(276, 186, 0, 2, 1);
        assert_eq!(
            outcome_with(Rule::Simple, c, 257, SEATS),
            Outcome::Approved { margin: 90 }
        );
        // The same count fails the fixed rule of a PEC.
        assert!(matches!(
            outcome_with(Rule::Fixed, c, 308, SEATS),
            Outcome::Open { missing: 32, .. }
        ));
    }

    #[test]
    fn simple_majority_needs_the_quorum() {
        // A new board: everybody undecided. Not approved, not rejected.
        let c = simple(0, 0, 0, 513, 0);
        assert_eq!(
            simple_outcome(c, 257),
            Outcome::Open {
                missing: 257,
                reachable: false,
                quorum_missing: 257
            }
        );
        // 200 × 10 with everybody else absent: no quorum is possible.
        assert_eq!(
            simple_outcome(simple(200, 10, 0, 0, 0), 257),
            Outcome::Rejected
        );
        // 200 × 10, 60 undecided: "yes" is ahead for sure but the quorum is open.
        assert_eq!(
            simple_outcome(simple(200, 10, 0, 60, 0), 257),
            Outcome::Open {
                missing: 0,
                reachable: false,
                quorum_missing: 47
            }
        );
    }

    #[test]
    fn simple_majority_tie_is_not_approval() {
        assert_eq!(
            simple_outcome(simple(150, 150, 0, 0, 0), 257),
            Outcome::Rejected
        );
        assert_eq!(
            simple_outcome(simple(150, 151, 0, 0, 1), 257),
            Outcome::Rejected
        );
    }

    #[test]
    fn simple_majority_open_counts_the_swing() {
        // 200 × 180, 40 disputed: "yes" needs 11 of the 40 to win for sure.
        assert_eq!(
            simple_outcome(simple(200, 180, 40, 0, 0), 257),
            Outcome::Open {
                missing: 11,
                reachable: true,
                quorum_missing: 0
            }
        );
        // 150 × 200, 100 undecided: still possible, but not with the disputed only.
        assert_eq!(
            simple_outcome(simple(150, 200, 0, 100, 0), 257),
            Outcome::Open {
                missing: 76,
                reachable: false,
                quorum_missing: 0
            }
        );
    }
}
