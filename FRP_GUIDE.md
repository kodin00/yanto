# Managing FRP in Yanto

Yanto uses FRP to expose services from a worker machine behind CGNAT through the public VPS.

## 1. Deploy or upgrade the VPS

From the Yanto directory on the VPS:

```bash
git pull --ff-only origin master
docker compose -f compose.yml up -d --build --remove-orphans
```

The default ports are:

- `7000/tcp` for the FRP client connection.
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

## 3. Connect the home server

If the home server is not listed under **Home clients**:

1. Click **Copy worker install command**.
2. Run the command with `sudo` on the home server.
3. Wait several seconds and refresh the FRP screen.

For an existing worker, rerun its install command after upgrading Yanto so its image includes FRPC v0.69.0.

## 4. Create a Minecraft forward

### Java Edition

1. Click **Minecraft Java**.
2. Select the home worker.
3. Keep the local target as `host.docker.internal:25565`.
4. Use public port `25565`, or another free port in the allowed range.
5. Create the tunnel and wait for **Online**.
6. Players connect to `VPS_IP:PUBLIC_PORT`.

### Bedrock Edition

1. Click **Minecraft Bedrock**.
2. Keep local port `19132`.
3. Choose a free UDP public port in the configured range.
4. Create the tunnel and wait for **Online**.

The Minecraft server must be reachable from the worker container. A native server or Docker-published port should listen on a host interface such as `0.0.0.0`, not only `127.0.0.1`.

## Common operations

- **Disable:** temporarily closes a forward without deleting it.
- **Edit:** changes the local target, protocol, worker, or public port.
- **Delete:** removes the forward from the worker on its next poll.
- **Copy endpoint:** copies the public address players should use.
- **Restart server:** restarts FRPS on the VPS; workers reconnect automatically.

## Troubleshooting

- **Syncing:** the worker has not applied the latest configuration yet. Confirm the worker is online and wait a few seconds.
- **Offline:** FRPC is connected incorrectly, FRPS is stopped, or the proxy could not start. Check the public IP, firewall, and port range.
- **Error:** read the client error shown in Yanto. Common causes are an unreachable local service or an outdated worker image.
- **FRPC online but tunnel offline:** verify Minecraft is listening on the configured local host and port.
- **Cannot connect externally:** confirm both the VPS firewall and provider firewall/security group allow the public port and protocol.

Useful logs:

```bash
# VPS
docker logs yanto-frps --tail 100

# Home server
cd /opt/yanto
docker compose -f compose.worker.yml --env-file .env.worker logs --tail 100 worker
```
