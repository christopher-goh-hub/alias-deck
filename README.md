# alias-deck

alias-deck manages shell aliases, functions, and environment variables as shareable packs. One pack compiles to bash (3.2 and later), zsh, fish, and PowerShell 7.

Packs come from strangers, so the compiler treats them as data. Every value is quoted for the target shell, loading your shell never runs pack code, and a pack can ask for a secret but never choose where it comes from.

Status: early. alias-deck installs packs by name from a registry, from git, or from local files. It previews and lints each pack before anything changes, pins what you approved, and wires the result into your shell. A GUI comes later.

## Install

On macOS or Linux:

```sh
curl -fsSL https://raw.githubusercontent.com/christopher-goh-hub/alias-deck/main/install.sh | sh
```

On Windows, in PowerShell:

```powershell
irm https://raw.githubusercontent.com/christopher-goh-hub/alias-deck/main/install.ps1 | iex
```

The script downloads the build for your machine and the release's `SHA256SUMS`. It refuses to install unless the archive's SHA-256 matches. It installs to `~/.local/bin` (on Windows, `%LOCALAPPDATA%\Programs\alias-deck\bin`) without sudo, and prints the `PATH` line to add if that directory is not on your `PATH`. Set `ALIAS_DECK_VERSION=v0.1.0` to install a specific release, or `ALIAS_DECK_INSTALL_DIR` to install somewhere else.

No release has been published yet, so until the first tag the scripts have nothing to download. Build from source instead:

```sh
cargo install --git https://github.com/christopher-goh-hub/alias-deck alias-deck
```

## Quickstart

```sh
alias-deck search                    # the starter packs in the registry
alias-deck add git-essentials        # fetch, lint, preview, confirm
alias-deck setup                     # one time: source the built file from your shell's startup file
exec $SHELL                          # the aliases are live
alias-deck update                    # later: shows what changed and asks again
```

`add` prints every function body, environment change, `PATH` change, alias, and lint finding before it writes anything. Being in the registry is not a review, so `add git-essentials` shows you the same preview as `add github:alice/dotpacks//git.toml`. A pack with a Danger finding, such as one that sets `LD_PRELOAD`, asks you to type its name. The lock pins the commit and the SHA-256 of what you approved, and a build refuses bytes that no longer match.

## Set up a server

A bare server needs only `curl` or `wget`, `tar`, and its shell. A plain `ssh host command` has no terminal to confirm on, so read the preview first, then approve with `--yes`:

```sh
ssh host 'curl -fsSL https://raw.githubusercontent.com/christopher-goh-hub/alias-deck/main/install.sh | sh'
ssh host '~/.local/bin/alias-deck add git-essentials'          # prints the preview, installs nothing
ssh host '~/.local/bin/alias-deck add git-essentials --yes'
ssh host '~/.local/bin/alias-deck setup --shell bash'
```

`--yes` does not approve a Danger finding. Name each Danger rule you accept with `--allow <rule>`. The compiled files have no runtime dependency on alias-deck.

[Install and update packs](docs/install.md) covers registries, refs, local packs, and every file alias-deck writes. [The registry](registry/README.md) lists the starter packs and explains how to submit one.

## Compile without installing

To print one compiled file without installing anything, use `compile`:

```sh
alias-deck compile examples/devkit.toml --shell zsh > ~/.alias-deck.zsh
```

Entries that a shell cannot run go to stderr with the reason:

```text
alias-deck: devkit/git/groot: omitted: no body for pwsh (pack has: posix, fish)
```

## Writing packs

See [the pack format reference](docs/pack-format.md) and [`examples/devkit.toml`](examples/devkit.toml). Run `alias-deck lint pack.toml` to see the findings the people who install your pack will see.

## Development

`cargo test` runs everything. `tests/shells.rs` sources the compiled output in every installed shell and checks what child processes receive byte for byte. The CLI's tests run the binary against `git+file://` fixture repositories with `HOME` and the XDG directories in a temp dir, so no test reads your dotfiles or the network. `tests/registry_gate.rs` loads every starter pack in each installed shell. `tests/install_script.rs` serves a fake release from 127.0.0.1 and runs `install.sh` and `install.ps1` against it.

A shell that is not installed is printed as skipped. `ALIAS_DECK_REQUIRE_SHELLS=bash,zsh,fish,pwsh` makes a missing shell fail the run, and CI sets it for each runner. `ALIAS_DECK_REQUIRE_ALL_SHELLS=1` means all four. `UPDATE_GOLDEN=1 cargo test --test golden` regenerates the golden files.

On macOS, `ALIAS_DECK_TEST_KEYCHAIN=1 cargo test --test e2e keychain` adds a throwaway item to your login keychain, reads it through a sourced `init.zsh`, and deletes it.

CI runs fmt, clippy, and the tests on Linux, macOS, and Windows, and builds the static Linux binary. A `v*` tag that matches the crate version builds the release archives and publishes them with `SHA256SUMS`.

The designs are in [docs/design/0001-pack-format-and-compiler.md](docs/design/0001-pack-format-and-compiler.md), [docs/design/0002-install-lockfile-lint.md](docs/design/0002-install-lockfile-lint.md), and [docs/design/0003-registry-ci-release.md](docs/design/0003-registry-ci-release.md).

## License

MIT. See [LICENSE](LICENSE).
