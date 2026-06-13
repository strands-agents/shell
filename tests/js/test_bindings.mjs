// Tests for the v0.1 Node bindings.
//
// Mirrors tests/python/test_bindings.py. Run with `npm test`.

import { test } from 'node:test'
import assert from 'node:assert/strict'
import fs from 'node:fs/promises'
import os from 'node:os'
import path from 'node:path'

import { Shell, ShellError, NotFoundError, FileTooLargeError } from '../../index.js'

async function makeHostDir() {
  return fs.mkdtemp(path.join(os.tmpdir(), 'strands-shell-bindings-test-'))
}

async function makeShell(hostDir) {
  return Shell.create({
    binds: [{ source: hostDir, destination: '/work', mode: 'direct' }],
    timeout: 10.0,
  })
}

const enc = (s) => new TextEncoder().encode(s)
const dec = (b) => new TextDecoder().decode(b)

// ---------------------------------------------------------------------------
// readFile
// ---------------------------------------------------------------------------

test('readFile returns Uint8Array', async () => {
  const hostDir = await makeHostDir()
  try {
    await fs.writeFile(path.join(hostDir, 'seed.txt'), 'hello from host')
    const shell = await makeShell(hostDir)
    const data = await shell.readFile('/work/seed.txt')
    assert.ok(data instanceof Uint8Array)
    assert.equal(dec(data), 'hello from host')
  } finally {
    await fs.rm(hostDir, { recursive: true, force: true })
  }
})

test('readFile missing rejects with NotFoundError', async () => {
  const hostDir = await makeHostDir()
  try {
    const shell = await makeShell(hostDir)
    await assert.rejects(
      () => shell.readFile('/work/does-not-exist'),
      (err) => {
        assert.ok(err instanceof NotFoundError, 'not a NotFoundError')
        assert.ok(err instanceof ShellError, 'not a ShellError')
        assert.equal(err.code, 'ENOENT')
        assert.equal(err.path, '/work/does-not-exist')
        return true
      },
    )
  } finally {
    await fs.rm(hostDir, { recursive: true, force: true })
  }
})

test('readFile preserves full byte range (binary roundtrip)', async () => {
  const hostDir = await makeHostDir()
  try {
    const shell = await makeShell(hostDir)
    const payload = new Uint8Array(256)
    for (let i = 0; i < 256; i++) payload[i] = i
    await shell.writeFile('/work/binary.bin', payload)
    const got = await shell.readFile('/work/binary.bin')
    assert.deepEqual(Array.from(got), Array.from(payload))
  } finally {
    await fs.rm(hostDir, { recursive: true, force: true })
  }
})

test('readFile respects maxFileSize (rejects with FileTooLargeError)', async () => {
  // Seed on the host so the file already exceeds the cap at read time; the
  // read must be bounded by maxFileSize instead of loading it all into memory
  // — important for direct-passthrough mounts to large host files.
  const hostDir = await makeHostDir()
  try {
    await fs.writeFile(path.join(hostDir, 'big.txt'), 'x'.repeat(4096))
    const shell = await Shell.create({
      binds: [{ source: hostDir, destination: '/work', mode: 'direct' }],
      limits: { maxFileSize: 1024 },
    })
    await assert.rejects(
      () => shell.readFile('/work/big.txt'),
      (err) => {
        assert.ok(err instanceof FileTooLargeError, `expected FileTooLargeError, got ${err.name}`)
        assert.equal(err.code, 'EFBIG')
        assert.equal(err.path, '/work/big.txt')
        return true
      },
    )
  } finally {
    await fs.rm(hostDir, { recursive: true, force: true })
  }
})

test('readFile at maxFileSize boundary succeeds', async () => {
  const hostDir = await makeHostDir()
  try {
    await fs.writeFile(path.join(hostDir, 'exact.txt'), 'y'.repeat(1024))
    const shell = await Shell.create({
      binds: [{ source: hostDir, destination: '/work', mode: 'direct' }],
      limits: { maxFileSize: 1024 },
    })
    const got = await shell.readFile('/work/exact.txt')
    assert.equal(got.length, 1024)
  } finally {
    await fs.rm(hostDir, { recursive: true, force: true })
  }
})

// ---------------------------------------------------------------------------
// writeFile
// ---------------------------------------------------------------------------

test('writeFile creates parent directories', async () => {
  const hostDir = await makeHostDir()
  try {
    const shell = await makeShell(hostDir)
    await shell.writeFile('/work/deep/dir/binary.bin', enc('data'))
    const got = await shell.readFile('/work/deep/dir/binary.bin')
    assert.equal(dec(got), 'data')
  } finally {
    await fs.rm(hostDir, { recursive: true, force: true })
  }
})

