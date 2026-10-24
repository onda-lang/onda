# Onda website

Jekyll site with documentation sourced from the repository's [docs](../docs)
directory. The playground uses the [shared browser IDE](../ui/playground/README.md).

## Documentation organization

`docs/` contains maintained reader-facing guides. Every top-level Markdown file has Jekyll front
matter and a stable permalink, and is linked from the documentation index. Keep compiler ownership
and design in `architecture.md`, executable contracts and shared analyses in `mir.md`, and public
interfaces in the relevant language/API guide. `host-output.md` covers the shared delegate/print
delivery model and host examples.

Benchmark commands and methodology live in [scripts/bench](../scripts/bench/README.md); dated
measurements and raw evidence live in its `results/` directory. Fold lasting design decisions into
the compiler guides and track unfinished work in `docs/todo/`, which the site excludes. Investigation
diaries, implementation checklists, and machine-local setup notes do not belong in public docs.

## Local preview

Run from the repository root with npm, the browser compiler build tools, and
Jekyll installed:

```sh
npm ci
npm run build:website
bash ./website/stage.sh
jekyll serve --source _site_source --baseurl "" --livereload
```

`build:website` generates the standard-library reference, compiler/playground
assets, and example catalog. `stage.sh` assembles `_site_source` with those assets,
documentation, and the homepage example. Rebuild and restage after changing
browser code or examples. GitHub Pages uses the same build and staging workflow.

`docs/stdlib.md` is generated from the compiler's embedded standard library.
Use `npm run check:stdlib-docs` to check that it is current.
