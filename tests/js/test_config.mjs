// Tests for the read-only config snapshot exposed via `shell.config()`.
//
// Mirrors tests/python/test_config.py. Run with `npm test`.
//
// The snapshot lets an embedder introspect a constructed shell without having
// held onto the config object. Its key guarantee: it never leaks secret
// values — only the source of a credential (literal vs env-var name).

import { test } from 'node:test'
import assert from 'node:assert/strict'
import fs from 'node:fs/promises'
import os from 'node:os'
import path from 'node:path'

import { Shell } from '../../index.js'

async function makeHostDir() {
  return fs.mkdtemp(path.join(os.tmpdir(), 'strands-shell-config-test-'))
}

test('default shell reports default config', async () => {
  const cfg = await (await Shell.create()).config()
  assert.deepEqual(cfg.binds, [])
  assert.deepEqual(cfg.credentials, [])
  assert.deepEqual(cfg.allowedUrls, [])
  assert.deepEqual(cfg.env, {})
  assert.equal(cfg.umask, 0o022)
  // The builder defaults to a 30s per-command timeout; the snapshot reports
  // the real effective value.
  assert.equal(cfg.timeout, 30)
  assert.equal(cfg.limits.maxDepth, 64)
  assert.equal(cfg.limits.maxOutput, 1 << 20)
  assert.equal(cfg.limits.maxFds, 128)
  assert.equal(cfg.limits.maxBgJobs, 8)
  assert.equal(cfg.limits.maxPipeline, 16)
  assert.equal(cfg.limits.maxInput, 1 << 20)
  assert.equal(cfg.limits.maxFileSize, 10 << 20)
  assert.equal(cfg.limits.maxInodes, 10000)
})

test('config reports binds', async () => {
  const hostDir = await makeHostDir()
  try {
    const shell = await Shell.create({
      binds: [
        { source: hostDir, destination: '/work', mode: 'direct', readonly: true },
        { source: hostDir, destination: '/copy', mode: 'copy' },
      ],
    })
    const cfg = await shell.config()
    assert.equal(cfg.binds.length, 2)
    assert.equal(cfg.binds[0].source, hostDir)
    assert.equal(cfg.binds[0].destination, '/work')
    assert.equal(cfg.binds[0].mode, 'direct')
    assert.equal(cfg.binds[0].readonly, true)
    assert.equal(cfg.binds[1].mode, 'copy')
    assert.equal(cfg.binds[1].readonly, false)
  } finally {
    await fs.rm(hostDir, { recursive: true, force: true })
  }
})

test('config reports allowedUrls, env, umask, timeout', async () => {
  const shell = await Shell.create({
    allowedUrls: ['https://api.example.com/', 'https://api.openai.com/'],
    env: { PROJECT: 'demo', STAGE: 'prod' },
    umask: 0o027,
    timeout: 12.5,
  })
  const cfg = await shell.config()
  assert.deepEqual(cfg.allowedUrls, ['https://api.example.com/', 'https://api.openai.com/'])
  assert.deepEqual(cfg.env, { PROJECT: 'demo', STAGE: 'prod' })
  assert.equal(cfg.umask, 0o027)
  assert.equal(cfg.timeout, 12.5)
})

test('config reports overridden limits', async () => {
  const shell = await Shell.create({ limits: { maxOutput: 2048, maxInodes: 500 } })
  const cfg = await shell.config()
  assert.equal(cfg.limits.maxOutput, 2048)
  assert.equal(cfg.limits.maxInodes, 500)
})

test('config credentials never leak literal token', async () => {
  const shell = await Shell.create({
    credentials: [{ url: 'https://api.example.com/*', token: 'sk-super-secret' }],
  })
  const cfg = await shell.config()
  const cred = cfg.credentials[0]
  assert.equal(cred.url, 'https://api.example.com/*')
  assert.equal(cred.kind, 'bearer')
  assert.equal(cred.fromLiteral, true)
  assert.equal(cred.envVar, null)
  assert.ok(!JSON.stringify(cfg).includes('sk-super-secret'))
})

test('config credentials report env var name not value', async () => {
  process.env.STRANDS_SHELL_TEST_TOKEN = 'value-must-not-leak'
  try {
    const shell = await Shell.create({
      credentials: [{ url: 'https://api.openai.com/*', envVar: 'STRANDS_SHELL_TEST_TOKEN' }],
    })
    const cfg = await shell.config()
    const cred = cfg.credentials[0]
    assert.equal(cred.envVar, 'STRANDS_SHELL_TEST_TOKEN')
    assert.equal(cred.fromLiteral, false)
    assert.ok(!JSON.stringify(cfg).includes('value-must-not-leak'))
  } finally {
    delete process.env.STRANDS_SHELL_TEST_TOKEN
  }
})

test('config snapshot is deep-frozen', async () => {
  const shell = await Shell.create({
    binds: [{ source: os.tmpdir(), destination: '/work', mode: 'direct' }],
  })
  const cfg = await shell.config()
  assert.ok(Object.isFrozen(cfg))
  assert.ok(Object.isFrozen(cfg.binds))
  assert.ok(Object.isFrozen(cfg.binds[0]))
  assert.ok(Object.isFrozen(cfg.limits))
  assert.throws(() => {
    'use strict'
    cfg.umask = 0
  })
})

test('config is a snapshot, not a live view', async () => {
  // A fresh snapshot is taken on each call; mutating the shell after the first
  // call must not retroactively change the already-returned object.
  const shell = await Shell.create({ env: { A: '1' } })
  const cfg = await shell.config()
  await shell.setEnv('A', '2')
  assert.equal(cfg.env.A, '1')
})
