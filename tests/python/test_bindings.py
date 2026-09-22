"""Pytest suite for the v0.1 Python bindings.

Covers the four file-operation methods on `strands_shell.Shell` plus the `FileInfo`
pyclass. Run with:

    .venv/bin/pytest tests/python -v

Each test builds its own `Shell` so failures stay isolated.
"""

import math
import os
import shutil
import tempfile

import pytest

import strands_shell


@pytest.fixture
def host_dir():
    """A throwaway host directory bound to /work in the shell's VFS."""
    path = tempfile.mkdtemp(prefix="strands-shell-bindings-test-")
    try:
        yield path
    finally:
        shutil.rmtree(path, ignore_errors=True)


@pytest.fixture
def shell(host_dir):
    """A shell with `host_dir` bound to /work via passthrough.

    `bind_direct` (rather than copy-mode `bind`) keeps host and VFS in sync,
    which lets tests seed via the host filesystem and observe through the
    shell. This matches how Strands users will normally use Strands Shell.

    Uses the config-driven constructor that is the public Python API, so every
    test that uses this fixture implicitly exercises it.
    """
    return strands_shell.Shell(
        binds=[strands_shell.Bind(host_dir, "/work", mode="direct")],
        timeout=10.0,
    )


def _seed(host_dir, rel_path, content):
    full = os.path.join(host_dir, rel_path)
    os.makedirs(os.path.dirname(full), exist_ok=True) if os.path.dirname(rel_path) else None
    with open(full, "wb") as f:
        f.write(content if isinstance(content, bytes) else content.encode())


# ---------------------------------------------------------------------------
# read_file
# ---------------------------------------------------------------------------

def test_read_file_returns_bytes(host_dir, shell):
    _seed(host_dir, "seed.txt", "hello from host")
    data = shell.read_file("/work/seed.txt")
    assert isinstance(data, bytes)
    assert data == b"hello from host"


def test_read_file_missing_raises_not_found(shell):
    # Typed: strands_shell.FileNotFoundError, which also inherits builtins.FileNotFoundError.
    with pytest.raises(strands_shell.FileNotFoundError) as exc_info:
        shell.read_file("/work/does-not-exist")
    err = exc_info.value
    assert isinstance(err, strands_shell.ShellError)
    assert isinstance(err, FileNotFoundError)  # stdlib builtin
    assert err.path == "/work/does-not-exist"


def test_read_file_preserves_full_byte_range(shell):
    payload = bytes(range(256))
    shell.write_file("/work/binary.bin", payload)
    assert shell.read_file("/work/binary.bin") == payload


def test_read_file_respects_max_file_size(host_dir):
    """A read larger than max_file_size must surface as FileTooLargeError
    rather than loading an unbounded payload into memory — important for
    direct-passthrough mounts to large host files. Seeded on the host so the
    file already exceeds the cap at read time."""
    _seed(host_dir, "big.txt", b"x" * 4096)
    shell = strands_shell.Shell(
        binds=[strands_shell.Bind(host_dir, "/work", mode="direct")],
        limits=strands_shell.Limits(max_file_size=1024),
    )
    with pytest.raises(strands_shell.FileTooLargeError) as exc_info:
        shell.read_file("/work/big.txt")
    assert isinstance(exc_info.value, strands_shell.ShellError)
    assert exc_info.value.path == "/work/big.txt"


def test_read_file_at_max_file_size_boundary_succeeds(host_dir):
    """A read exactly at the cap is allowed."""
    _seed(host_dir, "exact.txt", b"y" * 1024)
    shell = strands_shell.Shell(
        binds=[strands_shell.Bind(host_dir, "/work", mode="direct")],
        limits=strands_shell.Limits(max_file_size=1024),
    )
    assert shell.read_file("/work/exact.txt") == b"y" * 1024


# ---------------------------------------------------------------------------
# write_file
# ---------------------------------------------------------------------------

def test_write_file_creates_parent_directories(shell):
    shell.write_file("/work/deep/dir/binary.bin", b"data")
    assert shell.read_file("/work/deep/dir/binary.bin") == b"data"


