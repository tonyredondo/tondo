//! Interfaces of C (language 16.16): a program calls an object C made
//! through the table of functions its first word points to, as COM's do.

mod common;

use std::path::Path;

fn tondo(dir: &Path, args: &[&str]) -> (String, String, i32) {
    let output = common::command(env!("CARGO_BIN_EXE_tondo")).args(args).current_dir(dir).output().unwrap();
    let text = |bytes: &[u8]| String::from_utf8_lossy(bytes).into_owned();
    (text(&output.stdout), text(&output.stderr), output.status.code().unwrap_or(-1))
}

/// A counter of C: its table holds `IUnknown`'s three functions, then its
/// own, which add, fail on a negative number, and give the total through
/// a last pointer, with `S_FALSE` past 100.
const COUNTER: &str = r#"
#include <stdint.h>
#include <string.h>

typedef struct Counter Counter;
typedef struct {
    int32_t (*QueryInterface)(Counter *self, const unsigned char *iid, void **out);
    uint32_t (*AddRef)(Counter *self);
    uint32_t (*Release)(Counter *self);
    int32_t (*Add)(Counter *self, int32_t by);
    int32_t (*Total)(Counter *self, int64_t *out);
    int32_t (*Last)(Counter *self);
} CounterVtbl;

struct Counter {
    const CounterVtbl *vtbl;
    uint32_t refs;
    int64_t total;
    int32_t last;
};

static const unsigned char IID_IUnknown[16] = {0, 0, 0, 0, 0, 0, 0, 0, 0xC0, 0, 0, 0, 0, 0, 0, 0x46};
/* 6A1B2C3D-0000-4000-8000-00000000C0DE as a GUID lays it out on a little-endian machine */
static const unsigned char IID_ICounter[16] = {0x3D, 0x2C, 0x1B, 0x6A, 0, 0, 0, 0x40, 0x80, 0, 0, 0, 0, 0, 0xC0, 0xDE};

static int32_t query(Counter *self, const unsigned char *iid, void **out) {
    if (!memcmp(iid, IID_IUnknown, 16) || !memcmp(iid, IID_ICounter, 16)) {
        *out = self;
        self->refs++;
        return 0;
    }
    *out = 0;
    return (int32_t)0x80004002;
}
static uint32_t add_ref(Counter *self) { return ++self->refs; }
static uint32_t release(Counter *self) { return --self->refs; }
static int32_t add(Counter *self, int32_t by) {
    if (by < 0) return (int32_t)0x80070057;
    self->total += by;
    self->last = by;
    return 0;
}
static int32_t total(Counter *self, int64_t *out) {
    *out = self->total;
    return self->total > 100 ? 1 : 0;
}
static int32_t last(Counter *self) { return self->last; }

static const CounterVtbl table = {query, add_ref, release, add, total, last};

void *make_counter(void) {
    static Counter counter;
    counter.vtbl = &table;
    counter.refs = 1;
    counter.total = 0;
    return &counter;
}
"#;

const PROGRAM: &str = r#"import std.console
import std.ffi

@ffi.Interface(id: "6A1B2C3D-0000-4000-8000-00000000C0DE")
pub trait Counter: ffi.Unknown {
    fn add(self, by: Int32): !ffi.HResult
    fn total(self): Int ! ffi.HResult
    fn last(self): Int32
}

extern fn make_counter(): Pointer[ffi.Void]

fn asked(counter: CounterObject, id: ffi.Guid): ffi.HResult {
    let wanted = ffi.allocate[ffi.Guid]()
    let found = ffi.allocate[Pointer[ffi.Void]]()
    unsafe { wanted.write(id) }
    let result = counter.queryInterface(wanted, found)
    unsafe {
        ffi.free(wanted)
        ffi.free(found)
    }
    result
}

fn main() {
    let counter = unsafe { CounterObject.of(make_counter()) }
    console.println("held {counter.addRef()}, then {counter.release()}")
    match counter.add(40) {
        ok(_) => console.println("added {counter.last()}")
        err(failed) => console.println("failed {failed}")
    }
    match counter.add(-1) {
        ok(_) => console.println("added")
        err(failed) => console.println("failed {failed}")
    }
    console.println("total {counter.total()}")
    _ = counter.add(70)
    console.println("total {counter.total()}")
    console.println("{asked(counter, CounterObject.Id)} {asked(counter, ffi.UnknownObject.Id)} {asked(counter, ffi.Guid.Empty)}")
    let unknown = counter.unknown()
    let same = unsafe { counter.pointer().address() == unknown.pointer().address() }
    console.println("released {unknown.release()} {same}")
}
"#;

