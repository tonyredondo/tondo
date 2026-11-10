//! The MIR as text, to see what the compiler made of a function and what
//! each pass of a release build changed: `TONDO_DUMP_MIR=<name>` writes to
//! standard error every function whose name holds `<name>`, after lowering
//! and after each pass; `TONDO_DUMP_MIR=<name>@<pass>` after that pass
//! alone (`lower`, `inline`, `elide`, `thread`, `lends`, `moves`, `unroll`,
//! `prove`, `stack`). Nothing is done when it is not set.

use crate::ast::BinOp;
use crate::mir::*;
use crate::source::SourceMap;
use crate::types::Types;

/// What `TONDO_DUMP_MIR` asks for: the functions, and the stage.
fn asked() -> Option<&'static (String, Option<String>)> {
    static ASKED: std::sync::OnceLock<Option<(String, Option<String>)>> = std::sync::OnceLock::new();
    ASKED
        .get_or_init(|| {
            let value = std::env::var("TONDO_DUMP_MIR").ok().filter(|value| !value.is_empty())?;
            Some(match value.split_once('@') {
                Some((name, stage)) => (name.to_string(), Some(stage.to_string())),
                None => (value, None),
            })
        })
        .as_ref()
}

/// Whether the MIR of the function `name` is to be written, so that its
/// passes note what they decide.
pub fn noting(name: &str) -> bool {
    asked().is_some_and(|(asked, _)| name.contains(asked.as_str()))
}

/// Notes what `pass` decided of the function and why, when its MIR is to
/// be written: `text` is only made then.
pub fn note(body: &mut Body, pass: &str, text: impl FnOnce() -> String) {
    if asked().is_some_and(|(name, _)| body.name.contains(name.as_str())) {
        body.notes.push(format!("{pass}: {}", text()));
    }
}

/// Writes the bodies `TONDO_DUMP_MIR` asks for as they are after `stage`.
pub fn dump(stage: &str, bodies: &[Body], types: &Types, sources: &SourceMap) {
    let Some((name, only)) = asked() else { return };
    if only.as_deref().is_some_and(|only| only != stage) {
        return;
    }
    for body in bodies.iter().filter(|body| body.name.contains(name.as_str())) {
        eprintln!("== {} after {stage}\n{}", body.name, text(body, types, sources));
    }
}

/// A function's MIR: its locals, then each block's statements and how it
/// ends, each assignment with where it is in the source.
pub fn text(body: &Body, types: &Types, sources: &SourceMap) -> String {
    let mut out = String::new();
    let local = |local: &Local| format!("l{}", local.0);
    let params: Vec<String> = body
        .params
        .iter()
        .enumerate()
        .map(|(index, param)| {
            let loan = body.loans.get(index).copied().unwrap_or(false);
            format!("{}{}", if loan { "var " } else { "" }, local(param))
        })
        .collect();
    out.push_str(&format!("params ({})", params.join(", ")));
    if let Some(ret) = &body.ret {
        out.push_str(&format!(" -> {}", local(ret)));
    }
    out.push('\n');
    for note in &body.notes {
        out.push_str(&format!("  note {note}\n"));
    }
    for (index, ty) in body.locals.iter().enumerate() {
        let name = body.names.get(index).cloned().flatten().map(|name| format!(" `{name}`")).unwrap_or_default();
        let framed = if body.framed.contains(&Local(index as u32)) { " (in the frame)" } else { "" };
        out.push_str(&format!("  l{index}: {}{name}{framed}\n", types.display(*ty)));
    }
    for (index, block) in body.blocks.iter().enumerate() {
        out.push_str(&format!("b{index}:\n"));
        for statement in &block.stmts {
            out.push_str(&format!("    {}\n", statement_text(statement, types, sources)));
        }
        out.push_str(&format!("    {}\n", terminator_text(&block.terminator)));
    }
    out
}

fn statement_text(statement: &Statement, types: &Types, sources: &SourceMap) -> String {
    match statement {
        Statement::Assign(place, rvalue, span) => {
            let at = match span.start == 0 && span.end == 0 {
                true => String::new(),
                false => format!("  @{}", sources.location(*span)),
            };
            format!("{} = {}{at}", place_text(place), rvalue_text(rvalue, types))
        }
        Statement::Retain(place) => format!("retain {}", place_text(place)),
        Statement::Release(place) => format!("release {}", place_text(place)),
        Statement::DeferPush(body, place) => format!("defer push fn{} {}", body.0, place_text(place)),
        Statement::DeferPop => "defer pop".to_string(),
        Statement::Lend(local, place) => format!("l{} = lend {}", local.0, place_text(place)),
        Statement::Clear(local) => format!("clear l{}", local.0),
        Statement::Moved(place) => format!("moved {}", place_text(place)),
        Statement::LendAt(local, guard) => format!("l{} = lend the value of {}", local.0, operand_text(guard)),
        Statement::OsrLoad(point) => format!("load the state of point {point}"),
        Statement::Failing(operand, ..) => format!("failing {}", operand_text(operand)),
    }
}

fn terminator_text(terminator: &Terminator) -> String {
    match terminator {
        Terminator::Goto(to) => format!("goto b{}", to.0),
        Terminator::Branch(condition, then, otherwise) => {
            format!("branch {} ? b{} : b{}", operand_text(condition), then.0, otherwise.0)
        }
        Terminator::Return(operand) => format!("return {}", operand_text(operand)),
        Terminator::Panic { code, message, .. } => match message {
            Some(message) => format!("panic P{code:04} {}", operand_text(message)),
            None => format!("panic P{code:04}"),
        },
        Terminator::Unreachable => "unreachable".to_string(),
    }
}

