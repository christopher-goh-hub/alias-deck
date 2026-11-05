# Install and update packs

This guide covers installing a pack someone else wrote, wiring it into your shell, and taking updates. The same commands work on a laptop and on a server you reach over SSH.

## Add a pack

```sh
alias-deck add github:alice/dotpacks//git.toml
```

`add` fetches the pack, checks it, and prints a preview before it writes anything. The preview shows:

1. The pack's name, version, license, source, resolved commit, and the SHA-256 of its bytes.
2. Every function body in full. A `script` function runs in `/bin/sh`. A `native` function runs inside your shell, so it can change your directory and variables.
3. Every environment variable and `PATH` change, and each secret the pack asks for, with whether your secrets file binds it.
4. Every alias, as the command it runs.
5. Shadowing. These are names that already exist as commands on your `PATH`, and names another pack in your deck also defines, with the pack that wins.
6. Findings from the safety lint, most severe first. See [the lint rules](pack-format.md#safety-lint).

Then `add` asks you to confirm. A pack with a Danger finding asks you to type its name instead of `y`.

To pin a branch, tag, or commit, add `@ref`:

```sh
alias-deck add github:alice/dotpacks//git.toml@v2
```

A local file works too. alias-deck reads it again at every build and never pins it, because you own it.

```sh
alias-deck add ~/dotfiles/mine.toml
```

## Load your deck in your shell

```sh
alias-deck setup
exec $SHELL
```

`setup` adds one line to your shell's startup file, inside a marked block, and prints the file and the line. Running it again replaces the block instead of adding a second one. It reads `$SHELL` to pick the shell. Pass `--shell bash`, `zsh`, `fish`, or `pwsh` to choose another.

| Shell | File it edits |
|---|---|
| bash | `~/.bashrc`. On macOS also `~/.bash_profile` if it exists and does not mention `.bashrc` |
| zsh | `$ZDOTDIR/.zshrc`, or `~/.zshrc` |
| fish | `$XDG_CONFIG_HOME/fish/conf.d/alias-deck.fish`, a file of its own |
| pwsh | The file `$PROFILE.CurrentUserAllHosts` names |

If the startup file is a symlink, as dotfiles managers make it, `setup` edits the file the link points to.

## Install on a server over SSH

A plain `ssh host command` has no terminal, so alias-deck cannot ask you to confirm. It prints the preview and stops. Read the preview, then approve it with `--yes`:

```sh
ssh host alias-deck add github:alice/dotpacks//git.toml
ssh host alias-deck add github:alice/dotpacks//git.toml --yes
ssh host alias-deck setup --shell bash
```

`--yes` does not approve a Danger finding. Name each Danger rule you accept:

```sh
ssh host alias-deck add github:alice/dotpacks//git.toml --yes --allow env-loader-hijack
```

`ssh -t` gives alias-deck a terminal, so it can ask you as it does on a laptop. The compiled files have no runtime dependency on alias-deck, so a server needs only the shell itself.

## Take updates

```sh
alias-deck update          # every pack
alias-deck update alice-git
```

`update` resolves each pack's ref again. If the bytes are the same as the ones you approved, it prints `up to date` and asks nothing. Otherwise it prints what changed since you approved the pack: added, removed, and changed entries, with the old and new body of each changed function in full, and only the findings you have not approved before. Approving works as it does for `add`.

## Remove a pack and list your deck

```sh
alias-deck list
alias-deck remove alice-git
```

`list` prints your packs in precedence order. A later pack wins a name clash. `remove` takes a pack name or a source.

## Rebuild

alias-deck builds after every `add`, `update`, and `remove`. Run `alias-deck build` after you edit a local pack, your secrets file, or `deck.toml` by hand. A build always writes all four init files, so switching shells needs no rebuild.

## Files

| File | Holds | Written by |
|---|---|---|
| `$XDG_CONFIG_HOME/alias-deck/deck.toml` | Your packs, in precedence order, with an optional `ref` and `enabled = false` | You and alias-deck. Comments and order survive alias-deck's edits |
| `$XDG_CONFIG_HOME/alias-deck/deck.lock` | The commit and SHA-256 you approved for each git pack | alias-deck |
| `$XDG_CONFIG_HOME/alias-deck/secrets.toml` | Your secret bindings | You |
| `$XDG_DATA_HOME/alias-deck/packs/<sha256>.toml` | The approved bytes of each git pack | alias-deck |
| `$XDG_DATA_HOME/alias-deck/init.{bash,zsh,fish,ps1}` | The compiled files your shell sources | alias-deck |

`$XDG_CONFIG_HOME` defaults to `~/.config` and `$XDG_DATA_HOME` to `~/.local/share`. A relative path in `deck.toml` is relative to the directory that holds `deck.toml`. `alias-deck add ./x.toml` stores the absolute path.

A build reads a git pack only from the store and hashes it again. If the hash differs from the lock, the build fails and names the file. It writes nothing in that case, so the init files from the last good build stay in place. `alias-deck update` fetches the pack again and shows you what you would approve.

## Sources

| You type | Meaning |
|---|---|
| `./x.toml`, `../x.toml`, `/abs/x.toml`, `~/x.toml` | A local file |
| `github:owner/repo` | `pack.toml` in `https://github.com/owner/repo.git` |
| `github:owner/repo//dir/x.toml` | That file in the same repository |
| `git+https://host/repo.git//x.toml` | Any repository over HTTPS. `git+ssh://`, `git+git@host:path`, and `git+file://` work the same way |

alias-deck fetches with the `git` command, so your SSH keys and credential helpers apply. Git does not prompt for a username or password, so a fetch that needs one fails instead of waiting. alias-deck rejects a source that starts with `-`, a path inside the repository that contains `..`, and `http://` URLs.
