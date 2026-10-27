"""Pinned Claude SDK tolerance checks against Rust production-converter exports.
Run on build-test only. No supplier calls, real credentials, or model config writes.
"""
import argparse
import hashlib
import importlib.metadata
import json
import pathlib
import threading
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from anthropic import Anthropic

HERE = pathlib.Path(__file__).resolve().parent


def events(data):
    return [json.loads(line[6:]) for line in data.decode().splitlines() if line.startswith("data: ")]


def normalized(message):
    return {
        "id": message["id"],
        "text": "".join(block["text"] for block in message["content"] if block["type"] == "text"),
        "tools": [{"id": block["id"], "name": block["name"], "input": block["input"]}
                  for block in message["content"] if block["type"] == "tool_use"],
        "stop_reason": message["stop_reason"],
        "input": message["usage"]["input_tokens"],
        "output": message["usage"]["output_tokens"],
    }


def raw_final(values):
    start = next(v["message"] for v in values if v["type"] == "message_start")
    result = dict(start)
    blocks = {}
    arguments = {}
    for value in values:
        kind = value["type"]
        if kind == "content_block_start":
            index = value["index"]
            blocks[index] = dict(value["content_block"])
        elif kind == "content_block_delta":
            index, delta = value["index"], value["delta"]
            if delta["type"] == "text_delta":
                blocks[index]["text"] += delta["text"]
            elif delta["type"] == "input_json_delta":
                arguments[index] = arguments.get(index, "") + delta["partial_json"]
        elif kind == "message_delta":
            result["stop_reason"] = value["delta"]["stop_reason"]
            result["usage"] = value["usage"]
    for index, text in arguments.items():
        blocks[index]["input"] = json.loads(text)
    result["content"] = [blocks[index] for index in sorted(blocks)]
    return result


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--exports", type=pathlib.Path, required=True)
    parser.add_argument("--output", type=pathlib.Path, required=True)
    args = parser.parse_args()
    lock = json.loads((HERE / "anthropic-sdk-lock.json").read_text())
    version = importlib.metadata.version("anthropic")
    assert version == lock["version"], (version, lock["version"])
    payload = [b"", "application/json"]
    requests = []

    class Handler(BaseHTTPRequestHandler):
        def do_POST(self):
            assert self.path == "/v1/messages", self.path
            body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
            requests.append({"path": self.path, "body": body})
            self.send_response(200)
            self.send_header("Content-Type", payload[1])
            self.send_header("Content-Length", str(len(payload[0])))
            self.end_headers()
            self.wfile.write(payload[0])

        def log_message(self, *_):
            pass

    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    url = f"http://127.0.0.1:{server.server_port}"
    results = []
    request = {"model": "fixture-model", "max_tokens": 16,
               "messages": [{"role": "user", "content": "synthetic"}]}
    try:
        with Anthropic(api_key="synthetic-sdk-only", base_url=url, max_retries=0,
                       timeout=5, open_telemetry=False) as client:
            wire = (args.exports / "trace.messages.json").read_bytes()
            payload[:] = [wire, "application/json"]
            raw_request = urllib.request.Request(url + "/v1/messages", data=json.dumps({**request, "stream": False}).encode(),
                                                 headers={"Content-Type": "application/json"})
            with urllib.request.urlopen(raw_request, timeout=5) as response:
                raw = json.load(response)
            sdk = client.messages.create(**request, stream=False).model_dump(mode="json")
            assert normalized(sdk) == normalized(raw)
            assert sdk["tokengo_metadata"] == raw["tokengo_metadata"] == {"request_id": "trace-original"}
            assert sdk["id"] == "completion-original" and sdk["id"] != "trace-original"
            assert sdk["content"][1]["id"] == "call-original"
            assert requests[-2]["body"] == requests[-1]["body"] == {**request, "stream": False}
            results.append({"case": "json", "passed": True, "extra": sdk["tokengo_metadata"],
                            "normalized": normalized(sdk), "wire_sha256": hashlib.sha256(wire).hexdigest()})
            for timing in ("early", "late"):
                wire = (args.exports / f"trace-{timing}.messages.sse").read_bytes()
                payload[:] = [wire, "text/event-stream"]
                raw_request = urllib.request.Request(url + "/v1/messages", data=json.dumps({**request, "stream": True}).encode(),
                                                     headers={"Content-Type": "application/json"})
                with urllib.request.urlopen(raw_request, timeout=5) as response:
                    raw_events = events(response.read())
                sdk_events = [event.model_dump(mode="json") for event in client.messages.create(**request, stream=True)]
                raw_tail = next(v for v in raw_events if v["type"] == "message_delta")
                sdk_tail = next(v for v in sdk_events if v["type"] == "message_delta")
                assert sdk_tail["tokengo_metadata"] == raw_tail["tokengo_metadata"] == {"request_id": "trace-original"}
                assert sum(v["type"] == "message_stop" for v in sdk_events) == 1
                if timing == "early":
                    start = next(v for v in sdk_events if v["type"] == "message_start")
                    assert start["message"]["tokengo_metadata"] == {"request_id": "trace-original"}
                assert requests[-2]["body"] == requests[-1]["body"] == {**request, "stream": True}
                # Exercise SDK stream assembly for text/tools/terminal, without
                # asserting that its final snapshot merges unknown late extras.
                with client.messages.stream(**request) as stream:
                    assembled = stream.get_final_message().model_dump(mode="json")
                expected = normalized(raw_final(raw_events))
                assert normalized(assembled) == expected
                assert expected["text"] == "hello"
                assert expected["tools"] == [{"id": "call-original", "name": "lookup", "input": {"x": 1}}]
                assert expected["stop_reason"] == "tool_use"
                assert (expected["input"], expected["output"]) == (7, 3)
                assert expected["id"] != "trace-original"
                results.append({"case": "sse_" + timing, "passed": True, "event_extra": sdk_tail["tokengo_metadata"],
                                "normalized": expected, "wire_sha256": hashlib.sha256(wire).hexdigest(),
                                "late_extra_scope": "visible on message_delta; get_final_message extra merging not asserted"})
    finally:
        server.shutdown()
        server.server_close()
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps({"sdk": "anthropic-python", "version": version,
        "source": "synthetic loopback production-converter exports only; no real Anthropic source",
        "dependencies": {d.metadata["Name"]: d.version for d in importlib.metadata.distributions()},
        "results": results, "requests": requests}, ensure_ascii=False, indent=2) + "\n")
    print(f"Claude SDK {version}: JSON + early/late SSE tolerance checks passed")


if __name__ == "__main__":
    main()
