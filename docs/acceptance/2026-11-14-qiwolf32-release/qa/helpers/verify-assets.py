#!/usr/bin/env python3
"""Read-only verification of a downloaded TokenGo release (no Docker or network)."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import subprocess
import sys
import tarfile

DEFAULT_MANAGE = Path(__file__).resolve().parents[2] / 'tokengo/deploy/manage.py'


def require(condition, message):
    if not condition:
        raise ValueError(message)


def digest(path):
    value = hashlib.sha256()
    with path.open('rb') as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b''):
            value.update(chunk)
    return value.hexdigest()


def local_file(directory, name):
    # SHA256SUMS may use ./ prefixes; disallow all directories and links.
    clean = name[2:] if name.startswith('./') else name
    require(clean and Path(clean).name == clean and clean not in ('.', '..'), 'unsafe asset name')
    path = directory / clean
    require(path.is_file() and not path.is_symlink(), 'missing or linked asset: ' + clean)
    return path


def verify_sums(directory, filename):
    seen = set()
    for line in local_file(directory, filename).read_text().splitlines():
        match = re.fullmatch(r'([0-9a-f]{64}) [ *](.+)', line)
        require(match is not None, 'invalid checksum line in ' + filename)
        value, name = match.groups()
        path = local_file(directory, name)
        require(path.name not in seen, 'duplicate checksum: ' + path.name)
        require(digest(path) == value, 'checksum mismatch: ' + path.name)
        seen.add(path.name)
    require(seen, 'empty checksum manifest')
    return seen


def verify(directory, sha, manage):
    require(re.fullmatch(r'[0-9a-f]{40}', sha), 'invalid expected source SHA')
    require(directory.is_dir(), 'asset directory does not exist')
    native = f'tokengo-{sha[:12]}-linux-x86_64.tar.gz'
    require(verify_sums(directory, native + '.sha256') == {native}, 'unexpected native checksum assets')
    archive_path = local_file(directory, native)
    # The trusted helper rejects traversal, duplicate members, links/devices,
    # oversized extraction and bad per-file hashes before inspecting its temp tree.
    completed = subprocess.run([sys.executable, '-B', str(manage), 'verify', str(archive_path)],
                               check=True, text=True, capture_output=True)
    with tarfile.open(archive_path, 'r:gz') as archive:
        members = archive.getmembers()
        files = {m.name: m for m in members if m.isfile()}
        manifests = [name for name in files if name.count('/') == 1 and name.endswith('/manifest.json')]
        require(len(manifests) == 1, 'native manifest missing or ambiguous')
        root = manifests[0].split('/')[0]
        manifest = json.load(archive.extractfile(files[manifests[0]]))
        require(manifest.get('commit') == sha, 'native source commit mismatch')
        require(manifest.get('target') == 'x86_64-unknown-linux-gnu', 'native target mismatch')
        required = ('LICENSE', 'NOTICE', 'bin/tokengo', 'bin/tokengo-server', 'web/index.html')
        for name in required:
            require(root + '/' + name in files, 'native package missing ' + name)
        wasm = [m for name, m in files.items() if name.startswith(root + '/web/') and name.endswith('.wasm')]
        javascript = [name for name in files if name.startswith(root + '/web/') and name.endswith('.js')]
        require(wasm and javascript, 'native WASM or JS missing')
        for member in wasm:
            with archive.extractfile(member) as stream:
                require(stream.read(4) == b'\0asm', 'invalid WASM magic')
    image_archive = f'tokengo-{sha}-linux-amd64.tar.gz'
    image_names = verify_sums(directory, 'image-SHA256SUMS')
    required_image = {image_archive, 'compose.yaml', 'container.env.example',
                      'container-deployment.md', 'image-manifest.json', 'registry-image.txt'}
    require(required_image <= image_names, 'image checksum manifest omits required release assets')
    image = json.loads(local_file(directory, 'image-manifest.json').read_text())
    require(image.get('source_commit') == sha, 'image source commit mismatch')
    require(image.get('architecture') == 'amd64' and image.get('os') == 'linux', 'image platform mismatch')
    require(re.fullmatch(r'sha256:[0-9a-f]{64}', image.get('image_id', '')), 'invalid local image identity')
    registry = local_file(directory, 'registry-image.txt').read_text().strip()
    require(re.fullmatch(r'ghcr\.io/qiwolf/tokengo@sha256:[0-9a-f]{64}', registry), 'invalid registry digest identity')
    return {'result': 'passed', 'source_commit': sha, 'platform': 'linux/amd64',
            'native_archive': native, 'native_sha256': digest(archive_path),
            'image_archive': image_archive, 'image_sha256': digest(local_file(directory, image_archive)),
            'registry_image': registry, 'native_helper_result': completed.stdout.strip(),
            'checks': ['native-archive-sha256', 'native-per-file-checksums', 'source-manifest',
                       'server-cli-license-notice', 'wasm-magic-and-js', 'image-all-checksums',
                       'image-source-and-platform', 'registry-digest-format'],
            'limits': 'Does not pull or run images; root performs registry and runtime acceptance separately.'}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('assets', type=Path)
    parser.add_argument('--sha', required=True, help='Reviewed complete 40-character release commit')
    parser.add_argument('--manage', type=Path, default=DEFAULT_MANAGE)
    args = parser.parse_args()
    print(json.dumps(verify(args.assets.resolve(), args.sha, args.manage.resolve()), ensure_ascii=False, indent=2))


if __name__ == '__main__':
    main()
