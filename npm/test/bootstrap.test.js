'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { setTimeout: delay } = require('node:timers/promises');

const {
  acquireInstallLock,
  binaryFilename,
  cacheStateRoot,
  copyFileAtomic,
  findBinary,
  hasLocalCargoWorkspace,
  latestTagCachePath,
  packageRoot,
  parseChecksumHex,
  readLatestTagCache,
  releaseTargetFor,
  releaseUrls,
  resolveLatestReleaseTag,
  validateDownloadUrl,
  verifyArchiveChecksum,
  writeLatestTagCache,
} = require('../lib/bootstrap');

test('release target mapping matches published targets', () => {
  assert.equal(releaseTargetFor('linux', 'x64'), 'x86_64-unknown-linux-gnu');
  assert.equal(releaseTargetFor('linux', 'arm64'), 'aarch64-unknown-linux-gnu');
  assert.equal(releaseTargetFor('darwin', 'x64'), 'x86_64-apple-darwin');
  assert.equal(releaseTargetFor('darwin', 'arm64'), 'aarch64-apple-darwin');
  assert.equal(releaseTargetFor('win32', 'x64'), null);
});

test('binary filename adds .exe only on windows', () => {
  assert.equal(binaryFilename('amplihack', 'linux'), 'amplihack');
  assert.equal(binaryFilename('amplihack', 'win32'), 'amplihack.exe');
});

test('release URLs target the GitHub release archive and checksum', () => {
  const urls = releaseUrls('0.6.4', 'x86_64-unknown-linux-gnu');
  assert.equal(
    urls.archiveUrl,
    'https://github.com/rysweet/amplihack-rs/releases/download/v0.6.4/amplihack-x86_64-unknown-linux-gnu.tar.gz',
  );
  assert.equal(
    urls.checksumUrl,
    'https://github.com/rysweet/amplihack-rs/releases/download/v0.6.4/amplihack-x86_64-unknown-linux-gnu.tar.gz.sha256',
  );
});

test('checksum parser extracts the leading digest token', () => {
  const digest = 'a'.repeat(64);
  assert.equal(parseChecksumHex(`${digest}  amplihack.tar.gz\n`), digest);
  assert.throws(() => parseChecksumHex('not-a-digest\n'));
});

test('checksum verification requires the published digest to match', () => {
  const archiveBytes = Buffer.from('amplihack');
  const digest = require('node:crypto').createHash('sha256').update(archiveBytes).digest('hex');
  assert.doesNotThrow(() => verifyArchiveChecksum(archiveBytes, `${digest}  amplihack.tar.gz\n`, 'https://example.test/archive'));
  assert.throws(() => verifyArchiveChecksum(archiveBytes, `${'a'.repeat(64)}  amplihack.tar.gz\n`, 'https://example.test/archive'));
});

test('download URL validation only trusts GitHub release hosts', () => {
  assert.doesNotThrow(() => validateDownloadUrl('https://github.com/rysweet/amplihack-rs/releases/download/v0.6.4/amplihack-x86_64-unknown-linux-gnu.tar.gz'));
  assert.doesNotThrow(() => validateDownloadUrl('https://objects.githubusercontent.com/github-production-release-asset-2e65be/123'));
  assert.doesNotThrow(() => validateDownloadUrl('https://release-assets.githubusercontent.com/github-production-release-asset/123'));
  assert.throws(() => validateDownloadUrl('https://example.com/amplihack.tar.gz'));
});

test('findBinary locates nested binaries', async () => {
  const tempDir = await fs.promises.mkdtemp(path.join(os.tmpdir(), 'amplihack-npm-test-'));
  const nestedDir = path.join(tempDir, 'a', 'b');
  await fs.promises.mkdir(nestedDir, { recursive: true });
  const binaryPath = path.join(nestedDir, 'amplihack');
  await fs.promises.writeFile(binaryPath, '');
  assert.equal(findBinary(tempDir, 'amplihack'), binaryPath);
});

test('copyFileAtomic publishes the completed file contents', async () => {
  const tempDir = await fs.promises.mkdtemp(path.join(os.tmpdir(), 'amplihack-npm-copy-'));
  const source = path.join(tempDir, 'source');
  const destination = path.join(tempDir, 'destination');
  await fs.promises.writeFile(source, 'hello');
  await copyFileAtomic(source, destination, 0o700);
  assert.equal(await fs.promises.readFile(destination, 'utf8'), 'hello');
});

