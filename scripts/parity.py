#!/usr/bin/env python3
"""Compare our figma-agent's responses against the upstream Figma agent.

Run once per protocol (the CI runs it over HTTP and again over HTTPS). A single
invocation performs the full parity check for that protocol:

  1. /figma/font-files  — fetch from both agents, normalise, and diff the JSON.
  2. /figma/font-file    — for every shared font under the size cap, compare the
                           raw bytes (sha256). No sampling: all files are checked.

The script exits non-zero on the first mismatch. It depends only on the Python
standard library (no curl / jq / shasum, no pip installs), so the comparison
rules live in readable code instead of shell + jq one-liners.

Why the binary step polls upstream: the upstream agent warms its font cache
lazily, so a single cold fetch can return an empty body. We poll until upstream
returns a stable answer (N consecutive identical, non-empty hashes) and use that
as the comparison oracle; if it never stabilises we fail loudly rather than
silently skip.
"""

import argparse
import difflib
import hashlib
import json
import os
import ssl
import sys
import time
import urllib.error
import urllib.parse
import urllib.request

FONT_FILES_PATH = "/figma/font-files"
FONT_FILE_PATH = "/figma/font-file"

# Top-level keys compared verbatim between the two agents. Mirrors the fields
# the previous jq filter selected.
TOP_LEVEL_KEYS = (
    "version",
    "package",
    "modified_at",
    "modified_fonts",
    "machine_id",
    "launch_source",
)


def build_opener(insecure_tls):
    if insecure_tls:
        context = ssl._create_unverified_context()
    else:
        context = ssl.create_default_context()
    return urllib.request.build_opener(urllib.request.HTTPSHandler(context=context))


def fetch(opener, base_url, path, origin, timeout, file_param=None):
    """GET base_url+path and return the response body as bytes.

    When file_param is given it is appended as ?file=<value>, percent-encoded
    once with `/` left intact (matching the upstream agent's expectation).
    Returns None on any HTTP/connection error (the caller decides whether that
    is fatal or just "not warm yet").
    """
    url = base_url.rstrip("/") + path
    if file_param is not None:
        url += "?file=" + urllib.parse.quote(file_param, safe="/")
    request = urllib.request.Request(url, headers={"Origin": origin})
    try:
        with opener.open(request, timeout=timeout) as response:
            return response.read()
    except (urllib.error.URLError, TimeoutError):
        return None


def normalize(document):
    """Normalise a /figma/font-files document for comparison.

    Keeps a fixed set of top-level keys, sorts fontFiles by path, and within each
    path drops every face's modified_at (upstream's cache-insertion timestamp,
    not the file mtime, so impossible to match) and sorts faces by
    (postscript, style) to neutralise enumeration order.
    """
    result = {key: document.get(key) for key in TOP_LEVEL_KEYS}
    font_files = {}
    for path in sorted(document.get("fontFiles", {})):
        faces = [
            {key: value for key, value in face.items() if key != "modified_at"}
            for face in document["fontFiles"][path]
        ]
        faces.sort(key=lambda face: (face.get("postscript", ""), face.get("style", "")))
        font_files[path] = faces
    result["fontFiles"] = font_files
    return result


def render(document):
    return json.dumps(document, indent=2, ensure_ascii=False).splitlines()


def compare_font_files(upstream_doc, ours_doc, scheme):
    upstream_lines = render(normalize(upstream_doc))
    ours_lines = render(normalize(ours_doc))
    if upstream_lines == ours_lines:
        count = len(upstream_doc.get("fontFiles", {}))
        print(f"font-files ({scheme}): OK ({count} font entries match)")
        return True
    diff = difflib.unified_diff(
        upstream_lines, ours_lines, fromfile="upstream", tofile="ours", lineterm=""
    )
    print("\n".join(diff))
    print(f"::error::/figma/font-files mismatch over {scheme}")
    return False


def sha256_of(content):
    return hashlib.sha256(content).hexdigest() if content else None


