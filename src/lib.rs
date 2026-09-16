pub(crate) mod adapter;
pub mod audit;
pub mod ble;
pub mod cache_validation;
pub mod capability;
pub mod capture;
pub mod capture_events;
pub(crate) mod capture_writer;
pub mod capture_replay;
pub mod capture_report;
pub mod capture_tui;
pub mod diagnostic_job;
pub mod dpf_report;
pub mod dtc;
pub mod ea189;
pub mod ecu_identification;
pub mod ecu_identification_discovery;
pub mod effective_knowledge;
pub(crate) mod elm;
pub mod functional_discovery;
pub mod identity;
pub mod inventory_facts;
pub mod jsonl_capture;
pub mod knowledge_db;
pub mod layout_observation;
mod legacy_recording;
pub(crate) mod mf4_capture;
pub mod observed_inventory;
pub mod protocol;
mod read;
pub mod runtime_actor;
pub mod runtime_reducer;
pub mod runtime_state;
pub mod safety;
pub mod scheduler;
pub mod subscription_policy;
pub mod telemetry;
#[cfg(test)]
mod test_support;
pub mod topology;
pub mod topology_provider;
pub mod tui;
pub mod vehicle;
pub mod vehicle_cache;
pub mod vehicle_knowledge;

pub use identity::{
    decode_mode09_pid02, IdentityDecodeError, IdentitySource, Provenance, VehicleIdentity, Vin,
    VinError, MODE09_PID02_REQUEST,
};
pub use legacy_recording::{record, replay};
pub use read::{prepare_read, supported_signals, ReadRequest, Transaction};
pub use vehicle::{supported_profiles, ProfileMetadata, SignalMetadata};

pub fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_formats_bytes_as_uppercase_space_separated_pairs() {
        assert_eq!(hex(&[]), "");
        assert_eq!(hex(&[0x00]), "00");
        assert_eq!(hex(&[0x1a, 0xf8, 0x00]), "1A F8 00");
    }
}
