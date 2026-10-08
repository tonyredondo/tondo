//! Typed pure civil values admitted before publication to the hosted VM.
//! No provider access or host handle is involved.

use super::*;
use tondo_stdlib::civil_time::{
    CivilError, CivilOperation as Operation, Date, MonthPolicy, Time, UtcDateTime,
};

// The largest result is ResultOk(UtcDateTime(Date, Time)): eight detached
// values, with fixed names. The longest canonical string has thirty bytes.
// This is a conservative logical construction envelope, not RSS or mallocs.
const CIVIL_REPLY_BYTES: u64 = 8 * tondo_vm::runtime::TEST_DETACHED_VALUE_BYTES + 30;

fn civil_int(value: &RuntimeValue) -> Result<i64, VmError> {
    match value {
        RuntimeValue::Integer(number) => i64::try_from(*number)
            .map_err(|_| VmError::Host("civil value exceeds the Int domain".into())),
        _ => Err(VmError::Host("civil operation expects an Int".into())),
    }
}

fn date_value(date: Date) -> RuntimeValue {
    RuntimeValue::Record {
        name: "Date".into(),
        values: vec![
            RuntimeValue::Integer(date.year().into()),
            RuntimeValue::Integer(date.month().into()),
            RuntimeValue::Integer(date.day().into()),
        ],
    }
}

fn date_input(value: &RuntimeValue) -> Result<Date, VmError> {
    let RuntimeValue::Record { name, values } = value else {
        return Err(VmError::Host("invalid Date shape".into()));
    };
    let [year, month, day] = values.as_slice() else {
        return Err(VmError::Host("invalid Date shape".into()));
    };
    if name != "Date" {
        return Err(VmError::Host("invalid Date identity".into()));
    }
    Date::create(civil_int(year)?, civil_int(month)?, civil_int(day)?)
        .map_err(|_| VmError::Host("invalid Date value".into()))
}

fn time_value(time: Time) -> RuntimeValue {
    let nanos = ((time.hour() * 3600 + time.minute() * 60 + time.second()) * 1_000_000_000)
        + time.nanosecond();
    RuntimeValue::Record {
        name: "Time".into(),
        values: vec![RuntimeValue::Integer(nanos.into())],
    }
}

fn time_input(value: &RuntimeValue) -> Result<Time, VmError> {
    let RuntimeValue::Record { name, values } = value else {
        return Err(VmError::Host("invalid Time shape".into()));
    };
    let [nanos] = values.as_slice() else {
        return Err(VmError::Host("invalid Time shape".into()));
    };
    if name != "Time" {
        return Err(VmError::Host("invalid Time identity".into()));
    }
    let nanos = civil_int(nanos)?;
    if !(0..86_400_000_000_000).contains(&nanos) {
        return Err(VmError::Host("invalid Time value".into()));
    }
    Time::create(
        nanos / 3_600_000_000_000,
        nanos / 60_000_000_000 % 60,
        nanos / 1_000_000_000 % 60,
        nanos % 1_000_000_000,
    )
    .map_err(|_| VmError::Host("invalid Time value".into()))
}

fn utc_value(utc: UtcDateTime) -> RuntimeValue {
    RuntimeValue::Record {
        name: "UtcDateTime".into(),
        values: vec![date_value(utc.date()), time_value(utc.time())],
    }
}

pub(super) fn utc_input(value: &RuntimeValue) -> Result<UtcDateTime, VmError> {
    let RuntimeValue::Record { name, values } = value else {
        return Err(VmError::Host("invalid UtcDateTime shape".into()));
    };
    let [date, time] = values.as_slice() else {
        return Err(VmError::Host("invalid UtcDateTime shape".into()));
    };
    if name != "UtcDateTime" {
        return Err(VmError::Host("invalid UtcDateTime identity".into()));
    }
    Ok(UtcDateTime::create(date_input(date)?, time_input(time)?))
}

fn month_policy(value: &RuntimeValue) -> Result<MonthPolicy, VmError> {
    let RuntimeValue::Variant {
        name,
        variant,
        values,
    } = value
    else {
        return Err(VmError::Host("invalid MonthPolicy shape".into()));
    };
    if name != "MonthPolicy" || !values.is_empty() {
        return Err(VmError::Host("invalid MonthPolicy identity".into()));
    }
    match variant {
        0 => Ok(MonthPolicy::Reject),
        1 => Ok(MonthPolicy::Clamp),
        _ => Err(VmError::Host("invalid MonthPolicy tag".into())),
    }
}

