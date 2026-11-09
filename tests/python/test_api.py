"""Snapshot of the public `strands_shell` API.

Pins the names in `__all__`, the signatures of the public callables, and the
error class hierarchy, so a rename or removal fails here instead of in a
downstream user's code. If a change here is intended, update the snapshot in
the same PR and call it out.
"""

import builtins
import inspect

import pytest

import strands_shell


def test_all_is_exactly_the_public_surface():
    assert strands_shell.__all__ == [
        "Shell",
        "Bind",
        "Cred",
        "Limits",
        "Output",
        "FileInfo",
        "ShellConfig",
        "ConfigBind",
        "ConfigCred",
        "ConfigLimits",
        "ShellError",
        "FileNotFoundError",
        "PermissionDeniedError",
        "FileTooLargeError",
    ]
    for name in strands_shell.__all__:
        assert hasattr(strands_shell, name), name


# ---------------------------------------------------------------------------
# Signatures
# ---------------------------------------------------------------------------

# `strands_shell` uses `from __future__ import annotations`, so annotations are
# strings and render the same on every supported Python version.
CONSTRUCTOR_SIGNATURES = {
    "Shell": (
        "(*, binds: 'list[Bind] | None' = None, "
        "credentials: 'list[Cred] | None' = None, "
        "allowed_urls: 'list[str] | None' = None, "
        "env: 'dict[str, str] | None' = None, "
        "umask: 'int | None' = None, "
        "timeout: 'float | None' = None, "
        "limits: 'Limits | None' = None, "
        "config_file: 'str | None' = None) -> 'None'"
    ),
    "Bind": (
        "(source: 'str', destination: 'str', "
        "mode: \"Literal['direct', 'copy']\" = 'direct', "
        "readonly: 'bool' = False) -> None"
    ),
    "Cred": (
        "(url: 'str', token: 'str | None' = None, "
        "env_var: 'str | None' = None) -> None"
    ),
    "Limits": (
        "(max_output: 'int' = 1048576, max_file_size: 'int' = 10485760, "
        "max_fds: 'int' = 128, max_bg_jobs: 'int' = 8, "
        "max_pipeline: 'int' = 16, max_input: 'int' = 1048576, "
        "max_inodes: 'int' = 10000, max_depth: 'int' = 64) -> None"
    ),
    "ShellConfig": (
        "(binds: 'tuple[ConfigBind, ...]', "
        "credentials: 'tuple[ConfigCred, ...]', "
        "allowed_urls: 'tuple[str, ...]', env: 'dict[str, str]', "
        "umask: 'int', timeout: 'float | None', "
        "limits: 'ConfigLimits') -> None"
    ),
    "ConfigBind": (
        "(source: 'str', destination: 'str', "
        "mode: \"Literal['direct', 'copy']\", readonly: 'bool') -> None"
    ),
    "ConfigCred": (
        "(url: 'str', kind: 'str', methods: 'tuple[str, ...]', "
        "param: 'str | None', env_var: 'str | None', "
        "from_literal: 'bool') -> None"
    ),
    "ConfigLimits": (
        "(max_depth: 'int', max_output: 'int', max_fds: 'int', "
        "max_bg_jobs: 'int', max_pipeline: 'int', max_input: 'int', "
        "max_file_size: 'int', max_inodes: 'int') -> None"
    ),
    "ShellError": "(message: 'str', *, path: 'str' = '') -> 'None'",
}

SHELL_METHOD_SIGNATURES = {
    "run": "(self, command: 'str') -> 'Output'",
    "set_env": "(self, key: 'str', value: 'str') -> 'None'",
    "get_env": "(self, key: 'str') -> 'str | None'",
    "read_file": "(self, path: 'str', **kwargs: 'object') -> 'bytes'",
    "write_file": (
        "(self, path: 'str', content: 'bytes', **kwargs: 'object') -> 'None'"
    ),
    "remove_file": "(self, path: 'str', **kwargs: 'object') -> 'None'",
    "list_files": "(self, path: 'str', **kwargs: 'object') -> 'list[FileInfo]'",
}


@pytest.mark.parametrize("name", sorted(CONSTRUCTOR_SIGNATURES))
def test_constructor_signature(name):
    sig = inspect.signature(getattr(strands_shell, name))
    assert str(sig) == CONSTRUCTOR_SIGNATURES[name]


@pytest.mark.parametrize("name", sorted(SHELL_METHOD_SIGNATURES))
def test_shell_method_signature(name):
    sig = inspect.signature(getattr(strands_shell.Shell, name))
    assert str(sig) == SHELL_METHOD_SIGNATURES[name]


def test_shell_public_members_are_exactly_the_snapshot():
    public = {n for n in vars(strands_shell.Shell) if not n.startswith("_")}
    assert public == set(SHELL_METHOD_SIGNATURES) | {"config"}


def test_shell_config_is_a_read_only_property():
    prop = inspect.getattr_static(strands_shell.Shell, "config")
    assert isinstance(prop, property)
    assert prop.fset is None


def test_native_value_types_expose_their_fields():
    def public(cls):
        return {n for n in dir(cls) if not n.startswith("_")}

    assert public(strands_shell.Output) == {"status", "stdout", "stderr"}
    assert public(strands_shell.FileInfo) == {"name", "is_dir", "size"}


# ---------------------------------------------------------------------------
# Errors
# ---------------------------------------------------------------------------

ERROR_BASES = {
    "ShellError": (Exception,),
    "FileNotFoundError": (strands_shell.ShellError, builtins.FileNotFoundError),
    "PermissionDeniedError": (strands_shell.ShellError, builtins.PermissionError),
    "FileTooLargeError": (strands_shell.ShellError, OSError),
}


@pytest.mark.parametrize("name", sorted(ERROR_BASES))
def test_error_class_name_and_bases(name):
    cls = getattr(strands_shell, name)
    assert cls.__name__ == name
    assert cls.__module__ == "strands_shell"
    assert cls.__bases__ == ERROR_BASES[name]
