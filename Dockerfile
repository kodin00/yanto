FROM node:22-bookworm-slim AS build
WORKDIR /app
COPY package*.json ./
RUN npm ci
COPY . .
RUN npm run build

FROM node:22-bookworm-slim AS runtime
WORKDIR /app
ENV NODE_ENV=production
ARG TARGETARCH=amd64
ARG FRP_VERSION=0.69.0
RUN apt-get update \
  && apt-get install -y --no-install-recommends git openssh-client ca-certificates curl docker.io postgresql-client \
  && mkdir -p /usr/local/lib/docker/cli-plugins \
  && case "$TARGETARCH" in amd64) compose_arch="x86_64" ;; arm64) compose_arch="aarch64" ;; *) echo "Unsupported architecture: $TARGETARCH" && exit 1 ;; esac \
  && curl -fsSL "https://github.com/docker/compose/releases/download/v2.39.4/docker-compose-linux-${compose_arch}" -o /usr/local/lib/docker/cli-plugins/docker-compose \
  && chmod +x /usr/local/lib/docker/cli-plugins/docker-compose \
  && case "$TARGETARCH" in \
       amd64) frp_arch="amd64"; frp_sha="6b90d1cd28fc661f170c0de90dde03d2c63e4fd7ce0ae2da2ca1c28014b8146e" ;; \
       arm64) frp_arch="arm64"; frp_sha="24a4fc82b4c041835103419685ea124c4d6a7dbf83d0425481f5831b4ce4b3a4" ;; \
       *) echo "Unsupported FRP architecture: $TARGETARCH" && exit 1 ;; \
     esac \
  && curl -fsSL "https://github.com/fatedier/frp/releases/download/v${FRP_VERSION}/frp_${FRP_VERSION}_linux_${frp_arch}.tar.gz" -o /tmp/frp.tar.gz \
  && echo "${frp_sha}  /tmp/frp.tar.gz" | sha256sum -c - \
  && tar -xzf /tmp/frp.tar.gz -C /tmp \
  && install -m 0755 "/tmp/frp_${FRP_VERSION}_linux_${frp_arch}/frpc" /usr/local/bin/frpc \
  && install -m 0755 "/tmp/frp_${FRP_VERSION}_linux_${frp_arch}/frps" /usr/local/bin/frps \
  && rm -rf /tmp/frp.tar.gz "/tmp/frp_${FRP_VERSION}_linux_${frp_arch}" \
  && rm -rf /var/lib/apt/lists/*
COPY package*.json ./
RUN npm ci --omit=dev
COPY --from=build /app/dist ./dist
COPY config/frps.toml /etc/frp/frps.toml
EXPOSE 8080
CMD ["sh", "-c", "if [ \"$YANTO_NODE_ROLE\" = \"worker\" ]; then node dist/server/server/worker.js; else node dist/server/server/index.js; fi"]
