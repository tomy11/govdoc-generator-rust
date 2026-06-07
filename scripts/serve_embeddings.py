#!/usr/bin/env python3
"""Local OpenAI-compatible embeddings server for govdoc ingestion/retrieval.

Serves `POST /v1/embeddings` and `GET /v1/models` so TyphoonEmbeddingProvider
can use a local multilingual model — good for Thai — with no cloud key:

    EMBEDDING_BACKEND=remote
    EMBEDDING_BASE_URL=http://127.0.0.1:8090/v1
    EMBEDDING_MODEL=BAAI/bge-m3
    EMBEDDING_DIM=1024

Requires sentence-transformers (pulls torch):
    python3 -m pip install -U sentence-transformers

Model is configurable; bge-m3 (1024-dim) is a strong multilingual default that
needs no query/passage prefixes. Lighter option: intfloat/multilingual-e5-small
(set EMBEDDING_LOCAL_MODEL and EMBEDDING_DIM=384).
"""

import json
import os
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

MODEL_NAME = os.environ.get("EMBEDDING_LOCAL_MODEL", "BAAI/bge-m3")
HOST = os.environ.get("EMBEDDING_LOCAL_HOST", "127.0.0.1")
PORT = int(os.environ.get("EMBEDDING_LOCAL_PORT", "8090"))

print(f"Loading embedding model {MODEL_NAME} (first run downloads it)...", flush=True)
from sentence_transformers import SentenceTransformer  # noqa: E402

MODEL = SentenceTransformer(MODEL_NAME)
print(f"Serving embeddings on http://{HOST}:{PORT}/v1", flush=True)


def embed(inputs):
    vectors = MODEL.encode(inputs, normalize_embeddings=True)
    return [vector.tolist() for vector in vectors]


class Handler(BaseHTTPRequestHandler):
    def _send(self, code, payload):
        body = json.dumps(payload).encode("utf-8")
        self.send_response(code)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        if self.path.rstrip("/") == "/v1/models":
            self._send(200, {"object": "list", "data": [{"id": MODEL_NAME, "object": "model"}]})
        else:
            self._send(404, {"error": "not found"})

    def do_POST(self):
        if self.path.rstrip("/") != "/v1/embeddings":
            self._send(404, {"error": "not found"})
            return
        length = int(self.headers.get("content-length", 0))
        try:
            request = json.loads(self.rfile.read(length) or b"{}")
        except json.JSONDecodeError:
            self._send(400, {"error": "invalid JSON"})
            return

        inputs = request.get("input", "")
        if isinstance(inputs, str):
            inputs = [inputs]
        try:
            vectors = embed(inputs)
        except Exception as exc:  # noqa: BLE001
            self._send(500, {"error": str(exc)})
            return

        data = [
            {"object": "embedding", "index": i, "embedding": vector}
            for i, vector in enumerate(vectors)
        ]
        self._send(200, {"object": "list", "data": data, "model": MODEL_NAME})

    def log_message(self, *args):  # silence per-request logging
        pass


if __name__ == "__main__":
    ThreadingHTTPServer((HOST, PORT), Handler).serve_forever()
