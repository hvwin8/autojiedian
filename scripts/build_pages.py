#!/usr/bin/env python3
from __future__ import annotations

import argparse
import hashlib
import json
import os
import shutil
import subprocess
from datetime import datetime, timezone
from pathlib import Path
from typing import Any


ROOT = Path(__file__).resolve().parents[1]


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Prepare a static GitHub Pages site for autojiedian.")
    parser.add_argument("--output-dir", default="_site")
    parser.add_argument("--base-url", default="")
    parser.add_argument("--release-file", default="clash.yaml")
    parser.add_argument("--summary-file", default="artifacts/09_pipeline_summary.json")
    parser.add_argument("--registry-file", default="artifacts/10_source_registry.json")
    parser.add_argument("--fallback-registry-file", default="artifacts/source_registry.json")
    return parser.parse_args()


def read_json(path: Path) -> Any:
    return json.loads(path.read_text(encoding="utf-8"))


def count_release_proxies(release_file: Path) -> int:
    in_proxies = False
    count = 0
    for raw_line in release_file.read_text(encoding="utf-8", errors="ignore").splitlines():
        line = raw_line.rstrip()
        stripped = line.strip()
        if not stripped or stripped.startswith("#"):
            continue
        if not in_proxies:
            if line == "proxies:":
                in_proxies = True
            continue
        if not raw_line.startswith((" ", "\t", "-")) and stripped.endswith(":"):
            break
        if raw_line.startswith("- "):
            count += 1
    return count


