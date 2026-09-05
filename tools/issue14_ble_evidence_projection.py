from pathlib import Path


def replace_once(path: str, old: str, new: str) -> None:
    file = Path(path)
    text = file.read_text()
    if text.count(old) != 1:
        raise SystemExit(f"{path}: expected exactly one match, found {text.count(old)}")
    file.write_text(text.replace(old, new, 1))


replace_once(
    "src/ble.rs",
    "pub(crate) use crate::elm::ReadEvidenceError;\npub use crate::elm::ResponseObservation;\n",
    "pub(crate) use crate::elm::{ReadEvidenceError, ResponseObservation};\n",
)

replace_once(
    "src/ble.rs",
    "    /// Execute one already-typed targeted semantic read while retaining this session.\n    /// The outcome preserves every normalized responder observation.\n    pub async fn read_targeted_with_evidence(\n        &self,\n        request: TargetedReadRequest,\n    ) -> Result<ReadOutcome, String> {\n        self.session.read_targeted_with_evidence(request).await\n    }\n",
    "    /// Execute one already-typed targeted semantic read while retaining this session.\n    /// The outcome preserves every normalized responder observation as passive evidence.\n    pub async fn read_targeted_with_evidence(\n        &self,\n        request: TargetedReadRequest,\n    ) -> Result<TargetedReadOutcome, String> {\n        self.session\n            .read_targeted_with_evidence(request)\n            .await\n            .map(TargetedReadOutcome::from_internal)\n    }\n",
)

replace_once(
    "src/ble.rs",
    "#[derive(Debug, PartialEq)]\npub enum ReadOutcome {\n",
    "#[derive(Clone, Debug, PartialEq, Eq)]\npub struct TargetedReadObservation {\n    responses: Vec<crate::capture_events::ResponderEvidence>,\n    selected_responder: Option<String>,\n    selection_error: Option<String>,\n}\n\nimpl TargetedReadObservation {\n    fn from_internal(observation: ResponseObservation) -> Self {\n        Self {\n            responses: observation.responses,\n            selected_responder: observation.selected_responder,\n            selection_error: observation.selection_error,\n        }\n    }\n\n    pub fn responses(&self) -> &[crate::capture_events::ResponderEvidence] {\n        &self.responses\n    }\n\n    pub fn selected_responder(&self) -> Option<&str> {\n        self.selected_responder.as_deref()\n    }\n\n    pub fn selection_error(&self) -> Option<&str> {\n        self.selection_error.as_deref()\n    }\n}\n\n#[derive(Debug, PartialEq)]\npub enum TargetedReadOutcome {\n    Succeeded {\n        transaction: Transaction,\n        observations: Vec<TargetedReadObservation>,\n    },\n    Failed {\n        error: String,\n        observations: Vec<TargetedReadObservation>,\n    },\n}\n\nimpl TargetedReadOutcome {\n    fn from_internal(outcome: ReadOutcome) -> Self {\n        match outcome {\n            ReadOutcome::Succeeded {\n                transaction,\n                observations,\n            } => Self::Succeeded {\n                transaction,\n                observations: observations\n                    .into_iter()\n                    .map(TargetedReadObservation::from_internal)\n                    .collect(),\n            },\n            ReadOutcome::Failed {\n                error,\n                observations,\n            } => Self::Failed {\n                error,\n                observations: observations\n                    .into_iter()\n                    .map(TargetedReadObservation::from_internal)\n                    .collect(),\n            },\n        }\n    }\n}\n\n#[derive(Debug, PartialEq)]\npub(crate) enum ReadOutcome {\n",
)

replace_once(
    "src/main.rs",
    "            Ok(ble::ReadOutcome::Succeeded {\n",
    "            Ok(ble::TargetedReadOutcome::Succeeded {\n",
)
replace_once(
    "src/main.rs",
    "            Ok(ble::ReadOutcome::Failed {\n",
    "            Ok(ble::TargetedReadOutcome::Failed {\n",
)
replace_once(
    "src/main.rs",
    "    observations: &[ble::ResponseObservation],\n",
    "    observations: &[ble::TargetedReadObservation],\n",
)
