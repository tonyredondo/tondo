use serde_json::Value;
use tondo_stdlib::civil_time::{CivilError, Date, MonthPolicy, UtcDateTime};

fn expected_result(result: Result<String, CivilError>) -> Value {
    match result {
        Ok(value) => serde_json::json!({"ok": value}),
        Err(error) => serde_json::json!({"error": error.to_string()}),
    }
}

#[test]
fn pure_civil_arithmetic_matches_the_independent_bounded_gregorian_oracle() {
    let corpus: Value =
        serde_json::from_str(include_str!("fixtures/civil-core-oracle.json")).unwrap();
    assert_eq!(corpus["format"], "tondo-civil-core-oracle/1");
    assert_eq!(corpus["reference_only"], true);
    assert_eq!(corpus["native_tondo_promotion"], false);
    let cases = corpus["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 1272);
    for (index, case) in cases.iter().enumerate() {
        let input = case["input"].as_str().unwrap();
        let operation = case["operation"].as_str().unwrap();
        if operation == "date-components" {
            let date = Date::parse(input).unwrap();
            for (field, actual) in [
                ("year", date.year()),
                ("month", date.month()),
                ("day", date.day()),
                ("dayOfWeek", date.day_of_week()),
                ("dayOfYear", date.day_of_year()),
            ] {
                assert_eq!(
                    case[field].as_i64().unwrap(),
                    actual,
                    "oracle row {index}: {field}"
                );
            }
            continue;
        }
        let actual = if operation == "utc-add" {
            UtcDateTime::parse(input)
                .unwrap()
                .checked_add(case["durationNanoseconds"].as_i64().unwrap())
                .map(UtcDateTime::format)
        } else {
            let date = Date::parse(input).unwrap();
            let amount = case["amount"].as_i64().unwrap();
            if operation == "date-add-days" {
                date.add_days(amount).map(Date::format)
            } else {
                let policy = match case["policy"].as_str().unwrap() {
                    "Reject" => MonthPolicy::Reject,
                    "Clamp" => MonthPolicy::Clamp,
                    _ => panic!("oracle policy is outside the closed contract"),
                };
                match operation {
                    "date-add-months" => date.add_months(amount, policy).map(Date::format),
                    "date-add-years" => date.add_years(amount, policy).map(Date::format),
                    _ => panic!("oracle operation is outside the pure boundary"),
                }
            }
        };
        assert_eq!(
            expected_result(actual),
            case["result"],
            "oracle row {index}: {input}, {operation}"
        );
    }
}
