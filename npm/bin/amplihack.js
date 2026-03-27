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

main().catch((error) => {
  console.error(`amplihack npm wrapper failed: ${error.message}`);
  process.exit(1);
});
