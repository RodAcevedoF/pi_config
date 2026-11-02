# pi_config

Personal Pi setup tooling.

## pi-check

Compares the local Pi setup against the latest published versions, fetched live on every run:

- Pi itself (`@earendil-works/pi-coding-agent`, from npm)
- every `npm:` package in `~/.pi/agent/settings.json` (from npm), flagging pinned ones
- the gentle-ai binary bundled with gentle-pi (GitHub releases)
- the Engram CLI (GitHub releases) and the running Engram server (`/health`)

```bash
cargo install --path . --locked
pi-check
pi-check update      # pi update --all --no-approve, brew upgrade --formula engram, then check again
pi-check --agent-dir ~/.pi/agent --engram-port 7437
```

Restart Pi after `update` so it loads the updated packages.

The check also reports a `config (pi_config)` row: `ok` when every tracked file matches `$HOME`,
`outdated` with the number of drifted files otherwise (see below).

## Config registry

`home/` mirrors `$HOME`: `home/.pi/agent/AGENTS.md` is `~/.pi/agent/AGENTS.md`. This repository is
the source and record of the setup; files are copied into place, never symlinked.

```bash
pi-check config status                       # same / differs / missing per tracked file; exit 1 on drift
pi-check config apply                        # copy drifted files from home/ into $HOME
pi-check config capture                      # copy tracked files from $HOME back into home/
pi-check config capture ~/.gitignore_global  # start tracking another file under $HOME
pi-check config status --repo <dir> --home <dir>
```

- `apply` backs up every file it overwrites to `~/.pi/agent/backups/pi-config-<unix seconds>/`
  and keeps the existing file's mode (new files take the mode from the repo). Restart Pi afterwards.
- `--repo` defaults to the directory pi-check was built from (`cargo install --path .`), `--home` to `$HOME`.
- Volatile keys are never tracked: `capture` strips them and `apply` keeps the values already in `$HOME`.

| File | Volatile keys |
|---|---|
| `.pi/agent/settings.json` | `deviceId`, `lastChangelogVersion` |
| `.pi/agent/claude-bridge.json` | `startupNoticeShown` |

Never track `auth.json`, `models-store.json` or sessions: they hold credentials or per-machine state.

## Inside Pi: `/pi-check`

This repository is also a Pi package. Register it once:

```bash
pi install /Volumes/CODE/personal/projects/pi_config
```

| Command | Does |
|---|---|
| `/pi-check` | Runs the check and shows the table in the session |
| `/pi-check update` | Rebuilds pi-check from this repo into `~/.cargo/bin`, then runs `pi-check update` |
| `/pi-check install` | Only rebuilds and installs pi-check into `~/.cargo/bin` |
| `/pi-check config status\|apply\|capture [paths]` | Runs `pi-check config ...` with the installed binary (`~` in paths is expanded) |

The registry also tracks a `/handoff [focus]` prompt template (`home/.pi/agent/prompts/handoff.md`):
it asks the model to write the continuation prompt for a new session to `odd/handoff.md`, following
the "Session handoff" rule in `~/.pi/agent/AGENTS.md`, without relying on it recognizing the phrase.
Run `/reload` (or restart Pi) after applying it.

Set `GITHUB_TOKEN` to avoid GitHub's anonymous rate limit. Set `NO_COLOR` to disable colors.

| Status | Meaning | Action |
|---|---|---|
| `ok` | Installed matches latest | none |
| `outdated` | A newer version exists | the hint shows the command (`pi update --extensions`, `pi update pi`, `brew upgrade --formula engram`) or `pi-check update` |
| `pinned` | Newer version exists, but `settings.json` pins a version | remove the `@version` pin if it is not deliberate |
| `ahead` | Installed is newer than the latest release | none |
| `missing` | Not installed or not reachable | see hint |
| `unknown` | A version could not be read or fetched | check network or the tool |

Exit codes: `0` everything current (or deliberately pinned), `1` needs attention, `2` usage error.
