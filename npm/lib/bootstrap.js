'use strict';

const crypto = require('node:crypto');
const fs = require('node:fs');
const fsp = require('node:fs/promises');
const https = require('node:https');
const os = require('node:os');
const path = require('node:path');
const { setTimeout: delay } = require('node:timers/promises');
const { spawnSync } = require('node:child_process');

const GITHUB_REPO = 'rysweet/amplihack-rs';
const DOWNLOAD_SIZE_LIMIT = 512 * 1024 * 1024;
const ALLOWED_DOWNLOAD_PREFIXES = [
  'https://api.github.com/',
  'https://github.com/',
  'https://objects.githubusercontent.com/',
  'https://release-assets.githubusercontent.com/',
];
const INSTALL_LOCK_FILE = '.install-lock';
const INSTALL_LOCK_TIMEOUT_MS = 120000;
const INSTALL_LOCK_POLL_MS = 200;

function binaryFilename(name, platform = process.platform) {
  return platform === 'win32' ? `${name}.exe` : name;
}

function releaseTargetFor(platform = process.platform, arch = process.arch) {
  if (platform === 'linux' && arch === 'x64') {
    return 'x86_64-unknown-linux-gnu';
  }
  if (platform === 'linux' && arch === 'arm64') {
    return 'aarch64-unknown-linux-gnu';
  }
  if (platform === 'darwin' && arch === 'x64') {
    return 'x86_64-apple-darwin';
  }
  if (platform === 'darwin' && arch === 'arm64') {
    return 'aarch64-apple-darwin';
  }
  return null;
}

function releaseUrls(version, target) {
  const base = `https://github.com/${GITHUB_REPO}/releases/download/v${version}`;
  const archiveUrl = `${base}/amplihack-${target}.tar.gz`;
  return {
    archiveUrl,
    checksumUrl: `${archiveUrl}.sha256`,
  };
}

function packageRoot(rootDir = __dirname) {
  return path.resolve(rootDir, '..', '..');
}

function cacheRoot(version, platform = process.platform, arch = process.arch) {
  const explicit = process.env.AMPLIHACK_NPM_WRAPPER_CACHE;
  if (explicit) {
    return path.resolve(explicit);
  }
  return path.join(os.homedir(), '.cache', 'amplihack', 'npm-wrapper', version, `${platform}-${arch}`);
}

function hasLocalCargoWorkspace(root) {
  return [
    'Cargo.toml',
    path.join('bins', 'amplihack', 'Cargo.toml'),
    path.join('bins', 'amplihack-hooks', 'Cargo.toml'),
  ].every((relativePath) => fs.existsSync(path.join(root, relativePath)));
}

function findBinary(root, fileName) {
  const queue = [root];
  while (queue.length > 0) {
    const current = queue.shift();
    const entries = fs.readdirSync(current, { withFileTypes: true });
    for (const entry of entries) {
      const fullPath = path.join(current, entry.name);
      if (entry.isDirectory()) {
        queue.push(fullPath);
        continue;
      }
      if (entry.isFile() && entry.name === fileName) {
        return fullPath;
      }
    }
  }
  throw new Error(`Unable to locate ${fileName} under ${root}`);
}

function parseChecksumHex(text) {
  const hex = text.trim().split(/\s+/u)[0];
  if (!hex || !/^[a-fA-F0-9]{64}$/u.test(hex)) {
    throw new Error('Checksum file did not contain a valid SHA-256 digest');
  }
  return hex;
}

function validateDownloadUrl(url) {
  if (!ALLOWED_DOWNLOAD_PREFIXES.some((prefix) => url.startsWith(prefix))) {
    throw new Error(
      `Download URL is not from an allowed GitHub host: ${url}`,
    );
  }
}

function verifyArchiveChecksum(archiveBytes, checksumText, archiveUrl) {
  const expectedHex = parseChecksumHex(checksumText);
  const actualHex = crypto.createHash('sha256').update(archiveBytes).digest('hex');
  if (actualHex.toLowerCase() !== expectedHex.toLowerCase()) {
    throw new Error(`SHA-256 mismatch for ${archiveUrl}`);
  }
}

function processExists(pidText) {
  const pid = Number.parseInt(String(pidText || '').trim(), 10);
  if (!Number.isInteger(pid) || pid <= 0) {
    return null;
  }
  try {
    process.kill(pid, 0);
    return true;
  } catch (error) {
    if (error && error.code === 'ESRCH') {
      return false;
    }
    if (error && error.code === 'EPERM') {
      return true;
    }
    throw error;
  }
}

