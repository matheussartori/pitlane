#!/usr/bin/env python3
"""Phase 0 benchmark: does Pitlane's map ("Where to look") save tokens without hurting results?

Replay: for each commit of a test repository, go back to its parent and ask Claude Code for what the
commit did (its subject is the card). Each task runs twice, with and without the map, side by side
in two worktrees of the parent. The map is compiled from an index of the parent, so it cannot leak
the answer. Then a separate judge session compares each attempt with the real commit.

Uses the official `claude` CLI only. Never point it at a real project: worktrees and runs are made in
the repository you give it.

  python3 tools/bench/replay.py --repo <clone> --out <dir> --model sonnet <sha> [<sha> ...]
"""

import argparse
import concurrent.futures as cf
import json
import os
import re
import subprocess
import sys
import time
from pathlib import Path

RULES = (
    "You are working on one task in a git worktree made for it. Work only inside the current directory. "
    "Make the change the task asks for, the way this project would, with a test when the project tests that "
    "area. Do not commit, push or switch branches. When you are done, say in two sentences what you changed."
)
TOOLS = "Read,Edit,Write,Glob,Grep,Bash(git diff:*),Bash(git status:*),Bash(git log:*),Bash(ls:*)"
VERDICT = {
    "type": "object",
    "properties": {
        "verdict": {"type": "string", "enum": ["pass", "fail"]},
        "reason": {"type": "string"},
    },
    "required": ["verdict", "reason"],
}


def sh(args, cwd=None, input=None, timeout=None, check=True):
    p = subprocess.run(args, cwd=cwd, input=input, capture_output=True, text=True, timeout=timeout)
    if check and p.returncode != 0:
        raise RuntimeError(f"{' '.join(args[:3])}… failed: {p.stderr.strip()[:400]}")
    return p.stdout


def task_title(subject):
    return re.sub(r"\s*\(#\d+\)\s*$", "", subject).strip()


def code_files(names):
    return sorted(n for n in names if n.startswith("src/") and n.endswith((".ts", ".tsx")) and ".test." not in n)


def run_claude(cwd, prompt, model, max_turns, extra=(), timeout=1500):
    t = time.time()
    try:
        out = sh(
            ["claude", "-p", prompt, "--output-format", "json", "--model", model, "--max-turns", str(max_turns), *extra],
            cwd=cwd, timeout=timeout, check=False,
        )
        data = json.loads(out.strip().splitlines()[-1]) if out.strip() else {}
    except subprocess.TimeoutExpired:
        data = {"is_error": True, "result": "timeout"}
    except json.JSONDecodeError:
        data = {"is_error": True, "result": "unreadable output"}
    data["_wall_s"] = round(time.time() - t, 1)
    return data


def attempt(repo, out, sha, parent, title, brief, variant, model):
    wt = out / "wt" / f"{sha}-{variant}"
    if wt.exists():
        sh(["git", "worktree", "remove", "--force", str(wt)], cwd=repo, check=False)
    sh(["git", "worktree", "add", "-q", "--detach", str(wt), parent], cwd=repo)
    prompt = f"# Task: {title}\n\n(No spec beyond the title. Keep the change focused.)\n"
    if variant == "map" and brief:
        prompt += "\n" + brief + "\n"
    prompt += "\nWhen you finish, say what you changed.\n"
    r = run_claude(
        wt, prompt, model, 40,
        extra=["--permission-mode", "acceptEdits", "--allowedTools", TOOLS, "--setting-sources", "project,local", "--append-system-prompt", RULES],
    )
    sh(["git", "add", "-A", "-N"], cwd=wt, check=False)
    diff = sh(["git", "diff", "--no-color"], cwd=wt, check=False)
    names = sh(["git", "diff", "--name-only"], cwd=wt, check=False).split()
    u = r.get("usage") or {}
    return {
        "variant": variant,
        "error": r.get("result") if r.get("is_error") else None,
        "cost_usd": r.get("total_cost_usd"),
        "turns": r.get("num_turns"),
        "wall_s": r["_wall_s"],
        "tokens_in": (u.get("input_tokens") or 0) + (u.get("cache_creation_input_tokens") or 0) + (u.get("cache_read_input_tokens") or 0),
        "tokens_cached": u.get("cache_read_input_tokens") or 0,
        "tokens_out": u.get("output_tokens") or 0,
        "files": names,
        "diff": diff,
        "brief_chars": len(brief) if variant == "map" else 0,
    }


