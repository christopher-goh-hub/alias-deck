# Pack format reference

A pack is one TOML file of aliases, functions, and environment variables. alias-deck compiles it for bash (3.2 and later), zsh, fish, and PowerShell 7. The full example is [`examples/devkit.toml`](../examples/devkit.toml).

## File layout

```toml
format = 1

[pack]
name = "devkit"
version = "0.3.0"
license = "MIT"
description = "Everyday git shortcuts and toolchain paths."
requires = ["git"]           # optional; commands the pack expects on PATH
os = ["macos", "linux"]      # optional; defaults to all three

[groups.git]
description = "Git shortcuts" # optional
os = ["macos"]                # optional; narrows the pack's list

[groups.git.aliases]
[groups.git.functions.NAME]
[groups.git.env]
```

Unknown keys are errors. A typo such as `scirpt` fails the parse instead of dropping a function.

## Names

| What | Rule |
|---|---|
| Pack and group names | `[a-z0-9][a-z0-9-]*`, up to 64 characters |
| Alias and function names | `[A-Za-z0-9_.][A-Za-z0-9_.+-]*`, up to 64 characters, not `.`, not all digits |
| Environment variable names | `[A-Za-z_][A-Za-z0-9_]*`, up to 128 characters |
| `version` | semver, such as `1.2.0` |

A name that one shell reserves, such as `end` in fish or `function` anywhere, is left out of that shell's output and reported. The same pack still works in the other shells.

## Words and your home directory

Every value is literal text. Nothing expands except one rule: a word that is exactly `~`, or starts with `~/`, means your home directory. `~root` and `a~/b` stay literal.

## Aliases

An alias is a command and its leading arguments, never shell code.

```toml
[groups.git.aliases]
gs = "git status -sb"                                   # plain words, split on spaces
glog = ["git", "log", "--pretty=format:%C(yellow)%h"]   # any other text needs an array
dots = "cd ~/.dotfiles"
```

The string form allows only the characters `A-Z a-z 0-9 _ . / : = @ % + , -`, plus `~` at the start of a word. Use the array form for quotes, `$`, spaces inside a word, or anything else. Every array element reaches the program byte for byte.

An alias cannot hold a pipe, `&&`, or `$VAR`. Write a function for those.

In PowerShell, a one-word alias such as `g = "git"` becomes `Set-Alias`. Every other alias becomes a function, and PowerShell rewrites some arguments you type before the function sees them: a bare `--` is dropped, `a,b` becomes two arguments, `-Dx.y` splits, and wildcards glob. The compile report adds a note to those aliases.

## Functions

A function has exactly one of `script` or `shell.<dialect>`, plus an optional `description`.

### Script functions

```toml
[groups.git.functions.gcm]
description = "Commit all tracked changes with the arguments as the message"
script = '''
exec git commit -a -m "$*"
'''
```

The body is POSIX `sh`. Every shell runs it as `/bin/sh -c BODY NAME ARGS...`, so it behaves the same everywhere and `shellcheck -s sh` can check it. It runs in its own process, so it cannot change your directory or variables. Script functions are left out on Windows, which has no `/bin/sh`.

### In-shell functions

```toml
[groups.nav.functions.mkcd]
shell.posix = 'mkdir -p -- "$1" && cd -- "$1"'
shell.fish  = 'mkdir -p -- $argv[1]; and cd -- $argv[1]'
shell.pwsh  = 'New-Item -ItemType Directory -Force -Path $args[0] | Out-Null; Set-Location -LiteralPath $args[0]'
```

These run inside your shell, so they can `cd` or set variables. Write one body per dialect.

| Dialect | Used by |
|---|---|
| `posix` | bash and zsh (zsh runs it under `emulate -L sh`) |
| `bash` | bash, ahead of `posix` |
| `zsh` | zsh, ahead of `posix` |
| `fish` | fish |
| `pwsh` | PowerShell |

A shell with no matching body leaves the function out and reports `no body for <shell>`.

Bodies are stored as quoted text and evaluated when you call the function. Loading your shell never runs them, so a syntax error in a body shows up when you call it, not at startup.

## Environment variables

```toml
[groups.toolchain.env]
EDITOR = "nvim"
GOPATH = "~/go"
PATH = { prepend = ["~/.cargo/bin", "~/.local/bin"], append = ["~/go/bin"] }
GITHUB_TOKEN = { secret = "Personal access token read by gh" }
```

- A string sets and exports the variable.
- `prepend` and `append` work only on names that end in `PATH`. An entry already in the list stays where it is, so loading twice changes nothing. An entry that contains the list separator (`:`, or `;` on Windows) is left out and reported.
- `secret` names a need and says why. It never holds a value or a location.

## Secrets

You bind each secret in your own file, `~/.config/alias-deck/secrets.toml`, which is never part of a pack:

```toml
GITHUB_TOKEN = { keychain = { service = "alias-deck", account = "github-token" } }
NPM_TOKEN    = { file = "~/.config/npm/token" }
OPENAI_KEY   = { command = ["op", "read", "op://Private/OpenAI/credential"] }
```

| Source | Runs at shell startup |
|---|---|
| `keychain` on macOS | `/usr/bin/security find-generic-password -s SERVICE -a ACCOUNT -w` |
| `keychain` on Linux | `secret-tool lookup service SERVICE account ACCOUNT` |
| `file` | `/bin/cat -- PATH` (absolute, or starting with `~/`) |
| `command` | your own argv |

The variable is set only when the lookup exits 0 and prints something. Trailing newlines are removed, and nothing is printed on failure. The value never appears in a pack or in the compiled file. A secret with no binding is left out and reported, so you know what to bind.

## Several packs

`alias-deck compile a.toml b.toml` loads packs in order. When two entries share a name, the later one wins and the report names the one it replaced. `PATH` lists from every pack apply. One pack can define the same name in groups for different operating systems, because the OS check runs first.

## What gets reported

Every entry gets one line in the compile report: emitted, or omitted with a reason. The reasons are an OS condition, a name shadowed by a later entry, a name the shell reserves, no body for the shell, a script function on Windows, an unbound secret, a secret source with no lookup on this OS, and a path entry that contains the list separator.
