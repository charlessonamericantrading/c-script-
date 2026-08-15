#!/usr/bin/env node

// Wrapper ejecutor de linkc para el ecosistema Node.js / npm / npx.
// Si el binario nativo está disponible lo delega directamente con cero overhead.

const { spawn } = require('child_process');
const path = require('path');
const os = require('os');

const binaryName = os.platform() === 'win32' ? 'linkc.exe' : 'linkc';
const localBinary = path.join(__dirname, '..', '..', 'compiler', 'target', 'release', binaryName);

const args = process.argv.slice(2);
const child = spawn(localBinary, args, { stdio: 'inherit' });

child.on('error', (err) => {
  // Si no encuentra el binario local, intenta invocar linkc desde el PATH global
  const fallback = spawn('linkc', args, { stdio: 'inherit' });
  fallback.on('error', () => {
    console.error('Error: no se encontró el binario nativo linkc. Instalalo ejecutando cargo install o descarga el binario oficial.');
    process.exit(1);
  });
  fallback.on('exit', (code) => process.exit(code || 0));
});

child.on('exit', (code) => {
  process.exit(code || 0);
});
