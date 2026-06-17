"""Tests for the read-only config snapshot exposed via ``Shell.config``.

The snapshot lets an embedder introspect a constructed shell (binds, the
network allowlist, credential rules, env, umask, timeout, limits) without
having held onto the constructor arguments. Its single most important
guarantee is that it **never leaks secret values** — only the source of a
credential (a literal was supplied, or the name of the env var it reads from).
"""

import os
import shutil
import tempfile

import pytest

import strands_shell
from strands_shell import Bind, Cred, Limits, Shell


@pytest.fixture
def host_dir():
    path = tempfile.mkdtemp(prefix="strands-shell-config-test-")
    try:
        yield path
    finally:
        shutil.rmtree(path, ignore_errors=True)


def test_default_shell_reports_default_config():
    cfg = Shell().config
    assert cfg.binds == ()
    assert cfg.credentials == ()
    assert cfg.allowed_urls == ()
    assert cfg.env == {}
    assert cfg.umask == 0o022
    # The builder defaults to a 30s per-command timeout, and the snapshot
    # reports the real effective value rather than "unset".
    assert cfg.timeout == 30.0
    # Limits report the documented builder defaults.
    assert cfg.limits.max_depth == 64
    assert cfg.limits.max_output == 1 << 20
    assert cfg.limits.max_fds == 128
    assert cfg.limits.max_bg_jobs == 8
    assert cfg.limits.max_pipeline == 16
    assert cfg.limits.max_input == 1 << 20
    assert cfg.limits.max_file_size == 10 << 20
    assert cfg.limits.max_inodes == 10_000


def test_config_reports_binds(host_dir):
    cfg = Shell(
        binds=[
            Bind(host_dir, "/work", mode="direct", readonly=True),
            Bind(host_dir, "/copy", mode="copy"),
        ]
    ).config
    assert len(cfg.binds) == 2
    assert cfg.binds[0].source == host_dir
    assert cfg.binds[0].destination == "/work"
    assert cfg.binds[0].mode == "direct"
    assert cfg.binds[0].readonly is True
    assert cfg.binds[1].mode == "copy"
    assert cfg.binds[1].readonly is False


def test_config_reports_allowed_urls_env_umask_timeout():
    cfg = Shell(
        allowed_urls=["https://api.example.com/", "https://api.openai.com/"],
        env={"PROJECT": "demo", "STAGE": "prod"},
        umask=0o027,
        timeout=12.5,
    ).config
    assert cfg.allowed_urls == ("https://api.example.com/", "https://api.openai.com/")
    assert cfg.env == {"PROJECT": "demo", "STAGE": "prod"}
    assert cfg.umask == 0o027
    assert cfg.timeout == 12.5


def test_config_reports_overridden_limits():
    cfg = Shell(limits=Limits(max_output=2048, max_inodes=500)).config
    assert cfg.limits.max_output == 2048
    assert cfg.limits.max_inodes == 500


def test_config_credentials_never_leak_literal_token():
    cfg = Shell(
        credentials=[Cred("https://api.example.com/*", token="sk-super-secret")]
    ).config
    cred = cfg.credentials[0]
    assert cred.url == "https://api.example.com/*"
    assert cred.kind == "bearer"
    assert cred.from_literal is True
    assert cred.env_var is None
    # The secret itself must appear nowhere in the snapshot.
    assert "sk-super-secret" not in repr(cfg)


def test_config_credentials_report_env_var_name_not_value():
    os.environ["STRANDS_SHELL_TEST_TOKEN"] = "value-must-not-leak"
    try:
        cfg = Shell(
            credentials=[
                Cred("https://api.openai.com/*", env_var="STRANDS_SHELL_TEST_TOKEN")
            ]
        ).config
    finally:
        del os.environ["STRANDS_SHELL_TEST_TOKEN"]
    cred = cfg.credentials[0]
    assert cred.env_var == "STRANDS_SHELL_TEST_TOKEN"
    assert cred.from_literal is False
    assert "value-must-not-leak" not in repr(cfg)


def test_config_snapshot_is_frozen():
    cfg = Shell().config
    with pytest.raises(Exception):
        cfg.umask = 0  # type: ignore[misc]


def test_config_is_a_snapshot_not_a_live_view():
    # Mutating the shell's env after construction must not change the snapshot
    # that was already taken (it reflects build-time configuration).
    shell = Shell(env={"A": "1"})
    cfg = shell.config
    shell.set_env("A", "2")
    assert cfg.env == {"A": "1"}


def test_config_reexported_types_are_public():
    # The public dataclasses are importable from the package root.
    assert hasattr(strands_shell, "ShellConfig")
    assert hasattr(strands_shell, "ConfigBind")
    assert hasattr(strands_shell, "ConfigCred")
    assert hasattr(strands_shell, "ConfigLimits")
