#!/usr/bin/env bash

set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
destination="$repo_root/_site_source"

rm -rf "$destination"
mkdir -p "$destination"

cp -R "$repo_root/website/." "$destination/"
rm "$destination/docs"
cp -R "$repo_root/docs" "$destination/docs"
cp -R "$repo_root/assets/." "$destination/assets/"

echo "Staged Jekyll source at $destination"
