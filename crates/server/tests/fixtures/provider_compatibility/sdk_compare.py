"""Run only on build-test with the pinned SDK and converter-exported SSE.
No vendor calls, credentials, installation, or product configuration changes.
"""
import argparse
import hashlib
import importlib.metadata
import json
import pathlib
import threading
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from openai import OpenAI

HERE = pathlib.Path(__file__).resolve().parent


def events(data):
    return [json.loads(line[6:]) for line in data.decode().splitlines()
            if line.startswith("data: ") and line[6:] != "[DONE]"]


def normalize(response):
    output = response["output"]
    return {
        "status": response["status"],
        "text": "".join(part.get("text", "") for item in output
                        if item["type"] == "message" for part in item["content"]),
        "tools": [{"id": item["call_id"], "name": item["name"],
                   "arguments": json.loads(item["arguments"])} for item in output
                  if item["type"] == "function_call"],
        "input": response["usage"]["input_tokens"],
        "output": response["usage"]["output_tokens"],
    }


def normalize_chat(response):
    choice = response["choices"][0]
    message = choice["message"]
    return {
        "status": "incomplete" if choice["finish_reason"] == "length" else "completed",
        "text": message.get("content") or "",
        "tools": [{"id": tool["id"], "name": tool["function"]["name"],
                   "arguments": json.loads(tool["function"]["arguments"])}
                  for tool in message.get("tool_calls") or []],
        "input": response["usage"]["prompt_tokens"],
        "output": response["usage"]["completion_tokens"],
    }


