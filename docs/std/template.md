# std.template

text and HTML written from templates, text with actions between `{{` and `}}` whose expressions are Tondo's. A template whose text is known when the program compiles is compiled with it into code; a `Set` loads templates while the program runs, and writes them with the values its data has by reflection.

```tondo
import std.console
import std.template

pub type Post = { title: String, tags: Array[String] }

var site = template.Set.html()
site.add("post.html", r"""<h1>{{ title }}</h1>{{ for tag in tags }}<em>{{ tag }}</em>{{ end }}""")?
console.println(site.render("post.html", Post { title: "Tea & cake", tags: ["food"] })?)
```

<!-- From here on, docs/std/generate.to writes this page from `tondo api`: edit above this line. -->

## Reference

`tondo api std.template` shows the same in a terminal.

- [Functions](#functions): [`html`](#html), [`text`](#text)
- [Template](#template): [`Template.render`](#templaterender), [`Template.write`](#templatewrite)
- [Html](#html-1)
- [TemplateError](#templateerror)
- [Set](#set): [`Set.html`](#sethtml), [`Set.text`](#settext), [`Set.add`](#setadd), [`Set.addFile`](#setaddfile), [`Set.addFiles`](#setaddfiles), [`Set.functions`](#setfunctions), [`Set.render`](#setrender), [`Set.write`](#setwrite)

### Functions

#### html

```tondo
pub fn html[T](source: String): Template[T]
```

The template of HTML `source`, compiled with the program: a raw string,
`r"..."`, or `embed.text("...")`. It escapes each value for the place of
HTML where it goes.

#### text

```tondo
pub fn text[T](source: String): Template[T]
```

The text template `source`, compiled with the program, which escapes
nothing.

### Template

```tondo
pub type Template[T] = {
    // private fields
}
```

A template compiled with the program, which writes a `T`.

```tondo
import std.console
import std.template

pub type Post = { title: String, tags: Array[String] }

let page = template.html[Post](r"""<h1>{{ title }}</h1>{{ for tag in tags }}<em>{{ tag }}</em>{{ end }}""")
console.println(page.render(Post { title: "Tea & cake", tags: ["food", "<b>"] }))
```

#### Template.render

```tondo
pub fn Template[T].render(self, data: T): String
```

What the template writes for `data`.

#### Template.write

```tondo
pub fn Template[T].write(self, data: T, out: io.Writer): !io.IoError
```

Writes what the template writes for `data` to `out`.

### Html

```tondo
pub type Html = {
    value: String
}
```

HTML that the program trusts, which a template writes as it is in the
text of HTML, and escapes anywhere else.

```tondo
import std.console
import std.template

pub type Page = { title: String, body: template.Html }

let page = template.html[Page](r"""<h1>{{ title }}</h1>{{ body }}""")
let body = template.Html { value: "<p>Written by the program, <b>trusted</b>.</p>" }
console.println(page.render(Page { title: "<Tea>", body: body }))
```

Implements `Display`, `reflect.Reflect`, `serialization.Encode`, `serialization.Decode`.

### TemplateError

```tondo
pub type TemplateError = {
    message: String
    template: String
    line: Int
    column: Int
}
```

Why a template does not read, or could not be written: what, in which
template, and where.

```tondo
import std.console
import std.template

pub type Post = { title: String }

var site = template.Set.html()
site.add("post.html", r"""<h1>{{ title }}</h1><p>{{ author }}</p>""")?
match site.render("post.html", Post { title: "Tea" }) {
    ok(page) => console.println(page)
    err(error) => console.println("{error.template} at {error.line}:{error.column}: {error.message}")
}
```

Implements `Display`, `reflect.Reflect`.

### Set

```tondo
pub type Set = {
    // private fields
}
```

Templates loaded while the program runs, by name, as the themes of a
site generator. They see their data by reflection, with the names the
code uses, and call the `pub` methods of the values given with
`functions` as functions.

```tondo
import std.console
import std.template

pub type Page = { title: String, items: Array[String] }

var site = template.Set.html()
site.add("base.html", r"""<main>{{ block "main" }}{{ end }}</main>""")?
site.add("list.html", r"""{{ extends "base.html" }}{{ block "main" }}<h1>{{ title }}</h1>{{ for item in items }}<li>{{ item }}</li>{{ end }}{{ end }}""")?
console.println(site.render("list.html", Page { title: "Today", items: ["tea", "<cake>"] })?)
```

#### Set.html

```tondo
pub fn Set.html(): Set
```

An empty set of HTML templates, which escape each value for the place of
HTML where it goes.

#### Set.text

```tondo
pub fn Set.text(): Set
```

An empty set of text templates, which escape nothing.

#### Set.add

```tondo
pub fn Set.add(mut self, name: String, source: String): !TemplateError
```

Adds the template `source` as `name`, replacing one of that name, and
each template it defines with `define`, by its name.

#### Set.addFile

```tondo
pub fn Set.addFile(mut self, path: String): !TemplateError
```

Adds the template in the file at `path`, named by its path.

#### Set.addFiles

```tondo
pub fn Set.addFiles(mut self, files: fs.FileSystem, pattern: String = "**"): !TemplateError
```

Adds the templates of a tree that a pattern of `path.matches` takes,
each named by its path in the tree: `site.addFiles(fs.disk("themes/simple"),
"**/*.html")`, or `site.addFiles(embed.files("templates"))`.

#### Set.functions

```tondo
pub fn Set.functions[H: reflect.Reflect](mut self, helpers: H)
```

Makes the `pub` methods of `helpers` functions of the templates:
`{{ slug(title) }}` calls `helpers.slug(title)`.

#### Set.render

```tondo
pub fn Set.render[T: reflect.Reflect](self, name: String, data: T): String ! TemplateError
```

What the template `name` writes for `data`.

#### Set.write

```tondo
pub fn Set.write[T: reflect.Reflect](
    self,
    name: String,
    data: T,
    out: io.Writer,
): !TemplateError
```

Writes what the template `name` writes for `data` to `out`.
