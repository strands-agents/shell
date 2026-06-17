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
  const limits: ShellLimits = { maxOutput: 1 << 20, maxFileSize: 10 << 20 }
  const config: ShellConfig = {
    binds: [bind],
    credentials: [cred],
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

  await shell.setEnv('K', 'v')
  const _env: string | null = await shell.getEnv('K')

  const data: Uint8Array = await shell.readFile('/work/note.txt')
  await shell.writeFile('/work/note.txt', data)
  await shell.removeFile('/work/note.txt')

  const entries: FileInfo[] = await shell.listFiles('/work')
  const _name: string = entries[0].name

  // Read-only config snapshot: every field is typed and readonly.
  const snapshot: ShellConfigSnapshot = await shell.config()
  const _binds: readonly BindInfo[] = snapshot.binds
  const _mode: 'copy' | 'direct' = snapshot.binds[0].mode
  const _creds: readonly CredInfo[] = snapshot.credentials
  const _credKind: 'bearer' | 'query' = snapshot.credentials[0].kind
  const _envVar: string | null = snapshot.credentials[0].envVar
  const _fromLiteral: boolean = snapshot.credentials[0].fromLiteral
  const _allowed: readonly string[] = snapshot.allowedUrls
  const _umask: number = snapshot.umask
  const _timeout: number | null = snapshot.timeout
  const _limits: LimitsInfo = snapshot.limits
  const _maxOutput: number = snapshot.limits.maxOutput

  // Typed error hierarchy: subclasses are ShellErrors carrying path + code.
  try {
    await shell.readFile('/work/missing')
  } catch (err) {
    if (err instanceof ShellError) {
      const _path: string = err.path
      const code: ShellErrorCode = err.code
      const _isEnoent: boolean = code === 'ENOENT'
    }
    const _subclasses = [NotFoundError, PermissionDeniedError, FileTooLargeError]
  }
}

// Reference `usage` so it isn't flagged as unused under strict settings.
void usage
