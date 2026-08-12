#!/usr/bin/env bash

set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
destination="$repo_root/_site_source"
generated_play="$repo_root/target/website-play"
manifest="$generated_play/manifest.json"

if [[ ! -f "$manifest" ]]; then
  echo "Website playground assets are missing; run 'npm run build:website' first." >&2
  exit 1
fi

version="$(node -e 'const fs = require("node:fs"); console.log(JSON.parse(fs.readFileSync(process.argv[1], "utf8")).version)' "$manifest")"
asset_root="$(node -e 'const fs = require("node:fs"); console.log(JSON.parse(fs.readFileSync(process.argv[1], "utf8")).assetRoot)' "$manifest")"
if [[ -z "$version" || "$asset_root" != assets/play/* || "$asset_root" == *..* || ! -d "$generated_play/$asset_root" ]]; then
  echo "Website playground manifest does not identify a built asset directory." >&2
  exit 1
fi

rm -rf "$destination"
mkdir -p "$destination"

cp -R "$repo_root/website/." "$destination/"
rm "$destination/docs"
cp "$repo_root/examples/basic/saw_filter_saturator.onda" \
  "$destination/_includes/home-example.onda"
cp -R "$repo_root/docs" "$destination/docs"
cp -R "$repo_root/assets/." "$destination/assets/"
mkdir -p "$destination/_data" "$destination/assets/play"
cp -R "$generated_play/assets/play/." "$destination/assets/play/"
printf 'version: "%s"\nasset_path: "/%s"\n' "$version" "$asset_root" > "$destination/_data/onda.yml"

echo "Staged Jekyll source and Onda $version browser assets at $destination"
