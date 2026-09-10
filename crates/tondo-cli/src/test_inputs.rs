//! Explicit host input admission and value-free worker transport.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use tondo_compiler::test_input_runtime::{self, InputError, MaterializationTarget, WorkerInputs};
use tondo_compiler::test_inputs::{
    TestInputDescriptor, TestInputPlan, TestInputProfile, TestInputVisibility,
};

use crate::SnapshotInputs;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(test, derive(Default))]
#[serde(deny_unknown_fields)]
pub(crate) struct CapturedInputs {
    plan: Vec<u8>,
    test_plan_sha256: String,
    capabilities: BTreeSet<String>,
    public_values: BTreeMap<String, Vec<u8>>,
    max_bytes: u64,
}

fn environment_name(input: &TestInputDescriptor) -> Result<&str, String> {
    let name = input
        .source()
        .strip_prefix("environment:")
        .ok_or_else(|| format!("unsupported runtime source for input `{}`", input.name()))?;
    if input.capability() != Some("environment") || !valid_name(name) {
        return Err(format!(
            "input `{}` requires a valid environment name and capability",
            input.name()
        ));
    }
    if input.visibility() == TestInputVisibility::Secret
        && (input.provider() != Some("environment") || !input.descriptor().is_some_and(valid_name))
    {
        return Err(format!(
            "unsupported provider or descriptor for input `{}`",
            input.name()
        ));
    }
    Ok(name)
}

fn valid_name(name: &str) -> bool {
    !name.is_empty() && !name.bytes().any(|byte| byte == 0 || byte == b'=')
}

fn read_environment(name: &str) -> Option<Vec<u8>> {
    std::env::var_os(name).map(std::ffi::OsString::into_encoded_bytes)
}

impl CapturedInputs {
    pub(crate) fn capture(
        plan: &TestInputPlan,
        snapshots: &SnapshotInputs,
        max_bytes: u64,
    ) -> Result<Self, String> {
        let mut public_values = BTreeMap::new();
        let mut names = BTreeSet::new();
        let mut size = 0_u64;
        for input in plan.inputs() {
            if input.profile() == TestInputProfile::Build {
                continue;
            }
            let bytes = if let Some(store) = snapshots
                .stores
                .iter()
                .find(|store| input.name() == format!("snapshot:{}", store.name))
            {
                Some(
                    store
                        .store
                        .canonical_bytes()
                        .map_err(|error| error.to_string())?,
                )
            } else {
                let name = environment_name(input)?;
                if !names.insert(name.to_owned()) {
                    return Err(format!("runtime environment name `{name}` is duplicated"));
                }
                match input.visibility() {
                    TestInputVisibility::Public => {
                        Some(read_environment(name).ok_or_else(|| {
                            format!("public input `{}` is unavailable", input.name())
                        })?)
                    }
                    TestInputVisibility::Secret => None,
                }
            };
            if let Some(bytes) = bytes {
                size = size
                    .checked_add(bytes.len() as u64)
                    .filter(|size| *size <= max_bytes)
                    .ok_or("public runtime inputs exceed the memory budget")?;
                if input.sha256() != Some(tondo_compiler::artifact::sha256(&bytes).as_str()) {
                    return Err(format!(
                        "public input `{}` has a different SHA-256",
                        input.name()
                    ));
                }
                public_values.insert(input.name().into(), bytes);
            }
        }
        Ok(Self {
            plan: plan.canonical_bytes().map_err(|error| error.to_string())?,
            test_plan_sha256: plan.test_plan_sha256().into(),
            capabilities: plan
                .inputs()
                .iter()
                .filter_map(|input| input.capability().map(str::to_owned))
                .collect(),
            public_values,
            max_bytes,
        })
    }

    /// Only this worker path opens secret providers. Public values were
    /// captured before selection and are checked again against their hashes.
    pub(crate) fn materialize(&self) -> Result<(TestInputPlan, WorkerInputs), String> {
        let plan =
            TestInputPlan::parse_worker(&self.test_plan_sha256, &self.capabilities, &self.plan)
                .map_err(|error| error.to_string())?;
        let remaining = std::cell::Cell::new(self.max_bytes);
        let provider = |input: TestInputDescriptor| {
            let mut bytes = match input.visibility() {
                TestInputVisibility::Public => self.public_values.get(input.name()).cloned(),
                TestInputVisibility::Secret => {
                    environment_name(&input).map_err(|_| InputError::ProviderFailed {
                        name: input.name().into(),
                    })?;
                    read_environment(input.descriptor().unwrap_or_default())
                }
            }
            .ok_or_else(|| InputError::ProviderUnavailable {
                name: input.name().into(),
            })?;
            let Some(budget) = remaining.get().checked_sub(bytes.len() as u64) else {
                bytes.fill(0);
                return Err(InputError::ProviderFailed {
                    name: input.name().into(),
                });
            };
            remaining.set(budget);
            Ok(bytes)
        };
        let values =
            test_input_runtime::materialize(&plan, MaterializationTarget::Runtime, &provider)
                .map_err(|error| error.to_string())?;
        Ok((plan, values))
    }

    pub(crate) fn environment(
        plan: &TestInputPlan,
        values: &WorkerInputs,
    ) -> Result<BTreeMap<Vec<u8>, Vec<u8>>, String> {
        plan.inputs()
            .iter()
            .filter(|input| input.source().starts_with("environment:"))
            .map(|input| {
                Ok((
                    environment_name(input)?.as_bytes().to_vec(),
                    values
                        .get(input.name())
                        .map_err(|error| error.to_string())?
                        .to_vec(),
                ))
            })
            .collect()
    }
}
