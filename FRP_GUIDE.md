# Managing FRP in Yanto

Yanto runs FRPS on the public VPS and gives you a copyable FRPC client script/config for the machine behind CGNAT. Yanto no longer runs FRPC through a Yanto worker.

## 1. Deploy or upgrade the VPS

From the Yanto directory on the VPS:

```bash
git pull --ff-only origin master
docker compose -f compose.yml up -d --build --remove-orphans
```

The default ports are:

- `7000/tcp` for the FRP client control connection.
- `25560-25600/tcp` and `25560-25600/udp` for public forwards.

Open them in the VPS firewall:

```bash
sudo ufw allow 7000/tcp
sudo ufw allow 25560:25600/tcp
sudo ufw allow 25560:25600/udp
```

To use another range, set `FRP_BIND_PORT`, `FRP_PORT_START`, and `FRP_PORT_END` in `.env`, then recreate the Compose stack.

## 2. Configure Yanto

1. Open **FRP** in the Yanto sidebar.
2. Enter the VPS public IP or a DNS-only hostname.
3. Do not use a hostname with the Cloudflare proxy enabled; Minecraft TCP/UDP traffic must connect directly to the VPS.
4. Confirm the FRP server status shows **Running**.

The FRP screen can start, stop, and restart the server when needed.

## 3. Create a forward

### SSH example

Use these values for a simple SSH forward:

```toml
serverAddr = "x.x.x.x"
serverPort = 7000

[[proxies]]
name = "ssh"
type = "tcp"
localIP = "127.0.0.1"
localPort = 22
remotePort = 25560
```

`remotePort` must be inside the configured VPS range unless you change `FRP_PORT_START` and `FRP_PORT_END`.

### Minecraft Java

1. Click **Minecraft Java**.
2. Keep the local target as `127.0.0.1:25565` when the server runs on the same client machine.
3. Use public port `25565`, or another free port in the allowed range.
4. Create the tunnel.

### Minecraft Bedrock

1. Click **Minecraft Bedrock**.
2. Keep local port `19132`.
3. Choose a free UDP public port in the configured range.
4. Create the tunnel.

## 4. Run FRPC on the client server

1. Click **Copy client script** or **Copy frpc.toml**.
2. Review `serverAddr`, `serverPort`, `localIP`, `localPort`, and `remotePort`.
3. Run the script with `sudo` on the client server, or write `frpc.toml` yourself and run `frpc -c frpc.toml`.

For a service on the same client server, `127.0.0.1` is usually right. For a service on another LAN machine, use that machine's LAN IP.

## Common operations

- **Disable:** temporarily closes a forward after you copy/restart the FRPC config.
- **Edit:** changes the local target, protocol, or public port.
- **Delete:** removes the rule from Yanto; copy/restart FRPC so the client stops advertising it.
- **Copy endpoint:** copies the public address players should use.
- **Restart server:** restarts FRPS on the VPS; FRPC reconnects automatically.

## Troubleshooting

- **Offline:** FRPC is not connected, FRPS is stopped, or the proxy could not start. Check the public IP, token, TLS setting, firewall, and port range.
- **FRPC online but tunnel offline:** verify the service is listening on the configured local host and port from the client server.
- **Cannot connect externally:** confirm both the VPS firewall and provider firewall/security group allow the public port and protocol.

Useful logs:

```bash
# VPS
docker logs yanto-frps --tail 100

# Client server with systemd
sudo journalctl -u frpc -n 100 --no-pager
```
