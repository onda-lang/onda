# Onda website

Jekyll site with documentation sourced from the repository's [docs](../docs)
directory. The playground uses the [shared browser IDE](../ui/playground/README.md).

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
