# Templates

`std.template` writes text and HTML from templates: text with actions
between `{{` and `}}` whose expressions are Tondo's. A template whose text
the compiler sees, a raw string or `embed.text("...")`, compiles with the
program and is checked against the type of its data; a `template.Set`
loads templates while the program runs.

```tondo
import std.console
import std.template

pub type Post = { title: String, views: Int, tags: Array[String], draft: Bool, author: String? }
pub type Blog = { name: String, posts: Array[Post] }

fn main() {
    let page = template.html[Blog](r"""
        <h1>{{ name }}</h1>
        {{ for post in posts }}
          {{ if not post.draft }}
          <h2>{{ post.title }}</h2>
          <p>{{ post.views:>6 }} views · {{ post.tags.join(", ") }}</p>
          {{ with author = post.author }}<p>by {{ author }}</p>{{ end }}
          {{ end }}
        {{ else }}
        <p>No posts yet.</p>
        {{ end }}
        """)
    let post = Post { title: "Tea & cake", views: 42, tags: ["a", "b"], draft: false, author: some("Ada") }
    console.print(page.render(Blog { name: "Notes", posts: [post] }))
}
```

- A name is a variable of `let`, `for` or `with`, else a field of the
  data; `self` is the data. A misspelt field is a compile error that lists
  the fields.
- A condition is a `Bool`, as in code: `{{ if not posts.isEmpty() }}`. An
  optional opens with `{{ with x = optional }}`; an empty `for` writes its
  `{{ else }}`. `{{ let n = posts.length() }}`, `{{ break }}`, `{{ continue }}`.
- `{{ price:.2 }}` writes with a format, as interpolation does.
- A line that holds only blocks (`if`, `for`, `end`, `let`...) vanishes,
  with its line break; `{{-` and `-}}` trim the space around an action. A
  file's last line break is not written.
- A literal template is a raw string, `r"..."` or `r"""..."""`: in a plain
  string, `{{` is one brace.
- In files: `template.html[Page](embed.text("templates/page.html"))`. The
  page starts with `{{ extends "base.html" }}` and redefines the base's
  `{{ block "content" }}...{{ end }}`; `{{ include "card.html" post }}`
  writes another with that data. They are read next to it when compiling.
- A compiled template calls the functions and modules its file sees:
  `{{ slug(post.title) }}`, `{{ markdown.render(body) }}`.
- HTML templates escape each value for where it goes: text, attributes,
  URLs (a `javascript:` link becomes `about:invalid`), scripts (as JSON)
  and styles. `template.Html { value: "<b>x</b>" }` is written as it is.

## Templates loaded while running

```tondo
import std.console
import std.template

pub type Post = { title: String, tags: Array[String] }

pub type Helpers = {}

pub fn Helpers.shout(self, word: String): String {
    "{word.toUpper()}!"
}

fn main(): !template.TemplateError {
    var site = template.Set.html()
    site.add("base.html", r"""<main>{{ block "main" }}{{ end }}</main>""")?
    site.add("post.html", r"""{{ extends "base.html" }}{{ block "main" }}<h1>{{ shout(title) }}</h1>{{ end }}""")?
    site.functions(Helpers {})
    console.println(site.render("post.html", Post { title: "tea", tags: ["a"] })?)
}
```

- `site.addFile(path)`, and `site.addFiles(fs.disk("themes/simple"),
  "**/*.html")` or `site.addFiles(embed.files("templates"))`, named by their
  paths in the tree.
- A `Set` sees its data by reflection, with the names the code uses: fields
  and `pub` methods (`{{ self.summary() }}`), and these of the prelude:
  of `String` `length`, `isEmpty`, `contains`, `startsWith`, `endsWith`,
  `find`, `replace`, `split`, `lines`, `words`, `trim`, `trimStart`,
  `trimEnd`, `toUpper`, `toLower`, `padStart`, `padEnd`, `repeat`,
  `removePrefix`, `removeSuffix`; of `Array` `length`, `isEmpty`, `first`,
  `last`, `get`, `take`, `drop`, `reversed`, `enumerate`, `join`, `concat`;
  of `Map` `length`, `isEmpty`, `contains`, `get`, `keys`, `values`,
  `entries`; of optionals `isSome`, `isNone`, `unwrapOr`.
- Its functions are the `pub` methods of the values given to
  `site.functions(...)`.
- Its errors, `template:line:column: message`, come when it writes; a
  template whose text is a literal or embedded is read when compiling too.

## From Go's templates

```text
{{ .Title }}  {{ . }}                  {{ title }}  {{ self }}
{{ range $i, $p := .Posts }}           {{ for (i, p) in posts.enumerate() }}
{{ with .Author }}                     {{ with author = author }}
{{ if and (eq .A "x") .B }}            {{ if a == "x" and b }}
{{ len .Items }}  {{ index .M "k" }}   {{ items.length() }}  {{ m["k"] }}
{{ printf "%.2f" .Price }}             {{ price:.2 }}
{{ .Title | upper }}                   {{ title.toUpper() }}
{{ template "card" . }}                {{ include "card" self }}
{{ define "main" }} / {{ block "main" . }}
                                       {{ extends "base" }} / {{ block "main" }}
{{/* note */}}                         {{// note }}
```
