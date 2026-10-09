#!/usr/bin/env python3
"""
ClipSync installer end-to-end acceptance harness.

Runs only inside a disposable Docker container (see /.dockerenv). Never run on a
developer host with real systemctl, launchctl, or personal XDG directories.

Usage (in container, after building a Linux clipsync binary):
  python3 scripts/test/install_end_to_end.py /path/to/clipsync

Host CI may syntax-check only:
  python3 -m py_compile scripts/test/install_end_to_end.py
"""

from __future__ import annotations

import argparse
import os
import re
import shutil
import subprocess
import sys
import tempfile
import tarfile
import tomllib
from dataclasses import dataclass, field
from pathlib import Path
from typing import Mapping, Sequence

REPO_ROOT = Path(__file__).resolve().parents[2]
INSTALL_USER = REPO_ROOT / "scripts/install_user.sh"
UNINSTALL_USER = REPO_ROOT / "scripts/uninstall_user.sh"
LINUX_USER_SERVICE = REPO_ROOT / "scripts/lib/linux_user_service.sh"
MOCK_SYSTEMCTL = REPO_ROOT / "scripts/test/mock-systemctl.sh"
BUILD_LINUX = REPO_ROOT / "scripts/package/build-linux.sh"
POSTINST = REPO_ROOT / "pkg/debian/postinst"
PRERM = REPO_ROOT / "pkg/debian/prerm"

UNIT_HEADER = "[Unit]"


def require_disposable_docker() -> None:
    if not Path("/.dockerenv").is_file():
        raise SystemExit(
            "install_end_to_end.py refuses to run outside a disposable Docker container "
            "(missing /.dockerenv). On the host, run: python3 -m py_compile "
            "scripts/test/install_end_to_end.py"
        )
    if sys.platform != "linux":
        raise SystemExit(f"Linux container required, got platform={sys.platform}")


def log_pass(name: str) -> None:
    print(f"PASS: {name}", flush=True)


def log_fail(name: str, detail: str) -> None:
    print(f"FAIL: {name}: {detail}", flush=True)


@dataclass
class IsolatedEnv:
    root: Path
    xdg_config: Path
    xdg_data: Path
    install_dir: Path
    systemd_user_dir: Path
    systemctl_log: Path
    binary_source: Path
    unit_renderer: Path
    mock_systemctl: Path = MOCK_SYSTEMCTL

    @classmethod
    def create(cls, binary_source: Path) -> IsolatedEnv:
        root = Path(tempfile.mkdtemp(prefix="clipsync-install-e2e-"))
        xdg_config = root / "xdg-config"
        xdg_data = root / "xdg-data"
        install_dir = root / "prefix" / "bin"
        systemd_user_dir = xdg_config / "systemd" / "user"
        for directory in (xdg_config, xdg_data, install_dir, systemd_user_dir):
            directory.mkdir(parents=True, exist_ok=True)
        return cls(
            root=root,
            xdg_config=xdg_config,
            xdg_data=xdg_data,
            install_dir=install_dir,
            systemd_user_dir=systemd_user_dir,
            systemctl_log=root / "systemctl.log",
            binary_source=binary_source.resolve(),
            unit_renderer=binary_source.resolve(),
        )

    def base_env(self, extra: Mapping[str, str] | None = None) -> dict[str, str]:
        env = os.environ.copy()
        env.pop("CLIPSYNC_CONFIG", None)
        env.update(
            {
                "XDG_CONFIG_HOME": str(self.xdg_config),
                "XDG_DATA_HOME": str(self.xdg_data),
                "CLIPSYNC_INSTALL_DIR": str(self.install_dir),
                "CLIPSYNC_SYSTEMCTL": str(self.mock_systemctl),
                "CLIPSYNC_SKIP_PATH_UPDATE": "1",
                "CLIPSYNC_BINARY_SOURCE": str(self.binary_source),
                "CLIPSYNC_UNIT_RENDERER": str(self.unit_renderer),
                "CLIPSYNC_MOCK_SYSTEMCTL_LOG": str(self.systemctl_log),
            }
        )
        if extra:
            env.update(extra)
        return env

    def installed_binary(self) -> Path:
        return self.install_dir / "clipsync"

    def unit_path(self) -> Path:
        return self.systemd_user_dir / "clipsync.service"

    def config_path(self) -> Path:
        return self.xdg_config / "clipsync" / "config.toml"


