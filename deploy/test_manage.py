#!/usr/bin/env python3
"""Filesystem/HTTP acceptance; no real systemd, root writes, or provider calls."""
import contextlib
import io
import json
import os
from pathlib import Path
import sqlite3
import tarfile
import tempfile
import threading
import types
import unittest
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from unittest.mock import patch
import manage

SOURCE = Path(__file__).resolve().parent

def checksums(root):
    values = {str(p.relative_to(root)): manage.sha(p) for p in root.rglob('*') if p.is_file() and p.name != 'checksums.json'}
    (root / 'checksums.json').write_text(json.dumps(values))

def release(root, commit='a' * 40):
    root.mkdir(parents=True)
    (root / 'bin').mkdir()
    elf = bytearray(20)
    elf[:6] = b'\x7fELF\x02\x01'
    elf[18:20] = b'\x3e\x00'
    for name in ['tokengo-server', 'tokengo']:
        (root / 'bin' / name).write_bytes(elf)
    (root / 'web').mkdir()
    for name, data in [('index.html', b'<html>same commit</html>'), ('app.js', b'load_wasm()'), ('app_bg.wasm', b'\0asm\1\0\0\0')]:
        (root / 'web' / name).write_bytes(data)
    (root / 'deploy').mkdir()
    for name in ['tokengo.env.example', 'tokengo.service']:
        (root / 'deploy' / name).write_bytes((SOURCE / name).read_bytes())
    (root / 'manifest.json').write_text(json.dumps({'commit': commit, 'target': 'x86_64-unknown-linux-gnu'}))
    checksums(root)
    return root

def bundle(root, path):
    with tarfile.open(path, 'w:gz') as tar:
        tar.add(root, arcname=root.name)
    return path

class DeploymentTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name).resolve()
        self.constants = patch.multiple(manage, BASE=self.root / 'opt/tokengo', STATE=self.root / 'state', CONFIG=self.root / 'config', BACKUPS=self.root / 'backups', UNIT=self.root / 'unit', LOGS=self.root / 'logs')
        self.constants.start()
        self.addCleanup(self.constants.stop)
        self.addCleanup(self.temp.cleanup)

    def initialize(self):
        manage.STATE.mkdir()
        manage.CONFIG.mkdir()
        conn = sqlite3.connect(manage.STATE / 'tokengo.db')
        conn.execute('create table ledger (cost integer)')
        conn.execute('insert into ledger values (19)')
        conn.commit()
        conn.close()
        (manage.STATE / 'master.key').write_text('test-fixture-not-a-provider-key')
        (manage.STATE / 'master.key').chmod(0o600)
        outbox = manage.STATE / 'tokengo.db.billing-outbox'
        outbox.mkdir()
        (outbox / 'known-intent.json').write_text('{"input":7,"output":3}')
        old = release(manage.BASE / 'releases' / ('a' * 40))
        manage.set_current(old)
        env = (SOURCE / 'tokengo.env.example').read_text().replace('/var/lib/tokengo', str(manage.STATE)).replace('/opt/tokengo', str(manage.BASE)).replace('/var/log/tokengo', str(manage.LOGS))
        (manage.CONFIG / 'tokengo.env').write_text(env)
        manage.UNIT.write_text('old-unit')
        return old

    def test_initial_install_keeps_state_private_and_release_readable_by_service(self):
        tree = release(self.root / 'new-release', 'b' * 40)
        archive = bundle(tree, self.root / 'install.tar.gz')
        owner = types.SimpleNamespace(pw_uid=os.getuid(), pw_gid=os.getgid())
        old_mask = os.umask(0o077)
        try:
            with patch.object(manage.pwd, 'getpwnam', return_value=owner), patch.object(manage.shutil, 'chown'), patch.object(manage, 'systemctl'), patch.object(manage, 'await_health'), contextlib.redirect_stdout(io.StringIO()):
                manage.install_or_upgrade(archive, False)
        finally:
            os.umask(old_mask)
        current = (manage.BASE / 'current').resolve()
        self.assertEqual(manage.BASE.stat().st_mode & 0o777, 0o755)
        self.assertEqual(current.stat().st_mode & 0o777, 0o755)
        self.assertEqual((current / 'bin/tokengo-server').stat().st_mode & 0o777, 0o755)
        self.assertEqual(manage.STATE.stat().st_mode & 0o777, 0o700)
        self.assertEqual((manage.CONFIG / 'tokengo.env').stat().st_mode & 0o777, 0o600)
        # Fixed-path config is parsed as data, never sourced as shell code.
        env = manage.CONFIG / 'tokengo.env'
        env.write_text(env.read_text().replace('/var/lib/tokengo', str(manage.STATE)).replace('/opt/tokengo', str(manage.BASE)).replace('/var/log/tokengo', str(manage.LOGS)))
        self.assertEqual(manage.validate_config(), 'http://127.0.0.1:3160')

    def test_bundle_rejects_tamper_and_macos_binary(self):
        tree = release(self.root / 'release')
        path = bundle(tree, self.root / 'ok.tar.gz')
        manage.verify_bundle(path, self.root / 'unpack-ok')
        (tree / 'web/app.js').write_text('tampered')
        with self.assertRaises(ValueError):
            manage.verify_bundle(bundle(tree, self.root / 'bad.tar.gz'), self.root / 'unpack-bad')
        (tree / 'bin/tokengo-server').write_bytes(b'\xcf\xfa\xed\xfe' + bytes(20))
        checksums(tree)
        with self.assertRaisesRegex(ValueError, 'ELF'):
            manage.verify_bundle(bundle(tree, self.root / 'mac.tar.gz'), self.root / 'unpack-mac')

    def test_bundle_rejects_traversal_symlink_and_unhashed_file(self):
        for name, kind in [('../escape', tarfile.REGTYPE), ('release/link', tarfile.SYMTYPE)]:
            archive = self.root / ('bad-' + str(kind) + '.tar.gz')
            with tarfile.open(archive, 'w:gz') as tar:
                member = tarfile.TarInfo(name)
                member.type = kind
                member.linkname = '/etc/passwd'
                tar.addfile(member, io.BytesIO(b''))
            with self.assertRaises(ValueError):
                manage.verify_bundle(archive, self.root / 'unpack')
        tree = release(self.root / 'release')
        (tree / 'web/checksums.json').write_text('unhashed')
        with self.assertRaisesRegex(ValueError, 'file set'):
            manage.verify_files(tree)

    def test_snapshot_rewinds_ledger_master_and_pending_intent_together(self):
        old = self.initialize()
        with patch.object(manage, 'validate_config', return_value='http://127.0.0.1:3160'):
            snapshot = manage.make_snapshot()
        conn = sqlite3.connect(manage.STATE / 'tokengo.db')
        conn.execute('update ledger set cost=99')
        conn.commit()
        conn.close()
        (manage.STATE / 'master.key').write_text('different-fixture')
        (manage.STATE / 'tokengo.db.billing-outbox/known-intent.json').unlink()
        owner = types.SimpleNamespace(pw_uid=os.getuid(), pw_gid=os.getgid())
        with patch.object(manage, 'systemctl'), patch.object(manage.pwd, 'getpwnam', return_value=owner):
            failed = manage.activate_snapshot(snapshot)
        self.assertTrue(failed.exists())
        self.assertEqual((manage.BASE / 'current').resolve(), old)
        conn = sqlite3.connect(manage.STATE / 'tokengo.db')
        self.assertEqual(conn.execute('select cost from ledger').fetchone()[0], 19)
        conn.close()
        self.assertEqual((manage.STATE / 'master.key').read_text(), 'test-fixture-not-a-provider-key')
        self.assertTrue((manage.STATE / 'tokengo.db.billing-outbox/known-intent.json').is_file())
        self.assertEqual((manage.STATE / 'master.key').stat().st_mode & 0o777, 0o600)
        self.assertEqual(manage.STATE.stat().st_uid, os.getuid())

    def test_upgrade_failed_health_automatically_restores_previous_release(self):
        old = self.initialize()
        tree = release(self.root / 'new-release', 'b' * 40)
        archive = bundle(tree, self.root / 'upgrade.tar.gz')
        owner = types.SimpleNamespace(pw_uid=os.getuid(), pw_gid=os.getgid())
        calls = []
        with patch.object(manage, 'validate_config', return_value='http://127.0.0.1:3160'), patch.object(manage, 'stop'), patch.object(manage, 'systemctl', side_effect=lambda *a: calls.append(a)), patch.object(manage, 'await_health', side_effect=[ValueError('bad new release'), None]), patch.object(manage.pwd, 'getpwnam', return_value=owner), contextlib.redirect_stdout(io.StringIO()):
            with self.assertRaisesRegex(ValueError, 'bad new release'):
                manage.install_or_upgrade(archive, True)
        self.assertEqual((manage.BASE / 'current').resolve(), old)
        self.assertEqual(manage.UNIT.read_text(), 'old-unit')
        self.assertEqual(calls.count(('start', manage.SERVICE)), 2)
        self.assertEqual(len(list(manage.BACKUPS.iterdir())), 1)

    def test_readiness_checks_status_and_every_real_panel_asset(self):
        self.initialize()
        corrupt = [False]
        web = manage.BASE / 'current/web'
        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *args):
                pass
            def do_GET(self):
                if self.path == '/readyz':
                    body = b'{"status":"ok","checks":{"database":"ok"}}'
                elif corrupt[0] and self.path.endswith('.wasm'):
                    body = b'<html>SPA fallback</html>'
                else:
                    body = (web / self.path.removeprefix('/ui/')).read_bytes()
                self.send_response(200)
                self.end_headers()
                self.wfile.write(body)
        server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            with patch.object(manage, 'validate_config', return_value=f'http://127.0.0.1:{server.server_port}'):
                manage.health()
                corrupt[0] = True
                with self.assertRaisesRegex(ValueError, 'asset differs'):
                    manage.health()
        finally:
            server.shutdown()
            server.server_close()
            thread.join()

if __name__ == '__main__':
    unittest.main()
