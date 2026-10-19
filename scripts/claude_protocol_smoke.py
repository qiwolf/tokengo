#!/usr/bin/env python3
"""Real isolated Claude Code -> real gateway -> loopback Messages fixture.
Only a fixed harmless printf tool is allowed. No real provider or user auth.
Requires macOS sandbox-exec to deny non-loopback client/server networking.
Evidence includes assertions/counts only, never raw prompts/config/client output.
"""
import argparse
import contextlib
import http.server
import json
import os
from pathlib import Path
import shutil
import socket
import sqlite3
import subprocess
import tempfile
import threading
import time
import urllib.error
import urllib.request

POLICY = '''(version 1)(allow default)(deny network*)
(allow network-outbound (remote ip "localhost:*"))
(allow network-bind (local ip "localhost:*"))
(allow network-inbound (local ip "localhost:*"))
(deny mach-lookup (global-name "com.apple.securityd"))'''
TOOL_ID = 'toolu_controlled_smoke_1'
SIGNATURE = 'controlled-opaque-signature-not-a-real-provider-signature'


def free_port():
    with socket.socket() as sock:
        sock.bind(('127.0.0.1', 0))
        return sock.getsockname()[1]


def run(args, evidence):
    sandbox = shutil.which('sandbox-exec')
    if not sandbox:
        raise RuntimeError('required_loopback_network_sandbox_unavailable')
    prefix = [sandbox, '-p', POLICY]
    with tempfile.TemporaryDirectory(prefix='tokengo-claude-') as folder:
        tmp = Path(folder)
        work = tmp / 'work'
        home = tmp / 'home'
        config = tmp / 'claude'
        for directory in [work, home, config]:
            directory.mkdir(mode=0o700)
        # A whitelist avoids inheriting provider tokens, proxy credentials or
        # auth helpers. --bare additionally promises no OAuth/keychain reads.
        env = {'PATH': os.environ.get('PATH', '/usr/bin:/bin'), 'HOME': str(home),
               'TMPDIR': str(tmp), 'CLAUDE_CONFIG_DIR': str(config),
               'DISABLE_AUTOUPDATER': '1', 'DISABLE_TELEMETRY': '1',
               'DISABLE_ERROR_REPORTING': '1', 'CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC': '1',
               'CLAUDE_CODE_SKIP_PROMPT_HISTORY': '1', 'CLAUDE_CODE_DISABLE_TERMINAL_TITLE': '1',
               'LANG': 'en_US.UTF-8'}
        version = subprocess.run(prefix + [args.claude, '--version'], env=env, cwd=work,
                                 capture_output=True, text=True, timeout=15)
        if version.returncode:
            raise RuntimeError('isolated_client_version_failed')
        evidence['client_version'] = version.stdout.strip()
        evidence['network_isolation'] = 'macOS_OS_sandbox_loopback_only'
        evidence['oauth_keychain'] = 'disabled_by_bare_and_securityd_denial'
        failures = []
        max_rounds = 5

        class Source(http.server.BaseHTTPRequestHandler):
            def log_message(self, *_):
                pass

            def do_POST(self):
                try:
                    length = int(self.headers.get('Content-Length', '0'))
                    if not 0 <= length <= 8 * 1024 * 1024:
                        raise ValueError('source_body_limit')
                    body = json.loads(self.rfile.read(length))
                    if self.path.split('?')[0] != '/v1/messages':
                        failures.append('unexpected_source_path')
                        self.send_error(404)
                        return
                    evidence['rounds'] += 1
                    if evidence['rounds'] > max_rounds:
                        failures.append('excessive_source_rounds')
                        self.send_error(400)
                        return
                    evidence['native_stream_requested'] = body.get('stream') is True
                    evidence['source_credential_is_test_only'] = self.headers.get('x-api-key') == 'TEST-SOURCE-ONLY'
                    evidence['inbound_vkey_not_forwarded'] = self.headers.get('x-api-key') != key
                    messages = body.get('messages', [])
                    results = [block for message in messages if message.get('role') == 'user'
                               for block in message.get('content', []) if isinstance(block, dict) and block.get('type') == 'tool_result']
                    prior = [block for message in messages if message.get('role') == 'assistant'
                             for block in message.get('content', []) if isinstance(block, dict)]
                    usage = {'input_tokens': 11, 'cache_creation_input_tokens': 0,
                             'cache_read_input_tokens': 0, 'output_tokens': 0}
                    start = {'type': 'message_start', 'message': {'id': f'msg_controlled_{evidence["rounds"]}',
                             'type': 'message', 'role': 'assistant', 'model': 'native-source', 'content': [],
                             'stop_reason': None, 'stop_sequence': None, 'usage': usage}}
                    events = [start]
                    if not results:
                        if not any(t.get('name') == 'Bash' for t in body.get('tools', [])):
                            failures.append('client_bash_tool_missing')
                            self.send_error(400)
                            return
                        index = 0
                        if args.signature:
                            events.extend([
                                {'type': 'content_block_start', 'index': 0, 'content_block': {'type': 'thinking', 'thinking': ''}},
                                {'type': 'content_block_delta', 'index': 0, 'delta': {'type': 'thinking_delta', 'thinking': 'Controlled fixture.'}},
                                {'type': 'content_block_delta', 'index': 0, 'delta': {'type': 'signature_delta', 'signature': SIGNATURE}},
                                {'type': 'content_block_stop', 'index': 0}])
                            index = 1
                        events.extend([
                            {'type': 'content_block_start', 'index': index, 'content_block': {'type': 'tool_use', 'id': TOOL_ID, 'name': 'Bash', 'input': {}}},
                            {'type': 'content_block_delta', 'index': index, 'delta': {'type': 'input_json_delta', 'partial_json': json.dumps({'command': 'printf TOKENGO_TOOL_OK', 'description': 'Controlled harmless marker'})}},
                            {'type': 'content_block_stop', 'index': index},
                            {'type': 'message_delta', 'delta': {'stop_reason': 'tool_use', 'stop_sequence': None}, 'usage': {'output_tokens': 6}},
                            {'type': 'message_stop'}])
                    else:
                        evidence['tool_result_seen'] = any(r.get('tool_use_id') == TOOL_ID and not r.get('is_error', False) and 'TOKENGO_TOOL_OK' in json.dumps(r.get('content', '')) for r in results)
                        evidence['tool_id_roundtrip'] = any(b.get('type') == 'tool_use' and b.get('id') == TOOL_ID and b.get('name') == 'Bash' for b in prior)
                        if args.signature:
                            evidence['signature_roundtrip'] = any(b.get('type') == 'thinking' and b.get('signature') == SIGNATURE for b in prior)
                        events.extend([
                            {'type': 'content_block_start', 'index': 0, 'content_block': {'type': 'text', 'text': ''}},
                            {'type': 'content_block_delta', 'index': 0, 'delta': {'type': 'text_delta', 'text': 'TOKENGO_CLIENT_MULTITURN_OK'}},
                            {'type': 'content_block_stop', 'index': 0},
                            {'type': 'message_delta', 'delta': {'stop_reason': 'end_turn', 'stop_sequence': None}, 'usage': {'output_tokens': 4}},
                            {'type': 'message_stop'}])
                    wire = b''.join(('event: ' + v['type'] + '\ndata: ' + json.dumps(v) + '\n\n').encode() for v in events)
                    self.send_response(200)
                    self.send_header('Content-Type', 'text/event-stream')
                    self.send_header('Content-Length', str(len(wire)))
                    self.end_headers()
                    # Fragment actual HTTP writes through both the gateway and CLI.
                    for offset in range(0, len(wire), 73):
                        self.wfile.write(wire[offset:offset + 73])
                        self.wfile.flush()
                        time.sleep(.001)
                except (BrokenPipeError, ConnectionResetError):
                    failures.append('source_connection_closed')
                except Exception:
                    failures.append('source_fixture_exception')
                    self.send_error(500)

        source = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Source)
        source_thread = threading.Thread(target=source.serve_forever, daemon=True)
        source_thread.start()
        port = free_port()
        base = f'http://127.0.0.1:{port}'
        db = tmp / 'test.db'
        key = 'tk-CONTROLLED-CLAUDE-ONLY-NO-REAL-SECRET'
        server_env = {**env, 'TOKENGO_LISTEN': f'127.0.0.1:{port}', 'TOKENGO_DB': str(db),
                      'TOKENGO_MASTER_KEY_FILE': str(tmp / 'master.key'), 'TOKENGO_LOG_DIR': str(tmp / 'logs')}
        with (tmp / 'gateway.log').open('wb') as logs:
            server = subprocess.Popen(prefix + [args.server], env=server_env, cwd=tmp, stdout=logs, stderr=logs)
            try:
                for _ in range(100):
                    if server.poll() is not None:
                        raise RuntimeError('sandboxed_gateway_exited_before_ready')
                    try:
                        with urllib.request.urlopen(base + '/livez', timeout=.2):
                            break
                    except Exception:
                        time.sleep(.05)
                else:
                    raise RuntimeError('gateway_start_timeout')

                def post(path, value, token=None):
                    headers = {'Content-Type': 'application/json'}
                    if token:
                        headers['Authorization'] = 'Bearer ' + token
                    req = urllib.request.Request(base + path, json.dumps(value).encode(), headers, method='POST')
                    with urllib.request.urlopen(req, timeout=10) as response:
                        return json.load(response)
                setup = post('/api/auth/setup', {'password': 'controlled-smoke-password', 'gateway_key': key})
                channel=post('/api/channels', {'name': 'controlled-messages', 'base_url': f'http://127.0.0.1:{source.server_port}/v1',
                     'key': 'TEST-SOURCE-ONLY', 'protocols': {'native': ['messages'], 'allow_conversion': False},
                     'models': [{'model': 'smoke-native', 'upstream': 'native-source'}]}, setup['token'])
                if args.signature:
                    post('/api/pricing', {'channel_id':channel['id'],'model':'smoke-native',
                         'input_micro_per_million':0,'output_micro_per_million':0,
                         'max_input_tokens':1000000,'max_output_tokens':128000}, setup['token'])
                    evidence['opaque_context_bounds_configured']=True
                conn = sqlite3.connect(db)
                conn.execute('INSERT OR REPLACE INTO settings(key,value) VALUES(?,?)', ('model_routing:smoke-native', json.dumps({'retry_times': 0, 'probe_enabled': False, 'include_in_auto': False})))
                conn.commit()
                conn.close()
                client_env = {**env, 'ANTHROPIC_API_KEY': key, 'ANTHROPIC_BASE_URL': base,
                              'ANTHROPIC_CUSTOM_HEADERS': 'x-claude-code-session-id: controlled-claude-session',
                              'ANTHROPIC_MODEL': 'smoke-native', 'ANTHROPIC_DEFAULT_HAIKU_MODEL': 'smoke-native',
                              'ANTHROPIC_DEFAULT_SONNET_MODEL': 'smoke-native', 'ANTHROPIC_DEFAULT_OPUS_MODEL': 'smoke-native'}
                command = prefix + [args.claude, '--bare', '--print', '--verbose', '--output-format', 'stream-json',
                          '--include-partial-messages', '--no-session-persistence', '--setting-sources', '',
                          '--strict-mcp-config', '--mcp-config', '{"mcpServers":{}}', '--disable-slash-commands', '--no-chrome',
                          '--tools', 'Bash', '--allowedTools', 'Bash(printf TOKENGO_TOOL_OK)', '--model', 'smoke-native',
                          '--system-prompt', 'Execute only the controlled harmless marker tool and report its result.',
                          'Run the requested harmless local tool, then report its result.']
                result = subprocess.run(command, env=client_env, cwd=work, stdin=subprocess.DEVNULL,
                                        stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=70)
                stdout = result.stdout.decode(errors='replace')
                stderr = result.stderr.decode(errors='replace')
                evidence.update(client_exit=result.returncode, final_marker_seen='TOKENGO_CLIENT_MULTITURN_OK' in stdout,
                                partial_stream_event_seen='stream_event' in stdout,
                                diagnostic_categories=[label for label in ('sandbox', 'permission', 'stream', 'error', 'retry', 'authentication') if label in stderr.lower()])
                if result.returncode:
                    terminal=[]
                    for line in stdout.splitlines():
                        try:
                            value=json.loads(line)
                        except ValueError:
                            continue
                        if value.get('type')=='result':
                            terminal.append({'type':value.get('type'),'subtype':value.get('subtype'),'is_error':value.get('is_error'),'errors':value.get('errors',[]),'result':value.get('result','')[:1000]})
                    encoded=json.dumps(terminal)
                    for raw in [str(tmp), str(Path.home()), args.claude, args.server, key, 'Run the requested harmless local tool, then report its result.', 'Execute only the controlled harmless marker tool and report its result.']:
                        encoded=encoded.replace(raw,'[REDACTED]')
                    evidence['terminal_diagnostics']=json.loads(encoded)
                    diagnostic=stderr[:1500]
                    for raw in [str(tmp), str(Path.home()), args.claude, args.server, key]:
                        diagnostic=diagnostic.replace(raw,'[REDACTED]')
                    evidence['client_stderr_diagnostic']=diagnostic
                conn=sqlite3.connect(db)
                evidence['gateway_request_statuses']=[{'status':row[0],'error_code':row[1],'count':row[2]} for row in conn.execute('SELECT status,error_code,count(*) FROM requests GROUP BY status,error_code')]
                conn.close()
                evidence['failures'] = failures
                evidence['passed'] = result.returncode == 0 and evidence['rounds'] == 2 and evidence['tool_result_seen'] and evidence['tool_id_roundtrip'] and evidence['final_marker_seen'] and evidence['native_stream_requested'] and evidence['source_credential_is_test_only'] and evidence['inbound_vkey_not_forwarded'] and not failures and (not args.signature or evidence['signature_roundtrip'])
            finally:
                server.terminate()
                with contextlib.suppress(subprocess.TimeoutExpired):
                    server.wait(timeout=10)
                if server.poll() is None:
                    server.kill()
                    server.wait()
                source.shutdown()
                source.server_close()
                source_thread.join()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--server', required=True)
    parser.add_argument('--claude', required=True)
    parser.add_argument('--output', required=True)
    parser.add_argument('--signature', action='store_true', help='Check native opaque signature preservation, not cryptographic validity')
    args = parser.parse_args()
    args.server = str(Path(args.server).resolve())
    args.claude = str(Path(args.claude).resolve())
    evidence = {'kind': 'controlled_client_test', 'client': 'Claude Code', 'real_provider_tested': False,
                'deployment_tested': False, 'client_cancellation_tested': False, 'rounds': 0,
                'tool_result_seen': False, 'tool_id_roundtrip': False, 'signature_case': args.signature,
                'signature_roundtrip': False, 'passed': False}
    try:
        run(args, evidence)
    except subprocess.TimeoutExpired:
        evidence['blocker'] = 'isolated_client_timeout'
    except Exception as error:
        # Do not serialize exception text: HTTP bodies, local paths or prompts
        # must not enter this evidence artifact.
        evidence['blocker'] = str(error) if isinstance(error, RuntimeError) else type(error).__name__
    Path(args.output).write_text(json.dumps(evidence, indent=2) + '\n')
    print(json.dumps(evidence, indent=2))
    return 0 if evidence['passed'] else 1


if __name__ == '__main__':
    raise SystemExit(main())
