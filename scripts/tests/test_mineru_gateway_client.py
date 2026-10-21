"""Standard-library-only client contract tests; all identities/data are synthetic."""
import contextlib
import importlib.util
import json
import os
from pathlib import Path
import tempfile
import subprocess
import sys
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import unittest

CLIENT_PATH = Path(__file__).resolve().parents[1] / "mineru-gateway-client.py"
spec = importlib.util.spec_from_file_location("mineru_gateway_client", CLIENT_PATH)
client = importlib.util.module_from_spec(spec)
spec.loader.exec_module(client)
KEY = "synthetic-sensitive-vkey-marker"

@contextlib.contextmanager
def server(reply):
    seen = []
    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *args):
            pass
        def run_request(self):
            n = int(self.headers.get("Content-Length", 0))
            body = self.rfile.read(n) if n else b""
            seen.append({"method": self.command, "path": self.path,
                         "headers": self.headers, "body": body})
            status, headers, payload = reply(seen[-1])
            self.send_response(status)
            for name, value in headers.items():
                self.send_header(name, value)
            self.send_header("Content-Length", str(len(payload)))
            self.end_headers()
            try:
                self.wfile.write(payload)
            except (BrokenPipeError, ConnectionResetError):
                pass
        do_GET = run_request
        do_POST = run_request
    httpd = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    httpd.daemon_threads = True
    t = threading.Thread(target=httpd.serve_forever, daemon=True)
    t.start()
    try:
        yield f"http://127.0.0.1:{httpd.server_port}", seen
    finally:
        httpd.shutdown()
        httpd.server_close()
        t.join(timeout=2)

