//! Public-boundary observations used by generated reliability tests.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tondo_compiler::driver::{
    BuildTarget, CompilationRequest, CompilationStatus, DiagnosticFormat, Edition, HostProfile,
    Operation, ResourceLimits, SourceForm, execute,
};
use tondo_compiler::package::PackageGraph;
use tondo_compiler::source::{LogicalPath, ModulePath, SourceDatabase, SourceId, SourceInput};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Observation {
    pub accepted: bool,
    pub exit_code: u8,
    pub diagnostic_codes: Vec<String>,
    pub diagnostics_jsonl: String,
    pub stdout_hex: String,
}

pub fn observe(
    name: &str,
    source: impl Into<Arc<[u8]>>,
    operation: Operation,
    source_form: SourceForm,
    limits: ResourceLimits,
) -> Result<Observation, String> {
    let mut sources = SourceDatabase::new();
    let root = sources
        .add(SourceInput::virtual_file(
            SourceId::new(format!("reliability:{name}")).map_err(|error| error.to_string())?,
            ModulePath::new("reliability").map_err(|error| error.to_string())?,
            LogicalPath::new(format!("reliability/{name}.to"))
                .map_err(|error| error.to_string())?,
            source,
        ))
        .map_err(|error| error.to_string())?;
    let packages = PackageGraph::loose(&sources, root).map_err(|error| error.to_string())?;
    let request = CompilationRequest::new(
        operation,
        Edition::V0_1,
        BuildTarget::vm_hosted(),
        HostProfile::Hosted,
        BuildTarget::vm_hosted_capabilities(),
        DiagnosticFormat::Json,
        source_form,
        limits,
        packages,
        sources,
        root,
    )
    .map_err(|error| error.to_string())?;
    let output = execute(request).map_err(|error| error.to_string())?;
    let codes = output
        .diagnostics()
        .diagnostics()
        .iter()
        .map(|diagnostic| diagnostic.code().to_owned())
        .collect::<Vec<_>>();
    Ok(Observation {
        accepted: output.status() == CompilationStatus::Success,
        exit_code: output.exit_code(),
        diagnostic_codes: codes,
        diagnostics_jsonl: output
            .diagnostics()
            .json_lines()
            .map_err(|error| error.to_string())?,
        stdout_hex: encode_hex(output.stdout()),
    })
}

pub fn check(name: &str, source: &str) -> Result<Observation, String> {
    observe(
        name,
        Arc::<[u8]>::from(source.as_bytes()),
        Operation::Check,
        SourceForm::Module,
        ResourceLimits::default(),
    )
}

pub fn run(name: &str, source: &str, limits: ResourceLimits) -> Result<Observation, String> {
    observe(
        name,
        Arc::<[u8]>::from(source.as_bytes()),
        Operation::Run,
        SourceForm::Script,
        limits,
    )
}

pub fn format(name: &str, source: &[u8], source_form: SourceForm) -> Result<Observation, String> {
    observe(
        name,
        Arc::<[u8]>::from(source),
        Operation::Format,
        source_form,
        ResourceLimits::default(),
    )
}

pub fn decode_hex(value: &str) -> Result<Vec<u8>, String> {
    if !value.len().is_multiple_of(2) || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("hex input must contain complete ASCII byte pairs".into());
    }
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let high = nibble(pair[0])?;
            let low = nibble(pair[1])?;
            Ok((high << 4) | low)
        })
        .collect()
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut result = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        result.push(HEX[(byte >> 4) as usize] as char);
        result.push(HEX[(byte & 0x0f) as usize] as char);
    }
    result
}

fn nibble(value: u8) -> Result<u8, String> {
    match value {
        b'0'..=b'9' => Ok(value - b'0'),
        b'a'..=b'f' => Ok(value - b'a' + 10),
        b'A'..=b'F' => Ok(value - b'A' + 10),
        _ => Err("invalid hexadecimal digit".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn observations_are_stable_and_detached_from_paths() {
        let first = check("stable", "fn answer(): Int { 42 }\n").unwrap();
        let second = check("stable", "fn answer(): Int { 42 }\n").unwrap();
        assert_eq!(first, second);
        assert!(first.accepted);
        assert_eq!(first.exit_code, 0);
        assert!(first.diagnostics_jsonl.is_empty());
        assert!(first.stdout_hex.is_empty());
    }

    #[test]
    fn spawned_async_collect_uses_the_default_compiler_vm_instance() {
        let source = "import std.async\n\
type Counter = { remaining: Int }\n\
impl AsyncIterator[Int] for Counter {\n\
    async fn next(mut self): Int? {\n\
        await tick()\n\
        if self.remaining == 0 {\n\
            return none\n\
        }\n\
        let current = self.remaining\n\
        self.remaining -= 1\n\
        some(current)\n\
    }\n\
}\n\
async fn tick() {}\n\
fn main() {\n\
    scope {\n\
        let pending = spawn Counter { remaining: 3 }.collect(limit: 2)\n\
        let result = await pending\n\
        match result {\n\
            ok(values) => assert(values == [3, 2])\n\
            err(_) => panic(\"spawn collect failed\")\n\
        }\n\
    }\n\
}\n";
        let observation = run("async-collect", source, ResourceLimits::default()).unwrap();
        assert!(observation.accepted);
        assert_eq!(observation.exit_code, 0);
        assert!(observation.diagnostics_jsonl.is_empty());
        assert!(observation.stdout_hex.is_empty());
    }

    #[test]
    fn hex_codec_round_trips_binary_output() {
        let bytes = [0, 1, 15, 16, 127, 128, 255];
        assert_eq!(decode_hex(&encode_hex(&bytes)).unwrap(), bytes);
        assert_eq!(decode_hex("aBcD").unwrap(), [0xab, 0xcd]);
        assert!(decode_hex("0").is_err());
        assert!(decode_hex("gg").is_err());
    }
}
