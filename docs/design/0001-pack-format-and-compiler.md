# 0001: Pack format, core model, and shell compiler

Status: accepted (step 1). Synthesized from two independent design candidates. The archived candidates live under `/tmp/arena-alias-deck/candidate-{1,2}`, and each has a `RATIONALE.md` and a shell-verified harness.

## Goal

alias-deck is an MIT-licensed shell alias manager for a wide audience. Strangers share packs of aliases, functions, and environment variables. Step 1 defines four things: the pack file, the typed model behind it, a compiler from that model to bash (3.2 and later), zsh, fish, and PowerShell 7, and a harness that proves the output in the real shells. The GUI, the registry, install and lockfiles, and lint come later. This design leaves room for them without a redesign.

## Usage

A pack is a TOML file:

```toml
format = 1

[pack]
name = "devkit"
version = "0.3.0"
license = "MIT"
description = "Everyday git shortcuts, navigation helpers, and toolchain paths."
requires = ["git"]
os = ["macos", "linux"]

[groups.git.aliases]
gs = "git status -sb"                                     # shorthand: plain words only
glog = ["git", "log", "--pretty=format:%C(yellow)%h"]     # anything else: an argv array

[groups.git.functions.gcm]
description = "Commit all tracked changes with the arguments as the message"
script = '''
exec git commit -a -m "$*"
'''                                                        # runs in /bin/sh, identical in every shell

[groups.nav.functions.mkcd]
shell.posix = 'mkdir -p -- "$1" && cd -- "$1"'            # runs inside your shell, so it may cd
shell.fish  = 'mkdir -p -- $argv[1]; and cd -- $argv[1]'
shell.pwsh  = 'New-Item -ItemType Directory -Force -Path $args[0] | Out-Null; Set-Location -LiteralPath $args[0]'

[groups.toolchain.env]
EDITOR = "nvim"
PATH = { prepend = ["~/.cargo/bin"], append = ["~/go/bin"] }
GITHUB_TOKEN = { secret = "Token read by gh" }            # names a need, never a value or a source
```

The user keeps secret bindings in a file that is never part of a pack (`~/.config/alias-deck/secrets.toml`):

```toml
GITHUB_TOKEN = { keychain = { service = "alias-deck", account = "github-token" } }
NPM_TOKEN    = { file = "~/.config/npm/token" }
OPENAI_KEY   = { command = ["op", "read", "op://Private/OpenAI/credential"] }
```

Callers use the library like this:

```rust
let doc = PackDocument::parse(&src)?;                      // Err holds every problem, with byte spans
let bindings = Bindings::parse(&secrets_src)?;
let deck = Deck::new(vec![shared.pack().clone(), mine.pack().clone()]); // later packs win name clashes
let out = compile(&deck, &bindings, Target { shell: Shell::Fish, os: Os::Linux }, &CompileOptions::default());
fs::write(init_path, &out.source)?;
for r in out.entries.iter().filter(|r| r.outcome.is_omitted()) { eprintln!("{r}") }
```

## Shape

### Format

