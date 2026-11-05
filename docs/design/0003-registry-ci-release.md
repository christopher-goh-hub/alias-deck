# 0003. Registry, CI, and releases

## Goal

Steps 1 and 2 made a pack safe to compile and safe to install. Step 3 makes alias-deck reachable. People find packs by name, install the binary on any machine with one line, and every change is tested on macOS, Linux, and Windows before it lands.

User flow on a fresh server:

```sh
curl -fsSL https://raw.githubusercontent.com/christopher-goh-hub/alias-deck/main/install.sh | sh
alias-deck search git                  # find a pack
alias-deck add git-essentials --yes    # the same preview and lint as any source
alias-deck setup
```

Out of scope: the GUI (step 4), history suggestions (step 5), a registry website, and signed releases.

## Registry

### Where it lives

The official registry is `registry/` in this repository. `registry/index.toml` is the index, and `registry/packs/<name>.toml` holds the starter packs. A second repository would add a release process before there is anything to release.

### Index. Parsed in `alias-deck-core`

```toml
format = 1

[[packs]]
name = "git-essentials"                  # slug, unique in the index, equal to the pack's [pack].name
source = "github:christopher-goh-hub/alias-deck//registry/packs/git-essentials.toml"
description = "Short aliases for the Git commands you type all day"
tags = ["git", "vcs"]                    # slugs, optional
```

```rust
pub struct RegistryIndex { entries: Vec<RegistryEntry> }   // sorted by name, names unique
pub struct RegistryEntry { pub name: Slug, pub source: Source, pub description: Text, pub tags: Vec<Slug> }
```

`RegistryIndex::parse` follows every other alias-deck file. It needs `format = 1`, rejects unknown keys and duplicate names (`ParseErrorKind::DuplicateName`), and reports every error with a span. `source` must be a git source. A local path in a published index names a file on its author's disk. `RegistryEntry::matches(term)` is a case-insensitive substring match on the name, the description, and each tag.

### Which registries a deck reads

`deck.toml` may list `[[registries]]`, each a git `source` with an optional `ref`, edited through `toml_edit` like packs. `Manifest::registries()` returns that list, or the default when the file has none: `github:christopher-goh-hub/alias-deck//registry/index.toml` at ref `main`. A later registry wins a name both list.

### Cache

`load_registries(paths, refresh)` fetches each index through the existing `git::resolve` and `git::fetch_file`, and writes it to `$XDG_DATA_HOME/alias-deck/registries/<sha256 of source and ref>.toml`. The file is the index text after one comment line that records the source, ref, and commit, so a single atomic write keeps the index and its commit together. The file's modification time is its age.

| Cache | `refresh` | Result |
|---|---|---|
| younger than a day | no | the cache, with no fetch |
| missing, old, or `refresh` | | fetch, then rewrite the cache |
| present, but the fetch fails | | the cache, with a note naming the error and the cache's age |
| missing, and the fetch fails | | an error for that registry. Other registries still load |

### Commands

| Command | Does |
|---|---|
| `search [<term>] [--refresh]` | Prints each matching pack's name, description, tags, and source, one entry per name, then the registry and commit it read. With no term, it lists everything |
| `add <name>` | A bare slug resolves through the registries to a source. Then `add` runs the same fetch, preview, lint, approval, and lock as `add <source>` |

A name is anything that parses as a `Slug`. A local path always starts with `.`, `/`, `~`, or a drive letter, and a git source contains `:`, so the two never overlap. The preview adds one line, `listed in: <registry> (commit <short>). A listing is not a review: read what follows.` The manifest and the lock record the resolved source, not the name, so `update` and `build` never read a registry.

### Starter packs

| Pack | Why it is here |
|---|---|
| `git-essentials` | Git is the command developers type most. 31 aliases and `groot` |
| `docker` | Containers and Compose, with `dsh` to open a shell in a container |
| `kubectl` | The `k*` aliases most kubectl users already type, and `kns` for namespaces |
| `navigation` | `..`, `up N`, `mkcd`, and `cdtmp`. Every function needs native bodies because it changes directory |
| `everyday` | `ll`, `la`, `dud`, `path`, `ports`, and `extract`, the helpers people copy into every new machine |

