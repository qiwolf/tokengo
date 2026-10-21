#!/usr/bin/env python3
"""Optional standard-library MinerU synchronous client; not gateway runtime code.

Use --url https://gateway.example/mineru/file_parse --file synthetic.pdf.
Credentials come only from TOKENGO_API_KEY or a private TOKENGO_API_KEY_FILE.
With neither set, direct anonymous MinerU remains supported. No redirects,
proxy environment, automatic retries, or asynchronous resource downloads.
"""
import argparse
import http.client
import json
import mimetypes
import os
from pathlib import Path
import stat
import sys
import urllib.error
import urllib.parse
import urllib.request
import uuid


class ClientError(Exception):
    """Safe diagnostic: never includes endpoint URLs, credentials, or error bodies."""


def _validated_key(value):
    if not isinstance(value, str):
        raise ClientError("Invalid API key configuration")
    value = value.strip()
    if not value or len(value) > 4096 or any(not 33 <= ord(c) <= 126 for c in value):
        raise ClientError("Invalid API key configuration")
    return value


def load_key(environ=None):
    """Read a key once; two configured sources are an error, never precedence."""
    env = os.environ if environ is None else environ
    direct, filename = env.get("TOKENGO_API_KEY"), env.get("TOKENGO_API_KEY_FILE")
    if direct is not None and filename is not None:
        raise ClientError("Configure exactly one API key source")
    if direct is not None:
        return _validated_key(direct)
    if filename is None:
        return None
    fd = None
    try:
        fd = os.open(filename, os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0) | getattr(os, "O_NONBLOCK", 0))
        info = os.fstat(fd)
        if not stat.S_ISREG(info.st_mode) or info.st_size > 4097:
            raise ClientError("Invalid API key file")
        if os.name == "posix" and (info.st_uid != os.getuid() or info.st_mode & 0o077):
            raise ClientError("API key file must be owned by this user and private (0600)")
        raw = os.read(fd, 4098)
        return _validated_key(raw.decode("ascii"))
    except (OSError, UnicodeError, ValueError):
        raise ClientError("Unable to read private API key file") from None
    finally:
        if fd is not None:
            os.close(fd)


class _NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


