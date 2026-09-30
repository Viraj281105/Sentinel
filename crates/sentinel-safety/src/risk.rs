/// Risk classification of a cleanup target. Ordered from least to most risky.
///
/// `Protected` is never actionable; a planner risk ceiling must always be below it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RiskLevel {
    Safe,
    LowRisk,
    MediumRisk,
    HighRisk,
    Protected,
}

impl RiskLevel {
    /// Whether an operation at this level can ever be executed.
    pub fn is_actionable(self) -> bool {
        self != Self::Protected
    }

    /// Return the higher of two levels. Used when combining an untrusted claim
    /// (e.g. from AI) with a deterministic one: risk may only be raised.
    pub fn max_with(self, other: Self) -> Self {
        self.max(other)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordering_and_actionability() {
        assert!(RiskLevel::Safe < RiskLevel::LowRisk);
        assert!(RiskLevel::HighRisk < RiskLevel::Protected);
        assert!(!RiskLevel::Protected.is_actionable());
        assert!(RiskLevel::HighRisk.is_actionable());
        assert_eq!(
            RiskLevel::Safe.max_with(RiskLevel::MediumRisk),
            RiskLevel::MediumRisk
        );
        assert_eq!(
            RiskLevel::HighRisk.max_with(RiskLevel::Safe),
            RiskLevel::HighRisk
        );
    }
}
