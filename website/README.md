# Onda website

The site implementation lives in this directory. Documentation remains in the
repository-level `docs/` directory so the website and repository share one
source of truth.

To preview locally using the same self-contained source tree as GitHub Pages,
stage the site and serve it from the repository root:

```bash
bash ./website/stage.sh
jekyll serve --source _site_source --baseurl "" --livereload
```

GitHub Actions calls the same staging script before building the Pages artifact.