def run(
    args: Sequence[str],
    *,
    env: Mapping[str, str],
    cwd: Path | None = None,
    input_text: str | None = None,
    timeout: int = 120,
) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        list(args),
        env=dict(env),
        cwd=str(cwd) if cwd else None,
        input=input_text,
        capture_output=True,
        text=True,
        timeout=timeout,
        check=False,
    )


def parse_exec_start_binary(exec_start: str) -> str | None:
    trimmed = exec_start.strip()
    if not trimmed:
        return None
    if trimmed.startswith('"'):
        out: list[str] = []
        escaped = False
        for ch in trimmed[1:]:
            if escaped:
                out.append(ch)
                escaped = False
            elif ch == "\\":
                escaped = True
            elif ch == '"':
                break
            else:
                out.append(ch)
        decoded = "".join(out).replace("%%", "%")
        return decoded
    return trimmed.split()[0]


def inspect_unit(content: str) -> dict[str, object]:
    exec_binary: str | None = None
    uses_foreground = False
    wanted_by_graphical = False
    orders_after_pre = False
    for line in content.splitlines():
        line = line.strip()
        if line.startswith("ExecStart="):
            exec_line = line.removeprefix("ExecStart=").strip()
            uses_foreground = "start --foreground" in exec_line
            exec_binary = parse_exec_start_binary(exec_line)
        elif line.startswith("WantedBy="):
            wanted_by_graphical = "graphical-session.target" in line
        elif line.startswith("After="):
            orders_after_pre = "graphical-session-pre.target" in line
    return {
        "exec_binary": exec_binary,
        "uses_foreground": uses_foreground,
        "wanted_by_graphical_session": wanted_by_graphical,
        "orders_after_graphical_session_pre": orders_after_pre,
    }


def assert_unit_matches_binary(unit_text: str, binary: Path) -> None:
    info = inspect_unit(unit_text)
    expected = str(binary.resolve())
    if info["exec_binary"] != expected:
        raise AssertionError(
            f"unit ExecStart binary {info['exec_binary']!r} != {expected!r}"
        )
    if not info["uses_foreground"]:
        raise AssertionError("unit missing start --foreground")
    if not info["wanted_by_graphical_session"]:
        raise AssertionError("unit missing WantedBy=graphical-session.target")
    if not info["orders_after_graphical_session_pre"]:
        raise AssertionError("unit missing After=graphical-session-pre.target")


def read_systemctl_log(path: Path) -> str:
    if not path.is_file():
        return ""
    return path.read_text(encoding="utf-8", errors="replace")


def run_install_user(env: IsolatedEnv, extra_env: Mapping[str, str] | None = None) -> subprocess.CompletedProcess[str]:
    return run(["bash", str(INSTALL_USER)], env=env.base_env(extra_env))


def run_uninstall_user(
    env: IsolatedEnv, reply: str = "\n", extra_env: Mapping[str, str] | None = None
) -> subprocess.CompletedProcess[str]:
    return run(
        ["bash", str(UNINSTALL_USER)],
        env=env.base_env(extra_env),
        input_text=reply,
    )


def extract_tarball_install_sh() -> str:
    text = BUILD_LINUX.read_text(encoding="utf-8")
    marker = 'cat > "$TARBALL_DIR/install.sh" <<\'EOF\''
    start = text.find(marker)
    if start < 0:
        raise RuntimeError("could not find tarball install.sh heredoc in build-linux.sh")
    start = text.find("\n", start) + 1
    end = text.find("\nEOF", start)
    if end < 0:
        raise RuntimeError("could not find tarball install.sh heredoc terminator")
    return text[start:end] + "\n"


def build_tarball_layout(env: IsolatedEnv) -> Path:
    tarball_root = env.root / "tarball-pkg"
    for sub in (
        "bin",
        "lib",
        "share/systemd/user",
        "share/doc/clipsync",
    ):
        (tarball_root / sub).mkdir(parents=True, exist_ok=True)
    shutil.copy2(env.binary_source, tarball_root / "bin/clipsync")
    shutil.copy2(LINUX_USER_SERVICE, tarball_root / "lib/linux_user_service.sh")
    (tarball_root / "install.sh").write_text(extract_tarball_install_sh(), encoding="utf-8")
    (tarball_root / "install.sh").chmod(0o755)
    return tarball_root


