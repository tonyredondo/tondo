"""Generate the bounded independent Gregorian reference corpus for the pure core."""

import argparse
import calendar
import datetime
import hashlib
import json
from pathlib import Path


DAY_NS = 86_400_000_000_000
DATES = [
    "0001-01-01", "0001-01-31", "0001-12-31", "0004-02-29",
    "0100-02-28", "0400-02-29", "1600-02-29", "1700-02-28",
    "1800-02-28", "1900-02-28", "1969-12-31", "1970-01-01",
    "1999-12-31", "2000-01-31", "2000-02-29", "2000-03-31",
    "2001-01-31", "2001-02-28", "2024-02-29", "2026-10-08",
    "2100-02-28", "2400-02-29", "9999-01-01", "9999-12-31",
]
DELTAS = [-9_223_372_036_854_775_808, -366, -31, -1, 0, 1, 31, 366,
          9_223_372_036_854_775_807]


def date_text(value):
    return f"{value.year:04}-{value.month:02}-{value.day:02}"


def utc_text(day, nanos):
    hour, remainder = divmod(nanos, 3_600_000_000_000)
    minute, remainder = divmod(remainder, 60_000_000_000)
    second, fraction = divmod(remainder, 1_000_000_000)
    suffix = f".{fraction:09}".rstrip("0") if fraction else ""
    return f"{date_text(day)}T{hour:02}:{minute:02}:{second:02}{suffix}Z"


def produce():
    parent = json.loads(Path("testing/stdlib-civil-time.json").read_text())
    # Bind semantics rather than mutable owner promotion/tracker state. Closing
    # a metadata gate must not change any of these independent reference bytes.
    semantic_contract = {
        "data": {key: parent["data"][key] for key in ["calendar", "year_min", "year_max", "seconds"]},
        "signatures": [row for row in parent["surface"]["signatures"]
                       if row["id"].startswith(("date-", "time-", "utc-")) and row["id"] != "utc-in-zone"],
        "parsing": {key: parent["parsing"][key] for key in ["date", "time", "utc", "locale", "leap_seconds", "canonical_output"]},
    }
    cases = []
    for text in DATES:
        value = datetime.date.fromisoformat(text)
        cases.append({"operation": "date-components", "input": text,
                      "year": value.year, "month": value.month, "day": value.day,
                      "dayOfWeek": value.isoweekday(),
                      "dayOfYear": value.timetuple().tm_yday})
        for delta in DELTAS:
            try:
                result = {"ok": date_text(value + datetime.timedelta(days=delta))}
            except (OverflowError, ValueError):
                result = {"error": "OutOfRange"}
            cases.append({"operation": "date-add-days", "input": text,
                          "amount": delta, "result": result})
        for delta in [-13, -12, -1, 0, 1, 12, 13]:
            year, month_zero = divmod((value.year - 1) * 12 + value.month - 1 + delta, 12)
            year += 1
            month = month_zero + 1
            for policy in ["Reject", "Clamp"]:
                if not 1 <= year <= 9999:
                    result = {"error": "OutOfRange"}
                else:
                    last = calendar.monthrange(year, month)[1]
                    if policy == "Reject" and value.day > last:
                        result = {"error": "InvalidDate"}
                    else:
                        result = {"ok": date_text(datetime.date(year, month, min(value.day, last)))}
                cases.append({"operation": "date-add-months", "input": text,
                              "amount": delta, "policy": policy, "result": result})
        for delta in [-9_223_372_036_854_775_808, -4, -1, 0, 1, 4, 9_223_372_036_854_775_807]:
            year = value.year + delta
            for policy in ["Reject", "Clamp"]:
                if not 1 <= year <= 9999:
                    result = {"error": "OutOfRange"}
                else:
                    last = calendar.monthrange(year, value.month)[1]
                    if policy == "Reject" and value.day > last:
                        result = {"error": "InvalidDate"}
                    else:
                        result = {"ok": date_text(datetime.date(year, value.month, min(value.day, last)))}
                cases.append({"operation": "date-add-years", "input": text,
                              "amount": delta, "policy": policy, "result": result})
        for nanos in [0, 1, DAY_NS - 1]:
            for delta in [-9_223_372_036_854_775_808, -1, 0, 1,
                          9_223_372_036_854_775_807]:
                ordinal_delta, remainder = divmod(nanos + delta, DAY_NS)
                try:
                    output_day = value + datetime.timedelta(days=ordinal_delta)
                    result = {"ok": utc_text(output_day, remainder)}
                except (OverflowError, ValueError):
                    result = {"error": "OutOfRange"}
                cases.append({"operation": "utc-add", "input": utc_text(value, nanos),
                              "durationNanoseconds": delta, "result": result})
    assert len(cases) == 1272
    return {"format": "tondo-civil-core-oracle/1",
            "reference_only": True,
            "native_tondo_promotion": False,
            "generator_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
            "oracle": "python-datetime-and-calendar-with-exact-integer-nanoseconds",
            "semantic_contract_sha256": hashlib.sha256(json.dumps(semantic_contract, sort_keys=True, separators=(",", ":")).encode()).hexdigest(),
            "cases": cases}


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--check", action="store_true")
    parser.add_argument("--output", type=Path, default=Path("crates/tondo-reliability/tests/fixtures/civil-core-oracle.json"))
    arguments = parser.parse_args()
    first = json.dumps(produce(), sort_keys=True, separators=(",", ":")) + "\n"
    second = json.dumps(produce(), sort_keys=True, separators=(",", ":")) + "\n"
    assert first == second
    if arguments.check:
        if arguments.output.read_text() != first:
            raise SystemExit("civil core oracle differs; regenerate with scripts/civil_core_oracle.py")
    else:
        arguments.output.parent.mkdir(parents=True, exist_ok=True)
        arguments.output.write_text(first)
    print("Civil core oracle: 1272 deterministic reference cases; no native Tondo promotion.")