def judge(out, title, real_diff, cand_diff, model):
    jdir = out / "judge"
    jdir.mkdir(parents=True, exist_ok=True)
    if not cand_diff.strip():
        return {"verdict": "fail", "reason": "no change was made"}
    prompt = (
        "You compare a candidate change with the reference commit that solved the same task. Pass when the "
        "candidate makes the same behavioural fix or feature (it may be written differently, touch other "
        "lines, or add different tests); fail when it misses the point, is incomplete, or breaks something "
        "the reference keeps working. Judge behaviour, not style.\n\n"
        f"## Task\n{title}\n\n## Reference commit\n```diff\n{real_diff[:60000]}\n```\n\n"
        f"## Candidate\n```diff\n{cand_diff[:60000]}\n```\n"
    )
    r = run_claude(jdir, prompt, model, 3, extra=["--json-schema", json.dumps(VERDICT)], timeout=600)
    v = r.get("structured_output") or {}
    return {"verdict": v.get("verdict", "fail"), "reason": v.get("reason", r.get("result", "no verdict")), "cost_usd": r.get("total_cost_usd")}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--repo", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--model", default="sonnet")
    ap.add_argument("--judge-model", default="sonnet")
    ap.add_argument("--parallel", type=int, default=2)
    ap.add_argument("--brief-budget", type=int, default=12000, help="map size in characters; 4000 or less is the lean map")
    ap.add_argument("--only", choices=["map", "bare"], help="run one variant (reuse the other from an earlier run)")
    ap.add_argument("commits", nargs="+")
    a = ap.parse_args()
    repo, out = Path(a.repo).resolve(), Path(a.out).resolve()
    out.mkdir(parents=True, exist_ok=True)
    start_ref = sh(["git", "rev-parse", "HEAD"], cwd=repo).strip()

    # 1. Tasks and maps, one at a time: the index must be of each commit's parent.
    tasks = []
    for sha in a.commits:
        subject = sh(["git", "log", "-1", "--format=%s", sha], cwd=repo).strip()
        parent = sh(["git", "rev-parse", f"{sha}^"], cwd=repo).strip()
        real_diff = sh(["git", "show", "--no-color", "--format=", sha], cwd=repo)
        real_files = sh(["git", "show", "--name-only", "--format=", sha], cwd=repo).split()
        sh(["git", "checkout", "-q", "--detach", parent], cwd=repo)
        title = task_title(subject)
        brief = sh(["pit", "brief", "--text", title, "--budget", str(a.brief_budget)], cwd=repo, check=False)
        brief = "" if brief.startswith("nothing in the index") else brief.strip()
        tasks.append({"sha": sha, "parent": parent, "title": title, "real_files": real_files, "real_diff": real_diff, "brief": brief})
        print(f"prepared {sha} {title} (map {len(brief)} chars)", flush=True)
    sh(["git", "checkout", "-q", "--detach", start_ref], cwd=repo)

    # 2. Both variants of a task side by side, so they meet the same cache and limits.
    results = []
    with cf.ThreadPoolExecutor(max_workers=a.parallel) as pool:
        for t in tasks:
            variants = (a.only,) if a.only else ("map", "bare")
            futs = {pool.submit(attempt, repo, out, t["sha"], t["parent"], t["title"], t["brief"], v, a.model): v for v in variants}
            runs = {futs[f]: f.result() for f in cf.as_completed(futs)}
            for v in variants:
                r = runs[v]
                r["judge"] = judge(out, t["title"], t["real_diff"], r["diff"], a.judge_model)
                want = set(code_files(t["real_files"]))
                got = set(code_files(r["files"]))
                r["file_recall"] = round(len(want & got) / len(want), 2) if want else None
                r["extra_files"] = sorted(got - want)
            results.append({k: t[k] for k in ("sha", "title", "real_files")} | {"map_chars": len(t["brief"]), "runs": runs})
            print(
                t["sha"] + " " + " | ".join(f"{v}: {r['judge']['verdict']} ${r['cost_usd'] or 0:.2f} {r['tokens_in']//1000}k in {r['turns']}t" for v, r in runs.items()),
                flush=True,
            )
            (out / "results.json").write_text(json.dumps(results, indent=1))

    # 3. Summary.
    def agg(v):
        rs = [r["runs"][v] for r in results]
        n = len(rs)
        return {
            "passed": sum(r["judge"]["verdict"] == "pass" for r in rs),
            "of": n,
            "cost_usd": round(sum(r["cost_usd"] or 0 for r in rs), 3),
            "tokens_in": sum(r["tokens_in"] for r in rs),
            "tokens_out": sum(r["tokens_out"] for r in rs),
            "turns": sum(r["turns"] or 0 for r in rs),
            "wall_s": round(sum(r["wall_s"] for r in rs)),
            "file_recall": round(sum(r["file_recall"] or 0 for r in rs) / max(n, 1), 2),
        }
    summary = {v: agg(v) for v in ((a.only,) if a.only else ("map", "bare"))} | {"model": a.model, "tasks": len(results), "brief_budget": a.brief_budget}
    (out / "summary.json").write_text(json.dumps(summary, indent=1))
    print(json.dumps(summary, indent=1))
    # Clean up the worktrees; the repository is left as it was.
    for t in tasks:
        for v in ((a.only,) if a.only else ("map", "bare")):
            sh(["git", "worktree", "remove", "--force", str(out / "wt" / f"{t['sha']}-{v}")], cwd=repo, check=False)


if __name__ == "__main__":
    sys.exit(main())
