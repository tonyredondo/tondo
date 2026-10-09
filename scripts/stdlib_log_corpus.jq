def uint: type == "number" and . >= 0 and . <= 1048576 and floor == .;
def scalar: type == "string" and utf8bytelength <= 256;
def integer_string($signed):
  type == "string" and
  if $signed then
    test("^-?(0|[1-9][0-9]{0,18})$") and
    (if startswith("-") then .[1:] as $n | ($n | length) < 19 or $n <= "9223372036854775808"
     else length < 19 or . <= "9223372036854775807" end)
  else
    test("^(0|[1-9][0-9]{0,19})$") and
    (length < 20 or . <= "18446744073709551615")
  end;
def descriptor($depth):
  type == "object" and $depth <= 20 and
  if .kind == "Null" or .kind == "Redacted" then keys == ["kind"]
  else keys == ["kind","value"] and
    if .kind == "Bool" then (.value | type == "boolean")
    elif .kind == "Int" then (.value | integer_string(true))
    elif .kind == "UInt" then (.value | integer_string(false))
    elif .kind == "Float" then (.value | type == "string" and length <= 32 and
      (IN("NaN","inf","-inf") or test("^-?[0-9]+(\\.[0-9]+)?([eE][+-]?[0-9]+)?$")))
    elif .kind == "Text" then (.value | scalar)
    elif .kind == "Bytes" then (.value | type == "string" and length <= 512 and test("^([0-9a-f]{2})*$"))
    elif .kind == "Array" then (.value | type == "array" and length <= 128 and all(.[]; descriptor($depth+1)))
    elif .kind == "Object" then (.value | type == "array" and length <= 64 and
      all(.[]; type == "array" and length == 2 and (.[0] | scalar) and (.[1] | descriptor($depth+1))))
    else false end
  end;
def fields: type == "array" and length <= 64 and
  all(.[]; type == "array" and length == 2 and (.[0] | scalar) and (.[1] | descriptor(0)));
def event: type == "object" and keys == ["fields","level","message","target","timestamp"]
  and (.level | IN("Trace","Debug","Info","Warn","Error"))
  and (.target | scalar) and (.message | scalar) and (.fields | fields)
  and (.timestamp == null or (.timestamp |
    IN("2000-02-29T12:34:56Z","2026-10-09T09:00:00.123456789Z")));
def error: type == "string" and IN("DuplicateField","InvalidFieldKey","InvalidTarget","NonFiniteValue","ResourceLimit");
def limits: type == "object" and all(keys[]; IN("depth","event","fields","key","string","queue"))
  and all(.[]; uint);
def log_corpus:
  type == "object" and keys == ["cases","format","scope"]
  and .format == "tondo-stdlib-log-corpus/1" and .scope == "bounded-values-and-format-regressions"
  and (.cases | type == "array" and length == 34 and all(.[]; type == "object"))
  and ([.cases[].id] | unique | length) == 34
  and all(.cases[];
    type == "object" and (.id | type == "string" and test("^[a-z0-9-]+$"))
    and ([.. | objects | select(has("kind"))] | length <= 128)
    and if .operation == "format" then
      ((keys - ["id","operation","format","event","limits","record","error"]) | length == 0)
      and (.format | IN("Text","JsonLines")) and (.event | event)
      and ((has("limits") | not) or (.limits | limits))
      and (has("record") != has("error"))
      and (if has("error") then (.error | error) else
        (.record | type == "string" and utf8bytelength <= 262144 and endswith("\n")
          and (split("\n") | length) == 2) end)
    elif .operation == "field" then keys == ["error","existing","id","key","operation","value"]
      and (.existing | fields) and (.key | scalar)
      and (.value | descriptor(0)) and (.error | error) and (has("record") | not)
    elif .operation == "event" then keys == ["error","event","id","operation"]
      and (.event | event) and (.error | error) and (has("record") | not)
    else false end);
