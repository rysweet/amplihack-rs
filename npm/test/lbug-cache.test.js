'use strict';

// Issue #1917. Child environments are built from an allowlist; no real HOME,
// GitHub auth, Cargo, compiler, application runtime or network is used.
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { spawnSync } = require('node:child_process');

test('v1 npm source fallback retains release Cargo and provisions both binaries', (t) => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'issue-1917-npm-'));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const source = path.join(root, 'source');
  const tools = path.join(root, 'tools');
  const cache = path.join(root, 'cache');
  for (const dir of [source, tools, cache, path.join(root, 'home'), path.join(source, 'scripts')]) {
    fs.mkdirSync(dir, { recursive: true, mode: 0o700 });
  }
  for (const file of ['Cargo.toml', 'bins/amplihack/Cargo.toml', 'bins/amplihack-hooks/Cargo.toml']) {
    fs.mkdirSync(path.dirname(path.join(source, file)), { recursive: true });
    fs.writeFileSync(path.join(source, file), '# source fixture\n');
  }
  const trace = path.join(root, 'wrapper-args');
  fs.writeFileSync(path.join(source, 'scripts/cargo-with-lbug.sh'), `#!/bin/sh
printf '%s\\n' "$@" > "$TEST_TRACE"
exec cargo "$@"
`, { mode: 0o700 });
  fs.writeFileSync(path.join(tools, 'cargo'), `#!/bin/sh
printf '%s\\n' "$@" > "$TEST_TRACE"
mkdir -p "$CARGO_TARGET_DIR/release"
for name in amplihack amplihack-hooks; do
  printf '#!/bin/sh\\nexit 0\\n' > "$CARGO_TARGET_DIR/release/$name"
  chmod 700 "$CARGO_TARGET_DIR/release/$name"
done
`, { mode: 0o700 });
  const bootstrap = path.resolve(__dirname, '../lib/bootstrap.js');
  const script = `
    // A warm application-tag record avoids acquisition; fail closed if the
    // implementation tries networking despite it.
    require('node:https').get = () => { throw new Error('forbidden network'); };
    const b = require(process.argv[1]);
    b.writeLatestTagCache('1.2.0');
    b.ensureNativeBinaries({root: process.argv[2], version: '1.2.0'})
      .then(r => process.stdout.write(JSON.stringify(r)))
      .catch(e => { console.error(e.message); process.exitCode = 1; });
  `;
  const child = spawnSync(process.execPath, ['-e', script, bootstrap, source], {
    env: {
      PATH: `${tools}:/usr/bin:/bin`,
      HOME: path.join(root, 'home'),
      AMPLIHACK_NPM_WRAPPER_CACHE: cache,
      AMPLIHACK_NPM_WRAPPER_FORCE_SOURCE: '1',
      TEST_TRACE: trace,
    },
    encoding: 'utf8', timeout: 10000,
  });
  assert.equal(child.status, 0, child.stderr);
  assert.ok(fs.existsSync(trace), 'source fallback must invoke Cargo');
  const args = fs.readFileSync(trace, 'utf8').trim().split('\n');
  assert.deepEqual(args, ['build', '--release', '--locked', '--bin', 'amplihack', '--bin', 'amplihack-hooks']);
  const result = JSON.parse(child.stdout);
  assert.ok(fs.statSync(result.mainBinary).isFile());
  assert.ok(fs.statSync(result.hooksBinary).isFile());
});

test('v1 npm archive retains its Cargo workspace without deferred vendor inputs', (t) => {
  const root = path.resolve(__dirname, '../..');
  const fixture = fs.mkdtempSync(path.join(os.tmpdir(), 'issue-1917-pack-'));
  t.after(() => fs.rmSync(fixture, { recursive: true, force: true }));
  const config = path.join(fixture, 'empty.npmrc');
  const globalConfig = path.join(fixture, 'global.npmrc');
  fs.writeFileSync(config, '');
  fs.writeFileSync(globalConfig, '');
  const packed = spawnSync('npm', ['pack', '--json', '--offline', '--ignore-scripts',
    '--pack-destination', fixture, '--cache', path.join(fixture, 'npm-cache'),
    '--userconfig', config, '--globalconfig', globalConfig], {
    cwd: root, encoding: 'utf8', timeout: 30000,
    env: { PATH: process.env.PATH, HOME: fixture, TMPDIR: fixture, npm_config_audit: 'false' },
  });
  assert.equal(packed.status, 0, packed.stderr);
  const [record] = JSON.parse(packed.stdout);
  const archive = path.join(fixture, record.filename);
  assert.ok(fs.statSync(archive).isFile(), 'inspect the actual archive, not just package.json');
  const listing = spawnSync('/usr/bin/tar', ['-tzf', archive], {
    encoding: 'utf8', timeout: 10000,
    env: { PATH: '/usr/bin:/bin', HOME: fixture },
  });
  assert.equal(listing.status, 0, listing.stderr);
  const members = new Set(listing.stdout.trim().split('\n'));
  for (const file of [
    'Cargo.toml', 'Cargo.lock', 'bins/amplihack/Cargo.toml',
    'bins/amplihack-hooks/Cargo.toml', 'npm/lib/bootstrap.js',
  ]) {
    assert.ok(members.has(`package/${file}`), `distributed source omits ${file}`);
  }
  assert.ok(!members.has('package/vendor/lbug/Cargo.toml'));
});
