# Single Host, native binary, running as root

Yanto manages only the server it runs on. It ships as one static binary installed by `install.sh` as a systemd service running as root, with no container install. It writes `/etc/cron.d/yanto`, edits `authorized_keys`, and controls Host Daemons. Running as root is deliberate: Docker socket access is already root-equivalent, so a non-root user in the `docker` group plus sudoers rules would add moving parts without adding a real security boundary. systemd sandboxing options limit the blast radius where they don't break host management.

## Consequences

- The old multi-node Master/Worker feature is dropped. Managing several servers goes on the roadmap.
- The Owner account and the Host are fully trusted. That trust model must be stated in the README.
