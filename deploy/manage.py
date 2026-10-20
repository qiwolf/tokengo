#!/usr/bin/env python3
"""Offline release/backup validation and explicit local systemd lifecycle commands.
No SSH, proxy configuration, DNS changes, or production traffic switching.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import pwd
import shutil
import sqlite3
import subprocess
import tarfile
import tempfile
import time
import urllib.request

BASE = Path('/opt/tokengo')
STATE = Path('/var/lib/tokengo')
CONFIG = Path('/etc/tokengo')
LOGS = Path('/var/log/tokengo')
BACKUPS = Path('/var/backups/tokengo')
UNIT = Path('/etc/systemd/system/tokengo.service')
SERVICE = 'tokengo.service'

def sha(path):
    h = hashlib.sha256()
    with Path(path).open('rb') as f:
        for chunk in iter(lambda: f.read(1024 * 1024), b''):
            h.update(chunk)
    return h.hexdigest()

def safe_name(name):
    p = PurePosixPath(name)
    if not name or p.is_absolute() or any(v in ('', '.', '..') for v in name.split('/')):
        raise ValueError('unsafe archive or checksum path')
    return p

def verify_files(root):
    expected = json.loads((root / 'checksums.json').read_text())
    if not isinstance(expected, dict) or not expected:
        raise ValueError('empty checksums')
    actual = {str(p.relative_to(root)) for p in root.rglob('*') if p.is_file() and p.relative_to(root).as_posix() != 'checksums.json'}
    if actual != set(expected):
        raise ValueError('file set differs from checksum manifest')
    for name, digest in expected.items():
        safe_name(name)
        path = root / name
        if path.is_symlink() or not isinstance(digest, str) or sha(path) != digest:
            raise ValueError('checksum mismatch or symlink')
    if any(p.is_symlink() for p in root.rglob('*')):
        raise ValueError('symlink in verified tree')

def verify_bundle(archive, dest):
    # Validate every member before extraction; reject links, devices, duplicates,
    # traversal and oversized expansion. tar extraction gets no untrusted paths.
    with tarfile.open(archive, 'r:gz') as tar:
        members = tar.getmembers()
        names = set()
        roots = set()
        size = 0
        for m in members:
            p = safe_name(m.name.rstrip('/'))
            roots.add(p.parts[0])
            if str(p) in names or m.size < 0 or not (m.isfile() or m.isdir()):
                raise ValueError('duplicate or nonregular archive member')
            names.add(str(p))
            size += m.size
        if len(roots) != 1 or size > 1024 * 1024 * 1024 or len(members) > 20000:
            raise ValueError('invalid bundle root/size/member count')
        for m in members:
            target = dest / m.name
            if m.isdir():
                target.mkdir(parents=True, exist_ok=True)
            else:
                target.parent.mkdir(parents=True, exist_ok=True)
                with tar.extractfile(m) as source, target.open('xb') as output:
                    shutil.copyfileobj(source, output)
                target.chmod(0o755 if '/bin/' in m.name or m.name.endswith('.sh') else 0o644)
        root = dest / roots.pop()
    verify_files(root)
    manifest = json.loads((root / 'manifest.json').read_text())
    commit = manifest.get('commit', '')
    if len(commit) != 40 or any(c not in '0123456789abcdef' for c in commit) or manifest.get('target') != 'x86_64-unknown-linux-gnu':
        raise ValueError('unsupported release identity/target')
    for name in ['tokengo-server', 'tokengo']:
        with (root / 'bin' / name).open('rb') as f:
            header = f.read(20)
        if header[:6] != b'\x7fELF\x02\x01' or header[18:20] != b'\x3e\x00':
            raise ValueError('not a Linux x86_64 ELF64 release')
    if not (root / 'web/index.html').is_file() or not list((root / 'web').glob('*.wasm')):
        raise ValueError('missing panel')
    return root, commit

def sync_tree(root):
    for p in root.rglob('*'):
        if p.is_file():
            with p.open('rb') as f:
                os.fsync(f.fileno())
    for p in [*sorted((p for p in root.rglob('*') if p.is_dir()), reverse=True), root, root.parent]:
        fd = os.open(p, os.O_RDONLY | os.O_DIRECTORY)
        try:
            os.fsync(fd)
        finally:
            os.close(fd)

def systemctl(*args):
    subprocess.run(['systemctl', *args], check=True)

def stop():
    systemctl('stop', SERVICE)
    result = subprocess.run(['systemctl', 'is-active', '--quiet', SERVICE], check=False)
    if result.returncode == 0:
        raise ValueError('service still active')

def validate_config(config=None):
    config = CONFIG if config is None else config
    values = {}
    for raw in (config / 'tokengo.env').read_text().splitlines():
        line = raw.strip()
        if not line or line.startswith('#'):
            continue
        key, sep, value = line.partition('=')
        if not sep or key in values:
            raise ValueError('invalid/duplicate environment line')
        values[key] = value
    prescribed = {'TOKENGO_DB': str(STATE / 'tokengo.db'), 'TOKENGO_MASTER_KEY_FILE': str(STATE / 'master.key'),
                  'TOKENGO_WEB_DIR': str(BASE / 'current/web'), 'TOKENGO_LOG_DIR': str(LOGS)}
    if any(values.get(k) != v for k, v in prescribed.items()) or 'TOKENGO_MASTER_KEY' in values:
        raise ValueError('state/master/web paths differ from managed backup contract')
    listen = values.get('TOKENGO_LISTEN', '')
    host, sep, port = listen.rpartition(':')
    # Automation uses loopback readiness; explicit private-address changes require
    # adapting the reviewable probe, never silently opening 0.0.0.0 here.
    if not sep or host != '127.0.0.1' or not port.isdigit() or not 1 <= int(port) <= 65535:
        raise ValueError('managed deployment requires 127.0.0.1:<port>')
    return f'http://127.0.0.1:{port}'

def check_database(path):
    if not path.is_file():
        raise ValueError('database missing')
    conn = sqlite3.connect(f'{path.as_uri()}?mode=ro', uri=True)
    try:
        if conn.execute('PRAGMA integrity_check').fetchall() != [('ok',)]:
            raise ValueError('SQLite integrity_check failed')
    finally:
        conn.close()

def make_snapshot():
    # Caller must stop/quiesce first. Whole directory keeps DB/WAL/SHM/master and
    # database-bound billing outbox/dead-letter files at one stopped point.
    validate_config()
    check_database(STATE / 'tokengo.db')
    if not (STATE / 'master.key').is_file():
        raise ValueError('master key missing')
    target = (BASE / 'current').resolve(strict=True)
    if target.parent != (BASE / 'releases').resolve():
        raise ValueError('current is outside managed releases')
    BACKUPS.mkdir(parents=True, exist_ok=True, mode=0o700)
    snapshot = BACKUPS / (time.strftime('%Y%m%dT%H%M%SZ', time.gmtime()) + '-' + os.urandom(4).hex())
    snapshot.mkdir(mode=0o700)
    if any(p.is_symlink() or not (p.is_file() or p.is_dir()) for p in STATE.rglob('*')):
        raise ValueError('state contains links or nonregular files')
    shutil.copytree(STATE, snapshot / 'state', symlinks=False)
    shutil.copytree(CONFIG, snapshot / 'config', symlinks=False)
    shutil.copy2(UNIT, snapshot / 'tokengo.service')
    (snapshot / 'snapshot.json').write_text(json.dumps({'release': target.name, 'format': 1}) + '\n')
    hashes = {str(p.relative_to(snapshot)): sha(p) for p in snapshot.rglob('*') if p.is_file()}
    (snapshot / 'checksums.json').write_text(json.dumps(hashes, sort_keys=True) + '\n')
    sync_tree(snapshot)
    return snapshot

def activate_snapshot(snapshot):
    verify_files(snapshot)
    metadata = json.loads((snapshot / 'snapshot.json').read_text())
    release_name = metadata.get('release', '')
    safe_name(release_name)
    if '/' in release_name or metadata.get('format') != 1:
        raise ValueError('invalid snapshot identity')
    release = BASE / 'releases' / release_name
    verify_files(release)
    check_database(snapshot / 'state/tokengo.db')
    # Copy into a sibling before replacing; preserve failed state for forensics.
    replacement = STATE.with_name('tokengo.restore-' + os.urandom(4).hex())
    shutil.copytree(snapshot / 'state', replacement)
    owner = pwd.getpwnam('tokengo')
    for p in [replacement, *replacement.rglob('*')]:
        os.chown(p, owner.pw_uid, owner.pw_gid)
    sync_tree(replacement)
    previous = STATE.with_name('tokengo.failed-' + os.urandom(4).hex())
    STATE.rename(previous)
    replacement.rename(STATE)
    shutil.copytree(snapshot / 'config', CONFIG, dirs_exist_ok=True)
    shutil.copy2(snapshot / 'tokengo.service', UNIT)
    set_current(release)
    systemctl('daemon-reload')
    return previous

def set_current(release):
    link = BASE / ('current.next-' + os.urandom(4).hex())
    link.symlink_to(release)
    link.replace(BASE / 'current')

def health():
    base = validate_config()
    with urllib.request.urlopen(base + '/readyz', timeout=2) as response:
        status = json.load(response)
    if status.get('status') != 'ok' or status.get('checks', {}).get('database') != 'ok':
        raise ValueError('database readiness failed')
    web = BASE / 'current/web'
    # Read back every generated asset, not just an HTML 200 or SPA fallback.
    for p in web.rglob('*'):
        if not p.is_file():
            continue
        url = base + '/ui/' + str(p.relative_to(web))
        with urllib.request.urlopen(url, timeout=3) as response:
            body = response.read(64 * 1024 * 1024 + 1)
        if hashlib.sha256(body).hexdigest() != sha(p):
            raise ValueError('served panel asset differs from release')

def await_health():
    for attempt in range(20):
        try:
            health()
            return
        except Exception:
            if attempt == 19:
                raise
            time.sleep(1)

def install_or_upgrade(archive, upgrade):
    exists = (BASE / 'current').is_symlink()
    if exists != upgrade or (not upgrade and STATE.exists() and any(STATE.iterdir())):
        raise ValueError('use install only on empty state; upgrade requires current release')
    with tempfile.TemporaryDirectory() as tmp:
        tree, commit = verify_bundle(archive, Path(tmp))
        release = BASE / 'releases' / commit
        if release.exists():
            raise ValueError('release already installed; use explicit restore for rollback')
        if upgrade:
            validate_config()
            stop()
            try:
                snapshot = make_snapshot()
            except Exception:
                systemctl('start', SERVICE)
                await_health()
                raise
            print(f'Stopped backup: {snapshot}')
        else:
            try:
                pwd.getpwnam('tokengo')
            except KeyError:
                subprocess.run(['useradd', '--system', '--home-dir', str(STATE), '--shell', '/usr/sbin/nologin', 'tokengo'], check=True)
            for directory in [STATE, LOGS]:
                directory.mkdir(parents=True, exist_ok=True, mode=0o700)
                shutil.chown(directory, user='tokengo', group='tokengo')
            CONFIG.mkdir(parents=True, exist_ok=True, mode=0o700)
            shutil.copy2(tree / 'deploy/tokengo.env.example', CONFIG / 'tokengo.env')
            (CONFIG / 'tokengo.env').chmod(0o600)
        try:
            release.parent.mkdir(parents=True, exist_ok=True)
            BASE.chmod(0o755)
            release.parent.chmod(0o755)
            shutil.copytree(tree, release)
            for p in [release, *release.rglob('*')]:
                p.chmod(0o755 if p.is_dir() or p.parent.name == 'bin' or p.suffix == '.sh' else 0o644)
            shutil.copy2(tree / 'deploy/tokengo.service', UNIT)
            UNIT.chmod(0o644)
            set_current(release)
            systemctl('daemon-reload')
            systemctl('enable', SERVICE)
            systemctl('start', SERVICE)
            await_health()
        except Exception:
            stop()
            if upgrade:
                failed = activate_snapshot(snapshot)
                systemctl('start', SERVICE)
                await_health()
                print(f'Upgrade failed; restored backup. Failed state retained: {failed}')
            raise
        print(f'Local readiness and all panel assets passed: {commit}')

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('command', choices=['verify', 'install', 'upgrade', 'backup', 'restore', 'health'])
    parser.add_argument('path', nargs='?', type=Path)
    args = parser.parse_args()
    if args.command == 'verify':
        if args.path is None:
            parser.error('archive required')
        with tempfile.TemporaryDirectory() as temp:
            _, commit = verify_bundle(args.path, Path(temp))
            print(f'Bundle verified: {commit} x86_64-unknown-linux-gnu')
        return
    if os.geteuid() != 0 or os.uname().sysname != 'Linux':
        parser.error('lifecycle commands require root on the target Linux systemd host')
    os.umask(0o077)
    if args.command in ('install', 'upgrade'):
        if args.path is None:
            parser.error('archive required')
        install_or_upgrade(args.path, args.command == 'upgrade')
    elif args.command == 'backup':
        stop()
        print(make_snapshot())
        print('Service remains stopped. Start explicitly after copying backup off-host.')
    elif args.command == 'restore':
        if args.path is None:
            parser.error('snapshot directory required')
        stop()
        # Validate first, preserve the present stopped state, then explicitly rewind.
        verify_files(args.path)
        print(f'Pre-restore backup: {make_snapshot()}')
        print(f'Replaced state preserved: {activate_snapshot(args.path)}')
        systemctl('start', SERVICE)
        await_health()
    elif args.command == 'health':
        health()
        print('Local database readiness and every panel asset passed')

if __name__ == '__main__':
    main()