def systemd_analyze_verify(unit_path: Path, env: Mapping[str, str]) -> None:
    if shutil.which("systemd-analyze") is None:
        return
    runtime = Path(env["XDG_RUNTIME_DIR"])
    runtime.mkdir(mode=0o700, parents=True, exist_ok=True)
    proc = run(
        ["systemd-analyze", "--user", "--man=no", "verify", str(unit_path)],
        env=env,
        timeout=30,
    )
    if proc.returncode != 0:
        raise AssertionError(
            "systemd-analyze verify failed: "
            f"rc={proc.returncode} stderr={proc.stderr!r} stdout={proc.stdout!r}"
        )


def write_fail_systemctl(path: Path) -> None:
    path.write_text(
        "#!/usr/bin/env bash\n"
        'echo "forbidden systemctl invocation: $*" >&2\n'
        "exit 1\n",
        encoding="utf-8",
    )
    path.chmod(0o755)


def assert_stdout_is_pure_unit(stdout: str) -> None:
    if not stdout.startswith(UNIT_HEADER):
        raise AssertionError(f"stdout does not start with [Unit]: {stdout[:200]!r}")
    for line in stdout.splitlines():
        if re.match(r"^\d{4}-\d{2}-\d{2}", line):
            raise AssertionError(f"logging prefix leaked into stdout: {line!r}")
        if re.match(r"^(INFO|WARN|ERROR|DEBUG)(:|\s)", line):
            raise AssertionError(f"log level leaked into stdout: {line!r}")


@dataclass
class Results:
    passed: list[str] = field(default_factory=list)
    failed: list[tuple[str, str]] = field(default_factory=list)

    def record_pass(self, name: str) -> None:
        self.passed.append(name)
        log_pass(name)

    def record_fail(self, name: str, detail: str) -> None:
        self.failed.append((name, detail))
        log_fail(name, detail)


def test_fresh_install(env: IsolatedEnv) -> None:
    proc = run_install_user(env)
    if proc.returncode != 0:
        raise AssertionError(f"install_user exit {proc.returncode}: {proc.stderr}")
    binary = env.installed_binary()
    if not binary.is_file():
        raise AssertionError(f"missing installed binary: {binary}")
    unit = env.unit_path().read_text(encoding="utf-8")
    assert_unit_matches_binary(unit, binary)
    config = env.config_path()
    if not config.is_file():
        raise AssertionError(f"missing config: {config}")
    tomllib.loads(config.read_text(encoding="utf-8"))
    log = read_systemctl_log(env.systemctl_log)
    if "scope: --user" not in log or "daemon-reload" not in log or "enable" not in log:
        raise AssertionError(f"mock systemctl log missing expected operations: {log!r}")


def test_reinstall_preserves_config(env: IsolatedEnv) -> None:
    marker = "# preserved-by-acceptance\n"
    env.config_path().write_text(marker, encoding="utf-8")
    proc = run_install_user(env)
    if proc.returncode != 0:
        raise AssertionError(f"reinstall exit {proc.returncode}: {proc.stderr}")
    after = env.config_path().read_text(encoding="utf-8")
    if marker not in after:
        raise AssertionError("config was overwritten on reinstall")


def test_uninstall_newline_preserves_config(env: IsolatedEnv) -> None:
    marker = "# kept-after-uninstall\n"
    env.config_path().write_text(marker, encoding="utf-8")
    proc = run_uninstall_user(env, reply="\n")
    if proc.returncode != 0:
        raise AssertionError(f"uninstall exit {proc.returncode}: {proc.stderr}")
    if env.installed_binary().exists():
        raise AssertionError("binary still present after uninstall")
    if env.unit_path().exists():
        raise AssertionError("unit still present after uninstall")
    if not env.config_path().is_file():
        raise AssertionError("config removed despite newline at prompt")
    if marker not in env.config_path().read_text(encoding="utf-8"):
        raise AssertionError("config content changed after uninstall")


def test_install_systemctl_failure(env: IsolatedEnv, flag: str, label: str) -> None:
    fresh = IsolatedEnv.create(env.binary_source)
    proc = run_install_user(fresh, extra_env={flag: "1"})
    if proc.returncode == 0:
        raise AssertionError(f"install_user succeeded despite {label}")
    if "Installation completed!" in proc.stdout:
        raise AssertionError(f"install_user falsely reported completion on {label}")


