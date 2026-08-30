#!/usr/bin/env node

// Wrapper ejecutor universal de linkc para Node.js / npm / npx.
// 1. Usa el binario local del compilador si existe.
// 2. Usa el binario instalado globalmente en PATH o ~/.c-script/bin si existe.
// 3. Si no existe, descarga automáticamente el binario precompilado desde GitHub Releases.

const crossSpawn = require('cross-spawn');
const { spawnSync } = require('child_process');
const path = require('path');
const os = require('os');
const fs = require('fs');
const https = require('https');
const crypto = require('crypto');

const REPO = 'charlessonamericantrading/c-script-';
const isWin = os.platform() === 'win32';
const binaryName = isWin ? 'linkc.exe' : 'linkc';

const userHome = os.homedir();
const userInstallDir = path.join(userHome, '.c-script', 'bin');
const cachedBinary = path.join(userInstallDir, binaryName);
const repoLocalBinary = path.join(__dirname, '..', '..', 'compiler', 'target', 'release', binaryName);

const args = process.argv.slice(2);

// Guarda contra recursión infinita: la rama 3 (abajo) puede resolver "linkc"
// en el PATH y terminar encontrando OTRA instancia de ESTE MISMO script --
// confirmado en vivo, ej. bajo `npx`, que agrega temporalmente
// `node_modules/.bin` (con un shim apuntando de vuelta acá) al PATH antes de
// correr nada. Sin esta guarda, ese hijo repetiría la MISMA búsqueda,
// encontraría el MISMO shim, y así indefinidamente -- confirmado que produce
// cientos de procesos `node.exe` reales en segundos, no una preocupación
// teórica. `runBinary` marca esta variable de entorno en el hijo cuando
// arranca vía PATH; si YA está marcada al arrancar, la rama 3 nunca se
// intenta de nuevo -- pasa directo a la búsqueda/descarga normal.
const REENTRANT_GUARD = 'LINKC_NPM_WRAPPER_REENTRANT';
const isReentrant = process.env[REENTRANT_GUARD] === '1';

// `cross-spawn` (no `child_process.spawn` a secas) porque la rama 3 puede
// terminar lanzando un shim `.cmd`/`.bat` en vez de un `.exe` real -- Windows
// nunca puede lanzar eso directo sin pasar por un shell, y la alternativa
// obvia (`shell: true` + array de args) es exactamente el patrón que Node
// mismo marcó inseguro (DEP0190: concatena argumentos sin escaparlos de
// verdad -- un espacio o un `&` en un path/connection-string podría
// romperse o inyectar). `cross-spawn` resuelve ambos problemas de forma
// correcta y ya probada, sin que este wrapper tenga que reinventar el
// quoting de línea de comandos de Windows a mano.
function runBinary(binPath, viaPath) {
  const opts = { stdio: 'inherit' };
  if (viaPath) opts.env = { ...process.env, [REENTRANT_GUARD]: '1' };
  const child = crossSpawn(binPath, args, opts);
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

// Sigue redirects (GitHub siempre 302 de /releases/latest/download/<asset>
// hacia la URL real del asset en /releases/download/<tag>/<asset>) y entrega
// la respuesta final (status 200) a `onResponse`. Cualquier otro status, o un
// error de conexión, corta el proceso con un mensaje claro -- nunca deja al
// caller adivinar por qué algo quedó a medias.
function httpGetFollowingRedirects(url, onResponse) {
  https.get(url, (res) => {
    if (res.statusCode >= 300 && res.statusCode < 400 && res.headers.location) {
      return httpGetFollowingRedirects(res.headers.location, onResponse);
    }
    if (res.statusCode !== 200) {
      console.error(`[link-lang] Falló la descarga (HTTP ${res.statusCode}): ${url}`);
      process.exit(1);
    }
    onResponse(res);
  }).on('error', (err) => {
    console.error('[link-lang] Error de conexión:', err.message);
    process.exit(1);
  });
}

// SHA256SUMS.txt (generado por release.yml, formato `sha256sum` estándar:
// "<hex en minúscula>  <nombre de archivo>" por línea) es el único registro
// de qué binario es el legítimo -- sin esto, un CDN o registro comprometido
// podría entregar un binario distinto al que el asset dice ser, y correría
// igual, sin ningún aviso.
function fetchChecksums(callback) {
  const url = `https://github.com/${REPO}/releases/latest/download/SHA256SUMS.txt`;
  httpGetFollowingRedirects(url, (res) => {
    let body = '';
    res.setEncoding('utf8');
    res.on('data', (chunk) => { body += chunk; });
    res.on('end', () => {
      const checksums = new Map();
      for (const line of body.split('\n')) {
        const match = line.match(/^([0-9a-f]{64})\s+\*?(.+)$/);
        if (match) checksums.set(match[2].trim(), match[1]);
      }
      callback(checksums);
    });
  });
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

  httpGetFollowingRedirects(url, (res) => {
    res.pipe(file);
    file.on('finish', () => {
      file.close(() => {
        fetchChecksums((checksums) => {
          verifyChecksumAndExtract(tempFile, asset, checksums, callback);
        });
      });
    });
  });
}

function verifyChecksumAndExtract(tempFile, asset, checksums, callback) {
  const expected = checksums.get(asset);
  if (!expected) {
    fs.unlink(tempFile, () => {});
    console.error(`[link-lang] SHA256SUMS.txt no lista un checksum para '${asset}' -- abortando, no se puede confirmar que el binario descargado sea el legítimo.`);
    process.exit(1);
  }
  const hash = crypto.createHash('sha256');
  hash.update(fs.readFileSync(tempFile));
  const actual = hash.digest('hex');
  if (actual !== expected) {
    fs.unlink(tempFile, () => {});
    console.error(`[link-lang] El checksum SHA-256 del binario descargado NO coincide con el publicado -- abortando, nunca se ejecuta un binario sin verificar.`);
    console.error(`  esperado: ${expected}`);
    console.error(`  obtenido: ${actual}`);
    process.exit(1);
  }
  extractAsset(tempFile, asset, callback);
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
// 3. Chequear en PATH global -- salteada si ya estamos en un hijo reentrante
// (ver REENTRANT_GUARD arriba), para no repetir la misma búsqueda que ya
// llevó hasta acá.
else if (!isReentrant && spawnSync(isWin ? 'where' : 'which', ['linkc']).status === 0) {
  runBinary('linkc', true);
} else {
  // 4. Descargar automáticamente en demanda
  downloadReleaseBinary((installedPath) => {
    runBinary(installedPath);
  });
}