class Client:
    def __init__(self, url, key=None, timeout=1050,
                 max_body_bytes=20 * 1024 * 1024, max_response_bytes=64 * 1024 * 1024):
        try:
            parsed = urllib.parse.urlsplit(url)
            valid = (parsed.scheme in ("http", "https") and parsed.hostname
                     and not parsed.username and not parsed.password
                     and "@" not in parsed.netloc
                     and not parsed.query and not parsed.fragment
                     and parsed.path.endswith("/file_parse")
                     and not any(s in (".", "..") for s in parsed.path.split("/"))
                     and "//" not in parsed.path and "%" not in parsed.path
                     and "\\" not in url and not any(c.isspace() or ord(c) < 32 for c in url))
            _ = parsed.port  # Reject malformed ports before making any request.
        except (TypeError, ValueError):
            valid = False
        if not valid:
            raise ClientError("URL must be a clean HTTP(S) endpoint ending in /file_parse")
        if not 0 < timeout <= 3600 or not 0 < max_body_bytes <= 100 * 1024 * 1024 or not 0 < max_response_bytes <= 256 * 1024 * 1024:
            raise ClientError("Invalid timeout or transfer size limit")
        self.url = url
        self.health_url = url[:-len("/file_parse")] + "/health"
        self.key = None if key is None else _validated_key(key)
        self.timeout = timeout
        self.max_body_bytes = max_body_bytes
        self.max_response_bytes = max_response_bytes
        self.opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), _NoRedirect())

    def _request(self, url, data=None, content_type=None, timeout=None):
        headers = {"Accept-Encoding": "identity"}
        if self.key is not None:
            headers["Authorization"] = "Bearer " + self.key
        if content_type:
            headers["Content-Type"] = content_type
        request = urllib.request.Request(url, data=data, headers=headers,
                                         method="GET" if data is None else "POST")
        try:
            with self.opener.open(request, timeout=self.timeout if timeout is None else timeout) as response:
                if response.status != 200:
                    raise ClientError(f"Upstream returned HTTP {response.status}")
                raw = response.read(self.max_response_bytes + 1)
                if len(raw) > self.max_response_bytes:
                    raise ClientError("Response exceeds configured size limit")
                return raw, response.headers.get_content_type()
        except urllib.error.HTTPError as error:
            code = error.code
            error.close()
            raise ClientError(f"Upstream returned HTTP {code}") from None
        except (OSError, http.client.HTTPException, ValueError):
            raise ClientError("HTTP transport failed or timed out") from None

    def health(self):
        try:
            self._request(self.health_url, timeout=min(self.timeout, 30))
            return True
        except ClientError:
            return False

    def parse(self, filename, zip_output=False):
        path = Path(filename)
        try:
            with path.open("rb") as source:
                content = source.read(self.max_body_bytes + 1)
        except OSError:
            raise ClientError("Input document cannot be read") from None
        if len(content) > self.max_body_bytes:
            raise ClientError("Request exceeds configured size limit")
        boundary = "tokengo-mineru-" + uuid.uuid4().hex
        safe_name = "".join("_" if c in '\\"' or ord(c) < 32 or ord(c) == 127 else c for c in path.name)[:240]
        media_type = mimetypes.guess_type(path.name)[0] or "application/octet-stream"
        fields = (("return_md", "true"), ("response_format_zip", "true" if zip_output else "false"))
        body = b"".join((f'--{boundary}\r\nContent-Disposition: form-data; name="{name}"\r\n\r\n{value}\r\n').encode() for name, value in fields)
        body += (f'--{boundary}\r\nContent-Disposition: form-data; name="files"; filename="{safe_name}"\r\nContent-Type: {media_type}\r\n\r\n').encode("utf-8")
        body += content + f"\r\n--{boundary}--\r\n".encode()
        if len(body) > self.max_body_bytes:
            raise ClientError("Request exceeds configured size limit including multipart overhead")
        raw, response_type = self._request(self.url, body, "multipart/form-data; boundary=" + boundary)
        if zip_output:
            if response_type != "application/zip" or not raw.startswith(b"PK"):
                raise ClientError("Expected a ZIP response")
            return raw
        try:
            result = json.loads(raw)
            if not isinstance(result, dict) or not isinstance(result.get("results"), dict):
                raise ValueError()
            markdown = [item["md_content"] for item in result["results"].values()
                        if isinstance(item, dict) and isinstance(item.get("md_content"), str) and item["md_content"].strip()]
            if len(markdown) != 1 or result.get("status", "completed") != "completed":
                raise ValueError()
            return markdown[0].encode("utf-8")
        except (ValueError, TypeError, KeyError, UnicodeError):
            raise ClientError("Expected one completed inline Markdown result") from None


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--url", required=True, help="Explicit /file_parse endpoint")
    parser.add_argument("--file", required=True, type=Path, help="One document to upload as files")
    parser.add_argument("--output", type=Path, help="Output file; otherwise writes bytes to stdout")
    parser.add_argument("--format", choices=("markdown", "zip"), default="markdown")
    parser.add_argument("--timeout", type=float, default=1050, help="Socket operation timeout in seconds (default: 1050)")
    parser.add_argument("--max-body-mib", type=int, default=20, help="Whole multipart limit, at most 100 MiB")
    parser.add_argument("--max-response-mib", type=int, default=64, help="Response limit, at most 256 MiB")
    args = parser.parse_args(argv)
    try:
        client = Client(args.url, load_key(), args.timeout, args.max_body_mib * 1024 * 1024,
                        args.max_response_mib * 1024 * 1024)
        if not client.health():
            raise ClientError("MinerU health check failed; request was not retried")
        output = client.parse(args.file, zip_output=args.format == "zip")
        if args.output:
            args.output.write_bytes(output)
        else:
            sys.stdout.buffer.write(output)
        return 0
    except (ClientError, OSError) as error:
        print(f"mineru-client: {error if isinstance(error, ClientError) else 'Unable to write output'}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