def test_tarball_install_from_other_cwd(env: IsolatedEnv) -> None:
    tarball_root = build_tarball_layout(env)
    prefix = env.root / "install prefix with spaces"
    prefix.mkdir(parents=True, exist_ok=True)
    other_cwd = env.root / "unrelated-cwd"
    other_cwd.mkdir()
    runtime = env.root / "runtime"
    runtime.mkdir(mode=0o700, parents=True, exist_ok=True)
    unit_dir = env.xdg_config / "systemd" / "user"
    unit_dir.mkdir(parents=True, exist_ok=True)
    tenv = env.base_env(
        {
            "PREFIX": str(prefix),
            "CLIPSYNC_SYSTEMD_USER_DIR": str(unit_dir),
            "XDG_RUNTIME_DIR": str(runtime),
            "SYSTEMD_UNIT_PATH": str(unit_dir) + ":/usr/lib/systemd/user:/lib/systemd/user",
        }
    )
    proc = run(["bash", str(tarball_root / "install.sh")], env=tenv, cwd=other_cwd)
    if proc.returncode != 0:
        raise AssertionError(
            f"tarball install.sh failed: rc={proc.returncode} stderr={proc.stderr!r}"
        )
    installed = prefix / "bin" / "clipsync"
    if not installed.is_file():
        raise AssertionError(f"tarball install missing binary at {installed}")
    unit = env.unit_path().read_text(encoding="utf-8")
    assert_unit_matches_binary(unit, installed)


def test_special_path_units(env: IsolatedEnv) -> None:
    cases = [
        env.root / "prefix with spaces" / "bin" / "clipsync",
        env.root / "apps" / "a&b$%|" / "clipsync",
        env.root / "日本語" / "clipsync",
    ]
    runtime = env.root / "runtime-special"
    runtime.mkdir(mode=0o700, parents=True, exist_ok=True)
    for idx, dest in enumerate(cases):
        dest.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(env.binary_source, dest)
        dest.chmod(0o755)
        proc = run(
            [str(env.unit_renderer), "print-user-unit", "--binary", str(dest)],
            env=env.base_env(),
        )
        if proc.returncode != 0:
            raise AssertionError(f"print-user-unit failed for {dest}: {proc.stderr}")
        assert_stdout_is_pure_unit(proc.stdout)
        unit_dir = env.xdg_config / f"units-{idx}" / "user"
        unit_dir.mkdir(parents=True, exist_ok=True)
        unit_path = unit_dir / "clipsync.service"
        unit_path.write_text(proc.stdout, encoding="utf-8")
        verify_env = env.base_env(
            {
                "XDG_RUNTIME_DIR": str(runtime),
                "SYSTEMD_UNIT_PATH": str(unit_dir) + ":/usr/lib/systemd/user:/lib/systemd/user",
            }
        )
        systemd_analyze_verify(unit_path, verify_env)
        assert_unit_matches_binary(proc.stdout, dest)


def test_print_user_unit_pure_and_invalid_config(env: IsolatedEnv) -> None:
    binary = env.binary_source
    bad_config = env.root / "not-a-valid-config.toml"
    bad_config.write_text("[[[broken", encoding="utf-8")
    proc = run(
        [str(binary), "print-user-unit", "--binary", str(binary)],
        env=env.base_env({"CLIPSYNC_CONFIG": str(bad_config)}),
    )
    if proc.returncode != 0:
        raise AssertionError(f"print-user-unit failed with invalid CLIPSYNC_CONFIG: {proc.stderr}")
    if proc.stderr.strip():
        raise AssertionError(f"unexpected stderr from print-user-unit: {proc.stderr!r}")
    assert_stdout_is_pure_unit(proc.stdout)


