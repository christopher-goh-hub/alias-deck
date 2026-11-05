# The alias-deck registry

This directory is the official registry. `index.toml` maps pack names to where each pack lives, and `packs/` holds the starter packs this project maintains. `alias-deck search` reads the index, and `alias-deck add <name>` installs from it.

A listing is not a review. `add <name>` turns the name into a source, then fetches, lints, and previews the pack exactly as `add <source>` does. You still see every alias, function body, and environment change before anything installs. The lock records the source, not the name, so `alias-deck update` never asks the registry again.

## Starter packs

| Name | What it adds |
|---|---|
| `git-essentials` | `gs`, `ga`, `gc`, `gsw`, `gp`, `glog`, and 25 more Git aliases, plus `groot` to cd to the repository root |
| `docker` | `dps`, `dex`, `dlogs`, `drun`, the `dco`/`dcup`/`dcdn` Compose aliases, and `dsh` to open a shell in a container |
| `kubectl` | `k`, `kgp`, `kd`, `klf`, `kaf`, `kex`, and more, plus `kns` to show or switch the namespace |
| `navigation` | `..`, `...`, `....`, `up N`, `mkcd`, and `cdtmp` |
| `everyday` | `ll`, `la`, `dud`, `path`, `ports`, and `extract` |

Every function that has to run inside your shell has a body for bash and zsh, fish, and PowerShell. `extract` runs in `/bin/sh`, so Windows leaves it out, and `ll`, `la`, `dud`, and `ports` are for macOS and Linux only.

## Submit a pack

1. Put your pack in a public git repository of your own. Run `alias-deck lint your-pack.toml` and fix every Danger finding. A Danger finding is not always malicious, but a starter registry is the wrong place for it.
2. Open a pull request that adds one entry to `index.toml`. Keep the entries sorted by name.

   ```toml
   [[packs]]
   name = "your-pack"                      # must equal [pack].name in your file
   source = "github:you/your-repo//your-pack.toml"
   description = "One sentence on what it adds"
   tags = ["git"]                          # lowercase words, for search
   ```

   You may pin a tag or commit with `@`, as in `github:you/your-repo//your-pack.toml@v1`. Without one, people get your default branch as it stands when they run `add`.
3. CI parses `index.toml` and checks every entry's shape, so a typo, a duplicate name, or a local path fails the build.

What CI cannot do is fetch your pack. CI runs without network access to other repositories, so the deeper checks (the name matches, no Danger findings, it compiles for every shell, and it loads cleanly in bash, zsh, fish, and PowerShell) run only on packs in this repository. For an external pack, a maintainer runs `alias-deck lint` and `alias-deck add <source>` on it by hand before merging, and again when you ask for a changed `@` pin. Later changes on an unpinned default branch are not reviewed by anyone here. That is why the preview, the lock, and `alias-deck update`'s diff exist.

To add a starter pack to this repository instead, put it in `packs/` and point its entry at `github:christopher-goh-hub/alias-deck//registry/packs/<name>.toml`. `cargo test` then runs every check above on it, in every shell the machine has.

## Use another registry

A deck consults the official registry unless `deck.toml` lists its own. A list replaces the default, so name the default too if you want both. A later registry wins when two list the same name.

```toml
[[registries]]
source = "github:christopher-goh-hub/alias-deck//registry/index.toml"
ref = "main"

[[registries]]
source = "git+https://git.example.com/team/packs.git//index.toml"
```
