#!/usr/bin/env python3
"""Validate one explicitly supplied provider optimization stage without credentials."""

import json
import os
from pathlib import Path
import subprocess
import sys
import tarfile


def git(source, *args):
    return subprocess.check_output(["git", *args], cwd=source)


def main():
    source = Path(sys.argv[1]).resolve()
    stage = json.loads(Path(sys.argv[2]).read_text())
    result = Path(sys.argv[3]).resolve()
    result.mkdir(parents=True, exist_ok=True)
    base = git(source, "rev-parse", "HEAD").decode().strip()
    if base != stage["base"] or git(source, "status", "--porcelain"):
        raise SystemExit("Candidate checkout does not match its clean base")
    (result / "base.sha").write_text(base + "\n")
    (result / "base.commit").write_bytes(git(source, "cat-file", "commit", "HEAD"))
    patch = result / "input.patch"
    patch.write_text(stage["patch"])
    subprocess.run(["git", "apply", "--index", "--check", str(patch)], cwd=source, check=True)
    subprocess.run(["git", "apply", "--index", str(patch)], cwd=source, check=True)
    expected = set(git(source, "diff", "--cached", "--name-only").decode().splitlines())
    commands = []

    def run(args):
        if not args or args[0] != "just" or args[1] not in {
            "test", "fix", "fmt", "write-config-schema", "write-app-server-schema"
        }:
            raise ValueError("Only repository validation commands are permitted")
        if args[1] == "test" and "-p" not in args:
            raise ValueError("Full workspace tests require separate authorization")
        print("Running:", args, flush=True)
        log = result / f"command-{len(commands) + 1}.log"
        with log.open("w") as output:
            process = subprocess.Popen(args, cwd=source / "codex-rs", stdout=subprocess.PIPE,
                                       stderr=subprocess.STDOUT, text=True)
            for line in process.stdout:
                print(line, end="", flush=True)
                output.write(line)
            returncode = process.wait()
        commands.append({"command": args, "exit_code": returncode, "log": log.name})
        return returncode == 0

    ok = True
    for command in stage.get("prepare", []):
        ok = run(command) and ok
    if ok:
        for command in stage["tests"]:
            ok = run(command) and ok
    if ok and stage.get("lint_packages"):
        args = ["just", "fix"]
        for package in stage["lint_packages"]:
            args.extend(["-p", package])
        ok = run(args) and ok
    # AGENTS.md requires formatting after modifications, and no test rerun after fix/fmt.
    ok = run(["just", "fmt"]) and ok
    subprocess.run(["git", "add", "--all"], cwd=source, check=True)
    changed = set(git(source, "diff", "--cached", "--name-only").decode().splitlines())
    extra = stage.get("generated_paths", [])
    unexpected = sorted(p for p in changed - expected if not any(
        p == prefix or p.startswith(prefix.rstrip("/") + "/") for prefix in extra
    ))
    if unexpected:
        print("Unexpected paths:", unexpected, flush=True)
        ok = False
    whitespace = subprocess.run(["git", "diff", "--cached", "--check"], cwd=source)
    ok = ok and whitespace.returncode == 0
    (result / "candidate.tree").write_bytes(git(source, "write-tree"))
    (result / "validated.patch").write_bytes(git(source, "diff", "--cached", "--binary", "HEAD"))
    (result / "commit-message.txt").write_text(stage["message"] + "\n")
    report = {"base": base, "stage": stage["id"], "commands": commands,
              "unexpected_paths": unexpected, "ready": ok}
    (result / "report.json").write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n")
    with tarfile.open(result / "candidate-source.tar.gz", "w:gz") as archive:
        for entry in git(source, "ls-files", "-z").split(b"\0"):
            if entry:
                path = os.fsdecode(entry)
                if (source / path).exists() or (source / path).is_symlink():
                    archive.add(source / path, arcname=path, recursive=False)
    if ok:
        with open(os.environ["GITHUB_OUTPUT"], "a") as output:
            output.write("ready=true\n")
    return 0 if ok else 1


if __name__ == "__main__":
    raise SystemExit(main())
