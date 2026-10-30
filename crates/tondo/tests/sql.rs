//! `std.sql` (14.37 of the standard library) against PostgreSQL and MySQL,
//! when `TONDO_TEST_POSTGRES` and `TONDO_TEST_MYSQL` give their URLs, as CI
//! does on Linux; SQLite is a case of `tests/cases`.

use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

const PROGRAM: &str = r#"
import std.console
import std.env
import std.fs
import std.math
import std.sql
import std.sync

pub type Prefs = {
    theme: String
    tags: Array[String]
}

pub type User = {
    id: Int
    name: String
    email: String?
    prefs: Prefs?
    balance: math.Decimal?
    avatar: Array[Byte]?
    active: Bool
}

fn count(db: sql.Database): Int ! sql.SqlError {
    db.queryOne[Int]("SELECT count(*) FROM users")?
}

fn plus(db: sql.Database, index: Int): Int {
    db.queryOne[Int]("SELECT {index} + 1").unwrapOr(0)
}

fn main(): !sql.SqlError {
    let url = env.get("DATABASE_URL").unwrapOr("")
    let postgres = url.startsWith("postgres")
    var db = sql.open(url, connections: 4)?
    let nothing: Array[String] = []
    let seen = sync.Mutex(nothing)
    db.use((statement, next) {
        seen.update((all) { all.push("{statement.operation}") })
        let outcome = next(statement)
        outcome?
    })
    _ = db.execute("DROP TABLE IF EXISTS users, tondo_migrations")?
    let table = if postgres {
        "CREATE TABLE users (id BIGSERIAL PRIMARY KEY, name TEXT NOT NULL, email TEXT UNIQUE, prefs JSONB, balance NUMERIC(12, 2), avatar BYTEA, active BOOLEAN NOT NULL DEFAULT TRUE, CONSTRAINT positive CHECK (balance >= 0));"
    } else {
        "CREATE TABLE users (id BIGINT AUTO_INCREMENT PRIMARY KEY, name VARCHAR(100) NOT NULL, email VARCHAR(100) UNIQUE, prefs JSON, balance DECIMAL(12, 2), avatar BLOB, active BOOLEAN NOT NULL DEFAULT TRUE, CONSTRAINT positive CHECK (balance >= 0));"
    }
    let migrations = fs.memory()
    _ = migrations.writeText("001_users.sql", "{table}\nCREATE INDEX users_name ON users (name);\n")
    db.migrate(migrations)?
    db.migrate(migrations)?
    console.println("{db.query[String]("SELECT name FROM tondo_migrations")?}")

    let name = "ada'; DROP TABLE users; --"
    let balance: math.Decimal = 12.34
    let avatar = [Byte(1u8), Byte(255u8)]
    _ = db.execute("INSERT INTO users (name, email, balance, avatar) VALUES ({name}, {"ada@example.com"}, {balance}, {avatar})")?
    let prefs = "{{\"theme\": \"dark\", \"tags\": [\"x\", \"y\"]}}"
    let done = db.execute("INSERT INTO users (name, prefs, active) VALUES ({"bo"}, {prefs}, {false})")?
    console.println("{done.affected}")
    for user in db.query[User]("SELECT * FROM users ORDER BY id")? {
        console.println("{user.id} {user.name} {user.email} {user.prefs.map((p) { p.tags })} {user.balance} {user.avatar} {user.active}")
    }
    let ids = [1, 2, 9]
    let wanted: sql.Query = "id > {1}"
    console.println(
        "{db.query[String]("SELECT name FROM users WHERE id IN {ids} ORDER BY id")?.length()} {db.query[String]("SELECT name FROM {sql.identifier("users")} WHERE {wanted}")?}",
    )
    console.println("{db.queryOne[String?]("SELECT name FROM users WHERE id = {9}")?} {db.queryOne[Float]("SELECT {1.5} * 2.0")?}")

    let failures: Array[sql.Query] = [
        "INSERT INTO users (name, email) VALUES ({"cy"}, {"ada@example.com"})",
        "INSERT INTO users (email) VALUES ({"x@example.com"})",
        "UPDATE users SET balance = {-1} WHERE id = {1}",
        "SELEC 1",
    ]
    for query in failures {
        match db.execute(query) {
            ok(_) => console.println("ran")
            err(error) => console.println("{error.kind == sql.ErrorKind.Unique} {error.kind == sql.ErrorKind.NotNull} {error.kind == sql.ErrorKind.Check} {error.kind == sql.ErrorKind.Syntax} {error.code.isSome()}")
        }
    }
    match db.queryOne[String]("SELECT name FROM users WHERE id = {9}") {
        ok(_) => console.println("found")
        err(error) => console.println("{error.kind == sql.ErrorKind.NoRows}")
    }

    seen.set(nothing)
    let total = db.transaction((tx): Int ! sql.SqlError {
        _ = tx.execute("UPDATE users SET name = {"ada"} WHERE id = {1}")?
        let inner = tx.transaction((nested): Unit ! sql.SqlError {
            _ = nested.execute("DELETE FROM users WHERE id = {2}")?
            fail sql.SqlError.of(sql.ErrorKind.Other, "not this one")
        })
        console.println("{inner.isErr()}")
        count(tx)?
    })?
    console.println("{total} {seen.get()}")
    let crashed = recover {
        db.transaction((tx): Unit ! sql.SqlError {
            _ = tx.execute("DELETE FROM users")?
            panic("boom")
        })
    }
    console.println("{crashed.isErr()} {count(db)?} {db.queryOne[String]("SELECT name FROM users WHERE id = {1}")?}")

    var group = Group.new[Int]()
    for index in 0..20 {
        group.add(spawn plus(db, index))
    }
    var sum = 0
    for value in group.wait() {
        sum += value
    }
    console.println("{sum}")
    db.close()
}
"#;