async function acquireInstallLock(installRoot, {
  timeoutMs = INSTALL_LOCK_TIMEOUT_MS,
  pollMs = INSTALL_LOCK_POLL_MS,
} = {}) {
  const lockPath = path.join(installRoot, INSTALL_LOCK_FILE);
  const deadline = Date.now() + timeoutMs;

  while (true) {
    try {
      const handle = await fsp.open(lockPath, 'wx');
      await handle.writeFile(`${process.pid}\n`);
      await handle.close();
      return async () => {
        await fsp.rm(lockPath, { force: true });
      };
    } catch (error) {
      if (!error || error.code !== 'EEXIST') {
        throw error;
      }
      const holder = await fsp.readFile(lockPath, 'utf8').catch(() => '');
      const holderAlive = processExists(holder);
      if (holderAlive === false) {
        await fsp.rm(lockPath, { force: true });
        continue;
      }
      if (Date.now() >= deadline) {
        throw new Error(`Timed out waiting for install lock at ${lockPath}`);
      }
      await delay(pollMs);
    }
  }
}

async function copyFileAtomic(source, destination, mode = 0o755) {
  const tempDestination = `${destination}.tmp-${process.pid}-${crypto.randomUUID()}`;
  try {
    await fsp.copyFile(source, tempDestination);
    await fsp.chmod(tempDestination, mode);
    await fsp.rm(destination, { force: true });
    await fsp.rename(tempDestination, destination);
  } catch (error) {
    await fsp.rm(tempDestination, { force: true }).catch(() => {});
    throw error;
  }
}

function download(url) {
  return new Promise((resolve, reject) => {
    const seen = new Set();

    function fetch(currentUrl) {
      validateDownloadUrl(currentUrl);
      if (seen.has(currentUrl)) {
        reject(new Error(`Redirect loop while downloading ${url}`));
        return;
      }
      seen.add(currentUrl);

      https
        .get(currentUrl, {
          headers: {
            'User-Agent': 'amplihack-npm-wrapper',
            Accept: 'application/octet-stream,application/vnd.github+json',
          },
        }, (response) => {
          const statusCode = response.statusCode || 0;
          if ([301, 302, 303, 307, 308].includes(statusCode) && response.headers.location) {
            response.resume();
            fetch(response.headers.location);
            return;
          }
          if (statusCode < 200 || statusCode >= 300) {
            response.resume();
            reject(new Error(`HTTP ${statusCode} while downloading ${currentUrl}`));
            return;
          }

          const chunks = [];
          let total = 0;
          response.on('data', (chunk) => {
            total += chunk.length;
            if (total > DOWNLOAD_SIZE_LIMIT) {
              response.destroy(new Error(`Download exceeded ${DOWNLOAD_SIZE_LIMIT} bytes`));
              return;
            }
            chunks.push(chunk);
          });
          response.on('end', () => resolve(Buffer.concat(chunks)));
          response.on('error', reject);
        })
        .on('error', reject);
    }

    fetch(url);
  });
}

/**
 * Resolve the latest published release tag for the configured GitHub repo.
 *
 * Returns the bare semver string (e.g. "0.7.63"). Falls back to the
 * fallbackVersion parameter when the API call fails (offline, rate-limited,
 * etc.) so installation degrades gracefully — only the freshness suffers.
 *
 * Honors AMPLIHACK_NPM_VERSION as an explicit override (set by users or CI
 * who want a specific pinned version).
 */
async function resolveLatestReleaseTag(fallbackVersion) {
  const explicit = process.env.AMPLIHACK_NPM_VERSION;
  if (explicit) {
    return explicit.replace(/^v/, '');
  }
  if (process.env.AMPLIHACK_NPM_NO_LATEST === '1') {
    return fallbackVersion;
  }
  const url = `https://api.github.com/repos/${GITHUB_REPO}/releases/latest`;
  try {
    const body = await download(url);
    const data = JSON.parse(body.toString('utf8'));
    const tag = typeof data.tag_name === 'string' ? data.tag_name.replace(/^v/, '') : '';
    if (!/^\d+\.\d+\.\d+/.test(tag)) {
      return fallbackVersion;
    }
    return tag;
  } catch {
    return fallbackVersion;
  }
}

