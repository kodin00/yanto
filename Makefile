.PHONY: dev docker build test lint typecheck db-push

dev:
	npm run dev

docker:
	docker compose -f compose.yml up --build

build:
	npm run build

test:
	npm test

lint:
	npm run lint

typecheck:
	npm run typecheck

db-push:
	npx drizzle-kit push
