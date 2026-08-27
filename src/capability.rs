use std::{fmt, time::Duration};

/// Where the operational capability values came from.
///
/// A measured capability is an estimate from evidence, not a guaranteed
/// adapter maximum.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CapabilityProvenance {
    BuiltInDefault,
    MeasuredFromCapture,
}

/// The small, adapter-neutral set of session limits used by scheduling.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HardwareCapability {
    request_budget_per_second: u32,
    representative_read_latency: Duration,
    provenance: CapabilityProvenance,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CapabilityError {
    RequestBudgetMustBePositive,
    ReadLatencyMustBePositive,
}

impl HardwareCapability {
    /// Conservative fallback until a capability report supplies evidence.
    pub const fn conservative_default() -> Self {
        Self {
            request_budget_per_second: 4,
            representative_read_latency: Duration::from_millis(250),
            provenance: CapabilityProvenance::BuiltInDefault,
        }
    }

    pub fn new(
        request_budget_per_second: u32,
        representative_read_latency: Duration,
        provenance: CapabilityProvenance,
    ) -> Result<Self, CapabilityError> {
        if request_budget_per_second == 0 {
            return Err(CapabilityError::RequestBudgetMustBePositive);
        }
        if representative_read_latency.is_zero() {
            return Err(CapabilityError::ReadLatencyMustBePositive);
        }
        Ok(Self {
            request_budget_per_second,
            representative_read_latency,
            provenance,
        })
    }

    pub const fn request_budget_per_second(self) -> u32 {
        self.request_budget_per_second
    }

    pub const fn representative_read_latency(self) -> Duration {
        self.representative_read_latency
    }

    pub const fn provenance(self) -> CapabilityProvenance {
        self.provenance
    }
}

impl Default for HardwareCapability {
    fn default() -> Self {
        Self::conservative_default()
    }
}

impl fmt::Display for CapabilityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::RequestBudgetMustBePositive => "request budget must be greater than zero",
            Self::ReadLatencyMustBePositive => {
                "representative read latency must be greater than zero"
            }
        })
    }
}

impl std::error::Error for CapabilityError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conservative_default_is_valid_and_identified() {
        let capability = HardwareCapability::conservative_default();

        assert_eq!(capability.request_budget_per_second(), 4);
        assert_eq!(
            capability.representative_read_latency(),
            Duration::from_millis(250)
        );
        assert_eq!(
            capability.provenance(),
            CapabilityProvenance::BuiltInDefault
        );
        assert_eq!(capability, HardwareCapability::default());
    }

    #[test]
    fn rejects_zero_request_budget() {
        assert_eq!(
            HardwareCapability::new(
                0,
                Duration::from_millis(250),
                CapabilityProvenance::MeasuredFromCapture,
            ),
            Err(CapabilityError::RequestBudgetMustBePositive)
        );
    }

    #[test]
    fn rejects_zero_read_latency() {
        assert_eq!(
            HardwareCapability::new(4, Duration::ZERO, CapabilityProvenance::MeasuredFromCapture,),
            Err(CapabilityError::ReadLatencyMustBePositive)
        );
    }

    #[test]
    fn preserves_measured_provenance_and_values() {
        let capability = HardwareCapability::new(
            6,
            Duration::from_millis(187),
            CapabilityProvenance::MeasuredFromCapture,
        )
        .unwrap();

        assert_eq!(capability.request_budget_per_second(), 6);
        assert_eq!(
            capability.representative_read_latency(),
            Duration::from_millis(187)
        );
        assert_eq!(
            capability.provenance(),
            CapabilityProvenance::MeasuredFromCapture
        );
    }
}
