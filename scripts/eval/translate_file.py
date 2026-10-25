"""Translate a file of lines with the app's translator, one line at a time.

Used to draft the English reference and to try prompt variants without rebuilding the app.
Each line goes alone (no context), at the app's sampling settings, through a llama-server
started on a free port with the app's flags.

  python scripts/eval/translate_file.py --in testdata/eval/drama-130s.zh.txt \
      --out testdata/eval/drama-130s.en.draft.txt

`--prompt-file` replaces the instruction prefix; `{text}` marks where the line goes.
"""

import argparse
import json
import socket
import subprocess
import sys
import time
import urllib.request
from pathlib import Path

sys.stdout.reconfigure(encoding="utf-8")

PREFIX = (
    "Translate the following text into English. Note that you should only output the "
    "translated result without any additional explanation:\n\n"
)


def free_port():
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        return probe.getsockname()[1]


def start_server(root, model, port):
    log = open(root / ".agent/experiments/llama-translate-file.log", "w", encoding="utf-8")
    process = subprocess.Popen(
        [
            str(root / ".deps/llama-b11429/llama-server.exe"),
            "-m", str(model),
            "--host", "127.0.0.1", "--port", str(port),
            "--jinja", "-np", "1", "-c", "2048", "-t", "4", "-tb", "4",
            "--override-kv", "tokenizer.ggml.eos_token_id=int:120020",
            "--cache-ram", "0", "--no-webui",
        ],
        stdout=log, stderr=subprocess.STDOUT,
    )
    for _ in range(600):
        try:
            with urllib.request.urlopen(f"http://127.0.0.1:{port}/health", timeout=1) as reply:
                if reply.status == 200:
                    return process
        except Exception:
            time.sleep(0.2)
    process.kill()
    raise SystemExit("llama-server did not become ready")


def translate(port, prompt, max_tokens):
    body = {
        "messages": [{"role": "user", "content": prompt}],
        "temperature": 0.0,
        "top_k": 1,
        "top_p": 1.0,
        "min_p": 0.0,
        "repeat_penalty": 1.05,
        "max_tokens": max_tokens,
        "cache_prompt": True,
    }
    request = urllib.request.Request(
        f"http://127.0.0.1:{port}/v1/chat/completions",
        data=json.dumps(body).encode(),
        headers={"Content-Type": "application/json"},
    )
    with urllib.request.urlopen(request, timeout=120) as reply:
        payload = json.load(reply)
    choice = payload["choices"][0]
    return choice["message"]["content"].strip(), choice.get("finish_reason")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--in", dest="source", required=True)
    parser.add_argument("--out", required=True)
    parser.add_argument("--prompt-file")
    parser.add_argument("--root", default=".")
    parser.add_argument(
        "--model",
        default="models/hy-mt2-1.8b-ternary-tq2_0/Hy-MT2-1.8B-ternary-TQ2_0.gguf",
    )
    args = parser.parse_args()

    root = Path(args.root).resolve()
    template = Path(args.prompt_file).read_text(encoding="utf-8") if args.prompt_file else None

    lines = [
        line.strip()
        for line in Path(args.source).read_text(encoding="utf-8").splitlines()
        if line.strip()
    ]
    port = free_port()
    process = start_server(root, root / args.model, port)
    answers = []
    try:
        for index, line in enumerate(lines):
            if template:
                prompt = template.replace("{text}", line)
            else:
                prompt = PREFIX + line
            answer, reason = translate(port, prompt, max(24, min(160, 2 * len(line) + 8)))
            answers.append(answer)
            print(f"{index:3d} [{reason}] {line} -> {answer}", flush=True)
    finally:
        process.kill()
        process.wait(timeout=10)

    Path(args.out).write_text("\n".join(answers) + "\n", encoding="utf-8", newline="\n")


if __name__ == "__main__":
    main()