`everyday` won the fifth slot over the alternatives the brief suggested. Interactive `rm`, `cp`, and `mv` aliases teach a habit that fails on the next machine. Aliasing `ls` to `eza` breaks `ls` wherever `eza` is missing, and `requires` is metadata that nothing enforces. Every function that must run inside the shell has posix, fish, and pwsh bodies. `extract` is a `script` function, so Windows leaves it out, and the `ls`, `du`, and `ports` entries name their OS. In PowerShell, `git-essentials` replaces the built-in aliases `gc`, `gcm`, `gl`, and `gp`, and its description says so.

### Registry gate

`crates/alias-deck/tests/registry_gate.rs` runs in a plain `cargo test`. For each entry whose source is this repository, it maps the path to the file here and checks four things:

- The pack parses, and its `[pack].name` equals the entry's name.
- `lint` reports no Danger finding.
- Compiled together for every shell on every OS, no entry is left out, except by its own OS list or for lack of `/bin/sh` on Windows. A name two starter packs share would show up here as shadowing.
- In each installed shell (bash 3.2, bash 5, zsh, fish, and pwsh), sourcing prints nothing on stderr and defines every emitted name. `up 2`, `mkcd`, `cdtmp`, `groot`, `up x`, and `extract` must then land in the right directory or produce the right file.

An entry in another repository is printed as skipped, because CI cannot fetch it without the network. `registry/README.md` says that a maintainer runs `alias-deck lint` and `alias-deck add` on such a pack before merging.

## CI

`.github/workflows/ci.yml` runs on pushes to `main`, on pull requests, and by hand.

| Runner | Shells it installs | `ALIAS_DECK_REQUIRE_SHELLS` |
|---|---|---|
| `ubuntu-latest` | `apt install zsh fish`. bash and pwsh 7.6 come with the image | `bash,zsh,fish,pwsh` |
| `macos-latest` | `brew install fish bash`. `/bin/bash` is 3.2, and pwsh comes with the image | `bash,zsh,fish,pwsh` |
| `windows-latest` | none. Git Bash 5.3 and pwsh 7.6 come with the image | `bash,pwsh` |

The image contents come from the runner-images READMEs for Ubuntu 24.04, macOS 26 arm64, and Windows Server 2025, the images the `-latest` labels point at today. Each job runs `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, and `cargo test --workspace`. The Linux job also runs `shellcheck -s sh install.sh`. A second job builds `x86_64-unknown-linux-musl` in release mode and checks that the binary is static, so a release break shows up on the pull request.

Every third-party action is pinned to a full commit SHA with its tag in a comment: `actions/checkout` v7.0.1, `Swatinem/rust-cache` v2.9.3, `actions/upload-artifact` v7.0.2, and `actions/download-artifact` v8.0.2. The toolchain comes from the `rustup` that the images ship, so no toolchain action is needed. The workflow token is `contents: read`, and checkout does not persist credentials. `actionlint` passes on both workflows.

### Required shells

`ALIAS_DECK_REQUIRE_SHELLS` is a comma-separated list of `bash`, `zsh`, `fish`, and `pwsh`. A listed shell must run in at least one configuration of the core harness and be found by the CLI's tests, or the run fails. An unknown name panics, so a typo cannot quietly require nothing. `ALIAS_DECK_REQUIRE_ALL_SHELLS=1` still works and means all four.

## Windows

This is the first time alias-deck builds on Windows. `cargo check --workspace --all-targets --target x86_64-pc-windows-msvc` passes with no warnings. These changes came out of reading the code for Windows:

| Problem | Fix |
|---|---|
| Windows sets `USERPROFILE`, not `HOME`, so every command failed with `NoHome` | `Paths::from_env` falls back to `USERPROFILE` on Windows. The XDG defaults stay `~/.config` and `~/.local/share` |
| `add .\x.toml` stored `C:\...\x.toml`, which `Source::parse` rejected, so the next command could not read `deck.toml` | `Source::parse` accepts `C:\`, `C:/`, `.\`, `..\`, and `~\` as local paths |
| `setup` with no `--shell` failed, because Windows has no `$SHELL` | It defaults to pwsh when `$SHELL` is unset on Windows. Git Bash sets `$SHELL`, so bash is still detected there |
| The shadowing check looked for `git`, not `git.exe` | `executable_in` tries each extension in `PATHEXT` |
| `write_atomic` gave a new file the permissions of the current directory, and panicked if the cwd was gone | Unix sets 0644 on a new file. Windows keeps the temp file's defaults. `tempfile`'s `persist` already replaces an existing file on Windows |
| GitHub's Windows image sets `core.autocrlf=true`, which would turn the golden files into CRLF | `.gitattributes` sets `* text=auto eol=lf` |
| The tests used `git+file://C:\...` URLs and `\\?\` canonical paths, which git and the shells reject | `common::file_url` builds `file:///C:/...`, and `common::real_path` drops the `\\?\` prefix |
| The harness's probes were symlinks, and argv bytes came from `OsStrExt` | A Windows build hard-links or copies `name.exe`, and reads bytes through a `#[cfg]` helper |
| `find_shell` looked for `pwsh`, not `pwsh.exe`, and could pick `System32\bash.exe` | It adds `EXE_SUFFIX` and requires `--version` to succeed. The WSL launcher fails that check |

