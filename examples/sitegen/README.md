# sitegen: a static site generator

Reads a directory of pages written in Markdown, each with what it says of
itself in YAML at its top, and writes a site of HTML: a page for each one,
the list of them all, and a list for each tag. It is about 260 lines of
Tondo, and the templates, the YAML, the file systems and the tasks came
with `tondo`.

```text
sitegen/
  tondo.toml
  site/
    content/*.md                  the pages, with their front matter
    theme/base.html               the page every other one extends
    theme/post.html               a page
    theme/index.html              a list of pages
  src/
    main.to                       the command line
    content/content.to            a page: its front matter and its text
    content/content_test.to
    build/build.to                the build: load, render, write
    build/build_test.to           the build tested on a site in memory
```

## Run it

```sh
cd examples/sitegen
tondo run -- site -o public
tondo run -- --help
tondo test
```

```text
$ tondo run -- site -o public
  public/posts/values.html
  public/posts/hello-tondo.html
  public/index.html
  public/tags/language.html
  public/tags/first.html
built 2 pages, left out 1 drafts
```

`site/content/hello-tondo.md` is

```text
---
title: Hello, Tondo
date: 2026-11-10
tags: [language, first]
---
# A first post

This site is written by a program of under three hundred lines.
```

and `public/posts/hello-tondo.html` begins

```text
<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <title>Hello, Tondo · A Tondo site</title>
</head>
<body>
  <header><a href="/">A Tondo site</a></header>
  <main>
    <article>
      <h1>Hello, Tondo</h1>
      <p class="meta">2026-11-10 · <a href="/tags/language.html">language</a> · <a href="/tags/first.html">first</a></p>
      <h2>A first post</h2>
<p>This site is written by a program of under three hundred lines.</p>
```

A page that is not one, or a template that names what its data does not
have, stops the build and says where:

```text
$ tondo run -- site -o public
the site was not built: broken.md: a page starts with its title and date between two lines of `---`

$ tondo run -- site -o public
the site was not built: post.html:6:26: unknown name `dat`: it is no binding, nor a field of the data, `build.PostView`, whose fields are `site`, `title`, `date`, `tags`, `body`
```

## What to look at

**Front matter is a type.** `Meta` in
[`content.to`](src/content/content.to) is a record with defaults, and
`yaml.decode[Meta](front)` reads it. A page without a title, or whose `tags`
is not a list, fails there with the line of the YAML, and the rest of the
program only ever sees pages that are whole.

**Templates in Tondo's own expressions.** The theme is loaded as the
program runs, into a `template.Set`, since a theme is data of the site and
not of the program. What a template may say is still Tondo:

```text
{{ for tag in tags }} · <a href="/tags/{{ tag }}.html">{{ tag }}</a>{{ end }}
```

and each one is checked against the type it is rendered with, `PostView` or
`IndexView`: a wrong name is an error with the file, the line and the
fields there are, as above. `post.html` and `index.html` extend
`base.html` and fill its blocks.

**Escaping is by type.** Everything a template writes is escaped for
where it lands in the HTML. The body of a page is already HTML, made by
`content.html`, so `PostView.body` is a `template.Html`, the one type a
template writes as it is. Nothing is marked safe inside a template, where
it would be easy to mark too much.

**A file system is a value.** `build.build(site, out, name)` takes two
`fs.FileSystem`s. `main` gives it two directories, `fs.disk(...)`, and
[`build_test.to`](src/build/build_test.to) gives it `fs.memory()`: the
whole build is tested with no temporary directory and nothing to clean.
`site.sub("theme")` is the view of one directory that the templates are
loaded from, and they cannot name a file outside it.

**A task for each page.** The pages are rendered at once, each in a task
added to a `Group`, and `rendering.wait()` gives their HTML in the order
they were added. The first that fails cancels the others, and its error is
the build's. The `template.Set` is shared by all of them with no lock,
since rendering only reads it.

**One error for the whole build.** `BuildError` is an enum of what can go
wrong, a variant for each module it calls, and `mapErr` says which at each
call by the variant's name alone, `.mapErr(BuildError.Files)`: a variant is
the function that wraps its data. `main` matches on the result once, prints it, and exits with 1.

**The command line is a record.** `Sitegen` in [`main.to`](src/main.to)
has a field for each argument and option, with its default and its
documentation, and `cli.parse[Sitegen]()` gives the help, the errors and
the values.

## Going further

- `fs.watch` gives the changes of a directory as they happen: a loop over
  it that calls `build` is a `--serve` mode in a dozen lines.
- `std.markdown` is not there, which is why `content.html` writes the few
  forms these pages use. A real site would want the whole of it.