/// A program calls an object C made by its interface: each method is an
/// entry of the table, the required interface's first; `!ffi.HResult`
/// fails on a negative number, `T ! ffi.HResult` gives `T` through a last
/// pointer, and `queryInterface` answers by identity.
#[test]
fn a_program_calls_an_object_c_made() {
    if !cfg!(any(target_os = "linux", target_os = "macos")) || common::command("cc").arg("--version").output().is_err()
    {
        return;
    }
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("interfaces");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join("counter.c"), COUNTER).unwrap();
    let compiled =
        common::command("cc").args(["-c", "-O1", "counter.c", "-o", "counter.o"]).current_dir(&dir).output().unwrap();
    assert!(compiled.status.success(), "{}", String::from_utf8_lossy(&compiled.stderr));
    let archived = common::command("ar").args(["rcs", "libcounter.a", "counter.o"]).current_dir(&dir).output().unwrap();
    assert!(archived.status.success(), "{}", String::from_utf8_lossy(&archived.stderr));
    std::fs::write(dir.join("tondo.toml"), "[package]\nname = \"app\"\n\n[native]\nlibraries = [\"libcounter.a\"]\n")
        .unwrap();
    std::fs::write(dir.join("src/main.to"), PROGRAM).unwrap();
    let (stdout, stderr, status) = tondo(&dir, &["run", "."]);
    assert_eq!(status, 0, "{stdout}{stderr}");
    assert_eq!(
        stdout,
        "held 2, then 1\nadded 40\nfailed 0x80070057: The parameter is incorrect\ntotal ok(40)\ntotal ok(110)\n\
         0x00000000: The operation completed successfully 0x00000000: The operation completed successfully \
         0x80004002: No such interface supported\nreleased 2 true\n"
    );
}

/// What an interface cannot be, each said at once.
#[test]
fn an_interface_is_held_to_what_a_table_can_be() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("interfaces-wrong");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let program = "import std.ffi\n\n\
        @ffi.Interface(id: \"not a guid\")\npub trait Spelled {\n    fn one(self): Int32\n}\n\n\
        @ffi.Interface\npub trait Anonymous: ffi.Unknown {\n    fn two(self): Int32\n}\n\n\
        @ffi.Interface\npub trait Texts {\n    fn three(self, text: String): Int32\n    fn four(mut self)\n    \
        fn five(self): Int ! String\n}\n\n\
        trait Plain {}\n\n@ffi.Interface\npub trait OnPlain: Plain {}\n\n\
        @ffi.Interface(id: \"6A1B2C3D-0000-4000-8000-00000000C0DE\")\npub trait Counted: ffi.Unknown {\n    \
        fn add(self, by: Int32): !ffi.HResult\n}\n\ntype Empty = {}\n\nimpl Counted for Empty {}\n\n\
        fn main() {\n    _ = ffi.Guid.parse(\"1234\")\n}\n";
    std::fs::write(dir.join("main.to"), program).unwrap();
    let (_, stderr, status) = tondo(&dir, &["check", "main.to"]);
    assert_eq!(status, 1, "{stderr}");
    for said in [
        "E1708]: `not a guid` is not a GUID",
        "E1707]: the interface `Anonymous` requires `ffi.Unknown`, so `queryInterface` asks for it by its identity",
        "E1707]: the method `three` of the interface `Texts` takes `String`, which does not cross to C",
        "E1707]: the method `four` of the interface `Texts` takes `self`",
        "E1707]: the method `five` of the interface `Texts` fails with `String`",
        "E1707]: the interface `OnPlain` requires `Plain`, which is not an interface",
        "E1708]: `1234` is not a GUID",
        "E1114]: `impl Counted for Empty` is missing `add`",
    ] {
        assert!(stderr.contains(said), "{said}:\n{stderr}");
    }
}

/// A user of an object: C asks it for its interfaces, calls its methods
/// and counts who holds it, and lets it go at last.
const DRIVER: &str = r#"
#include <stdint.h>

typedef struct Object Object;
typedef struct {
    int32_t (*QueryInterface)(Object *self, const unsigned char *iid, void **out);
    uint32_t (*AddRef)(Object *self);
    uint32_t (*Release)(Object *self);
    int32_t (*Add)(Object *self, int32_t by);
    int32_t (*Total)(Object *self, int64_t *out);
    int32_t (*Seven)(Object *self);
} Table;
struct Object { const Table *table; };

/* 6A1B2C3D-0000-4000-8000-00000000C0DE */
static const unsigned char IID_Tallied[16] = {0x3D, 0x2C, 0x1B, 0x6A, 0, 0, 0, 0x40, 0x80, 0, 0, 0, 0, 0, 0xC0, 0xDE};
static const unsigned char IID_Other[16] = {1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16};

/* What it saw, in `said`, in order. */
void drive(Object *object, int64_t *said) {
    void *found = 0, *missing = (void *)1;
    said[0] = object->table->QueryInterface(object, IID_Tallied, &found);
    said[1] = object->table->QueryInterface(object, IID_Other, &missing);
    said[2] = found == object;
    said[3] = missing == 0;
    said[4] = object->table->Add(object, 5);
    said[5] = object->table->Add(object, -1);
    int64_t total = 0;
    said[6] = object->table->Total(object, &total);
    said[7] = total;
    said[8] = object->table->Seven(object);
    said[9] = object->table->AddRef(object);
    said[10] = object->table->Release(object);
    said[11] = object->table->Release(object);
    said[12] = object->table->Release(object);
}
"#;

