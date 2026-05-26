FROM node:22-bookworm-slim AS build
WORKDIR /app
COPY package*.json ./
RUN npm ci
COPY . .
RUN npm run build

FROM node:22-bookworm-slim AS runtime
WORKDIR /app
ENV NODE_ENV=production
ARG TARGETARCH
RUN apt-get update \
  && apt-get install -y --no-install-recommends git openssh-client ca-certificates curl docker.io postgresql-client \
  && mkdir -p /usr/local/lib/docker/cli-plugins \
  && case "$TARGETARCH" in amd64) compose_arch="x86_64" ;; arm64) compose_arch="aarch64" ;; *) echo "Unsupported architecture: $TARGETARCH" && exit 1 ;; esac \
  && curl -fsSL "https://github.com/docker/compose/releases/download/v2.39.4/docker-compose-linux-${compose_arch}" -o /usr/local/lib/docker/cli-plugins/docker-compose \
  && chmod +x /usr/local/lib/docker/cli-plugins/docker-compose \
  && rm -rf /var/lib/apt/lists/*
COPY package*.json ./
RUN npm ci --omit=dev
COPY --from=build /app/dist ./dist
EXPOSE 8080
CMD ["sh", "-c", "if [ \"$YANTO_NODE_ROLE\" = \"worker\" ]; then node dist/server/server/worker.js; else node dist/server/server/index.js; fi"]
