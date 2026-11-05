# 0002. Install, lockfile, and safety lint

## Goal

alias-deck is for many users who share packs with strangers. Step 1 made a pack compile safely. Step 2 makes a pack you got from someone else safe to install. You see exactly what it adds before anything changes. Nothing changes after you approved it unless you approve again. One command wires it into your shell. The same commands work on a bare server over SSH.

User flow on a laptop or a server:

```sh
alias-deck add github:alice/dotpacks//git.toml   # fetch, lint, preview, confirm
alias-deck setup                                  # one-time: source the built file from your rc
exec $SHELL                                       # aliases are live
alias-deck update                                 # later: shows what changed, asks again
```

Out of scope: registry search and the pack index (step 3), the GUI (step 4), history suggestions (step 5).

## Data shapes

### Deck manifest. `$XDG_CONFIG_HOME/alias-deck/deck.toml`

Written by the user, the CLI, and later the GUI. It is edited with `toml_edit` so comments and order survive. Order is precedence, and later packs win name clashes, as in `Deck::new`.

```toml
format = 1

[[packs]]
source = "github:alice/dotpacks//git.toml"   # pinned through the lock
ref = "v2"                                   # optional branch, tag, or commit

[[packs]]
source = "~/dotfiles/mine.toml"              # local file, read live
enabled = false                              # optional, default true
```

### Source. A parsed type, never a raw string past the boundary

```rust
pub enum Source {
    /// A file on this machine. Read at every build. You own it, so it is not pinned.
    Local(PathBuf),
    /// A file at a path inside a git repository. Pinned to a commit and a content hash.
    Git { url: GitUrl, path: RepoPath, reference: Option<GitRef> },
}
```

Accepted spellings:

| Spelling | Meaning |
|---|---|
| `./x.toml`, `/abs/x.toml`, `~/x.toml` | `Local` |
| `github:owner/repo` | `Git`, url `https://github.com/owner/repo.git`, path `pack.toml` |
| `github:owner/repo//dir/x.toml` | same, with an explicit path |
| `git+https://host/r.git//x.toml`, `git+file:///tmp/r//x.toml` | any git remote. `git+file` keeps tests hermetic |

`ref` comes from the manifest key or an `@ref` suffix on the command line (`add github:a/b@v2`). `RepoPath` rejects `..`, absolute paths, and empty segments. `GitUrl` accepts only `https://`, `ssh://`, `git@host:` and `file://`. Option-looking values starting with `-` are rejected, so a URL can never become a git flag.

### Lockfile. `$XDG_CONFIG_HOME/alias-deck/deck.lock`

Machine-written, deterministic, one table per `Git` source. Local sources have no entry.

```toml
format = 1

[[packs]]
source = "github:alice/dotpacks//git.toml"
commit = "3f1c...40 hex"
sha256 = "9a0e...64 hex"      # of the pack bytes the user approved
name = "alice-git"
version = "2.1.0"
```

### Store. `$XDG_DATA_HOME/alias-deck/` (default `~/.local/share/alias-deck/`)

- `packs/<sha256>.toml` holds the approved bytes, content-addressed. A build reads only these for `Git` sources and re-hashes them. A mismatch fails the build for that pack. It never refetches silently.
- `init.bash`, `init.zsh`, `init.fish`, `init.ps1` are the compiled outputs. All four are always built, so switching shells needs no rebuild. Each is written to a temp file and renamed, so a crash never leaves a half-written init file.

Fetching uses the `git` CLI, so there is no HTTP or TLS stack in the binary. `git ls-remote` resolves the ref to a commit. Then `git fetch --depth 1 <commit>` runs into a temp repo, and `git show <commit>:<path>` reads the bytes. Every git invocation passes the URL after `--` and sets `GIT_TERMINAL_PROMPT=0`.

### Lint. Pure, in `alias-deck-core`

```rust
pub enum Severity { Info, Warn, Danger }   // ordered

pub struct Finding {
    pub rule: Rule,          // stable id, such as `env-loader-hijack`
    pub severity: Severity,
    pub entry: EntryRef,
    pub message: String,
}

pub fn lint(pack: &Pack) -> Vec<Finding>;   // deterministic order
```

Rules are one table of `(Rule, Severity, matcher)`, with no scattered conditionals. Each rule has a stable kebab-case id that users can name in `--allow <rule>`. The initial set:

| Rule | Severity | Fires on |
|---|---|---|
| `env-loader-hijack` | Danger | Setting a var that injects code into other programs: `LD_PRELOAD`, `LD_LIBRARY_PATH`, `LD_AUDIT`, any `DYLD_*`, `BASH_ENV`, `ENV`, `ZDOTDIR`, `PROMPT_COMMAND`, `NODE_OPTIONS`, `PYTHONSTARTUP`, `PYTHONPATH`, `PERL5OPT`, `PERL5LIB`, `RUBYOPT`, `GIT_SSH`, `GIT_SSH_COMMAND`, `GIT_EXEC_PATH`, `GIT_CONFIG_GLOBAL`, `SSH_ASKPASS`, `SUDO_ASKPASS`, `GIT_ASKPASS` |
| `path-unsafe-dir` | Danger | A PATH entry that is relative, or under `/tmp`, `/var/tmp`, `/dev/shm`, `/private/tmp` |
| `shadows-sensitive` | Danger | An alias or function named `sudo`, `su`, `doas`, `ssh`, `scp`, `sftp`, `ssh-add`, `ssh-keygen`, `gpg`, `gpg2`, `passwd`, `login`, `security`, `op`, `pass`, `kinit`, `secret-tool` |
| `body-pipe-to-shell` | Danger | A function body that pipes a download to a shell, such as `curl … \| sh` or `iwr … \| iex` |
| `env-traffic` | Warn | `http_proxy`, `https_proxy`, `all_proxy` (any case), `NO_PROXY`, `SSL_CERT_FILE`, `CURL_CA_BUNDLE`, `REQUESTS_CA_BUNDLE`, `NODE_EXTRA_CA_CERTS`, `GIT_SSL_NO_VERIFY`, `NODE_TLS_REJECT_UNAUTHORIZED` |
| `env-history` | Warn | `HISTFILE`, `HISTSIZE`, `HISTFILESIZE`, `SAVEHIST`, `HISTCONTROL`, `HISTIGNORE` |
| `body-risky` | Warn | `eval`, `base64 -d`, `rm -rf` on `/` `~` or `$HOME`, `chmod 777`, `sudo`, redirects into shell rc files |
| `shadows-core` | Info | An alias or function named like a core tool: `ls`, `cd`, `rm`, `cp`, `mv`, `cat`, `grep`, `find`, `git`, `curl`, `make`, and a short fixed list |
| `path-prepend` | Info | Any PATH prepend. It shadows system tools |
| `native-function` | Info | `InShell` bodies. They run inside your shell and can change the directory and variables |
| `secret` | Info | Declares a secret. Shows the reason and whether a binding exists |

Body rules are text heuristics, and the preview says so. They flag code for a person to read. They do not prove it safe. The real control is that the preview shows every body in full.

### Approval policy. One function, one table

| Highest severity | Interactive | `--yes` |
|---|---|---|
| Info | Asks `[y/N]` | Proceeds |
| Warn | Asks `[y/N]` | Proceeds and prints the warnings |
| Danger | Asks you to type the pack name | Refuses unless each Danger rule is named in `--allow <rule>` |

### Preview and update diff

`add` prints a preview built from the parsed pack, before anything is written:

1. Name, version, license, source, and resolved commit.
2. Every function body in full, labelled by kind (`script`: runs in `/bin/sh`, or native: runs in your shell).
3. Every env change and PATH change. Secrets with their reason and binding status.
4. Aliases as a compact `name = argv` list.
5. Shadowing: names that exist as commands on this machine's PATH, which is environment-dependent and so not in pure lint, plus clashes with packs already in the deck. Clashes say which pack wins.
6. Findings grouped by severity.

`update` builds the same preview as an entry-level diff between the approved bytes and the new bytes. It shows added, removed, and changed entries, with full old and new bodies for changed functions, and only new findings. If the bytes are identical, it says "up to date" and asks nothing.

## Commands

| Command | Does |
|---|---|
| `add <source>[@ref] [--yes] [--allow <rule>]...` | Fetch, lint, preview, confirm, write store, lock, and manifest, then build |
| `remove <pack-name\|source>` | Drop from manifest and lock, then build. Store bytes are left alone |
| `update [<pack-name>] [--yes] [--allow <rule>]...` | Re-resolve the ref and show the diff. Confirm, then rewrite lock and build |
| `list` | Packs in precedence order with version, commit, entry counts, and enabled flag |
| `build` | Compile every enabled pack into the four init files. Report omitted entries once per shell |
| `setup [--shell <s>]` | Idempotently wire the init file into the shell. Print the exact file and line it changed |
| `lint <pack.toml>...` | Authors run this. It prints findings and exits nonzero on Danger |
| `compile` | Unchanged |

`setup` targets:

| Shell | File | Line |
|---|---|---|
| bash | `~/.bashrc`, and on macOS also `~/.bash_profile` if it exists and does not source `.bashrc` | `[ -f "<init>" ] && . "<init>"` |
| zsh | `${ZDOTDIR:-$HOME}/.zshrc` | same |
| fish | `~/.config/fish/conf.d/alias-deck.fish`, a new file. No rc edit | `source "<init>"` |
| pwsh | `pwsh -NoProfile -Command '$PROFILE.CurrentUserAllHosts'` | `if (Test-Path '<init>') { . '<init>' }` |

The line goes inside a marked block (`# >>> alias-deck >>>` … `# <<< alias-deck <<<`). Re-running replaces the block in place, so it is never duplicated.

## Where code lives

- `alias-deck-core` stays pure and does no IO. It gains the `lint` module, `Source` parsing, the manifest and lock models (parse and serialize through `toml_edit`), and `diff_packs(old, new)`.
- `crates/alias-deck` gains a library target (`src/lib.rs`) for the IO operations: fetch, store, build, setup, and the approval policy. That way the future Tauri app calls the same functions the CLI does. `main.rs` stays argument parsing and rendering.

## Verification

- Lint is table-driven, with one positive and one near-miss negative per rule.
- End to end tests build a `git+file://` fixture repo in a temp dir and run with `HOME`, `XDG_CONFIG_HOME` and `XDG_DATA_HOME` pointed at temp dirs. They cover:
  - `add --yes` writes the lock and the store, and builds the four init files.
  - Sourcing `init.zsh` and `init.bash` in the real shells defines the alias.
  - A new commit upstream makes `update` show the changed function body in the diff.
  - `update --yes` refuses a new Danger finding without `--allow`.
  - Tampering with the store file fails the build.
  - `setup` run twice leaves one block.
  - A local source change is picked up live by `build`.
- The macOS keychain lookup has an opt-in real test (`ALIAS_DECK_TEST_KEYCHAIN=1`). It adds a throwaway generic password, compiles a secret bound to it, sources the init file in zsh, checks the value, and deletes the item.