TOML with a top-level `format = 1`. Unknown keys are errors, so a typo like `scirpt` fails instead of silently dropping a function. TOML's duplicate-key rule makes names unique inside a table without extra code. Literal strings (`'''`) hold function bodies byte for byte. The GUI edits through `toml_edit`, so comments, key order, and untouched bytes survive a save.

### Model (per model-the-domain, type-system-discipline)

- `Text` is a string with no NUL byte. No shell can carry NUL in argv or env.
- `CommandName` must match `[A-Za-z0-9_.][A-Za-z0-9_.+-]{0,63}`, must not be `.`, and must not be all digits. Whether a name is reserved in a given shell is a compile outcome, not a parse error, so one pack stays valid everywhere.
- `EnvName` must match `[A-Za-z_][A-Za-z0-9_]{0,127}`.
- `Slug` names packs and groups and must match `[a-z0-9][a-z0-9-]{0,63}`.
- `Word` is either `Literal(Text)` or `Home(suffix)`. A value that is exactly `~` or starts with `~/` becomes `Home`. That is the format's only expansion rule.
- `Argv` is a program plus args. The program is non-empty, does not start with `-`, and contains no `=`. The shorthand string form allows only `[A-Za-z0-9_./:=@%+,-]` words split on spaces. Anything else needs the array form.
- `FunctionBody` is either `Script(Text)` or `InShell(InShell)`. An `InShell` is a non-empty `BTreeMap<Dialect, Text>` with dialects `posix`, `bash`, `zsh`, `fish`, and `pwsh`. bash prefers `bash` and falls back to `posix`. zsh prefers `zsh` and falls back to `posix`, which it runs under `emulate -L sh`.
- `EnvValue` is `Set(Word)`, `PathList { prepend, append }`, or `Secret { purpose }`. `PathList` is allowed only on names ending in `PATH`, because fish treats only those as lists.
- `Entry` is `Alias`, `Function`, or `Env`. Lint can sort these into trust tiers. Aliases and env values are data. `Script` functions are isolated code. `InShell` functions are code that runs inside the user's shell.
- A `Pack` has private fields and comes only from `PackDocument`, which is the single source of truth. Edits work on the TOML document, and the `Pack` is re-derived from it afterwards. Nothing serializes a `Pack` back to text.

### Compiler

`compile(&Deck, &Bindings, Target, &CompileOptions) -> Compiled`. The function is pure and total, and its output is deterministic.

- **Lowering.** `lower()` makes each policy decision once: the OS gate (pack OS set AND group OS set), name shadowing across packs, reserved names, dialect choice, self-reference such as `grep = "grep --color=auto"`, and secret binding. The result for each entry is `Item` or `OmitReason`. The `EntryReport` list is built before rendering. So every entry missing from `source` is `Omitted(reason)` by construction, and nothing is dropped silently.
- **Phase order.** Secrets come first, then env and PATH, then commands. A lookup therefore runs before any pack PATH edit could shadow the program it calls.
- **Backends.** There is one backend per shell family, a private `Backend` trait that renders an `Item` into a `Writer`. Adding nushell means adding one `Shell` variant, one `Dialect` variant, and one backend file. Exhaustive matches fail the build until all three exist.

### The writer is the safety boundary (per encode-lessons-in-structure)

Pack bytes reach the output through only two doors.

1. Quoted, through `arg`, `expr`, `text`, `body`, or `nested`.
2. Bare, but only as a validated name whose grammar excludes every metacharacter.

Compiler syntax goes through `code(&'static str)`, so a runtime string cannot become code. `nested` renders a sub-program and quotes it as one literal, which builds the second quoting layer in bash `alias x='…'` and fish `--wraps '…'`. Comments carry only validated identifiers (pack slug and semver), never descriptions. The writer and backends are private to the crate, so no caller can emit shell text another way.

Function bodies are emitted as quoted literals and evaluated when the function is called.

| Shell | Template |
|---|---|
| bash | `function NAME { eval '<body>'; }` |
| zsh | `function NAME { emulate -L sh; eval '<body>'; }` (for a `posix` body) |
| fish | `function NAME; eval '<body>'; end` |
| pwsh | `. ([scriptblock]::Create('<body>')) @args` |
| `Script`, every shell | `/bin/sh -c '<body>' NAME "$@"` |

A hostile body that closes its own wrapper ran at source time in 5 of 5 shells with verbatim bodies, and in 0 of 5 with these templates. Sourcing the compiled file never runs pack code, and one broken body cannot break shell startup. bash and zsh use `function NAME {` and run `unalias NAME` first, because an existing alias would expand inside `NAME() {`.

### Quoting (measured byte for byte in bash 3.2, bash 5.3, zsh 5.9, fish 4.9, pwsh 7.6)

| Shell | Literal | Bare when |
|---|---|---|
| bash, zsh | `'…'`, with `'` written as `'\''` | `[A-Za-z0-9_./:@%+,=-]+`, not starting with `=` (zsh EQUALS), and not containing `==` or `:=` (zsh MAGIC_EQUAL_SUBST expands `a==ls` to `a=/bin/ls`) |
| fish | `'…'`, with `\` and `'` backslash-escaped | `[A-Za-z0-9_./:@%+,=-]+`, and not starting with `%` (fish 4.9 still expands `%self` to its PID) |
| pwsh | `'…'`, doubling `'` and U+2018 to U+201B | argument position only, and only `-{0,2}[A-Za-z][A-Za-z0-9_-]*` |

`$HOME` is written `"$HOME"'/suffix'` in bash, zsh, and fish, and `(Join-Path $HOME 'suffix')` in pwsh. In pwsh, expression positions are always quoted, because `$env:X = -n` is a parse error.

### Shell specifics

- **pwsh.** Every command definition is preceded by `Remove-Alias -Name N -Force -ErrorAction Ignore`. `gp` and `gcm` are ReadOnly built-ins that outrank functions.
  - A one-word alias becomes `Set-Alias`, which keeps native argument passing.
  - Every other alias becomes a function. It forwards pipeline input through `$MyInvocation.ExpectingInput`, and gets the note `PwshArgRewriting`.
  - A self-referential alias resolves the real program with `Get-Command -CommandType Application`.
- **fish.** Aliases become `function NAME --wraps '…'`, which gives completions. fish's own `alias` evaluates text at startup, so it isn't used. `CompileOptions.fish_aliases = Abbr` switches to `abbr --add`, because that is a user preference, not a pack field. Self-reference goes through `command`. fish's reserved names (28, measured) become `OmitReason::ReservedName`.
- **PATH lists.** Adding an entry that is already present does nothing, matching rustup's env script. Sourcing twice is therefore a no-op, and no empty element (which would mean cwd) is ever created. The emitted code works under `set -u`. A literal entry that contains the list separator is `Omitted(SeparatorInPathEntry)`.

### Secrets

A pack only declares that it needs a secret. The user's `Bindings` decide where the value comes from, so a stranger's pack cannot choose to read `~/.ssh/id_ed25519`. Each binding compiles to a lookup argv from a fixed template:

| Source | Template |
|---|---|
| macOS keychain | `/usr/bin/security find-generic-password -s S -a A -w` |
| Linux keychain | `secret-tool lookup service S account A` |
| file | `/bin/cat -- PATH` |
| command | the user's own argv |

Every shell wraps the lookup the same way. The variable is set only if the command exits 0 and prints something, trailing newlines are removed, and nothing is printed on failure. An unbound secret is `Omitted(SecretUnbound)`, which the GUI shows as a to-do. The value never appears in a pack, the model, or the compiled file. The compiled file has no runtime dependency on alias-deck, so it runs on a bare server.

## Tradeoffs accepted

- Aliases can't contain pipes, `&&`, or `$VAR`. In exchange, an alias means the same argv in all four shells and lint can read its program. Anything richer becomes a `script`.
- `script` functions fork `/bin/sh` on each call, even in bash and zsh. In exchange, they behave the same in every shell and `shellcheck -s sh` checks them.
- Bodies run through `eval`, so syntax errors show up at call time and `type mkcd` shows a string. In exchange, sourcing never runs pack code. Lint will parse bodies offline.
- Double-quoted bash and zsh alias text is ugly when a word holds `'`. In exchange, one proven quote function composes mechanically.
- Every secret costs one lookup exec at shell startup. In exchange, secrets are never written to disk.

## Known limitations

- pwsh on Unix globs wildcard arguments that pass through a wrapper function, so `gs '*.txt'` reaches git as file names. It also rewrites `--`, `a,b`, and `-Dx.y`. `Set-Alias` avoids this for one-word aliases. Other aliases carry the `PwshArgRewriting` note.
- Windows is unverified. On Windows, `script` functions are `Omitted(NeedsPosixSh)`.
- `requires` is metadata only for now. A future `doctor` command will check it.

## Alternatives rejected

- **KDL** (candidate 2). Argv-as-arguments and raw strings read well, but bare `--color=auto` parses as a property and `007` as an integer. Tooling is also thin for a contributor base we want to be large.
- **Aliases as shell text.** pwsh can't run bash syntax, and lint would see opaque strings.
- **Functions as executables in a PATH shim directory.** A stranger's `ls` would then shadow the system binary for every script and Makefile, not just the interactive prompt.
- **A pack-declared secret source, or a value resolved at compile time.** The first lets a stranger choose what gets read. The second writes the secret into a file that often ends up in a dotfiles repo.
- **Raising an error on cross-pack name clashes** (candidate 1). Users need their own pack to override a shared one, so the later pack wins and the shadowed entry is reported.
