# openHC documentation site

Astro + [Starlight](https://starlight.astro.build/). Builds to static HTML and
deploys to GitHub Pages via `.github/workflows/docs.yml`.

```sh
npm install
npm run dev       # http://localhost:4321/openHC/
npm run build     # -> dist/
```

### Regenerating the lockfile

CI runs `npm ci`, which needs a lockfile that resolves on Linux as well as on
whatever machine last touched it. **Plain `npm install` is not enough**: it prunes
the transitive dependencies of platform-specific optional packages that the
current machine did not install, and the result fails on the runner with
`Missing: @emnapi/core... from lock file`.

Regenerate from a clean slate instead, which resolves the whole graph from
registry metadata rather than from whatever happens to be on disk:

```sh
rm -rf node_modules package-lock.json
npm install --package-lock-only
npm ci
```

## Editing pages

Content is plain Markdown in `src/content/docs/`. Edit the files.

Run `npm run dev` alongside your editor and pages reload as you save. Every page
carries an **"Open in editor"** link while the dev server is running, which opens
that page's source file directly; in a production build the same link points at
GitHub instead.

Frontmatter is validated by `src/content.config.ts`, so a typo or a missing
`title` fails the build rather than shipping. The fields are:

| Field | Where it shows |
|---|---|
| `title` | page heading, sidebar, browser tab |
| `description` | search results and link previews |
| `sidebar.order` | position within the section |
| `sidebar.label` | sidebar text, when it should differ from the title |
| `topic` | build log only — the eyebrow above the title |
| `summary` | build log only — the standfirst, and the card text on `/log/` |

Adding a page is adding a `.md` file in the right directory; the sidebar picks it
up from `sidebar.order`.

## Where it is served from

`src/site.config.mjs` is the only place that knows the URL:

| Deployment | Setting |
|---|---|
| Project Pages (default) | `https://the1truejoe.github.io/openHC/` |
| Custom domain, or a `<user>.github.io` repo | `OHC_SITE_BASE=/ npm run build` |

Internal links in content are written **without** the base (`/ea/ea1/`, not
`/openHC/ea/ea1/`). `src/plugins/hast-base-links.mjs` adds the prefix at build
time, so the same content works either way — verified by building both.

If you rename the repo, change the default in `src/site.config.mjs` and nothing
else.

## Layout

```
src/content/docs/     the pages, one directory per sidebar section
src/content.config.ts frontmatter schema (adds `topic` and `summary`)
src/components/       Starlight overrides — the build-log eyebrow, the edit link
src/styles/theme.css  the theme
src/plugins/          base-aware link rewriting
src/site.config.mjs   site URL and base
```

## A note on the build log

Entries carry a `topic` (the eyebrow) and a `summary` (the standfirst under the
title). The cards on `/log/` are **generated from that frontmatter**, so the
index cannot drift from the entries — if you change a summary, the card changes.
Deliberately no dates: entries are ordered by `sidebar.order`, which is what was
learned in what order, not when it was committed.
