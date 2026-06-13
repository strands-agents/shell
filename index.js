// @strands-agents/shell — customer-facing JS API.
//
// Wraps the napi-generated native loader (`native.js`) with:
//   * a config-driven `Shell.create(config)` factory (the native layer exposes
//     a fluent builder, which we keep internal), and
//   * a typed `ShellError` hierarchy. The native file ops reject with an Error
//     whose message is a tab-delimited "{code}\t{path}\t{message}" envelope
//     (set in src/js.rs::file_error); we parse it and re-throw the matching
//     subclass with `.code` / `.path` / `.message` properties.

const native = require('./native.js')

// --- Error hierarchy ------------------------------------------------------

class ShellError extends Error {
  constructor(message, { path = '', code = 'EOTHER' } = {}) {
    super(message)
    this.name = new.target.name
    this.path = path
    this.code = code
  }
}

class NotFoundError extends ShellError {}
class PermissionDeniedError extends ShellError {}
class FileTooLargeError extends ShellError {}

const ERROR_BY_CODE = {
  ENOENT: NotFoundError,
  EACCES: PermissionDeniedError,
  EFBIG: FileTooLargeError,
  EOTHER: ShellError,
}

// Parse the tab-delimited envelope from native and build a typed error.
// Falls back to a generic ShellError if the message isn't in envelope form
// (e.g. a bug-level rejection from deeper in napi).
function toTypedError(err) {
  const raw = err && typeof err.message === 'string' ? err.message : String(err)
  const firstTab = raw.indexOf('\t')
  const secondTab = firstTab === -1 ? -1 : raw.indexOf('\t', firstTab + 1)
  if (firstTab === -1 || secondTab === -1) {
    return new ShellError(raw)
  }
  const code = raw.slice(0, firstTab)
  const path = raw.slice(firstTab + 1, secondTab)
  const message = raw.slice(secondTab + 1)
  const Cls = ERROR_BY_CODE[code] || ShellError
  return new Cls(message, { path, code: ERROR_BY_CODE[code] ? code : 'EOTHER' })
}

// Wrap a native file-op promise so its rejection is re-thrown as a typed error.
async function mapErrors(promise) {
  try {
    return await promise
  } catch (err) {
    throw toTypedError(err)
  }
}

// --- Shell ----------------------------------------------------------------

class Shell {
  // Private — constructed via Shell.create(). Holds the native shell.
  constructor(inner) {
    this._inner = inner
  }

  /**
   * Create a sandboxed shell from a config object. Async because the native
   * worker thread is spawned and mounts are materialized during build.
   */
  static async create(config = {}) {
    const {
      binds = [],
      credentials = [],
      allowedUrls = [],
      env = {},
      umask,
      timeout,
      limits,
      configFile,
    } = config

    const b = new native.ShellBuilder()

    // configFile merges in first; explicit options below win over it. Each
    // behavioral/limit setting is applied only when the caller actually passed
    // it (`undefined` means "unset"), so omitting an option never silently
    // clobbers a value the TOML set. The Rust core supplies the real defaults
    // when nothing is configured here or in the file.
    if (configFile !== undefined) {
      b.configFile(configFile)
    }

    for (const bind of binds) {
      const { source, destination, mode = 'direct', readonly = false } = bind
      if (mode === 'direct' && readonly) {
        b.bindDirectReadonly(source, destination)
      } else if (mode === 'direct') {
        b.bindDirect(source, destination)
      } else if (readonly) {
        b.bindReadonly(source, destination)
      } else {
        b.bind(source, destination)
      }
    }

    for (const cred of credentials) {
      const hasToken = cred.token !== undefined && cred.token !== null
      const hasEnv = cred.envVar !== undefined && cred.envVar !== null
      if (hasToken === hasEnv) {
        throw new Error('CredConfig requires exactly one of `token` or `envVar`')
      }
      if (hasToken) {
        b.credential(cred.url, cred.token)
      } else {
        b.credentialFromEnv(cred.url, cred.envVar)
      }
    }

    for (const prefix of allowedUrls) {
      b.allowUrl(prefix)
    }

    for (const [key, value] of Object.entries(env)) {
      b.env(key, value)
    }

    if (umask !== undefined) {
      b.umask(umask)
    }
    if (timeout !== undefined) {
      // Reject non-positive / non-finite up front: zero would expire every
      // command immediately (there is no "unlimited" sentinel — omit timeout
      // instead), and a negative/NaN/Infinity value would panic the native
      // Duration::from_secs_f64 across the FFI boundary.
      if (typeof timeout !== 'number' || !Number.isFinite(timeout) || timeout <= 0) {
        throw new Error(
          'timeout must be a positive, finite number of seconds (omit it for no timeout)',
        )
      }
      b.timeout(timeout)
    }

    // Limits — namespaced. Apply only when a bundle is passed; within it, each
    // field falls back to the documented default so a partial { maxOutput }
    // still pins the others to their defaults rather than to whatever the
    // config file set. (Passing no `limits` at all leaves the file/core in
    // charge.)
    if (limits !== undefined) {
      const L = {
        maxOutput: 1 << 20,
        maxFileSize: 10 << 20,
        maxFds: 128,
        maxBgJobs: 8,
        maxPipeline: 16,
        maxInput: 1 << 20,
        maxInodes: 10000,
        maxDepth: 64,
        ...limits,
      }
      b.maxOutput(L.maxOutput)
      b.maxFileSize(L.maxFileSize)
      b.maxFds(L.maxFds)
      b.maxBgJobs(L.maxBgJobs)
      b.maxPipeline(L.maxPipeline)
      b.maxInput(L.maxInput)
      b.maxInodes(L.maxInodes)
      b.maxDepth(L.maxDepth)
    }

    const inner = await b.build()
    return new Shell(inner)
  }

  // ---- Command execution ----

  run(command) {
    return this._inner.run(command)
  }

  // ---- Environment ----

  setEnv(key, value) {
    return this._inner.setEnv(key, value)
  }

  getEnv(key) {
    return this._inner.getEnv(key)
  }

  // ---- VFS file operations ----

  readFile(path) {
    return mapErrors(this._inner.readFile(path))
  }

  writeFile(path, content) {
    return mapErrors(this._inner.writeFile(path, content))
  }

  removeFile(path) {
    return mapErrors(this._inner.removeFile(path))
  }

  listFiles(path) {
    return mapErrors(this._inner.listFiles(path))
  }
}

module.exports = {
  Shell,
  ShellError,
  NotFoundError,
  PermissionDeniedError,
  FileTooLargeError,
}
