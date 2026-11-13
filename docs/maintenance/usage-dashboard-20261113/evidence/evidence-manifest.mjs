import fs from 'node:fs';
import path from 'node:path';
import crypto from 'node:crypto';
const root = path.dirname(new URL(import.meta.url).pathname);
const target = path.join(root, 'hashes.json');
const files = fs.readdirSync(root, {recursive:true}).filter(f => fs.statSync(path.join(root,f)).isFile() && f !== 'hashes.json').sort();
const hashes = Object.fromEntries(files.map(f => [f,crypto.createHash('sha256').update(fs.readFileSync(path.join(root,f))).digest('hex')]));
if(process.argv.includes('--write')) fs.writeFileSync(target,JSON.stringify(hashes,null,2)+'\n');
else { const old=JSON.parse(fs.readFileSync(target)); if(JSON.stringify(old)!==JSON.stringify(hashes)) throw new Error('Evidence hashes changed'); console.log(`${files.length} evidence files verified`); }
