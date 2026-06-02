# Yanto Improvement Plan

## Summary
Implement UI and API improvements for project tiles, Cloudflare hostname handling, deployment retry, containers, backups, and app-wide loading feedback.

Use the chosen defaults:
- Cloudflare: one hostname route per project; "Publish" becomes update/edit when one already exists.
- Project secret: tile fetches and copies the deploy token immediately.

## Key Changes
- Redesign project tiles into a cleaner operational layout with title/repo, vertical deployment/container status, tunnel summary, container count, and grouped actions.
- Add tile copy actions for deploy URL, webhook URL, and deploy token.
- Hide Cloudflare hostname fields behind an explicit configure button.
- Enforce one Cloudflare route per project in backend behavior and database indexes.
- Add retry for failed deployments using the normal manual deploy endpoint.
- Redesign containers into a single consistent vertical list per project group.
- Remove the backup history status column.
- Make Postgres targets take the full horizontal width before backup history.
- Add visible loading states beyond toasts across views and buttons.

## API / Type Changes
- Add `GET /api/projects/:id/deploy-token` returning `{ deployToken: string }` for authenticated users.
- Add `api.projectDeployToken(id)`.
- Upsert a project's single Cloudflare route when publishing.
- Keep public project list responses stripped of `deployToken`.

## Test Plan
- Add/adjust API tests for `projectDeployToken`.
- Add backend coverage for deploy-token reveal and Cloudflare single-route behavior where feasible.
- Run `npm run typecheck`, `npm run test:run`, and `npm run build`.
- Manually verify Projects, Deployments, Containers, and Backups views on desktop and narrow viewport.

## Assumptions
- "1 project will have 1 TUNNEL/hostname max" means one Cloudflare hostname route per project, while the existing node-level tunnel is reused.
- Retry deployment means rerun the same project's normal manual deploy flow, not replaying historical env overrides.
- Copying deploy token directly is acceptable for authenticated dashboard users.
