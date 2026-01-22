#! /bin/sh

version="v$(cargo pkgid -p cadmus | cut -d '#' -f 2)"
archive="plato-kobo.tar.gz"
if ! [ -e "$archive" ]; then
	info_url="https://api.github.com/repos/ogkevin/cadmus/releases/tags/${version}"
	echo "Downloading ${archive}."
	release_url=$(wget -q -O - "$info_url" | jq -r ".assets[] | select(.name == \"$archive\").browser_download_url")
	wget -q --show-progress "$release_url"
fi

tar --wildcards -xzvf "$archive" "./$@"
