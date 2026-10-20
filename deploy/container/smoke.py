#!/usr/bin/env python3
"""Exercise an isolated image with a disposable volume and synthetic credentials."""
import json
import re
import secrets
import subprocess
import sys
import time
import urllib.error
import urllib.request
import uuid


def docker(*args):
    return subprocess.check_output(['docker', *args], text=True).strip()


def main():
    image = sys.argv[1] if len(sys.argv) > 1 else 'tokengo:local'
    name = 'tokengo-smoke-' + uuid.uuid4().hex[:12]
    volume = name + '-data'
    docker('volume', 'create', volume)
    try:
        docker('run', '-d', '--name', name, '--read-only', '--cap-drop', 'ALL',
               '--security-opt', 'no-new-privileges:true', '--tmpfs', '/tmp:rw,size=64m,mode=1777',
               '--mount', 'type=volume,src=' + volume + ',dst=/data',
               '-p', '127.0.0.1::3160', image)
        info = json.loads(docker('inspect', name))[0]
        port = info['NetworkSettings']['Ports']['3160/tcp'][0]['HostPort']
        base = 'http://127.0.0.1:' + port

        def request(path, body=None):
            data = None if body is None else json.dumps(body).encode()
            req = urllib.request.Request(base + path, data=data,
                                         headers={'Content-Type': 'application/json'})
            with urllib.request.urlopen(req, timeout=5) as response:
                return response.read()

        def ready():
            for _ in range(90):
                try:
                    if json.loads(request('/readyz'))['status'] == 'ok':
                        return
                except (OSError, ValueError):
                    pass
                time.sleep(1)
            raise AssertionError('container did not become ready')

        ready()
        assert docker('exec', name, 'id', '-u') == '10001'
        assert info['HostConfig']['ReadonlyRootfs'] is True
        docker('exec', name, '/usr/local/bin/tokengo-healthcheck')
        assert json.loads(request('/api/auth/status'))['needs_setup'] is True
        html = request('/ui/').decode()
        wasm_paths = re.findall(r'[\"\'](/ui/[\w.-]+\.wasm)[\"\']', html)
        assert wasm_paths, 'panel must contain an actual WASM asset'
        for asset in set(wasm_paths):
            path = asset if asset.startswith('/') else '/ui/' + asset
            assert request(path)[:4] == b'\0asm', path
        try:
            request('/api/channels')
            raise AssertionError('unauthenticated admin endpoint was accepted')
        except urllib.error.HTTPError as error:
            assert error.code == 401
        # The response contains a disposable Gateway Key; never print or persist it.
        request('/api/auth/setup', {'password': secrets.token_urlsafe(32)})
        assert json.loads(request('/api/auth/status'))['needs_setup'] is False
        key_before = docker('exec', name, 'sha256sum', '/data/master.key').split()[0]
        assert docker('exec', name, 'stat', '-c', '%a:%u', '/data/master.key') == '600:10001'
        docker('stop', '-t', '150', name)
        exit_code = json.loads(docker('inspect', name))[0]['State']['ExitCode']
        assert exit_code == 0, 'expected graceful exit, got ' + str(exit_code)
        docker('start', name)
        # Docker may allocate a new host port when a dynamic binding restarts.
        restarted = json.loads(docker('inspect', name))[0]
        port = restarted['NetworkSettings']['Ports']['3160/tcp'][0]['HostPort']
        base = 'http://127.0.0.1:' + port
        ready()
        assert json.loads(request('/api/auth/status'))['needs_setup'] is False
        key_after = docker('exec', name, 'sha256sum', '/data/master.key').split()[0]
        assert key_before == key_after, 'master key changed across restart'
        assert docker('exec', name, 'test', '-f', '/data/tokengo.db') == ''
        docker('exec', name, '/usr/local/bin/tokengo-healthcheck')
        docker('stop', '-t', '150', name)
        assert json.loads(docker('inspect', name))[0]['State']['ExitCode'] == 0
        print(json.dumps({'image': image, 'image_id': info['Image'], 'result': 'passed',
                          'checks': ['nonroot', 'read-only-root', 'readyz-json', 'wasm-served',
                                     'admin-auth', 'fresh-setup', 'persistent-db-and-master-key',
                                     'key-permissions', 'graceful-stop-and-restart']}, indent=2))
    finally:
        subprocess.run(['docker', 'rm', '-f', name], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        subprocess.run(['docker', 'volume', 'rm', volume], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


if __name__ == '__main__':
    main()
