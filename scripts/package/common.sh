#!/usr/bin/env bash
# Shared helpers for ClipSync companion packaging (local-dev / unsigned by default).
# shellcheck shell=bash

set -euo pipefail

clipsync_package_script_dir() {
  cd "$(dirname "${BASH_SOURCE[0]}")" && pwd
}

clipsync_package_project_root() {
  cd "$(clipsync_package_script_dir)/../.." && pwd
}

clipsync_package_version() {
  local root
  root="$(clipsync_package_project_root)"
  grep '^version' "$root/Cargo.toml" | head -1 | cut -d'"' -f2
}

clipsync_package_arch() {
  uname -m
}

# Optional Rust 1.95 toolchain pin for GUI crate; never mutates rustup default.
# Override with CLIPSYNC_RUSTUP_TOOLCHAIN= (empty) to use active toolchain, or
# CLIPSYNC_RUSTUP_TOOLCHAIN=1.95.0 (default when unset in package builds).
clipsync_package_cargo() {
  local toolchain="${CLIPSYNC_RUSTUP_TOOLCHAIN-1.95.0}"
  if [[ -n "$toolchain" ]]; then
    command cargo "+${toolchain}" "$@"
  else
    command cargo "$@"
  fi
}

clipsync_package_log() {
  printf '[package] %s\n' "$*"
}

clipsync_package_warn() {
  printf '[package] WARN: %s\n' "$*" >&2
}

clipsync_package_die() {
  printf '[package] ERROR: %s\n' "$*" >&2
  exit 1
}

# Escape a string for embedding in JSON (controlled metadata only).
clipsync_json_escape() {
  local value="$1"
  local out="" i c
  local -i n=${#value}
  for ((i = 0; i < n; i++)); do
    c="${value:i:1}"
    case "$c" in
      \\) out+='\\' ;;
      \") out+='\"' ;;
      $'\n') out+='\n' ;;
      $'\r') out+='\r' ;;
      $'\t') out+='\t' ;;
      *) out+="$c" ;;
    esac
  done
  printf '%s' "$out"
}

# Escape a string for embedding in an XML/plist text node.
clipsync_plist_escape() {
  local value="$1"
  local out="" i c
  local -i n=${#value}
  for ((i = 0; i < n; i++)); do
    c="${value:i:1}"
    case "$c" in
      \&) out+='&amp;' ;;
      \<) out+='&lt;' ;;
      \>) out+='&gt;' ;;
      \") out+='&quot;' ;;
      *) out+="$c" ;;
    esac
  done
  printf '%s' "$out"
}

# True when packaging uses override/fixture binaries (not a fresh cargo release build).
clipsync_package_fixture_mode() {
  [[ "${CLIPSYNC_PACKAGE_FIXTURE_MODE:-0}" == "1" \
    || "${CLIPSYNC_PACKAGE_FIXTURE_MODE:-}" == "true" \
    || "${CLIPSYNC_PACKAGE_SKIP_BUILD:-0}" == "1" ]]
}

# Guard against catastrophic rm -rf targets before force-replace.
clipsync_package_path_is_dangerous() {
  local path="$1"
  local resolved
  [[ -z "$path" || "$path" == "/" ]] && return 0
  resolved="$(cd "$(dirname "$path")" 2>/dev/null && pwd)/$(basename "$path")" || resolved="$path"
  case "$resolved" in
    /|/bin|/usr|/usr/bin|/etc|/System|/Users|/home|"$HOME") return 0 ;;
  esac
  # Refuse empty basename or path that is exactly the dist root parent of /
  [[ "$(basename "$resolved")" == "/" ]] && return 0
  return 1
}