async function installFromRelease(version, installRoot) {
  const target = releaseTargetFor();
  if (!target) {
    throw new Error('No published release target exists for this Node platform/architecture');
  }

  const { archiveUrl, checksumUrl } = releaseUrls(version, target);
  const archiveBytes = await download(archiveUrl);
  const checksumBytes = await download(checksumUrl);
  verifyArchiveChecksum(archiveBytes, checksumBytes.toString('utf8'), archiveUrl);

  const tempRoot = await fsp.mkdtemp(path.join(os.tmpdir(), 'amplihack-npm-'));
  try {
    const archivePath = path.join(tempRoot, 'amplihack.tar.gz');
    const extractDir = path.join(tempRoot, 'extract');
    await fsp.mkdir(extractDir, { recursive: true });
    await fsp.writeFile(archivePath, archiveBytes);

    const tar = spawnSync('tar', ['-xzf', archivePath, '-C', extractDir], {
      stdio: 'inherit',
    });
    if (tar.error) {
      throw new Error(`Failed to extract release archive with tar: ${tar.error.message}`);
    }
    if (tar.status !== 0) {
      throw new Error(`tar extraction failed with exit code ${tar.status}`);
    }

    const binDir = path.join(installRoot, 'bin');
    await fsp.mkdir(binDir, { recursive: true });
    for (const binary of ['amplihack', 'amplihack-hooks']) {
      const source = findBinary(extractDir, binaryFilename(binary));
      const destination = path.join(binDir, binaryFilename(binary));
      await copyFileAtomic(source, destination);
    }
  } finally {
    await fsp.rm(tempRoot, { recursive: true, force: true });
  }
}

async function buildFromSource(root, installRoot) {
  const targetDir = path.join(installRoot, 'target');
  const build = spawnSync(
    'cargo',
    ['build', '--release', '--locked', '--bin', 'amplihack', '--bin', 'amplihack-hooks'],
    {
      cwd: root,
      stdio: 'inherit',
      env: {
        ...process.env,
        CARGO_TARGET_DIR: targetDir,
      },
    },
  );

  if (build.error) {
    throw new Error(`Failed to launch cargo build: ${build.error.message}`);
  }
  if (build.status !== 0) {
    throw new Error(`cargo build exited with code ${build.status}`);
  }

  const binDir = path.join(installRoot, 'bin');
  const releaseDir = path.join(targetDir, 'release');
  await fsp.mkdir(binDir, { recursive: true });
  for (const binary of ['amplihack', 'amplihack-hooks']) {
    const source = path.join(releaseDir, binaryFilename(binary));
    const destination = path.join(binDir, binaryFilename(binary));
    await copyFileAtomic(source, destination);
  }
}

async function ensureNativeBinaries({ root, version }) {
  // Resolve the freshest available release tag at install time. The
  // package.json `version` field can drift behind the latest published
  // release (the release workflow publishes new tags without rewriting
  // package.json), so trusting `pkg.version` results in npx installs that
  // ship a stale binary. Falling back to `version` keeps offline / API-
  // rate-limited installs working.
  const effectiveVersion = await resolveLatestReleaseTag(version);
  const installRoot = cacheRoot(effectiveVersion);
  const binDir = path.join(installRoot, 'bin');
  const mainBinary = path.join(binDir, binaryFilename('amplihack'));
  const hooksBinary = path.join(binDir, binaryFilename('amplihack-hooks'));
  if (fs.existsSync(mainBinary) && fs.existsSync(hooksBinary)) {
    return { mainBinary, hooksBinary, installRoot };
  }

  await fsp.mkdir(installRoot, { recursive: true });
  const releaseInstallLock = await acquireInstallLock(installRoot);
  try {
    if (fs.existsSync(mainBinary) && fs.existsSync(hooksBinary)) {
      return { mainBinary, hooksBinary, installRoot };
    }

    const forceSource = process.env.AMPLIHACK_NPM_WRAPPER_FORCE_SOURCE === '1';
    const localCargoWorkspace = hasLocalCargoWorkspace(root);
    const errors = [];

    if (!forceSource) {
      try {
        await installFromRelease(effectiveVersion, installRoot);
        return { mainBinary, hooksBinary, installRoot };
      } catch (error) {
        errors.push(`release download failed: ${error.message}`);
      }
    }

    if (localCargoWorkspace) {
      try {
        await buildFromSource(root, installRoot);
        return { mainBinary, hooksBinary, installRoot };
      } catch (error) {
        errors.push(`source build failed: ${error.message}`);
      }
    } else {
      errors.push('local Cargo workspace not present for source-build fallback');
    }

    throw new Error(errors.join('\n'));
  } finally {
    await releaseInstallLock();
  }
}

function runAmplihack(binaryPath, args) {
  const child = spawnSync(binaryPath, args, {
    stdio: 'inherit',
    env: process.env,
  });
  if (child.error) {
    throw child.error;
  }
  process.exit(child.status ?? 1);
}

module.exports = {
  acquireInstallLock,
  binaryFilename,
  cacheRoot,
  copyFileAtomic,
  ensureNativeBinaries,
  findBinary,
  hasLocalCargoWorkspace,
  installFromRelease,
  packageRoot,
  parseChecksumHex,
  releaseTargetFor,
  releaseUrls,
  resolveLatestReleaseTag,
  runAmplihack,
  validateDownloadUrl,
  verifyArchiveChecksum,
};
