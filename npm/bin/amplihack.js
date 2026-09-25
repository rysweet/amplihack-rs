#!/usr/bin/env node
'use strict';

const { ensureNativeBinaries, packageRoot, runAmplihack } = require('../lib/bootstrap');
const pkg = require('../../package.json');

async function main() {
  const root = packageRoot(__dirname);
  const { mainBinary } = await ensureNativeBinaries({
    root,
    version: pkg.version,
  });
  runAmplihack(mainBinary, process.argv.slice(2));
}

// The Rust installer recognises this launcher by the message below (see
// `is_amplihack_npm_launcher` in crates/amplihack-cli). Keep it verbatim.
main().catch((error) => {
  console.error(`amplihack npm wrapper failed: ${error.message}`);
  process.exit(1);
});
