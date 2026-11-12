#!/usr/bin/env bash
# Runs inside the Arch Linux container started by release-aur.sh. Builds and
# installs clipsync-bin from /work/pkg, then writes .SRCINFO back there.
set -euo pipefail

if [[ ! -f /.dockerenv && ! -f /run/.containerenv ]]; then
  echo "aur-container-check.sh only runs inside the AUR check container" >&2
  exit 1
fi
VERSION="${CLIPSYNC_AUR_VERSION:?the package version must be passed in}"
PKG=/work/pkg

die() {
  printf '[aur-check] FAILED: %s\n' "$*" >&2
  exit 1
}
log() {
  printf '[aur-check] %s\n' "$*"
}

# pacman's seccomp sandbox fails under amd64 emulation (Docker on Apple Silicon);
# this container is thrown away after the check.
PACMAN=(pacman --noconfirm --disable-sandbox)
log "updating Arch and installing namcap"
"${PACMAN[@]}" -Syu --needed namcap desktop-file-utils >/dev/null || die "pacman -Syu failed"

useradd -m builder
chown -R builder: "$PKG"
cd "$PKG"
su builder -c 'makepkg --printsrcinfo >.SRCINFO' || die "makepkg --printsrcinfo failed"
log "building the package"
# Dependencies come from pacman -U below, so the build itself needs none.
su builder -c 'makepkg --force --nodeps --noconfirm' || die "makepkg failed"

shopt -s nullglob
packages=(clipsync-bin-"$VERSION"-*.pkg.tar.zst)
[[ ${#packages[@]} -eq 1 ]] || die "expected one clipsync-bin $VERSION package, found ${#packages[@]}"
package="${packages[0]}"

lint="$(namcap PKGBUILD "$package" 2>&1)" || die "namcap could not run"
[[ -z "$lint" ]] || printf '%s\n' "$lint" | sed 's/^/[aur-check] namcap: /'
# Errors, and linked libraries whose package is missing from depends, block the push.
if grep -E ' E: |detected and not included' <<<"$lint" >/dev/null; then
  die "namcap reported errors or missing dependencies"
fi

log "installing $package with its dependencies"
"${PACMAN[@]}" -U "$package" >/dev/null || die "pacman -U failed"

missing="$(ldd /usr/bin/clipsync /usr/bin/clipsync-desktop | grep 'not found' || true)"
[[ -z "$missing" ]] || die "unresolved libraries:
$missing"
# The GUI loads X11, Wayland, xkbcommon and GL libraries at runtime, which ldd and
# namcap cannot see; their names are embedded in the binaries. The base image has
# none of them, so each must arrive with the package dependencies.
runtime_libs="$(strings -a -n 6 /usr/bin/clipsync /usr/bin/clipsync-desktop \
  | grep -oE 'lib[A-Za-z0-9_+-]{1,40}\.so\.[0-9]+' | sort -u)"
[[ -n "$runtime_libs" ]] || die "found no library names in the binaries"
for lib in $runtime_libs; do
  [[ -e "/usr/lib/$lib" ]] || die "runtime library $lib is not installed by the package dependencies"
done
log "runtime libraries present: $(tr '\n' ' ' <<<"$runtime_libs")"

version_out="$(clipsync --version)" || die "clipsync --version failed"
[[ "$version_out" == *"$VERSION"* ]] || die "clipsync --version printed '$version_out', expected $VERSION"
clipsync-desktop --help >/dev/null || die "clipsync-desktop --help failed"
desktop-file-validate /usr/share/applications/clipsync.desktop || die "desktop entry does not validate"
grep -qx 'Exec=clipsync-desktop' /usr/share/applications/clipsync.desktop \
  || die "desktop entry does not launch clipsync-desktop"
for license in LICENSE-MIT LICENSE-APACHE; do
  [[ -s "/usr/share/licenses/clipsync-bin/$license" ]] || die "missing $license"
done
if pacman -Qlq clipsync-bin | grep -E '/systemd/|^/etc/' >/dev/null; then
  die "package must not install service units or files under /etc"
fi

log "installed files:"
pacman -Qlq clipsync-bin | grep -v '/$' | sed 's/^/[aur-check]   /'
log "ok: $version_out"
