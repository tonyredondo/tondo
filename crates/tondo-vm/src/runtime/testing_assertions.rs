use std::fmt::{self, Write as _};

use super::*;

const TRUNCATION_MARKER: &str = "...<truncated>";
const CURSOR_BYTES: u64 = 64;

struct DiagnosticText {
    text: String,
    truncated: bool,
    _memory: Option<VmMemoryCharge>,
}

impl fmt::Write for DiagnosticText {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        if self.truncated {
            return Err(fmt::Error);
        }
        let remaining = super::super::TEST_ASSERTION_VALUE_BYTES - self.text.len();
        let end = text.floor_char_boundary(remaining.min(text.len()));
        self.text.push_str(&text[..end]);
        self.truncated = end != text.len();
        if self.truncated {
            Err(fmt::Error)
        } else {
            Ok(())
        }
    }
}

enum DisplayCursor {
    Value(BytecodeTypeId, Value),
    Array(BytecodeTypeId, HeapHandle, usize),
}

impl Engine<'_, '_> {
    pub(super) fn prepare_testing_assertion(
        &mut self,
        metadata: &BytecodeCallable,
        values: &[Value],
    ) -> Result<OperationResult, VmError> {
        let display = metadata
            .assertion_display
            .ok_or_else(|| VmError::invariant("assertion has no Display dispatch"))?;
        let name = metadata.name.split('[').next().unwrap_or(&metadata.name);
        let (prefix, separator, suffix, first, second, owned) = match (name, values) {
            ("std.testing.assertEqual" | "std.testing.assertNotEqual", [expected, actual]) => {
                let equal = self.value_equal(expected, actual)?;
                if equal == (name == "std.testing.assertEqual") {
                    return Ok(OperationResult::Value(Value::Unit));
                }
                if name == "std.testing.assertEqual" {
                    (
                        "assertion failed: expected ",
                        ", actual ",
                        "",
                        expected.clone(),
                        Some(actual.clone()),
                        None,
                    )
                } else {
                    (
                        "assertion failed: values are equal (",
                        "",
                        ")",
                        expected.clone(),
                        None,
                        None,
                    )
                }
            }
            ("std.testing.assertOk" | "std.testing.assertErr", [result]) => {
                let Value::Heap(handle) = result else {
                    return Err(VmError::invariant("assertion input is not a Result"));
                };
                let (ok, value) = match self.heap.get(*handle)? {
                    HeapObject::ResultOk(value) => {
                        (true, present(value, "assertion success payload")?.clone())
                    }
                    HeapObject::ResultErr(value) => {
                        (false, present(value, "assertion error payload")?.clone())
                    }
                    _ => {
                        return Err(VmError::invariant(
                            "assertion input has a non-Result object",
                        ));
                    }
                };
                if ok == (name == "std.testing.assertOk") {
                    // The argument was consumed by the call. Move its payload;
                    // unwrapping must not require Copy or duplicate a resource.
                    self.replace_object(
                        *handle,
                        if ok {
                            HeapObject::ResultOk(None)
                        } else {
                            HeapObject::ResultErr(None)
                        },
                        std::slice::from_ref(&value),
                    )?;
                    return Ok(OperationResult::Value(value));
                }
                if ok {
                    (
                        "assertion failed: expected Err, got Ok(",
                        "",
                        ")",
                        value,
                        None,
                        Some(result.clone()),
                    )
                } else {
                    (
                        "assertion failed: expected Ok, got Err(",
                        "",
                        ")",
                        value,
                        None,
                        Some(result.clone()),
                    )
                }
            }
            _ => {
                return Err(VmError::invariant(
                    "assertion callable has an invalid argument list",
                ));
            }
        };
        let result = (|| {
            let first = match self.testing_display_text(display, first)? {
                Ok(text) => text,
                Err(panic) => return Ok(OperationResult::Panic(panic.0, panic.1)),
            };
            let second = match second {
                Some(value) => match self.testing_display_text(display, value)? {
                    Ok(text) => Some(text),
                    Err(panic) => return Ok(OperationResult::Panic(panic.0, panic.1)),
                },
                None => None,
            };
            let second_text = second.as_ref().map_or("", |text| text.text.as_str());
            let length = prefix.len()
                + first.text.len()
                + separator.len()
                + second_text.len()
                + suffix.len();
            let _memory = self.reserve_scheduler_memory(
                super::super::TEST_DETACHED_VALUE_BYTES + length as u64,
                values,
            )?;
            let mut message = String::with_capacity(length);
            for part in [prefix, &first.text, separator, second_text, suffix] {
                message.push_str(part);
            }
            Ok(OperationResult::Panic(PanicCode::AssertionFailed, message))
        })();
        if let Some(owned) = owned {
            self.execute_terminal_value(metadata.parameters[0].ty, owned)?;
        }
        result
    }

