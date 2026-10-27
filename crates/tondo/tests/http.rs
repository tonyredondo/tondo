//! `std.http` (14.24 of the standard library) against a small HTTP/1.1
//! server of the test's own, which answers by path.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::process::Command;

/// A request as the server read it: method, target, headers (lowercase
/// names) and body, without chunked framing.
struct Request {
    method: String,
    target: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

fn read_request(stream: &mut BufReader<TcpStream>) -> Option<Request> {
    let mut line = String::new();
    stream.read_line(&mut line).ok()?;
    let mut parts = line.split_whitespace();
    let (method, target) = (parts.next()?.to_string(), parts.next()?.to_string());
    let mut headers = Vec::new();
    loop {
        let mut header = String::new();
        stream.read_line(&mut header).ok()?;
        let header = header.trim_end();
        if header.is_empty() {
            break;
        }
        let (name, value) = header.split_once(':')?;
        headers.push((name.trim().to_lowercase(), value.trim().to_string()));
    }
    let find = |name: &str| headers.iter().find(|(n, _)| n == name).map(|(_, v)| v.clone());
    let mut body = Vec::new();
    if let Some(length) = find("content-length") {
        body.resize(length.parse().ok()?, 0);
        stream.read_exact(&mut body).ok()?;
    } else if find("transfer-encoding").is_some_and(|t| t.contains("chunked")) {
        loop {
            let mut size = String::new();
            stream.read_line(&mut size).ok()?;
            let size = usize::from_str_radix(size.trim(), 16).ok()?;
            let mut chunk = vec![0; size + 2];
            stream.read_exact(&mut chunk).ok()?;
            if size == 0 {
                break;
            }
            body.extend_from_slice(&chunk[..size]);
        }
    }
    Some(Request { method, target, headers, body })
}

fn respond(stream: &mut TcpStream, status: &str, headers: &[(&str, String)], body: &[u8]) {
    let mut head = format!("HTTP/1.1 {status}\r\ncontent-length: {}\r\nconnection: close\r\n", body.len());
    for (name, value) in headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str("\r\n");
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(body);
}

fn serve(stream: TcpStream) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let Some(request) = read_request(&mut reader) else { return };
    let mut stream = stream;
    let header =
        |name: &str| request.headers.iter().find(|(n, _)| n == name).map(|(_, v)| v.clone()).unwrap_or_default();
    let path = request.target.split('?').next().unwrap_or_default();
    let query = request.target.split_once('?').map(|(_, q)| q.to_string()).unwrap_or_default();
    match path {
        "/hello" => respond(
            &mut stream,
            "200 OK",
            &[
                ("content-type", "text/plain".into()),
                ("x-echo", header("x-test")),
                ("set-cookie", "a=1".into()),
                ("set-cookie", "b=2".into()),
            ],
            format!("hello {} {query}", request.method).as_bytes(),
        ),
        "/json" => respond(
            &mut stream,
            "200 OK",
            &[("content-type", "application/json".into())],
            br#"{"name":"ada","age":36}"#,
        ),
        "/echo" => {
            let mut body = format!("{} {}\n", header("content-type"), request.body.len()).into_bytes();
            body.extend_from_slice(&request.body);
            respond(&mut stream, "200 OK", &[], &body)
        }
        "/missing" => respond(&mut stream, "404 Not Found", &[], b"nothing here"),
        "/slow" => {
            std::thread::sleep(std::time::Duration::from_secs(3));
            respond(&mut stream, "200 OK", &[], b"late")
        }
        "/redirect" => respond(&mut stream, "302 Found", &[("location", "/hello?from=redirect".into())], b""),
        "/events" => respond(
            &mut stream,
            "200 OK",
            &[("content-type", "text/event-stream".into())],
            b": comment\nevent: greeting\ndata: hello\ndata: world\nid: 1\n\ndata: second\n\n",
        ),
        "/lines" => respond(&mut stream, "200 OK", &[], b"{\"n\":1}\n{\"n\":2}\n{\"n\":3}\n"),
        "/big" => respond(&mut stream, "200 OK", &[], &vec![b'x'; 1 << 20]),
        "/latin" => respond(
            &mut stream,
            "200 OK",
            &[("content-type", "text/plain; charset=ISO-8859-1".into())],
            b"a\xf1o \x80\nl\xednea\n",
        ),
        "/sjis" => {
            respond(&mut stream, "200 OK", &[("content-type", "text/plain; charset=Shift_JIS".into())], b"\x93\xfa")
        }
        _ => respond(&mut stream, "500 Internal Server Error", &[], b"unknown path"),
    }
}

