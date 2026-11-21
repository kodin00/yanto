# Drive Host Daemons instead of Yanto-managed containers

The TypeScript version ran `cloudflared` and `frps` as containers it created. The rewrite drives daemons installed natively on the Host (`cloudflared`, `frps`, and Traefik), managed through systemd. Tunnels and proxies then survive Yanto restarts and upgrades, and match how operators normally run these tools. Cloudflare tunnels stay remote-managed through the Cloudflare API. The host `cloudflared` reaches Projects via ports they publish on `127.0.0.1`, not via Docker networks.

The Hosted Registry is the exception: it is a workload, so it runs as a `registry:2` container Yanto starts when it is enabled.
