"""Strands Shell — a virtual shell sandbox for AI agents.

This package is the customer-facing Python API. It wraps the native extension
(``strands_shell._native``, built from Rust via maturin) with:

* a config-driven :class:`Shell` constructor (flat keyword args plus the
  :class:`Bind` / :class:`Cred` / :class:`Limits` option dataclasses), mirroring
  the ``Agent`` / ``Swarm`` constructor shape in the Strands SDK, and
* a typed :class:`ShellError` exception hierarchy whose subclasses also inherit
  the matching stdlib exceptions, so adapter code can ``except FileNotFoundError``
  directly.
"""

from __future__ import annotations

import builtins
import math
from dataclasses import dataclass
from typing import Literal

from strands_shell import _native

__all__ = [
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

# Value types are re-exported straight from the native module — they are plain
# data carriers with the right attribute names already.
Output = _native.Output
FileInfo = _native.FileInfo


# --------------------------------------------------------------------------- #
# Option dataclasses
# --------------------------------------------------------------------------- #


@dataclass(frozen=True)
class Bind:
    """A bind-mount entry mapping a host path into the VFS.

    ``mode="direct"`` is host passthrough (host-side writes after construction
    are visible, and VFS writes hit the host); ``mode="copy"`` snapshots the
    host directory into the VFS at construction time. ``readonly=True`` rejects
    writes through the mount.
    """

    source: str
    destination: str
    mode: Literal["direct", "copy"] = "direct"
    readonly: bool = False


@dataclass(frozen=True)
class Cred:
    """A credential injection rule.

    Exactly one of ``token`` / ``env_var`` must be set. ``env_var`` is resolved
    against the process environment when the :class:`Shell` is constructed.
    """

    url: str
    token: str | None = None
    env_var: str | None = None

    def __post_init__(self) -> None:
        if (self.token is None) == (self.env_var is None):
            raise ValueError(
                "Cred requires exactly one of `token` or `env_var` to be set"
            )


@dataclass(frozen=True)
class Limits:
    """Resource caps for a :class:`Shell`.

    Bundled together (rather than as flat constructor kwargs) so the behavioral
    settings on the constructor stay visually separate from the protective
    caps. Mirrors the MCP server's ``[limits]`` TOML table. Override only the
    caps you care about; the rest keep their defaults.
    """

    max_output: int = 1 << 20  # 1 MiB
    max_file_size: int = 10 << 20  # 10 MiB
    max_fds: int = 128
    max_bg_jobs: int = 8
    max_pipeline: int = 16
    max_input: int = 1 << 20  # 1 MiB
    max_inodes: int = 10_000
    max_depth: int = 64


# --------------------------------------------------------------------------- #
# Read-only config snapshot (returned by ``Shell.config``)
# --------------------------------------------------------------------------- #


@dataclass(frozen=True)
class ConfigBind:
    """A bind mount as reported by :attr:`Shell.config`.

    Read-only view; mirrors the :class:`Bind` you pass in, with ``mode``
    normalized to ``"copy"`` / ``"direct"``.
    """

    source: str
    destination: str
    mode: Literal["direct", "copy"]
    readonly: bool


@dataclass(frozen=True)
class ConfigCred:
    """A credential rule as reported by :attr:`Shell.config`.

    The secret value is **never** exposed. ``env_var`` holds the environment
    variable name when the credential was configured from the environment;
    ``from_literal`` is ``True`` when a literal token was supplied directly
    (its value is still withheld).
    """

    url: str
    kind: str
    methods: tuple[str, ...]
    param: str | None
    env_var: str | None
    from_literal: bool


@dataclass(frozen=True)
class ConfigLimits:
    """Resource caps as reported by :attr:`Shell.config`.

    Unlike :class:`Limits` (the input bundle), this view always carries every
    cap with its concrete active value.
    """

    max_depth: int
    max_output: int
    max_fds: int
    max_bg_jobs: int
    max_pipeline: int
    max_input: int
    max_file_size: int
    max_inodes: int


@dataclass(frozen=True)
class ShellConfig:
    """A read-only snapshot of how a :class:`Shell` was configured.

    Returned by :attr:`Shell.config`. Lets an embedder introspect a constructed
    shell after the fact (to build tool descriptions, surface the network
    allowlist, or report active limits) without having held onto the
    construction arguments. Secret values are never included.
    """

    binds: tuple[ConfigBind, ...]
    credentials: tuple[ConfigCred, ...]
    allowed_urls: tuple[str, ...]
    env: dict[str, str]
    umask: int
    timeout: float | None
    limits: ConfigLimits


def _snapshot_from_native(native_config: object) -> ShellConfig:
    """Convert a ``_native.ShellConfig`` into the frozen public dataclass."""
    return ShellConfig(
        binds=tuple(
            ConfigBind(
                source=b.source,
                destination=b.destination,
                mode=b.mode,  # type: ignore[arg-type]
                readonly=b.readonly,
            )
            for b in native_config.binds
        ),
        credentials=tuple(
            ConfigCred(
                url=c.url,
                kind=c.kind,
                methods=tuple(c.methods),
                param=c.param,
                env_var=c.env_var,
                from_literal=c.from_literal,
            )
            for c in native_config.credentials
        ),
        allowed_urls=tuple(native_config.allowed_urls),
        env=dict(native_config.env),
        umask=native_config.umask,
        timeout=native_config.timeout,
        limits=ConfigLimits(
            max_depth=native_config.limits.max_depth,
            max_output=native_config.limits.max_output,
            max_fds=native_config.limits.max_fds,
            max_bg_jobs=native_config.limits.max_bg_jobs,
            max_pipeline=native_config.limits.max_pipeline,
            max_input=native_config.limits.max_input,
            max_file_size=native_config.limits.max_file_size,
            max_inodes=native_config.limits.max_inodes,
        ),
    )


# --------------------------------------------------------------------------- #
# Exception hierarchy
# --------------------------------------------------------------------------- #


class ShellError(Exception):
    """Base for all Strands Shell file-op failures.

    Carries the offending ``path`` and the kernel ``message``. Subclasses
    differentiate the common failure types and additionally inherit the
    matching stdlib exception, so ``except FileNotFoundError`` catches
    :class:`FileNotFoundError` below without any translation shim.
    """

    def __init__(self, message: str, *, path: str = "") -> None:
        super().__init__(message)
        self.path = path
        self.message = message


class FileNotFoundError(ShellError, builtins.FileNotFoundError):
    """A path did not exist (read / remove / list of a missing path)."""


class PermissionDeniedError(ShellError, builtins.PermissionError):
    """A write or remove was blocked — read-only mount or mount policy."""


class FileTooLargeError(ShellError, OSError):
    """``max_file_size`` or ``max_inodes`` was exceeded on write."""


# Map the native error's `kind` discriminator onto the typed subclasses.
_ERROR_BY_KIND = {
    "not_found": FileNotFoundError,
    "permission_denied": PermissionDeniedError,
    "too_large": FileTooLargeError,
    "other": ShellError,
}


def _raise_typed(exc: BaseException) -> "ShellError":
    """Translate a ``_native.NativeShellError`` into the typed hierarchy."""
    kind = getattr(exc, "kind", "other")
    path = getattr(exc, "path", "")
    message = getattr(exc, "message", str(exc))
    cls = _ERROR_BY_KIND.get(kind, ShellError)
    return cls(message, path=path)


# --------------------------------------------------------------------------- #
# Shell
# --------------------------------------------------------------------------- #


class Shell:
    """A sandboxed shell environment.

    Constructed directly with config — no builder, no factory. Mounts and
    credentials go in as lists of :class:`Bind` / :class:`Cred`; resource caps
    go in a single :class:`Limits` bundle; behavioral settings (``env``,
    ``umask``, ``timeout``) are top-level keyword args.

    A Cedar authorization policy can be attached with ``policy_file`` (a path)
    or ``policy`` (the policy text inline) — pass at most one. A policy only
    ever *adds* restrictions on top of the mounts and SSRF guard; with none,
    behavior is unchanged. See the README's "Authorization Policies" section.

    State (cwd, env, functions, open fds) persists across :meth:`run` calls.
    There is no ``close()`` — the embedded interpreter and in-process VFS are
    released by refcounting when the last reference drops.
    """

    def __init__(
        self,
        *,
        binds: list[Bind] | None = None,
        credentials: list[Cred] | None = None,
        allowed_urls: list[str] | None = None,
        env: dict[str, str] | None = None,
        umask: int | None = None,
        timeout: float | None = None,
        limits: Limits | None = None,
        config_file: str | None = None,
        policy_file: str | None = None,
        policy: str | None = None,
    ) -> None:
        if policy_file is not None and policy is not None:
            raise ValueError("pass at most one of `policy_file` or `policy`")

        builder = _native.Shell.builder()

        # config_file merges in first; explicit args below win over it. Each
        # behavioral/limit setting is applied only when the caller actually
        # passed it (``None`` means "unset"), so defaulting an argument never
        # silently clobbers a value the TOML set. The Rust core supplies the
        # real defaults when nothing is configured here or in the file.
        if config_file is not None:
            builder.config_file(config_file)

        # A Cedar policy passed explicitly here overrides any `policy` key the
        # config file set (same "explicit args win" rule as above).
        if policy_file is not None:
            builder.policy_file(policy_file)
        elif policy is not None:
            builder.policy_str(policy)

        for b in binds or []:
            if b.mode == "direct" and b.readonly:
                builder.bind_direct_readonly(b.source, b.destination)
            elif b.mode == "direct":
                builder.bind_direct(b.source, b.destination)
            elif b.readonly:
                builder.bind_readonly(b.source, b.destination)
            else:
                builder.bind(b.source, b.destination)

        for c in credentials or []:
            if c.token is not None:
                builder.credential(c.url, c.token)
            else:
                builder.credential_from_env(c.url, c.env_var)

        for prefix in allowed_urls or []:
            builder.allow_url(prefix)

        for key, value in (env or {}).items():
            builder.env(key, value)

        if umask is not None:
            builder.umask(umask)
        if timeout is not None:
            # Reject non-positive / non-finite up front: zero would expire every
            # command immediately (there is no "unlimited" sentinel — omit
            # timeout instead), and a negative/NaN/inf value would panic the
            # native Duration::from_secs_f64 across the FFI boundary.
            if not math.isfinite(timeout) or timeout <= 0:
                raise ValueError(
                    "timeout must be a positive, finite number of seconds "
                    "(omit it for no timeout)"
                )
            builder.timeout(timeout)
        if limits is not None:
            builder.max_output(limits.max_output)
            builder.max_file_size(limits.max_file_size)
            builder.max_fds(limits.max_fds)
            builder.max_bg_jobs(limits.max_bg_jobs)
            builder.max_pipeline(limits.max_pipeline)
            builder.max_input(limits.max_input)
            builder.max_inodes(limits.max_inodes)
            builder.max_depth(limits.max_depth)

        self._shell = builder.build()

    # ---- Command execution ----

    def run(self, command: str) -> Output:
        """Run a command and capture its output. Never raises for command-level
        failures — check :attr:`Output.status`."""
        return self._shell.run(command)

    # ---- Environment ----

    def set_env(self, key: str, value: str) -> None:
        self._shell.set_env(key, value)

    def get_env(self, key: str) -> str | None:
        return self._shell.get_env(key)

    # ---- Configuration introspection ----

    @property
    def config(self) -> ShellConfig:
        """A read-only snapshot of how this shell was configured.

        Reports bind mounts, credential rules, the network allowlist, seeded
        environment variables, umask, timeout, and resource caps. Useful for
        introspecting a constructed shell — e.g. to build tool descriptions or
        surface the allowlist — without having held onto the constructor args.

        Secret values are never included: each :class:`ConfigCred` reports its
        URL pattern, kind, and source (literal vs environment variable name),
        but never the token itself.
        """
        return _snapshot_from_native(self._shell.config())

    # ---- VFS file operations ----
    # Each accepts **kwargs and ignores unknown keys, matching the
    # kwargs-tolerant strands.sandbox.Sandbox contract the adapter passes
    # through to.

    def read_file(self, path: str, **kwargs: object) -> bytes:
        try:
            return self._shell.read_file(path)
        except _native.NativeShellError as exc:
            raise _raise_typed(exc) from None

    def write_file(self, path: str, content: bytes, **kwargs: object) -> None:
        try:
            self._shell.write_file(path, content)
        except _native.NativeShellError as exc:
            raise _raise_typed(exc) from None

    def remove_file(self, path: str, **kwargs: object) -> None:
        try:
            self._shell.remove_file(path)
        except _native.NativeShellError as exc:
            raise _raise_typed(exc) from None

    def list_files(self, path: str, **kwargs: object) -> list[FileInfo]:
        try:
            return self._shell.list_files(path)
        except _native.NativeShellError as exc:
            raise _raise_typed(exc) from None