test('install lock serializes concurrent installers', async () => {
  const tempDir = await fs.promises.mkdtemp(path.join(os.tmpdir(), 'amplihack-npm-lock-'));
  const releaseFirst = await acquireInstallLock(tempDir, { timeoutMs: 1000, pollMs: 10 });
  let acquiredSecond = false;
  const secondLock = acquireInstallLock(tempDir, { timeoutMs: 1000, pollMs: 10 }).then((release) => {
    acquiredSecond = true;
    return release;
  });
  await delay(50);
  assert.equal(acquiredSecond, false);
  await releaseFirst();
  const releaseSecond = await secondLock;
  assert.equal(acquiredSecond, true);
  await releaseSecond();
});

test('workspace detection requires both Rust binary crates', async () => {
  const tempDir = await fs.promises.mkdtemp(path.join(os.tmpdir(), 'amplihack-npm-workspace-'));
  await fs.promises.mkdir(path.join(tempDir, 'bins', 'amplihack'), { recursive: true });
  await fs.promises.mkdir(path.join(tempDir, 'bins', 'amplihack-hooks'), { recursive: true });
  await fs.promises.writeFile(path.join(tempDir, 'Cargo.toml'), '');
  await fs.promises.writeFile(path.join(tempDir, 'bins', 'amplihack', 'Cargo.toml'), '');
  assert.equal(hasLocalCargoWorkspace(tempDir), false);
  await fs.promises.writeFile(path.join(tempDir, 'bins', 'amplihack-hooks', 'Cargo.toml'), '');
  assert.equal(hasLocalCargoWorkspace(tempDir), true);
});

test('package version stays aligned with Cargo workspace version', () => {
  const repoRoot = packageRoot(__dirname);
  const packageJson = JSON.parse(fs.readFileSync(path.join(repoRoot, 'package.json'), 'utf8'));
  const cargoToml = fs.readFileSync(path.join(repoRoot, 'Cargo.toml'), 'utf8');
  const match = cargoToml.match(/\[workspace\.package\][\s\S]*?version = "([^"]+)"/u);
  assert.ok(match, 'workspace.package.version must exist');
  assert.equal(packageJson.version, match[1]);
});

test('resolveLatestReleaseTag honors AMPLIHACK_NPM_VERSION override', async () => {
  const prev = process.env.AMPLIHACK_NPM_VERSION;
  process.env.AMPLIHACK_NPM_VERSION = 'v9.9.9';
  try {
    const tag = await resolveLatestReleaseTag('0.0.1');
    assert.equal(tag, '9.9.9', 'leading v stripped from override');
  } finally {
    if (prev === undefined) {
      delete process.env.AMPLIHACK_NPM_VERSION;
    } else {
      process.env.AMPLIHACK_NPM_VERSION = prev;
    }
  }
});

test('resolveLatestReleaseTag falls back when network disabled', async () => {
  const prev = process.env.AMPLIHACK_NPM_NO_LATEST;
  const prevExplicit = process.env.AMPLIHACK_NPM_VERSION;
  process.env.AMPLIHACK_NPM_NO_LATEST = '1';
  delete process.env.AMPLIHACK_NPM_VERSION;
  try {
    const tag = await resolveLatestReleaseTag('1.2.3');
    assert.equal(tag, '1.2.3');
  } finally {
    if (prev === undefined) {
      delete process.env.AMPLIHACK_NPM_NO_LATEST;
    } else {
      process.env.AMPLIHACK_NPM_NO_LATEST = prev;
    }
    if (prevExplicit !== undefined) {
      process.env.AMPLIHACK_NPM_VERSION = prevExplicit;
    }
  }
});

// ─────────────────────────────────────────────────────────────────────────────
// Issue #333: TTL cache for latest release tag.
// ─────────────────────────────────────────────────────────────────────────────