# Refuse to overwrite an existing packaging output unless force is requested.
# Keeps the previous bundle intact (no in-place clobber of dist artifacts).
clipsync_package_refuse_overwrite() {
  local path="$1"
  local force="${CLIPSYNC_PACKAGE_FORCE:-0}"
  if [[ -e "$path" ]]; then
    if [[ "$force" == "1" || "$force" == "true" ]]; then
      if clipsync_package_path_is_dangerous "$path"; then
        clipsync_package_die "refusing CLIPSYNC_PACKAGE_FORCE remove of dangerous path: $path"
      fi
      clipsync_package_warn "CLIPSYNC_PACKAGE_FORCE set; removing existing output: $path"
      rm -rf "$path"
      return 0
    fi
    clipsync_package_die "refusing to overwrite existing output: $path (set CLIPSYNC_PACKAGE_FORCE=1 to replace, or remove it)"
  fi
}

clipsync_package_dist_root() {
  local root
  root="$(clipsync_package_project_root)"
  printf '%s\n' "${CLIPSYNC_PACKAGE_DIST:-$root/dist/companion}"
}

clipsync_package_target_dir() {
  local root
  root="$(clipsync_package_project_root)"
  printf '%s\n' "${CARGO_TARGET_DIR:-$root/target}"
}

