# mnml-tickets-bitbucket

Bitbucket Cloud pull-request viewer for [mnml](https://mnml.sh) —
terminal TUI with configurable tabs (per-repo, "PRs I opened",
"PRs I'm reviewing"). Runs standalone in any terminal or as a
hosted mnml pane. Member of the `mnml-tickets-*` integration class
alongside [jira](https://github.com/chris-mclennan/mnml-tickets-jira)
and [github](https://github.com/chris-mclennan/mnml-tickets-github).

```
┌─ bitbucket PRs ──────────────────────────────────────────────────┐
│ ▸1.Mine (3)  2.Reviewing (7)  3.tattle-api PRs (12)              │
└──────────────────────────────────────────────────────────────────┘
┌─ Mine ───────────────────────────────────────────────────────────┐
│ REPO              │ PR     │ STATE │ AUTHOR  │ BRANCH → DEST     │
│ tattlecorp/api   │ #1234  │ OPEN  │ Chris   │ chris/fix → main  │
│ tattlecorp/web   │ #821   │ OPEN  │ Chris   │ chris/redesign…   │
│ …                                                                 │
└──────────────────────────────────────────────────────────────────┘
  1-9 tab · ↑↓/jk move · Enter/o open · r refresh · q quit
```

## Install

```sh
cargo install --git https://github.com/chris-mclennan/mnml-tickets-bitbucket mnml-tickets-bitbucket
```

Homebrew tap + binary releases will follow once the binary stabilises.

## Setup

1. **Create a Bitbucket app password** at
   <https://bitbucket.org/account/settings/app-passwords/>.

   Minimum scopes: **Pull requests: Read**. Add **Account: Read** if
   you want `mode = "mine"` / `mode = "reviewing"` tabs (those need
   `/2.0/user` to resolve your account_id).

2. **Save the app password** to `~/.config/mnml-tickets-bitbucket/token`
   with `chmod 600`:

   ```sh
   mkdir -p ~/.config/mnml-tickets-bitbucket
   pbpaste > ~/.config/mnml-tickets-bitbucket/token   # or paste it in $EDITOR
   chmod 600 ~/.config/mnml-tickets-bitbucket/token
   ```

3. **Run once** to scaffold the config template:

   ```sh
   mnml-tickets-bitbucket
   ```

   Writes `~/.config/mnml-tickets-bitbucket.toml`. Edit `email`,
   `workspace`, and the `[[tabs]]` list.

4. **Re-run** — the TUI launches with your configured tabs.

5. **Verify** the resolved config + auth state:

   ```sh
   mnml-tickets-bitbucket --check
   ```

   Hits `/2.0/user` to confirm the app password works.

## Tab modes

Each `[[tabs]]` entry is one tab. Three shapes are supported:

### Per-repo

```toml
[[tabs]]
name  = "tattle-api PRs"
repo  = "tattle-api"
state = "OPEN"             # OPEN / MERGED / DECLINED / SUPERSEDED
```

Uses the default workspace from the top of the config. Override
per-tab with `workspace = "otherws"` if needed.

### `mode = "mine"` — PRs you opened

```toml
[[tabs]]
name = "Mine"
mode = "mine"
```

Resolves to a workspace-spanning BBQL query —
`author.account_id = "<your-id>"`. Requires **Account: Read** on
the app password.

### `mode = "reviewing"` — PRs you're a reviewer on

```toml
[[tabs]]
name = "Reviewing"
mode = "reviewing"
```

Same as `mine` but with `reviewers.account_id = "<your-id>"`.

### Custom BBQL

For finer-grained control you can supply a raw Bitbucket Query
Language string via `q`. Either as the only filter (no `mode`,
no `repo`) or layered on top of an auto-mode tab:

```toml
[[tabs]]
name  = "Stale PRs"
repo  = "tattle-api"
state = "OPEN"
q     = "updated_on <= 2026-05-01T00:00:00+00:00"
```

```toml
[[tabs]]
name = "My recent merges"
mode = "mine"
state = "MERGED"
q    = "updated_on >= 2026-05-01"
```

BBQL reference:
<https://developer.atlassian.com/cloud/bitbucket/rest/intro/#filtering-and-sorting-results>

## Keys

| Chord                | Action                                            |
|----------------------|---------------------------------------------------|
| `1`-`9`              | Switch to that tab                                |
| `Tab` / `BackTab`    | Cycle tabs forward / back                         |
| `↑` / `k`, `↓` / `j` | Move selection                                    |
| `PgUp` / `PgDn`      | Jump 10 rows                                      |
| `g` / `G`            | Top / bottom                                      |
| `Enter` / `o`        | Open focused PR in your browser                   |
| `r`                  | Refresh active tab                                |
| `q` / `Esc` / `Ctrl+C` | Quit                                            |

Auto-refresh runs every `refresh_interval_secs` seconds (default `60`,
set to `0` to disable).

## Two run modes

### Standalone

```sh
mnml-tickets-bitbucket
```

Works in any terminal. No mnml required.

### Blit-host (hosted as an mnml pane)

```vim
:host.launch mnml-tickets-bitbucket
```

mnml spawns it with `--blit <socket>` and renders the streamed cell
grid as a native `Pane::BlitHost`. See
[Building integrations](https://mnml.sh/manual/integrations/building/)
for the protocol details.

## Wire it into mnml's left rail

```toml
[[ui.integration_icon]]
id       = "bitbucket"
glyph    = "\U000F0093"            # nf-md-bitbucket
fallback = "B"
command  = ":host.launch mnml-tickets-bitbucket"
color    = "blue"
tooltip  = "Open Bitbucket PRs"
```

Setting `[[ui.integration_icon]]` **replaces** mnml's built-in
defaults, so copy them in first if you want to extend rather than
replace.

## Limitations (v0.1)

- **PRs only.** Bitbucket Cloud's issue tracker isn't covered — most
  teams use Jira for issues, and the API surface is small enough that
  a v0.2 add is straightforward if anyone wants it.
- **First page only.** The Bitbucket API caps `pagelen` at 50 and
  v0.1 doesn't paginate. For tabs with >50 open PRs, the oldest are
  truncated.
- **No detail panel.** v0.2 — the family pattern (description /
  comments / approve toggle) ports from `mnml-tickets-jira`.

## Status

**v0.1** (current):

- Standalone + blit-host modes
- Per-repo tabs with state filter (OPEN / MERGED / DECLINED / SUPERSEDED)
- `mode = "mine"` / `mode = "reviewing"` auto-tabs (workspace-spanning BBQL)
- Custom `q` BBQL for layered filters
- 1-9 / Tab / Enter navigation · `r` refresh
- Auto-refresh on `refresh_interval_secs`
- `--check` for resolved-config + whoami verification

## License

MIT.
