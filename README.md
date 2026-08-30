# orgo

Turn a folder of org files into a website.

You write posts the way you already do — an `.org` file per page, in whatever directory
structure suits you — and orgo builds a complete site from them: pages, navigation, a blog
index, tags, an RSS feed, syntax-highlighted code. It is one binary with nothing to
install alongside it, and **you do not need Emacs to build your site**, only to write in a
format Emacs made.

Org is the source language here, not something to convert away from first. Tools that
route org through markdown lose what markdown has no words for — property drawers, a
heading's TODO state and tags, `#+` keywords, ID links, captions on images. orgo keeps all
of it, and its output is checked page by page against what Emacs' own exporter produces
from the same file.

**Documentation: <https://krazywarez.github.io/orgo/>** — that site is written in org and
built by orgo, so it doubles as the longest worked example available.

## Install

You need [Rust](https://rustup.rs) (1.88 or newer). Nothing else — syntax highlighting and
its themes are compiled in.

```sh
cargo install orgo
```

Or from a checkout, if you want to build the version you can read:

```sh
git clone https://gitbay.org/krz/orgo
cd orgo
cargo install --path .
```

Either way you get an `orgo` command on your `PATH`. Full notes, including how to run it
without installing anything: <https://krazywarez.github.io/orgo/install.html>

## Your First Site

```sh
orgo init my-site
orgo serve my-site -o _site
```

Open <http://127.0.0.1:3000>. Edit `my-site/index.org`, save, and the page reloads on its
own — that is the loop you will spend your time in.

`init` writes a starter post, a page layout you can edit, and a config file with every
setting explained in comments. It never overwrites a file you already have. It also picks
one of the four built-in themes — `plain`, `blog`, `wiki`, `docs` — so the site is styled
from the first build; change `theme` in `orgo.toml`, or empty it and write your own CSS.

## Org-Mode, Anywhere

```sh
orgo build ~/notes -o _site
```

No config file, no templates, no orgo-specific markup in your files. You get a real site:
every page, links between them resolved, navigation across the top, code highlighted. That
is a supported way to use it rather than a demo — configuration changes what you get, it
is never what makes it work.

Nothing that should stay private is published: dot-directories like `.git`, your templates
and the output folder itself are all skipped.

Want to know what orgo will make of your files before trusting it with them?
`orgo audit ~/notes` reports which org constructs you use and how each one lands, with
counts and line numbers — never the text of your writing, so the report is safe to share.

## Additional Features

Each of these is a few lines of config, and each has a page in the guide:

| Add | Documented in |
|---|---|
| A blog index, newest first | [Collections](https://krazywarez.github.io/orgo/guide/03-collections.html) |
| Tag pages, and an index of tags | [Collections](https://krazywarez.github.io/orgo/guide/03-collections.html) |
| An RSS feed | [Collections](https://krazywarez.github.io/orgo/guide/03-collections.html) |
| Numbered pages when a list gets long | [Collections](https://krazywarez.github.io/orgo/guide/03-collections.html) |
| A built-in theme for a blog, a wiki or a doc site | [Configuration](https://krazywarez.github.io/orgo/guide/02-configuration.html) |
| Your own design, in ordinary HTML templates | [Templates](https://krazywarez.github.io/orgo/guide/04-templates.html) |
| Drafts that stay unpublished until you say so | [Authoring](https://krazywarez.github.io/orgo/guide/06-authoring.html) |
| A table of contents on long posts | [Authoring](https://krazywarez.github.io/orgo/guide/06-authoring.html) |
| Clean URLs that survive a renamed file | [Authoring](https://krazywarez.github.io/orgo/guide/06-authoring.html) |

Rebuilds only touch the pages that actually changed, so saving a post on a site with
hundreds of them stays instant.

## Speed

Publishing one real site ([cleberg.net](https://cleberg.net)) — 178 org files, ~180 pages — three ways. Median of three runs
each, measured back to back on one machine:

| | Time | |
|---|---|---|
| weblorg (`emacs --script publish.el`) | 49.0s | |
| weblorg + [build.py](https://github.com/ccleberg/cleberg.net/blob/8ec9cdfeae71068a8924dd9f61b9cc28c947ec31/build.py) | 50.3s | |
| orgo, cold build | **0.22s** | 223× faster |
| orgo, nothing changed since last build | **0.13s** | 377× faster |

That middle row is the interesting one. weblorg alone does not group a blog index by year,
write a tags page, rewrite image URLs, minify CSS or emit a sitemap — so I
wrote ~600 lines of Python to do those on top of it. orgo does four of the five natively —
the sitemap included, since writing this table is what prompted it.

Read the numbers with three things in mind. The weblorg figures include Emacs starting and
loading its packages, which you pay on every publish and cannot avoid. orgo emits 13 pages
weblorg does not, one per tag, so it is doing slightly more work. And the two do not
produce byte-identical output — the differences are deliberate and listed under
[Org support](https://krazywarez.github.io/orgo/guide/05-org-support.html).

Apple M2 Pro, 12 cores, macOS 26.6, Emacs 30.2, orgo built with `--release`.

## Docs

<https://krazywarez.github.io/orgo/>

| Page | What is in it |
|---|---|
| [Quick start](https://krazywarez.github.io/orgo/quickstart.html) | A working site in two commands, then your own writing, then your own design. |
| [Install](https://krazywarez.github.io/orgo/install.html) | Getting the binary, and running it without installing anything. |
| [Commands](https://krazywarez.github.io/orgo/guide/01-cli.html) | Every command and flag, and what each is for. |
| [Configuration](https://krazywarez.github.io/orgo/guide/02-configuration.html) | Every setting in `orgo.toml`, what it changes, and what it costs. |
| [Collections](https://krazywarez.github.io/orgo/guide/03-collections.html) | Blog indexes, tag pages, pagination and RSS feeds. |
| [Templates](https://krazywarez.github.io/orgo/guide/04-templates.html) | Layouts, and every variable a template can use. |
| [Org support](https://krazywarez.github.io/orgo/guide/05-org-support.html) | Which org syntax is handled, which is not, and how the rest degrades. |
| [Authoring](https://krazywarez.github.io/orgo/guide/06-authoring.html) | URLs, drafts, excerpts, tables of contents. |
| [Incremental builds](https://krazywarez.github.io/orgo/guide/07-incremental.html) | How it decides what to rebuild. |
| [Watching and serving](https://krazywarez.github.io/orgo/guide/08-workflow.html) | The write-save-see loop. |
| [Auditing](https://krazywarez.github.io/orgo/guide/09-auditing.html) | Reading a corpus before trusting a tool with it. |
| [Deploying](https://krazywarez.github.io/orgo/guide/10-deploying.html) | Producing a production build, and putting it somewhere. |

## Building

```sh
cargo test                              # includes a differential check against Emacs, when present
cargo run -- serve docs -o docs/_site   # read the documentation locally
```

## Licence

[0BSD](LICENSE). Do what you like with it.
