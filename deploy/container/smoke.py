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
    containers = [name]
    volume = name + '-data'
    docker('volume', 'create', volume)

    def start_container(container_name):
        docker('run', '-d', '--name', container_name, '--read-only', '--cap-drop', 'ALL',
               '--security-opt', 'no-new-privileges:true', '--tmpfs', '/tmp:rw,size=64m,mode=1777',
               '--mount', 'type=volume,src=' + volume + ',dst=/data',
               '-p', '127.0.0.1::3160', image)
        info = json.loads(docker('inspect', container_name))[0]
        port = info['NetworkSettings']['Ports']['3160/tcp'][0]['HostPort']
        return info, 'http://127.0.0.1:' + port

    try:
        info, base = start_container(name)

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
        # CLI has version/routing subcommands; it does not implement --help.
        # version is local and must not contact a server or an upstream.
        assert docker('exec', name, '/opt/tokengo/bin/tokengo', 'version').startswith('tokengo ')
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
        db_inode = docker('exec', name, 'stat', '-c', '%i', '/data/tokengo.db')
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
        # Recreate the container, keeping only its volume. Restart alone could
        # accidentally pass if state were stored in the container filesystem.
        docker('rm', name)
        name += '-recreated'
        containers.append(name)
        recreated, base = start_container(name)
        assert recreated['Id'] != info['Id']
        assert recreated['Image'] == info['Image']
        ready()
        assert json.loads(request('/api/auth/status'))['needs_setup'] is False
        assert docker('exec', name, 'stat', '-c', '%i', '/data/tokengo.db') == db_inode
        assert docker('exec', name, 'sha256sum', '/data/master.key').split()[0] == key_before
        assert docker('exec', name, 'stat', '-c', '%a:%u', '/data/master.key') == '600:10001'
        docker('exec', name, '/usr/local/bin/tokengo-healthcheck')
        docker('stop', '-t', '150', name)
        assert json.loads(docker('inspect', name))[0]['State']['ExitCode'] == 0
        print(json.dumps({'image': image, 'image_id': info['Image'], 'result': 'passed',
                          'checks': ['nonroot', 'read-only-root', 'readyz-json', 'wasm-served',
                                     'admin-auth', 'fresh-setup', 'persistent-db-and-master-key',
                                     'key-permissions', 'cli-version', 'graceful-stop-and-restart',
                                     'container-recreation-with-existing-volume']}, indent=2))
    finally:
        for container_name in containers:
            subprocess.run(['docker', 'rm', '-f', container_name], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        subprocess.run(['docker', 'volume', 'rm', volume], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


if __name__ == '__main__':
    main()