fn server() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            std::thread::spawn(move || serve(stream));
        }
    });
    format!("http://{address}")
}

#[test]
fn client_against_a_local_server() {
    let base = server();
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("http-client");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("report.txt"), "the report").unwrap();
    let program = r#"
import std.console
import std.env
import std.fs
import std.http
import std.io
import std.time

pub type Person = { name: String, age: Int }

fn status(client: http.Client, index: Int): Int {
    match client.get("/hello", query: ["i": "{index}"]) {
        ok(response) => response.status()
        err(_) => 0
    }
}

fn main(): !(http.HttpError | fs.FsError) {
    let base = env.get("BASE").unwrapOr("")
    let hello = http.get("{base}/hello", query: ["q": "a b"], headers: ["X-Test": "yes"])?
    console.println("{hello.status()} {hello.reason()} {hello.version()} {hello.header("X-ECHO")} {hello.headerValues("set-cookie")}")
    console.println(hello.text()?)
    for line in hello.lines() {
        console.println("{line.isErr()}")
    }
    console.print(http.get("{base}/latin")?.text()?)
    for line in http.get("{base}/latin")?.lines() {
        console.println(line?)
    }
    console.println("{http.get("{base}/sjis")?.text()}")

    let client = http.Client.new(baseUrl: base, headers: ["X-Test": "client"])
    let ada: Person = client.get("/json")?.json()?
    console.println("{ada.name} {ada.age}")
    console.println(client.post("/echo", body: http.Body.json(ada))?.text()?)
    console.println(client.post("/echo", body: http.Body.form(["a": "1", "b": "x y"]))?.text()?)
    let form = http.Body.multipart([
        http.Part.field("title", "Report"),
        http.Part.file("report", path: "report.txt"),
        http.Part.bytes("data", "abc".bytes(), fileName: "data.bin", contentType: "application/octet-stream"),
    ])
    let multipart = client.post("/echo", body: form)?.text()?
    console.println("{multipart.startsWith("multipart/form-data; boundary=")} {multipart.contains("the report")} {multipart.contains("filename=\"report.txt\"")} {multipart.contains("Report")}")
    let streamed = client.put("/echo", body: http.Body.stream(io.reader("streamed body"), contentType: "text/plain"))?.text()?
    console.println(streamed)

    match client.get("/missing") {
        err(http.HttpError.Status(response)) => console.println("{response.status()} {response.text()?}")
        _ => console.println("expected a status error")
    }
    console.println("{client.get("/missing")}")
    console.println("{client.get("/slow", timeout: time.milliseconds(200))}")
    console.println("{"{http.get("http://127.0.0.1:1/")}".startsWith("err(cannot connect")}")
    console.println("{"{http.get("not a url")}".startsWith("err(invalid URL")}")
    let redirected = client.get("/redirect")?
    console.println("{redirected.url().endsWith("/hello?from=redirect")} {redirected.text()?}")

    for event in client.get("/events")?.events() {
        let e = event?
        console.println("{e.event}: {e.data} {e.id}")
    }
    var count = 0
    for line in client.get("/lines")?.lines() {
        count += line?.length()
    }
    console.println("{count}")
    let out = fs.open("big.bin", fs.OpenMode.Write)?.writer()
    console.println("{client.get("/big")?.copyTo(out)?}")
    out.close().expect("closes")
    console.println("{fs.metadata("big.bin")?.size}")

    // Tasks wait for their requests without holding a worker.
    var group = Group.new[Int]()
    for index in 0..64 {
        group.add(spawn status(client, index))
    }
    console.println("{group.wait().all((code) { code == 200 })}")
}
"#;
    std::fs::write(dir.join("main.to"), program).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .args(["run", "main.to"])
        .current_dir(&dir)
        .env("BASE", &base)
        .env("TONDO_RT_CHECK_LEAKS", "1")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let expected = "\
200 OK HTTP/1.1 some(yes) [a=1, b=2]
hello GET q=a%20b
true
año €
línea
año €
línea
err(the body: it is in `shift_jis`, a charset Tondo does not decode: read its bytes with `bytes()` instead)
ada 36
application/json 23
{\"name\":\"ada\",\"age\":36}
application/x-www-form-urlencoded 11
a=1&b=x%20y
true true true true
text/plain 13
streamed body
404 nothing here
err(GET BASE/missing answered 404 Not Found)
err(the request timed out)
true
true
true hello GET from=redirect
greeting: hello
world some(1)
message: second none
21
1048576
1048576
true
"
    .replace("BASE", &base);
    assert_eq!(stdout, expected, "{stderr}");
    assert!(output.status.success(), "{stderr}");
}

/// A WebSocket server that echoes text, reverses bytes and closes after
/// `bye`.
fn websocket_server() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            std::thread::spawn(move || {
                use tungstenite::Message;
                let Ok(mut socket) = tungstenite::accept(stream) else { return };
                while let Ok(message) = socket.read() {
                    match message {
                        Message::Text(text) => {
                            let _ = socket.send(Message::Text(format!("echo: {text}").into()));
                            if text.as_str() == "bye" {
                                let _ = socket.close(None);
                            }
                        }
                        Message::Binary(data) => {
                            let reversed: Vec<u8> = data.iter().rev().copied().collect();
                            let _ = socket.send(Message::Binary(reversed.into()));
                        }
                        Message::Close(_) => break,
                        _ => {}
                    }
                }
            });
        }
    });
    format!("ws://{address}")
}

