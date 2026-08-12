# scwx

Fast SSH, database and port-forward access to Scaleway infrastructure, from one binary with fuzzy search, tmux integration and shell completion.

## Install

Download the binary for your platform from the [latest release](https://github.com/MaelitoP/scwx/releases/latest):

```sh
curl -fsSL -o /usr/local/bin/scwx \
  https://github.com/MaelitoP/scwx/releases/latest/download/scwx-$(uname -m | sed 's/arm64/aarch64/')-apple-darwin
chmod +x /usr/local/bin/scwx
```

Update later with `scwx update`.

Runtime dependencies: `ssh`, `fzf` (pickers), `mysql` client (only for `scwx db`).

## Setup

scwx reads your Scaleway credentials from `~/.config/scw/config.yaml` (the standard `scw` CLI config; `scw init` creates it). `SCW_SECRET_KEY`, `SCW_PROFILE` and `SCW_DEFAULT_PROJECT_ID` are honored as overrides.

Optional settings live in `~/.config/scwx/config.toml`. Everything has a default; a typical setup:

```toml
[ssh]
key = "~/.local/share/ssh/id_ed25519_scaleway"

[naming]
strip_prefixes = ["platform-ingestor-"]

[db]
secret_project_id = "00000000-0000-0000-0000-000000000000"
```

### Shell completion

```sh
scwx completions zsh > ~/.zsh/completions/_scwx
scwx completions fish > ~/.config/fish/completions/scwx.fish
scwx completions bash > /usr/local/etc/bash_completion.d/scwx
```

Subcommands and flags complete in all shells; server/database/tunnel names also tab-complete in zsh and fish.

### SSH config integration

`scwx sync-ssh` writes one `Host` entry per server to `~/.ssh/config.d/scaleway`. Add this line to `~/.ssh/config` once:

```
Include config.d/scaleway
```

After that, plain `ssh`, `scp` and IDE remote sessions work on every server name, with tab completion.

## Commands

| Command | What it does |
|---|---|
| `scwx connect [query]` | Pick a server (fzf) and open an SSH session; in tmux, `enter` opens a window, `ctrl-s`/`ctrl-v` a split, `ctrl-o` stays inline |
| `scwx db [name]` | Pick a database and open a mysql session through a tunnel; `-e "SELECT ..."` or a stdin pipe runs a query non-interactively, `-- <args>` passes flags to mysql |
| `scwx pf [query]` | Start a background port-forward tunnel |
| `scwx pf ls` / `scwx pf stop [name]` | List / stop tunnels |
| `scwx ls` | List the inventory (`--json` for scripts) |
| `scwx sync-ssh` | Regenerate the SSH host entries |
| `scwx update` | Update to the latest release |

`scwx --help` and `scwx <command> --help` are the authoritative reference. `--env prod|beta|dev` filters everywhere; `--refresh` bypasses the inventory cache.