function withIsolatedCache(testFn) {
  return async (t) => {
    const tempDir = await fs.promises.mkdtemp(path.join(os.tmpdir(), 'amplihack-tagcache-'));
    const prevCache = process.env.AMPLIHACK_NPM_WRAPPER_CACHE;
    const prevVersion = process.env.AMPLIHACK_NPM_VERSION;
    const prevNoLatest = process.env.AMPLIHACK_NPM_NO_LATEST;
    const prevTtl = process.env.AMPLIHACK_NPM_LATEST_TTL_MS;
    process.env.AMPLIHACK_NPM_WRAPPER_CACHE = tempDir;
    delete process.env.AMPLIHACK_NPM_VERSION;
    delete process.env.AMPLIHACK_NPM_NO_LATEST;
    delete process.env.AMPLIHACK_NPM_LATEST_TTL_MS;
    try {
      await testFn(t, tempDir);
    } finally {
      if (prevCache === undefined) delete process.env.AMPLIHACK_NPM_WRAPPER_CACHE;
      else process.env.AMPLIHACK_NPM_WRAPPER_CACHE = prevCache;
      if (prevVersion === undefined) delete process.env.AMPLIHACK_NPM_VERSION;
      else process.env.AMPLIHACK_NPM_VERSION = prevVersion;
      if (prevNoLatest === undefined) delete process.env.AMPLIHACK_NPM_NO_LATEST;
      else process.env.AMPLIHACK_NPM_NO_LATEST = prevNoLatest;
      if (prevTtl === undefined) delete process.env.AMPLIHACK_NPM_LATEST_TTL_MS;
      else process.env.AMPLIHACK_NPM_LATEST_TTL_MS = prevTtl;
      await fs.promises.rm(tempDir, { recursive: true, force: true });
    }
  };
}

test('latest-tag cache lives in cacheStateRoot, not per-version cacheRoot', withIsolatedCache(async () => {
  const expected = path.join(cacheStateRoot(), '.latest-tag.json');
  assert.equal(latestTagCachePath(), expected);
}));

test('readLatestTagCache returns null when cache file is missing', withIsolatedCache(async () => {
  assert.equal(readLatestTagCache(), null);
}));

test('writeLatestTagCache + readLatestTagCache roundtrip a valid tag', withIsolatedCache(async () => {
  writeLatestTagCache('1.2.3', 1_000_000);
  assert.equal(readLatestTagCache(1_000_000), '1.2.3');
}));

test('readLatestTagCache returns null after TTL expiry', withIsolatedCache(async () => {
  process.env.AMPLIHACK_NPM_LATEST_TTL_MS = '60000';
  writeLatestTagCache('1.2.3', 1_000_000);
  assert.equal(readLatestTagCache(1_000_000), '1.2.3');
  assert.equal(readLatestTagCache(1_000_000 + 60_001), null);
}));

test('TTL=0 disables the cache entirely', withIsolatedCache(async () => {
  process.env.AMPLIHACK_NPM_LATEST_TTL_MS = '0';
  writeLatestTagCache('1.2.3', 1_000_000);
  assert.equal(readLatestTagCache(1_000_000), null);
}));

test('readLatestTagCache rejects corrupt JSON without throwing', withIsolatedCache(async (_, tempDir) => {
  await fs.promises.writeFile(latestTagCachePath(), 'this is not json{{{');
  assert.equal(readLatestTagCache(), null);
}));

test('readLatestTagCache rejects unknown schema versions', withIsolatedCache(async () => {
  await fs.promises.writeFile(latestTagCachePath(), JSON.stringify({
    schema: 99,
    tag: '1.2.3',
    fetched_at: Date.now(),
  }));
  assert.equal(readLatestTagCache(), null);
}));

test('writeLatestTagCache rejects malformed tags', withIsolatedCache(async () => {
  writeLatestTagCache('not-a-version', 1_000_000);
  assert.equal(fs.existsSync(latestTagCachePath()), false);
}));

test('AMPLIHACK_NPM_NO_LATEST=1 bypasses cache and returns fallback', withIsolatedCache(async () => {
  // Even with a fresh cache entry, the env var override must win.
  writeLatestTagCache('9.9.9', Date.now());
  process.env.AMPLIHACK_NPM_NO_LATEST = '1';
  const tag = await resolveLatestReleaseTag('1.2.3');
  assert.equal(tag, '1.2.3');
}));

test('AMPLIHACK_NPM_VERSION override beats both cache and AMPLIHACK_NPM_NO_LATEST', withIsolatedCache(async () => {
  writeLatestTagCache('9.9.9', Date.now());
  process.env.AMPLIHACK_NPM_NO_LATEST = '1';
  process.env.AMPLIHACK_NPM_VERSION = 'v7.7.7';
  const tag = await resolveLatestReleaseTag('1.2.3');
  assert.equal(tag, '7.7.7');
}));

test('resolveLatestReleaseTag returns cached tag without network call', withIsolatedCache(async () => {
  // Pre-populate cache; if resolveLatestReleaseTag tried the network here it
  // would hit api.github.com — but the cache should short-circuit first.
  writeLatestTagCache('5.6.7', Date.now());
  const tag = await resolveLatestReleaseTag('1.2.3');
  assert.equal(tag, '5.6.7');
}));
