# linkcheck: a concurrent link checker

Starts at a page of a site, follows every link to the pages of the same
site, and asks whether what they link to elsewhere is there. It keeps a
fixed number of requests out at a time and no more than so many a second,
and says which links lead nowhere and which page has them. About 360 lines
of Tondo, and the HTTP client, the URLs, the patterns, the tasks and the
rate limiter came with `tondo`.

```text
linkcheck/
  tondo.toml
  demo/                           a small site with four links that are broken
  src/
    main.to                       the command line, and the deadline
    links/links.to                the links of a page, and what they point to
    links/links_test.to
    crawl/crawl.to                the crawl: one task that decides, workers that ask
  tests/crawl.to                  the crawl against servers started by the tests
```

## Run it

```sh
tondo run examples/scripts/serve.to -- examples/linkcheck/demo 8765 &
tondo run examples/linkcheck -- http://localhost:8765
tondo test examples/linkcheck
```

```text
$ tondo run examples/linkcheck -- http://localhost:8765
checked 10 addresses in 14 ms: 5 pages read, 4 links that lead nowhere
┌──────────────────────────────────────────┬────────────────────────────────────────────────────────────────┬──────────────────────────────────────────┐
│ link                                     │ answer                                                         │ linkedFrom                               │
├──────────────────────────────────────────┼────────────────────────────────────────────────────────────────┼──────────────────────────────────────────┤
│ http://localhost:8765/guide/upgrade.html │ 404                                                            │ http://localhost:8765/guide/install.html │
│ http://localhost:8765/images/diagram.png │ 404                                                            │ http://localhost:8765/guide.html         │
│ http://localhost:8765/pricing.html       │ 404                                                            │ http://localhost:8765                    │
│ http://localhost:9/status                │ cannot connect: 127.0.0.1:9: Connection refused (os error 111) │ http://localhost:8765                    │
└──────────────────────────────────────────┴────────────────────────────────────────────────────────────────┴──────────────────────────────────────────┘

$ echo $?
1
```

It exits with 1 when a link leads nowhere, so a build can stop on it, and
with 2 when the site could not be checked at all:

```text
$ tondo run examples/linkcheck -- localhost:8765
the site was not checked: `localhost:8765` is not an address of the web, as `https://example.com`

$ tondo run examples/linkcheck -- http://localhost:8765 --workers 1 --rate 2 --deadline 1s
the site was not checked: timed out after 1s
```

`--json` writes every address that was asked for, for another program:

```text
$ tondo run examples/linkcheck -- http://localhost:8765 --json
{
  "checked": [
    {
      "url": "http://localhost:8765",
      "from": "",
      "outcome": {
        "Fine": 200
      },
      "ms": 2
    },
    ...
```

## What to look at

**One task decides, the others ask.** `crawl` in
[`crawl.to`](src/crawl/crawl.to) starts the workers and then runs a loop of
its own. It alone holds the set of addresses already seen and the count of
answers still to come. The workers hold neither: they take a `Job` from one
channel and send a `Visit` to another.

```text
for out > 0 {
    let answer = match inbox.receive() { ... }
    out -= 1
    for link in answer.links {
        if seen.contains(link) { continue }
        _ = seen.insert(link)
        out += 1
        jobs.send(Job { url: link, from: answer.checked.url, inside })
    }
}
```

With one owner there is no lock, and no way for two workers to ask for the
same address. The crawl is over when nothing is out, which only the owner of
the count can know.

**The workers are a `Group`.** Eight tasks run the same function over the
same `Receiver`, and each job goes to one of them. When the loop ends,
`jobs.close()` ends theirs, and `workers.wait()` returns once every one has
gone. `crawl` leaves no task behind: that is what a function returning means
in Tondo.

**A rate limit is a value they share.** `sync.RateLimiter(options.rate)` is
given to every worker, and `pace.wait()` before each request keeps all of
them together under the limit. The HTTP client is shared the same way, with
its connections.

**Every way to fail has a name.** A request ends as one of three `Outcome`s,
and the `match` that builds it has an arm for each kind of `http.HttpError`
that matters here: a status that says no, no answer in time, and everything
else with its own words. Nothing is caught, and nothing can be forgotten:
a new variant of the error would have to be given an arm or fall in the last
one on purpose.

**A deadline is not a parameter.** In [`main.to`](src/main.to),

```text
let result = within options.deadline.unwrapOr(time.hours(24)) {
    crawl.crawl(options.url, settings)
}
```

gives the whole crawl a time limit. No function of `crawl.to` knows about
it: when the time passes, the loop's `receive` ends, the workers are
cancelled in the middle of their requests, and `within` gives
`err(time.TimedOut)`.

**What a module sets up once is a context.** The pattern that finds links
is compiled the first time [`links.to`](src/links/links.to) needs it, as the
initial value of a field of `context Patterns`, and every task shares it
after that. There is no global variable and nothing to initialize in `main`.

**Addresses are resolved, not glued.** `page.resolve(link)` gives what
`../index.html`, `//cdn.example.com/a.js` or `/about#team` mean on the page
they are written in, by the rules of RFC 3986. The tests of
[`links_test.to`](src/links/links_test.to) are the table of them.

**Tests against real servers.** [`tests/crawl.to`](tests/crawl.to) starts
two servers on free ports for each test, a site and another one it links
to, and crawls them over HTTP. One test counts the requests the server is
answering at a time and holds the crawl to its number of workers; another
keeps what the other site was asked, which must be `HEAD` and only for the
addresses linked to.

## Going further

- `TONDO_DIAGNOSTICS=1 tondo run examples/linkcheck -- <url>` and, from
  another terminal, `tondo ps` names the running program and
  `tondo blocked <name>` shows its tasks that wait, and what each waits for.
- The `sitegen` example writes a site that this one can check: build it,
  serve `public/` with `examples/scripts/serve.to`, and run `linkcheck` on
  it in the script that publishes.