test('writeFile truncates on overwrite', async () => {
  const hostDir = await makeHostDir()
  try {
    await fs.writeFile(path.join(hostDir, 'seed.txt'), 'hello from host')
    const shell = await makeShell(hostDir)
    await shell.writeFile('/work/seed.txt', enc('short'))
    const got = await shell.readFile('/work/seed.txt')
    assert.equal(dec(got), 'short')
  } finally {
    await fs.rm(hostDir, { recursive: true, force: true })
  }
})

test('writeFile accepts empty payload', async () => {
  const hostDir = await makeHostDir()
  try {
    const shell = await makeShell(hostDir)
    await shell.writeFile('/work/empty.txt', new Uint8Array(0))
    const got = await shell.readFile('/work/empty.txt')
    assert.equal(got.length, 0)
  } finally {
    await fs.rm(hostDir, { recursive: true, force: true })
  }
})

test('writeFile at root of bind mount', async () => {
  const hostDir = await makeHostDir()
  try {
    const shell = await makeShell(hostDir)
    const data = new Uint8Array([0, 1, 2])
    await shell.writeFile('/work/root_level.bin', data)
    const got = await shell.readFile('/work/root_level.bin')
    assert.deepEqual(Array.from(got), [0, 1, 2])
  } finally {
    await fs.rm(hostDir, { recursive: true, force: true })
  }
})

test('writeFile handles 2 MiB payload (exercises stall-detection bound)', async () => {
  const hostDir = await makeHostDir()
  try {
    const shell = await makeShell(hostDir)
    const big = new Uint8Array(2 * 1024 * 1024)
    for (let i = 0; i < big.length; i++) big[i] = (i * 31) & 0xff
    await shell.writeFile('/work/big.bin', big)
    const got = await shell.readFile('/work/big.bin')
    assert.equal(got.length, big.length)
    // Spot-check a few bytes; full deepEqual is slow on 2 MiB.
    assert.equal(got[0], 0)
    assert.equal(got[1], 31)
    assert.equal(got[big.length - 1], big[big.length - 1])
  } finally {
    await fs.rm(hostDir, { recursive: true, force: true })
  }
})

test('writeFile pure-VFS path (no bind mount)', async () => {
  const shell = await Shell.create({ timeout: 10.0 })
  await shell.writeFile('/tmp/vfs_only.txt', enc('in-memory'))
  const got = await shell.readFile('/tmp/vfs_only.txt')
  assert.equal(dec(got), 'in-memory')
})

test('writeFile size-limit rejects with FileTooLargeError, not a timeout', async () => {
  const hostDir = await makeHostDir()
  try {
    const shell = await Shell.create({
      binds: [{ source: hostDir, destination: '/work', mode: 'copy' }],
      limits: { maxFileSize: 64 },
    })
    await assert.rejects(
      () => shell.writeFile('/work/too_big.bin', new Uint8Array(1024).fill(0x78)),
      (err) => {
        assert.ok(err instanceof FileTooLargeError, `expected FileTooLargeError, got ${err.name}`)
        assert.equal(err.code, 'EFBIG')
        return true
      },
    )
  } finally {
    await fs.rm(hostDir, { recursive: true, force: true })
  }
})

// ---------------------------------------------------------------------------
// removeFile
// ---------------------------------------------------------------------------

test('removeFile removes entry', async () => {
  const hostDir = await makeHostDir()
  try {
    await fs.writeFile(path.join(hostDir, 'seed.txt'), 'hello')
    const shell = await makeShell(hostDir)
    await shell.removeFile('/work/seed.txt')
    const names = (await shell.listFiles('/work')).map(e => e.name)
    assert.ok(!names.includes('seed.txt'))
  } finally {
    await fs.rm(hostDir, { recursive: true, force: true })
  }
})

test('removeFile missing rejects with NotFoundError', async () => {
  const hostDir = await makeHostDir()
  try {
    const shell = await makeShell(hostDir)
    await assert.rejects(
      () => shell.removeFile('/work/does-not-exist'),
      (err) => err instanceof NotFoundError && err.code === 'ENOENT',
    )
  } finally {
    await fs.rm(hostDir, { recursive: true, force: true })
  }
})

// ---------------------------------------------------------------------------
// listFiles
// ---------------------------------------------------------------------------

test('listFiles returns structured FileInfo with size and isDir', async () => {
  const hostDir = await makeHostDir()
  try {
    await fs.writeFile(path.join(hostDir, 'seed.txt'), 'short')  // 5 bytes
    await fs.mkdir(path.join(hostDir, 'sub'))
    const shell = await makeShell(hostDir)
    const entries = await shell.listFiles('/work')
    const byName = new Map(entries.map(e => [e.name, e]))
    assert.ok(byName.has('seed.txt'))
    assert.ok(byName.has('sub'))
    assert.equal(byName.get('seed.txt').isDir, false)
    assert.equal(byName.get('sub').isDir, true)
    assert.equal(byName.get('seed.txt').size, 5)
    // Directories don't carry a size; napi-rs surfaces Option::None as
    // the property being absent (undefined), not null.
    assert.equal(byName.get('sub').size, undefined)
  } finally {
    await fs.rm(hostDir, { recursive: true, force: true })
  }
})

