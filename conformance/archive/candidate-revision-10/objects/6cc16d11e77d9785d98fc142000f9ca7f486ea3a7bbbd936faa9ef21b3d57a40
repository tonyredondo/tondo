use std::env;
use std::io::{self, BufRead, Write};
use std::process::ExitCode;

use tondo_conformance::protocol::{AdapterRequest, AdapterResponse, AdapterResult};
use tondo_reference_adapter::ReferenceAdapter;

fn main() -> ExitCode {
    if env::args().skip(1).collect::<Vec<_>>() != ["--stdio"] {
        eprintln!("usage: tondo-reference-adapter --stdio");
        return ExitCode::from(2);
    }
    match serve() {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("tondo-reference-adapter: {message}");
            ExitCode::from(1)
        }
    }
}

fn serve() -> Result<(), String> {
    let stdin = io::stdin();
    let mut stdout = io::stdout().lock();
    let mut adapter = ReferenceAdapter;
    for line in stdin.lock().lines() {
        let line = line.map_err(|error| format!("cannot read request: {error}"))?;
        let response = match serde_json::from_str::<AdapterRequest>(&line) {
            Ok(request) => adapter.handle(&request),
            Err(error) => AdapterResponse {
                protocol: tondo_conformance::ADAPTER_PROTOCOL.into(),
                sequence: 0,
                case_id: String::new(),
                result: AdapterResult::Error {
                    message: format!("invalid request JSON: {error}"),
                },
            },
        };
        serde_json::to_writer(&mut stdout, &response)
            .map_err(|error| format!("cannot encode response: {error}"))?;
        stdout
            .write_all(b"\n")
            .and_then(|()| stdout.flush())
            .map_err(|error| format!("cannot write response: {error}"))?;
    }
    Ok(())
}