def test_write_file_truncates_on_overwrite(host_dir, shell):
    _seed(host_dir, "seed.txt", "hello from host")
    shell.write_file("/work/seed.txt", b"short")
    assert shell.read_file("/work/seed.txt") == b"short"


def test_write_file_accepts_empty_payload(shell):
    shell.write_file("/work/empty.txt", b"")
    assert shell.read_file("/work/empty.txt") == b""


def test_write_file_at_root_of_bind_mount(shell):
    shell.write_file("/work/root_level.bin", b"\x00\x01\x02")
    assert shell.read_file("/work/root_level.bin") == b"\x00\x01\x02"


def test_write_file_handles_large_payload(shell):
    """2 MiB exceeds the 8 KiB drain pipe; exercises the stall-detection bound."""
    big = bytes((i * 31) & 0xff for i in range(2 * 1024 * 1024))
    shell.write_file("/work/big.bin", big)
    assert shell.read_file("/work/big.bin") == big


def test_write_file_pure_vfs_path(shell):
    """Pure-VFS write (no bind mount) goes through the kernel's in-memory drain."""
    shell.write_file("/tmp/vfs_only.txt", b"in-memory")
    assert shell.read_file("/tmp/vfs_only.txt") == b"in-memory"


def test_write_file_size_limit_surfaces_clear_error(host_dir):
    """Writing past max_file_size must error as FileTooLargeError, not look
    like a timeout."""
    shell = strands_shell.Shell(
        binds=[strands_shell.Bind(host_dir, "/work", mode="copy")],
        limits=strands_shell.Limits(max_file_size=64),
    )
    with pytest.raises(strands_shell.FileTooLargeError) as exc_info:
        shell.write_file("/work/too_big.bin", b"x" * 1024)
    assert isinstance(exc_info.value, strands_shell.ShellError)


# ---------------------------------------------------------------------------
# remove_file
# ---------------------------------------------------------------------------

def test_remove_file_removes_entry(host_dir, shell):
    _seed(host_dir, "seed.txt", "hello")
    shell.remove_file("/work/seed.txt")
    names = {e.name for e in shell.list_files("/work")}
    assert "seed.txt" not in names


def test_remove_file_missing_raises_not_found(shell):
    with pytest.raises(strands_shell.FileNotFoundError) as exc_info:
        shell.remove_file("/work/does-not-exist")
    assert isinstance(exc_info.value, strands_shell.ShellError)


# ---------------------------------------------------------------------------
# list_files
# ---------------------------------------------------------------------------

def test_list_files_returns_structured_file_info(host_dir, shell):
    _seed(host_dir, "seed.txt", "short")  # 5 bytes
    os.makedirs(os.path.join(host_dir, "sub"))
    entries = shell.list_files("/work")
    by_name = {e.name: e for e in entries}
    assert "seed.txt" in by_name
    assert "sub" in by_name
    assert by_name["seed.txt"].is_dir is False
    assert by_name["sub"].is_dir is True
    assert by_name["seed.txt"].size == 5
    assert by_name["sub"].size is None  # directories don't carry a size


def test_list_files_on_nested_directory(host_dir, shell):
    os.makedirs(os.path.join(host_dir, "sub"))
    _seed(host_dir, "sub/nested.txt", "nested file")
    nested = shell.list_files("/work/sub")
    assert {e.name for e in nested} == {"nested.txt"}
    assert nested[0].is_dir is False


def test_list_files_missing_raises_not_found(shell):
    with pytest.raises(strands_shell.FileNotFoundError) as exc_info:
        shell.list_files("/work/no/such/dir")
    assert isinstance(exc_info.value, strands_shell.ShellError)


# ---------------------------------------------------------------------------
# Integration with shell.run() and FileInfo repr
# ---------------------------------------------------------------------------

def test_run_interleaves_with_native_vfs_calls(shell):
    shell.write_file("/work/from_python.txt", b"hi")
    out = shell.run("ls /work")
    assert out.status == 0
    assert "from_python.txt" in out.stdout


