#!/usr/bin/env bash
set -euo pipefail

repo_root="/Users/kodin/Documents/yanto"
cd "$repo_root"

branch="$(git branch --show-current)"
if [[ "$branch" != "master" ]]; then
  echo "Expected master branch, found ${branch}." >&2
  exit 1
fi

current_version="$(node -p "require('./package.json').version")"
next_version="$(node -e "
const [major, minor] = process.argv[1].split('.').map(Number);
if (!Number.isInteger(major) || !Number.isInteger(minor)) {
  throw new Error('Invalid semver: ' + process.argv[1]);
}
console.log([major, minor + 1, 0].join('.'));
" "$current_version")"

node -e "
const fs = require('fs');
for (const file of ['package.json', 'package-lock.json']) {
  const data = JSON.parse(fs.readFileSync(file, 'utf8'));
  data.version = process.argv[1];
  if (data.packages && data.packages['']) {
    data.packages[''].version = process.argv[1];
  }
  fs.writeFileSync(file, JSON.stringify(data, null, 2) + '\n');
}
" "$next_version"

npm run typecheck

git add package.json package-lock.json "$@"
git commit -m "chore: release v${next_version}"
git push origin master

echo "Released v${next_version}"
