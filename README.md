# alias-deck

alias-deck manages shell aliases, functions, and environment variables as shareable packs. One pack compiles to bash (3.2 and later), zsh, fish, and PowerShell 7.

Packs come from strangers, so the compiler treats them as data. Every value is quoted for the target shell, loading your shell never runs pack code, and a pack can ask for a secret but never choose where it comes from.

Status: early. This release installs packs from git or local files, previews and lints them before anything changes, pins what you approved, and wires the result into your shell. A registry and a GUI come later.

## Quickstart

```sh
cargo install --path crates/alias-deck
alias-deck add github:alice/dotpacks//git.toml   # fetch, lint, preview, confirm
alias-deck setup                                  # one time: source the built file from your shell's startup file
exec $SHELL                                       # the aliases are live
alias-deck update                                 # later: shows what changed and asks again
```

`add` prints every function body, environment change, `PATH` change, alias, and lint finding before it writes anything. A pack with a Danger finding, such as one that sets `LD_PRELOAD`, asks you to type its name. Without a terminal, as over plain `ssh`, pass `--yes`, plus `--allow <rule>` for each Danger finding you accept. The lock pins the commit and the SHA-256 of what you approved, and a build refuses bytes that no longer match.

[Install and update packs](docs/install.md) covers servers, refs, local packs, and every file alias-deck writes.

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

`cargo test` runs everything, including `tests/shells.rs`, which sources the compiled output in every installed shell and checks what child processes receive byte for byte. `crates/alias-deck/tests/e2e.rs` runs the binary against `git+file://` fixture repositories with `HOME` and the XDG directories in a temp dir, so no test reads your dotfiles or the network. A shell that is not installed is printed as skipped. Set `ALIAS_DECK_REQUIRE_ALL_SHELLS=1` to make a missing shell fail the run. `UPDATE_GOLDEN=1 cargo test --test golden` regenerates the golden files.

On macOS, `ALIAS_DECK_TEST_KEYCHAIN=1 cargo test --test e2e keychain` adds a throwaway item to your login keychain, reads it through a sourced `init.zsh`, and deletes it.

The designs are in [docs/design/0001-pack-format-and-compiler.md](docs/design/0001-pack-format-and-compiler.md) and [docs/design/0002-install-lockfile-lint.md](docs/design/0002-install-lockfile-lint.md).

## License

MIT. See [LICENSE](LICENSE).
