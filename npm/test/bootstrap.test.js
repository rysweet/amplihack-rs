'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');

const {
  binaryFilename,
  findBinary,
  hasLocalCargoWorkspace,
  packageRoot,
  parseChecksumHex,
  releaseTargetFor,
  releaseUrls,
  validateDownloadUrl,
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
