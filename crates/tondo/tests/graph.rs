//! `tondo graph` (toolchain 7.11): the packages, modules and files of a
//! program and what each imports, from a project, a directory or a file.

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;

fn tondo(dir: &Path, args: &[&str]) -> (String, String, i32) {
    let output = Command::new(env!("CARGO_BIN_EXE_tondo")).args(args).current_dir(dir).output().unwrap();
    let text = |bytes: &[u8]| String::from_utf8_lossy(bytes).into_owned();
    (text(&output.stdout), text(&output.stderr), output.status.code().unwrap_or(-1))
}

fn write(path: PathBuf, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// A project that imports one of its modules and a dependency by another
/// name, which imports a module of its own; in a directory of its own for
/// each test.
fn project(name: &str) -> PathBuf {
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let root = root.canonicalize().unwrap();
    write(
        root.join("app/tondo.toml"),
        "[package]\nname = \"app\"\n\n[dependencies]\nfigures = { path = \"../shapes\" }\n",
    );
    write(
        root.join("app/src/main.to"),
        "import std.console\nimport app.models\nimport figures\n\nfn main() {\n    console.println(\"{models.name()} {figures.area(1.0)}\")\n}\n",
    );
    write(root.join("app/src/models/user.to"), "pub fn name(): String {\n    return \"x\"\n}\n");
    write(
        root.join("app/src/models/role.to"),
        "import std.embed\nimport std.json\n\npub const Roles: String = embed.text(\"roles.csv\")\n\npub fn role(): Int {\n    return 1\n}\n",
    );
    write(root.join("app/src/models/roles.csv"), "admin\nmember\n");
    write(root.join("app/src/models/user_test.to"), "test named {\n    assert(name() == \"x\")\n}\n");
    write(root.join("shapes/tondo.toml"), "[package]\nname = \"shapes\"\n");
    write(
        root.join("shapes/src/shapes.to"),
        "import shapes.geo\n\npub fn area(r: Float): Float {\n    return geo.pi() * r * r\n}\n",
    );
    write(root.join("shapes/src/geo/geo.to"), "pub fn pi(): Float {\n    return 3.14\n}\n");
    root
}

#[test]
fn shows_modules_packages_and_files() {
    let root = project("graph-views");
    let app = root.join("app");
    let (stdout, stderr, status) = tondo(&app, &["graph"]);
    assert_eq!(status, 0, "{stderr}");
    assert_eq!(
        stdout,
        "\
app  src/main.to
  -> std.console
  -> app.models
  -> shapes
app.models  src/models/role.to src/models/user.to
  -> std.embed
  -> std.json
  embeds src/models/roles.csv
shapes  shapes/src/shapes.to
  -> shapes.geo
shapes.geo  shapes/src/geo/geo.to
std.console
std.embed
std.json
"
    );
    let (stdout, _, _) = tondo(&app, &["graph", "--packages"]);
    let dir = |name: &str| root.join(name).display().to_string();
    assert_eq!(stdout, format!("app  {}\n  -> shapes as figures\nshapes  {}\n", dir("app"), dir("shapes")));
    let (stdout, _, _) = tondo(&app, &["graph", "--files"]);
    assert_eq!(
        stdout,
        "\
src/main.to
  -> std.console
  -> src/models/role.to
  -> src/models/user.to
  -> shapes/src/shapes.to
src/models/role.to
  -> std.embed
  -> std.json
src/models/user.to
shapes/src/shapes.to
  -> shapes/src/geo/geo.to
shapes/src/geo/geo.to
"
    );
}

#[test]
fn narrows_to_a_module_and_finds_the_project_of_a_path() {
    let root = project("graph-paths");
    let app = root.join("app");
    // What a module reaches, and what reaches it.
    let (stdout, _, _) = tondo(&app, &["graph", "--module", "shapes"]);
    assert_eq!(stdout, "shapes  shapes/src/shapes.to\n  -> shapes.geo\nshapes.geo  shapes/src/geo/geo.to\n");
    let (stdout, _, _) = tondo(&app, &["graph", "--module", "std.json", "--reverse"]);
    assert_eq!(
        stdout,
        "app  src/main.to\n  -> app.models\napp.models  src/models/role.to src/models/user.to\n  -> std.json\n  embeds src/models/roles.csv\nstd.json\n"
    );
    // A directory or file of the sources is its module, in its project.
    let (stdout, _, _) = tondo(&root, &["graph", "app/src/models/user_test.to"]);
    assert_eq!(
        stdout,
        "app.models  src/models/role.to src/models/user.to\n  -> std.embed\n  -> std.json\n  embeds src/models/roles.csv\nstd.embed\nstd.json\n"
    );
    let (stdout, stderr, status) = tondo(&root, &["graph", "app/src/models", "--packages", "--json"]);
    assert_eq!(status, 0, "{stderr}");
    let graph: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(graph["format"], "tondo-graph-0.1/1");
    assert_eq!(
        (&graph["root"], &graph["project"], &graph["focus"]),
        (&Value::from(app.display().to_string()), &true.into(), &"app.models".into())
    );
    assert_eq!(graph["packages"][0]["dependencies"], serde_json::json!([{ "alias": "figures", "package": "shapes" }]));
    assert!(graph.get("modules").is_none(), "{graph}");
    // Another file of the project: the project, without a module.
    write(app.join("tests/api.to"), "test api {}\n");
    let (stdout, _, _) = tondo(&root, &["graph", "app/tests/api.to", "--json"]);
    let graph: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!((&graph["focus"], graph["modules"].as_array().unwrap().len()), (&Value::Null, 7));
    assert_eq!(
        graph["modules"][1]["embeds"],
        serde_json::json!([{ "path": app.join("src/models/roles.csv"), "file": app.join("src/models/role.to"), "line": 4 }])
    );
    let main = &graph["modules"][0];
    assert_eq!(main["files"], serde_json::json!([app.join("src/main.to")]));
    assert_eq!(
        main["imports"][2],
        serde_json::json!({ "module": "shapes", "file": app.join("src/main.to"), "line": 3 })
    );

    // A file on its own.
    write(root.join("alone/hello.to"), "import std.console\n\nfn main() {\n    console.println(\"hi\")\n}\n");
    let (stdout, _, _) = tondo(&root, &["graph", "alone/hello.to"]);
    assert_eq!(stdout, "hello  hello.to\n  -> std.console\nstd.console\n");
    let (stdout, _, _) = tondo(&root, &["graph", "alone/hello.to", "--json"]);
    let graph: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(graph["project"], false);

    let (_, stderr, status) = tondo(&app, &["graph", "--module", "nothing"]);
    assert_eq!(status, 3);
    assert!(stderr.contains("the program has no module `nothing`; its modules are app, app.models"), "{stderr}");
    let (_, stderr, status) = tondo(&root, &["graph", "alone"]);
    assert_eq!(
        (status, stderr.trim()),
        (
            3,
            format!("tondo: {} is not in a Tondo project: add src/main.to or main.to", root.join("alone").display())
                .as_str()
        )
    );
    let (_, stderr, status) = tondo(&app, &["graph", "--reverse"]);
    assert_eq!(status, 2);
    assert!(stderr.starts_with("tondo: --reverse goes with --module <module>\nusage:\n  tondo graph"), "{stderr}");
}

#[test]
fn draws_with_graphviz_and_mermaid() {
    let app = project("graph-drawn").join("app");
    let (stdout, _, _) = tondo(&app, &["graph", "--module", "shapes", "--dot"]);
    assert_eq!(
        stdout,
        "\
digraph tondo {
    rankdir=LR;
    node [shape=box];
    subgraph cluster_0 {
        label=\"shapes\";
        \"shapes\";
        \"shapes.geo\";
    }
    \"shapes\" -> \"shapes.geo\";
}
"
    );
    let (stdout, _, _) = tondo(&app, &["graph", "--packages", "--mermaid"]);
    assert_eq!(stdout, "flowchart LR\n    n0[\"app\"]\n    n1[\"shapes\"]\n    n0 --> n1\n");
    let (stdout, _, _) = tondo(&app, &["graph", "--module", "app.models", "--mermaid"]);
    assert_eq!(
        stdout,
        "\
flowchart LR
    subgraph p0[\"app\"]
        n0[\"app.models\"]
    end
    subgraph p1[\"std\"]
        n1[\"std.embed\"]
        n2[\"std.json\"]
    end
    n0 --> n1
    n0 --> n2
"
    );
}
