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
