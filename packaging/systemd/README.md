# Running catalogy under systemd

`catalogy serve` runs in the foreground and shuts down gracefully on
`SIGTERM`/`SIGINT` (it drains in-flight HTTP requests, then exits), so it slots
cleanly into systemd. These templates give it a managed lifecycle: start on
boot, restart on failure, clean `systemctl stop`.

Two ways to run it — pick one.

## Option A — system service (starts at boot, runs as a chosen user)

```sh
# 1. Install the binary somewhere on PATH for root's ExecStart.
sudo install -m755 target/release/catalogy /usr/local/bin/catalogy

# 2. Install the environment file and edit it (set CATALOGY_MODEL_DIR, etc.).
sudo install -d /etc/catalogy
sudo install -m600 packaging/systemd/catalogy.env.example /etc/catalogy/catalogy.env
sudo $EDITOR /etc/catalogy/catalogy.env

# 3. Install the unit and set the User= line to the account that owns
#    ~/.local/share/catalogy and can read your media.
sudo install -m644 packaging/systemd/catalogy.service /etc/systemd/system/catalogy.service
sudo $EDITOR /etc/systemd/system/catalogy.service   # replace REPLACE_WITH_USER

# 4. Enable + start.
sudo systemctl daemon-reload
sudo systemctl enable --now catalogy.service
```

Check it: `systemctl status catalogy` and `journalctl -u catalogy -f`.

## Option B — user service (no root, runs as you)

Simplest fit for a personal rig — runs as your login, so `~/.local/share`
and your media resolve naturally and no `User=` line is needed.

```sh
# Binary on your PATH (e.g. cargo's bin dir already is):
cargo install --path .          # -> ~/.cargo/bin/catalogy

mkdir -p ~/.config/catalogy ~/.config/systemd/user
cp packaging/systemd/catalogy.env.example ~/.config/catalogy/catalogy.env
$EDITOR ~/.config/catalogy/catalogy.env

# Adapt the unit for --user: drop the User= line, and point
# EnvironmentFile/ExecStart at your paths:
#   EnvironmentFile=%h/.config/catalogy/catalogy.env
#   ExecStart=%h/.cargo/bin/catalogy serve --port ${CATALOGY_PORT}
cp packaging/systemd/catalogy.service ~/.config/systemd/user/catalogy.service
$EDITOR ~/.config/systemd/user/catalogy.service

systemctl --user daemon-reload
systemctl --user enable --now catalogy.service
# Optional: run even when you're not logged in.
sudo loginctl enable-linger "$USER"
```

Check it: `systemctl --user status catalogy` and `journalctl --user -u catalogy -f`.

## Notes

- **Port.** Default is `18080`, set via `CATALOGY_PORT` in the env file. Avoid
  `8080` — `llama-swap` owns it on thebeast. If the port is taken, catalogy now
  exits with a clear `port NNNN is already in use` message (no panic), and
  systemd will report the failure rather than flapping silently.
- **Graceful stop.** `systemctl stop catalogy` sends `SIGTERM`; the server
  stops within a second or two, well under `TimeoutStopSec=20`. No SIGKILL.
- **Offline.** The env file sets the `HF_*`/proxy vars that match catalogy's
  strictly-offline invariant. The Rust server doesn't phone home regardless,
  but keep them as a guarantee.
- **Hardening.** The unit ships with light hardening (`NoNewPrivileges`,
  `PrivateTmp`) and commented-out `ProtectSystem`/`ProtectHome`. Those stricter
  options can block reading media originals the server needs to stream — enable
  them only after confirming file serving still works.
