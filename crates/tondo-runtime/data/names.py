# Writes names.txt, the names that `format` and the parsing of std.time
# use in each language (standard library 14.3.5), from CLDR's
# cldr-dates-full package: `python3 -I names.py <package> > names.txt`.
#
# A first line says where the names come from; then a line per locale:
# its tag, a tab, and 52 names after one another, each
# followed by `|`: the months as a date writes them, abbreviated and alone,
# the days of the week from Sunday, abbreviated, and AM and PM. A region
# with the names of its language is left out.

import json
import os
import sys

package = sys.argv[1]
main = os.path.join(package, "main")


def names(tag):
    path = os.path.join(main, tag, "ca-gregorian.json")
    with open(path, encoding="utf-8") as file:
        gregorian = json.load(file)["main"][tag]["dates"]["calendars"]["gregorian"]
    months, days = gregorian["months"], gregorian["days"]
    periods = gregorian["dayPeriods"]["format"]["abbreviated"]
    out = [months["format"]["wide"][str(m)] for m in range(1, 13)]
    out += [months["format"]["abbreviated"][str(m)] for m in range(1, 13)]
    out += [months["stand-alone"]["wide"][str(m)] for m in range(1, 13)]
    week = ["sun", "mon", "tue", "wed", "thu", "fri", "sat"]
    out += [days["format"]["wide"][d] for d in week]
    out += [days["format"]["abbreviated"][d] for d in week]
    out += [periods["am"], periods["pm"]]
    return out


with open(os.path.join(package, "package.json"), encoding="utf-8") as file:
    version = json.load(file)["version"]
print(f"# CLDR {version}, cldr-dates-full. Copyright © 1991-2026 Unicode, Inc. Unicode License v3: https://www.unicode.org/license.txt")
tags = sorted(os.listdir(main), key=lambda t: (t != "en", t))
written = {}
for tag in tags:
    found = names(tag)
    if any("|" in name or "\t" in name or "\n" in name for name in found):
        sys.exit(f"{tag} has a name with a separator")
    language = tag.split("-")[0]
    if language != tag and written.get(language) == found:
        continue
    written[tag] = found
    print(tag + "\t" + "".join(name + "|" for name in found))
