import { mkdirSync, existsSync, writeFileSync, chmodSync } from 'node:fs';
import { execFileSync } from 'node:child_process';
import { networkInterfaces, hostname } from 'node:os';
import { fileURLToPath } from 'node:url';

const directory = fileURLToPath(new URL('../.cert/', import.meta.url));
mkdirSync(directory, { recursive: true, mode: 0o700 });
const run = args => execFileSync('openssl', args, { cwd: directory, stdio: 'inherit' });
if (!existsSync(`${directory}/ca-key.pem`)) {
  run(['req', '-x509', '-newkey', 'rsa:2048', '-nodes', '-days', '365', '-keyout', 'ca-key.pem', '-out', 'local-ca.pem', '-subj', '/CN=MapLibre Local XR Development', '-addext', 'basicConstraints=critical,CA:TRUE', '-addext', 'keyUsage=critical,keyCertSign,cRLSign']);
  chmodSync(`${directory}/ca-key.pem`, 0o600);
}
const addresses = ['127.0.0.1', ...Object.values(networkInterfaces()).flat().filter(i => i && !i.internal && i.family === 'IPv4').map(i => i.address)];
const dns = [...new Set(['localhost', hostname(), `${hostname().replace(/\.local$/, '')}.local`])];
writeFileSync(`${directory}/server.ext`, `subjectAltName=${[...dns.map(d => `DNS:${d}`), ...addresses.map(ip => `IP:${ip}`)].join(',')}\nbasicConstraints=CA:FALSE\nkeyUsage=digitalSignature,keyEncipherment\nextendedKeyUsage=serverAuth\n`);
run(['req', '-newkey', 'rsa:2048', '-nodes', '-keyout', 'server-key.pem', '-out', 'server.csr', '-subj', '/CN=MapLibre Local XR']);
chmodSync(`${directory}/server-key.pem`, 0o600);
run(['x509', '-req', '-in', 'server.csr', '-CA', 'local-ca.pem', '-CAkey', 'ca-key.pem', '-CAcreateserial', '-days', '30', '-extfile', 'server.ext', '-out', 'server.pem']);
run(['x509', '-in', 'local-ca.pem', '-outform', 'der', '-out', 'local-ca.cer']);
console.info(`Local certificate ready for ${addresses.join(', ')}. Trust .cert/local-ca.cer on the headset before opening HTTPS. Private keys stay in .cert; do not share them.`);
