use crate::identity::Vin;
use serde_json::{json, Value};

const SOURCE_NAME: &str = "NHTSA vPIC DecodeWMI";
const SOURCE_DOCS_URL: &str = "https://vpic.nhtsa.dot.gov/api/";

struct PrefixRecord {
    wmi: &'static str,
    manufacturer: &'static str,
    make_hint: Option<&'static str>,
}

// Snapshot of individually checked vPIC DecodeWMI records; this is deliberately
// partial and does not imply a model, engine, or ECU profile.
const PREFIXES: &[PrefixRecord] = &[
    PrefixRecord {
        wmi: "WVW",
        manufacturer: "VOLKSWAGEN AG",
        make_hint: Some("Volkswagen"),
    },
    PrefixRecord {
        wmi: "WVG",
        manufacturer: "VOLKSWAGEN AG",
        make_hint: Some("Volkswagen"),
    },
    PrefixRecord {
        wmi: "WV1",
        manufacturer: "VOLKSWAGEN GROUP OF AMERICA, INC.",
        make_hint: Some("Volkswagen"),
    },
    PrefixRecord {
        wmi: "WV2",
        manufacturer: "VOLKSWAGEN GROUP OF AMERICA, INC.",
        make_hint: Some("Volkswagen"),
    },
    PrefixRecord {
        wmi: "WAU",
        manufacturer: "AUDI AG",
        make_hint: Some("Audi"),
    },
    PrefixRecord {
        wmi: "WA1",
        manufacturer: "AUDI AG",
        make_hint: Some("Audi"),
    },
    PrefixRecord {
        wmi: "WUA",
        manufacturer: "AUDI SPORT GMBH",
        make_hint: Some("Audi"),
    },
    PrefixRecord {
        wmi: "TRU",
        manufacturer: "AUDI AG",
        make_hint: Some("Audi"),
    },
    PrefixRecord {
        wmi: "3VW",
        manufacturer: "VOLKSWAGEN DE MEXICO SA DE CV",
        make_hint: Some("Volkswagen"),
    },
    PrefixRecord {
        wmi: "WDB",
        manufacturer: "MERCEDES-BENZ CARS",
        make_hint: None,
    },
    PrefixRecord {
        wmi: "WDC",
        manufacturer: "MERCEDES-BENZ CARS",
        make_hint: Some("Mercedes-Benz"),
    },
    PrefixRecord {
        wmi: "WDD",
        manufacturer: "MERCEDES-BENZ CARS",
        make_hint: Some("Mercedes-Benz"),
    },
    PrefixRecord {
        wmi: "WBA",
        manufacturer: "BMW AG",
        make_hint: Some("BMW"),
    },
    PrefixRecord {
        wmi: "WBS",
        manufacturer: "BMW M GMBH",
        make_hint: Some("BMW"),
    },
    PrefixRecord {
        wmi: "WBY",
        manufacturer: "BMW AG",
        make_hint: Some("BMW"),
    },
    PrefixRecord {
        wmi: "WMW",
        manufacturer: "BMW AG",
        make_hint: Some("MINI"),
    },
    PrefixRecord {
        wmi: "W0L",
        manufacturer: "ADAM OPEL AG",
        make_hint: Some("Opel"),
    },
    PrefixRecord {
        wmi: "VF1",
        manufacturer: "RENAULT GROUP",
        make_hint: None,
    },
    PrefixRecord {
        wmi: "VF3",
        manufacturer: "AUTOMOBILES PEUGEOT",
        make_hint: Some("Peugeot"),
    },
    PrefixRecord {
        wmi: "YV1",
        manufacturer: "VOLVO CAR CORPORATION",
        make_hint: Some("Volvo"),
    },
    PrefixRecord {
        wmi: "ZFA",
        manufacturer: "STELLANTIS EUROPE SPA",
        make_hint: Some("Fiat"),
    },
    PrefixRecord {
        wmi: "ZAR",
        manufacturer: "ALFA ROMEO S.P.A.",
        make_hint: Some("Alfa Romeo"),
    },
    PrefixRecord {
        wmi: "ZFF",
        manufacturer: "FERRARI S.P.A.",
        make_hint: Some("Ferrari"),
    },
    PrefixRecord {
        wmi: "ZAM",
        manufacturer: "MASERATI S.P.A.",
        make_hint: Some("Maserati"),
    },
    PrefixRecord {
        wmi: "JHM",
        manufacturer: "HONDA MOTOR CO., LTD.",
        make_hint: Some("Honda"),
    },
    PrefixRecord {
        wmi: "1HG",
        manufacturer: "AMERICAN HONDA MOTOR CO., INC.",
        make_hint: Some("Honda"),
    },
    PrefixRecord {
        wmi: "JTD",
        manufacturer: "TOYOTA MOTOR CORPORATION",
        make_hint: Some("Toyota"),
    },
    PrefixRecord {
        wmi: "4T1",
        manufacturer: "TOYOTA MOTOR MANUFACTURING, KENTUCKY, INC.",
        make_hint: Some("Toyota"),
    },
    PrefixRecord {
        wmi: "2T1",
        manufacturer: "TOYOTA MOTOR MANUFACTURING CANADA",
        make_hint: Some("Toyota"),
    },
    PrefixRecord {
        wmi: "JN1",
        manufacturer: "NISSAN MOTOR COMPANY, LTD",
        make_hint: None,
    },
    PrefixRecord {
        wmi: "JM1",
        manufacturer: "MAZDA MOTOR CORPORATION",
        make_hint: Some("Mazda"),
    },
    PrefixRecord {
        wmi: "KMH",
        manufacturer: "HYUNDAI MOTOR CO",
        make_hint: None,
    },
    PrefixRecord {
        wmi: "KNA",
        manufacturer: "KIA CORPORATION",
        make_hint: Some("Kia"),
    },
    PrefixRecord {
        wmi: "JF1",
        manufacturer: "SUBARU CORPORATION",
        make_hint: None,
    },
    PrefixRecord {
        wmi: "4S3",
        manufacturer: "SUBARU OF AMERICA, INC",
        make_hint: Some("Subaru"),
    },
    PrefixRecord {
        wmi: "1G1",
        manufacturer: "GENERAL MOTORS LLC",
        make_hint: Some("Chevrolet"),
    },
    PrefixRecord {
        wmi: "1FT",
        manufacturer: "FORD MOTOR COMPANY",
        make_hint: Some("Ford"),
    },
    PrefixRecord {
        wmi: "5YJ",
        manufacturer: "TESLA, INC.",
        make_hint: Some("Tesla"),
    },
    PrefixRecord {
        wmi: "WP0",
        manufacturer: "DR. ING. H.C.F. PORSCHE AG",
        make_hint: Some("Porsche"),
    },
];

