use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::ADAPTER_PROTOCOL;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TargetSelection {
    pub name: String,
    pub profile: String,
    pub capabilities: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WireSource {
    pub source_id: String,
    pub module: String,
    pub logical_path: String,
    pub contents_hex: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WireOperation {
    Format,
    Check,
    Run,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WireSourceForm {
    Module,
    Script,
    Fragment,
    Syntax,
    StandaloneBlock,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WireSourceAction {
    pub operation: WireOperation,
    pub form: WireSourceForm,
    pub root: String,
    pub sources: Vec<WireSource>,
    pub arguments: Vec<String>,
    pub gc_threshold: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WireSemanticAction {
    pub source: WireSourceAction,
    pub queries: Vec<crate::manifest::SemanticQuery>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WireBuildInput {
    pub logical_path: String,
    pub contents_hex: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WireDeterminismAction {
    pub manifest_hex: String,
    pub lockfile_hex: String,
    pub inputs: Vec<WireBuildInput>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DocCategory {
    Syntax,
    Fragment,
    Script,
    CompileFail,
    Pseudocode,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WireDocumentFenceAction {
    pub file: String,
    pub fence_byte: u64,
    pub category: DocCategory,
    pub fixture: Option<String>,
    pub fixture_manifest_hex: String,
    pub fixture_manifest_sha256: String,
    pub expected_codes: Vec<String>,
    pub source_hex: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "kind")]
pub enum AdapterAction {
    Describe,
    Source(WireSourceAction),
    Semantic(WireSemanticAction),
    Memory {
        scenario: crate::manifest::MemoryScenario,
    },
    Determinism(WireDeterminismAction),
    DocumentFence(WireDocumentFenceAction),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdapterRequest {
    pub protocol: String,
    pub sequence: u64,
    pub case_id: String,
    pub target: TargetSelection,
    pub action: AdapterAction,
}

impl AdapterRequest {
    pub fn new(
        sequence: u64,
        case_id: impl Into<String>,
        target: TargetSelection,
        action: AdapterAction,
    ) -> Self {
        Self {
            protocol: ADAPTER_PROTOCOL.into(),
            sequence,
            case_id: case_id.into(),
            target,
            action,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CompilationState {
    Success,
    Rejected,
    NotApplicable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Observation {
    pub compilation: CompilationState,
    pub exit_code: i32,
    pub diagnostics: Vec<Value>,
    pub stdout_hex: String,
    pub stderr_hex: String,
    pub formatted_hex: Option<String>,
    pub data: Value,
}

impl Observation {
    pub fn empty() -> Self {
        Self {
            compilation: CompilationState::NotApplicable,
            exit_code: 0,
            diagnostics: Vec::new(),
            stdout_hex: String::new(),
            stderr_hex: String::new(),
            formatted_hex: None,
            data: Value::Null,
        }
    }

    pub fn diagnostic_codes(&self) -> Result<Vec<&str>, String> {
        self.diagnostics
            .iter()
            .map(|diagnostic| {
                diagnostic
                    .as_object()
                    .and_then(|object| object.get("code"))
                    .and_then(Value::as_str)
                    .ok_or_else(|| "every diagnostic must contain a string `code`".to_owned())
            })
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "status")]
pub enum AdapterResult {
    Ok { observation: Observation },
    Unsupported { reason: String },
    Error { message: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AdapterResponse {
    pub protocol: String,
    pub sequence: u64,
    pub case_id: String,
    #[serde(flatten)]
    pub result: AdapterResult,
}

impl AdapterResponse {
    pub fn success(request: &AdapterRequest, observation: Observation) -> Self {
        Self {
            protocol: ADAPTER_PROTOCOL.into(),
            sequence: request.sequence,
            case_id: request.case_id.clone(),
            result: AdapterResult::Ok { observation },
        }
    }
}

impl<'de> Deserialize<'de> for AdapterResponse {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(rename_all = "kebab-case")]
        enum OkStatus {
            Ok,
        }

        #[derive(Deserialize)]
        #[serde(rename_all = "kebab-case")]
        enum UnsupportedStatus {
            Unsupported,
        }

        #[derive(Deserialize)]
        #[serde(rename_all = "kebab-case")]
        enum ErrorStatus {
            Error,
        }

        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct OkResponse {
            protocol: String,
            sequence: u64,
            case_id: String,
            #[serde(rename = "status")]
            _status: OkStatus,
            observation: Observation,
        }

        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct UnsupportedResponse {
            protocol: String,
            sequence: u64,
            case_id: String,
            #[serde(rename = "status")]
            _status: UnsupportedStatus,
            reason: String,
        }

        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct ErrorResponse {
            protocol: String,
            sequence: u64,
            case_id: String,
            #[serde(rename = "status")]
            _status: ErrorStatus,
            message: String,
        }

        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Response {
            Ok(OkResponse),
            Unsupported(UnsupportedResponse),
            Error(ErrorResponse),
        }

        let (protocol, sequence, case_id, result) = match Response::deserialize(deserializer)? {
            Response::Ok(response) => (
                response.protocol,
                response.sequence,
                response.case_id,
                AdapterResult::Ok {
                    observation: response.observation,
                },
            ),
            Response::Unsupported(response) => (
                response.protocol,
                response.sequence,
                response.case_id,
                AdapterResult::Unsupported {
                    reason: response.reason,
                },
            ),
            Response::Error(response) => (
                response.protocol,
                response.sequence,
                response.case_id,
                AdapterResult::Error {
                    message: response.message,
                },
            ),
        };
        Ok(Self {
            protocol,
            sequence,
            case_id,
            result,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_messages_reject_unknown_fields() {
        let value = serde_json::json!({
            "protocol": ADAPTER_PROTOCOL,
            "sequence": 1,
            "case_id": "case",
            "target": {
                "name": "target",
                "profile": "hosted",
                "capabilities": [],
                "unknown": true
            },
            "action": {"kind": "describe"}
        });
        assert!(serde_json::from_value::<AdapterRequest>(value).is_err());
    }

    #[test]
    fn adapter_responses_round_trip_and_reject_unknown_fields() {
        let response = AdapterResponse::success(
            &AdapterRequest::new(
                7,
                "case",
                TargetSelection {
                    name: "tondo-vm-hosted".into(),
                    profile: "hosted".into(),
                    capabilities: Vec::new(),
                },
                AdapterAction::Describe,
            ),
            Observation::empty(),
        );
        let encoded = serde_json::to_value(&response).unwrap();
        assert_eq!(
            serde_json::from_value::<AdapterResponse>(encoded.clone()).unwrap(),
            response
        );

        let mut unknown = encoded.as_object().unwrap().clone();
        unknown.insert("unknown".into(), Value::Bool(true));
        assert!(serde_json::from_value::<AdapterResponse>(Value::Object(unknown)).is_err());
    }
}
