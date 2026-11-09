// Type-level test for the public TypeScript surface (`index.d.ts`).
//
// This file is never executed — `tsc --noEmit` checks that the declared types
// are internally consistent and usable the way a consumer would use them. If
// `index.d.ts` drifts (a renamed method, a wrong signature, a missing export),
// this stops compiling and `cargo xtask check` / CI fails.

import {
  Shell,
  ShellError,
  NotFoundError,
  PermissionDeniedError,
  FileTooLargeError,
  type Output,
  type FileInfo,
  type ShellConfig,
  type BindConfig,
  type CredConfig,
  type ShellLimits,
  type ShellErrorCode,
  type ShellConfigSnapshot,
  type BindInfo,
  type CredInfo,
  type LimitsInfo,
} from '../../index.js'

async function usage(): Promise<void> {
  // Config object exercises every field and the literal-typed `mode`.
  const bind: BindConfig = { source: '/host', destination: '/work', mode: 'copy', readonly: true }
  const cred: CredConfig = { url: 'https://api.example.com/', envVar: 'API_TOKEN' }
  const literalCred: CredConfig = { url: 'https://api.example.com/', token: 'secret' }
  const directBind: BindConfig = { source: '/host', destination: '/direct', mode: 'direct' }
  const limits: ShellLimits = {
    maxOutput: 1 << 20,
    maxFileSize: 10 << 20,
    maxFds: 128,
    maxBgJobs: 8,
    maxPipeline: 16,
    maxInput: 1 << 20,
    maxInodes: 10_000,
    maxDepth: 64,
  }
  const config: ShellConfig = {
    binds: [bind, directBind],
    credentials: [cred, literalCred],
    allowedUrls: ['https://api.example.com/'],
    env: { PROJECT: 'demo' },
    umask: 0o022,
    timeout: 30,
    limits,
    configFile: '/path/to/sandbox.toml',
  }

  const shell: Shell = await Shell.create(config)
  await Shell.create() // config is optional

  const out: Output = await shell.run('echo hi | tr a-z A-Z')
  const _status: number = out.status
  const _stdout: string = out.stdout
  const _stderr: string = out.stderr

  await shell.setEnv('K', 'v')
  const _env: string | null = await shell.getEnv('K')

  const data: Uint8Array = await shell.readFile('/work/note.txt')
  await shell.writeFile('/work/note.txt', data)
  await shell.removeFile('/work/note.txt')

  const entries: FileInfo[] = await shell.listFiles('/work')
  const _name: string = entries[0].name
  const _isDir: boolean | undefined = entries[0].isDir
  const _size: number | undefined = entries[0].size
  // @ts-expect-error FileInfo fields are readonly
  entries[0].name = 'x'

  // Read-only config snapshot: every field is typed and readonly.
  const snapshot: ShellConfigSnapshot = await shell.config()
  const _binds: readonly BindInfo[] = snapshot.binds
  const _source: string = snapshot.binds[0].source
  const _destination: string = snapshot.binds[0].destination
  const _mode: 'copy' | 'direct' = snapshot.binds[0].mode
  const _readonly: boolean = snapshot.binds[0].readonly
  const _creds: readonly CredInfo[] = snapshot.credentials
  const _credUrl: string = snapshot.credentials[0].url
  const _credKind: 'bearer' | 'query' = snapshot.credentials[0].kind
  const _methods: readonly string[] = snapshot.credentials[0].methods
  const _param: string | null = snapshot.credentials[0].param
  const _envVar: string | null = snapshot.credentials[0].envVar
  const _fromLiteral: boolean = snapshot.credentials[0].fromLiteral
  const _allowed: readonly string[] = snapshot.allowedUrls
  const _snapshotEnv: Readonly<Record<string, string>> = snapshot.env
  const _umask: number = snapshot.umask
  const _timeout: number | null = snapshot.timeout
  const _limits: LimitsInfo = snapshot.limits
  const _allLimits: number[] = [
    snapshot.limits.maxDepth,
    snapshot.limits.maxOutput,
    snapshot.limits.maxFds,
    snapshot.limits.maxBgJobs,
    snapshot.limits.maxPipeline,
    snapshot.limits.maxInput,
    snapshot.limits.maxFileSize,
    snapshot.limits.maxInodes,
  ]

  // Positive reads alone do not protect `readonly`; each write below must fail.
  // @ts-expect-error ShellConfigSnapshot fields are readonly
  snapshot.umask = 0
  // @ts-expect-error ShellConfigSnapshot fields are readonly
  snapshot.timeout = null
  // @ts-expect-error ShellConfigSnapshot fields are readonly
  snapshot.binds = []
  // @ts-expect-error snapshot arrays are readonly
  snapshot.binds.push(snapshot.binds[0])
  // @ts-expect-error snapshot arrays are readonly
  snapshot.allowedUrls.push('https://evil.example/')
  // @ts-expect-error snapshot env is readonly
  snapshot.env.PROJECT = 'other'
  // @ts-expect-error BindInfo fields are readonly
  snapshot.binds[0].readonly = false
  // @ts-expect-error CredInfo fields are readonly
  snapshot.credentials[0].fromLiteral = false
  // @ts-expect-error CredInfo methods are readonly
  snapshot.credentials[0].methods.push('DELETE')
  // @ts-expect-error LimitsInfo fields are readonly
  snapshot.limits.maxOutput = 0

  // Typed error hierarchy: subclasses are ShellErrors carrying path + code.
  try {
    await shell.readFile('/work/missing')
  } catch (err) {
    if (err instanceof ShellError) {
      const _path: string = err.path
      const code: ShellErrorCode = err.code
      const _isEnoent: boolean = code === 'ENOENT'
      // @ts-expect-error ShellError fields are readonly
      err.path = ''
      // @ts-expect-error ShellError fields are readonly
      err.code = 'EOTHER'
    }
    const _subclasses: (typeof ShellError)[] = [NotFoundError, PermissionDeniedError, FileTooLargeError]
  }

  // @ts-expect-error the constructor is private; use Shell.create()
  new Shell()
}

// Exhaustive records: a removed or added member of each union fails to compile.
const _errorCodes: Record<ShellErrorCode, true> = { ENOENT: true, EACCES: true, EFBIG: true, EOTHER: true }
const _bindModes: Record<NonNullable<BindConfig['mode']>, true> = { direct: true, copy: true }
const _snapshotModes: Record<BindInfo['mode'], true> = { direct: true, copy: true }
const _credKinds: Record<CredInfo['kind'], true> = { bearer: true, query: true }

// Reference `usage` so it isn't flagged as unused under strict settings.
void usage