/// Describe only the manufacturer identity indicated by a locally known WMI.
/// No network request is made; model and engine profile are always unresolved.
pub fn describe(vin: &str) -> Value {
    let normalized = vin.to_ascii_uppercase();
    let Ok(vin) = Vin::parse(&normalized) else {
        return json!({
            "vin_format_valid": false,
            "validation": "format_only",
            "status": "invalid_vin",
            "wmi": null,
            "low_volume_manufacturer": false,
            "manufacturer": null,
            "make_hint": null,
            "confidence": "unknown",
            "coverage": "partial",
            "model": null,
            "engine": null,
            "reason": "VIN must contain exactly 17 ASCII VIN characters; the full VIN is not returned.",
            "source": SOURCE_NAME,
            "source_url": SOURCE_DOCS_URL,
        });
    };

    let wmi = &vin.as_str()[..3];
    let low_volume = wmi.as_bytes()[2] == b'9';
    let record = if low_volume {
        None
    } else {
        PREFIXES.iter().find(|record| record.wmi == wmi)
    };

    json!({
        "vin_format_valid": true,
        "validation": "format_only",
        "status": if low_volume { "low_volume_requires_six_character_wmi" } else if record.is_some() { "identified" } else { "unknown_prefix" },
        "wmi": wmi,
        "low_volume_manufacturer": low_volume,
        "manufacturer": record.map(|record| record.manufacturer),
        "make_hint": record.and_then(|record| record.make_hint),
        "confidence": if record.is_some() { "high" } else { "unknown" },
        "coverage": "partial",
        "model": null,
        "engine": null,
        "reason": if low_volume {
            "The third WMI character is 9; resolving a low-volume manufacturer requires a six-character WMI."
        } else if record.is_some() {
            "The local WMI record identifies a manufacturer only; model and engine remain unknown."
        } else {
            "This prefix is absent from the local partial mapping; no manufacturer, model, or engine is inferred."
        },
        "source": SOURCE_NAME,
        "source_url": record.map(|_| format!("https://vpic.nhtsa.dot.gov/api/vehicles/DecodeWMI/{wmi}?format=json")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_case_and_reports_only_a_cautious_make_hint() {
        let result = describe("wvw1a2b3c4d5e6f78");

        assert_eq!(result["vin_format_valid"], true);
        assert_eq!(result["validation"], "format_only");
        assert_eq!(result["wmi"], "WVW");
        assert_eq!(result["manufacturer"], "VOLKSWAGEN AG");
        assert_eq!(result["make_hint"], "Volkswagen");
    }

    #[test]
    fn rejects_malformed_vins_and_disallowed_characters() {
        assert_eq!(describe("WVW1")["status"], "invalid_vin");
        assert_eq!(describe("WVWIOQ12345678901")["vin_format_valid"], false);
    }

    #[test]
    fn unknown_prefix_is_explicit_and_does_not_guess_a_make() {
        let result = describe("ZZZ1A2B3C4D5E6F78");

        assert_eq!(result["status"], "unknown_prefix");
        assert_eq!(result["manufacturer"], Value::Null);
        assert_eq!(result["make_hint"], Value::Null);
        assert_eq!(result["confidence"], "unknown");
    }

    #[test]
    fn low_volume_wmi_is_never_resolved_from_three_characters() {
        let result = describe("AB91A2B3C4D5E6F78");

        assert_eq!(result["status"], "low_volume_requires_six_character_wmi");
        assert_eq!(result["manufacturer"], Value::Null);
        assert_eq!(result["make_hint"], Value::Null);
    }

    #[test]
    fn manufacturer_match_never_validates_a_model_or_engine_profile() {
        let result = describe("WVW1A2B3C4D5E6F78");

        assert_eq!(result["model"], Value::Null);
        assert_eq!(result["engine"], Value::Null);
        assert_eq!(
            result["reason"],
            "The local WMI record identifies a manufacturer only; model and engine remain unknown."
        );
    }
}