test('listFiles on nested directory', async () => {
  const hostDir = await makeHostDir()
  try {
    await fs.mkdir(path.join(hostDir, 'sub'))
    await fs.writeFile(path.join(hostDir, 'sub', 'nested.txt'), 'nested file')
    const shell = await makeShell(hostDir)
    const nested = await shell.listFiles('/work/sub')
    assert.deepEqual(nested.map(e => e.name).sort(), ['nested.txt'])
    assert.equal(nested[0].isDir, false)
  } finally {
    await fs.rm(hostDir, { recursive: true, force: true })
  }
})

test('listFiles missing rejects with NotFoundError', async () => {
  const hostDir = await makeHostDir()
  try {
    const shell = await makeShell(hostDir)
    await assert.rejects(
      () => shell.listFiles('/work/no/such/dir'),
      (err) => err instanceof NotFoundError && err.code === 'ENOENT',
    )
  } finally {
    await fs.rm(hostDir, { recursive: true, force: true })
  }
})

// ---------------------------------------------------------------------------
// run() interleaves with native VFS calls; state persists
// ---------------------------------------------------------------------------

test('run interleaves with native VFS calls', async () => {
  const hostDir = await makeHostDir()
  try {
    const shell = await makeShell(hostDir)
    await shell.writeFile('/work/from_node.txt', enc('hi'))
    const out = await shell.run('ls /work')
    assert.equal(out.status, 0)
    assert.match(out.stdout, /from_node\.txt/)
  } finally {
    await fs.rm(hostDir, { recursive: true, force: true })
  }
})

test('cwd persists across run() calls', async () => {
  const hostDir = await makeHostDir()
  try {
    const shell = await makeShell(hostDir)
    await shell.run('cd /work')
    const out = await shell.run('pwd')
    assert.equal(out.status, 0)
    assert.match(out.stdout, /\/work/)
  } finally {
    await fs.rm(hostDir, { recursive: true, force: true })
  }
})

test('Buffer is accepted as Uint8Array on writeFile', async () => {
  // Node Buffer extends Uint8Array, so passing a Buffer should work
  // without conversion. This is a key DX guarantee from the doc.
  const hostDir = await makeHostDir()
  try {
    const shell = await makeShell(hostDir)
    await shell.writeFile('/work/buf.txt', Buffer.from('from a Buffer'))
    const got = await shell.readFile('/work/buf.txt')
    assert.equal(Buffer.from(got).toString('utf8'), 'from a Buffer')
  } finally {
    await fs.rm(hostDir, { recursive: true, force: true })
  }
})

test('configFile values are not clobbered by omitted options', async () => {
  // Regression: omitting umask/timeout/limits must NOT overwrite what the
  // TOML set. A value present only in the file must survive.
  const hostDir = await makeHostDir()
  try {
    const cfg = path.join(hostDir, 'shell.toml')
    await fs.writeFile(cfg, 'umask = "077"\n')
    const shell = await Shell.create({ configFile: cfg })
    const out = await shell.run('umask')
    assert.equal(out.stdout.trim(), '0077')
  } finally {
    await fs.rm(hostDir, { recursive: true, force: true })
  }
})

test('explicit option overrides configFile', async () => {
  const hostDir = await makeHostDir()
  try {
    const cfg = path.join(hostDir, 'shell.toml')
    await fs.writeFile(cfg, 'umask = "077"\n')
    const shell = await Shell.create({ configFile: cfg, umask: 0o022 })
    const out = await shell.run('umask')
    assert.equal(out.stdout.trim(), '0022')
  } finally {
    await fs.rm(hostDir, { recursive: true, force: true })
  }
})

// ---------------------------------------------------------------------------
// timeout validation (Shell.create rejects non-positive / non-finite)
// ---------------------------------------------------------------------------

test('Shell.create rejects timeout: 0', async () => {
  await assert.rejects(
    () => Shell.create({ timeout: 0 }),
    /timeout must be a positive, finite number/,
  )
})

test('Shell.create rejects negative / non-finite timeout', async () => {
  for (const bad of [-1, NaN, Infinity, -Infinity]) {
    await assert.rejects(
      () => Shell.create({ timeout: bad }),
      /timeout must be a positive, finite number/,
      `timeout: ${bad} should be rejected`,
    )
  }
})

test('Shell.create allows omitted timeout (no limit) and positive values', async () => {
  const noTimeout = await Shell.create({})
  assert.equal((await noTimeout.run('echo ok')).stdout.trim(), 'ok')
  const withTimeout = await Shell.create({ timeout: 5.0 })
  assert.equal((await withTimeout.run('echo ok')).stdout.trim(), 'ok')
})