#[test]
fn websocket_against_a_local_server() {
    let url = websocket_server();
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("http-websocket");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let program = r#"
import std.console
import std.env
import std.http

fn main(): !http.HttpError {
    let socket = http.WebSocket.connect("{env.get("WS").unwrapOr("")}/chat", headers: ["X-Test": "yes"])?
    socket.sendText("hola")?
    socket.sendBytes([Byte(1u8), Byte(2u8), Byte(3u8)])?
    console.println("{socket.receive()?}")
    socket.sendText("bye")?
    for message in socket {
        match message? {
            http.Message.Text(text) => console.println(text)
            http.Message.Binary(bytes) => console.println("{bytes}")
        }
    }
    console.println("closed {socket.receive()?}")
    match http.WebSocket.connect("ws://127.0.0.1:1/") {
        err(error) => console.println("{"{error}".startsWith("cannot connect")}")
        ok(_) => console.println("connected")
    }
}
"#;
    std::fs::write(dir.join("main.to"), program).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .args(["run", "main.to"])
        .current_dir(&dir)
        .env("WS", &url)
        .env("TONDO_RT_CHECK_LEAKS", "1")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(stdout, "some(echo: hola)\n[3, 2, 1]\necho: bye\nclosed none\ntrue\n", "{stderr}");
    assert!(output.status.success(), "{stderr}");
}

