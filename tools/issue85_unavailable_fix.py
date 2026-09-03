from pathlib import Path


def replace_once(path: str, old: str, new: str) -> None:
    file = Path(path)
    text = file.read_text()
    if text.count(old) != 1:
        raise SystemExit(f"{path}: expected exactly one patch anchor")
    file.write_text(text.replace(old, new, 1))


replace_once(
    "src/ecu_identification.rs",
    '''            IdentificationResultStatus::Unsupported
            | IdentificationResultStatus::NegativeResponse
            | IdentificationResultStatus::Unavailable => {
                if self.nrc.is_none() || self.value.is_some() {
                    return Err(
                        "negative ECU identification evidence requires NRC and no value".into(),
                    );
                }
            }
''',
    '''            IdentificationResultStatus::Unsupported
            | IdentificationResultStatus::NegativeResponse => {
                if self.nrc.is_none() || self.value.is_some() {
                    return Err(
                        "negative ECU identification evidence requires NRC and no value".into(),
                    );
                }
            }
            IdentificationResultStatus::Unavailable => {
                if self.value.is_some()
                    || (self.nrc.is_none() && self.errors.is_empty())
                {
                    return Err(
                        "unavailable ECU identification evidence requires NRC or explicit error and no value".into(),
                    );
                }
            }
''',
)

path = Path("src/ecu_identification_discovery.rs")
text = path.read_text()
anchor = '''    #[test]
    fn timeout_transport_error_and_not_probed_never_collapse() {
'''
test = '''    #[test]
    fn adapter_unavailable_without_nrc_remains_explicit() {
        let plan = plan();
        let f189 = candidate(&plan, 0xf189);
        let ecu = mapping("7E0", "7E8", Confidence::Verified);

        let unavailable = classify_normalized(
            &plan,
            &f189,
            &ecu,
            Vec::new(),
            vec!["ELM327 rejected UDS 22 response: NO DATA".into()],
            false,
            true,
        )
        .unwrap();

        assert_eq!(unavailable.status(), IdentificationResultStatus::Unavailable);
        assert_eq!(unavailable.nrc(), None);
        assert!(!unavailable.errors().is_empty());
    }

'''
if text.count(anchor) != 1:
    raise SystemExit("src/ecu_identification_discovery.rs: test anchor changed")
path.write_text(text.replace(anchor, test + anchor, 1))
