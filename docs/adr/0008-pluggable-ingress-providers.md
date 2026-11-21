# Hostnames route through a pluggable Ingress Provider (Cloudflare Tunnel or Traefik)

The TypeScript version hard-wired public routing to Cloudflare Tunnels. The rewrite models a Hostname as "public name → Project loopback port" and routes it through an Ingress Provider: a Cloudflare Tunnel (no open ports, edge TLS) or a host Traefik (ports 80/443, Let's Encrypt TLS, file provider). DNS record management through the Cloudflare API is separate from the provider choice, so a Traefik Hostname can still get its A/AAAA record from Yanto.