const IMPLEMENTED: &str = r#"import std.console
import std.ffi
import std.sync

@ffi.Interface(id: "6A1B2C3D-0000-4000-8000-00000000C0DE")
pub trait Tallied: ffi.Unknown {
    fn add(self, by: Int32): !ffi.HResult
    fn total(self): Int ! ffi.HResult
    fn seven(self): Int32 {
        7
    }
}

type Tally = {
    name: String
    total: sync.Atomic[Int]
}

impl Tallied for Tally {
    fn add(self, by: Int32): !ffi.HResult {
        if by < 0 {
            fail ffi.HResult.InvalidArgument
        }
        _ = self.total.add(Int(by))
    }

    fn total(self): Int ! ffi.HResult {
        self.total.load() + self.name.length()
    }
}

extern fn drive(object: Pointer[ffi.Void], said: Pointer[Int])

fn main() {
    let tally = Tally { name: "tally", total: sync.Atomic(30) }
    let said = ffi.allocate[Int](13)
    unsafe {
        drive(ffi.object(tally), said)
        var seen: Array[Int] = []
        for at in 0..13 {
            seen.push(said.offset(at).read())
        }
        console.println("{seen}")
        ffi.free(said)
    }
    console.println("the value has {tally.total.load()}")
}
"#;

/// C calls a Tondo value through the table the compiler writes for its
/// type: `queryInterface` answers for the interfaces it implements, a
/// method by default is an entry too, a failure is its `HResult`, and the
/// count reaching zero gives the value back once, as the check of leaks
/// sees.
#[test]
fn c_calls_a_tondo_value_through_its_interface() {
    if !cfg!(any(target_os = "linux", target_os = "macos")) || common::command("cc").arg("--version").output().is_err()
    {
        return;
    }
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("interfaces-implemented");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join("driver.c"), DRIVER).unwrap();
    let compiled =
        common::command("cc").args(["-c", "-O1", "driver.c", "-o", "driver.o"]).current_dir(&dir).output().unwrap();
    assert!(compiled.status.success(), "{}", String::from_utf8_lossy(&compiled.stderr));
    let archived = common::command("ar").args(["rcs", "libdriver.a", "driver.o"]).current_dir(&dir).output().unwrap();
    assert!(archived.status.success(), "{}", String::from_utf8_lossy(&archived.stderr));
    std::fs::write(dir.join("tondo.toml"), "[package]\nname = \"app\"\n\n[native]\nlibraries = [\"libdriver.a\"]\n")
        .unwrap();
    std::fs::write(dir.join("src/main.to"), IMPLEMENTED).unwrap();
    let output = common::command(env!("CARGO_BIN_EXE_tondo"))
        .args(["run", "--debug", "."])
        .env("TONDO_RT_CHECK_LEAKS", "1")
        .current_dir(&dir)
        .output()
        .unwrap();
    let (stdout, stderr) = (String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    assert!(output.status.success(), "{stdout}{stderr}");
    // Asked for its interface, it is held once more; asked for another,
    // it answers none. Then it adds, refuses -1, totals 30 + 5 and five
    // letters, gives 7 by default, and is let go to nothing.
    assert_eq!(stdout, "[0, -2147467262, 1, 1, 0, -2147024809, 0, 40, 7, 3, 2, 1, 0]\nthe value has 35\n", "{stderr}");
}

/// What a trait's methods are named and how their parameters are: an `impl`
/// takes its methods' names from its trait, so only the trait's
/// declaration is held to the conventions, as a trait `tondo bind` writes
/// keeps C's; and a parameter named `_`, or as the derived code names its
/// own, is passed on all the same.
#[test]
fn an_implementation_takes_its_names_from_its_trait() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("interfaces-names");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let program = "import std.console\nimport std.ffi\n\n\
        @ffi.Interface(id: \"6A1B2C3D-0000-4000-8000-00000000BEEF\")\npub trait Touched: ffi.Unknown {\n    \
        fn Touch(self, _: Int32, call: Int32, result: Int32): ffi.HResult\n}\n\n\
        type Toucher = {}\n\n\
        impl Touched for Toucher {\n    fn Touch(self, _: Int32, call: Int32, result: Int32): ffi.HResult {\n        \
        console.println(\"touched {call} {result}\")\n        ffi.HResult.Ok\n    }\n}\n\n\
        fn main() {\n    let object = ffi.object(Toucher {})\n    let touched = unsafe { TouchedObject.of(object) }\n    \
        console.println(\"{touched.Touch(1, 2, 3).isOk()}\")\n    _ = unsafe { ffi.UnknownObject.of(object).release() }\n}\n";
    std::fs::write(dir.join("touched.to"), program).unwrap();
    let (out, err, code) = tondo(&dir, &["run", "touched.to"]);
    assert_eq!((out.as_str(), code), ("touched 2 3\ntrue\n", 0), "{err}");
    assert_eq!(err.matches("W1004").count(), 1, "only the trait's declaration: {err}");
}
