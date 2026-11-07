#!/usr/bin/env python3
"""Package only verified Linux ELF and same-worktree WASM release artifacts."""
import hashlib
import json
import os
from pathlib import Path
import platform
import shutil
import struct
import subprocess
import sys
import tarfile
import tempfile

ROOT = Path(__file__).resolve().parents[1]
SCHEMA_VERSION = 45

def digest(path):
    h = hashlib.sha256()
    with path.open('rb') as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b''):
            h.update(chunk)
    return h.hexdigest()

def linux_elf(path):
    header = path.read_bytes()[:20]
    if len(header) != 20 or header[:6] != b'\x7fELF\x02\x01' or struct.unpack('<H', header[18:20])[0] != 62:
        raise ValueError('artifact must be little-endian Linux x86_64 ELF64')

def stage_migrations(root, tree):
    """Archive reviewable SQL with the same byte concatenation as schema.rs."""
    sources = {
        '43': ('crates/server/src/db/request_paths.sql', 'deploy/migrations/43-request-paths.sql'),
        '44': ('crates/server/src/db/account_overview.sql', 'deploy/migrations/44-account-overview.sql'),
    }
    parts = [
        ('crates/server/src/db/query_performance.sql', 'deploy/migrations/45-query-performance.part.sql'),
        ('crates/server/src/db/model_invocation_totals.sql', 'deploy/migrations/45-model-invocation-totals.part.sql'),
    ]
    # Read every required part first; a missing migration must never produce a
    # distributable archive that silently omits part of the executed migration.
    payloads = {source: (root / source).read_bytes() for source, _ in [*sources.values(), *parts]}
    metadata = {}
    for version, (source, archive) in sources.items():
        target = tree / archive
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(payloads[source])
        metadata[version] = {'source_path': source, 'archive_path': archive,
                             'sha256': hashlib.sha256(payloads[source]).hexdigest()}
    part_metadata = []
    for source, archive in parts:
        (tree / archive).write_bytes(payloads[source])
        part_metadata.append({'source_path': source, 'archive_path': archive,
                              'sha256': hashlib.sha256(payloads[source]).hexdigest()})
    # schema.rs uses concat!(include_str!(part0), "\n", include_str!(part1)).
    # In particular, do not strip a final newline or normalize source line ends.
    combined = payloads[parts[0][0]] + b'\n' + payloads[parts[1][0]]
    archive = 'deploy/migrations/45-query-performance.sql'
    (tree / archive).write_bytes(combined)
    metadata['45'] = {'archive_path': archive, 'sha256': hashlib.sha256(combined).hexdigest(),
                      'parts': part_metadata, 'composition': 'parts[0] + "\\n" + parts[1]'}
    return metadata

def main():
    if platform.system() != 'Linux' or platform.machine() != 'x86_64':
        raise ValueError('Linux x86_64 native builder required')
    if subprocess.check_output(['git', 'status', '--porcelain', '--untracked-files=normal'], cwd=ROOT).strip():
        raise ValueError('release requires clean committed source')
    commit = subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip()
    output = Path(sys.argv[1]).resolve()
    output.mkdir(parents=True, exist_ok=True)
    name = f'tokengo-{commit[:12]}-linux-x86_64'
    with tempfile.TemporaryDirectory() as temp:
        tree = Path(temp) / name
        (tree / 'bin').mkdir(parents=True)
        for binary in ['tokengo-server', 'tokengo']:
            source = ROOT / 'target/x86_64-unknown-linux-gnu/release' / binary
            linux_elf(source)
            shutil.copy2(source, tree / 'bin' / binary)
        web = ROOT / 'crates/web/dist'
        if not (web / 'index.html').is_file() or not list(web.glob('*.wasm')) or not list(web.glob('*.js')):
            raise ValueError('Trunk release output missing index, WASM, or JS')
        shutil.copytree(web, tree / 'web')
        shutil.copytree(ROOT / 'deploy', tree / 'deploy', ignore=shutil.ignore_patterns('__pycache__'))
        migrations = stage_migrations(ROOT, tree)
        for document in ['deployment.md', 'operations-tokens.md', 'account-login.md']:
            shutil.copy2(ROOT / 'docs' / document, tree / document)
        # Ship the complete matching development/user docs and an optional
        # standard-library client. The Rust gateway has no Python dependency.
        shutil.copytree(ROOT / 'docs', tree / 'docs')
        (tree / 'tools').mkdir()
        shutil.copy2(ROOT / 'scripts' / 'mineru-gateway-client.py', tree / 'tools' / 'mineru-gateway-client.py')
        shutil.copy2(ROOT / 'LICENSE', tree / 'LICENSE')
        shutil.copy2(ROOT / 'NOTICE', tree / 'NOTICE')
        manifest = {'commit': commit, 'target': 'x86_64-unknown-linux-gnu', 'rust': '1.98.0',
                    'trunk': '0.21.14', 'cargo_lock_sha256': digest(ROOT / 'Cargo.lock'),
                    'schema_version': SCHEMA_VERSION, 'migrations': migrations}
        (tree / 'manifest.json').write_text(json.dumps(manifest, indent=2) + '\n')
        checksums = {str(p.relative_to(tree)): digest(p) for p in tree.rglob('*') if p.is_file()}
        (tree / 'checksums.json').write_text(json.dumps(checksums, sort_keys=True, indent=2) + '\n')
        archive = output / f'{name}.tar.gz'
        if archive.exists():
            raise ValueError('output exists; use a fresh output directory')
        with tarfile.open(archive, 'w:gz') as tar:
            tar.add(tree, arcname=name)
        (output / f'{archive.name}.sha256').write_text(f'{digest(archive)}  {archive.name}\n')
        print(archive)

if __name__ == '__main__':
    main()
