#! /bin/sh
#
# Bundle Cadmus for Kobo devices (with or without NickelMenu)
#
# Usage:
#   bundle.sh [--skip-download] [--no-nickel]
#
# Examples:
#   bundle.sh                          Auto-download NickelMenu and bundle
#   bundle.sh --skip-download          Use cached NickelMenu archive
#   bundle.sh --no-nickel              Create KoboRoot without NickelMenu
#   NICKEL_VERSION=0.7.0 bundle.sh     Download NickelMenu v0.7.0
#
# Environment Variables:
#   NICKEL_VERSION    NickelMenu version to download (default: 0.6.0). Ensure this
#                     version exists at https://github.com/pgaskin/NickelMenu/releases

set -e

NICKEL_VERSION=${NICKEL_VERSION:-0.6.0}
CACHE_DIR=".cache"
NICKEL_MENU_REPO="pgaskin/NickelMenu"
NICKEL_MENU_ARCHIVE="${CACHE_DIR}/NickelMenu-${NICKEL_VERSION}-KoboRoot.tgz"

check_dependencies() {
	missing=""
	for cmd in curl wget jq sha256sum tar; do
		if ! command -v "$cmd" >/dev/null 2>&1; then
			missing="${missing} $cmd"
		fi
	done

	if [ -n "$missing" ]; then
		echo "Error: Missing required commands:$missing" >&2
		echo "Please install them and try again" >&2
		exit 5
	fi
}

download_nickel_menu() {
	mkdir -p "$CACHE_DIR"

	if [ -f "$NICKEL_MENU_ARCHIVE" ]; then
		echo "Using cached NickelMenu v${NICKEL_VERSION}"
		return 0
	fi

	echo "Downloading NickelMenu v${NICKEL_VERSION}..."

	info_url="https://api.github.com/repos/${NICKEL_MENU_REPO}/releases/tags/v${NICKEL_VERSION}"
	echo "Fetching release info from: $info_url" >&2

	temp_file="${CACHE_DIR}/release_info.json"
	if ! curl -f "$info_url" >"$temp_file"; then
		curl_exit=$?
		echo "Error: Failed to fetch GitHub API (curl exit code: $curl_exit)" >&2
		echo "Possible causes:" >&2
		echo "  - Network connectivity issues" >&2
		echo "  - GitHub API rate limiting" >&2
		echo "  - Invalid release version: v${NICKEL_VERSION}" >&2
		echo "Check: https://github.com/${NICKEL_MENU_REPO}/releases" >&2
		rm -f "$temp_file"
		exit 1
	fi

	if ! jq empty "$temp_file"; then
		echo "Error: GitHub API returned invalid JSON" >&2
		echo "Response:" >&2
		cat "$temp_file" >&2
		exit 1
	fi

	download_url=$(jq -r '.assets[] | select(.name | endswith("KoboRoot.tgz")).browser_download_url' "$temp_file" 2>/dev/null)
	expected_sha256=$(jq -r '.assets[] | select(.name | endswith("KoboRoot.tgz")).digest' "$temp_file" 2>/dev/null | cut -d: -f2)

	if [ -z "$download_url" ] || [ "$download_url" = "null" ]; then
		echo "Error: Could not find NickelMenu v${NICKEL_VERSION} KoboRoot.tgz asset" >&2
		echo "Release info:" >&2
		jq '.assets[].name' "$temp_file" 2>/dev/null || cat "$temp_file"
		echo "" >&2
		echo "Check: https://github.com/${NICKEL_MENU_REPO}/releases/tag/v${NICKEL_VERSION}" >&2
		exit 1
	fi

	echo "Downloading from: $download_url" >&2
	if ! wget -q "$download_url" -O "$NICKEL_MENU_ARCHIVE"; then
		wget_exit=$?
		echo "Error: Failed to download NickelMenu (wget exit code: $wget_exit)" >&2
		rm -f "$NICKEL_MENU_ARCHIVE"
		exit 1
	fi

	if [ -n "$expected_sha256" ] && [ "$expected_sha256" != "null" ]; then
		echo "Verifying SHA256 checksum..."
		actual_sha256=$(sha256sum "$NICKEL_MENU_ARCHIVE" | cut -d' ' -f1)

		if [ "$actual_sha256" != "$expected_sha256" ]; then
			echo "Error: SHA256 checksum mismatch" >&2
			echo "Expected: $expected_sha256" >&2
			echo "Got:      $actual_sha256" >&2
			rm -f "$NICKEL_MENU_ARCHIVE"
			exit 1
		fi
		echo "Checksum verified successfully"
	fi

	echo "Downloaded NickelMenu to $NICKEL_MENU_ARCHIVE"
}

validate_archive() {
	archive="$1"

	if ! tar -tzf "$archive" >/dev/null 2>&1; then
		echo "Error: Invalid or corrupted archive: $archive" >&2
		exit 1
	fi
}

extract_and_merge() {
	archive="$1"

	mkdir bundle
	cd bundle || exit 1

	tar -xzf "../$archive"
	mv mnt/onboard/.adds .
	rm -Rf mnt

	mv ../dist .adds/cadmus
	cp ../contrib/NickelMenu/* .adds/nm

	cd ..
}

create_bundle_cadmus_only() {
	mkdir -p bundle/mnt/onboard/.adds/cadmus
	cp -r dist/* bundle/mnt/onboard/.adds/cadmus/

	cd bundle || exit 1

	echo "Creating KoboRoot.tgz (Cadmus only)..."
	tar -czf "KoboRoot.tgz" mnt

	rm -Rf mnt
	cd ..

	echo "Bundle created: bundle/KoboRoot.tgz"
	echo "Place this file in the .kobo directory on your Kobo device"
}

create_bundle_with_nickel() {
	cd bundle || exit 1

	mkdir -p mnt/onboard
	mv .adds mnt/onboard/.adds

	echo "Creating KoboRoot-nm.tgz (with NickelMenu)..."
	tar -czf "KoboRoot-nm.tgz" usr mnt

	rm -Rf usr mnt
	cd ..

	echo "Bundle created: bundle/KoboRoot-nm.tgz"
	echo "Place this file in the .kobo directory on your Kobo device"
}

skip_download=false
no_nickel=false

for arg in "$@"; do
	case "$arg" in
	--skip-download)
		skip_download=true
		;;
	--no-nickel)
		no_nickel=true
		;;
	*)
		echo "Unknown option: $arg" >&2
		echo "Usage: bundle.sh [--skip-download] [--no-nickel]" >&2
		exit 1
		;;
	esac
done

check_dependencies

[ -d dist ] || ./dist.sh
[ -d bundle ] && rm -Rf bundle

if [ "$no_nickel" = true ]; then
	create_bundle_cadmus_only
else
	if [ "$skip_download" = false ]; then
		download_nickel_menu
	else
		if [ ! -f "$NICKEL_MENU_ARCHIVE" ]; then
			echo "Error: NickelMenu archive not found at $NICKEL_MENU_ARCHIVE" >&2
			echo "Remove --skip-download flag to auto-download" >&2
			exit 1
		fi
		echo "Using cached NickelMenu v${NICKEL_VERSION}"
	fi

	validate_archive "$NICKEL_MENU_ARCHIVE"
	extract_and_merge "$NICKEL_MENU_ARCHIVE"
	create_bundle_with_nickel
fi
