// Tests for the internal native ShellBuilder API (`native.js`).
//
// The customer-facing surface is the config-driven `Shell.create()` (see
// test_bindings.mjs). The builder is now an internal detail the wrapper
// translates config into; these tests pin its contract so the regression
// where every setter returned `undefined` can't recur, and so the wrapper
// has a stable base. Mirrors tests/python/test_builder.py.

import { test } from 'node:test'
import assert from 'node:assert/strict'
import fs from 'node:fs/promises'
import os from 'node:os'
import path from 'node:path'

import * as native from '../../native.js'

async function makeHostDir() {
  return fs.mkdtemp(path.join(os.tmpdir(), 'strands-shell-builder-test-'))
}

test('builder setter returns the builder so calls can chain', () => {
  const b = native.Shell.builder()
  const same = b.timeout(5.0)
  assert.ok(same, 'timeout() returned undefined — chaining is broken')
})

test('the exact pattern from docs/js-bindings.md works', async () => {
  const shell = await native.Shell.builder().timeout(20.0).build()
  const out = await shell.run('pwd')
  assert.equal(out.status, 0)
  assert.match(out.stdout, /\/home\/lash/)
})

test('full chain spanning bind, limits, env, umask', async () => {
  const hostDir = await makeHostDir()
  try {
    const shell = await native.Shell.builder()
      .bindDirect(hostDir, '/work')
      .timeout(10.0)
      .maxOutput(1 << 20)
      .maxFileSize(1 << 20)
      .env('FOO', 'bar')
      .umask(0o022)
      .build()
    assert.equal(await shell.getEnv('FOO'), 'bar')
    const out = await shell.run('ls /work')
    assert.equal(out.status, 0)
  } finally {
    await fs.rm(hostDir, { recursive: true, force: true })
  }
})

test('repeated setters keep the latest value', async () => {
  const shell = await native.Shell.builder()
    .env('KEY', 'first')
    .env('KEY', 'second')
    .build()
  assert.equal(await shell.getEnv('KEY'), 'second')
})

test('builder cannot be reused after build', async () => {
  const b = native.Shell.builder()
  await b.build()
  await assert.rejects(() => b.build(), /builder consumed/)
  assert.throws(() => b.timeout(5.0), /builder consumed/)
})

test('statement-by-statement style still works', async () => {
  const hostDir = await makeHostDir()
  try {
    const builder = native.Shell.builder()
    builder.bindDirect(hostDir, '/work')
    builder.timeout(10.0)
    const shell = await builder.build()
    const out = await shell.run('ls /work')
    assert.equal(out.status, 0)
  } finally {
    await fs.rm(hostDir, { recursive: true, force: true })
  }
})

test('bindDirect reflects host changes after build (passthrough)', async () => {
  const hostDir = await makeHostDir()
  try {
    const shell = await native.Shell.builder().bindDirect(hostDir, '/work').build()
    await fs.writeFile(path.join(hostDir, 'from_host.txt'), 'hello')
    const data = await shell.readFile('/work/from_host.txt')
    assert.equal(new TextDecoder().decode(data), 'hello')
  } finally {
    await fs.rm(hostDir, { recursive: true, force: true })
  }
})

test('bind copy-mode snapshots at build time, not later', async () => {
  const hostDir = await makeHostDir()
  try {
    await fs.writeFile(path.join(hostDir, 'seed.txt'), 'snapshot')
    const shell = await native.Shell.builder().bind(hostDir, '/work').build()
    const data = await shell.readFile('/work/seed.txt')
    assert.equal(new TextDecoder().decode(data), 'snapshot')
    // Host changes after build are NOT reflected in copy-mode mount
    await fs.writeFile(path.join(hostDir, 'added_after.txt'), 'not visible')
    await assert.rejects(
      () => shell.readFile('/work/added_after.txt'),
      /no such|not found|does not exist/i,
    )
  } finally {
    await fs.rm(hostDir, { recursive: true, force: true })
  }
})