def sha256_of(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def ensure_clean_dir(path: Path) -> None:
    if path.exists():
        shutil.rmtree(path)
    path.mkdir(parents=True, exist_ok=True)


def resolve_commit_sha() -> str:
    env_value = str(os.environ.get("GITHUB_SHA") or "").strip()
    if env_value:
        return env_value
    try:
        result = subprocess.run(
            ["git", "rev-parse", "HEAD"],
            cwd=ROOT,
            check=True,
            capture_output=True,
            text=True,
        )
        return result.stdout.strip()
    except Exception:
        return ""


def load_summary(summary_file: Path, release_file: Path) -> tuple[dict[str, Any], str]:
    if summary_file.exists():
        return read_json(summary_file), "artifact"
    release_proxy_count = count_release_proxies(release_file)
    return (
        {
            "candidate_source_count": 0,
            "raw_proxy_count": release_proxy_count,
            "unique_proxy_count": release_proxy_count,
            "useful_proxy_count": release_proxy_count,
            "final_release_proxy_count": release_proxy_count,
            "summary_mode": "derived_from_release_file",
        },
        "derived_from_release_file",
    )


def build_index_html(base_url: str, latest: dict[str, Any]) -> str:
    summary = latest.get("summary") or {}
    files = latest.get("files") or {}
    clash = files.get("clash") or {}
    summary_json = files.get("summary") or {}
    registry_json = files.get("source_registry") or {}
    rules = files.get("rules") or {}
    rules_link = (
        f'<li><a href="{rules.get("path", "rules/")}">rules/</a></li>'
        if rules
        else ""
    )
    return f"""<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>autojiedian distribution</title>
  <style>
    :root {{
      color-scheme: light;
      --bg: #f5efe4;
      --paper: #fffaf2;
      --ink: #1f2328;
      --muted: #5d6670;
      --line: rgba(31, 35, 40, 0.12);
      --accent: #0f766e;
    }}
    * {{ box-sizing: border-box; }}
    body {{
      margin: 0;
      font-family: "Segoe UI Variable", "Microsoft YaHei UI", sans-serif;
      color: var(--ink);
      background:
        radial-gradient(circle at top left, rgba(15, 118, 110, 0.12), transparent 28%),
        linear-gradient(180deg, #f7f1e7 0%, #efe6d8 100%);
    }}
    main {{
      width: min(960px, calc(100vw - 32px));
      margin: 24px auto;
      padding: 24px;
      background: var(--paper);
      border: 1px solid var(--line);
      border-radius: 24px;
      box-shadow: 0 16px 40px rgba(74, 52, 34, 0.12);
    }}
    h1 {{
      margin: 0 0 8px;
      font-size: 34px;
    }}
    p, li {{
      color: var(--muted);
      line-height: 1.6;
    }}
    .grid {{
      display: grid;
      grid-template-columns: repeat(3, minmax(0, 1fr));
      gap: 12px;
      margin: 20px 0 24px;
    }}
    .card {{
      padding: 16px;
      border: 1px solid var(--line);
      border-radius: 18px;
      background: rgba(255, 255, 255, 0.72);
    }}
    .label {{
      font-size: 12px;
      color: var(--muted);
      text-transform: uppercase;
      letter-spacing: 0.08em;
    }}
    .value {{
      margin-top: 8px;
      font-size: 28px;
      font-weight: 700;
    }}
    code {{
      padding: 2px 6px;
      border-radius: 999px;
      background: rgba(15, 118, 110, 0.08);
      color: #0b6158;
    }}
    a {{
      color: var(--accent);
      text-decoration: none;
    }}
    a:hover {{
      text-decoration: underline;
    }}
    ul {{
      padding-left: 20px;
    }}
    @media (max-width: 760px) {{
      .grid {{
        grid-template-columns: 1fr;
      }}
      h1 {{
        font-size: 28px;
      }}
    }}
  </style>
</head>
<body>
  <main>
    <h1>autojiedian distribution</h1>
    <p>Controlled distribution layer for the latest generated Clash subscription.</p>
    <div class="grid">
      <div class="card">
        <div class="label">Release Nodes</div>
        <div class="value">{summary.get("final_release_proxy_count", 0)}</div>
      </div>
      <div class="card">
        <div class="label">Unique Candidates</div>
        <div class="value">{summary.get("unique_proxy_count", 0)}</div>
      </div>
      <div class="card">
        <div class="label">Generated At</div>
        <div class="value" style="font-size:18px">{latest.get("generated_at", "-")}</div>
      </div>
    </div>
    <p>Primary source should still prefer <code>raw.githubusercontent.com</code>. This Pages site is the controlled fallback layer.</p>
    <ul>
      <li><a href="{clash.get('path', 'clash.yaml')}">clash.yaml</a></li>
      <li><a href="{summary_json.get('path', 'summary.json')}">summary.json</a></li>
      <li><a href="{registry_json.get('path', 'source-registry.json')}">source-registry.json</a></li>
      {rules_link}
      <li><a href="latest.json">latest.json</a></li>
    </ul>
    <p>Base URL: <code>{base_url or '-'}</code></p>
    <p>Commit: <code>{latest.get("commit", "-")}</code></p>
  </main>
</body>
</html>
"""


def main() -> int:
    args = parse_args()
    output_dir = (ROOT / args.output_dir).resolve()
    release_file = (ROOT / args.release_file).resolve()
    summary_file = (ROOT / args.summary_file).resolve()
    registry_file = (ROOT / args.registry_file).resolve()
    fallback_registry_file = (ROOT / args.fallback_registry_file).resolve()
    if not release_file.exists():
        raise FileNotFoundError(f"release file not found: {release_file}")

    source_registry_path = registry_file if registry_file.exists() else fallback_registry_file
    rules_dir = (ROOT / "rules").resolve()
    summary, summary_source = load_summary(summary_file, release_file)
    ensure_clean_dir(output_dir)

    shutil.copy2(release_file, output_dir / "clash.yaml")
    summary_output_path = output_dir / "summary.json"
    summary_output_path.write_text(
        json.dumps(summary, ensure_ascii=False, indent=2),
        encoding="utf-8",
    )
    if source_registry_path.exists():
        shutil.copy2(source_registry_path, output_dir / "source-registry.json")
    if rules_dir.exists() and rules_dir.is_dir():
        shutil.copytree(rules_dir, output_dir / "rules", dirs_exist_ok=True)

    base_url = str(args.base_url or "").rstrip("/")
    latest = {
        "generated_at": datetime.now(timezone.utc).isoformat(),
        "commit": resolve_commit_sha(),
        "base_url": base_url,
        "summary_source": summary_source,
        "files": {
            "clash": {
                "path": "clash.yaml",
                "url": f"{base_url}/clash.yaml" if base_url else "clash.yaml",
                "size": release_file.stat().st_size,
                "sha256": sha256_of(release_file),
            },
            "summary": {
                "path": "summary.json",
                "url": f"{base_url}/summary.json" if base_url else "summary.json",
                "size": summary_output_path.stat().st_size,
            },
        },
        "summary": summary,
    }
    if source_registry_path.exists():
        latest["files"]["source_registry"] = {
            "path": "source-registry.json",
            "url": f"{base_url}/source-registry.json" if base_url else "source-registry.json",
            "size": source_registry_path.stat().st_size,
        }
    if rules_dir.exists() and rules_dir.is_dir():
        latest["files"]["rules"] = {
            "path": "rules/",
            "url": f"{base_url}/rules/" if base_url else "rules/",
        }

    (output_dir / "latest.json").write_text(
        json.dumps(latest, ensure_ascii=False, indent=2),
        encoding="utf-8",
    )
    (output_dir / "index.html").write_text(
        build_index_html(base_url, latest),
        encoding="utf-8",
    )
    (output_dir / ".nojekyll").write_text("", encoding="utf-8")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
