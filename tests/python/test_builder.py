"""Tests for the internal native ShellBuilder API (`strands_shell._native`).

The customer-facing surface is the config-driven `strands_shell.Shell` (see
test_bindings.py). The builder is now an internal detail that the wrapper
translates config into; these tests pin its contract so the regression where
every setter returned None can't recur, and so the wrapper has a stable base.
"""

import os
import shutil
import tempfile

import pytest

from strands_shell import _native


@pytest.fixture
def host_dir():
    path = tempfile.mkdtemp(prefix="strands-shell-builder-test-")
    try:
        yield path
    finally:
        shutil.rmtree(path, ignore_errors=True)


def test_builder_returns_builder_from_setter():
    """Each setter must return the builder so calls can chain."""
    b = _native.Shell.builder()
    same = b.timeout(5.0)
    assert same is not None, "timeout() returned None — chaining is broken"


def test_minimal_chained_build_works():
    shell = _native.Shell.builder().timeout(20.0).build()
    out = shell.run("pwd")
    assert out.status == 0
    assert "/home/lash" in out.stdout


def test_full_chain_with_bind_and_limits(host_dir):
    """Multi-step chains spanning bind, limits, and timeout."""
    shell = (
        _native.Shell.builder()
            .bind_direct(host_dir, "/work")
            .timeout(10.0)
            .max_output(1 << 20)
            .max_file_size(1 << 20)
            .env("FOO", "bar")
            .umask(0o022)
            .build()
    )
    assert shell.get_env("FOO") == "bar"
    out = shell.run("ls /work")
    assert out.status == 0


def test_builder_methods_are_idempotent_when_repeated():
    """Setting the same option twice should keep the latest value."""
    shell = (
        _native.Shell.builder()
            .env("KEY", "first")
            .env("KEY", "second")
            .build()
    )
    assert shell.get_env("KEY") == "second"


def test_builder_cannot_be_reused_after_build():
    """Once built, the builder is consumed; further calls error clearly."""
    b = _native.Shell.builder()
    b.build()
    with pytest.raises(RuntimeError, match="builder consumed"):
        b.build()
    with pytest.raises(RuntimeError, match="builder consumed"):
        b.timeout(5.0)


def test_statement_by_statement_style_still_works(host_dir):
    builder = _native.Shell.builder()
    builder.bind_direct(host_dir, "/work")
    builder.timeout(10.0)
    shell = builder.build()
    assert shell.run("ls /work").status == 0


def test_bind_direct_passthrough_reflects_host_changes(host_dir):
    """Sanity check: bind_direct produces a real passthrough mount."""
    shell = _native.Shell.builder().bind_direct(host_dir, "/work").build()
    # Write on the host side after building
    with open(os.path.join(host_dir, "from_host.txt"), "w") as f:
        f.write("hello")
    assert shell.read_file("/work/from_host.txt") == b"hello"


def test_bind_copy_mode_snapshots_at_build_time(host_dir):
    """Sanity check: bind (copy mode) snapshots at build time, not later."""
    with open(os.path.join(host_dir, "seed.txt"), "w") as f:
        f.write("snapshot")
    shell = _native.Shell.builder().bind(host_dir, "/work").build()
    assert shell.read_file("/work/seed.txt") == b"snapshot"
    # Host changes after build are NOT reflected in copy-mode mount
    with open(os.path.join(host_dir, "added_after.txt"), "w") as f:
        f.write("not visible")
    # Native layer raises NativeShellError (the wrapper maps it to typed errors).
    with pytest.raises(_native.NativeShellError):
        shell.read_file("/work/added_after.txt")
