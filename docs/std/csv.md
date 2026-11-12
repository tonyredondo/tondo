# std.csv

comma-separated values (RFC 4180), read and written a record at a time, so a file of any size streams through.

```tondo
import std.console
import std.csv
import std.io

for row in csv.rows(io.reader("name,age\nAda,36\nAlan,41\n")) {
    console.println(row?.join(" | "))
}
```

<!-- From here on, docs/std/generate.to writes this page from `tondo api`: edit above this line. -->

## Reference

`tondo api std.csv` shows the same in a terminal.

- [Functions](#functions): [`rows`](#rows), [`records`](#records), [`parse`](#parse), [`parseRecords`](#parserecords), [`format`](#format), [`formatRecords`](#formatrecords)
- [CsvError](#csverror)
- [Rows](#rows-1)
- [Records](#records-1)
- [Writer](#writer): [`Writer`](#writer-1), [`Writer.write`](#writerwrite)

### Functions

#### rows

```tondo
pub fn rows(input: io.Reader, separator: Char = ','): Rows
```

The records of the stream, each as its fields, read as they are asked
for.

#### records

```tondo
pub fn records(input: io.Reader, separator: Char = ','): Records
```

The records of the stream whose first record is the header, each as a
map from the header's names to its fields.

#### parse

```tondo
pub fn parse(input: String, separator: Char = ','): Array[Array[String]] ! CsvError
```

Every record of a CSV text, each as its fields.

#### parseRecords

```tondo
pub fn parseRecords(input: String, separator: Char = ','): Array[Map[String, String]] ! CsvError
```

Every record of a CSV text whose first record is the header.

#### format

```tondo
pub fn format(table: Array[Array[String]], separator: Char = ','): String
```

The CSV text of the records, a line each.

#### formatRecords

```tondo
pub fn formatRecords(entries: Array[Map[String, String]], separator: Char = ','): String
```

The CSV text of records with the same names: a header with the first
record's names, then a line for each.

### CsvError

```tondo
pub enum CsvError {
    /// A quote inside a field that does not start with one, or text after a
    /// closing quote, at this line and column.
    Malformed(Int, Int)
    /// A quoted field that does not end, from this line on.
    Unterminated(Int)
    /// A record at this line with other than the header's number of fields:
    /// the line, the header's count and the record's.
    FieldCount(Int, Int, Int)
    /// Reading the input failed.
    Io(io.IoError)
}
```

Why a CSV stream could not be read: a quote out of place, a quoted field
that does not end, a record with other than the header's number of
fields, or the stream failed.

```tondo
import std.console
import std.csv
import std.io

for row in csv.rows(io.reader("a,\"b\nc\n")) {
    match row {
        ok(fields) => console.println("{fields}")
        err(csv.CsvError.Unterminated(line)) => console.println("a quote opened at line {line} does not close")
        err(error) => console.println("{error}")
    }
}
```

Implements `Display`, `reflect.Reflect`.

### Rows

```tondo
pub type Rows = {
    // private fields
}
```

The records of a CSV stream, each as its fields, read as they are asked
for: `for row in csv.rows(file) { let fields = row? }`. Blank lines are
skipped; after an error there are no more.

```tondo
import std.console
import std.csv
import std.fs

let file = fs.open("people.csv", fs.OpenMode.Read)?
for row in csv.rows(file.reader(), separator: ';') {
    console.println(row?.join(" | "))
}
```

Implements `Iterator[Array[String] ! CsvError]`.

### Records

```tondo
pub type Records = {
    // private fields
}
```

The records of a CSV stream whose first record is the header, each as a
map from the header's names to its fields, in the header's order.

```tondo
import std.console
import std.csv
import std.io

for record in csv.records(io.reader("name,age\nAda,36\n")) {
    let fields = record?
    console.println("{fields["name"]} is {fields["age"]}")
}
```

Implements `Iterator[Map[String, String] ! CsvError]`.

### Writer

```tondo
pub type Writer = {
    // private fields
}
```

Writes records to a stream, a line each; a field with the separator, a
quote or a line break goes in quotes.

```tondo
import std.csv
import std.fs

let file = fs.open("people.csv", fs.OpenMode.Write)?
let out = csv.Writer(file.writer())
out.write(["name", "age"])?
out.write(["Ada", "36"])?
```

#### Writer

```tondo
pub fn Writer(output: io.Writer, separator: Char = ','): Writer
```

A writer of CSV records to the stream, with `separator` between fields.

#### Writer.write

```tondo
pub fn Writer.write(self, fields: Array[String]): !io.IoError
```

Writes the fields as one record, quoting those that need it.