class ClientContract(unittest.TestCase):
    def test_key_env_and_file_are_mutually_exclusive(self):
        with self.assertRaises(client.ClientError):
            client.load_key({"TOKENGO_API_KEY": KEY, "TOKENGO_API_KEY_FILE": "/unused"})
        self.assertIsNone(client.load_key({}))
        self.assertEqual(client.load_key({"TOKENGO_API_KEY": KEY}), KEY)

    def test_private_key_file_newline_supported_and_insecure_mode_rejected(self):
        with tempfile.TemporaryDirectory() as d:
            p = Path(d) / "key"
            p.write_text(KEY + "\n")
            p.chmod(0o600)
            self.assertEqual(client.load_key({"TOKENGO_API_KEY_FILE": str(p)}), KEY)
            if os.name == "posix":
                p.chmod(0o644)
                with self.assertRaises(client.ClientError):
                    client.load_key({"TOKENGO_API_KEY_FILE": str(p)})

    @unittest.skipUnless(hasattr(os, "mkfifo"), "POSIX named pipes only")
    def test_key_named_pipe_is_rejected_without_waiting_for_writer(self):
        with tempfile.TemporaryDirectory() as d:
            p = Path(d) / "not-a-key-file"
            os.mkfifo(p, 0o600)
            with self.assertRaises(client.ClientError):
                client.load_key({"TOKENGO_API_KEY_FILE": str(p)})

    def test_header_injection_and_excessive_key_rejected_without_echo(self):
        for secret in [KEY + "\r\nX-Leak: yes", "x" * 4097]:
            with self.assertRaises(client.ClientError) as e:
                client.load_key({"TOKENGO_API_KEY": secret})
            self.assertNotIn(secret, str(e.exception))

    def test_only_clean_file_parse_urls_accepted(self):
        for url in ["file:///tmp/file_parse", "http://u:p@host/file_parse",
                    "http://host/file_parse?token=secret", "http://host/file_parse#x",
                    "http://host/other", "http://host/mineru/../file_parse"]:
            with self.assertRaises(client.ClientError):
                client.Client(url, key=KEY)

    def test_health_and_upload_both_authenticate_and_keep_multipart_files(self):
        def reply(req):
            if req["path"].endswith("/health"):
                return 200, {"Content-Type": "application/json"}, b'{"status":"healthy"}'
            return 200, {"Content-Type": "application/json"}, json.dumps({
                "results": {"synthetic": {"md_content": "# Synthetic\nred square"}}
            }).encode()
        with server(reply) as (base, seen), tempfile.TemporaryDirectory() as d:
            p = Path(d) / "synthetic.png"
            p.write_bytes(b"\x89PNG\r\n\x1a\nsynthetic")
            c = client.Client(base + "/mineru/file_parse", key=KEY)
            self.assertTrue(c.health())
            self.assertEqual(c.parse(p), b"# Synthetic\nred square")
            self.assertEqual([r["path"] for r in seen], ["/mineru/health", "/mineru/file_parse"])
            self.assertTrue(all(r["headers"].get("Authorization") == "Bearer " + KEY for r in seen))
            self.assertIn(b'name="files"', seen[1]["body"])
            self.assertIn(p.read_bytes(), seen[1]["body"])
            self.assertNotIn(KEY.encode(), seen[1]["body"])

    def test_direct_default_has_no_authentication_header(self):
        with server(lambda r: (200, {}, b'{"status":"healthy"}')) as (base, seen):
            self.assertTrue(client.Client(base + "/file_parse").health())
            self.assertNotIn("Authorization", seen[0]["headers"])

    def test_redirect_never_receives_health_or_upload_credential(self):
        with server(lambda r: (200, {}, b"unexpected")) as (other, other_seen):
            def redirect(req):
                return 307, {"Location": other + "/steal"}, b"redirect"
            with server(redirect) as (base, seen), tempfile.TemporaryDirectory() as d:
                c = client.Client(base + "/file_parse", key=KEY)
                self.assertFalse(c.health())
                p = Path(d) / "synthetic.pdf"
                p.write_bytes(b"%PDF-synthetic")
                with self.assertRaises(client.ClientError):
                    c.parse(p)
                self.assertEqual(len(seen), 2)
                self.assertEqual(other_seen, [])

    def test_upstream_error_body_and_token_never_appear_in_error(self):
        with server(lambda r: (401, {}, ("reflected " + KEY).encode())) as (base, _), tempfile.TemporaryDirectory() as d:
            p = Path(d) / "synthetic.pdf"
            p.write_bytes(b"%PDF-synthetic")
            with self.assertRaises(client.ClientError) as e:
                client.Client(base + "/file_parse", key=KEY).parse(p)
            self.assertNotIn(KEY, str(e.exception))
            self.assertNotIn("reflected", str(e.exception))
            self.assertIn("401", str(e.exception))

    def test_request_and_response_size_limits(self):
        with server(lambda r: (200, {}, b"x" * 1025)) as (base, seen), tempfile.TemporaryDirectory() as d:
            p = Path(d) / "synthetic.pdf"
            p.write_bytes(b"%PDF-synthetic")
            with self.assertRaises(client.ClientError):
                client.Client(base + "/file_parse", key=KEY, max_body_bytes=10).parse(p)
            self.assertEqual(seen, [])
            with self.assertRaises(client.ClientError):
                client.Client(base + "/file_parse", key=KEY, max_response_bytes=1024).parse(p)
            self.assertEqual(len(seen), 1)

    def test_zip_is_returned_as_original_bytes(self):
        payload = b"PK\x03\x04\x00synthetic-archive\x00\xff"
        with server(lambda r: (200, {"Content-Type": "application/zip"}, payload)) as (base, seen), tempfile.TemporaryDirectory() as d:
            p = Path(d) / "synthetic.pdf"
            p.write_bytes(b"%PDF-synthetic")
            result = client.Client(base + "/file_parse", key=KEY).parse(p, zip_output=True)
            self.assertEqual(result, payload)
            self.assertIn(b'name="response_format_zip"', seen[0]["body"])

    def test_cli_markdown_stdout_and_zip_output_file(self):
        zip_payload = b"PK\x03\x04synthetic-cli-zip\xff"
        def reply(req):
            if req["method"] == "GET":
                return 200, {}, b'{"status":"healthy"}'
            if b'name="response_format_zip"\r\n\r\ntrue' in req["body"]:
                return 200, {"Content-Type": "application/zip"}, zip_payload
            return 200, {"Content-Type": "application/json"}, b'{"status":"completed","results":{"synthetic":{"md_content":"# CLI\\nSynthetic"}}}'
        with server(reply) as (base, seen), tempfile.TemporaryDirectory() as d:
            source = Path(d) / "synthetic.pdf"
            source.write_bytes(b"%PDF-synthetic")
            env = {k: v for k, v in os.environ.items()
                   if k not in ("TOKENGO_API_KEY", "TOKENGO_API_KEY_FILE")}
            env["TOKENGO_API_KEY"] = KEY
            args = [sys.executable, str(CLIENT_PATH), "--url", base + "/mineru/file_parse",
                    "--file", str(source), "--timeout", "2"]
            markdown = subprocess.run(args, env=env, capture_output=True, timeout=5)
            self.assertEqual(markdown.returncode, 0, markdown.stderr)
            self.assertEqual(markdown.stdout, b"# CLI\nSynthetic")
            output = Path(d) / "result.zip"
            zipped = subprocess.run(args + ["--format", "zip", "--output", str(output)],
                                    env=env, capture_output=True, timeout=5)
            self.assertEqual(zipped.returncode, 0, zipped.stderr)
            self.assertEqual(zipped.stdout, b"")
            self.assertEqual(output.read_bytes(), zip_payload)
            self.assertEqual(len(seen), 4)
            self.assertTrue(all(r["headers"].get("Authorization") == "Bearer " + KEY for r in seen))
            self.assertNotIn(KEY.encode(), markdown.stderr + zipped.stderr)

    def test_health_failure_and_timeouts_are_bounded(self):
        import time
        def slow(req):
            time.sleep(.2)
            return 200, {}, b"{}"
        with server(slow) as (base, _):
            start = time.monotonic()
            self.assertFalse(client.Client(base + "/file_parse", key=KEY, timeout=.03).health())
            self.assertLess(time.monotonic() - start, .18)

if __name__ == "__main__":
    unittest.main()