def test_reject_bad_paths_without_clobbering_unit(env: IsolatedEnv) -> None:
    binary = env.installed_binary()
    if not binary.is_file():
        run_install_user(env)
    marker_unit = (
        "[Unit]\nDescription=preserved marker\n"
        "[Service]\nExecStart=/bin/true\n"
        "[Install]\nWantedBy=default.target\n"
    )
    unit_path = env.unit_path()
    unit_path.parent.mkdir(parents=True, exist_ok=True)
    unit_path.write_text(marker_unit, encoding="utf-8")

    for path in ["/quote'dir/clipsync", '/quote"dir/clipsync', "/back\\slash/clipsync"]:
        rejected = run([str(env.unit_renderer), "print-user-unit", "--binary", path], env=env.base_env())
        assert rejected.returncode != 0 and "install directory" in rejected.stderr

    relative = run(
        [str(env.unit_renderer), "print-user-unit", "--binary", "relative/clipsync"],
        env=env.base_env(),
    )
    if relative.returncode == 0:
        raise AssertionError("print-user-unit should reject relative binary path")

    newline = run(
        [str(env.unit_renderer), "print-user-unit", "--binary", "/bad\npath"],
        env=env.base_env(),
    )
    if newline.returncode == 0:
        raise AssertionError("print-user-unit should reject newline in binary path")

    write_proc = run(
        [
            "bash",
            "-c",
            f'source "{LINUX_USER_SERVICE}" && clipsync_write_user_unit "relative/clipsync"',
        ],
        env=env.base_env(),
    )
    if write_proc.returncode == 0:
        raise AssertionError("clipsync_write_user_unit should fail for relative path")
    if unit_path.read_text(encoding="utf-8") != marker_unit:
        raise AssertionError("failed unit write must not replace existing unit file")


def test_debian_maintainer_scripts(env: IsolatedEnv) -> None:
    fail_systemctl = env.root / "systemctl"
    write_fail_systemctl(fail_systemctl)
    scripts_env = env.base_env({"PATH": f"{fail_systemctl.parent}:{os.environ.get('PATH', '')}"})
    before_config = set(env.xdg_config.rglob("*")) if env.xdg_config.exists() else set()
    post = run(["bash", str(POSTINST), "configure"], env=scripts_env)
    if post.returncode != 0:
        raise AssertionError(f"postinst configure failed: {post.stderr!r}")
    prerm = run(["bash", str(PRERM), "remove"], env=scripts_env)
    if prerm.returncode != 0:
        raise AssertionError(f"prerm remove failed: {prerm.stderr!r}")
    after_config = set(env.xdg_config.rglob("*")) if env.xdg_config.exists() else set()
    if after_config != before_config:
        raise AssertionError("maintainer scripts modified user configuration tree")


def test_download_installer(env: IsolatedEnv) -> None:
    fixture = IsolatedEnv.create(env.binary_source)
    mocks = fixture.root / "mock-bin"
    mocks.mkdir()
    archive = fixture.root / "release.tar.gz"
    with tarfile.open(archive, "w:gz") as output:
        output.add(fixture.binary_source, arcname="clipsync")
    curl = mocks / "curl"
    curl.write_text("#!/usr/bin/env python3\nimport os,shutil,sys\na=sys.argv[1:]\nassert a[0]=='-fsSL' and a[2]=='-o'\nassert a[1].endswith('/releases/download/v0.1.0/clipsync-linux-aarch64.tar.gz') or a[1].endswith('/releases/download/v0.1.0/clipsync-linux-x86_64.tar.gz')\nshutil.copyfile(os.environ['CLIPSYNC_FIXTURE_ARCHIVE'],a[3])\n")
    curl.chmod(0o755)
    shutil.copy2(MOCK_SYSTEMCTL, mocks / "systemctl")
    script = (REPO_ROOT / "scripts/install.sh").read_text()
    variables = fixture.base_env({
        "PATH": str(mocks) + ":" + os.environ["PATH"],
        "INSTALL_DIR": str(fixture.install_dir),
        "VERSION": "v0.1.0",
        "CLIPSYNC_FIXTURE_ARCHIVE": str(archive),
    })
    # bash stdin exercises the standalone curl-pipe path without the helper file.
    proc = run(["bash"], env=variables, input_text=script, cwd=fixture.root)
    assert proc.returncode == 0, proc.stderr + proc.stdout
    assert_unit_matches_binary(fixture.unit_path().read_text(), fixture.installed_binary())
    proc = run(["bash"], env={**variables, "CLIPSYNC_MOCK_SYSTEMCTL_ENABLE_FAIL": "1"}, input_text=script, cwd=fixture.root)
    assert proc.returncode != 0, "download installer hid service enable failure"


def run_case(results: Results, name: str, fn) -> None:
    try:
        fn()
        results.record_pass(name)
    except Exception as exc:  # noqa: BLE001 — acceptance harness reports all failures
        results.record_fail(name, str(exc))


