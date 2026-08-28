//! Host portability probe for the selected Cranelift backend.
//!
//! This is deliberately smaller than the native evaluation adapter. It does
//! not claim that the Tondo native ABI or full lowering is complete; it only
//! proves that the pinned Cranelift version can select the host ISA and emit a
//! native object for the target runner.

use std::env;
use std::error::Error;

use cranelift_codegen::Context;
use cranelift_codegen::ir::{AbiParam, Function, InstBuilder, Signature, UserFuncName};
use cranelift_codegen::settings::{self, Configurable};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift_module::{Linkage, Module, default_libcall_names};
use cranelift_object::{ObjectBuilder, ObjectModule};
use serde::Serialize;

const CRANELIFT_VERSION: &str = "0.132.3";

#[derive(Debug, Serialize)]
struct ProbeReport {
    format: &'static str,
    status: &'static str,
    backend: &'static str,
    cranelift_version: &'static str,
    target: String,
    architecture: &'static str,
    os: &'static str,
    object_format: &'static str,
    object_bytes: usize,
}

fn main() -> Result<(), Box<dyn Error>> {
    let target = parse_target(env::args().skip(1))?;
    let object = emit_probe_object()?;
    let object_format = detect_object_format(&object).ok_or_else(|| {
        format!(
            "Cranelift emitted an object with an unknown {} host format",
            env::consts::OS
        )
    })?;

    let report = ProbeReport {
        format: "tondo-native-portability-probe/1",
        status: "passed",
        backend: "cranelift",
        cranelift_version: CRANELIFT_VERSION,
        target,
        architecture: env::consts::ARCH,
        os: env::consts::OS,
        object_format,
        object_bytes: object.len(),
    };
    println!("{}", serde_json::to_string(&report)?);
    Ok(())
}

fn parse_target(mut args: impl Iterator<Item = String>) -> Result<String, Box<dyn Error>> {
    let mut target = None;
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--target" => {
                target = Some(args.next().ok_or("--target requires a value")?);
            }
            "--help" | "-h" => {
                println!("usage: cranelift-portability --target TRIPLE");
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument `{other}`").into()),
        }
    }
    target.ok_or_else(|| "--target is required".into())
}

fn emit_probe_object() -> Result<Vec<u8>, Box<dyn Error>> {
    let mut flags = settings::builder();
    flags.set("opt_level", "speed")?;
    let isa_builder = cranelift_native::builder().map_err(|error| error.to_string())?;
    let isa = isa_builder.finish(settings::Flags::new(flags))?;
    let mut module = ObjectModule::new(ObjectBuilder::new(
        isa,
        "tondo-native-portability",
        default_libcall_names(),
    )?);

    let mut signature = Signature::new(module.isa().default_call_conv());
    signature
        .returns
        .push(AbiParam::new(cranelift_codegen::ir::types::I64));
    let function_id =
        module.declare_function("tondo_portability_probe", Linkage::Export, &signature)?;

    let mut function = Function::with_name_signature(UserFuncName::user(0, 0), signature);
    let mut builder_context = FunctionBuilderContext::new();
    {
        let mut builder = FunctionBuilder::new(&mut function, &mut builder_context);
        let entry = builder.create_block();
        builder.switch_to_block(entry);
        let value = builder.ins().iconst(cranelift_codegen::ir::types::I64, 42);
        builder.ins().return_(&[value]);
        builder.seal_block(entry);
        builder.finalize();
    }

    let mut context = Context::for_function(function);
    module.define_function(function_id, &mut context)?;
    module.clear_context(&mut context);
    Ok(module.finish().emit()?)
}

fn detect_object_format(bytes: &[u8]) -> Option<&'static str> {
    match env::consts::OS {
        "linux" => bytes.starts_with(b"\x7fELF").then_some("elf"),
        "macos" => (bytes.starts_with(&[0xcf, 0xfa, 0xed, 0xfe])
            || bytes.starts_with(&[0xfe, 0xed, 0xfa, 0xcf]))
        .then_some("macho"),
        "windows" => {
            (bytes.starts_with(&[0x64, 0x86]) || bytes.starts_with(&[0x64, 0xaa])).then_some("coff")
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::detect_object_format;

    #[test]
    fn recognizes_the_host_object_magic() {
        let bytes = match std::env::consts::OS {
            "linux" => b"\x7fELFprobe".as_slice(),
            "macos" => &[0xcf, 0xfa, 0xed, 0xfe, 0, 0, 0, 0],
            "windows" => &[0x64, 0x86, 0, 0],
            _ => return,
        };
        assert!(detect_object_format(bytes).is_some());
    }

    #[test]
    fn rejects_a_foreign_or_empty_object() {
        assert_eq!(detect_object_format(&[]), None);
        assert_eq!(detect_object_format(b"not-an-object"), None);
    }
}