    fn testing_display_text(
        &mut self,
        display: crate::bytecode::BytecodeAssertionDisplay,
        value: Value,
    ) -> Result<Result<DiagnosticText, (PanicCode, String)>, VmError> {
        self.retain_temporary(&value);
        let value = if let Some(callable) = display.callable {
            match self.invoke_sync_value(
                Value::Function {
                    callable,
                    arguments: Vec::new(),
                },
                vec![(BytecodeCallArgumentTarget::Receiver, value)],
            )? {
                Ok(value) => value,
                Err(panic) => return Ok(Err(panic)),
            }
        } else {
            value
        };
        self.retain_temporary(&value);
        let capacity = super::super::TEST_ASSERTION_VALUE_BYTES + TRUNCATION_MARKER.len();
        let memory = self.reserve_scheduler_memory(
            super::super::TEST_DETACHED_VALUE_BYTES + capacity as u64,
            std::slice::from_ref(&value),
        )?;
        let mut output = DiagnosticText {
            text: String::with_capacity(capacity),
            truncated: false,
            _memory: memory,
        };
        if display.callable.is_some() {
            let result = output.write_str(self.string_value(&value)?);
            if result.is_err() && !output.truncated {
                return Err(VmError::invariant(
                    "assertion Display output could not be formatted",
                ));
            }
        } else {
            self.write_intrinsic_testing_display(display.value_type, value, &mut output)?;
        }
        if output.truncated {
            output.text.push_str(TRUNCATION_MARKER);
        }
        Ok(Ok(output))
    }

    /// Walk arrays one child at a time. The pending stack is admitted before
    /// growth and formatting stops at the prefix, without snapshotting values.
    fn write_intrinsic_testing_display(
        &mut self,
        ty: BytecodeTypeId,
        value: Value,
        output: &mut DiagnosticText,
    ) -> Result<(), VmError> {
        let mut pending = Vec::new();
        let mut memory = self.reserve_scheduler_memory(0, &[])?;
        self.push_display_cursor(&mut pending, &mut memory, DisplayCursor::Value(ty, value))?;
        while let Some(cursor) = pending.pop() {
            if output.truncated {
                break;
            }
            let formatted = match cursor {
                DisplayCursor::Array(element, handle, index) => {
                    let HeapObject::Array(values) = self.heap.get(handle)? else {
                        return Err(VmError::invariant("Display Array changed shape"));
                    };
                    if index == values.len() {
                        output.write_str("]")
                    } else {
                        let value = present(&values[index], "Display Array element")?.clone();
                        if index != 0 && output.write_str(", ").is_err() {
                            break;
                        }
                        self.push_display_cursor(
                            &mut pending,
                            &mut memory,
                            DisplayCursor::Array(element, handle, index + 1),
                        )?;
                        self.push_display_cursor(
                            &mut pending,
                            &mut memory,
                            DisplayCursor::Value(element, value),
                        )?;
                        Ok(())
                    }
                }
                DisplayCursor::Value(mut ty, value) => {
                    let mut remaining = self.program.types.len();
                    while let Some(BytecodeTypeKind::OpaqueResult { witness, .. }) =
                        self.program.ty(ty).map(|ty| &ty.kind)
                    {
                        if remaining == 0 {
                            return Err(VmError::invariant(
                                "Display contains an opaque type cycle",
                            ));
                        }
                        remaining -= 1;
                        ty = *witness;
                    }
                    match self.program.ty(ty).map(|ty| &ty.kind) {
                        Some(BytecodeTypeKind::Scalar(BytecodeScalarType::String)) => {
                            output.write_str(self.string_value(&value)?)
                        }
                        Some(BytecodeTypeKind::Scalar(scalar)) => {
                            write_scalar_display(*scalar, &value, output)?
                        }
                        Some(BytecodeTypeKind::Intrinsic {
                            constructor: BytecodeIntrinsicType::Array,
                            arguments,
                        }) => {
                            let element = arguments[0];
                            let Value::Heap(handle) = value else {
                                return Err(VmError::invariant("Display Array is not managed"));
                            };
                            if output.write_str("[").is_err() {
                                break;
                            }
                            self.push_display_cursor(
                                &mut pending,
                                &mut memory,
                                DisplayCursor::Array(element, handle, 0),
                            )?;
                            Ok(())
                        }
                        // Standard error names have a closed bounded schema.
                        // Scalars and collections stream into the prefix above.
                        _ => {
                            let _scalar_memory = self.reserve_scheduler_memory(128, &[])?;
                            output.write_str(&self.display_text(ty, value)?)
                        }
                    }
                }
            };
            if formatted.is_err() && !output.truncated {
                return Err(VmError::invariant("intrinsic assertion Display failed"));
            }
        }
        Ok(())
    }

    fn push_display_cursor(
        &mut self,
        pending: &mut Vec<DisplayCursor>,
        memory: &mut Option<VmMemoryCharge>,
        cursor: DisplayCursor,
    ) -> Result<(), VmError> {
        if pending.len() == pending.capacity() {
            let count = pending.len().checked_add(1).ok_or(VmError::ResourceLimit {
                resource: "memory",
                limit: self.limits.max_heap_bytes,
            })?;
            if let Some(memory) = memory {
                memory.resize((count as u64).checked_mul(CURSOR_BYTES).ok_or(
                    VmError::ResourceLimit {
                        resource: "memory",
                        limit: memory.budget().limit(),
                    },
                )?)?;
            }
            pending
                .try_reserve_exact(1)
                .map_err(|_| VmError::ResourceLimit {
                    resource: "memory",
                    limit: self.limits.max_heap_bytes,
                })?;
        }
        pending.push(cursor);
        Ok(())
    }
}
