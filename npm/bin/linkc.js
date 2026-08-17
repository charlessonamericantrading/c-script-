#!/usr/bin/env node

// Wrapper ejecutor universal de linkc para Node.js / npm / npx.
// 1. Usa el binario local del compilador si existe.
// 2. Usa el binario instalado globalmente en PATH o ~/.c-script/bin si existe.
// 3. Si no existe, descarga automáticamente el binario precompilado desde GitHub Releases.

const { spawn, spawnSync } = require('child_process');
const path = require('path');
const os = require('os');
const fs = require('fs');
const https = require('https');

const REPO = 'charlessonamericantrading/c-script-';
const isWin = os.platform() === 'win32';
const binaryName = isWin ? 'linkc.exe' : 'linkc';

const userHome = os.homedir();
const userInstallDir = path.join(userHome, '.c-script', 'bin');
const cachedBinary = path.join(userInstallDir, binaryName);
const repoLocalBinary = path.join(__dirname, '..', '..', 'compiler', 'target', 'release', binaryName);

const args = process.argv.slice(2);

function runBinary(binPath) {
  const child = spawn(binPath, args, { stdio: 'inherit' });
  child.on('exit', (code) => process.exit(code || 0));
  child.on('error', (err) => {
    console.error(`Error al ejecutar ${binPath}:`, err.message);
    process.exit(1);
  });
}

function resolveAsset() {
  const platform = os.platform();
  const arch = os.arch();

  if (platform === 'win32' && arch === 'x64') {
    return 'linkc-x86_64-pc-windows-msvc.zip';
  } else if (platform === 'linux' && arch === 'x64') {
    return 'linkc-x86_64-unknown-linux-gnu.tar.gz';
  } else if (platform === 'darwin' && (arch === 'arm64' || arch === 'aarch64')) {
    return 'linkc-aarch64-apple-darwin.tar.gz';
  } else if (platform === 'darwin' && arch === 'x64') {
    return 'linkc-x86_64-apple-darwin.tar.gz';
  }
  return null;
}

function downloadReleaseBinary(callback) {
  const asset = resolveAsset();
  if (!asset) {
    console.error(`[link-lang] Plataforma o arquitectura no soportada para descarga automática: ${os.platform()} (${os.arch()})`);
    console.error('Por favor instala linkc manualmente con Rust (cargo build --release).');
    process.exit(1);
  }

  if (!fs.existsSync(userInstallDir)) {
    fs.mkdirSync(userInstallDir, { recursive: true });
  }

  const url = `https://github.com/${REPO}/releases/latest/download/${asset}`;
  console.log(`[link-lang] Descargando binario nativo linkc desde ${url}...`);

  const tempFile = path.join(os.tmpdir(), `linkc_download_${Date.now()}_${asset}`);
  const file = fs.createWriteStream(tempFile);

  function followRedirect(targetUrl) {
    https.get(targetUrl, (res) => {
      if (res.statusCode >= 300 && res.statusCode < 400 && res.headers.location) {
        return followRedirect(res.headers.location);
      }
      if (res.statusCode !== 200) {
        console.error(`[link-lang] Falló la descarga (HTTP ${res.statusCode}).`);
        process.exit(1);
      }
      res.pipe(file);
      file.on('finish', () => {
        file.close(() => {
          extractAsset(tempFile, asset, callback);
        });
      });
    }).on('error', (err) => {
      fs.unlink(tempFile, () => {});
      console.error('[link-lang] Error de conexión:', err.message);
      process.exit(1);
    });
  }

  followRedirect(url);
}

function extractAsset(tempFile, asset, callback) {
  try {
    if (asset.endsWith('.zip')) {
      const psCmd = `Expand-Archive -Path "${tempFile}" -DestinationPath "${userInstallDir}" -Force`;
      spawnSync('powershell', ['-NoProfile', '-Command', psCmd], { stdio: 'inherit' });
    } else if (asset.endsWith('.tar.gz')) {
      spawnSync('tar', ['-xzf', tempFile, '-C', userInstallDir], { stdio: 'inherit' });
    }
    fs.unlink(tempFile, () => {});

    if (fs.existsSync(cachedBinary)) {
      if (!isWin) {
        fs.chmodSync(cachedBinary, 0o755);
      }
      callback(cachedBinary);
    } else {
      console.error('[link-lang] El binario no se encontró tras la descompresión.');
      process.exit(1);
    }
  } catch (err) {
    console.error('[link-lang] Error al descomprimir el binario:', err.message);
    process.exit(1);
  }
}

// 1. Chequear binario en repositorio local
if (fs.existsSync(repoLocalBinary)) {
  runBinary(repoLocalBinary);
}
// 2. Chequear binario en cache de usuario (~/.c-script/bin)
else if (fs.existsSync(cachedBinary)) {
  runBinary(cachedBinary);
}
// 3. Chequear en PATH global
else {
  const which = spawnSync(isWin ? 'where' : 'which', ['linkc']);
  if (which.status === 0) {
    runBinary('linkc');
  } else {
    // 4. Descargar automáticamente en demanda
    downloadReleaseBinary((installedPath) => {
      runBinary(installedPath);
    });
  }
}