fn civil_error(error: CivilError) -> RuntimeValue {
    RuntimeValue::Variant {
        name: "CivilError".into(),
        variant: error as u32,
        values: Vec::new(),
    }
}

fn civil_result<T>(result: Result<T, CivilError>, encode: fn(T) -> RuntimeValue) -> RuntimeValue {
    match result {
        Ok(value) => RuntimeValue::ResultOk(Box::new(encode(value))),
        Err(error) => RuntimeValue::ResultErr(Box::new(civil_error(error))),
    }
}

impl BootstrapHost {
    pub(super) fn invoke_civil_time_admitted(
        &mut self,
        name: &str,
        arguments: &[RuntimeValue],
        response: &mut VmHostReturnBudget<'_>,
        admission: &mut VmHostImportAdmission<'_>,
    ) -> Result<RuntimeValue, VmError> {
        let operation = Operation::from_name(name)
            .ok_or_else(|| VmError::Host("unknown civil core operation".into()))?;
        response.reserve(CIVIL_REPLY_BYTES, [])?;
        let value = match (operation, arguments) {
            (Operation::DateCreate, [year, month, day]) => civil_result(
                Date::create(civil_int(year)?, civil_int(month)?, civil_int(day)?),
                date_value,
            ),
            (Operation::DateParse, [RuntimeValue::String(text)]) => {
                civil_result(Date::parse(text), date_value)
            }
            (Operation::DateYear, [date]) => RuntimeValue::Integer(date_input(date)?.year().into()),
            (Operation::DateMonth, [date]) => {
                RuntimeValue::Integer(date_input(date)?.month().into())
            }
            (Operation::DateDay, [date]) => RuntimeValue::Integer(date_input(date)?.day().into()),
            (Operation::DateDayOfWeek, [date]) => {
                RuntimeValue::Integer(date_input(date)?.day_of_week().into())
            }
            (Operation::DateDayOfYear, [date]) => {
                RuntimeValue::Integer(date_input(date)?.day_of_year().into())
            }
            (Operation::DateAddDays, [date, amount]) => {
                civil_result(date_input(date)?.add_days(civil_int(amount)?), date_value)
            }
            (Operation::DateAddMonths, [date, amount, policy]) => civil_result(
                date_input(date)?.add_months(civil_int(amount)?, month_policy(policy)?),
                date_value,
            ),
            (Operation::DateAddYears, [date, amount, policy]) => civil_result(
                date_input(date)?.add_years(civil_int(amount)?, month_policy(policy)?),
                date_value,
            ),
            (Operation::DateFormat, [date]) => RuntimeValue::String(date_input(date)?.format()),
            (Operation::TimeCreate, [hour, minute, second, nanos]) => civil_result(
                Time::create(
                    civil_int(hour)?,
                    civil_int(minute)?,
                    civil_int(second)?,
                    civil_int(nanos)?,
                ),
                time_value,
            ),
            (Operation::TimeParse, [RuntimeValue::String(text)]) => {
                civil_result(Time::parse(text), time_value)
            }
            (Operation::TimeHour, [time]) => RuntimeValue::Integer(time_input(time)?.hour().into()),
            (Operation::TimeMinute, [time]) => {
                RuntimeValue::Integer(time_input(time)?.minute().into())
            }
            (Operation::TimeSecond, [time]) => {
                RuntimeValue::Integer(time_input(time)?.second().into())
            }
            (Operation::TimeNanosecond, [time]) => {
                RuntimeValue::Integer(time_input(time)?.nanosecond().into())
            }
            (Operation::TimeFormat, [time]) => RuntimeValue::String(time_input(time)?.format()),
            (Operation::UtcCreate, [date, time]) => {
                utc_value(UtcDateTime::create(date_input(date)?, time_input(time)?))
            }
            (Operation::UtcParse, [RuntimeValue::String(text)]) => {
                civil_result(UtcDateTime::parse(text), utc_value)
            }
            (Operation::UtcDate, [utc]) => date_value(utc_input(utc)?.date()),
            (Operation::UtcTime, [utc]) => time_value(utc_input(utc)?.time()),
            (Operation::UtcAdd, [utc, amount]) => {
                civil_result(utc_input(utc)?.checked_add(civil_int(amount)?), utc_value)
            }
            (Operation::UtcFormat, [utc]) => RuntimeValue::String(utc_input(utc)?.format()),
            _ => return Err(VmError::Host("invalid civil core call arguments".into())),
        };
        response.shrink(
            value
                .retained_bytes()
                .ok_or_else(|| VmError::Invariant("civil reply storage overflows".into()))?,
        )?;
        // Pure value calculation has no provider effects. Construction is
        // preadmitted above; the exact result must also fit the receiving VM
        // before it can be published. A refusal drops both temporary value
        // and transport charge without creating a host handle.
        let imported = admission.prepare_current(VmHostReturnPreview::Value(&value))?;
        admission.commit(&mut [imported])?;
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_production_vm_refuses_small_heaps_and_recovers_without_handles() {
        use crate::driver::Operation;
        use tondo_vm::runtime::{VmLimits, execute_with_limits};
        let source = "import std.time\nfn main(): !time.CivilError {\n\
            let utc=time.UtcDateTime.parse(\"9999-12-31T23:59:59.999999999Z\")?\n\
            assert(utc.format()==\"9999-12-31T23:59:59.999999999Z\")\n}\n";
        let (program, entry) =
            compile_host_admission_source(source, BTreeSet::new(), Operation::Run);
        let mut refused = 0;
        let mut succeeded = 0;
        for limits in (1..=4097)
            .step_by(256)
            .map(|max_heap_bytes| VmLimits {
                max_heap_bytes,
                ..VmLimits::default()
            })
            .chain((1..=16).map(|max_heap_objects| VmLimits {
                max_heap_bytes: 16_384,
                max_heap_objects,
                ..VmLimits::default()
            }))
            .chain(std::iter::once(VmLimits::default()))
        {
            let mut host = BootstrapHost::default();
            match execute_with_limits(&program, entry, &mut host, limits) {
                Ok(_) => succeeded += 1,
                Err(VmError::ResourceLimit { .. } | VmError::OutOfMemory { .. }) => refused += 1,
                Err(error) => panic!("{limits:?}: {error:?}"),
            }
            assert!(host.values.is_empty());
            assert!(host.buffer_memory.is_empty());
            assert!(host.test_memory.is_none());
        }
        assert!(
            refused > 0 && succeeded > 0,
            "heap admission transition was not exercised: refused={refused}, succeeded={succeeded}"
        );
    }

    #[test]
    fn civil_reply_refusal_and_delivery_release_every_transport_charge() {
        let mut host = BootstrapHost::default();
        let arguments = [RuntimeValue::String(
            "9999-12-31T23:59:59.999999999Z".into(),
        )];
        let short = VmMemoryBudget::new(CIVIL_REPLY_BYTES - 1);
        assert!(matches!(
            host.invoke_admitted("std.time.UtcDateTime.parse", &arguments, Some(&short)),
            Err(VmError::ResourceLimit {
                resource: "memory",
                ..
            })
        ));
        assert_eq!(short.live_bytes(), 0);
        assert!(host.values.is_empty());
        let exact = VmMemoryBudget::new(CIVIL_REPLY_BYTES);
        let result = host
            .invoke_admitted("std.time.UtcDateTime.parse", &arguments, Some(&exact))
            .unwrap();
        assert_eq!(
            result.value,
            civil_result(
                UtcDateTime::parse("9999-12-31T23:59:59.999999999Z"),
                utc_value
            )
        );
        assert_eq!(
            result.memory.as_ref().unwrap().bytes(),
            result.value.retained_bytes().unwrap()
        );
        drop(result);
        assert_eq!(exact.live_bytes(), 0);
        assert!(host.values.is_empty());
        for (name, text) in [
            ("std.time.Date.parse", "bad"),
            ("std.time.Time.parse", "23:59:60"),
            ("std.time.UtcDateTime.parse", "2024-01-01T00:00:00+01:00"),
        ] {
            let result = host
                .invoke_admitted(name, &[RuntimeValue::String(text.into())], Some(&exact))
                .unwrap();
            assert!(matches!(result.value, RuntimeValue::ResultErr(_)));
            assert_eq!(
                result.memory.as_ref().unwrap().bytes(),
                result.value.retained_bytes().unwrap()
            );
            drop(result);
            assert_eq!(exact.live_bytes(), 0);
        }
    }

    #[test]
    fn civil_typed_carriers_refuse_wrong_shapes_and_ranges() {
        let date = Date::parse("2024-02-29").unwrap();
        let time = Time::parse("23:59:59.999999999").unwrap();
        let utc = UtcDateTime::create(date, time);
        assert_eq!(date_input(&date_value(date)).unwrap(), date);
        assert_eq!(time_input(&time_value(time)).unwrap(), time);
        assert_eq!(utc_input(&utc_value(utc)).unwrap(), utc);
        for value in [
            RuntimeValue::Unit,
            RuntimeValue::Record {
                name: "Date".into(),
                values: vec![],
            },
            RuntimeValue::Record {
                name: "Other".into(),
                values: vec![
                    RuntimeValue::Integer(2024),
                    RuntimeValue::Integer(2),
                    RuntimeValue::Integer(29),
                ],
            },
            RuntimeValue::Record {
                name: "Date".into(),
                values: vec![
                    RuntimeValue::Integer(2023),
                    RuntimeValue::Integer(2),
                    RuntimeValue::Integer(29),
                ],
            },
            RuntimeValue::Record {
                name: "Date".into(),
                values: vec![
                    RuntimeValue::Integer(i128::MAX),
                    RuntimeValue::Integer(2),
                    RuntimeValue::Integer(29),
                ],
            },
            RuntimeValue::Record {
                name: "Date".into(),
                values: vec![
                    RuntimeValue::String("2024".into()),
                    RuntimeValue::Integer(2),
                    RuntimeValue::Integer(29),
                ],
            },
        ] {
            assert!(matches!(date_input(&value), Err(VmError::Host(_))));
        }
        for value in [
            RuntimeValue::Unit,
            RuntimeValue::Record {
                name: "Time".into(),
                values: vec![],
            },
            RuntimeValue::Record {
                name: "Other".into(),
                values: vec![RuntimeValue::Integer(0)],
            },
            RuntimeValue::Record {
                name: "Time".into(),
                values: vec![RuntimeValue::Integer(-1)],
            },
            RuntimeValue::Record {
                name: "Time".into(),
                values: vec![RuntimeValue::Integer(86_400_000_000_000)],
            },
            RuntimeValue::Record {
                name: "Time".into(),
                values: vec![RuntimeValue::Bool(false)],
            },
        ] {
            assert!(matches!(time_input(&value), Err(VmError::Host(_))));
        }
        for value in [
            RuntimeValue::Unit,
            RuntimeValue::Record {
                name: "UtcDateTime".into(),
                values: vec![],
            },
            RuntimeValue::Record {
                name: "Other".into(),
                values: vec![date_value(date), time_value(time)],
            },
            RuntimeValue::Record {
                name: "UtcDateTime".into(),
                values: vec![RuntimeValue::Unit, time_value(time)],
            },
        ] {
            assert!(matches!(utc_input(&value), Err(VmError::Host(_))));
        }
        for (tag, expected) in [(0, MonthPolicy::Reject), (1, MonthPolicy::Clamp)] {
            assert_eq!(
                month_policy(&RuntimeValue::Variant {
                    name: "MonthPolicy".into(),
                    variant: tag,
                    values: vec![]
                })
                .unwrap(),
                expected
            );
        }
        for value in [
            RuntimeValue::Unit,
            RuntimeValue::Variant {
                name: "Other".into(),
                variant: 0,
                values: vec![],
            },
            RuntimeValue::Variant {
                name: "MonthPolicy".into(),
                variant: 2,
                values: vec![],
            },
            RuntimeValue::Variant {
                name: "MonthPolicy".into(),
                variant: 0,
                values: vec![RuntimeValue::Unit],
            },
        ] {
            assert!(matches!(month_policy(&value), Err(VmError::Host(_))));
        }
        let mut host = BootstrapHost::default();
        let budget = VmMemoryBudget::new(CIVIL_REPLY_BYTES);
        assert!(
            host.invoke_admitted("std.time.Date.create", &[], Some(&budget))
                .is_err()
        );
        assert_eq!(budget.live_bytes(), 0);
        assert!(host.values.is_empty());
    }
}