# Escape a path for Freedesktop Desktop Entry Exec= (two layers + %% field codes).
# Layer 1: Exec quoting (\ " $ `) and literal %% for '%'.
# Layer 2: Desktop-entry string escaping of backslash (so a literal \ needs \\\\).
# Rejects newlines/CRs and '=' in the executable path (spec-forbidden).
# Returns non-zero on reject; prints escaped Exec argument on success.
clipsync_desktop_escape_exec_arg() {
  local value="$1"
  local exec_level="" out="" i c
  local -i n

  if [[ "$value" == *$'\n'* || "$value" == *$'\r'* ]]; then
    printf '%s\n' "desktop Exec path must not contain newlines" >&2
    return 1
  fi
  if [[ "$value" == *'='* ]]; then
    printf '%s\n' "desktop Exec executable path must not contain '='" >&2
    return 1
  fi

  n=${#value}
  for ((i = 0; i < n; i++)); do
    c="${value:i:1}"
    case "$c" in
      \\) exec_level+='\\' ;;
      \") exec_level+='\"' ;;
      \$) exec_level+='\$' ;;
      \`) exec_level+='\`' ;;
      %) exec_level+='%%' ;;
      *) exec_level+="$c" ;;
    esac
  done
  exec_level="\"${exec_level}\""

  n=${#exec_level}
  for ((i = 0; i < n; i++)); do
    c="${exec_level:i:1}"
    case "$c" in
      \\) out+='\\' ;;
      *) out+="$c" ;;
    esac
  done
  printf '%s' "$out"
}

# Best-effort reverse of clipsync_desktop_escape_exec_arg for unit tests (single arg).
clipsync_desktop_unescape_exec_arg() {
  local raw="$1"
  local after_string="" after_exec="" i c
  local -i n

  # String unescape: \\ → \, ignore other escapes except known ones.
  n=${#raw}
  for ((i = 0; i < n; i++)); do
    c="${raw:i:1}"
    if [[ "$c" == '\' && i+1 -lt n ]]; then
      i=$((i + 1))
      c="${raw:i:1}"
      case "$c" in
        \\) after_string+='\' ;;
        n) after_string+=$'\n' ;;
        r) after_string+=$'\r' ;;
        t) after_string+=$'\t' ;;
        s) after_string+=' ' ;;
        *) after_string+="$c" ;;
      esac
    else
      after_string+="$c"
    fi
  done

  # Strip surrounding quotes and Exec-unescape; expand %% → %.
  if [[ "$after_string" == \"*\" && ${#after_string} -ge 2 ]]; then
    after_string="${after_string:1:${#after_string}-2}"
  fi
  n=${#after_string}
  for ((i = 0; i < n; i++)); do
    c="${after_string:i:1}"
    if [[ "$c" == '%' && i+1 -lt n && "${after_string:i+1:1}" == '%' ]]; then
      after_exec+='%'
      i=$((i + 1))
      continue
    fi
    if [[ "$c" == '\' && i+1 -lt n ]]; then
      i=$((i + 1))
      after_exec+="${after_string:i:1}"
      continue
    fi
    after_exec+="$c"
  done
  printf '%s' "$after_exec"
}

# Detect host binary kind: elf | macho | script | other
clipsync_package_binary_kind() {
  local path="$1"
  local info
  [[ -f "$path" ]] || { printf 'missing'; return 0; }
  info="$(file -b "$path" 2>/dev/null || true)"
  case "$info" in
    *ELF*) printf 'elf' ;;
    *Mach-O*|*Mach\ O*) printf 'macho' ;;
    *shell\ script*|*text\ executable*|*ASCII\ text*) printf 'script' ;;
    *) printf 'other' ;;
  esac
}

# Require binary format for a target OS, or allow script/other only in fixture mode.
# Records skip reason on stdout when fixture-skipping format enforcement.
clipsync_package_require_binary_format() {
  local path="$1"
  local want_os="$2" # linux|macos
  local kind same_arch_peer="${3:-}"
  kind="$(clipsync_package_binary_kind "$path")"

  if clipsync_package_fixture_mode && [[ "$kind" == "script" || "$kind" == "other" ]]; then
    printf 'fixture-skip-format path=%s kind=%s want=%s\n' "$path" "$kind" "$want_os"
    return 0
  fi

  case "$want_os" in
    linux)
      [[ "$kind" == "elf" ]] || clipsync_package_die "expected ELF for Linux package, got kind=$kind ($path). Refusing to package host binaries of the wrong format. Use CLIPSYNC_PACKAGE_FIXTURE_MODE=1 only for intentional fixture tests."
      ;;
    macos)
      [[ "$kind" == "macho" ]] || clipsync_package_die "expected Mach-O for macOS package, got kind=$kind ($path). Refusing to package host binaries of the wrong format."
      ;;
    *) clipsync_package_die "unknown want_os: $want_os" ;;
  esac

  if [[ -n "$same_arch_peer" && -f "$same_arch_peer" ]]; then
    local a b
    a="$(file -b "$path" 2>/dev/null || true)"
    b="$(file -b "$same_arch_peer" 2>/dev/null || true)"
    # Cheap same-arch check: both should mention the same uname -m token when present.
    local arch
    arch="$(clipsync_package_arch)"
    if [[ "$a" == *"$arch"* || "$b" == *"$arch"* ]]; then
      if [[ "$a" == *"$arch"* && "$b" != *"$arch"* ]] || [[ "$b" == *"$arch"* && "$a" != *"$arch"* ]]; then
        clipsync_package_die "CLI/GUI architecture mismatch: cli=[$a] gui=[$b]"
      fi
    fi
  fi
}

# Atomic install of a single binary; retains <dest>.previous when present.
# Does not invent config/history migration. Safe for a running destination (new inode).
clipsync_package_install_binary_safe() {
  local source_binary="$1"
  local destination="$2"
  local temporary_binary previous

  [[ -f "$source_binary" ]] || clipsync_package_die "missing binary: $source_binary"
  mkdir -p "$(dirname "$destination")"

  if [[ -e "$destination" ]]; then
    previous="${destination}.previous"
    rm -f "$previous"
    cp "$destination" "$previous"
    chmod 755 "$previous" 2>/dev/null || true
  fi

  temporary_binary=$(mktemp "${destination}.XXXXXX") || return 1
  if ! cp "$source_binary" "$temporary_binary" || ! chmod 755 "$temporary_binary"; then
    rm -f "$temporary_binary"
    return 1
  fi
  if ! mv -f "$temporary_binary" "$destination"; then
    rm -f "$temporary_binary"
    return 1
  fi
  return 0
}

# Install CLI+GUI as a pair. If the second swap fails, roll the first back so the
# installed pair is never left mismatched. Does not restart or health-check any service.
clipsync_package_install_binary_pair_safe() {
  local src_cli="$1" dst_cli="$2" src_gui="$3" dst_gui="$4"
  local cli_had=0 gui_had=0
  local tmp

  [[ -f "$src_cli" ]] || { printf 'missing: %s\n' "$src_cli" >&2; return 1; }
  [[ -f "$src_gui" ]] || { printf 'missing: %s\n' "$src_gui" >&2; return 1; }
  mkdir -p "$(dirname "$dst_cli")" "$(dirname "$dst_gui")"

  # Preserve both previous binaries before either swap.
  if [[ -e "$dst_cli" ]]; then
    rm -f "${dst_cli}.previous"
    cp "$dst_cli" "${dst_cli}.previous"
    chmod 755 "${dst_cli}.previous" 2>/dev/null || true
    cli_had=1
  fi
  if [[ -e "$dst_gui" ]]; then
    rm -f "${dst_gui}.previous"
    cp "$dst_gui" "${dst_gui}.previous"
    chmod 755 "${dst_gui}.previous" 2>/dev/null || true
    gui_had=1
  fi

  tmp=$(mktemp "${dst_cli}.XXXXXX") || return 1
  if ! cp "$src_cli" "$tmp" || ! chmod 755 "$tmp" || ! mv -f "$tmp" "$dst_cli"; then
    rm -f "$tmp"
    return 1
  fi

  # Test-only hook: simulate GUI swap failure after CLI was replaced.
  if [[ "${CLIPSYNC_PACKAGE_INJECT_PAIR_FAIL:-}" == "gui" ]]; then
    if [[ "$cli_had" -eq 1 ]]; then
      cp "${dst_cli}.previous" "$dst_cli"
      chmod 755 "$dst_cli"
    else
      rm -f "$dst_cli"
    fi
    printf '%s\n' "GUI install failed (injected); CLI rolled back to previous pair state (no service restart)." >&2
    return 1
  fi

  tmp=$(mktemp "${dst_gui}.XXXXXX") || true
  if [[ -z "${tmp:-}" ]] || ! cp "$src_gui" "$tmp" || ! chmod 755 "$tmp" || ! mv -f "$tmp" "$dst_gui"; then
    rm -f "${tmp:-}"
    # Roll back CLI to the preserved previous (or remove if there was none).
    if [[ "$cli_had" -eq 1 ]]; then
      cp "${dst_cli}.previous" "$dst_cli"
      chmod 755 "$dst_cli"
    else
      rm -f "$dst_cli"
    fi
    # GUI dest left unchanged; .previous files retained for manual repair.
    printf '%s\n' "GUI install failed; CLI rolled back to previous pair state (no service restart)." >&2
    return 1
  fi
  return 0
}

clipsync_package_artifact_label_unsigned() {
  printf 'UNSIGNED-DEVELOPMENT'
}

clipsync_package_write_provenance() {
  local out_file="$1"
  local kind="$2"
  local version arch commit toolchain fixture features_note
  version="$(clipsync_json_escape "$(clipsync_package_version)")"
  arch="$(clipsync_json_escape "$(clipsync_package_arch)")"
  commit="$(clipsync_json_escape "$(git -C "$(clipsync_package_project_root)" rev-parse --short HEAD 2>/dev/null || echo unknown)")"
  toolchain="$(clipsync_json_escape "${CLIPSYNC_RUSTUP_TOOLCHAIN-1.95.0}")"
  if clipsync_package_fixture_mode; then
    fixture=true
    features_note='Fixture/override binaries — feature flags and release linkage are NOT verified from these inputs.'
  else
    fixture=false
    features_note='Built with clipsync --features tui; clipsync-desktop without ui-test; integration-tests omitted.'
  fi
  cat >"$out_file" <<EOF
{
  "product": "ClipSync companion",
  "kind": "$(clipsync_json_escape "$kind")",
  "version": "$version",
  "arch": "$arch",
  "git": "$commit",
  "signing": "$(clipsync_package_artifact_label_unsigned)",
  "notarized": false,
  "public_release_ready": false,
  "fixture_mode": $fixture,
  "rust_toolchain": "$toolchain",
  "features": {
    "clipsync": ["tui"],
    "clipsync_desktop": [],
    "disabled": ["integration-tests", "ui-test"],
    "verified": $([ "$fixture" = true ] && echo false || echo true)
  },
  "notes": "$(clipsync_json_escape "Local development artifact. Not Developer ID signed or notarized. Do not distribute as a public release. $features_note")"
}
EOF
}