Tests that assert behavior Windows does not have are adjusted at the test, not in the product. `setup_follows_a_symlinked_rc_file` is Unix-only. The e2e tests expect script functions to be omitted on Windows. `setup_for_pwsh_edits_its_profile_once` is ignored on Windows, because pwsh puts the profile in the real Documents folder whatever `HOME` says, and a test must never write there.

The byte-exact harness in `tests/shells.rs` does not run on Windows, and it prints why. Its expectations assume `:` as the `PATH` separator, `/` in joined paths, and a `HOME` that pwsh honors. On Windows, pwsh takes `$HOME` from the profile folder and `Join-Path` writes `\`. Under Git Bash, MSYS rewrites any argument that looks like a POSIX path before a Windows program sees it. Porting the harness needs a Windows machine to iterate on. Until then, Windows coverage comes from the CLI's tests, which source the built files in pwsh and Git Bash and check that aliases are defined and that `up`, `mkcd`, `cdtmp`, and `groot` work in pwsh.

## Releases

`.github/workflows/release.yml` runs on a `v*` tag and by hand. It fails if the tag is not `v` plus the crate version.

| Target | Runner |
|---|---|
| `x86_64-unknown-linux-musl` | `ubuntu-latest` |
| `aarch64-unknown-linux-musl` | `ubuntu-24.04-arm` |
| `aarch64-apple-darwin` | `macos-latest` |
| `x86_64-apple-darwin` | `macos-latest`, cross-compiled with the Apple toolchain |
| `x86_64-pc-windows-msvc` | `windows-latest` |

Each build runs `--version` where the binary can run on its runner, and checks that the Linux binaries are static. It packs `alias-deck`, `LICENSE`, and `README.md` into `alias-deck-<version>-<target>.tar.gz`, or `.zip` for Windows. The `publish` job runs only for a tag, and it alone has `contents: write`. It writes one `SHA256SUMS` over all archives and calls `gh release create --verify-tag --generate-notes` with the workflow token. A manual run builds and uploads the archives as artifacts and publishes nothing.

### install.sh and install.ps1

`install.sh` is POSIX sh and passes `shellcheck -s sh`. It maps `uname -s` and `uname -m` to a target. An x86_64 shell under Rosetta still gets the arm64 build. Then it downloads `SHA256SUMS` and finds the line for that target, which also gives the versioned file name. That lookup lets the default URL be `releases/latest/download/` with no GitHub API call and no JSON parsing. It downloads the archive and compares its SHA-256, from `sha256sum` or `shasum -a 256`. If neither tool exists, or the hashes differ, it stops and writes nothing. It copies the binary next to `$ALIAS_DECK_INSTALL_DIR/alias-deck`, defaulting to `~/.local/bin`, and renames it into place. Then it prints the version, a `PATH` line if needed, and the next commands. curl gets `--proto =https --tlsv1.2` unless `ALIAS_DECK_BASE_URL` points it at a test server.

`install.ps1` does the same with `Invoke-WebRequest` and `Get-FileHash`, installs to `%LOCALAPPDATA%\Programs\alias-deck\bin`, and prints the command that adds that directory to the user's `PATH`. It does not change `PATH` itself, matching `install.sh`, which never edits a startup file. It throws instead of calling `exit`, because under `irm | iex` `exit` closes the user's window, and it avoids syntax that Windows PowerShell 5.1 lacks.

`tests/install_script.rs` serves a fake release from a small HTTP server on 127.0.0.1. The release holds an archive of the binary this build produced, a decoy archive for another target, and a `SHA256SUMS` that lists both. `install.sh` runs under every POSIX sh on the machine: on macOS that is `/bin/sh`, bash 3.2, and dash, and on Ubuntu `/bin/sh` is dash. One run must install a binary whose `--version` matches the crate. A second run, with a wrong checksum, must exit 1 and leave the install directory missing. `install.ps1` gets the same two cases under pwsh. Off Windows, the test compares the installed bytes instead of running `alias-deck.exe`.

## Changes made during implementation

- **A `[[registries]]` list replaces the default instead of adding to it.** The brief said the default could be "overridden or extended". One rule covers both: list the default too to extend. Appending to an implicit default could never drop it. A company that wants only its internal registry needs that, and so does a hermetic test.
- **An index entry may pin a ref with `@`.** `source` parses with `Source::parse_with_ref`, so `github:you/repo//p.toml@v1` works. A submitter can pin a reviewed tag, and the ref reaches the manifest exactly as `add <source>@v1` would put it there.
- **Linux release builds use native runners instead of cargo-zigbuild or cross.** `ubuntu-24.04-arm` builds `aarch64-unknown-linux-musl` natively, so both Linux targets need only `rustup target add` and `musl-tools`. That is one less pinned third-party tool. A musl build in an arm64 Linux container linked a static binary with no extra tools.
- **`ALIAS_DECK_REQUIRE_ALL_SHELLS=1` is now "all four shells", not "every configuration".** A shell counts as present when at least one of its configurations runs. On macOS, a missing Homebrew bash 5 no longer fails the run while bash 3.2 runs. That is the price of one rule that also works on Linux, which has no bash 3.2.
- **The byte-exact harness is POSIX-only for now.** See the Windows section.
- **Windows keeps the XDG layout.** `%APPDATA%` would be more native. One layout on every OS keeps `docs/install.md` to one table, and a deck synced between machines finds its files in the same place.
- **The registry gate also checks behavior.** The brief asked that zsh and bash load the starter packs without error. The gate sources them in every installed shell and runs each directory-changing function, because a body that loads cleanly can still `cd` to the wrong place. A mutation that makes fish's `up` climb `./` instead of `../` fails it.
- **The cache records its commit in a comment line, not a sidecar file.** One atomic write keeps the index and its commit from disagreeing after a crash.

## Known gaps

- The first CI run on Windows is the first time the tests run there. Nothing above has executed on Windows. Git Bash's handling of `C:\` paths in the setup line is the most likely failure.
- Nothing fetches or lints packs in other repositories automatically. A maintainer does it by hand at review time.
- The default registry ref is `main`, and alias-deck caches its index for a day, so a pack removed from the index can still resolve from a cache for up to a day.
- The `wget` fallback in `install.sh` is not tested, because the test machines all have curl.
- Releases are not signed. `SHA256SUMS` proves that the archive matches the release. It does not prove who made the release.
