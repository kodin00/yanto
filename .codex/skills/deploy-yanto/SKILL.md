---
name: deploy-yanto
description: Project-scoped Yanto release workflow. Use when Codex is asked to deploy Yanto, publish the current Yanto app, bump the pre-1.0 minor version such as v0.1.0 to v0.2.0, commit release-owned changes, or push the release commit to master for /Users/kodin/Documents/yanto.
---

# Deploy Yanto

## Overview

Publish Yanto by bumping the app's `0.x` minor version, validating the TypeScript build surface, committing only intentional release files, and pushing `master`.

## Workflow

1. Work from `/Users/kodin/Documents/yanto`.
2. Inspect `git status --short` before editing. Preserve unrelated user changes.
3. Confirm the current branch is `master` before pushing.
4. Bump the minor version, resetting patch to `0` (`0.1.0` -> `0.2.0`).
5. Run `npm run typecheck`.
6. Stage only release-owned files unless the user explicitly asks to include more.
7. Commit with `chore: release vX.Y.0`.
8. Push `master` to `origin`.

## Script

Use `scripts/deploy_yanto.sh` for the standard workflow:

```bash
.codex/skills/deploy-yanto/scripts/deploy_yanto.sh
```

Pass extra paths only when those files are part of the release commit:

```bash
.codex/skills/deploy-yanto/scripts/deploy_yanto.sh src/client/App.tsx src/client/styles.css
```

The script updates `package.json` and `package-lock.json`, runs typecheck, commits staged release paths, and pushes `origin master`.
