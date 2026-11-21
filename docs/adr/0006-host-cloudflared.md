# Use the host's cloudflared instead of a Yanto-managed container

The TypeScript version ran a `cloudflared` container per Node. The rewrite drives the `cloudflared` already installed on the host, so tunnels survive Yanto restarts and upgrades and match how operators normally run cloudflared. Yanto keeps managing tunnels, DNS, hostnames, and routes through the Cloudflare API.
