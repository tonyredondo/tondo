//! The templates a `template.Set` loads (standard library 14.31): read with
//! the syntax the compiler reads too, and given to `std/template.to` as the
//! JSON of its own types, which it decodes.

use super::template_syntax::{self as syntax, Code, Node, Op, Spec};
use super::{Str, str_bytes, str_new};
use std::fmt::Write;
use std::ops::Range;

/// A template read: `{"extends": ..., "nodes": [...], "error": null, "at":
/// 0}`, or the error and where it is.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_template_parse(source: *const Str, html: u8) -> *mut Str {
    let source = String::from_utf8_lossy(unsafe { str_bytes(source) });
    let mut out = String::new();
    let read = syntax::parse(&source, html != 0).and_then(|template| {
        let mut nodes = String::new();
        list(&mut nodes, &source, &template.nodes)?;
        Ok((template.extends.map(|(name, _)| name), nodes))
    });
    match read {
        Ok((extends, nodes)) => {
            let extends = extends.map_or("null".to_string(), |name| quote(&name));
            let _ = write!(out, "{{\"extends\":{extends},\"nodes\":{nodes},\"error\":null,\"at\":0}}");
        }
        Err(error) => {
            let message = quote(&error.message);
            let _ = write!(out, "{{\"extends\":null,\"nodes\":[],\"error\":{message},\"at\":{}}}", error.at);
        }
    }
    str_new(out.as_bytes())
}

fn list(out: &mut String, source: &str, nodes: &[Node]) -> Result<(), syntax::Error> {
    out.push('[');
    for (index, node) in nodes.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        self::node(out, source, node)?;
    }
    out.push(']');
    Ok(())
}

fn optional(out: &mut String, source: &str, nodes: &Option<Vec<Node>>) -> Result<(), syntax::Error> {
    match nodes {
        Some(nodes) => list(out, source, nodes),
        None => {
            out.push_str("null");
            Ok(())
        }
    }
}

fn node(out: &mut String, source: &str, node: &Node) -> Result<(), syntax::Error> {
    match node {
        Node::Text(text) => {
            let _ = write!(out, "{{\"Text\":{}}}", quote(text));
        }
        Node::Write { value, format, place } => {
            out.push_str("{\"Write\":{\"value\":");
            code(out, source, value)?;
            out.push_str(",\"format\":");
            match format {
                Some(range) => spec(out, &syntax::spec(&source[range.clone()]).unwrap_or(BLANK)),
                None => out.push_str("null"),
            }
            let _ = write!(out, ",\"place\":{}}}}}", *place as u8);
        }
        Node::If { branches, otherwise, .. } => {
            out.push_str("{\"If\":{\"branches\":[");
            for (index, (condition, body)) in branches.iter().enumerate() {
                out.push_str(if index > 0 { ",{\"condition\":" } else { "{\"condition\":" });
                code(out, source, condition)?;
                out.push_str(",\"body\":");
                list(out, source, body)?;
                out.push('}');
            }
            out.push_str("],\"otherwise\":");
            optional(out, source, otherwise)?;
            out.push_str("}}");
        }
        Node::For { pattern, items, body, otherwise, .. } => {
            let names: Vec<String> = syntax::pattern(source, pattern.clone())?.iter().map(|n| quote(n)).collect();
            let _ = write!(out, "{{\"For\":{{\"names\":[{}],\"items\":", names.join(","));
            code(out, source, items)?;
            out.push_str(",\"body\":");
            list(out, source, body)?;
            out.push_str(",\"otherwise\":");
            optional(out, source, otherwise)?;
            out.push_str("}}");
        }
        Node::With { name, value, body, otherwise, .. } => {
            let _ = write!(out, "{{\"With\":{{\"name\":{},\"value\":", quote(&source[name.clone()]));
            code(out, source, value)?;
            out.push_str(",\"body\":");
            list(out, source, body)?;
            out.push_str(",\"otherwise\":");
            optional(out, source, otherwise)?;
            out.push_str("}}");
        }
        Node::Let { name, value } => {
            let _ = write!(out, "{{\"Let\":{{\"name\":{},\"value\":", quote(&source[name.clone()]));
            code(out, source, value)?;
            out.push_str("}}");
        }
        Node::Block { name, body, .. } => {
            let _ = write!(out, "{{\"Block\":{{\"name\":{},\"body\":", quote(name));
            list(out, source, body)?;
            out.push_str("}}");
        }
        Node::Include { at, name, data } => {
            let _ = write!(out, "{{\"Include\":{{\"name\":{},\"data\":", quote(name));
            match data {
                Some(range) => code(out, source, range)?,
                None => out.push_str("null"),
            }
            let _ = write!(out, ",\"at\":{at}}}}}");
        }
        Node::Break => out.push_str("\"Break\""),
        Node::Continue => out.push_str("\"Continue\""),
    }
    Ok(())
}

const BLANK: Spec = Spec {
    fill: ' ',
    align: None,
    plus: false,
    alternate: false,
    zero: false,
    width: None,
    precision: None,
    kind: None,
};

fn spec(out: &mut String, spec: &Spec) {
    let char_or_null = |c: Option<char>| c.map_or("null".to_string(), |c| quote(&c.to_string()));
    let number = |n: Option<u32>| n.map_or("null".to_string(), |n| n.to_string());
    let _ = write!(
        out,
        "{{\"fill\":{},\"align\":{},\"plus\":{},\"alternate\":{},\"zero\":{},\"width\":{},\"precision\":{},\"kind\":{}}}",
        quote(&spec.fill.to_string()),
        char_or_null(spec.align),
        spec.plus,
        spec.alternate,
        spec.zero,
        number(spec.width),
        number(spec.precision),
        char_or_null(spec.kind)
    );
}

fn code(out: &mut String, source: &str, range: &Range<usize>) -> Result<(), syntax::Error> {
    let Code { ops, at } = syntax::expression(source, range.clone())?;
    out.push_str("{\"ops\":[");
    for (index, op) in ops.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        let _ = match op {
            Op::Int(value) => write!(out, "{{\"Int\":{value}}}"),
            Op::Float(value) => write!(out, "{{\"Float\":{value:?}}}"),
            Op::Text(text) => write!(out, "{{\"Text\":{}}}", quote(text)),
            Op::Bool(value) => write!(out, "{{\"Bool\":{value}}}"),
            Op::Name(name) => write!(out, "{{\"Name\":{}}}", quote(name)),
            Op::Field(name) => write!(out, "{{\"Field\":{}}}", quote(name)),
            Op::Index => write!(out, "\"Index\""),
            Op::Call(name, count) => write!(out, "{{\"Call\":[{},{count}]}}", quote(name)),
            Op::Method(name, count) => write!(out, "{{\"Method\":[{},{count}]}}", quote(name)),
            Op::Not => write!(out, "\"Not\""),
            Op::Negate => write!(out, "\"Negate\""),
            Op::Binary(operator) => write!(out, "{{\"Binary\":{}}}", quote(operator)),
            Op::Range(inclusive) => write!(out, "{{\"Range\":{inclusive}}}"),
            Op::And(skip) => write!(out, "{{\"And\":{skip}}}"),
            Op::Or(skip) => write!(out, "{{\"Or\":{skip}}}"),
        };
    }
    let at: Vec<String> = at.iter().map(usize::to_string).collect();
    let _ = write!(out, "],\"at\":[{}]}}", at.join(","));
    Ok(())
}

/// A JSON string.
fn quote(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