const EXPECTED: &str = "[001_users.sql]
1
1 ada'; DROP TABLE users; -- some(ada@example.com) none some(12.34) some([1, 255]) true
2 bo none some([x, y]) none none false
2 [bo]
none 3.0
true false false false true
false true false false true
false false true false true
false false false true true
true
true
2 [begin, execute, begin, execute, rollback, query, commit]
true 2 ada
210
";

/// Waits for the server of a URL to take connections, as a container that
/// starts with the tests does.
fn reachable(url: &str) {
    let address = url.split('@').nth(1).and_then(|rest| rest.split('/').next()).unwrap();
    let started = Instant::now();
    while std::net::TcpStream::connect(address).is_err() {
        assert!(started.elapsed() < Duration::from_secs(180), "nothing listens at {address}");
        std::thread::sleep(Duration::from_millis(500));
    }
}

fn check(name: &str, variable: &str) {
    let Ok(url) = std::env::var(variable) else { return };
    reachable(&url);
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("main.to"), PROGRAM).unwrap();
    // A server that just started may still refuse its first connections.
    let started = Instant::now();
    loop {
        let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
            .args(["run", "main.to"])
            .current_dir(&dir)
            .env("DATABASE_URL", &url)
            .env("TONDO_RT_CHECK_LEAKS", "1")
            .output()
            .unwrap();
        let (stdout, stderr) = (String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        let refused = stderr.contains("connection") || stderr.contains("closed");
        if output.status.success() || !refused || started.elapsed() > Duration::from_secs(120) {
            assert_eq!(stdout, EXPECTED, "{stderr}");
            assert!(output.status.success(), "{stderr}");
            return;
        }
        std::thread::sleep(Duration::from_secs(2));
    }
}

#[test]
fn postgres() {
    check("postgres", "TONDO_TEST_POSTGRES");
}

#[test]
fn mysql() {
    check("mysql", "TONDO_TEST_MYSQL");
}