/// A Tondo program in a directory of its own, run with `TONDO_RT_CHECK_LEAKS`.
fn run(name: &str, program: &str, files: &[(&str, String)]) -> (String, String, bool) {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for (path, text) in files {
        let path = dir.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    std::fs::write(dir.join("main.to"), program).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_tondo"))
        .args(["run", "main.to"])
        .current_dir(&dir)
        .env("TONDO_RT_CHECK_LEAKS", "1")
        .output()
        .unwrap();
    let text = |bytes: &[u8]| String::from_utf8_lossy(bytes).into_owned();
    (text(&output.stdout), text(&output.stderr), output.status.success())
}

#[test]
fn server_routes_files_and_uploads() {
    let program = r#"import std.console
import std.encoding
import std.fs
import std.http
import std.io
import std.time

pub type NewUser = { name: String, email: String }
pub type User = { id: Int, name: String, email: String }

fn main(): !(http.HttpError | fs.FsError) {
    var app = http.App.new()
    app.use((request, next) {
        let reply = next(request)?
        reply.header("x-served-by", "tondo")
    })
    app.get("/users/:id", (request) {
        let id = request.intParam("id")?
        if id == 7 {
            fail http.Problem.notFound("no user 7")
        }
        http.Reply.json(User { id, name: "ada", email: "ada@example.com" })
    })
    app.get("/users/me", (request) { http.Reply.text("me") })
    app.post("/text", (request) { http.Reply.text(request.text()?) })
    app.post("/users", (request) {
        let input: NewUser = request.json()?
        http.Reply.json(User { id: 1, name: input.name, email: input.email }, status: 201)
    })
    app.get("/search", (request) {
        http.Reply.text("{request.query("q")} {request.queryValues("tag")} {request.intQuery("page")?}")
    })
    app.get("/boom", (request) {
        fail http.Problem.from("the database is down")
    })
    app.get("/crash", (_) {
        let empty: Array[String] = []
        http.Reply.text(empty[0])
    })
    app.get("/events", (request) {
        let (sender, receiver) = channel[http.Event](4)
        spawn detached produce(sender)
        http.Reply.events(receiver)
    })
    app.websocket("/chat", (socket) {
        for message in socket {
            socket.sendText("echo: {message?}")?
        }
    })
    var admin = http.App.new()
    admin.use((request, next) {
        match request.header("authorization") {
            some(_) => next(request)?
            none => fail http.Problem.unauthorized()
        }
    })
    admin.get("/stats", (request) { http.Reply.text("stats") })
    app.mount("/admin", admin)
    app.files("/files", "data")
    app.uploads("/files", "data")
    app.files("/browse", "data", listing: true)

    let server = app.start()?
    let client = http.Client.new(baseUrl: server.url())
    console.println(server.url())

    let ada = client.get("/users/42")?
    console.println("{ada.status()} {ada.header("x-served-by")} {ada.text()?}")
    console.println(client.get("/users/me")?.text()?)
    console.println("{client.get("/users/x")}")
    match client.get("/users/7") {
        err(http.HttpError.Status(response)) => console.println("{response.status()} {response.header("content-type")} {response.text()?}")
        other => console.println("unexpected {other}")
    }
    let created = client.post("/users", body: http.Body.json(NewUser { name: "bo", email: "bo@example.com" }))?
    console.println("{created.status()} {created.text()?}")
    console.println("{client.post("/users", body: http.Body.text("nope"))}")
    console.println(client.get("/search", query: ["q": "a b", "page": "2"])?.text()?)
    console.println(client.get("/search?tag=x&tag=y")?.text()?)
    let latin = encoding.hexDecode("61f16f2080").expect("hexadecimal")
    console.println(client.post("/text", body: http.Body.bytes(latin, contentType: "text/plain; charset=latin1"))?.text()?)
    console.println("{client.get("/boom")}")
    console.println("{client.get("/nowhere")}")
    console.println("{client.delete("/users/1")}")
    console.println("{client.get("/admin/stats")}")
    console.println(client.get("/admin/stats", headers: ["Authorization": "Bearer x"])?.text()?)

    for event in client.get("/events")?.events() {
        let e = event?
        console.println("event {e.event} {e.data} {e.id}")
    }

    let socket = http.WebSocket.connect("{server.url().replace("http://", "ws://")}/chat")?
    socket.sendText("hi")?
    console.println("{socket.receive()?}")
    socket.close()?

    let whole = client.get("/files/letters.txt")?
    console.println("{whole.header("accept-ranges")} {whole.header("content-type")} {whole.text()?}")
    let part = client.get("/files/letters.txt", headers: ["Range": "bytes=10-"])?
    console.println("{part.status()} {part.header("content-range")} {part.text()?}")
    let tail = client.get("/files/letters.txt", headers: ["Range": "bytes=-3"])?
    console.println("{tail.status()} {tail.text()?}")
    let etag = whole.header("etag").unwrapOr("")
    console.println("{client.get("/files/letters.txt", headers: ["If-None-Match": etag])?.status()}")
    console.println("{client.get("/files/letters.txt", headers: ["Range": "bytes=50-"])}")
    console.println("{client.get("/files/missing.txt")}")
    console.println("{client.get("/files/..%2Fsecret")}")
    let head = client.head("/files/letters.txt")?
    console.println("{head.header("content-length")}")

    let first = client.put("/files/up/notes.txt", body: http.Body.text("hello "))?
    console.println("{first.status()} {first.text()?}")
    let resumed = client.put("/files/up/notes.txt", body: http.Body.text("world"), headers: ["Content-Range": "bytes 6-10/11"])?
    console.println("{resumed.status()} {resumed.text()?}")
    console.println(client.get("/files/up/notes.txt")?.text()?)
    let form = http.Body.multipart([
        http.Part.field("note", "ignored"),
        http.Part.bytes("file", "abc".bytes(), fileName: "a.bin"),
        http.Part.bytes("file", "defg".bytes(), fileName: "b.bin"),
    ])
    console.println(client.post("/files/batch", body: form)?.text()?)
    console.println(client.get("/files/batch/b.bin")?.text()?)
    // A directory, listed: folders first, each file with its size and a
    // link to download it; a parent but at the top; none without `listing`.
    let top = client.get("/browse/")?.text()?
    let up = top.find("<a href=\"/browse/up\">up/</a>").unwrapOr(-1)
    let letters = top.find("<a href=\"/browse/letters.txt\">letters.txt</a></td><td class=\"size\">20 B</td>").unwrapOr(-1)
    console.println("{up >= 0} {letters > up} {top.contains("download>download")} {top.contains("../")}")
    let folder = client.get("/browse/up")?.text()?
    console.println("{folder.contains("<a href=\"/browse/\">../</a>")} {folder.contains("/browse/up/notes.txt")}")
    console.println("{client.get("/files/up")}")
    // A handler that panics answers 500, and the server goes on.
    console.println("{client.get("/crash")}")
    console.println(client.get("/users/me")?.text()?)
    server.stop()
}

fn produce(events: Sender[http.Event]) {
    events.send(http.Event { event: "tick", data: "one", id: some("1") })
    events.send(http.Event { event: "message", data: "two\nlines", id: none })
    events.close()
}
"#;
    let (stdout, stderr, success) = run("http-server", program, &[("data/letters.txt", "0123456789abcdefghij".into())]);
    let base = stdout.lines().next().unwrap_or_default().to_string();
    let expected = "BASE\n200 some(tondo) {\"id\":42,\"name\":\"ada\",\"email\":\"ada@example.com\"}\nme\nerr(GET BASE/users/x answered 400 Bad Request)\n404 some(application/problem+json) {\"status\":404,\"detail\":\"no user 7\"}\n201 {\"id\":1,\"name\":\"bo\",\"email\":\"bo@example.com\"}\nerr(POST BASE/users answered 400 Bad Request)\nsome(a b) [] some(2)\nnone [x, y] none\naño €\nerr(GET BASE/boom answered 500 Internal Server Error)\nerr(GET BASE/nowhere answered 404 Not Found)\nerr(DELETE BASE/users/1 answered 405 Method Not Allowed)\nerr(GET BASE/admin/stats answered 401 Unauthorized)\nstats\nevent tick one some(1)\nevent message two\nlines none\nsome(echo: hi)\nsome(bytes) some(text/plain) 0123456789abcdefghij\n206 some(bytes 10-19/20) abcdefghij\n206 hij\n304\nerr(GET BASE/files/letters.txt answered 416 Range Not Satisfiable)\nerr(GET BASE/files/missing.txt answered 404 Not Found)\nerr(GET BASE/files/..%2Fsecret answered 403 Forbidden)\nsome(20)\n201 [{\"path\":\"up/notes.txt\",\"size\":6}]\n200 [{\"path\":\"up/notes.txt\",\"size\":11}]\nhello world\n[{\"path\":\"batch/a.bin\",\"size\":3},{\"path\":\"batch/b.bin\",\"size\":4}]\ndefg\ntrue true true false\ntrue true\nerr(GET BASE/files/up answered 404 Not Found)\nerr(GET BASE/crash answered 500 Internal Server Error)\nme\n".replace("BASE", &base);
    assert_eq!(stdout, expected, "{stderr}");
    assert!(success, "{stderr}");
    assert!(stderr.contains("http: 500 the database is down"), "{stderr}");
    assert!(stderr.contains("panic[P0001]: bounds: index 0 is out of bounds: the array is empty\n"), "{stderr}");
    assert!(stderr.contains("  method: GET\n  path: /crash\n"), "{stderr}");
}

#[test]
fn server_over_tls_and_http2() {
    use rcgen::{BasicConstraints, CertificateParams, CertifiedIssuer, IsCa, KeyPair};
    let mut authority = CertificateParams::new(Vec::<String>::new()).unwrap();
    authority.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    let authority = CertifiedIssuer::self_signed(authority, KeyPair::generate().unwrap()).unwrap();
    let key = KeyPair::generate().unwrap();
    let names = vec!["localhost".to_string(), "127.0.0.1".to_string()];
    let certificate = CertificateParams::new(names).unwrap().signed_by(&key, &authority).unwrap();
    let program = r#"
import std.console
import std.http

fn main(): !http.HttpError {
    var app = http.App.new()
    app.get("/", (request) { http.Reply.text("secure {request.version()}") })
    let server = app.start(tls: http.Tls.files("server.pem", "server.key"))?
    let client = http.Client.new(trust: "authority.pem")
    let response = client.get(server.url())?
    console.println("{server.url().startsWith("https://")} {response.version()} {response.text()?}")
    console.println("{"{http.get(server.url())}".startsWith("err(cannot connect")}")
    console.println("{"{http.Client.new(trust: "missing.pem").get(server.url())}".startsWith("err(cannot connect: the trusted certificates `missing.pem`")}")
    server.stop()
}
"#;
    let files =
        [("authority.pem", authority.pem()), ("server.pem", certificate.pem()), ("server.key", key.serialize_pem())];
    let (stdout, stderr, success) = run("http-tls", program, &files);
    assert_eq!(stdout, "true HTTP/2.0 secure HTTP/2.0\ntrue\ntrue\n", "{stderr}");
    assert!(success, "{stderr}");
}

#[test]
fn typed_handlers_and_openapi() {
    let program = r##"import std.console
import std.http

pub type NewUser = { name: String, email: String? }
pub type User = { id: Int, name: String, email: String?, role: Role }
pub enum Role {
    Admin
    Member
}

pub enum StoreError {
    Down
}

impl Display for StoreError {
    fn display(self): String {
        "the store is down"
    }
}

fn find(id: Int): User? {
    if id == 7 { none } else { some(User { id, name: "ada", email: none, role: Role.Admin }) }
}

fn listUsers(page: Int?, search: String): Array[User] {
    [User { id: page.unwrapOr(1), name: search, email: none, role: Role.Member }]
}

fn main(): !http.HttpError {
    var app = http.App.new()
    app.get("/users/:id", (id: Int) { find(id) })
    app.get("/users", listUsers)
    app.post("/users", (input: NewUser, request: http.Request) {
        User { id: 2, name: input.name, email: input.email, role: Role.Member }
    })
    app.delete("/users/:id", (id: Int) {
        console.println("deleting {id}")
    })
    app.get("/hello/:name", (name: String) { "hello {name}" })
    app.get("/store", (): String ! StoreError {
        fail StoreError.Down
    })
    app.get("/raw", (request) { http.Reply.text("raw {request.path()}") })
    app.openApi(title: "Users", version: "2.0")
    let server = app.start()?
    let client = http.Client.new(baseUrl: server.url())
    console.println(server.url())
    console.println(client.get("/users/3")?.text()?)
    console.println("{client.get("/users/7")}")
    console.println("{client.get("/users/x")}")
    console.println(client.get("/users", query: ["page": "4", "search": "bo"])?.text()?)
    console.println("{client.get("/users")}")
    console.println(client.post("/users", body: http.Body.json(NewUser { name: "cy", email: some("cy@x") }))?.text()?)
    console.println("{client.delete("/users/9")?.status()}")
    console.println(client.get("/hello/world")?.text()?)
    console.println("{client.get("/store")}")
    console.println(client.get("/raw")?.text()?)
    console.println(client.get("/openapi.json")?.text()?)
    server.stop()
}
"##;
    let (stdout, stderr, success) = run("http-typed", program, &[]);
    let base = stdout.lines().next().unwrap_or_default().to_string();
    let expected = r##"BASE
{"id":3,"name":"ada","email":null,"role":"Admin"}
err(GET BASE/users/7 answered 404 Not Found)
err(GET BASE/users/x answered 400 Bad Request)
[{"id":4,"name":"bo","email":null,"role":"Member"}]
err(GET BASE/users answered 400 Bad Request)
{"id":2,"name":"cy","email":"cy@x","role":"Member"}
deleting 9
204
hello world
err(GET BASE/store answered 500 Internal Server Error)
raw /raw
{"openapi":"3.1.0","info":{"title":"Users","version":"2.0"},"paths":{"/users/{id}":{"get":{"parameters":[{"name":"id","in":"path","required":true,"schema":{"type":"integer"}}],"responses":{"400":{"description":"Bad Request","content":{"application/problem+json":{"schema":{}}}},"200":{"description":"OK","content":{"application/json":{"schema":{"$ref":"#/components/schemas/User"}}}},"404":{"description":"Not Found","content":{"application/problem+json":{"schema":{}}}}}},"delete":{"parameters":[{"name":"id","in":"path","required":true,"schema":{"type":"integer"}}],"responses":{"400":{"description":"Bad Request","content":{"application/problem+json":{"schema":{}}}},"204":{"description":"No Content"}}}},"/users":{"get":{"parameters":[{"name":"page","in":"query","required":false,"schema":{"type":"integer"}},{"name":"search","in":"query","required":true,"schema":{"type":"string"}}],"responses":{"400":{"description":"Bad Request","content":{"application/problem+json":{"schema":{}}}},"200":{"description":"OK","content":{"application/json":{"schema":{"type":"array","items":{"$ref":"#/components/schemas/User"}}}}}}},"post":{"requestBody":{"required":true,"content":{"application/json":{"schema":{"$ref":"#/components/schemas/NewUser"}}}},"responses":{"400":{"description":"Bad Request","content":{"application/problem+json":{"schema":{}}}},"200":{"description":"OK","content":{"application/json":{"schema":{"$ref":"#/components/schemas/User"}}}}}}},"/hello/{name}":{"get":{"parameters":[{"name":"name","in":"path","required":true,"schema":{"type":"string"}}],"responses":{"200":{"description":"OK","content":{"text/plain":{"schema":{"type":"string"}}}}}}},"/store":{"get":{"responses":{"default":{"description":"A problem (RFC 9457)","content":{"application/problem+json":{"schema":{}}}},"200":{"description":"OK","content":{"text/plain":{"schema":{"type":"string"}}}}}}},"/raw":{"get":{"responses":{"default":{"description":"The reply"}}}}},"components":{"schemas":{"Role":{"type":"string","enum":["Admin","Member"]},"User":{"type":"object","properties":{"id":{"type":"integer"},"name":{"type":"string"},"email":{"anyOf":[{"type":"string"},{"type":"null"}]},"role":{"$ref":"#/components/schemas/Role"}},"required":["id","name","role"]},"NewUser":{"type":"object","properties":{"name":{"type":"string"},"email":{"anyOf":[{"type":"string"},{"type":"null"}]}},"required":["name"]}}}}
"##.replace("BASE", &base);
    assert_eq!(stdout, expected, "{stderr}");
    assert!(success, "{stderr}");
}
