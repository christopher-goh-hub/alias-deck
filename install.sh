#!/bin/sh
# Install alias-deck from a GitHub release on macOS or Linux:
#
#   curl -fsSL https://raw.githubusercontent.com/christopher-goh-hub/alias-deck/main/install.sh | sh
#
# It downloads the archive for this machine and the release's SHA256SUMS, refuses to install
# unless the archive's SHA-256 matches, and puts the binary in a directory you own. No sudo.
#
#   ALIAS_DECK_VERSION      a release tag such as v0.1.0 (default: the latest release)
#   ALIAS_DECK_INSTALL_DIR  where the binary goes (default: $HOME/.local/bin)
#   ALIAS_DECK_BASE_URL     the directory holding the archives, so tests can serve a local copy
#
# Everything runs inside main, so a download cut off halfway runs nothing.

set -eu

REPO=christopher-goh-hub/alias-deck

die() {
	printf 'alias-deck install: %s\n' "$*" >&2
	exit 1
}

detect_target() {
	os=$(uname -s)
	arch=$(uname -m)
	case $os in
	Linux) os=unknown-linux-musl ;;
	Darwin) os=apple-darwin ;;
	MINGW* | MSYS* | CYGWIN*)
		die "on Windows, run this in PowerShell instead: irm https://raw.githubusercontent.com/$REPO/main/install.ps1 | iex"
		;;
	*) die "there is no build for $os; build from source with cargo install --git https://github.com/$REPO alias-deck" ;;
	esac
	case $arch in
	x86_64 | amd64) arch=x86_64 ;;
	arm64 | aarch64) arch=aarch64 ;;
	*) die "there is no build for $arch; build from source with cargo install --git https://github.com/$REPO alias-deck" ;;
	esac
	# An x86_64 shell under Rosetta on Apple silicon still gets the native build.
	if [ "$os" = apple-darwin ] && [ "$arch" = x86_64 ] && [ "$(sysctl -n sysctl.proc_translated 2>/dev/null || true)" = 1 ]; then
		arch=aarch64
	fi
	printf '%s-%s\n' "$arch" "$os"
}

download() {
	if command -v curl >/dev/null 2>&1; then
		if [ -n "${ALIAS_DECK_BASE_URL:-}" ]; then
			curl -fsSL -o "$2" "$1"
		else
			curl --proto '=https' --tlsv1.2 -fsSL -o "$2" "$1"
		fi
	elif command -v wget >/dev/null 2>&1; then
		wget -q -O "$2" "$1"
	else
		die "need curl or wget to download"
	fi || die "could not download $1"
}

sha256() {
	if command -v sha256sum >/dev/null 2>&1; then
		sha256sum "$1" | awk '{ print $1 }'
	elif command -v shasum >/dev/null 2>&1; then
		shasum -a 256 "$1" | awk '{ print $1 }'
	else
		die "need sha256sum or shasum to verify the download; refusing to install a file nobody checked"
	fi
}

main() {
	target=$(detect_target)
	if [ -n "${ALIAS_DECK_BASE_URL:-}" ]; then
		base=${ALIAS_DECK_BASE_URL%/}
	elif [ -n "${ALIAS_DECK_VERSION:-}" ]; then
		base="https://github.com/$REPO/releases/download/$ALIAS_DECK_VERSION"
	else
		base="https://github.com/$REPO/releases/latest/download"
	fi
	dir=${ALIAS_DECK_INSTALL_DIR:-${HOME:?set HOME or ALIAS_DECK_INSTALL_DIR}/.local/bin}

	tmp=$(mktemp -d)
	trap 'rm -rf "$tmp"' EXIT
	trap 'exit 1' INT TERM

	# SHA256SUMS names every archive, so it also tells us the file name for this version.
	download "$base/SHA256SUMS" "$tmp/SHA256SUMS"
	suffix="-$target.tar.gz"
	line=$(awk -v s="$suffix" '{ f = $2; sub(/^\*/, "", f) } length(f) > length(s) && substr(f, length(f) - length(s) + 1) == s { print $1, f; exit }' "$tmp/SHA256SUMS")
	[ -n "$line" ] || die "the release has no build for $target"
	expected=${line%% *}
	archive=${line#* }
	case $archive in
	alias-deck-*) ;;
	*) die "unexpected file name in SHA256SUMS: $archive" ;;
	esac
	case $archive in
	*[!A-Za-z0-9._-]*) die "unexpected file name in SHA256SUMS: $archive" ;;
	esac
	case $expected in
	*[!0-9a-f]*) die "SHA256SUMS has no valid checksum for $archive" ;;
	esac
	[ ${#expected} -eq 64 ] || die "SHA256SUMS has no valid checksum for $archive"

	download "$base/$archive" "$tmp/$archive"
	actual=$(sha256 "$tmp/$archive")
	[ "$actual" = "$expected" ] ||
		die "checksum mismatch for $archive: SHA256SUMS says $expected, the download is $actual. Nothing was installed."

	mkdir "$tmp/unpacked"
	tar -xzf "$tmp/$archive" -C "$tmp/unpacked" alias-deck || die "could not unpack $archive"
	mkdir -p "$dir" || die "could not create $dir"
	# Copy next to the target, then rename, so a running alias-deck is never half-written.
	cp "$tmp/unpacked/alias-deck" "$dir/.alias-deck.new" || die "could not write to $dir"
	chmod 755 "$dir/.alias-deck.new"
	mv -f "$dir/.alias-deck.new" "$dir/alias-deck"
	version=$("$dir/alias-deck" --version) || die "installed $dir/alias-deck, but it does not run"

	printf 'Installed %s to %s (sha256 %s).\n' "$version" "$dir/alias-deck" "$actual"
	case ":${PATH:-}:" in
	*":$dir:"*) ;;
	*)
		printf '\n%s is not on your PATH. Add this line to your shell startup file:\n' "$dir"
		printf "  export PATH=\"%s:\$PATH\"\n" "$dir"
		;;
	esac
	printf '\nNext:\n'
	printf '  alias-deck search                    # browse the starter packs\n'
	printf '  alias-deck add git-essentials        # see everything it adds, then confirm\n'
	printf '  alias-deck setup                     # load your deck in every new shell\n'
}

main "$@"
