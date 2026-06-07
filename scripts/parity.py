#!/usr/bin/env python3
"""Compare our local figma-agent against the upstream Figma agent.

Run once per protocol (CI runs it over HTTP and HTTPS). Each run diffs the
normalised /figma/font-files JSON, then compares the raw bytes of every shared
font under the size cap (no sampling). Exits non-zero on the first mismatch.
Stdlib only (no curl/jq/shasum); targets Python 3.9.
"""

import argparse
import difflib
import functools
import hashlib
import json
import os
import ssl
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from typing import Callable, Optional

FONT_FILES_PATH = "/figma/font-files"
FONT_FILE_PATH = "/figma/font-file"

# Top-level keys compared verbatim between the two agents.
TOP_LEVEL_KEYS = (
    "version",
    "package",
    "modified_at",
    "modified_fonts",
    "machine_id",
    "launch_source",
)

# `fetch` with opener/origin/timeout/base_url bound; callers pass (path, file_param=...).
Fetcher = Callable[..., Optional[bytes]]


def build_opener(insecure_tls: bool) -> urllib.request.OpenerDirector:
    context = ssl._create_unverified_context() if insecure_tls else ssl.create_default_context()
    return urllib.request.build_opener(urllib.request.HTTPSHandler(context=context))


def fetch(
    opener: urllib.request.OpenerDirector,
    origin: str,
    timeout: float,
    base_url: str,
    path: str,
    file_param: Optional[str] = None,
) -> Optional[bytes]:
    """GET base_url+path; return the body, or None on any HTTP/connection error.

    file_param is appended as ?file=<value>, encoded once with `/` intact. Config
    args lead so callers can bind them with functools.partial.
    """
    url = base_url.removesuffix("/") + path
    if file_param is not None:
        url += "?file=" + urllib.parse.quote(file_param, safe="/")
    request = urllib.request.Request(url, headers={"Origin": origin})
    try:
        with opener.open(request, timeout=timeout) as response:
            return response.read()
    except (urllib.error.URLError, TimeoutError):
        return None


def sha256_of(content: Optional[bytes]) -> Optional[str]:
    return hashlib.sha256(content).hexdigest() if content else None


def normalize(document: dict) -> dict:
    """Normalise /figma/font-files for comparison: keep a fixed set of top-level
    keys, sort fontFiles by path, and per path drop each face's modified_at
    (upstream's cache timestamp) and sort faces by (postscript, style).
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


def compare_font_files(upstream_doc: dict, local_doc: dict, scheme: str) -> bool:
    def render(document: dict) -> list[str]:
        return json.dumps(normalize(document), indent=2, ensure_ascii=False).splitlines()

    upstream_lines, local_lines = render(upstream_doc), render(local_doc)
    if upstream_lines == local_lines:
        print(f"font-files ({scheme}): OK ({len(upstream_doc.get('fontFiles', {}))} entries match)")
        return True
    diff = difflib.unified_diff(
        upstream_lines, local_lines, fromfile="upstream", tofile="local", lineterm=""
    )
    print("\n".join(diff))
    print(f"::error::/figma/font-files mismatch over {scheme}")
    return False


def stable_upstream_hash(upstream: Fetcher, args: argparse.Namespace, path: str) -> tuple[Optional[str], int]:
    """Poll upstream until `stable_streak` consecutive identical non-empty hashes
    (its cache warms lazily, so cold fetches return empty). Returns (hash,
    attempts), or (None, attempts) if it never stabilises.
    """
    previous: Optional[str] = None
    streak = empty = 0
    for attempt in range(1, args.max_poll_attempts + 1):
        current = sha256_of(upstream(FONT_FILE_PATH, file_param=path))
        if current is None:
            empty += 1
            streak = 0
        else:
            streak = streak + 1 if current == previous else 1
        previous = current
        if current is not None and streak >= args.stable_streak:
            return current, attempt
        if current is None:  # only wait while the cache is still warming
            time.sleep(args.poll_interval_seconds)
    print(
        f"::error::upstream never stabilized ({args.stable_streak}x identical non-empty) "
        f"for: {path} [{args.max_poll_attempts} attempts, {empty} empty]"
    )
    return None, args.max_poll_attempts


def compare_binaries(upstream: Fetcher, local: Fetcher, args: argparse.Namespace, paths: list[str], scheme: str) -> bool:
    compared = skipped = 0
    print(f"::group::font-file binaries ({scheme})")
    try:
        for path in paths:
            if not os.path.isfile(path) or os.path.getsize(path) > args.max_file_bytes:
                skipped += 1
                continue

            upstream_hash, attempts = stable_upstream_hash(upstream, args, path)
            if upstream_hash is None:
                return False

            local_hash = sha256_of(local(FONT_FILE_PATH, file_param=path))
            if upstream_hash != local_hash:
                print(f"::error::binary diff: {path}\n  upstream: {upstream_hash}\n  local:    {local_hash}")
                return False

            note = f"  (warmed in {attempts} polls)" if attempts > 1 else ""
            print(f"ok {upstream_hash}  {path}{note}")
            compared += 1
    finally:
        print("::endgroup::")
    print(f"binaries ({scheme}): OK (compared {compared}, skipped {skipped})")
    return True


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--upstream-url", required=True, help="Base URL of the upstream agent")
    parser.add_argument("--local-url", required=True, help="Base URL of the local agent")
    parser.add_argument("--origin-header", required=True, help="Origin request header value")
    parser.add_argument("--insecure-tls", action="store_true", help="Skip TLS verification (self-signed HTTPS cert)")
    parser.add_argument("--max-file-bytes", type=int, default=33554432, help="Skip files larger than this (32 MiB cap)")
    parser.add_argument("--stable-streak", type=int, default=3, help="Consecutive identical non-empty hashes required")
    parser.add_argument("--max-poll-attempts", type=int, default=30, help="Max upstream fetches per file")
    parser.add_argument("--poll-interval-seconds", type=float, default=2.0, help="Delay between polls while warming")
    parser.add_argument("--request-timeout-seconds", type=float, default=60.0, help="Per-request timeout")
    return parser.parse_args(argv)


def main(argv: list[str]) -> int:
    args = parse_args(argv)
    scheme = "HTTPS" if args.upstream_url.lower().startswith("https") else "HTTP"

    bound = functools.partial(
        fetch, build_opener(args.insecure_tls), args.origin_header, args.request_timeout_seconds
    )
    upstream: Fetcher = functools.partial(bound, args.upstream_url)
    local: Fetcher = functools.partial(bound, args.local_url)

    upstream_raw, local_raw = upstream(FONT_FILES_PATH), local(FONT_FILES_PATH)
    if upstream_raw is None or local_raw is None:
        which = "upstream" if upstream_raw is None else "local"
        print(f"::error::could not fetch {FONT_FILES_PATH} from {which} over {scheme}")
        return 1

    upstream_doc, local_doc = json.loads(upstream_raw), json.loads(local_raw)
    if not compare_font_files(upstream_doc, local_doc, scheme):
        return 1

    paths = sorted(upstream_doc.get("fontFiles", {}))
    return 0 if compare_binaries(upstream, local, args, paths, scheme) else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