def write_result_markdown(results: Results, binary: Path, out_path: Path) -> None:
    out_path.parent.mkdir(parents=True, exist_ok=True)
    status = "PASS" if not results.failed else "FAIL"
    lines = [
        "# ClipSync install acceptance (install_end_to_end.py)",
        "",
        f"- **Status**: {status}",
        f"- **Binary**: `{binary}`",
        f"- **Passed**: {len(results.passed)}",
        f"- **Failed**: {len(results.failed)}",
        "",
    ]
    if results.passed:
        lines.append("## Passed")
        for name in results.passed:
            lines.append(f"- {name}")
        lines.append("")
    if results.failed:
        lines.append("## Failed")
        for name, detail in results.failed:
            lines.append(f"- **{name}**: {detail}")
        lines.append("")
    out_path.write_text("\n".join(lines), encoding="utf-8")


def main(argv: Sequence[str] | None = None) -> int:
    require_disposable_docker()
    parser = argparse.ArgumentParser(description="ClipSync installer acceptance (Docker only)")
    parser.add_argument(
        "binary",
        type=Path,
        help="Path to an already-built Linux clipsync binary",
    )
    parser.add_argument(
        "--result",
        type=Path,
        default=Path("/tmp/clipsync-cursor-workers/install-acceptance-result.md"),
        help="Machine-readable summary output path",
    )
    args = parser.parse_args(argv)
    binary = args.binary.resolve()
    if not binary.is_file():
        print(f"binary not found: {binary}", file=sys.stderr)
        return 2
    if not os.access(binary, os.X_OK):
        print(f"binary is not executable: {binary}", file=sys.stderr)
        return 2

    for path in (
        INSTALL_USER,
        UNINSTALL_USER,
        LINUX_USER_SERVICE,
        MOCK_SYSTEMCTL,
        BUILD_LINUX,
        POSTINST,
        PRERM,
    ):
        if not path.is_file():
            print(f"missing required repo file: {path}", file=sys.stderr)
            return 2

    results = Results()
    base_env = IsolatedEnv.create(binary)
    run_case(results, "download installer through stdin", lambda: test_download_installer(base_env))

    run_case(results, "fresh_install_binary_config_unit", lambda: test_fresh_install(base_env))
    run_case(
        results,
        "reinstall_preserves_config",
        lambda: test_reinstall_preserves_config(base_env),
    )

    uninstall_env = IsolatedEnv.create(binary)
    run_install_user(uninstall_env)
    run_case(
        results,
        "uninstall_newline_preserves_config_removes_binary_unit",
        lambda: test_uninstall_newline_preserves_config(uninstall_env),
    )

    run_case(
        results,
        "install_fails_on_mock_daemon_reload",
        lambda: test_install_systemctl_failure(base_env, "CLIPSYNC_MOCK_SYSTEMCTL_RELOAD_FAIL", "daemon-reload"),
    )
    run_case(
        results,
        "install_fails_on_mock_enable",
        lambda: test_install_systemctl_failure(base_env, "CLIPSYNC_MOCK_SYSTEMCTL_ENABLE_FAIL", "enable"),
    )

    tarball_env = IsolatedEnv.create(binary)
    run_case(
        results,
        "tarball_install_sh_spaces_prefix_other_cwd",
        lambda: test_tarball_install_from_other_cwd(tarball_env),
    )

    special_env = IsolatedEnv.create(binary)
    run_case(
        results,
        "special_path_unit_generation_systemd_analyze",
        lambda: test_special_path_units(special_env),
    )

    pure_env = IsolatedEnv.create(binary)
    run_case(
        results,
        "print_user_unit_stdout_pure_invalid_config",
        lambda: test_print_user_unit_pure_and_invalid_config(pure_env),
    )

    clobber_env = IsolatedEnv.create(binary)
    run_case(
        results,
        "reject_bad_paths_without_clobbering_unit",
        lambda: test_reject_bad_paths_without_clobbering_unit(clobber_env),
    )

    maint_env = IsolatedEnv.create(binary)
    run_case(
        results,
        "debian_postinst_prerm_no_user_config",
        lambda: test_debian_maintainer_scripts(maint_env),
    )

    write_result_markdown(results, binary, args.result)
    print(f"RESULT: {args.result}")
    return 1 if results.failed else 0


if __name__ == "__main__":
    raise SystemExit(main())
