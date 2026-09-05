//! Finite, transport-free OBD target candidates for bounded VAG exploration.
//!
//! This is a candidate catalogue, not a discovery result.  Only the engine
//! entry carries a semantic name; the remaining entries are deliberately
//! anonymous physical address pairs until the vehicle provides evidence.

/// One closed physical request/responder candidate.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ObdCandidate {
    name: &'static str,
    target: &'static str,
    expected_responder: &'static str,
}

impl ObdCandidate {
    /// A stable display name.  Names do not assert an ECU role except for
    /// `engine`.
    pub const fn name(self) -> &'static str {
        self.name
    }

    /// The physical ELM request header.
    pub const fn target(self) -> &'static str {
        self.target
    }

    /// The physical ELM response header expected for this candidate.
    pub const fn expected_responder(self) -> &'static str {
        self.expected_responder
    }
}

const CANDIDATES: [ObdCandidate; 8] = [
    ObdCandidate {
        name: "engine",
        target: "7E0",
        expected_responder: "7E8",
    },
    ObdCandidate {
        name: "candidate-7E1",
        target: "7E1",
        expected_responder: "7E9",
    },
    ObdCandidate {
        name: "candidate-7E2",
        target: "7E2",
        expected_responder: "7EA",
    },
    ObdCandidate {
        name: "candidate-7E3",
        target: "7E3",
        expected_responder: "7EB",
    },
    ObdCandidate {
        name: "candidate-7E4",
        target: "7E4",
        expected_responder: "7EC",
    },
    ObdCandidate {
        name: "candidate-7E5",
        target: "7E5",
        expected_responder: "7ED",
    },
    ObdCandidate {
        name: "candidate-7E6",
        target: "7E6",
        expected_responder: "7EE",
    },
    ObdCandidate {
        name: "candidate-7E7",
        target: "7E7",
        expected_responder: "7EF",
    },
];

/// The only candidate set available to the bounded scanner.
pub const fn candidates() -> &'static [ObdCandidate] {
    &CANDIDATES
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidates_are_closed_and_ordered() {
        let candidates = candidates();
        assert_eq!(candidates.len(), 8);
        assert_eq!(candidates[0].name(), "engine");

        let pairs: Vec<_> = candidates
            .iter()
            .map(|candidate| (candidate.target(), candidate.expected_responder()))
            .collect();
        assert_eq!(
            pairs,
            vec![
                ("7E0", "7E8"),
                ("7E1", "7E9"),
                ("7E2", "7EA"),
                ("7E3", "7EB"),
                ("7E4", "7EC"),
                ("7E5", "7ED"),
                ("7E6", "7EE"),
                ("7E7", "7EF"),
            ]
        );
    }

    #[test]
    fn only_engine_has_a_semantic_name() {
        assert_eq!(candidates()[0].name(), "engine");
        assert!(candidates()[1..]
            .iter()
            .all(|candidate| candidate.name().starts_with("candidate-")));
    }
}
