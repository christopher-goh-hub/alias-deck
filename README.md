# alias-deck

alias-deck manages shell aliases, functions, and environment variables as shareable packs. One pack compiles to bash (3.2 and later), zsh, fish, and PowerShell 7.

Packs come from strangers, so the compiler treats them as data. Every value is quoted for the target shell, loading your shell never runs pack code, and a pack can ask for a secret but never choose where it comes from.

Status: early. This release has the pack format, the compiler, and a CLI that prints the compiled file. Install, a registry, lint, and a GUI come later.

## Quickstart

```sh
cargo build --release
./target/release/alias-deck compile examples/devkit.toml --shell zsh > ~/.alias-deck.zsh
echo 'source ~/.alias-deck.zsh' >> ~/.zshrc
```

`--shell` takes `bash`, `zsh`, `fish`, or `pwsh`, and defaults to your `$SHELL`. For PowerShell, write to a file ending in `.ps1` and dot-source it from `$PROFILE`. `--os` takes `macos`, `linux`, or `windows`, and defaults to this machine. Secrets are bound in `~/.config/alias-deck/secrets.toml`, or the file you pass with `--secrets`.

The compiled source goes to stdout. Entries that could not be compiled for your shell go to stderr with the reason:

```text
alias-deck: devkit/git/groot: omitted: no body for pwsh (pack has: posix, fish)
```

## Writing packs

See [the pack format reference](docs/pack-format.md) and [`examples/devkit.toml`](examples/devkit.toml).

## Development

`cargo test` runs everything, including `tests/shells.rs`, which sources the compiled output in every installed shell and checks what child processes receive byte for byte. A shell that is not installed is printed as skipped. Set `ALIAS_DECK_REQUIRE_ALL_SHELLS=1` to make a missing shell fail the run. `UPDATE_GOLDEN=1 cargo test --test golden` regenerates the golden files.

The design is in [docs/design/0001-pack-format-and-compiler.md](docs/design/0001-pack-format-and-compiler.md).

## License

MIT. See [LICENSE](LICENSE).