def normalize_chat_stream(chunks):
    text, tools, usage, finish = "", {}, {}, None
    for chunk in chunks:
        if chunk.get("usage"):
            usage.update({k: v for k, v in chunk["usage"].items() if v is not None})
        for choice in chunk.get("choices", []):
            finish = choice.get("finish_reason") or finish
            delta = choice.get("delta") or {}
            text += delta.get("content") or ""
            for tool in delta.get("tool_calls") or []:
                record = tools.setdefault(tool["index"], {"id": "", "name": "", "arguments": ""})
                record["id"] = tool.get("id") or record["id"]
                function = tool.get("function") or {}
                record["name"] = function.get("name") or record["name"]
                record["arguments"] += function.get("arguments") or ""
    return {"status": "incomplete" if finish == "length" else "completed", "text": text,
            "tools": [{**tool, "arguments": json.loads(tool["arguments"])} for _, tool in sorted(tools.items())],
            "input": usage.get("prompt_tokens"), "output": usage.get("completion_tokens")}


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--exports", type=pathlib.Path, required=True)
    parser.add_argument("--output", type=pathlib.Path, required=True)
    args = parser.parse_args()
    lock = json.loads((HERE / "sdk-lock.json").read_text())
    version = importlib.metadata.version("openai")
    assert version == lock["version"], (version, lock["version"])
    manifest = json.loads((HERE / "v1/manifest.json").read_text())
    requests = []
    payload = [b"", "text/event-stream"]

    class Handler(BaseHTTPRequestHandler):
        def do_POST(self):
            body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
            assert self.path in ("/v1/responses", "/v1/chat/completions")
            requests.append({"path": self.path, "body": body})
            self.send_response(200)
            self.send_header("Content-Type", payload[1])
            self.send_header("Content-Length", str(len(payload[0])))
            self.end_headers()
            self.wfile.write(payload[0])

        def log_message(self, *_):
            pass

    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    worker = threading.Thread(target=server.serve_forever, daemon=True)
    worker.start()
    url = f"http://127.0.0.1:{server.server_port}/v1"
    results = []
    try:
        with OpenAI(api_key="synthetic-sdk-only", base_url=url, max_retries=0, timeout=5) as client:
            for case in manifest["cases"]:
                if case["expected"]["result"] == "error":
                    continue  # converter's fail-closed rejection is tested in Rust
                wire = (args.exports / (case["id"] + ".responses.sse")).read_bytes()
                payload[0] = wire
                request = {"model": "fixture-model", "input": "synthetic", "stream": True}
                raw_request = urllib.request.Request(url + "/responses", data=json.dumps(request).encode(),
                                                     headers={"Content-Type": "application/json"})
                with urllib.request.urlopen(raw_request, timeout=5) as response:
                    raw = events(response.read())
                sdk = [item.model_dump(mode="json") for item in client.responses.create(**request)]
                raw_terminal = [v["response"] for v in raw if v["type"] in ("response.completed", "response.incomplete")]
                sdk_terminal = [v["response"] for v in sdk if v["type"] in ("response.completed", "response.incomplete")]
                assert len(raw_terminal) == len(sdk_terminal) == 1, case["id"]
                actual = normalize(sdk_terminal[0])
                assert actual == normalize(raw_terminal[0]), case["id"]
                expected = dict(case["expected"])
                expected["status"] = expected.pop("result")
                assert actual == expected, (case["id"], actual, expected)
                assert requests[-2]["body"] == requests[-1]["body"] == request
                results.append({"case": case["id"], "protocol": "responses", "stream": True, "status": "passed", "source": "synthetic_converter_export",
                                "wire_sha256": hashlib.sha256(wire).hexdigest(), "normalized": actual,
                                "raw_event_count": len(raw), "sdk_event_count": len(sdk)})
                chat_wire = (args.exports / (case["id"] + ".chat.sse")).read_bytes()
                payload[:] = [chat_wire, "text/event-stream"]
                chat_request = {"model": "fixture-model", "messages": [{"role": "user", "content": "synthetic"}], "stream": True}
                raw_request = urllib.request.Request(url + "/chat/completions", data=json.dumps(chat_request).encode(),
                                                     headers={"Content-Type": "application/json"})
                with urllib.request.urlopen(raw_request, timeout=5) as response:
                    raw_chat = events(response.read())
                sdk_chat = [item.model_dump(mode="json") for item in client.chat.completions.create(**chat_request)]
                chat_observed = normalize_chat_stream(sdk_chat)
                assert chat_observed == normalize_chat_stream(raw_chat) == expected, (case["id"], chat_observed, expected)
                assert requests[-2]["body"] == requests[-1]["body"] == chat_request
                results.append({"case": case["id"], "protocol": "chat", "stream": True, "status": "passed",
                                "source": "synthetic_converter_export", "normalized": chat_observed,
                                "wire_sha256": hashlib.sha256(chat_wire).hexdigest()})
                # Buffered JSON is exported from the same production conversion.
                for protocol in ("responses", "chat"):
                    wire_json = (args.exports / (case["id"] + "." + protocol + ".json")).read_bytes()
                    payload[:] = [wire_json, "application/json"]
                    if protocol == "responses":
                        json_request = {"model": "fixture-model", "input": "synthetic", "stream": False}
                        path = "/responses"
                        sdk_response = client.responses.create(**json_request).model_dump(mode="json")
                        normalize_value = normalize
                    else:
                        json_request = {"model": "fixture-model", "messages": [{"role": "user", "content": "synthetic"}], "stream": False}
                        path = "/chat/completions"
                        sdk_response = client.chat.completions.create(**json_request).model_dump(mode="json")
                        normalize_value = normalize_chat
                    raw_request = urllib.request.Request(url + path, data=json.dumps(json_request).encode(),
                                                         headers={"Content-Type": "application/json"})
                    with urllib.request.urlopen(raw_request, timeout=5) as response:
                        raw_response = json.load(response)
                    observed = normalize_value(sdk_response)
                    assert observed == normalize_value(raw_response) == expected, (case["id"], protocol, observed, expected)
                    assert requests[-2]["body"] == requests[-1]["body"] == json_request
                    results.append({"case": case["id"], "protocol": protocol, "stream": False,
                                    "status": "passed", "source": "synthetic_converter_export",
                                    "wire_sha256": hashlib.sha256(wire_json).hexdigest(), "normalized": observed})
                payload[:] = [wire, "text/event-stream"]
    finally:
        server.shutdown()
        server.server_close()
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps({"sdk": "openai-python", "version": version,
        "source": "loopback synthetic production-converter exports, no real supplier",
        "dependencies": {d.metadata["Name"]: d.version for d in importlib.metadata.distributions()},
        "results": results, "requests": requests}, ensure_ascii=False, indent=2) + "\n")
    print(f"SDK {version}: {len(results)} synthetic converter/raw/SDK comparisons passed")


if __name__ == "__main__":
    main()
