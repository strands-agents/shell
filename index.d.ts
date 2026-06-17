// @strands-agents/shell — public TypeScript declarations.
//
// Hand-authored wrapper types over the napi-generated native binding
// (see native.d.ts). The customer-facing surface is the config-driven
// `Shell.create()` plus the typed `ShellError` hierarchy; the fluent
// builder in native.d.ts is internal.

/** Output from a shell command execution. */
export interface Output {
  status: number
  stdout: string
  stderr: string
}

/** Metadata about a file or directory in the VFS. */
export interface FileInfo {
  readonly name: string
  readonly isDir?: boolean
  readonly size?: number
}

/** A bind-mount entry mapping a host path into the VFS. */
export interface BindConfig {
  source: string
  destination: string
  /** `'direct'` passthrough (default) or `'copy'` build-time snapshot. */
  mode?: 'direct' | 'copy'
  /** Reject writes through this mount. Default false. */
  readonly?: boolean
}

/** A credential injection rule. Exactly one of `token` / `envVar` must be set. */
export interface CredConfig {
  url: string
  token?: string
  envVar?: string
}

/** Resource caps. Every field is optional and independently defaulted. */
export interface ShellLimits {
  maxOutput?: number
  maxFileSize?: number
  maxFds?: number
  maxBgJobs?: number
  maxPipeline?: number
  maxInput?: number
  maxInodes?: number
  maxDepth?: number
}

/** Options accepted by `Shell.create()`. All fields optional. */
export interface ShellConfig {
  binds?: BindConfig[]
  credentials?: CredConfig[]
  allowedUrls?: string[]
  env?: Record<string, string>
  /** File-creation umask. Default 0o022. */
  umask?: number
  /** Per-command wall-clock timeout in seconds. Default 30. */
  timeout?: number
  limits?: ShellLimits
  /** Path to a TOML config file; merges in. */
  configFile?: string
}

/** A bind mount in a {@link ShellConfigSnapshot}. */
export interface BindInfo {
  readonly source: string
  readonly destination: string
  /** `'copy'` (build-time snapshot) or `'direct'` (host passthrough). */
  readonly mode: 'copy' | 'direct'
  readonly readonly: boolean
}

/**
 * A credential rule in a {@link ShellConfigSnapshot}.
 *
 * The secret value is never exposed. `envVar` holds the environment-variable
 * name when the credential was configured from the environment; `fromLiteral`
 * is `true` when a literal token was supplied directly (its value is withheld).
 */
export interface CredInfo {
  readonly url: string
  readonly kind: 'bearer' | 'query'
  readonly methods: readonly string[]
  readonly param: string | null
  readonly envVar: string | null
  readonly fromLiteral: boolean
}

/** Resource caps in a {@link ShellConfigSnapshot}. Every cap is present. */
export interface LimitsInfo {
  readonly maxDepth: number
  readonly maxOutput: number
  readonly maxFds: number
  readonly maxBgJobs: number
  readonly maxPipeline: number
  readonly maxInput: number
  readonly maxFileSize: number
  readonly maxInodes: number
}

/**
 * A read-only snapshot of how a {@link Shell} was configured, returned by
 * {@link Shell.config}. Lets an embedder introspect a constructed shell — to
 * build tool descriptions, surface the network allowlist, or report active
 * limits — without having held onto the {@link ShellConfig} it was built from.
 * Secret values are never included.
 */
export interface ShellConfigSnapshot {
  readonly binds: readonly BindInfo[]
  readonly credentials: readonly CredInfo[]
  readonly allowedUrls: readonly string[]
  readonly env: Readonly<Record<string, string>>
  readonly umask: number
  /** Per-command timeout in seconds, or `null` for no timeout. */
  readonly timeout: number | null
  readonly limits: LimitsInfo
}

/** errno-style discriminator carried on every {@link ShellError}. */
export type ShellErrorCode = 'ENOENT' | 'EACCES' | 'EFBIG' | 'EOTHER'

/** Base error for file operations. Carries `.path` and `.code`. */
export declare class ShellError extends Error {
  readonly path: string
  readonly code: ShellErrorCode
}
/** A path did not exist. `code === 'ENOENT'`. */
export declare class NotFoundError extends ShellError {}
/** A write/remove was blocked — read-only mount or policy. `code === 'EACCES'`. */
export declare class PermissionDeniedError extends ShellError {}
/** `maxFileSize` / `maxInodes` exceeded. `code === 'EFBIG'`. */
export declare class FileTooLargeError extends ShellError {}

/** A sandboxed shell environment. */
export declare class Shell {
  private constructor()
  /** Create a sandboxed shell from a config object. */
  static create(config?: ShellConfig): Promise<Shell>
  /** Run a command and capture output. Resolves even on non-zero exit. */
  run(command: string): Promise<Output>
  /** Set an environment variable (in-process state). */
  setEnv(key: string, value: string): Promise<void>
  /** Get an environment variable. */
  getEnv(key: string): Promise<string | null>
  /**
   * A read-only snapshot of how this shell was configured. Secret values are
   * never included — see {@link CredInfo}. The returned object is deep-frozen.
   */
  config(): Promise<ShellConfigSnapshot>
  /** Read a file as raw bytes. Rejects with {@link NotFoundError} if missing. */
  readFile(path: string): Promise<Uint8Array>
  /** Write raw bytes; creates parent dirs (mkdir -p) and truncates. */
  writeFile(path: string, content: Uint8Array): Promise<void>
  /** Remove a file. Rejects with {@link NotFoundError} if missing. */
  removeFile(path: string): Promise<void>
  /** List directory entries as {@link FileInfo} (basenames only). */
  listFiles(path: string): Promise<FileInfo[]>
}