pub fn place_text(place: &Place) -> String {
    let mut out = format!("l{}", place.local.0);
    for projection in &place.path {
        match projection {
            Proj::Field(field) => out.push_str(&format!(".{field}")),
            Proj::Variant(variant, field) => out.push_str(&format!(".v{variant}.{field}")),
            Proj::Index(index, _) => out.push_str(&format!("[l{}]", index.0)),
            Proj::InBounds(index) => out.push_str(&format!("[l{} in bounds]", index.0)),
            Proj::Deref => out.push_str(".*"),
            Proj::Captures(_) => out.push_str(".captures"),
        }
    }
    out
}

pub fn operand_text(operand: &Operand) -> String {
    match operand {
        Operand::Copy(place) => place_text(place),
        Operand::Const(constant) => match constant {
            Const::Int(value, _) => value.to_string(),
            Const::Float(bits, _) => f64::from_bits(*bits).to_string(),
            Const::Char(c) => format!("{c:?}"),
            Const::Bool(b) => b.to_string(),
            Const::Str(text) => format!("{text:?}"),
            Const::Bytes(bytes, _) => format!("<{} bytes>", bytes.len()),
            Const::Unit => "()".to_string(),
        },
    }
}

fn list(operands: &[Operand]) -> String {
    operands.iter().map(operand_text).collect::<Vec<_>>().join(", ")
}

fn rvalue_text(rvalue: &Rvalue, types: &Types) -> String {
    match rvalue {
        Rvalue::Use(operand) => operand_text(operand),
        Rvalue::Unary(op, operand) => format!("{op:?} {}", operand_text(operand)),
        Rvalue::Binary(op, a, b) => {
            let checked = matches!(op, BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Rem | BinOp::Shl);
            let checked = if checked { "  (checked)" } else { "" };
            format!("{} {} {}{checked}", operand_text(a), op.as_str(), operand_text(b))
        }
        Rvalue::Exact(op, a, b) => format!("{} {} {}", operand_text(a), op.as_str(), operand_text(b)),
        Rvalue::Increment(operand) => format!("{} + 1", operand_text(operand)),
        Rvalue::Call(callee, operands) => format!("call {callee:?}({})", list(operands)),
        Rvalue::CallIndirect(callee, operands) => format!("call {}({})", operand_text(callee), list(operands)),
        Rvalue::FnValue(id) => format!("fn{}", id.0),
        Rvalue::Erase(env, id, retain) => {
            format!("closure fn{} of {}{}", id.0, operand_text(env), if *retain { " (retained)" } else { "" })
        }
        Rvalue::Spawn(body, _) => format!("spawn {}", operand_text(body)),
        Rvalue::TaskOp(op, operands, ..) => format!("{op:?}({})", list(operands)),
        Rvalue::Concat(pieces) => {
            let pieces: Vec<String> = pieces
                .iter()
                .map(|piece| match piece {
                    Piece::Text(text) => format!("{text:?}"),
                    Piece::Value(operand, ty) => format!("{{{}: {}}}", operand_text(operand), types.display(*ty)),
                    Piece::Taken(place) => format!("{{{} taken}}", place_text(place)),
                })
                .collect();
            format!("concat {}", pieces.join(" "))
        }
        Rvalue::Aggregate(operands) => format!("({})", list(operands)),
        Rvalue::Variant(variant, operands) => format!("variant {variant}({})", list(operands)),
        Rvalue::Discriminant(place) => format!("tag of {}", place_text(place)),
        Rvalue::Convert(operand) => format!("convert {}", operand_text(operand)),
        Rvalue::Array(operands, _) => format!("[{}]", list(operands)),
        Rvalue::ArrayOp(op, operands, _) => format!("{op:?}({})", list(operands)),
        Rvalue::Length(operand) => format!("length of {}", operand_text(operand)),
        Rvalue::StrOp(op, operands, _) => format!("{op:?}({})", list(operands)),
        Rvalue::MapOp(op, operands, _) => format!("{op:?}({})", list(operands)),
        Rvalue::Mutate(mutation, place, operands, ..) => {
            format!("{mutation:?} {}({})", place_text(place), list(operands))
        }
        // The rest by name, with the locals they read.
        other => {
            let name = match other {
                Rvalue::ExternFn(_) => "extern fn",
                Rvalue::Callback(..) => "callback",
                Rvalue::Layout(..) => "layout",
                Rvalue::Reflect(..) => "reflect",
                Rvalue::Fixed(..) => "fixed",
                Rvalue::TestMembers(_) => "test members",
                Rvalue::Select(..) => "select",
                Rvalue::Cancelled => "cancelled",
                Rvalue::Unwinding => "unwinding",
                Rvalue::Restart(_) => "restart",
                Rvalue::Transfer(_) => "transfer",
                Rvalue::CallOsr(..) => "call for a live frame",
                Rvalue::OsrPoint => "live point",
                Rvalue::Slice(..) => "slice",
                Rvalue::AssignSlice(..) => "assign slice",
                Rvalue::CheckRegion(..) => "check region",
                Rvalue::Pointer(..) => "pointer",
                _ => "rvalue",
            };
            let mut read = Vec::new();
            rvalue_uses(other, &mut read);
            let read: Vec<String> = read.iter().map(|local| format!("l{}", local.0)).collect();
            match read.is_empty() {
                true => name.to_string(),
                false => format!("{name} reading {}", read.join(", ")),
            }
        }
    }
}
