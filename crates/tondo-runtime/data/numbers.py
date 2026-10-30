# Writes numbers.txt, what `std.format` needs to write numbers and to pick
# plural forms in each language (standard library 10.6), from CLDR's
# cldr-numbers-full and cldr-core packages:
# `python3 -I numbers.py <numbers package> <core package> > numbers.txt`.
#
# A first line says where the data comes from. Then, after a tab each:
# - `fractions`, and each currency whose amounts do not have two decimals,
#   as `JPY=0|`;
# - `plurals`, a language, and its rules of cardinal plurals, as
#   `one=i = 1 and v = 0|`, without their samples;
# - a locale, its digits, decimal separator, group separator, minus sign,
#   plus sign, percent sign, minimum grouping digits and its patterns of
#   decimals, percents and currencies, each followed by `|`; then the
#   symbols of currencies, as `USD=US$|`. A language keeps the symbols that
#   are not their code, and a region those that are not its language's; a
#   region whose numbers and symbols are its language's is left out.

import json
import os
import sys

numbers_package, core_package = sys.argv[1], sys.argv[2]
main = os.path.join(numbers_package, "main")
supplemental = os.path.join(core_package, "supplemental")


def load(*parts):
    with open(os.path.join(*parts), encoding="utf-8") as file:
        return json.load(file)


systems = load(supplemental, "numberingSystems.json")["supplemental"]["numberingSystems"]


def numbers(tag):
    found = load(main, tag, "numbers.json")["main"][tag]["numbers"]
    system = found["defaultNumberingSystem"]
    digits = systems[system].get("_digits", "0123456789")
    symbols = found.get(f"symbols-numberSystem-{system}") or found["symbols-numberSystem-latn"]
    def pattern(kind):
        formats = found.get(f"{kind}Formats-numberSystem-{system}") or found[f"{kind}Formats-numberSystem-latn"]
        return formats["standard"]
    return [
        digits,
        symbols["decimal"],
        symbols["group"],
        symbols["minusSign"],
        symbols["plusSign"],
        symbols["percentSign"],
        found["minimumGroupingDigits"],
        pattern("decimal"),
        pattern("percent"),
        pattern("currency"),
    ]


def currencies(tag):
    found = load(main, tag, "currencies.json")["main"][tag]["numbers"]["currencies"]
    return {code: entry["symbol"] for code, entry in found.items() if "symbol" in entry}


def clean(values, separators="|=\t\n"):
    for value in values:
        if any(c in value for c in separators):
            sys.exit(f"{value!r} has a separator")
    return values


version = load(numbers_package, "package.json")["version"]
print(f"# CLDR {version}, cldr-numbers-full and cldr-core. Copyright © 1991-2026 Unicode, Inc. Unicode License v3: https://www.unicode.org/license.txt")

fractions = load(supplemental, "currencyData.json")["supplemental"]["currencyData"]["fractions"]
odd = sorted((code, f["_digits"]) for code, f in fractions.items() if code != "DEFAULT" and f["_digits"] != "2")
print("fractions\t" + "".join(f"{code}={digits}|" for code, digits in odd))

plurals = load(supplemental, "plurals.json")["supplemental"]["plurals-type-cardinal"]
for language in sorted(plurals):
    rules = []
    for key, rule in plurals[language].items():
        category = key.removeprefix("pluralRule-count-")
        condition = rule.split("@")[0].strip()
        if category != "other":
            rules.append(f"{category}={condition}|")
    print(f"plurals\t{language}\t" + "".join(clean(rules, "\t\n")))

tags = sorted(os.listdir(main), key=lambda t: (t != "en", t))
written = {}
for tag in tags:
    found = clean(numbers(tag))
    symbols = currencies(tag)
    language = tag.split("-")[0]
    if language != tag and language in written:
        parent_numbers, parent_symbols = written[language]
        own = {code: symbol for code, symbol in symbols.items() if parent_symbols.get(code, code) != symbol}
        if found == parent_numbers and not own:
            continue
    else:
        own = {code: symbol for code, symbol in symbols.items() if symbol != code}
    if language == tag:
        written[tag] = (found, symbols)
    clean(list(own) + list(own.values()))
    shown = "".join(f"{code}={symbol}|" for code, symbol in sorted(own.items()))
    print(tag + "\t" + "".join(value + "|" for value in found) + "\t" + shown)