def test_file_info_repr_is_pythonic(shell):
    shell.write_file("/work/x.txt", b"data")
    entry = next(e for e in shell.list_files("/work") if e.name == "x.txt")
    rep = repr(entry)
    assert rep.startswith("FileInfo(")
    assert "is_dir=False" in rep  # not Some(false)
    assert "size=4" in rep  # not Some(4)


# ---------------------------------------------------------------------------
# config_file merge semantics
# ---------------------------------------------------------------------------

def test_config_file_values_are_not_clobbered_by_defaults(tmp_path):
    """A value set only in config_file must survive — defaulting a constructor
    arg must NOT silently overwrite it. Regression for the merge bug where
    umask/timeout/limits were applied unconditionally."""
    cfg = tmp_path / "shell.toml"
    cfg.write_text('umask = "077"\n')
    shell = strands_shell.Shell(config_file=str(cfg))
    # umask from the file (077) must hold, not the binding's old 0o022 default.
    assert shell.run("umask").stdout.strip() == "0077"


def test_explicit_arg_overrides_config_file(tmp_path):
    """An explicitly passed arg still wins over the config_file value."""
    cfg = tmp_path / "shell.toml"
    cfg.write_text('umask = "077"\n')
    shell = strands_shell.Shell(config_file=str(cfg), umask=0o022)
    assert shell.run("umask").stdout.strip() == "0022"


# --------------------------------------------------------------------------- #
# timeout validation (Shell rejects non-positive / non-finite)
# --------------------------------------------------------------------------- #


def test_zero_timeout_raises_value_error():
    with pytest.raises(ValueError, match="positive, finite"):
        strands_shell.Shell(timeout=0)


@pytest.mark.parametrize("bad", [-1.0, math.nan, math.inf, -math.inf])
def test_negative_or_nonfinite_timeout_raises_value_error(bad):
    with pytest.raises(ValueError, match="positive, finite"):
        strands_shell.Shell(timeout=bad)


def test_omitted_and_positive_timeout_allowed():
    # Omitted timeout => no limit; a positive value => bounded. Both build.
    assert strands_shell.Shell().run("echo ok").stdout.strip() == "ok"
    assert strands_shell.Shell(timeout=5.0).run("echo ok").stdout.strip() == "ok"


# --------------------------------------------------------------------------- #
# Cedar authorization policies
# --------------------------------------------------------------------------- #

_READ_ONLY_POLICY = (
    'permit(principal, action in ['
    'Agent::Action::"fs:read", Agent::Action::"fs:stat", Agent::Action::"fs:list"'
    '], resource);'
)


def test_policy_str_denies_unpermitted_action():
    """An inline read-only policy allows reads but denies writes."""
    shell = strands_shell.Shell(policy=_READ_ONLY_POLICY)
    assert shell.run("ls /").status == 0
    # A write is not permitted by the policy => denied, non-zero exit.
    assert shell.run("echo hi > /home/lash/x.txt").status != 0


def test_policy_file_loads_from_path(tmp_path):
    """A policy supplied as a file path behaves like the inline form."""
    policy = tmp_path / "read-only.cedar"
    policy.write_text(_READ_ONLY_POLICY)
    shell = strands_shell.Shell(policy_file=str(policy))
    assert shell.run("ls /").status == 0
    assert shell.run("echo hi > /home/lash/x.txt").status != 0


def test_no_policy_is_unchanged():
    """With no policy, writes are allowed (default-allow)."""
    shell = strands_shell.Shell()
    assert shell.run("echo hi > /home/lash/x.txt && cat /home/lash/x.txt").status == 0


def test_policy_and_policy_file_are_mutually_exclusive(tmp_path):
    policy = tmp_path / "p.cedar"
    policy.write_text(_READ_ONLY_POLICY)
    with pytest.raises(ValueError, match="at most one"):
        strands_shell.Shell(policy=_READ_ONLY_POLICY, policy_file=str(policy))


def test_malformed_policy_raises():
    """A malformed policy fails at construction (build())."""
    with pytest.raises(Exception):
        strands_shell.Shell(policy="permit(garbage")