def stable_upstream_hash(opener, args, path):
    """Poll upstream until its hash is stable, or give up.

    Returns (hash, attempts_used) on success, or (None, attempts_used) if the
    response never stabilised to `stable_streak` consecutive identical,
    non-empty hashes within `max_poll_attempts`.
    """
    previous = None
    streak = 0
    for attempt in range(1, args.max_poll_attempts + 1):
        current = sha256_of(
            fetch(
                opener,
                args.upstream_url,
                FONT_FILE_PATH,
                args.origin_header,
                args.request_timeout_seconds,
                file_param=path,
            )
        )
        if current is not None and current == previous:
            streak += 1
        else:
            streak = 1
        previous = current
        if current is not None and streak >= args.stable_streak:
            return current, attempt
        time.sleep(args.poll_interval_seconds)
    return None, args.max_poll_attempts


def compare_binaries(opener, args, paths, scheme):
    compared = 0
    skipped = 0
    print(f"::group::font-file binaries ({scheme})")
    try:
        for path in paths:
            if not os.path.isfile(path) or os.path.getsize(path) > args.max_file_bytes:
                skipped += 1
                continue

            upstream_hash, attempts = stable_upstream_hash(opener, args, path)
            if upstream_hash is None:
                print(
                    f"::error::upstream never stabilized "
                    f"({args.stable_streak}x identical non-empty) for: {path}"
                )
                return False

            ours_hash = sha256_of(
                fetch(
                    opener,
                    args.ours_url,
                    FONT_FILE_PATH,
                    args.origin_header,
                    args.request_timeout_seconds,
                    file_param=path,
                )
            )
            if upstream_hash != ours_hash:
                print(f"::error::binary diff: {path}")
                print(f"  upstream: {upstream_hash}")
                print(f"  ours:     {ours_hash}")
                return False

            note = f"  (warmed in {attempts} polls)" if attempts > 1 else ""
            print(f"ok {upstream_hash}  {path}{note}")
            compared += 1
    finally:
        print("::endgroup::")
    print(f"binaries ({scheme}): OK (compared {compared}, skipped {skipped})")
    return True


def parse_args(argv):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--upstream-url",
        required=True,
        help="Base URL of the upstream agent, e.g. http://127.0.0.1:44950",
    )
    parser.add_argument(
        "--ours-url",
        required=True,
        help="Base URL of our agent, e.g. http://127.0.0.1:45000",
    )
    parser.add_argument(
        "--origin-header",
        required=True,
        help="Value of the Origin request header, e.g. https://www.figma.com",
    )
    parser.add_argument(
        "--insecure-tls",
        action="store_true",
        help="Skip TLS verification (for the HTTPS step's self-signed cert)",
    )
    parser.add_argument(
        "--max-file-bytes",
        type=int,
        default=33554432,
        help="Skip font files larger than this (upstream's 32 MiB cap)",
    )
    parser.add_argument(
        "--stable-streak",
        type=int,
        default=3,
        help="Consecutive identical, non-empty upstream hashes required",
    )
    parser.add_argument(
        "--max-poll-attempts",
        type=int,
        default=20,
        help="Maximum upstream fetches per file while waiting for stability",
    )
    parser.add_argument(
        "--poll-interval-seconds",
        type=float,
        default=1.0,
        help="Delay between upstream poll attempts",
    )
    parser.add_argument(
        "--request-timeout-seconds",
        type=float,
        default=60.0,
        help="Per-request timeout",
    )
    return parser.parse_args(argv)


def main(argv):
    args = parse_args(argv)
    scheme = "HTTPS" if args.upstream_url.lower().startswith("https") else "HTTP"
    opener = build_opener(args.insecure_tls)

    upstream_raw = fetch(
        opener, args.upstream_url, FONT_FILES_PATH, args.origin_header, args.request_timeout_seconds
    )
    ours_raw = fetch(
        opener, args.ours_url, FONT_FILES_PATH, args.origin_header, args.request_timeout_seconds
    )
    if upstream_raw is None or ours_raw is None:
        which = "upstream" if upstream_raw is None else "ours"
        print(f"::error::could not fetch {FONT_FILES_PATH} from {which} over {scheme}")
        return 1

    upstream_doc = json.loads(upstream_raw)
    ours_doc = json.loads(ours_raw)

    if not compare_font_files(upstream_doc, ours_doc, scheme):
        return 1

    paths = sorted(upstream_doc.get("fontFiles", {}))
    if not compare_binaries(opener, args, paths, scheme):
        return 1

    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
